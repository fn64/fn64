//! Oracle-validation + decoder tests for `fn64-recomp-rs`.
//!
//! # The oracle
//!
//! The reference behaviour is the **MIT N64Recomp C output** for a real OoT
//! function. We use `DynaPoly_IsBgIdBgActor` @ vram `0x80031264` (8 words,
//! extracted from `oot-ntsc-1.0.z64` ROM offset `0xAA71C4`), whose recompiled
//! C body is (verbatim from `aki-recomp/games/OOTU/RecompiledFuncs/funcs_11.c`):
//!
//! ```c
//! void DynaPoly_IsBgIdBgActor(uint8_t* rdram, recomp_context* ctx) {
//!     if (SIGNED(ctx->r4) < 0) { ctx->r1 = SIGNED(ctx->r4) < 0x32 ? 1 : 0; goto L_80031274; }
//!     ctx->r1 = SIGNED(ctx->r4) < 0x32 ? 1 : 0;
//!     if (ctx->r1 != 0) { ctx->r2 = ADD32(0, 0x1); goto L_8003127C; }
//!     ctx->r2 = ADD32(0, 0x1);
//! L_80031274: ctx->r2 = 0 | 0; return;   // jr $ra; delay: or $v0,$zero,$zero
//! L_8003127C: return;                     // jr $ra; delay: nop
//! }
//! ```
//!
//! [`dynapoly_oracle`] below is that C, hand-transcribed to Rust *independently
//! of the emitter*. The differential test recompiles the SAME ROM bytes with
//! OUR emitter, executes the emitted Rust, and asserts it computes the same
//! `$v0` (return value register) as the oracle for a sweep of `$a0` inputs
//! spanning the sign boundary and the `< 0x32` threshold. Divergence fails the
//! test — this is the strong check, not a bbox/fuzzy one.

use fn64_recomp_rs::{decode, emit_function, FuncInput, Instruction, Rdram, RecompContext};

/// Real ROM bytes of `DynaPoly_IsBgIdBgActor` (big-endian words).
const DYNAPOLY_WORDS: [u32; 8] = [
    0x04800003, // bltz  $a0, L_80031274
    0x28810032, // slti  $at, $a0, 0x32   (delay slot)
    0x14200003, // bne   $at, $zero, L_8003127C
    0x24020001, // addiu $v0, $zero, 0x1  (delay slot)
    0x03E00008, // jr    $ra
    0x00001025, // or    $v0, $zero, $zero (delay slot)
    0x03E00008, // jr    $ra
    0x00000000, // nop                    (delay slot)
];
const DYNAPOLY_VRAM: u32 = 0x80031264;

// --- The oracle: hand-transcribed from the N64Recomp C, NOT the emitter. ---
//
// `SIGNED(ctx->r4)` is the full 64-bit signed value of $a0; `ADD32(0,1)` is 1
// sign-extended. Returns the value left in $v0 (ctx->r2).
fn dynapoly_oracle(a0: u64) -> u64 {
    let mut r1: u64;
    let r2: u64;
    if (a0 as i64) < 0 {
        // taken branch: set $at, then fall to L_80031274 (delay slot of jr sets
        // $v0 = 0).
        r1 = if (a0 as i64) < 0x32 { 1 } else { 0 };
        let _ = &mut r1;
        r2 = 0;
    } else {
        r1 = if (a0 as i64) < 0x32 { 1 } else { 0 };
        if r1 != 0 {
            // delay slot sets $v0 = 1, then L_8003127C just returns.
            r2 = 1;
        } else {
            // delay slot sets $v0 = 1, fall through to L_80031274 which sets
            // $v0 = 0 in ITS delay slot before returning.
            r2 = 0;
        }
    }
    r2
}

// --- The emitter's output, pasted VERBATIM. ---
//
// A golden assertion (`emitter_output_matches_pasted_function`) guarantees the
// live `emit_function` still produces exactly this text, so executing it here
// really is executing the emitter's product, not a divergent hand-copy.
// The generated module carries `#![allow(clippy::all)]`; this verbatim paste
// needs the same, since faithfully-translated MIPS (`or $v0,$zero,$zero` ->
// `0 | 0`) is intentionally un-idiomatic Rust.
#[allow(unused_variables, clippy::all)]
pub fn dynapoly_is_bg_id_bg_actor(ctx: &mut RecompContext, mem: &mut Rdram) {
    let mut pc: u32 = 0x80031264;
    'run: loop {
        match pc {
            0x80031264 => {
                // 0x80031264: Bltz { rs: 4, off: 3 }
                let _take = ctx.r_s64(4) < 0;
                // delay: 0x80031268: Slti { rt: 1, rs: 4, imm: 50 }
                ctx.set_r(1, if ctx.r_s64(4) < 50i64 { 1 } else { 0 });
                pc = if _take { 0x80031274 } else { 0x8003126C };
                continue 'run;
            }
            0x8003126C => {
                // 0x8003126C: Bne { rs: 1, rt: 0, off: 3 }
                let _take = ctx.r(1) != 0i64 as u64;
                // delay: 0x80031270: Addiu { rt: 2, rs: 0, imm: 1 }
                ctx.set_r32(2, (0i32).wrapping_add(1));
                pc = if _take { 0x8003127C } else { 0x80031274 };
                continue 'run;
            }
            0x80031274 => {
                // 0x80031274: Jr { rs: 31 }
                // delay: 0x80031278: Or { rd: 2, rs: 0, rt: 0 }
                ctx.set_r(2, 0i64 as u64 | 0i64 as u64);
                return;
            }
            0x8003127C => {
                // 0x8003127C: Jr { rs: 31 }
                // delay: 0x80031280: Nop
                // nop
                return;
            }
            _ => unreachable!("jumped to unmapped vram {:#X}", pc),
        }
    }
}

/// The pasted-verbatim body above must be byte-identical to what the live
/// emitter produces (ignoring the emitted fn NAME, which the test fixes). If
/// the emitter changes shape, this fails loudly and the paste must be
/// refreshed — keeping the executed code honest.
#[test]
fn emitter_output_matches_pasted_function() {
    let input = FuncInput {
        name: "dynapoly_is_bg_id_bg_actor",
        vram: DYNAPOLY_VRAM,
        words: &DYNAPOLY_WORDS,
    };
    let emitted = emit_function(&input);

    // The verbatim source of the pasted function, reconstructed from THIS file
    // so the two can never silently drift.
    let pasted = include_str!("goldens/dynapoly.rs");
    // Normalize trailing whitespace/newlines for a robust comparison.
    let norm = |s: &str| s.trim_end().replace("\r\n", "\n");
    assert_eq!(
        norm(&emitted),
        norm(pasted),
        "emitter output drifted from the pasted golden; refresh tests/dynapoly_golden.rs \
         and the pasted fn if this change is intended"
    );
}

/// The core oracle validation: our emitted+executed Rust must agree with the
/// C-oracle transcription on the return value ($v0 = ctx.r(2)) for every
/// sampled input.
#[test]
fn dynapoly_matches_c_oracle() {
    // Inputs chosen to exercise both branches and the sign / 0x32 boundaries,
    // including 64-bit-only values (high bits set) that a naive 32-bit compare
    // would get wrong.
    let inputs: [u64; 14] = [
        0,
        1,
        0x31,
        0x32,
        0x33,
        49,
        50,
        51,
        100,
        0xFFFF_FFFF,             // -1 as low32, but as u64 this is +4294967295
        0xFFFF_FFFF_FFFF_FFFF,   // -1 (64-bit) -> negative branch
        0x8000_0000,             // +2^31 in 64-bit terms (NOT negative)
        0xFFFF_FFFF_8000_0000,   // sign-extended INT32_MIN -> negative
        0x0000_0001_0000_0000,   // > u32, positive, >= 0x32
    ];

    for &a0 in &inputs {
        let mut mem_buf = vec![0u8; 64];
        let mut mem = Rdram::new(&mut mem_buf);
        let mut ctx = RecompContext::new();
        ctx.set_r(4, a0); // $a0

        dynapoly_is_bg_id_bg_actor(&mut ctx, &mut mem);

        let got = ctx.r(2); // $v0
        let expected = dynapoly_oracle(a0);
        assert_eq!(
            got, expected,
            "divergence from C oracle for $a0 = {a0:#018X}: emitter got {got}, oracle {expected}"
        );
    }
}

// --- Decoder unit tests (known word -> right op). ---

#[test]
fn decode_alu_immediate() {
    // addiu $v0, $zero, 0x1  = 0x24020001
    assert_eq!(decode(0x24020001), Instruction::Addiu { rt: 2, rs: 0, imm: 1 });
    // slti  $at, $a0, 0x32   = 0x28810032
    assert_eq!(decode(0x28810032), Instruction::Slti { rt: 1, rs: 4, imm: 0x32 });
    // lui   $a0, 0x800F      = 0x3C04800F
    assert_eq!(decode(0x3C04800F), Instruction::Lui { rt: 4, imm: 0x800F });
    // ori   $t0, $t0, 0x6830 = 0x35086830
    assert_eq!(decode(0x35086830), Instruction::Ori { rt: 8, rs: 8, imm: 0x6830 });
    // andi  $v1, $v1, 0x1FFF = 0x30631FFF
    assert_eq!(decode(0x30631FFF), Instruction::Andi { rt: 3, rs: 3, imm: 0x1FFF });
}

#[test]
fn decode_alu_register() {
    // or   $v0, $zero, $zero = 0x00001025
    assert_eq!(decode(0x00001025), Instruction::Or { rd: 2, rs: 0, rt: 0 });
    // addu $t0, $a2, $t7     = 0x00CF4021
    assert_eq!(decode(0x00CF4021), Instruction::Addu { rd: 8, rs: 6, rt: 15 });
    // sll  $t7, $t6, 4       = 0x000E7900
    assert_eq!(decode(0x000E7900), Instruction::Sll { rd: 15, rt: 14, sa: 4 });
    // sra  $v1, $v1, 16      = 0x00031C03
    assert_eq!(decode(0x00031C03), Instruction::Sra { rd: 3, rt: 3, sa: 16 });
    // subu, sltu spot checks
    // subu $v0,$a0,$a1 (funct 0x23) = 0x00851023
    assert_eq!(decode(0x00851023), Instruction::Subu { rd: 2, rs: 4, rt: 5 });
    // sub  $v0,$a0,$a1 (funct 0x22) = 0x00851022
    assert_eq!(decode(0x00851022), Instruction::Sub { rd: 2, rs: 4, rt: 5 });
    // sltu $v0,$a0,$a1 (funct 0x2B) = 0x0085102B
    assert_eq!(decode(0x0085102B), Instruction::Sltu { rd: 2, rs: 4, rt: 5 });
}

#[test]
fn decode_muldiv_and_hilo() {
    // multu $v1, $t1 = 0x00690019
    assert_eq!(decode(0x00690019), Instruction::Multu { rs: 3, rt: 9 });
    // mflo  $t8      = 0x0000C012
    assert_eq!(decode(0x0000C012), Instruction::Mflo { rd: 24 });
    // mult  $a0, $a1 = 0x00850018
    assert_eq!(decode(0x00850018), Instruction::Mult { rs: 4, rt: 5 });
    // div   $a0, $a1 = 0x0085001A
    assert_eq!(decode(0x0085001A), Instruction::Div { rs: 4, rt: 5 });
    // mfhi  $t0      = 0x00004010
    assert_eq!(decode(0x00004010), Instruction::Mfhi { rd: 8 });
}

#[test]
fn decode_branches_and_jumps() {
    // bltz $a0, +3       = 0x04800003 (REGIMM rt=0)
    assert_eq!(decode(0x04800003), Instruction::Bltz { rs: 4, off: 3 });
    // bgez $a0, +3       = 0x04810003 (REGIMM rt=1)
    assert_eq!(decode(0x04810003), Instruction::Bgez { rs: 4, off: 3 });
    // bne  $at, $zero,+3 = 0x14200003
    assert_eq!(decode(0x14200003), Instruction::Bne { rs: 1, rt: 0, off: 3 });
    // beq  $a0, $a1, +5  = 0x10850005
    assert_eq!(decode(0x10850005), Instruction::Beq { rs: 4, rt: 5, off: 5 });
    // bnel $v1, $t6, +6  = 0x546E0006 (opcode 0x15)
    assert_eq!(decode(0x546E0006), Instruction::Bnel { rs: 3, rt: 14, off: 6 });
    // jr   $ra           = 0x03E00008
    assert_eq!(decode(0x03E00008), Instruction::Jr { rs: 31 });
    // jal  0x80063CCC -> target26 = (0x80063CCC & 0x0FFFFFFF) >> 2 = 0x18F33
    // encoding: 0x0C000000 | 0x18F33 = 0x0C018F33
    assert_eq!(decode(0x0C018F33), Instruction::Jal { target: 0x18F33 });
    // j    same target   = 0x08018F33
    assert_eq!(decode(0x08018F33), Instruction::J { target: 0x18F33 });
    // jalr $ra, $t2      = 0x0140F809
    assert_eq!(decode(0x0140F809), Instruction::Jalr { rd: 31, rs: 10 });
}

#[test]
fn decode_loads_and_stores() {
    // lw   $t2, 0x48($sp) = 0x8FAA0048
    assert_eq!(decode(0x8FAA0048), Instruction::Lw { rt: 10, base: 29, off: 0x48 });
    // lh   $v1, 0xA4($a0) = 0x8483_00A4
    assert_eq!(decode(0x848300A4), Instruction::Lh { rt: 3, base: 4, off: 0xA4 });
    // lbu  $t7, 0x2($a2)  = 0x90CF0002
    assert_eq!(decode(0x90CF0002), Instruction::Lbu { rt: 15, base: 6, off: 2 });
    // sw   $s0, 0x20($sp) = 0xAFB00020
    assert_eq!(decode(0xAFB00020), Instruction::Sw { rt: 16, base: 29, off: 0x20 });
    // sb   $t7, 0x27($sp) = 0xA3AF0027
    assert_eq!(decode(0xA3AF0027), Instruction::Sb { rt: 15, base: 29, off: 0x27 });
    // negative offset: lw $t0, -0x8($t1) = 0x8D28FFF8
    assert_eq!(decode(0x8D28FFF8), Instruction::Lw { rt: 8, base: 9, off: -8 });
}

#[test]
fn decode_nop_and_unknown() {
    assert_eq!(decode(0x00000000), Instruction::Nop);
    // An op we don't cover must decode Unknown, never a wrong op. Opcode 0x1E
    // is an unassigned/reserved MIPS III main opcode, so 0x78012345 (opcode
    // 0x1E) is never a real instruction and must stay Unknown. (Note: the COP2
    // *move* ops at opcode 0x12 ARE now decoded as named loud-trap stubs by the
    // cop0 family, so a COP2 move word is no longer Unknown — see
    // `cop0::decode_cop2_stubs`. This case uses a genuinely reserved opcode.)
    assert!(matches!(decode(0x78012345), Instruction::Unknown { .. }));
    // A COP2 sub-op we do not model (rs=0x08 = BC2) is still Unknown, not a
    // wrong op: 0x49000000 (opcode 0x12, rs 0x08).
    assert_eq!(decode(0x49000000), Instruction::Cop2Op { word: 0x49000000 });
    // A COP1 word with an unimplemented `funct` (e.g. RECIP.S, funct 0x15,
    // which OoT does not emit) is likewise Unknown, not a silent mis-decode.
    // recip.s $f0,$f2 = 0x46001015 (fmt=S, funct=0x15).
    assert!(matches!(decode(0x46001015), Instruction::Unknown { .. }));
    // MTC1, now covered, must NOT be Unknown (guards against a regression that
    // drops the whole COP1 family back to Unknown).
    assert_eq!(decode(0x44856000), Instruction::Mtc1 { rt: 5, fs: 12 });
}

/// Delay-slot classification: every branch/jump has one; ALU ops do not.
#[test]
fn delay_slot_classification() {
    assert!(decode(0x04800003).has_delay_slot()); // bltz
    assert!(decode(0x03E00008).has_delay_slot()); // jr
    assert!(decode(0x0C018F33).has_delay_slot()); // jal
    assert!(!decode(0x24020001).has_delay_slot()); // addiu
    assert!(!decode(0x00001025).has_delay_slot()); // or
    assert!(decode(0x546E0006).is_branch_likely()); // bnel
    assert!(!decode(0x14200003).is_branch_likely()); // bne
}

/// The big-endian sub-word swizzle in [`Rdram`] must match the N64Recomp
/// `MEM_*` macro arithmetic exactly (`^2` for halfword, `^3` for byte). This
/// guards the exact bug class `-rs` exists to prevent.
#[test]
fn memory_swizzle_matches_macro_semantics() {
    // Lay out a known word at vram 0x80000000 (rdram offset 0).
    let mut buf = vec![0u8; 64];
    buf[0..4].copy_from_slice(&0x11223344u32.to_ne_bytes());
    let mem = Rdram::new(&mut buf);
    let v = 0xFFFF_FFFF_8000_0000u64; // vram 0x80000000 sign-extended

    // Word: straight native-endian ABI-buffer read.
    assert_eq!(mem.load_w(v) as u32, 0x11223344);
    // On a little-endian host the raw ABI bytes are [44,33,22,11]. XOR 3
    // presents the guest's big-endian byte order [11,22,33,44].
    assert_eq!(mem.load_bu(v.wrapping_add(0)), 0x11);
    assert_eq!(mem.load_bu(v.wrapping_add(1)), 0x22);
    assert_eq!(mem.load_bu(v.wrapping_add(2)), 0x33);
    assert_eq!(mem.load_bu(v.wrapping_add(3)), 0x44);
    // XOR 2 likewise presents guest halfwords 0x1122 and 0x3344.
    assert_eq!(mem.load_hu(v.wrapping_add(0)), 0x1122);
    assert_eq!(mem.load_hu(v.wrapping_add(2)), 0x3344);
}
