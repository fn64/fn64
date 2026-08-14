//! Equivalence guard for the per-call IMEM predecode table in
//! `rsp::interpreter::run_imem`.
//!
//! The interpreter used to call `decode(word, pc)` twice per retired step
//! (instruction + delay slot). It now builds one table per `run_imem` call:
//! `table[i] = decode(words[i], 0x1000 + 4*i)`. These tests pin the exact
//! claim that substitution relies on:
//!
//! 1. On a real 1024-word IMEM image (the AKI audio ucode text and its
//!    trailing ROM bytes), the table entry at every index equals the per-step
//!    decode expression the old loop used, including the delay-slot pairing
//!    `decode(words[i+1], pc.wrapping_add(4))`.
//! 2. The same holds on a synthetic image that places every decode class —
//!    including branch/link forms with extreme immediates — at the 0x1FFC
//!    wrap boundary.
//! 3. The pc-window lemma: `decode(word, pc)` depends only on
//!    `(word, pc & 0x0fff)`. This is the invariant that makes
//!    `0x1000 + 4*i` the correct table key; a future `decode` edit that uses
//!    the raw pc breaks this test instead of the emulation.
//! 4. Behavioral: hand-written programs through `run_imem` covering
//!    taken/untaken branches with live delay slots, jal/jr return, the
//!    sequential 0x1FFC -> 0x1000 wrap, an mtc0 overlay-swap in a delay slot
//!    (resume_address write), and a second call re-entering mid-IMEM via
//!    `ctx.resume_address`. Expected constants were derived from the
//!    pre-change per-step-decode semantics.

use fn64_audio::rsp::decode::decode;
use fn64_audio::rsp::interpreter::{predecode_imem, run_imem};
use fn64_audio::rsp::runtime::RspMachine;
use fn64_audio::rsp::RspExitReason;

/// The AKI audio ucode text sits at this ROM offset (see
/// `tests/aki_ucode_coverage_probe.rs`); reading a full 4096 bytes from its
/// head yields 1024 words of real instruction-stream bytes.
const AKI_UCODE_ROM_OFF: usize = 0x39510;
const IMEM_WORDS: usize = 1024;

/// Assert the predecode table reproduces both per-step decode expressions the
/// old loop used: `decode(words[i], pc_i)` for the instruction and
/// `decode(words[i+1], pc_i.wrapping_add(4))` for its delay slot.
fn assert_table_matches_per_step_decode(words: &[u32]) {
    let table = predecode_imem(words);
    assert_eq!(table.len(), words.len());
    for (i, &word) in words.iter().enumerate() {
        let pc = 0x1000 + 4 * i as u32;
        assert_eq!(
            table[i],
            decode(word, pc),
            "instruction decode diverged at index {i} (pc {pc:#06x}, word {word:#010x})"
        );
    }
    for i in 0..words.len().saturating_sub(1) {
        let pc = 0x1000 + 4 * i as u32;
        assert_eq!(
            table[i + 1],
            decode(words[i + 1], pc.wrapping_add(4)),
            "delay-slot decode diverged at index {} (branch pc {pc:#06x})",
            i + 1
        );
    }
}

/// Deterministic PRNG for the synthetic image and the lemma pcs; plain LCG,
/// no new dependencies (Knuth MMIX constants).
fn lcg(state: &mut u64) -> u32 {
    *state = state
        .wrapping_mul(6364136223846793005)
        .wrapping_add(1442695040888963407);
    (*state >> 32) as u32
}

#[test]
fn real_aki_imem_predecode_matches_per_step_decode() {
    let Some(rom_path) = std::env::var_os("FN64_WM2000_ROM") else {
        eprintln!(
            "SKIP real_aki_imem_predecode_matches_per_step_decode: set FN64_WM2000_ROM \
             to the WM2000 .z64 to run the real-IMEM predecode equivalence guard."
        );
        return;
    };
    let rom = std::fs::read(&rom_path)
        .unwrap_or_else(|e| panic!("reading {}: {e}", rom_path.to_string_lossy()));
    let words: Vec<u32> = rom[AKI_UCODE_ROM_OFF..AKI_UCODE_ROM_OFF + 4 * IMEM_WORDS]
        .chunks_exact(4)
        .map(|c| u32::from_be_bytes(c.try_into().unwrap()))
        .collect();
    assert_eq!(words.len(), IMEM_WORDS);
    assert_table_matches_per_step_decode(&words);
}

#[test]
fn synthetic_full_imem_predecode_matches_per_step_decode() {
    let mut state = 0x5eed_5eed_5eed_5eedu64;
    let mut words: Vec<u32> = (0..IMEM_WORDS).map(|_| lcg(&mut state)).collect();

    // Systematic sweep: one representative of every decode class, placed low
    // in the image...
    let classes = [
        (0x09u32 << 26) | (1 << 21) | (2 << 16) | 0x8000,           // addiu (imm sign edge)
        (0x0Fu32 << 26) | (3 << 16) | 0xFFFF,                       // lui
        (0x23u32 << 26) | (4 << 21) | (5 << 16) | 0x7FFF,           // lw
        (0x2Bu32 << 26) | (4 << 21) | (5 << 16) | 0x8000,           // sw
        (6 << 21) | (7 << 16) | (8 << 11) | 0x21,                   // addu (SPECIAL)
        (9 << 16) | (10 << 11) | (3 << 6),                          // sll
        (0x04u32 << 26) | (1 << 21) | (2 << 16) | 0x7FFF,           // beq, max fwd
        (0x05u32 << 26) | (1 << 21) | (2 << 16) | 0x8000,           // bne, max back
        (0x06u32 << 26) | (3 << 21) | 0x0123,                       // blez
        (0x07u32 << 26) | (3 << 21) | 0xFFF0,                       // bgtz
        (0x01u32 << 26) | (4 << 21) | (0x00 << 16) | 1,             // bltz
        (0x01u32 << 26) | (4 << 21) | (0x01 << 16) | 0xFFFF,        // bgez
        (0x01u32 << 26) | (5 << 21) | (0x10 << 16) | 0x4000,        // bltzal (link)
        (0x01u32 << 26) | (5 << 21) | (0x11 << 16) | 0xC000,        // bgezal (link)
        (0x02u32 << 26) | 0x03FF_FFFF,                              // j, max target26
        (0x03u32 << 26) | 0x0000_0410,                              // jal
        (6 << 21) | 0x08,                                           // jr
        (7 << 21) | (8 << 11) | 0x09,                               // jalr
        0x0000_000D,                                                // break
        (0x10u32 << 26) | (0x00 << 21) | (9 << 16) | (5 << 11),     // mfc0
        (0x10u32 << 26) | (0x04 << 21) | (9 << 16) | (2 << 11),     // mtc0
        (0x12u32 << 26) | (0x00 << 21) | (10 << 16) | (11 << 11) | (4 << 7), // mfc2
        (0x12u32 << 26) | (0x04 << 21) | (10 << 16) | (11 << 11) | (4 << 7), // mtc2
        (0x12u32 << 26) | (0x02 << 21) | (12 << 16) | (1 << 11),    // cfc2
        (0x12u32 << 26) | (0x06 << 21) | (12 << 16) | (2 << 11),    // ctc2
        (0x12u32 << 26) | (1 << 25) | (3 << 21) | (8 << 16) | (9 << 11) | (10 << 6) | 0x06, // vmudn
        (0x32u32 << 26) | (2 << 21) | (4 << 16) | (0x04 << 11) | (6 << 7) | 0x7F, // lqv
        (0x3Au32 << 26) | (3 << 21) | (5 << 16) | (0x04 << 11) | (6 << 7) | 0x01, // sqv
        0xFC00_0000,                                                // unknown opcode
        0,                                                          // nop
    ];
    words[..classes.len()].copy_from_slice(&classes);

    // ...and pc-sensitive branch/link forms with extreme immediates at the
    // 0x1FF0..0x1FFC tail, where branch_target/link_address must mask the
    // 0x2000 overflow back into the IMEM window.
    words[1020] = (0x04u32 << 26) | (1 << 21) | (2 << 16) | 0x7FFF; // beq at 0x1FF0
    words[1021] = (0x01u32 << 26) | (3 << 21) | (0x10 << 16) | 0x8000; // bltzal at 0x1FF4
    words[1022] = (0x03u32 << 26) | 0x03FF_FFFF; // jal at 0x1FF8 (link 0x2000 -> 0x000)
    words[1023] = (4 << 21) | (5 << 11) | 0x09; // jalr at 0x1FFC (link 0x2004 -> 0x004)

    assert_table_matches_per_step_decode(&words);
}

#[test]
fn decode_depends_only_on_pc_low_twelve_bits() {
    // The pc-window lemma the table key relies on: pc feeds decode() only
    // through branch_target() and link_address(), both of which mask through
    // & 0x0FFF after an add, so decode(word, pc) == decode(word, 0x1000 |
    // (pc & 0x0FFF)) for every word-aligned pc.
    let pc_sensitive_words = [
        (0x04u32 << 26) | (1 << 21) | (2 << 16) | 0x8000,    // beq, most-negative imm
        (0x05u32 << 26) | (1 << 21) | (2 << 16) | 0x7FFF,    // bne, most-positive imm
        (0x06u32 << 26) | (3 << 21) | 0x0001,                // blez
        (0x07u32 << 26) | (3 << 21) | 0xFFFF,                // bgtz
        (0x01u32 << 26) | (4 << 21) | (0x00 << 16) | 0x0100, // bltz
        (0x01u32 << 26) | (4 << 21) | (0x01 << 16) | 0xFF00, // bgez
        (0x01u32 << 26) | (5 << 21) | (0x10 << 16) | 0x8000, // bltzal
        (0x01u32 << 26) | (5 << 21) | (0x11 << 16) | 0x7FFF, // bgezal
        (0x02u32 << 26) | 0x03FF_FFFF,                       // j
        (0x03u32 << 26) | 0x0000_0410,                       // jal
        (7 << 21) | (8 << 11) | 0x09,                        // jalr
        (6 << 21) | 0x08,                                    // jr (pc-independent)
        (0x09u32 << 26) | (1 << 21) | (2 << 16) | 5,         // addiu (pc-independent)
    ];
    let mut state = 0x0123_4567_89ab_cdefu64;
    for word in pc_sensitive_words {
        for _ in 0..1000 {
            let pc = lcg(&mut state) & !3;
            assert_eq!(
                decode(word, pc),
                decode(word, 0x1000 | (pc & 0x0FFF)),
                "decode used pc bits outside the 12-bit IMEM window: word {word:#010x}, \
                 pc {pc:#010x}"
            );
        }
    }
}

// --- instruction-word builders for the behavioral programs ---

fn addiu(rt: u32, rs: u32, imm: u32) -> u32 {
    (0x09 << 26) | (rs << 21) | (rt << 16) | (imm & 0xFFFF)
}
fn beq(rs: u32, rt: u32, imm: u32) -> u32 {
    (0x04 << 26) | (rs << 21) | (rt << 16) | (imm & 0xFFFF)
}
fn bne(rs: u32, rt: u32, imm: u32) -> u32 {
    (0x05 << 26) | (rs << 21) | (rt << 16) | (imm & 0xFFFF)
}
fn jal(word_index: u32) -> u32 {
    (0x03 << 26) | ((0x1000 + 4 * word_index) >> 2)
}
fn jr(rs: u32) -> u32 {
    (rs << 21) | 0x08
}
fn mtc0(cop0: u32, rt: u32) -> u32 {
    (0x10 << 26) | (0x04 << 21) | (rt << 16) | (cop0 << 11)
}
const BREAK: u32 = 0x0000_000D;

#[test]
fn behavioral_branch_delay_and_link_flow_matches_pre_change_constants() {
    // Program (idx: pc):
    //   0: 0x1000 addiu r1, r0, 5
    //   1: 0x1004 beq   r0, r0, +2      taken -> 0x1010
    //   2: 0x1008   (delay) addiu r1, r1, 10   -> r1 = 15
    //   3: 0x100c addiu r1, r1, 100     skipped by the branch
    //   4: 0x1010 bne   r0, r0, +5      never taken -> fall to 0x1018
    //   5: 0x1014   (delay) addiu r2, r0, 7    delay runs even untaken
    //   6: 0x1018 jal   0x1040          r31 = (0x1018+8) & 0xfff = 0x020
    //   7: 0x101c   (delay) addiu r3, r0, 1
    //   8: 0x1020 break
    //  16: 0x1040 addiu r4, r0, 9
    //  17: 0x1044 jr    r31             -> 0x1020
    //  18: 0x1048   (delay) addiu r4, r4, 1    -> r4 = 10
    let mut words = vec![0u32; 32];
    words[0] = addiu(1, 0, 5);
    words[1] = beq(0, 0, 2);
    words[2] = addiu(1, 1, 10);
    words[3] = addiu(1, 1, 100);
    words[4] = bne(0, 0, 5);
    words[5] = addiu(2, 0, 7);
    words[6] = jal(16);
    words[7] = addiu(3, 0, 1);
    words[8] = BREAK;
    words[16] = addiu(4, 0, 9);
    words[17] = jr(31);
    words[18] = addiu(4, 4, 1);

    let mut rdram = vec![0u8; 0x100];
    let mut machine = RspMachine::new(&mut rdram);
    let result = run_imem(&words, 0, &mut machine, 100);

    // Constants from the pre-change per-step-decode interpreter semantics.
    assert_eq!(result.reason, RspExitReason::Broke);
    assert_eq!(result.pc, 0x1020);
    assert_eq!(result.steps, 7);
    assert_eq!(machine.reg(1), 15);
    assert_eq!(machine.reg(2), 7);
    assert_eq!(machine.reg(3), 1);
    assert_eq!(machine.reg(4), 10);
    assert_eq!(machine.reg(31), 0x020);
}

#[test]
fn behavioral_sequential_wrap_from_1ffc_to_1000() {
    // A full image: the instruction at 0x1FFC has no delay word (both the old
    // `words.get(idx + 1)` and the new `decoded.get(idx + 1)` are None), and
    // sequential flow wraps to 0x1000.
    let mut words = vec![0u32; IMEM_WORDS];
    words[1023] = addiu(5, 0, 3);
    words[0] = BREAK;

    let mut rdram = vec![0u8; 0x100];
    let mut machine = RspMachine::new(&mut rdram);
    let result = run_imem(&words, 0xFFC, &mut machine, 10);

    assert_eq!(result.reason, RspExitReason::Broke);
    assert_eq!(result.pc, 0x1000);
    assert_eq!(result.steps, 2);
    assert_eq!(machine.reg(5), 3);
}

#[test]
fn behavioral_mtc0_swap_in_delay_slot_then_resume_mid_imem() {
    // An overlay-swap DMA (mtc0 c2 with an IMEM destination) issued from a
    // taken branch's delay slot: run_delay writes ctx.resume_address, the
    // call exits SwapOverlay, and the next run_imem call must enter mid-IMEM
    // at the recorded resume pc.
    //
    //   0: 0x1000 addiu r8, r0, 0x1000     IMEM destination bit
    //   1: 0x1004 mtc0  c0, r8             SP_MEM_ADDR = 0x1000
    //   2: 0x1008 mtc0  c1, r0             SP_DRAM_ADDR = 0
    //   3: 0x100c beq   r0, r0, +2         taken -> resume 0x1018
    //   4: 0x1010   (delay) mtc0 c2, r0    8-byte read DMA into IMEM -> swap
    //   6: 0x1018 addiu r9, r0, 42         resume target
    //   7: 0x101c break
    let mut words = vec![0u32; 16];
    words[0] = addiu(8, 0, 0x1000);
    words[1] = mtc0(0, 8);
    words[2] = mtc0(1, 0);
    words[3] = beq(0, 0, 2);
    words[4] = mtc0(2, 0);
    words[6] = addiu(9, 0, 42);
    words[7] = BREAK;

    let mut rdram = vec![0u8; 0x100];
    let mut machine = RspMachine::new(&mut rdram);

    let first = run_imem(&words, 0, &mut machine, 100);
    assert_eq!(first.reason, RspExitReason::SwapOverlay);
    assert_eq!(first.pc, 0x1018);
    assert_eq!(first.steps, 4);
    assert_eq!(machine.ctx.resume_address, 0x1018);

    // Second entry: pc argument 0 is ignored because resume_address is set;
    // the table covers every index, so a mid-IMEM start is just a nonzero
    // starting idx.
    let second = run_imem(&words, 0, &mut machine, 100);
    assert_eq!(second.reason, RspExitReason::Broke);
    assert_eq!(second.pc, 0x101c);
    assert_eq!(second.steps, 2);
    assert_eq!(machine.reg(9), 42);
    assert_eq!(machine.ctx.resume_address, 0);
}
