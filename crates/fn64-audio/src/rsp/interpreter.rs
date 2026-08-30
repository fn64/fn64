//! General clean-room RSP instruction interpreter.
//!
//! The decoder, scalar semantics, vector operations, and delay-slot rules are
//! the same typed components used by the Rust RSP recompiler. This runner is
//! the universal fallback for an IMEM generation that has no digest-selected
//! HLE or precompiled translation. Encodings and behavior come from the public
//! SGI *Nintendo 64 RSP Programmer's Guide* and MIT Rabbitizer tables; no GPL
//! runtime implementation was consulted.

use super::context::RspExitReason;
use super::decode::{
    decode, AluImmOp, AluRegOp, BranchOp, BranchZOp, Instr, LoadOp, ShiftOp, StoreOp,
};
use super::ops::{dispatch, OpInvocation, OpStatus};
use super::recomp::runtime::{RspDpEndStep, RspMachine};
use super::recomp::{trap_delay_slot_control, trap_imem_overrun, trap_unknown, trap_unknown_vu};
use std::sync::{
    atomic::{AtomicU64, Ordering},
    OnceLock,
};

static EXEC_TRACE_SEQUENCE: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, PartialEq, Eq)]
enum RspExecutionTrace {
    Disabled,
    Enabled {
        instruction_limit: u64,
        gprs: Box<[u8]>,
    },
}

fn rsp_execution_trace() -> &'static RspExecutionTrace {
    static CONFIG: OnceLock<RspExecutionTrace> = OnceLock::new();
    CONFIG.get_or_init(|| {
        let enabled = std::env::var_os("RSP_TRACE_EXEC").is_some();
        if !enabled {
            return RspExecutionTrace::Disabled;
        }
        let raw_limit = std::env::var("RSP_TRACE_EXEC_LIMIT").ok();
        let raw_gprs = std::env::var("RSP_TRACE_EXEC_GPRS").ok();
        parse_rsp_execution_trace(true, raw_limit.as_deref(), raw_gprs.as_deref())
    })
}

fn parse_rsp_execution_trace(
    enabled: bool,
    raw_limit: Option<&str>,
    raw_gprs: Option<&str>,
) -> RspExecutionTrace {
    if !enabled {
        return RspExecutionTrace::Disabled;
    }
    let instruction_limit = raw_limit
        .map(|raw| {
            raw.parse::<u64>()
                .unwrap_or_else(|_| panic!("RSP_TRACE_EXEC_LIMIT must be an integer, got {raw:?}"))
        })
        .unwrap_or(u64::MAX);
    let gprs = raw_gprs
        .map(|raw| {
            raw.split(',')
                .map(|field| {
                    let register = field.trim().parse::<u8>().unwrap_or_else(|_| {
                        panic!(
                            "RSP_TRACE_EXEC_GPRS must be comma-separated register indices, got {raw:?}"
                        )
                    });
                    assert!(
                        register < 32,
                        "RSP_TRACE_EXEC_GPRS register index must be below 32, got {register}"
                    );
                    register
                })
                .collect::<Box<[_]>>()
        })
        .unwrap_or_default();
    RspExecutionTrace::Enabled {
        instruction_limit,
        gprs,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InterpreterResult {
    pub reason: RspExitReason,
    pub pc: u32,
    pub steps: u64,
}

/// Execute one complete 4 KiB IMEM image from `pc` until an architectural
/// exit or the deterministic instruction budget is exhausted.
pub fn run_imem(
    words: &[u32],
    pc: u32,
    machine: &mut RspMachine<'_>,
    step_budget: u64,
) -> InterpreterResult {
    assert!(
        !words.is_empty() && words.len() <= 0x400,
        "RSP IMEM image must contain 1..=1024 words"
    );
    assert!(pc.is_multiple_of(4), "RSP PC {pc:#x} is not word-aligned");
    assert!(step_budget > 0, "RSP interpreter budget must be nonzero");

    // Predecode the whole IMEM image once per call instead of decoding every
    // step. Measured motivation (rt64 lane, 30 s in-process self-time sample):
    // per-step decode() was 2549 self-time samples vs 655 for execute — ~55%
    // of all RSP-interpreter time — because the loop decoded both the current
    // word AND its delay slot on every retired step (~128k-214k decode calls
    // per run_imem entry). This table costs <=1024 decode calls per entry
    // (~5 entries/field), an expected ~2.5-2.8 ms/field recovery. It is exact
    // because every pc this loop ever passes to decode is normalized into the
    // 0x1000..=0x1ffc window, so words[idx] is always decoded at
    // pc = 0x1000 + 4*idx — precisely the table key (see `predecode_imem`).
    let decoded = predecode_imem(words);

    let mut pc = if machine.ctx.resume_address != 0 {
        let resume = 0x1000 | (machine.ctx.resume_address & 0x0fff);
        machine.ctx.resume_address = 0;
        resume
    } else {
        0x1000 | (pc & 0x0fff)
    };
    let step_base = machine.ctx.steps;
    let mut steps = 0u64;
    let trace = match (
        crate::rsp::recomp::content_safe_diagnostics(),
        rsp_execution_trace(),
    ) {
        (
            false,
            RspExecutionTrace::Enabled {
                instruction_limit,
                gprs,
            },
        ) => Some((*instruction_limit, gprs.as_ref())),
        _ => None,
    };

    loop {
        if steps == step_budget {
            machine.ctx.steps = machine.ctx.steps.saturating_add(steps);
            return InterpreterResult {
                reason: RspExitReason::StepLimit,
                pc,
                steps,
            };
        }
        let idx = ((pc & 0x0fff) / 4) as usize;
        let Some(&word) = words.get(idx) else {
            machine.ctx.steps = machine.ctx.steps.saturating_add(steps);
            return InterpreterResult {
                reason: trap_imem_overrun(pc),
                pc,
                steps,
            };
        };
        steps += 1;
        let delay_word = words.get(idx + 1).copied();
        if let Some((trace_limit, trace_gprs)) = trace {
            let sequence = EXEC_TRACE_SEQUENCE.fetch_add(1, Ordering::Relaxed);
            if sequence < trace_limit {
                eprintln!(
                    "[fn64-rsp-exec] seq={} step={steps} pc={pc:#06x} word={word:#010x} \
                     delay={:#010x}",
                    sequence + 1,
                    delay_word.unwrap_or_default(),
                );
                if !trace_gprs.is_empty() {
                    let values = trace_gprs
                        .iter()
                        .map(|&register| (register, machine.reg(register)))
                        .collect::<Vec<_>>();
                    eprintln!("[fn64-rsp-gpr] {values:08x?}");
                }
            }
        }
        // In-bounds: `words.get(idx)` succeeded above and
        // `decoded.len() == words.len()`.
        let instr = decoded[idx];
        // `Some` iff `delay_word` is `Some`, by construction of the table.
        let delay = decoded.get(idx + 1).copied();
        debug_assert_eq!(instr, decode(word, pc));
        debug_assert_eq!(
            delay,
            delay_word.map(|delay_word| decode(delay_word, pc.wrapping_add(4)))
        );

        let reason = match instr {
            Instr::Break => {
                machine.break_rsp();
                Some(RspExitReason::Broke)
            }
            Instr::Nop => {
                pc = next_pc(pc);
                None
            }
            Instr::Lui { rt, imm } => {
                machine.set_reg(rt, (imm as u32) << 16);
                pc = next_pc(pc);
                None
            }
            Instr::AluImm { op, rt, rs, imm } => {
                exec_alu_imm(machine, op, rt, rs, imm);
                pc = next_pc(pc);
                None
            }
            Instr::AluReg { op, rd, rs, rt } => {
                exec_alu_reg(machine, op, rd, rs, rt);
                pc = next_pc(pc);
                None
            }
            Instr::Shift { op, rd, rt, sa } => {
                exec_shift(machine, op, rd, rt, sa as u32, None);
                pc = next_pc(pc);
                None
            }
            Instr::ShiftVar { op, rd, rt, rs } => {
                exec_shift(machine, op, rd, rt, 0, Some(rs));
                pc = next_pc(pc);
                None
            }
            Instr::Load { op, rt, base, off } => {
                exec_load(machine, op, rt, base, off);
                pc = next_pc(pc);
                None
            }
            Instr::Store { op, rt, base, off } => {
                exec_store(machine, op, rt, base, off);
                pc = next_pc(pc);
                None
            }
            Instr::Mfc0 { rt, cop0 } => {
                let value = machine.read_cp0(cop0);
                machine.set_reg(rt, value);
                pc = next_pc(pc);
                None
            }
            Instr::Mtc0 { rt, cop0 } => {
                let value = machine.reg(rt);
                let reason = machine.write_cp0_at_step(
                    cop0,
                    value,
                    RspDpEndStep::new(step_base.saturating_add(steps)),
                );
                if reason.is_some() {
                    machine.ctx.resume_address = next_pc(pc);
                } else {
                    pc = next_pc(pc);
                }
                reason
            }
            Instr::Mfc2 { rt, vs, elem } => {
                let value = machine.mfc2(vs, elem);
                machine.set_reg(rt, value);
                pc = next_pc(pc);
                None
            }
            Instr::Mtc2 { rt, vs, elem } => {
                let value = machine.reg(rt);
                machine.mtc2(vs, elem, value);
                pc = next_pc(pc);
                None
            }
            Instr::Cfc2 { rt, cd } => {
                let value = machine.cfc2(cd);
                machine.set_reg(rt, value);
                pc = next_pc(pc);
                None
            }
            Instr::Ctc2 { rt, cd } => {
                let value = machine.reg(rt);
                machine.ctc2(cd, value);
                pc = next_pc(pc);
                None
            }
            Instr::VLoad {
                op,
                vt,
                elem,
                base,
                off,
            } => {
                let value = machine.reg(base);
                machine.vload(op, vt, elem, value, off);
                pc = next_pc(pc);
                None
            }
            Instr::VStore {
                op,
                vt,
                elem,
                base,
                off,
            } => {
                let value = machine.reg(base);
                machine.vstore(op, vt, elem, value, off);
                pc = next_pc(pc);
                None
            }
            Instr::Vu {
                op,
                vd,
                vs,
                vt,
                e,
                de,
            } => {
                let invocation = OpInvocation {
                    vd: vd as usize,
                    vs: vs as usize,
                    vt: vt as usize,
                    e: e as usize,
                    de: de as usize,
                    vs_index: vs as usize,
                };
                match dispatch(machine.vu(), op, invocation) {
                    OpStatus::Executed => {
                        pc = next_pc(pc);
                        None
                    }
                    OpStatus::Unimplemented(op) => Some(trap_unknown_vu(pc, op)),
                }
            }
            Instr::Branch { op, rs, rt, target } => {
                let taken = match op {
                    BranchOp::Beq => machine.reg(rs) == machine.reg(rt),
                    BranchOp::Bne => machine.reg(rs) != machine.reg(rt),
                };
                let resume = if taken {
                    target as u32
                } else {
                    pc.wrapping_add(8)
                };
                let reason = run_delay(
                    machine,
                    delay,
                    pc.wrapping_add(4),
                    resume,
                    RspDpEndStep::new(step_base.saturating_add(steps)),
                );
                pc = resume;
                reason
            }
            Instr::BranchZ {
                op,
                rs,
                target,
                link,
            } => {
                let value = machine.reg(rs) as i32;
                let taken = match op {
                    BranchZOp::Blez => value <= 0,
                    BranchZOp::Bgtz => value > 0,
                    BranchZOp::Bltz => value < 0,
                    BranchZOp::Bgez => value >= 0,
                };
                if let Some(ret) = link {
                    machine.set_reg(31, ret as u32);
                }
                let resume = if taken {
                    target as u32
                } else {
                    pc.wrapping_add(8)
                };
                let reason = run_delay(
                    machine,
                    delay,
                    pc.wrapping_add(4),
                    resume,
                    RspDpEndStep::new(step_base.saturating_add(steps)),
                );
                pc = resume;
                reason
            }
            Instr::Jump { target } => {
                let resume = target as u32;
                let reason = run_delay(
                    machine,
                    delay,
                    pc.wrapping_add(4),
                    resume,
                    RspDpEndStep::new(step_base.saturating_add(steps)),
                );
                pc = resume;
                reason
            }
            Instr::Jal { target, ret } => {
                machine.set_reg(31, ret as u32);
                let resume = target as u32;
                let reason = run_delay(
                    machine,
                    delay,
                    pc.wrapping_add(4),
                    resume,
                    RspDpEndStep::new(step_base.saturating_add(steps)),
                );
                pc = resume;
                reason
            }
            Instr::Jr { rs } => {
                let resume = 0x1000 | (machine.reg(rs) & 0x0fff);
                let reason = run_delay(
                    machine,
                    delay,
                    pc.wrapping_add(4),
                    resume,
                    RspDpEndStep::new(step_base.saturating_add(steps)),
                );
                pc = resume;
                reason
            }
            Instr::Jalr { rd, rs, ret } => {
                let resume = 0x1000 | (machine.reg(rs) & 0x0fff);
                machine.set_reg(rd, ret as u32);
                let reason = run_delay(
                    machine,
                    delay,
                    pc.wrapping_add(4),
                    resume,
                    RspDpEndStep::new(step_base.saturating_add(steps)),
                );
                pc = resume;
                reason
            }
            Instr::Unknown { word } => Some(trap_unknown(pc, word)),
        };

        if let Some(reason) = reason {
            machine.ctx.steps = machine.ctx.steps.saturating_add(steps);
            return InterpreterResult { reason, pc, steps };
        }
    }
}

/// Decode one complete IMEM image into a table with one [`Instr`] per word:
/// entry `i` holds `decode(words[i], 0x1000 + 4 * i as u32)` — exactly the
/// `(word, pc)` pair the step loop passed to `decode` when it decoded per
/// step, because [`run_imem`] normalizes every pc into the 0x1000..=0x1ffc
/// window before decoding (entry pc, `next_pc`, decoded branch/jump targets,
/// and the `Jr`/`Jalr` masking all preserve it).
///
/// INVARIANT this relies on: `decode(word, pc)` is a pure function of
/// `(word, pc & 0x0fff)` — pc feeds only `branch_target()` and
/// `link_address()`, both of which mask through `& 0x0FFF` after an add.
/// Pinned by the pc-window lemma test in
/// `tests/rsp_predecode_equivalence.rs`; the `debug_assert_eq!` pair in the
/// step loop re-checks full equivalence on every debug-build run.
///
/// `pub` (rather than `pub(crate)`) solely so the integration-test crate can
/// exercise table/per-step equivalence directly; it is also the seam where a
/// digest-keyed cross-call cache would slot in if a measurement ever
/// justifies one.
pub fn predecode_imem(words: &[u32]) -> Vec<Instr> {
    words
        .iter()
        .enumerate()
        .map(|(i, &word)| decode(word, 0x1000 + (i as u32) * 4))
        .collect()
}

fn next_pc(pc: u32) -> u32 {
    0x1000 | (pc.wrapping_add(4) & 0x0fff)
}

fn run_delay(
    machine: &mut RspMachine<'_>,
    delay: Option<Instr>,
    delay_pc: u32,
    resume_pc: u32,
    step: RspDpEndStep,
) -> Option<RspExitReason> {
    match delay {
        None | Some(Instr::Nop) => None,
        Some(Instr::AluImm { op, rt, rs, imm }) => {
            exec_alu_imm(machine, op, rt, rs, imm);
            None
        }
        Some(Instr::AluReg { op, rd, rs, rt }) => {
            exec_alu_reg(machine, op, rd, rs, rt);
            None
        }
        Some(Instr::Shift { op, rd, rt, sa }) => {
            exec_shift(machine, op, rd, rt, sa as u32, None);
            None
        }
        Some(Instr::ShiftVar { op, rd, rt, rs }) => {
            exec_shift(machine, op, rd, rt, 0, Some(rs));
            None
        }
        Some(Instr::Lui { rt, imm }) => {
            machine.set_reg(rt, (imm as u32) << 16);
            None
        }
        Some(Instr::Load { op, rt, base, off }) => {
            exec_load(machine, op, rt, base, off);
            None
        }
        Some(Instr::Store { op, rt, base, off }) => {
            exec_store(machine, op, rt, base, off);
            None
        }
        Some(Instr::Mfc0 { rt, cop0 }) => {
            let value = machine.read_cp0(cop0);
            machine.set_reg(rt, value);
            None
        }
        Some(Instr::Mtc0 { rt, cop0 }) => {
            let value = machine.reg(rt);
            let reason = machine.write_cp0_at_step(cop0, value, step);
            if reason.is_some() {
                machine.ctx.resume_address = resume_pc;
            }
            reason
        }
        Some(Instr::Mfc2 { rt, vs, elem }) => {
            let value = machine.mfc2(vs, elem);
            machine.set_reg(rt, value);
            None
        }
        Some(Instr::Mtc2 { rt, vs, elem }) => {
            let value = machine.reg(rt);
            machine.mtc2(vs, elem, value);
            None
        }
        Some(Instr::Cfc2 { rt, cd }) => {
            let value = machine.cfc2(cd);
            machine.set_reg(rt, value);
            None
        }
        Some(Instr::Ctc2 { rt, cd }) => {
            let value = machine.reg(rt);
            machine.ctc2(cd, value);
            None
        }
        Some(Instr::VLoad {
            op,
            vt,
            elem,
            base,
            off,
        }) => {
            let value = machine.reg(base);
            machine.vload(op, vt, elem, value, off);
            None
        }
        Some(Instr::VStore {
            op,
            vt,
            elem,
            base,
            off,
        }) => {
            let value = machine.reg(base);
            machine.vstore(op, vt, elem, value, off);
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
            match dispatch(machine.vu(), op, invocation) {
                OpStatus::Executed => None,
                OpStatus::Unimplemented(op) => Some(trap_unknown_vu(delay_pc, op)),
            }
        }
        Some(Instr::Break) => {
            machine.break_rsp();
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

fn exec_alu_imm(machine: &mut RspMachine<'_>, op: AluImmOp, rt: u8, rs: u8, imm: u16) {
    let signed = imm as i16 as i32 as u32;
    let value = match op {
        AluImmOp::Addi | AluImmOp::Addiu => machine.reg(rs).wrapping_add(signed),
        AluImmOp::Andi => machine.reg(rs) & imm as u32,
        AluImmOp::Ori => machine.reg(rs) | imm as u32,
        AluImmOp::Xori => machine.reg(rs) ^ imm as u32,
        AluImmOp::Slti => ((machine.reg(rs) as i32) < imm as i16 as i32) as u32,
        AluImmOp::Sltiu => (machine.reg(rs) < signed) as u32,
    };
    machine.set_reg(rt, value);
}

fn exec_alu_reg(machine: &mut RspMachine<'_>, op: AluRegOp, rd: u8, rs: u8, rt: u8) {
    let value = match op {
        AluRegOp::Add | AluRegOp::Addu => machine.reg(rs).wrapping_add(machine.reg(rt)),
        AluRegOp::Sub | AluRegOp::Subu => machine.reg(rs).wrapping_sub(machine.reg(rt)),
        AluRegOp::And => machine.reg(rs) & machine.reg(rt),
        AluRegOp::Or => machine.reg(rs) | machine.reg(rt),
        AluRegOp::Xor => machine.reg(rs) ^ machine.reg(rt),
        AluRegOp::Nor => !(machine.reg(rs) | machine.reg(rt)),
        AluRegOp::Slt => ((machine.reg(rs) as i32) < (machine.reg(rt) as i32)) as u32,
        AluRegOp::Sltu => (machine.reg(rs) < machine.reg(rt)) as u32,
    };
    machine.set_reg(rd, value);
}

fn exec_shift(
    machine: &mut RspMachine<'_>,
    op: ShiftOp,
    rd: u8,
    rt: u8,
    immediate: u32,
    variable: Option<u8>,
) {
    let amount = variable.map_or(immediate, |reg| machine.reg(reg) & 31);
    let value = match op {
        ShiftOp::Sll => machine.reg(rt) << amount,
        ShiftOp::Srl => machine.reg(rt) >> amount,
        ShiftOp::Sra => ((machine.reg(rt) as i32) >> amount) as u32,
    };
    machine.set_reg(rd, value);
}

fn exec_load(machine: &mut RspMachine<'_>, op: LoadOp, rt: u8, base: u8, off: i16) {
    let addr = machine.reg(base).wrapping_add(off as i32 as u32);
    let value = match op {
        LoadOp::Lb => machine.load_b(addr),
        LoadOp::Lbu => machine.load_bu(addr),
        LoadOp::Lh => machine.load_h(addr),
        LoadOp::Lhu => machine.load_hu(addr),
        LoadOp::Lw => machine.load_w(addr),
    };
    machine.set_reg(rt, value);
}

fn exec_store(machine: &mut RspMachine<'_>, op: StoreOp, rt: u8, base: u8, off: i16) {
    let addr = machine.reg(base).wrapping_add(off as i32 as u32);
    let value = machine.reg(rt);
    match op {
        StoreOp::Sb => machine.store_b(addr, value),
        StoreOp::Sh => machine.store_h(addr, value),
        StoreOp::Sw => machine.store_w(addr, value),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disabled_execution_trace_ignores_dependent_configuration() {
        assert_eq!(
            parse_rsp_execution_trace(false, Some("not-an-integer"), Some("99,wat")),
            RspExecutionTrace::Disabled
        );
    }

    #[test]
    fn enabled_execution_trace_parses_limit_and_registers_once() {
        assert_eq!(
            parse_rsp_execution_trace(true, Some("17"), Some("1, 8,31")),
            RspExecutionTrace::Enabled {
                instruction_limit: 17,
                gprs: Box::from([1, 8, 31]),
            }
        );
    }

    #[test]
    #[should_panic(expected = "RSP_TRACE_EXEC_GPRS register index must be below 32")]
    fn enabled_execution_trace_rejects_out_of_range_registers() {
        let _ = parse_rsp_execution_trace(true, None, Some("32"));
    }

    #[test]
    fn executes_scalar_delay_slot_and_break_from_arbitrary_pc() {
        let mut words = vec![0u32; 0x400];
        // addiu r2,r0,1; beq r0,r0,+1; addiu r2,r2,2; break
        words[0x20] = (0x09 << 26) | (2 << 16) | 1;
        words[0x21] = (0x04 << 26) | 1;
        words[0x22] = (0x09 << 26) | (2 << 21) | (2 << 16) | 2;
        words[0x23] = 0x0000_000d;
        let mut rdram = vec![0u8; 0x100];
        let mut machine = RspMachine::new(&mut rdram);

        let result = run_imem(&words, 0x80, &mut machine, 20);
        assert_eq!(result.reason, RspExitReason::Broke);
        assert_eq!(machine.reg(2), 3);
        assert_eq!(result.steps, 3);
    }

    #[test]
    fn budget_exit_preserves_pc_for_deterministic_resume() {
        let words = vec![0u32; 0x400];
        let mut rdram = vec![0u8; 0x100];
        let mut machine = RspMachine::new(&mut rdram);
        let result = run_imem(&words, 0, &mut machine, 3);
        assert_eq!(result.reason, RspExitReason::StepLimit);
        assert_eq!(result.pc, 0x100c);
        assert_eq!(result.steps, 3);
    }

    #[test]
    fn dpc_submission_retains_the_exact_interpreter_dp_end_step() {
        let addiu = |rt: u32, imm: u16| (0x09 << 26) | (rt << 16) | u32::from(imm);
        let mtc0 = |rt: u32, cop0: u32| (0x10 << 26) | (0x04 << 21) | (rt << 16) | (cop0 << 11);
        let words = [
            addiu(2, 0x20),
            mtc0(2, 8),
            addiu(2, 0x40),
            mtc0(2, 9),
            0x0000_000d,
        ];
        let mut rdram = vec![0u8; 0x100];
        let mut machine = RspMachine::new(&mut rdram);
        machine.ctx.steps = 7;

        let result = run_imem(&words, 0, &mut machine, 20);

        assert_eq!(result.reason, RspExitReason::Broke);
        assert_eq!(result.steps, 5);
        assert_eq!(machine.ctx.steps, 12);
        let submissions = machine.take_dp_submissions();
        assert_eq!(submissions.len(), 1);
        assert_eq!(submissions[0].dp_end_step(), Some(RspDpEndStep::new(11)));
    }
}
