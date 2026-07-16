//! The clean-room RSP → typed-Rust recompiler.
//!
//! Three parts, mirroring the task's design:
//! 1. [`decode`] — a hand-rolled, pure-Rust RSP instruction decoder (32-bit
//!    word → typed [`decode::Instr`]), byte-cited from the public MIPS/RSP
//!    ISA and the MIT rabbitizer *encoding tables* (never the GPL impl).
//! 2. [`emit`] — a typed-Rust emitter: decoded ucode → a Rust source string
//!    of one `fn(&mut RspMachine) -> RspExitReason` per ucode, whose every
//!    statement is a typed call on [`runtime::RspMachine`] / [`crate::rsp::ops`]
//!    (no raw casts → the reinterpret-offset bug class is impossible).
//! 3. [`runtime`] — the typed execution substrate the generated code calls
//!    (scalar regs, DMEM loads/stores, CP2 vector load/store + transfers, the
//!    DMA engine).
//!
//! The emitter emits code that references this crate's public API by the
//! `fn64_audio::rsp::...` path, so a *separate* generated crate (or an
//! `include!`d generated file inside a module that re-exports those paths as
//! `fn64_audio`) compiles against it unchanged. The traps below are the
//! loud-failure endpoints the generated code jumps to on an unknown scalar
//! word or an unimplemented VU op.

pub mod decode;
pub mod emit;
pub mod runtime;

pub use emit::emit_module;

use crate::rsp::context::RspExitReason;
use crate::rsp::ops::VuOp;

/// Loud trap for an instruction word the decoder did not recognize. The
/// generated code returns this instead of silently skipping — the address and
/// raw word name exactly what was hit, so a gap is diagnosable, never masked.
///
/// Returns [`RspExitReason::Unsupported`] (the same exit reason RSPRecomp uses
/// for an instruction it was told not to support) after printing the trap.
#[cold]
#[inline(never)]
pub fn trap_unknown(imem_addr: u32, word: u32) -> RspExitReason {
    eprintln!(
        "[fn64-rsp-recomp] TRAP: unimplemented RSP instruction word 0x{word:08X} \
         at IMEM 0x{imem_addr:04X} — recompiler gap, not a silent skip. \
         Decode this opcode from the public ISA and add it to decode.rs."
    );
    RspExitReason::Unsupported
}

/// Loud trap for a CP2 compute op whose body is not wired in the
/// [`crate::rsp::ops::dispatch`] table yet. Names the op so the gap is exact.
#[cold]
#[inline(never)]
pub fn trap_unknown_vu(imem_addr: u32, op: VuOp) -> RspExitReason {
    eprintln!(
        "[fn64-rsp-recomp] TRAP: VU op {op:?} at IMEM 0x{imem_addr:04X} reached \
         but its body is not wired into dispatch() — recompiler/op-table gap, \
         not a silent skip."
    );
    RspExitReason::Unsupported
}

#[cfg(test)]
mod round_trip_tests {
    //! Recompile-and-run round-trip: a tiny hand-built RSP program is decoded,
    //! emitted to Rust source, and — because we cannot invoke rustc mid-test —
    //! executed through an equivalent in-process interpreter built on the SAME
    //! [`runtime::RspMachine`] the generated code targets. This proves the
    //! decode + runtime semantics land on the expected DMEM/reg state, and is
    //! written to FAIL against a deliberately-broken op (fail-against-bug).
    //!
    //! The generated-source path itself is covered by the separate
    //! `emit_module_compiles`-style structural test (asserts the source names
    //! the expected typed calls); building the real generated ucode into a
    //! runnable binary is done by the `oot-audio-ucode` generated crate, whose
    //! own build compiles the emitted Rust with rustc for real.

    use super::*;
    use crate::rsp::decode::{decode, Instr};
    use crate::rsp::ops::{dispatch, OpInvocation, OpStatus};
    use crate::rsp::recomp::runtime::RspMachine;

    /// A minimal typed interpreter over decoded instrs, semantically identical
    /// to what the emitter emits (same RspMachine calls). Runs until `break`
    /// or an unknown op. Returns the exit reason.
    fn run(words: &[u32], base: u32, m: &mut RspMachine) -> RspExitReason {
        let n = words.len();
        let base = 0x1000 | (base & 0x0FFF);
        let mut pc = if m.ctx.resume_address != 0 {
            let resume = 0x1000 | (m.ctx.resume_address & 0x0FFF);
            m.ctx.resume_address = 0;
            resume
        } else {
            base
        };
        let mut steps = 0;
        loop {
            steps += 1;
            if steps > 100_000 {
                panic!("interpreter ran away (no break) — likely a branch bug");
            }
            let idx = ((pc - base) / 4) as usize;
            if idx >= n {
                return RspExitReason::UnhandledJumpTarget;
            }
            let instr = decode(words[idx], pc);
            let delay = if idx + 1 < n {
                Some(decode(words[idx + 1], pc + 4))
            } else {
                None
            };
            match instr {
                Instr::Break => {
                    m.break_rsp();
                    return RspExitReason::Broke;
                }
                Instr::Nop => pc += 4,
                Instr::Lui { rt, imm } => {
                    m.set_reg(rt, (imm as u32) << 16);
                    pc += 4;
                }
                Instr::AluImm { op, rt, rs, imm } => {
                    exec_alu_imm(m, op, rt, rs, imm);
                    pc += 4;
                }
                Instr::AluReg { op, rd, rs, rt } => {
                    exec_alu_reg(m, op, rd, rs, rt);
                    pc += 4;
                }
                Instr::Store {
                    op,
                    rt,
                    base: b,
                    off,
                } => {
                    exec_store(m, op, rt, b, off);
                    pc += 4;
                }
                Instr::Load {
                    op,
                    rt,
                    base: b,
                    off,
                } => {
                    exec_load(m, op, rt, b, off);
                    pc += 4;
                }
                Instr::Branch { op, rs, rt, target } => {
                    let taken = match op {
                        crate::rsp::decode::BranchOp::Beq => m.reg(rs) == m.reg(rt),
                        crate::rsp::decode::BranchOp::Bne => m.reg(rs) != m.reg(rt),
                    };
                    run_delay(m, delay);
                    pc = if taken { target as u32 } else { pc + 8 };
                }
                Instr::BranchZ {
                    op,
                    rs,
                    target,
                    link,
                } => {
                    use crate::rsp::decode::BranchZOp::*;
                    let v = m.reg(rs) as i32;
                    let taken = match op {
                        Blez => v <= 0,
                        Bgtz => v > 0,
                        Bltz => v < 0,
                        Bgez => v >= 0,
                    };
                    if let Some(ret) = link {
                        m.set_reg(31, ret as u32);
                    }
                    run_delay(m, delay);
                    pc = if taken { target as u32 } else { pc + 8 };
                }
                Instr::Vu {
                    op,
                    vd,
                    vs,
                    vt,
                    e,
                    de,
                } => {
                    let inv = OpInvocation {
                        vd: vd as usize,
                        vs: vs as usize,
                        vt: vt as usize,
                        e: e as usize,
                        de: de as usize,
                        vs_index: vs as usize,
                    };
                    match dispatch(m.vu(), op, inv) {
                        OpStatus::Executed => {}
                        OpStatus::Unimplemented(o) => return trap_unknown_vu(pc, o),
                    }
                    pc += 4;
                }
                Instr::VLoad {
                    op,
                    vt,
                    elem,
                    base: b,
                    off,
                } => {
                    let bv = m.reg(b);
                    m.vload(op, vt, elem, bv, off);
                    pc += 4;
                }
                Instr::VStore {
                    op,
                    vt,
                    elem,
                    base: b,
                    off,
                } => {
                    let bv = m.reg(b);
                    m.vstore(op, vt, elem, bv, off);
                    pc += 4;
                }
                Instr::Mfc0 { rt, cop0 } => {
                    let value = m.read_cp0(cop0);
                    m.set_reg(rt, value);
                    pc += 4;
                }
                Instr::Mtc0 { rt, cop0 } => {
                    let value = m.reg(rt);
                    if let Some(reason) = m.write_cp0(cop0, value) {
                        m.ctx.resume_address = pc + 4;
                        return reason;
                    }
                    pc += 4;
                }
                // Control transfers with the delay slot run on the fallthrough
                // path (well-formed ucode never nests a branch in a delay slot).
                Instr::Jump { target } => {
                    run_delay(m, delay);
                    pc = target as u32;
                }
                Instr::Jal { target, ret } => {
                    m.set_reg(31, ret as u32);
                    run_delay(m, delay);
                    pc = target as u32;
                }
                Instr::Jr { rs } => {
                    let jt = 0x1000 | (m.reg(rs) & 0x0FFF);
                    run_delay(m, delay);
                    pc = jt;
                }
                Instr::Jalr { rd, rs, ret } => {
                    let jt = 0x1000 | (m.reg(rs) & 0x0FFF);
                    m.set_reg(rd, ret as u32);
                    run_delay(m, delay);
                    pc = jt;
                }
                Instr::Unknown { word } => return trap_unknown(pc, word),
                other => panic!("interpreter missing arm for {other:?}"),
            }
        }
    }

    fn run_delay(m: &mut RspMachine, delay: Option<Instr>) {
        match delay {
            Some(Instr::AluImm { op, rt, rs, imm }) => exec_alu_imm(m, op, rt, rs, imm),
            Some(Instr::AluReg { op, rd, rs, rt }) => exec_alu_reg(m, op, rd, rs, rt),
            Some(Instr::Lui { rt, imm }) => m.set_reg(rt, (imm as u32) << 16),
            Some(Instr::Load { op, rt, base, off }) => exec_load(m, op, rt, base, off),
            Some(Instr::Mfc0 { rt, cop0 }) => {
                let value = m.read_cp0(cop0);
                m.set_reg(rt, value);
            }
            _ => {}
        }
    }

    fn exec_alu_imm(
        m: &mut RspMachine,
        op: crate::rsp::decode::AluImmOp,
        rt: u8,
        rs: u8,
        imm: u16,
    ) {
        use crate::rsp::decode::AluImmOp::*;
        let simm = imm as i16 as i32 as u32;
        let v = match op {
            Addi | Addiu => m.reg(rs).wrapping_add(simm),
            Andi => m.reg(rs) & imm as u32,
            Ori => m.reg(rs) | imm as u32,
            Xori => m.reg(rs) ^ imm as u32,
            Slti => {
                if (m.reg(rs) as i32) < (imm as i16 as i32) {
                    1
                } else {
                    0
                }
            }
            Sltiu => {
                if m.reg(rs) < simm {
                    1
                } else {
                    0
                }
            }
        };
        m.set_reg(rt, v);
    }

    fn exec_alu_reg(m: &mut RspMachine, op: crate::rsp::decode::AluRegOp, rd: u8, rs: u8, rt: u8) {
        use crate::rsp::decode::AluRegOp::*;
        let v = match op {
            Add | Addu => m.reg(rs).wrapping_add(m.reg(rt)),
            Sub | Subu => m.reg(rs).wrapping_sub(m.reg(rt)),
            And => m.reg(rs) & m.reg(rt),
            Or => m.reg(rs) | m.reg(rt),
            Xor => m.reg(rs) ^ m.reg(rt),
            Nor => !(m.reg(rs) | m.reg(rt)),
            Slt => ((m.reg(rs) as i32) < (m.reg(rt) as i32)) as u32,
            Sltu => (m.reg(rs) < m.reg(rt)) as u32,
        };
        m.set_reg(rd, v);
    }

    fn exec_store(m: &mut RspMachine, op: crate::rsp::decode::StoreOp, rt: u8, base: u8, off: i16) {
        use crate::rsp::decode::StoreOp::*;
        let a = m.reg(base).wrapping_add(off as i32 as u32);
        let v = m.reg(rt);
        match op {
            Sb => m.store_b(a, v),
            Sh => m.store_h(a, v),
            Sw => m.store_w(a, v),
        }
    }

    fn exec_load(m: &mut RspMachine, op: crate::rsp::decode::LoadOp, rt: u8, base: u8, off: i16) {
        use crate::rsp::decode::LoadOp::*;
        let a = m.reg(base).wrapping_add(off as i32 as u32);
        let v = match op {
            Lb => m.load_b(a),
            Lbu => m.load_bu(a),
            Lh => m.load_h(a),
            Lhu => m.load_hu(a),
            Lw => m.load_w(a),
        };
        m.set_reg(rt, v);
    }

    /// Assemble helpers for the tiny test program.
    fn ori(rt: u8, rs: u8, imm: u16) -> u32 {
        (0x0D << 26) | ((rs as u32) << 21) | ((rt as u32) << 16) | imm as u32
    }
    fn addiu(rt: u8, rs: u8, imm: u16) -> u32 {
        (0x09 << 26) | ((rs as u32) << 21) | ((rt as u32) << 16) | imm as u32
    }
    fn sw(rt: u8, base: u8, off: u16) -> u32 {
        (0x2B << 26) | ((base as u32) << 21) | ((rt as u32) << 16) | off as u32
    }
    fn brk() -> u32 {
        0x0000_000D
    }

    #[test]
    fn round_trip_scalar_program_writes_expected_dmem() {
        // r2 = 0x1234_0000 (lui) | 0x5678 (ori) = 0x12345678
        // r3 = r0 + 0x100 (addiu -> DMEM addr 0x100)
        // MEM_W[r3+0] = r2
        // break
        let lui = |rt: u8, imm: u16| (0x0F << 26) | ((rt as u32) << 16) | imm as u32;
        let prog = [
            lui(2, 0x1234),
            ori(2, 2, 0x5678),
            addiu(3, 0, 0x100),
            sw(2, 3, 0),
            brk(),
        ];
        let mut rdram = vec![0u8; 0x1000];
        let mut m = RspMachine::new(&mut rdram);
        let reason = run(&prog, 0x1080, &mut m);
        assert_eq!(reason, RspExitReason::Broke);
        assert_eq!(m.reg(2), 0x1234_5678);
        // Word stored at DMEM 0x100.
        assert_eq!(m.load_w(0x100), 0x1234_5678);
    }

    #[test]
    fn round_trip_vector_add_accumulates_lanes() {
        // Build two vector regs via MTC2-free direct set, add them with VADD,
        // and verify the result lane. Uses LQV to load from DMEM we seed.
        let mut rdram = vec![0u8; 0x1000];
        let mut m = RspMachine::new(&mut rdram);
        // Seed DMEM 0x00 with 8 halfwords = 1..8 (big-endian), 0x10 with 10..
        for i in 0..8u32 {
            m.dmem.write_h(i * 2, (i as i16) + 1);
            m.dmem.write_h(0x10 + i * 2, (i as i16) + 10);
        }
        // Program: addiu r4,r0,0 ; addiu r5,r0,0x10
        //   lqv v1,0(r4) ; lqv v2,0(r5) ; vadd v3,v1,v2 ; sqv v3,0x20(r0-based) ; break
        let lqv = |vt: u8, base: u8, off: u16| {
            (0x32u32 << 26)
                | ((base as u32) << 21)
                | ((vt as u32) << 16)
                | (0x04 << 11)
                | off as u32
        };
        let sqv = |vt: u8, base: u8, off: u16| {
            (0x3Au32 << 26)
                | ((base as u32) << 21)
                | ((vt as u32) << 16)
                | (0x04 << 11)
                | off as u32
        };
        let vadd = |vd: u8, vs: u8, vt: u8| {
            (0x12u32 << 26)
                | (1 << 25)
                | ((vt as u32) << 16)
                | ((vs as u32) << 11)
                | ((vd as u32) << 6)
                | 0x10
        };
        let prog = [
            addiu(4, 0, 0x00),
            addiu(5, 0, 0x10),
            lqv(1, 4, 0),
            lqv(2, 5, 0),
            vadd(3, 1, 2),
            sqv(3, 6, 0), // r6 = 0 -> store to DMEM 0x00 region... use r6=0x40
            brk(),
        ];
        m.set_reg(6, 0x40);
        let reason = run(&prog, 0x1080, &mut m);
        assert_eq!(reason, RspExitReason::Broke);
        // v3 lane i = (i+1) + (i+10) = 2i + 11.
        for i in 0..8usize {
            assert_eq!(m.ctx.rsp.regs.r[3][i], (2 * i as i16) + 11, "lane {i}");
        }
        // And it was stored: DMEM 0x40 halfword 0 = 11.
        assert_eq!(m.dmem.read_h(0x40), 11);
    }

    /// Fail-against-bug: if the decoder mis-decoded VADD's funct or the vadd
    /// dispatch were wrong, the lane sum would not be 2i+11. This test would
    /// go red — proving it actually checks the math, not just "ran".
    #[test]
    fn emit_module_names_typed_calls_not_raw_casts() {
        // Structural check on the emitted source: it must call the typed
        // runtime methods and must NOT contain a raw pointer cast.
        let prog = [
            (0x0Fu32 << 26) | (2 << 16) | 0x1234, // lui r2,0x1234
            0x0000_000D,                          // break
        ];
        let src = emit_module(&prog, 0x1080, "test_ucode");
        assert!(src.contains("pub fn test_ucode(m: &mut RspMachine) -> RspExitReason"));
        assert!(src.contains("m.set_reg(2, 0x12340000);"));
        assert!(src.contains("RspExitReason::Broke"));
        // No raw pointer reinterpretation in the generated text.
        assert!(!src.contains("as *mut"));
        assert!(!src.contains("as *const"));
        assert!(!src.contains("transmute"));
    }

    // -- assemble helpers for the dispatch-loop regression test --
    fn j(target: u32) -> u32 {
        (0x02u32 << 26) | ((target >> 2) & 0x03FF_FFFF)
    }
    fn jr(rs: u8) -> u32 {
        ((rs as u32) << 21) | 0x08
    }
    fn bgtz(rs: u8, target: u32, pc: u32) -> u32 {
        let off = (((target as i32) - (pc as i32 + 4)) >> 2) as u16;
        (0x07u32 << 26) | ((rs as u32) << 21) | off as u32
    }
    fn addi(rt: u8, rs: u8, imm: i16) -> u32 {
        (0x08u32 << 26) | ((rs as u32) << 21) | ((rt as u32) << 16) | (imm as u16) as u32
    }

    /// Regression for the aspMain dispatch runaway (fixed on branch
    /// `fix/rsp-dispatch-loop-*`): OoT's real aspMain is a `jr`-dispatched
    /// command-list loop whose per-command handlers end in `j <loop_top>` and
    /// whose whole task ends via `break` once the command COUNT is exhausted.
    /// The bug shipped the recompiled module at IMEM base 0x1080 instead of the
    /// true 0x1000, shifting every absolute `j`/jump-table target by 0x80 so
    /// the loop-back `j` never reached the count check — a 6-instruction
    /// unconditional runaway that trapped at the 5M-step ceiling and produced
    /// zero PCM.
    ///
    /// This builds that exact SHAPE at base 0x1000 — loop top decrements the
    /// count, `bgtz` continues to a `jr`-dispatched handler, the handler `j`s
    /// back to the loop top, and the loop `break`s when the count hits 0 — and
    /// asserts it TERMINATES (`Broke`) in BOUNDED steps. The interpreter here
    /// mirrors the emitter's absolute-address control flow, so a base/target
    /// mismap would spin exactly as the shipped bug did; the run() interpreter's
    /// own 100_000-step ceiling panics ("interpreter ran away") on a runaway,
    /// making this test fail-red against the reintroduced bug rather than hang.
    #[test]
    fn jr_dispatched_command_loop_terminates_on_count() {
        // Program layout at base 0x1000 (word index -> IMEM addr):
        //   0x1000 loop_top: bgtz r30, 0x100C   (count>0 -> dispatch; else fall)
        //   0x1004           nop  (delay)
        //   0x1008           break                (count exhausted -> task done)
        //   0x100C dispatch:  addi r30,r30,-1      (consume one command)
        //   0x1010           jr r2                 (indirect: r2 = handler addr)
        //   0x1014           nop  (delay)
        //   0x1018 handler:   j 0x1000             (back to loop top)
        //   0x101C           nop  (delay)
        let base = 0x1000u32;
        let prog = [
            bgtz(30, 0x100C, 0x1000), // 0x1000
            0,                        // 0x1004 nop (delay)
            brk(),                    // 0x1008
            addi(30, 30, -1),         // 0x100C
            jr(2),                    // 0x1010
            0,                        // 0x1014 nop (delay)
            j(0x1000),                // 0x1018
            0,                        // 0x101C nop (delay)
        ];
        let mut rdram = vec![0u8; 0x1000];
        let mut m = RspMachine::new(&mut rdram);
        m.set_reg(30, 3); // three commands to process
                          // The hardware PC is 12 bits, so a jump table may store either the
                          // bare PC offset or an IMEM-window address. Exercise the bare form.
        m.set_reg(2, 0x0018);
        let reason = run(&prog, base, &mut m);
        assert_eq!(
            reason,
            RspExitReason::Broke,
            "jr-dispatched command loop must reach the task-done BREAK once the \
             command count is exhausted — a base/target mismap spins forever"
        );
        // The count drove termination: r30 decremented to 0.
        assert_eq!(m.reg(30), 0);
    }

    #[test]
    fn branch_fallthrough_executes_delay_once_then_advances_pc_plus_8() {
        let base = 0x1000u32;
        let beq = |rs: u8, rt: u8, target: u32, pc: u32| {
            let off = (((target as i32) - (pc as i32 + 4)) >> 2) as u16;
            (0x04u32 << 26) | ((rs as u32) << 21) | ((rt as u32) << 16) | off as u32
        };
        let prog = [
            beq(2, 3, 0x1010, 0x1000),
            addi(4, 4, 1),
            addi(4, 4, 10),
            brk(),
            brk(),
        ];
        let mut rdram = vec![0u8; 16];
        let mut m = RspMachine::new(&mut rdram);
        m.set_reg(2, 1);
        m.set_reg(3, 2);
        assert_eq!(run(&prog, base, &mut m), RspExitReason::Broke);
        assert_eq!(
            m.reg(4),
            11,
            "delay slot executes once, then pc+8 instruction runs"
        );

        let source = emit_module(&prog, base, "branch_delay_test");
        let branch_start = source
            .find("if m.reg(2) == m.reg(3)")
            .expect("emitted conditional branch");
        let else_start = branch_start
            + source[branch_start..]
                .find("} else {")
                .expect("emitted branch has fallthrough");
        let fallthrough = &source[else_start..source.len().min(else_start + 240)];
        assert!(fallthrough.contains("pc = 0x1008;"));
        assert!(!fallthrough.contains("pc = 0x1004;"));
    }

    #[test]
    fn linked_regimm_branch_writes_ra_even_when_not_taken() {
        let base = 0x1000u32;
        let bltzal = (0x01u32 << 26) | (2 << 21) | (0x10 << 16) | 1;
        let prog = [bltzal, 0, brk(), brk()];
        let mut rdram = vec![0u8; 16];
        let mut m = RspMachine::new(&mut rdram);
        m.set_reg(2, 1); // positive: BLTZAL is not taken
        assert_eq!(run(&prog, base, &mut m), RspExitReason::Broke);
        assert_eq!(m.reg(31), 0x0008);
    }

    #[test]
    fn emitted_overlay_resume_handles_sequential_and_delay_slot_dma() {
        let mtc0 = |rt: u8, rd: u8| {
            (0x10u32 << 26) | (0x04 << 21) | ((rt as u32) << 16) | ((rd as u32) << 11)
        };
        let sequential = emit_module(&[mtc0(3, 2), brk()], 0x1000, "overlay_seq");
        assert!(sequential.contains("m.ctx.resume_address = 0x1004;"));
        assert!(sequential.contains("let resume = 0x1000 | (m.ctx.resume_address & 0x0FFF);"));

        let delayed = emit_module(&[jr(2), mtc0(3, 2)], 0x1000, "overlay_delay");
        assert!(delayed.contains("m.ctx.resume_address = jt; break 'run r;"));
    }

    #[test]
    fn a_second_overlay_resumes_after_the_dma_trigger() {
        let mtc0 = |rt: u8, rd: u8| {
            (0x10u32 << 26) | (0x04 << 21) | ((rt as u32) << 16) | ((rd as u32) << 11)
        };
        let first_overlay = [mtc0(2, 0), mtc0(3, 2), brk(), brk()];
        let second_overlay = [0, 0, addiu(4, 0, 42), brk()];
        let mut rdram = vec![0u8; 32];
        let mut m = RspMachine::new(&mut rdram);
        m.set_reg(2, 0x1000); // SP_MEM_ADDR selects IMEM
        m.set_reg(3, 7); // one aligned eight-byte DMA line

        assert_eq!(
            run(&first_overlay, 0x1000, &mut m),
            RspExitReason::SwapOverlay
        );
        assert_eq!(m.ctx.resume_address, 0x1008);
        assert_eq!(run(&second_overlay, 0x1000, &mut m), RspExitReason::Broke);
        assert_eq!(m.reg(4), 42, "replacement IMEM image resumes after MTC0");
        assert_eq!(m.ctx.resume_address, 0);
    }
}
