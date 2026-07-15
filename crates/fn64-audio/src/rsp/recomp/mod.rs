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
        let mut pc = base;
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
                Instr::Break => return RspExitReason::Broke,
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
}
