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

use std::cell::Cell;

use crate::rsp::context::RspExitReason;
use crate::rsp::ops::VuOp;

thread_local! {
    static CONTENT_SAFE_DIAGNOSTIC_DEPTH: Cell<u32> = const { Cell::new(0) };
}

/// Thread-local guard used by private-input characterization to suppress any
/// diagnostic that would reproduce instruction or guest-memory values.
pub(crate) struct ContentSafeDiagnosticsGuard;

impl ContentSafeDiagnosticsGuard {
    pub(crate) fn enter() -> Self {
        CONTENT_SAFE_DIAGNOSTIC_DEPTH.with(|depth| {
            depth.set(
                depth
                    .get()
                    .checked_add(1)
                    .expect("diagnostic guard depth overflow"),
            );
        });
        Self
    }
}

impl Drop for ContentSafeDiagnosticsGuard {
    fn drop(&mut self) {
        CONTENT_SAFE_DIAGNOSTIC_DEPTH.with(|depth| {
            depth.set(
                depth
                    .get()
                    .checked_sub(1)
                    .expect("unbalanced diagnostic guard"),
            );
        });
    }
}

pub(crate) fn content_safe_diagnostics() -> bool {
    CONTENT_SAFE_DIAGNOSTIC_DEPTH.with(|depth| depth.get() != 0)
}

/// Loud trap for an instruction word the decoder did not recognize. The
/// generated code returns this instead of silently skipping — the address and
/// raw word name exactly what was hit, so a gap is diagnosable, never masked.
///
/// Returns [`RspExitReason::Unsupported`] (the same exit reason RSPRecomp uses
/// for an instruction it was told not to support) after printing the trap.
#[cold]
#[inline(never)]
pub fn trap_unknown(imem_addr: u32, word: u32) -> RspExitReason {
    let context = if content_safe_diagnostics() {
        format!("unimplemented RSP instruction at IMEM 0x{imem_addr:04X} (word redacted)")
    } else {
        format!("unimplemented RSP instruction word 0x{word:08X} at IMEM 0x{imem_addr:04X}")
    };
    fn64_runtime::record_unsupported_event(
        fn64_runtime::UnsupportedSubsystem::Audio,
        "audio.rsp.unknown-instruction",
        context.clone(),
        None,
        fn64_runtime::UnsupportedDisposition::ReturnedError,
    );
    eprintln!(
        "[fn64-rsp-recomp] TRAP: {context} — recompiler gap, not a silent skip. \
         Decode this opcode from the public ISA and add it to decode.rs."
    );
    RspExitReason::Unsupported
}

/// Loud trap for a CP2 compute op whose body is not wired in the
/// [`crate::rsp::ops::dispatch`] table yet. Names the op so the gap is exact.
#[cold]
#[inline(never)]
pub fn trap_unknown_vu(imem_addr: u32, op: VuOp) -> RspExitReason {
    let context = if content_safe_diagnostics() {
        format!("unimplemented VU operation at IMEM 0x{imem_addr:04X} (operation redacted)")
    } else {
        format!("VU op {op:?} at IMEM 0x{imem_addr:04X} has no dispatch body")
    };
    fn64_runtime::record_unsupported_event(
        fn64_runtime::UnsupportedSubsystem::Audio,
        "audio.rsp.unimplemented-vu-op",
        context.clone(),
        None,
        fn64_runtime::UnsupportedDisposition::ReturnedError,
    );
    eprintln!("[fn64-rsp-recomp] TRAP: {context} — recompiler/op-table gap, not a silent skip.");
    RspExitReason::Unsupported
}

#[cold]
#[inline(never)]
pub fn trap_unhandled_jump(imem_addr: u32) -> RspExitReason {
    let context = format!("RSP jump target at IMEM 0x{imem_addr:04X} has no admitted instruction");
    fn64_runtime::record_unsupported_event(
        fn64_runtime::UnsupportedSubsystem::Audio,
        "audio.rsp.unhandled-jump-target",
        context.clone(),
        None,
        fn64_runtime::UnsupportedDisposition::ReturnedError,
    );
    eprintln!("[fn64-rsp-recomp] TRAP: {context}");
    RspExitReason::Unsupported
}

#[cold]
#[inline(never)]
pub fn trap_imem_overrun(imem_addr: u32) -> RspExitReason {
    let context = format!("RSP execution ran outside admitted IMEM at 0x{imem_addr:04X}");
    fn64_runtime::record_unsupported_event(
        fn64_runtime::UnsupportedSubsystem::Audio,
        "audio.rsp.imem-overrun",
        context.clone(),
        None,
        fn64_runtime::UnsupportedDisposition::ReturnedError,
    );
    eprintln!("[fn64-rsp-recomp] TRAP: {context}");
    RspExitReason::Unsupported
}

#[cold]
#[inline(never)]
pub fn trap_step_budget(imem_addr: u32) -> RspExitReason {
    let context =
        format!("recompiled RSP exceeded its fixed step budget at IMEM 0x{imem_addr:04X}");
    fn64_runtime::record_unsupported_event(
        fn64_runtime::UnsupportedSubsystem::Audio,
        "audio.rsp.recompiler-step-budget",
        context.clone(),
        None,
        fn64_runtime::UnsupportedDisposition::ReturnedError,
    );
    eprintln!("[fn64-rsp-recomp] TRAP: {context}");
    RspExitReason::Unsupported
}

/// Loud endpoint for a control-transfer instruction in a branch delay slot.
/// Both the interpreter and emitted RSP bodies route through this one typed
/// recorder before preserving the existing panic.
#[cold]
#[inline(never)]
pub fn trap_delay_slot_control(imem_addr: u32, instruction: impl std::fmt::Debug) -> ! {
    let context = if content_safe_diagnostics() {
        format!(
            "RSP delay-slot control transfer is unsupported at IMEM 0x{imem_addr:04X} \
             (instruction redacted)"
        )
    } else {
        format!(
            "unsupported RSP control transfer {instruction:?} in delay slot at IMEM 0x{imem_addr:04X}"
        )
    };
    fn64_runtime::record_unsupported_event(
        fn64_runtime::UnsupportedSubsystem::Audio,
        "audio.rsp.delay-slot-control-transfer",
        context.clone(),
        None,
        fn64_runtime::UnsupportedDisposition::LoudTrap,
    );
    panic!("{context}")
}

#[cfg(test)]
mod unsupported_event_tests {
    use super::*;

    #[test]
    fn rsp_gap_endpoints_record_typed_events_before_return_or_panic() {
        fn64_runtime::arm_unsupported_events(None).unwrap();
        assert_eq!(
            trap_unknown(0x1040, 0xffff_ffff),
            RspExitReason::Unsupported
        );
        assert_eq!(
            trap_unknown_vu(0x1080, VuOp::Vmulf),
            RspExitReason::Unsupported
        );
        let panic = std::panic::catch_unwind(|| trap_delay_slot_control(0x10c0, "Jr"));
        assert!(panic.is_err());
        assert_eq!(trap_unhandled_jump(0x1100), RspExitReason::Unsupported);
        assert_eq!(trap_imem_overrun(0x2100), RspExitReason::Unsupported);
        assert_eq!(trap_step_budget(0x1140), RspExitReason::Unsupported);

        let events = fn64_runtime::copy_unsupported_events();
        assert_eq!(events.len(), 6);
        assert_eq!(
            events[0].subsystem,
            fn64_runtime::UnsupportedSubsystem::Audio
        );
        assert_eq!(
            events[0].operation,
            concat!("audio.rsp.", "unknown-instruction")
        );
        assert_eq!(
            events[0].disposition,
            fn64_runtime::UnsupportedDisposition::ReturnedError
        );
        assert_eq!(
            events[1].operation,
            concat!("audio.rsp.", "unimplemented-vu-op")
        );
        assert_eq!(
            events[2].operation,
            concat!("audio.rsp.", "delay-slot-control-transfer")
        );
        assert_eq!(
            events[2].disposition,
            fn64_runtime::UnsupportedDisposition::LoudTrap
        );
        assert_eq!(
            events[3].operation,
            concat!("audio.rsp.", "unhandled-jump-target")
        );
        assert_eq!(events[4].operation, concat!("audio.rsp.", "imem-overrun"));
        assert_eq!(
            events[5].operation,
            concat!("audio.rsp.", "recompiler-step-budget")
        );

        fn64_runtime::arm_unsupported_events(None).unwrap();
        {
            let _guard = ContentSafeDiagnosticsGuard::enter();
            assert_eq!(
                trap_unknown(0x1040, 0xdead_beef),
                RspExitReason::Unsupported
            );
            assert_eq!(
                trap_unknown_vu(0x1080, VuOp::Vmulf),
                RspExitReason::Unsupported
            );
        }
        let redacted = fn64_runtime::copy_unsupported_events();
        assert_eq!(redacted.len(), 2);
        assert!(redacted[0].context.contains("word redacted"));
        assert!(!redacted[0].context.contains("DEADBEEF"));
        assert!(redacted[1].context.contains("operation redacted"));
        assert!(!redacted[1].context.contains("Vmulf"));
    }
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
                return trap_unhandled_jump(pc);
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
                Instr::Shift { op, rd, rt, sa } => {
                    exec_shift(m, op, rd, rt, sa as u32, None);
                    pc += 4;
                }
                Instr::ShiftVar { op, rd, rt, rs } => {
                    exec_shift(m, op, rd, rt, 0, Some(rs));
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
                    let next_pc = if taken { target as u32 } else { pc + 8 };
                    if let Some(reason) = run_delay(m, delay, pc + 4, next_pc) {
                        return reason;
                    }
                    pc = next_pc;
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
                    let next_pc = if taken { target as u32 } else { pc + 8 };
                    if let Some(reason) = run_delay(m, delay, pc + 4, next_pc) {
                        return reason;
                    }
                    pc = next_pc;
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
                Instr::Mfc2 { rt, vs, elem } => {
                    let value = m.mfc2(vs, elem);
                    m.set_reg(rt, value);
                    pc += 4;
                }
                Instr::Mtc2 { rt, vs, elem } => {
                    let value = m.reg(rt);
                    m.mtc2(vs, elem, value);
                    pc += 4;
                }
                Instr::Cfc2 { rt, cd } => {
                    let value = m.cfc2(cd);
                    m.set_reg(rt, value);
                    pc += 4;
                }
                Instr::Ctc2 { rt, cd } => {
                    let value = m.reg(rt);
                    m.ctc2(cd, value);
                    pc += 4;
                }
                // Control transfers with the delay slot run on the fallthrough
                // path (well-formed ucode never nests a branch in a delay slot).
                Instr::Jump { target } => {
                    let next_pc = target as u32;
                    if let Some(reason) = run_delay(m, delay, pc + 4, next_pc) {
                        return reason;
                    }
                    pc = next_pc;
                }
                Instr::Jal { target, ret } => {
                    m.set_reg(31, ret as u32);
                    let next_pc = target as u32;
                    if let Some(reason) = run_delay(m, delay, pc + 4, next_pc) {
                        return reason;
                    }
                    pc = next_pc;
                }
                Instr::Jr { rs } => {
                    let jt = 0x1000 | (m.reg(rs) & 0x0FFF);
                    if let Some(reason) = run_delay(m, delay, pc + 4, jt) {
                        return reason;
                    }
                    pc = jt;
                }
                Instr::Jalr { rd, rs, ret } => {
                    let jt = 0x1000 | (m.reg(rs) & 0x0FFF);
                    m.set_reg(rd, ret as u32);
                    if let Some(reason) = run_delay(m, delay, pc + 4, jt) {
                        return reason;
                    }
                    pc = jt;
                }
                Instr::Unknown { word } => return trap_unknown(pc, word),
            }
        }
    }

    /// Execute every non-control instruction the emitter accepts in a delay
    /// slot. This match stays exhaustive so a newly decoded instruction cannot
    /// silently widen the interpreter/emitter oracle gap.
    fn run_delay(
        m: &mut RspMachine,
        delay: Option<Instr>,
        delay_pc: u32,
        resume_pc: u32,
    ) -> Option<RspExitReason> {
        match delay {
            None | Some(Instr::Nop) => None,
            Some(Instr::AluImm { op, rt, rs, imm }) => {
                exec_alu_imm(m, op, rt, rs, imm);
                None
            }
            Some(Instr::AluReg { op, rd, rs, rt }) => {
                exec_alu_reg(m, op, rd, rs, rt);
                None
            }
            Some(Instr::Shift { op, rd, rt, sa }) => {
                exec_shift(m, op, rd, rt, sa as u32, None);
                None
            }
            Some(Instr::ShiftVar { op, rd, rt, rs }) => {
                exec_shift(m, op, rd, rt, 0, Some(rs));
                None
            }
            Some(Instr::Lui { rt, imm }) => {
                m.set_reg(rt, (imm as u32) << 16);
                None
            }
            Some(Instr::Load { op, rt, base, off }) => {
                exec_load(m, op, rt, base, off);
                None
            }
            Some(Instr::Store { op, rt, base, off }) => {
                exec_store(m, op, rt, base, off);
                None
            }
            Some(Instr::Mfc0 { rt, cop0 }) => {
                let value = m.read_cp0(cop0);
                m.set_reg(rt, value);
                None
            }
            Some(Instr::Mtc0 { rt, cop0 }) => {
                let value = m.reg(rt);
                let reason = m.write_cp0(cop0, value);
                if reason.is_some() {
                    m.ctx.resume_address = resume_pc;
                }
                reason
            }
            Some(Instr::Mfc2 { rt, vs, elem }) => {
                let value = m.mfc2(vs, elem);
                m.set_reg(rt, value);
                None
            }
            Some(Instr::Mtc2 { rt, vs, elem }) => {
                let value = m.reg(rt);
                m.mtc2(vs, elem, value);
                None
            }
            Some(Instr::Cfc2 { rt, cd }) => {
                let value = m.cfc2(cd);
                m.set_reg(rt, value);
                None
            }
            Some(Instr::Ctc2 { rt, cd }) => {
                let value = m.reg(rt);
                m.ctc2(cd, value);
                None
            }
            Some(Instr::VLoad {
                op,
                vt,
                elem,
                base,
                off,
            }) => {
                let value = m.reg(base);
                m.vload(op, vt, elem, value, off);
                None
            }
            Some(Instr::VStore {
                op,
                vt,
                elem,
                base,
                off,
            }) => {
                let value = m.reg(base);
                m.vstore(op, vt, elem, value, off);
                None
            }
            Some(Instr::Vu {
                op,
                vd,
                vs,
                vt,
                e,
                de,
            }) => {
                let invocation = OpInvocation {
                    vd: vd as usize,
                    vs: vs as usize,
                    vt: vt as usize,
                    e: e as usize,
                    de: de as usize,
                    vs_index: vs as usize,
                };
                match dispatch(m.vu(), op, invocation) {
                    OpStatus::Executed => None,
                    OpStatus::Unimplemented(op) => Some(trap_unknown_vu(delay_pc, op)),
                }
            }
            Some(Instr::Break) => {
                m.break_rsp();
                Some(RspExitReason::Broke)
            }
            Some(Instr::Unknown { word }) => Some(trap_unknown(delay_pc, word)),
            Some(
                illegal @ (Instr::Branch { .. }
                | Instr::BranchZ { .. }
                | Instr::Jump { .. }
                | Instr::Jal { .. }
                | Instr::Jr { .. }
                | Instr::Jalr { .. }),
            ) => trap_delay_slot_control(delay_pc, illegal),
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

    fn exec_shift(
        m: &mut RspMachine,
        op: crate::rsp::decode::ShiftOp,
        rd: u8,
        rt: u8,
        sa: u32,
        rs: Option<u8>,
    ) {
        use crate::rsp::decode::ShiftOp::*;
        let amount = rs.map_or(sa, |reg| m.reg(reg) & 31);
        let value = match op {
            Sll => m.reg(rt) << amount,
            Srl => m.reg(rt) >> amount,
            Sra => ((m.reg(rt) as i32) >> amount) as u32,
        };
        m.set_reg(rd, value);
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

    /// Fail-against-bug: `run_delay` used to omit `Store`, even though the
    /// emitter inlined the same instruction as a typed `store_w` call.
    #[test]
    fn delay_slot_store_executes_and_matches_emitter() {
        let base = 0x1000u32;
        let beq = (0x04u32 << 26) | 2; // beq r0,r0,0x100c
        let prog = [beq, sw(2, 3, 0), brk(), brk()];
        let mut rdram = vec![0u8; 0x1000];
        let mut m = RspMachine::new(&mut rdram);
        m.set_reg(2, 0x1234_5678);
        m.set_reg(3, 0x100);

        assert_eq!(run(&prog, base, &mut m), RspExitReason::Broke);
        assert_eq!(
            m.load_w(0x100),
            0x1234_5678,
            "the interpreter must execute a scalar store in the branch delay slot"
        );

        let source = emit_module(&prog, base, "delay_store_test");
        assert!(source.contains(
            "let a = m.reg(3).wrapping_add(0x00000000); let v = m.reg(2); m.store_w(a, v);"
        ));
    }

    /// Fail-against-bug: stale generated aspMain had the branch delay slot
    /// inlined into the not-taken arm, then set `pc` to the delay-slot label
    /// itself. That executes the delay slot twice on fallthrough and shifts
    /// later audio DMA source pointers by one loop body.
    #[test]
    fn emitted_conditional_branch_fallthrough_skips_inlined_delay_slot() {
        let base = 0x1000u32;
        let bne_not_taken = 0x1400_0001; // bne r0,r0,0x1008; delay slot at 0x1004
        let prog = [bne_not_taken, sw(2, 3, 0), brk()];

        let source = emit_module(&prog, base, "delay_fallthrough_test");
        assert!(
            source.contains("} else {\n                    let a = m.reg(3).wrapping_add(0x00000000); let v = m.reg(2); m.store_w(a, v);\n                    pc = 0x1008;"),
            "not-taken branch must resume at pc+8 after the inlined delay slot, not at the delay slot label:\n{source}"
        );
        assert!(
            !source.contains("} else {\n                    let a = m.reg(3).wrapping_add(0x00000000); let v = m.reg(2); m.store_w(a, v);\n                    pc = 0x1004;"),
            "fallthrough to pc+4 would execute the delay slot twice"
        );
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

    #[test]
    fn emitted_mtc0_passes_its_exact_local_step_without_per_instruction_machine_writes() {
        let mtc0 = |rt: u8, rd: u8| {
            (0x10u32 << 26) | (0x04 << 21) | ((rt as u32) << 16) | ((rd as u32) << 11)
        };
        let src = emit_module(&[mtc0(2, 8), mtc0(3, 9), brk()], 0x1000, "dpc_steps");

        assert!(src.contains("let step_base = m.ctx.steps;"));
        assert_eq!(
            src.matches("RspDpEndStep::new(step_base.saturating_add(steps))")
                .count(),
            2
        );
        assert!(src.contains("m.ctx.steps = step_base.saturating_add(steps);"));
        assert!(!src.contains("m.ctx.steps = steps;"));
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
