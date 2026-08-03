//! Straight-line op emission: register accessors, FPU arithmetic and
//! compares, guards, exceptions, memory faults, and ll/sc -- split from
//! emit.rs, which keeps runner scaffolding and control transfer.
use super::*;

pub(super) fn r(idx: Reg) -> String {
    if idx == 0 {
        "0i64 as u64".to_string()
    } else {
        format!("ctx.r({})", idx)
    }
}

/// Register read as `i32` (low word, signed).
pub(super) fn rs32(idx: Reg) -> String {
    if idx == 0 {
        "0i32".to_string()
    } else {
        format!("ctx.r_s32({})", idx)
    }
}

/// Register read as `u32` (low word, unsigned).
pub(super) fn ru32(idx: Reg) -> String {
    if idx == 0 {
        "0u32".to_string()
    } else {
        format!("ctx.r_u32({})", idx)
    }
}

/// Register read as `i64` (full register, signed) — the `SIGNED(reg)`/`ToS64`
/// operand for SLT/SLTI and the single-operand branches, which MIPS III
/// evaluates on the whole 64-bit register.
pub(super) fn rs64(idx: Reg) -> String {
    if idx == 0 {
        "0i64".to_string()
    } else {
        format!("ctx.r_s64({})", idx)
    }
}

/// Register read as `u64` (full register, unsigned) — the `ToU64` operand for
/// SLTU/SLTIU.
pub(super) fn ru64(idx: Reg) -> String {
    if idx == 0 {
        "0u64".to_string()
    } else {
        format!("ctx.r_u64({})", idx)
    }
}

pub(super) fn emit_fpu_i32(
    out: &mut String,
    mem_fault: MemFault,
    fd: Reg,
    fs: Reg,
    single: bool,
    mode: Option<u8>,
) {
    let suffix = if single { "s" } else { "d" };
    let mode = mode.map_or_else(|| "None".to_string(), |m| format!("Some({})", m));
    match mem_fault {
        MemFault::Panic => {
            let _ = writeln!(out, "            {{ let r = ctx.fpu_to_i32_{suffix}({fs}, {mode}); ctx.set_f_bits({fd}, r as u32); }}");
        }
        MemFault::Fault { .. } => {
            let finish = mem_fault.fpu_exception_finish();
            let _ = writeln!(out, "            {{ let r = match ctx.try_fpu_to_i32_{suffix}({fs}, {mode}) {{ Ok(value) => value, Err(_) => {{ {finish} }} }}; ctx.set_f_bits({fd}, r as u32); }}");
        }
    }
}

pub(super) fn emit_fpu_i64(
    out: &mut String,
    mem_fault: MemFault,
    fd: Reg,
    fs: Reg,
    single: bool,
    mode: Option<u8>,
) {
    let suffix = if single { "s" } else { "d" };
    let mode = mode.map_or_else(|| "None".to_string(), |m| format!("Some({})", m));
    match mem_fault {
        MemFault::Panic => {
            let _ = writeln!(out, "            {{ let r = ctx.fpu_to_i64_{suffix}({fs}, {mode}); ctx.set_d_bits({fd}, r as u64); }}");
        }
        MemFault::Fault { .. } => {
            let finish = mem_fault.fpu_exception_finish();
            let _ = writeln!(out, "            {{ let r = match ctx.try_fpu_to_i64_{suffix}({fs}, {mode}) {{ Ok(value) => value, Err(_) => {{ {finish} }} }}; ctx.set_d_bits({fd}, r as u64); }}");
        }
    }
}

pub(super) fn emit_fixed_to_float(
    out: &mut String,
    mem_fault: MemFault,
    fd: Reg,
    fs: Reg,
    destination: char,
    source: char,
) {
    let destination_lower = destination.to_ascii_lowercase();
    let source_lower = source.to_ascii_lowercase();
    let setter = if destination == 'S' {
        "set_f_bits"
    } else {
        "set_d_bits"
    };
    match mem_fault {
        MemFault::Panic => {
            let _ = writeln!(
                out,
                "            {{ let r = ctx.cvt_{destination_lower}_{source_lower}_bits({fs}); ctx.{setter}({fd}, r); }}"
            );
        }
        MemFault::Fault { .. } => {
            let finish = mem_fault.fpu_exception_finish();
            let _ = writeln!(
                out,
                "            {{ let r = match ctx.try_cvt_{destination_lower}_{source_lower}_bits({fs}) {{ Ok(value) => value, Err(_) => {{ {finish} }} }}; ctx.{setter}({fd}, r); }}"
            );
        }
    }
}

pub(super) fn emit_float_to_float(
    out: &mut String,
    mem_fault: MemFault,
    fd: Reg,
    fs: Reg,
    destination: char,
    source: char,
) {
    let destination_lower = destination.to_ascii_lowercase();
    let source_lower = source.to_ascii_lowercase();
    let setter = if destination == 'S' {
        "set_f_bits"
    } else {
        "set_d_bits"
    };
    match mem_fault {
        MemFault::Panic => {
            let _ = writeln!(
                out,
                "            {{ let r = ctx.cvt_{destination_lower}_{source_lower}_bits({fs}); ctx.{setter}({fd}, r); }}"
            );
        }
        MemFault::Fault { .. } => {
            let finish = mem_fault.fpu_exception_finish();
            let _ = writeln!(
                out,
                "            {{ let r = match ctx.try_cvt_{destination_lower}_{source_lower}_bits({fs}) {{ Ok(value) => value, Err(_) => {{ {finish} }} }}; ctx.{setter}({fd}, r); }}"
            );
        }
    }
}

pub(super) fn emit_fpu_compare(
    out: &mut String,
    mem_fault: MemFault,
    single: bool,
    fs: Reg,
    ft: Reg,
    cond: u8,
) {
    let suffix = if single { "s" } else { "d" };
    match mem_fault {
        MemFault::Panic => {
            let _ = writeln!(
                out,
                "            ctx.fpu_compare_{suffix}({fs}, {ft}, {cond});"
            );
        }
        MemFault::Fault { .. } => {
            let finish = mem_fault.fpu_exception_finish();
            let _ = writeln!(
                out,
                "            if ctx.try_fpu_compare_{suffix}({fs}, {ft}, {cond}).is_err() {{ {finish} }}"
            );
        }
    }
}

/// Emit the shared kernel-or-Status.CU0 check before any COP0-visible effect.
/// A COP0 branch is guarded before its delay slot; a COP0 instruction in
/// another branch's delay slot retains the branch EPC and sets Cause.BD.
pub(super) fn emit_bank_cop0_guard(
    out: &mut String,
    instr: Instruction,
    fault_vram: u32,
    epc: u32,
    branch_delay: bool,
    bank: BankId,
    already_counted: bool,
) {
    if !instr.requires_cop0() {
        return;
    }
    let _ = writeln!(out, "            if !ctx.cop0_usable() {{");
    if !already_counted {
        let _ = writeln!(out, "                executed += 1;");
    }
    let _ = writeln!(out, "                finish!(BlockExit::Fault(CpuFault {{");
    let _ = writeln!(out, "                    at: ExecutionKey::new(BankId::new({:#018X}), GuestPc::new({fault_vram:#010X})),", bank.get());
    let _ = writeln!(out, "                    kind: CpuFaultKind::Exception {{");
    let _ = writeln!(
        out,
        "                        exception: CpuException::CoprocessorUnusable,"
    );
    let _ = writeln!(
        out,
        "                        epc: GuestPc::new({epc:#010X}),"
    );
    let _ = writeln!(out, "                        branch_delay: {branch_delay},");
    let _ = writeln!(out, "                        instruction_code: 0,");
    let _ = writeln!(out, "                        bad_vaddr: None,");
    let _ = writeln!(out, "                        coprocessor: Some(0),");
    let _ = writeln!(out, "                    }},");
    let _ = writeln!(out, "                }}));");
    let _ = writeln!(out, "            }}");
}

/// Emit the Status.CU1 check before any COP1-visible effect. A branch checks
/// before its delay slot; a COP1 delay instruction checks after the branch has
/// retired and therefore reports the branch EPC with Cause.BD set.
pub(super) fn emit_bank_cop1_guard(
    out: &mut String,
    instr: Instruction,
    fault_vram: u32,
    epc: u32,
    branch_delay: bool,
    bank: BankId,
    already_counted: bool,
) {
    if !instr.requires_cop1() {
        return;
    }
    let _ = writeln!(out, "            if ctx.cop0_status & (1 << 29) == 0 {{");
    if !already_counted {
        let _ = writeln!(out, "                executed += 1;");
    }
    let _ = writeln!(out, "                finish!(BlockExit::Fault(CpuFault {{");
    let _ = writeln!(out, "                    at: ExecutionKey::new(BankId::new({:#018X}), GuestPc::new({fault_vram:#010X})),", bank.get());
    let _ = writeln!(out, "                    kind: CpuFaultKind::Exception {{");
    let _ = writeln!(
        out,
        "                        exception: CpuException::CoprocessorUnusable,"
    );
    let _ = writeln!(
        out,
        "                        epc: GuestPc::new({epc:#010X}),"
    );
    let _ = writeln!(out, "                        branch_delay: {branch_delay},");
    let _ = writeln!(out, "                        instruction_code: 0,");
    let _ = writeln!(out, "                        bad_vaddr: None,");
    let _ = writeln!(out, "                        coprocessor: Some(1),");
    let _ = writeln!(out, "                    }},");
    let _ = writeln!(out, "                }}));");
    let _ = writeln!(out, "            }}");
}

pub(super) fn emit_bank_overflow(
    out: &mut String,
    instr: Instruction,
    fault_vram: u32,
    epc: u32,
    branch_delay: bool,
    bank: BankId,
    already_counted: bool,
) -> bool {
    use Instruction::*;
    let (result, write) = match instr {
        Addi { rt, rs, imm } => (
            format!("({}).checked_add({})", rs32(rs), imm as i32),
            format!("ctx.set_r32({rt}, value);"),
        ),
        Add { rd, rs, rt } => (
            format!("({}).checked_add({})", rs32(rs), rs32(rt)),
            format!("ctx.set_r32({rd}, value);"),
        ),
        Sub { rd, rs, rt } => (
            format!("({}).checked_sub({})", rs32(rs), rs32(rt)),
            format!("ctx.set_r32({rd}, value);"),
        ),
        Daddi { rt, rs, imm } => (
            format!("({}).checked_add({}i64)", rs64(rs), imm as i64),
            format!("ctx.set_r({rt}, value as u64);"),
        ),
        Dadd { rd, rs, rt } => (
            format!("({}).checked_add({})", rs64(rs), rs64(rt)),
            format!("ctx.set_r({rd}, value as u64);"),
        ),
        Dsub { rd, rs, rt } => (
            format!("({}).checked_sub({})", rs64(rs), rs64(rt)),
            format!("ctx.set_r({rd}, value as u64);"),
        ),
        _ => return false,
    };
    let _ = writeln!(out, "            if let Some(value) = {result} {{");
    let _ = writeln!(out, "                {write}");
    let _ = writeln!(out, "            }} else {{");
    if !already_counted {
        let _ = writeln!(out, "                executed += 1;");
    }
    let _ = writeln!(out, "                finish!(BlockExit::Fault(CpuFault {{");
    let _ = writeln!(out, "                    at: ExecutionKey::new(BankId::new({:#018X}), GuestPc::new({fault_vram:#010X})),", bank.get());
    let _ = writeln!(out, "                    kind: CpuFaultKind::Exception {{");
    let _ = writeln!(
        out,
        "                        exception: CpuException::IntegerOverflow,"
    );
    let _ = writeln!(
        out,
        "                        epc: GuestPc::new({epc:#010X}),"
    );
    let _ = writeln!(out, "                        branch_delay: {branch_delay},");
    let _ = writeln!(out, "                        instruction_code: 0,");
    let _ = writeln!(out, "                        bad_vaddr: None,");
    let _ = writeln!(out, "                        coprocessor: None,");
    let _ = writeln!(out, "                    }},");
    let _ = writeln!(out, "                }}));");
    let _ = writeln!(out, "            }}");
    true
}

/// Emit the alignment checks that architecturally precede aligned memory
/// operations in the arbitrary-PC lane. The effective address is sampled
/// before any destination, memory, or LLbit state can change.
pub(super) fn emit_bank_address_exception(
    out: &mut String,
    instr: Instruction,
    fault_vram: u32,
    epc: u32,
    branch_delay: bool,
    _bank: BankId,
    already_counted: bool,
) -> bool {
    use Instruction::*;
    let (base, off, alignment, exception) = match instr {
        Lh { base, off, .. } | Lhu { base, off, .. } => (base, off, 2u32, "AddressErrorLoad"),
        Lw { base, off, .. }
        | Lwu { base, off, .. }
        | Ll { base, off, .. }
        | Lwc1 { base, off, .. } => (base, off, 4, "AddressErrorLoad"),
        Ld { base, off, .. } | Lld { base, off, .. } | Ldc1 { base, off, .. } => {
            (base, off, 8, "AddressErrorLoad")
        }
        Sh { base, off, .. } => (base, off, 2, "AddressErrorStore"),
        Sw { base, off, .. } | Sc { base, off, .. } | Swc1 { base, off, .. } => {
            (base, off, 4, "AddressErrorStore")
        }
        Sd { base, off, .. } | Scd { base, off, .. } | Sdc1 { base, off, .. } => {
            (base, off, 8, "AddressErrorStore")
        }
        _ => return false,
    };
    let _ = writeln!(
        out,
        "            let effective_address = Rdram::eff_addr({}, {});",
        r(base),
        off
    );
    let _ = writeln!(
        out,
        "            if effective_address & {:#010X} != 0 {{",
        alignment - 1
    );
    if !already_counted {
        let _ = writeln!(out, "                executed += 1;");
    }
    let site = if branch_delay {
        format!("FaultSite::delay(expected_bank, {fault_vram:#010X}, {epc:#010X})")
    } else {
        format!("FaultSite::straight(expected_bank, {fault_vram:#010X})")
    };
    let kind = match exception {
        "AddressErrorLoad" => "DataAccessKind::Load",
        "AddressErrorStore" => "DataAccessKind::Store",
        _ => unreachable!("aligned memory operations only raise load/store address errors"),
    };
    let _ = writeln!(
        out,
        "                finish!(address_error({site}, {kind}, effective_address));"
    );
    let _ = writeln!(out, "            }}");
    emit_straight(
        out,
        instr,
        fault_vram,
        &MemFault::Fault {
            pc: fault_vram,
            epc,
            branch_delay,
            retired: if branch_delay {
                "(executed - 2)"
            } else {
                "executed"
            },
        },
    );
    true
}

/// ERET is a privileged transfer without a delay slot. The arbitrary-PC lane
/// can express it directly as a resolved transfer after applying CP0/LLbit
/// state; whole-function output retains its host-boundary trap because it has
/// no typed transfer return.
pub(super) fn emit_bank_eret(out: &mut String, instr: Instruction, bank: BankId) -> bool {
    if !matches!(instr, Instruction::Eret) {
        return false;
    }
    let _ = writeln!(out, "            executed += 1;");
    let _ = writeln!(out, "            let target = ctx.exception_return_pc();");
    let _ = writeln!(out, "            ctx.advance_cop0_random(1);");
    let _ = writeln!(out, "            finish!(BlockExit::ResolveTransfer {{");
    let _ = writeln!(
        out,
        "                source_bank: BankId::new({:#018X}),",
        bank.get()
    );
    let _ = writeln!(out, "                target_pc: GuestPc::new(target),");
    let _ = writeln!(out, "            }});");
    true
}

/// Emit a synchronous architectural exception for an arbitrary-PC runner.
/// Whole-function output retains its historical loud panic until it also has
/// an exception-return ABI; the block lane can preserve exact bank/PC/EPC/BD
/// context in its existing typed `BlockExit::Fault` boundary today.
pub(super) fn emit_bank_exception(
    out: &mut String,
    instr: Instruction,
    fault_vram: u32,
    epc: u32,
    branch_delay: bool,
    bank: BankId,
    already_counted: bool,
) -> bool {
    use Instruction::*;
    let (condition, exception, code) = match instr {
        Syscall { code } => (None, "Syscall", code),
        Break { code } => (None, "Breakpoint", code),
        Unknown { .. } => (None, "ReservedInstruction", 0),
        Tge { rs, rt, code } => (
            Some(format!("{} >= {}", rs64(rs), rs64(rt))),
            "Trap",
            code as u32,
        ),
        Tgeu { rs, rt, code } => (
            Some(format!("{} >= {}", ru64(rs), ru64(rt))),
            "Trap",
            code as u32,
        ),
        Tlt { rs, rt, code } => (
            Some(format!("{} < {}", rs64(rs), rs64(rt))),
            "Trap",
            code as u32,
        ),
        Tltu { rs, rt, code } => (
            Some(format!("{} < {}", ru64(rs), ru64(rt))),
            "Trap",
            code as u32,
        ),
        Teq { rs, rt, code } => (Some(format!("{} == {}", r(rs), r(rt))), "Trap", code as u32),
        Tne { rs, rt, code } => (Some(format!("{} != {}", r(rs), r(rt))), "Trap", code as u32),
        Tgei { rs, imm } => (
            Some(format!("{} >= {}i64", rs64(rs), imm as i64)),
            "Trap",
            0,
        ),
        Tgeiu { rs, imm } => (
            Some(format!("{} >= {}u64", ru64(rs), imm as i64 as u64)),
            "Trap",
            0,
        ),
        Tlti { rs, imm } => (Some(format!("{} < {}i64", rs64(rs), imm as i64)), "Trap", 0),
        Tltiu { rs, imm } => (
            Some(format!("{} < {}u64", ru64(rs), imm as i64 as u64)),
            "Trap",
            0,
        ),
        Teqi { rs, imm } => (
            Some(format!("{} == {}i64", rs64(rs), imm as i64)),
            "Trap",
            0,
        ),
        Tnei { rs, imm } => (
            Some(format!("{} != {}i64", rs64(rs), imm as i64)),
            "Trap",
            0,
        ),
        _ => return false,
    };
    if let Some(condition) = &condition {
        let _ = writeln!(out, "            if {condition} {{");
    }
    let pad = if condition.is_some() { "    " } else { "" };
    if !already_counted {
        let _ = writeln!(out, "            {pad}executed += 1;");
    }
    let _ = writeln!(out, "            {pad}finish!(BlockExit::Fault(CpuFault {{");
    let _ = writeln!(out, "            {pad}    at: ExecutionKey::new(BankId::new({:#018X}), GuestPc::new({fault_vram:#010X})),", bank.get());
    let _ = writeln!(out, "            {pad}    kind: CpuFaultKind::Exception {{");
    let _ = writeln!(
        out,
        "            {pad}        exception: CpuException::{exception},"
    );
    let _ = writeln!(
        out,
        "            {pad}        epc: GuestPc::new({epc:#010X}),"
    );
    let _ = writeln!(
        out,
        "            {pad}        branch_delay: {branch_delay},"
    );
    let _ = writeln!(
        out,
        "            {pad}        instruction_code: {code:#010X},"
    );
    let _ = writeln!(out, "            {pad}        bad_vaddr: None,");
    let _ = writeln!(out, "            {pad}        coprocessor: None,");
    let _ = writeln!(out, "            {pad}    }},");
    let _ = writeln!(out, "            {pad}}}));");
    if condition.is_some() {
        let _ = writeln!(out, "            }}");
    }
    true
}

pub(super) fn emit_trap(out: &mut String, condition: &str, mnemonic: &str, code: u16) {
    let _ = writeln!(
        out,
        "            if {} {{ panic!(\"MIPS {} trap (code {:#X})\"); }}",
        condition, mnemonic, code
    );
}

pub(super) fn emit_data_control_word(out: &mut String, vram: u32) {
    let _ = writeln!(
        out,
        "            panic!(\"control transfer at {vram:#010X} has no admitted delay slot or is architecturally UNPREDICTABLE in a delay slot\");"
    );
}

/// Selects the historical panicking memory boundary for whole-function output
/// or the typed out-of-range boundary required by arbitrary-PC runners.
#[derive(Clone, Copy)]
pub(super) enum MemFault {
    Panic,
    Fault {
        pc: u32,
        epc: u32,
        branch_delay: bool,
        retired: &'static str,
    },
}

impl MemFault {
    fn site(self) -> String {
        match self {
            Self::Panic => unreachable!("panicking memory accesses do not have a typed fault site"),
            Self::Fault {
                pc,
                epc,
                branch_delay,
                ..
            } => {
                if branch_delay {
                    format!("FaultSite::delay(expected_bank, {pc:#010X}, {epc:#010X})")
                } else {
                    format!("FaultSite::straight(expected_bank, {pc:#010X})")
                }
            }
        }
    }

    fn finish(self) -> String {
        match self {
            Self::Panic => unreachable!("panicking memory accesses do not emit a typed fault"),
            Self::Fault { retired, .. } => format!(
                "return finish_data_access_error(__fa, {}, executed, {retired});",
                self.site()
            ),
        }
    }

    fn fpu_exception_finish(self) -> String {
        match self {
            Self::Panic => "fn64_recomp_rs::trap_unsupported(\"enabled FCSR cause written by CTC1 in whole-function lane\");".to_string(),
            Self::Fault {
                pc,
                epc,
                branch_delay,
                ..
            } => {
                let count = if branch_delay { "" } else { "executed += 1; " };
                format!(
                    "{count}finish!(BlockExit::Fault(CpuFault {{ at: ExecutionKey::new(expected_bank, GuestPc::new({pc:#010X})), kind: CpuFaultKind::Exception {{ exception: CpuException::FloatingPoint, epc: GuestPc::new({epc:#010X}), branch_delay: {branch_delay}, instruction_code: 0, bad_vaddr: None, coprocessor: None }} }}));"
                )
            }
        }
    }

    fn store(self, out: &mut String, unchecked: &str, checked: &str) {
        match self {
            Self::Panic => {
                let _ = writeln!(out, "            {unchecked}");
            }
            Self::Fault { .. } => {
                let _ = writeln!(
                    out,
                    "            if let Err(__fa) = {checked} {{ {} }}",
                    self.finish()
                );
            }
        }
    }

    fn load(
        self,
        out: &mut String,
        unchecked: &str,
        checked: &str,
        consume: impl FnOnce(&str) -> String,
    ) {
        match self {
            Self::Panic => {
                let _ = writeln!(out, "            {}", consume(unchecked));
            }
            Self::Fault { .. } => {
                let _ = writeln!(
                    out,
                    "            let __mv = match {checked} {{ Ok(value) => value, Err(__fa) => {{ {} }} }}; {}",
                    self.finish(),
                    consume("__mv")
                );
            }
        }
    }
}

pub(super) fn emit_ll(out: &mut String, mem_fault: MemFault, rt: Reg, base: Reg, off: i16) {
    let addr = format!("Rdram::eff_addr({}, {})", r(base), off);
    match mem_fault {
        MemFault::Panic => {
            let _ = writeln!(out, "            {{ let addr = {addr}; let value = mem.load_w(addr); ctx.set_r32({rt}, value); ctx.set_ll_reservation(addr, 4); }}");
        }
        MemFault::Fault { .. } => {
            let _ = writeln!(out, "            {{ let addr = {addr}; let value = match mem.try_load_w_translated(ctx, addr) {{ Ok(value) => value, Err(__fa) => {{ {} }} }}; ctx.set_r32({rt}, value); ctx.set_ll_reservation(addr, 4); }}", mem_fault.finish());
        }
    }
}

pub(super) fn emit_lld(out: &mut String, mem_fault: MemFault, rt: Reg, base: Reg, off: i16) {
    let addr = format!("Rdram::eff_addr({}, {})", r(base), off);
    match mem_fault {
        MemFault::Panic => {
            let _ = writeln!(out, "            {{ let addr = {addr}; let value = mem.load_d(addr); ctx.set_r({rt}, value); ctx.set_ll_reservation(addr, 8); }}");
        }
        MemFault::Fault { .. } => {
            let _ = writeln!(out, "            {{ let addr = {addr}; let value = match mem.try_load_d_translated(ctx, addr) {{ Ok(value) => value, Err(__fa) => {{ {} }} }}; ctx.set_r({rt}, value); ctx.set_ll_reservation(addr, 8); }}", mem_fault.finish());
        }
    }
}

pub(super) fn emit_sc(out: &mut String, mem_fault: MemFault, rt: Reg, base: Reg, off: i16, double: bool) {
    let addr = format!("Rdram::eff_addr({}, {})", r(base), off);
    let (value, width, store, checked_store) = if double {
        (ru64(rt), 8, "store_d", "try_store_d_translated")
    } else {
        (ru32(rt), 4, "store_w", "try_store_w_translated")
    };
    match mem_fault {
        MemFault::Panic => {
            let _ = writeln!(out, "            {{ let addr = {addr}; let value = {value}; if ctx.take_ll_reservation(addr, {width}) {{ mem.{store}(addr, value); ctx.set_r({rt}, 1); }} else {{ ctx.set_r({rt}, 0); }} }}");
        }
        MemFault::Fault { .. } => {
            let finish = mem_fault.finish();
            let _ = writeln!(out, "            {{ let addr = {addr}; let value = {value}; if let Err(__fa) = Rdram::check_store_translation(ctx, addr) {{ {finish} }} if ctx.take_ll_reservation(addr, {width}) {{ if let Err(__fa) = mem.{checked_store}(ctx, addr, value) {{ {finish} }} ctx.set_r({rt}, 1); }} else {{ ctx.set_r({rt}, 0); }} }}");
        }
    }
}

/// Wrap a `ctx.fpu_*(...)` arithmetic call (which returns `true` on an enabled
/// FP exception) for the whole-function / straight-line lane, which has no
/// exception-return ABI: a trap panics loudly, mirroring the
/// `.expect("MIPS ADD integer overflow")` shape the integer arithmetic uses.
/// The bank lane never reaches here for these ops — it short-circuits with
/// [`emit_bank_fpu_trap`] to produce a typed `BlockExit::Fault` instead.
pub(super) fn emit_fpu_arith_call(call: &str) -> String {
    format!("if {call} {{ fn64_recomp_rs::trap_unsupported(\"enabled COP1 exception\"); }}")
}

/// If `instr` is a COP1 arithmetic op that can raise an enabled FP exception,
/// emit the bank-lane trap check and return `true` (short-circuiting
/// `emit_straight`, exactly as [`emit_bank_overflow`] does for integer
/// arithmetic). The emitted `ctx.fpu_*` call returns `true` when an enabled
/// exception fired — the FCSR Cause field is written but the destination
/// register and sticky Flags are not — and that turns into a typed ExcCode-15
/// `BlockExit::Fault` carrying the exact EPC/BD.
pub(super) fn emit_bank_fpu_trap(
    out: &mut String,
    instr: Instruction,
    fault_vram: u32,
    epc: u32,
    branch_delay: bool,
    bank: BankId,
    already_counted: bool,
) -> bool {
    use Instruction::*;
    let call = match instr {
        AddS { fd, fs, ft } => format!("ctx.fpu_add_s({fd}, {fs}, {ft})"),
        SubS { fd, fs, ft } => format!("ctx.fpu_sub_s({fd}, {fs}, {ft})"),
        MulS { fd, fs, ft } => format!("ctx.fpu_mul_s({fd}, {fs}, {ft})"),
        DivS { fd, fs, ft } => format!("ctx.fpu_div_s({fd}, {fs}, {ft})"),
        AbsS { fd, fs } => format!("ctx.fpu_abs_s({fd}, {fs})"),
        NegS { fd, fs } => format!("ctx.fpu_neg_s({fd}, {fs})"),
        SqrtS { fd, fs } => format!("ctx.fpu_sqrt_s({fd}, {fs})"),
        AddD { fd, fs, ft } => format!("ctx.fpu_add_d({fd}, {fs}, {ft})"),
        SubD { fd, fs, ft } => format!("ctx.fpu_sub_d({fd}, {fs}, {ft})"),
        MulD { fd, fs, ft } => format!("ctx.fpu_mul_d({fd}, {fs}, {ft})"),
        DivD { fd, fs, ft } => format!("ctx.fpu_div_d({fd}, {fs}, {ft})"),
        AbsD { fd, fs } => format!("ctx.fpu_abs_d({fd}, {fs})"),
        NegD { fd, fs } => format!("ctx.fpu_neg_d({fd}, {fs})"),
        SqrtD { fd, fs } => format!("ctx.fpu_sqrt_d({fd}, {fs})"),
        _ => return false,
    };
    let _ = writeln!(out, "            if {call} {{");
    if !already_counted {
        let _ = writeln!(out, "                executed += 1;");
    }
    let _ = writeln!(out, "                finish!(BlockExit::Fault(CpuFault {{");
    let _ = writeln!(out, "                    at: ExecutionKey::new(BankId::new({:#018X}), GuestPc::new({fault_vram:#010X})),", bank.get());
    let _ = writeln!(out, "                    kind: CpuFaultKind::Exception {{");
    let _ = writeln!(
        out,
        "                        exception: CpuException::FloatingPoint,"
    );
    let _ = writeln!(
        out,
        "                        epc: GuestPc::new({epc:#010X}),"
    );
    let _ = writeln!(out, "                        branch_delay: {branch_delay},");
    let _ = writeln!(out, "                        instruction_code: 0,");
    let _ = writeln!(out, "                        bad_vaddr: None,");
    let _ = writeln!(out, "                        coprocessor: None,");
    let _ = writeln!(out, "                    }},");
    let _ = writeln!(out, "                }}));");
    let _ = writeln!(out, "            }}");
    true
}

/// Emit a straight-line (non-control-transfer) instruction as typed Rust.
pub(super) fn emit_straight(out: &mut String, instr: Instruction, _vram: u32, mem_fault: &MemFault) {
    use Instruction::*;
    let line = |out: &mut String, s: String| {
        let _ = writeln!(out, "            {}", s);
    };
    let unsupported = |out: &mut String, context: String| {
        line(
            out,
            format!("fn64_recomp_rs::trap_unsupported({context:?});"),
        );
    };
    match instr {
        Nop => line(out, "// nop".to_string()),

        // --- ALU immediate (results are 32-bit, sign-extended into GPR) ---
        Addi { rt, rs, imm } => line(
            out,
            format!(
                "ctx.set_r32({}, ({}).checked_add({}).expect(\"MIPS ADDI integer overflow\"));",
                rt,
                rs32(rs),
                imm as i32
            ),
        ),
        Addiu { rt, rs, imm } => line(
            out,
            format!("ctx.set_r32({}, ({}).wrapping_add({}));", rt, rs32(rs), imm as i32),
        ),
        // SLTI/SLTIU compare the full 64-bit register (ToS64/ToU64) against
        // the sign-extended immediate. `imm as i64` sign-extends; for SLTIU
        // the same sign-extended value is reinterpreted as u64.
        Slti { rt, rs, imm } => line(
            out,
            format!("ctx.set_r({}, if {} < {}i64 {{ 1 }} else {{ 0 }});", rt, rs64(rs), imm as i64),
        ),
        Sltiu { rt, rs, imm } => line(
            out,
            format!(
                "ctx.set_r({}, if {} < {}u64 {{ 1 }} else {{ 0 }});",
                rt,
                ru64(rs),
                imm as i64 as u64
            ),
        ),
        Andi { rt, rs, imm } => {
            line(out, format!("ctx.set_r({}, {} & {:#X});", rt, r(rs), imm as u64))
        }
        Ori { rt, rs, imm } => {
            line(out, format!("ctx.set_r({}, {} | {:#X});", rt, r(rs), imm as u64))
        }
        Xori { rt, rs, imm } => {
            line(out, format!("ctx.set_r({}, {} ^ {:#X});", rt, r(rs), imm as u64))
        }
        Lui { rt, imm } => {
            // Emit the constant as a `u32` literal cast to `i32`: a high LUI
            // (e.g. 0x800F0000) has bit 31 set, so a bare `…i32` literal would
            // overflow the `i32` range (a rustc `overflowing_literals` error).
            line(out, format!("ctx.set_r32({}, {:#X}u32 as i32);", rt, ((imm as u32) << 16)))
        }

        // --- ALU register ---
        Add { rd, rs, rt } => line(
            out,
            format!(
                "ctx.set_r32({}, ({}).checked_add({}).expect(\"MIPS ADD integer overflow\"));",
                rd,
                rs32(rs),
                rs32(rt)
            ),
        ),
        Addu { rd, rs, rt } => line(
            out,
            format!("ctx.set_r32({}, ({}).wrapping_add({}));", rd, rs32(rs), rs32(rt)),
        ),
        Sub { rd, rs, rt } => line(
            out,
            format!(
                "ctx.set_r32({}, ({}).checked_sub({}).expect(\"MIPS SUB integer overflow\"));",
                rd,
                rs32(rs),
                rs32(rt)
            ),
        ),
        Subu { rd, rs, rt } => line(
            out,
            format!("ctx.set_r32({}, ({}).wrapping_sub({}));", rd, rs32(rs), rs32(rt)),
        ),
        And { rd, rs, rt } => line(out, format!("ctx.set_r({}, {} & {});", rd, r(rs), r(rt))),
        Or { rd, rs, rt } => line(out, format!("ctx.set_r({}, {} | {});", rd, r(rs), r(rt))),
        Xor { rd, rs, rt } => line(out, format!("ctx.set_r({}, {} ^ {});", rd, r(rs), r(rt))),
        Nor { rd, rs, rt } => line(out, format!("ctx.set_r({}, !({} | {}));", rd, r(rs), r(rt))),
        Slt { rd, rs, rt } => line(
            out,
            format!("ctx.set_r({}, if {} < {} {{ 1 }} else {{ 0 }});", rd, rs64(rs), rs64(rt)),
        ),
        Sltu { rd, rs, rt } => line(
            out,
            format!("ctx.set_r({}, if {} < {} {{ 1 }} else {{ 0 }});", rd, ru64(rs), ru64(rt)),
        ),

        // --- Shifts (32-bit, sign-extended) ---
        Sll { rd, rt, sa } => {
            line(out, format!("ctx.set_r32({}, (({}) << {}) as i32);", rd, ru32(rt), sa))
        }
        Srl { rd, rt, sa } => {
            line(out, format!("ctx.set_r32({}, (({}) >> {}) as i32);", rd, ru32(rt), sa))
        }
        Sra { rd, rt, sa } => {
            line(out, format!("ctx.set_r32({}, {} >> {});", rd, rs32(rt), sa))
        }
        Sllv { rd, rt, rs } => line(
            out,
            format!("ctx.set_r32({}, (({}) << ({} & 31)) as i32);", rd, ru32(rt), ru32(rs)),
        ),
        Srlv { rd, rt, rs } => line(
            out,
            format!("ctx.set_r32({}, (({}) >> ({} & 31)) as i32);", rd, ru32(rt), ru32(rs)),
        ),
        Srav { rd, rt, rs } => line(
            out,
            format!("ctx.set_r32({}, {} >> ({} & 31));", rd, rs32(rt), ru32(rs)),
        ),

        // --- Mult/Div (write HI/LO). MIPS keeps 32x32 -> 64 in {hi,lo}. ---
        Mult { rs, rt } => line(
            out,
            format!(
                "{{ let p = ({} as i64) * ({} as i64); ctx.lo = (p as i32) as i64 as u64; ctx.hi = ((p >> 32) as i32) as i64 as u64; }}",
                rs32(rs),
                rs32(rt)
            ),
        ),
        Multu { rs, rt } => line(
            out,
            format!(
                "{{ let p = ({} as u64) * ({} as u64); ctx.lo = (p as i32) as i64 as u64; ctx.hi = ((p >> 32) as i32) as i64 as u64; }}",
                ru32(rs),
                ru32(rt)
            ),
        ),
        Div { rs, rt } => line(
            out,
            format!("ctx.div_s32({}, {});", rs32(rs), rs32(rt)),
        ),
        Divu { rs, rt } => line(
            out,
            format!("ctx.div_u32({}, {});", ru32(rs), ru32(rt)),
        ),
        Mfhi { rd } => line(out, format!("ctx.set_r({}, ctx.hi);", rd)),
        Mflo { rd } => line(out, format!("ctx.set_r({}, ctx.lo);", rd)),
        Mthi { rs } => line(out, format!("ctx.hi = {};", r(rs))),
        Mtlo { rs } => line(out, format!("ctx.lo = {};", r(rs))),

        // --- Loads ---
        Lw { rt, base, off } => mem_fault.load(
            out,
            &format!("mem.load_w(Rdram::eff_addr({}, {}))", r(base), off),
            &format!("mem.try_load_w_translated(ctx, Rdram::eff_addr({}, {}))", r(base), off),
            |value| format!("ctx.set_r32({rt}, {value});"),
        ),
        Lwu { rt, base, off } => mem_fault.load(
            out,
            &format!("mem.load_w(Rdram::eff_addr({}, {}))", r(base), off),
            &format!("mem.try_load_w_translated(ctx, Rdram::eff_addr({}, {}))", r(base), off),
            |value| format!("ctx.set_r({rt}, {value} as u32 as u64);"),
        ),
        Ll { rt, base, off } => emit_ll(out, *mem_fault, rt, base, off),
        Lh { rt, base, off } => mem_fault.load(
            out,
            &format!("mem.load_h(Rdram::eff_addr({}, {}))", r(base), off),
            &format!("mem.try_load_h_translated(ctx, Rdram::eff_addr({}, {}))", r(base), off),
            |value| format!("ctx.set_r32({rt}, {value} as i32);"),
        ),
        Lhu { rt, base, off } => mem_fault.load(
            out,
            &format!("mem.load_hu(Rdram::eff_addr({}, {}))", r(base), off),
            &format!("mem.try_load_hu_translated(ctx, Rdram::eff_addr({}, {}))", r(base), off),
            |value| format!("ctx.set_r({rt}, {value} as u64);"),
        ),
        Lb { rt, base, off } => mem_fault.load(
            out,
            &format!("mem.load_b(Rdram::eff_addr({}, {}))", r(base), off),
            &format!("mem.try_load_b_translated(ctx, Rdram::eff_addr({}, {}))", r(base), off),
            |value| format!("ctx.set_r32({rt}, {value} as i32);"),
        ),
        Lbu { rt, base, off } => mem_fault.load(
            out,
            &format!("mem.load_bu(Rdram::eff_addr({}, {}))", r(base), off),
            &format!("mem.try_load_bu_translated(ctx, Rdram::eff_addr({}, {}))", r(base), off),
            |value| format!("ctx.set_r({rt}, {value} as u64);"),
        ),
        Lwl { rt, base, off } => mem_fault.load(
            out,
            &format!("mem.load_wl(ctx.r({rt}), Rdram::eff_addr({}, {}))", r(base), off),
            &format!("mem.try_load_wl_translated(ctx, ctx.r({rt}), Rdram::eff_addr({}, {}))", r(base), off),
            |value| format!("ctx.set_r32({rt}, {value});"),
        ),
        Lwr { rt, base, off } => mem_fault.load(
            out,
            &format!("mem.load_wr(ctx.r({rt}), Rdram::eff_addr({}, {}))", r(base), off),
            &format!("mem.try_load_wr_translated(ctx, ctx.r({rt}), Rdram::eff_addr({}, {}))", r(base), off),
            |value| format!("ctx.set_r32({rt}, {value});"),
        ),

        // --- Stores ---
        Sw { rt, base, off } => mem_fault.store(
            out,
            &format!("mem.store_w(Rdram::eff_addr({}, {}), {});", r(base), off, ru32(rt)),
            &format!("mem.try_store_w_translated(ctx, Rdram::eff_addr({}, {}), {})", r(base), off, ru32(rt)),
        ),
        Sh { rt, base, off } => mem_fault.store(
            out,
            &format!("mem.store_h(Rdram::eff_addr({}, {}), {} as u16);", r(base), off, ru32(rt)),
            &format!("mem.try_store_h_translated(ctx, Rdram::eff_addr({}, {}), {} as u16)", r(base), off, ru32(rt)),
        ),
        Sb { rt, base, off } => mem_fault.store(
            out,
            &format!("mem.store_b(Rdram::eff_addr({}, {}), {} as u8);", r(base), off, ru32(rt)),
            &format!("mem.try_store_b_translated(ctx, Rdram::eff_addr({}, {}), {} as u8)", r(base), off, ru32(rt)),
        ),
        Swl { rt, base, off } => mem_fault.store(
            out,
            &format!("mem.store_wl(Rdram::eff_addr({}, {}), {});", r(base), off, ru32(rt)),
            &format!("mem.try_store_wl_translated(ctx, Rdram::eff_addr({}, {}), {})", r(base), off, ru32(rt)),
        ),
        Swr { rt, base, off } => mem_fault.store(
            out,
            &format!("mem.store_wr(Rdram::eff_addr({}, {}), {});", r(base), off, ru32(rt)),
            &format!("mem.try_store_wr_translated(ctx, Rdram::eff_addr({}, {}), {})", r(base), off, ru32(rt)),
        ),
        Sc { rt, base, off } => emit_sc(out, *mem_fault, rt, base, off, false),

        // --- 64-bit doubleword ALU immediate ---
        // DADDI/DADDIU: full 64-bit add of rs and the sign-extended immediate;
        // the trapping form uses checked arithmetic.
        Daddi { rt, rs, imm } => line(
            out,
            format!(
                "ctx.set_r({}, ({} as i64).checked_add({}i64).expect(\"MIPS DADDI integer overflow\") as u64);",
                rt,
                ru64(rs),
                imm as i64
            ),
        ),
        Daddiu { rt, rs, imm } => line(
            out,
            format!("ctx.set_r({}, ({}).wrapping_add({}i64 as u64));", rt, ru64(rs), imm as i64),
        ),

        // --- 64-bit doubleword ALU register ---
        Dadd { rd, rs, rt } => line(
            out,
            format!(
                "ctx.set_r({}, ({}).checked_add({}).expect(\"MIPS DADD integer overflow\") as u64);",
                rd,
                rs64(rs),
                rs64(rt)
            ),
        ),
        Daddu { rd, rs, rt } => line(
            out,
            format!("ctx.set_r({}, ({}).wrapping_add({}));", rd, ru64(rs), ru64(rt)),
        ),
        Dsub { rd, rs, rt } => line(
            out,
            format!(
                "ctx.set_r({}, ({}).checked_sub({}).expect(\"MIPS DSUB integer overflow\") as u64);",
                rd,
                rs64(rs),
                rs64(rt)
            ),
        ),
        Dsubu { rd, rs, rt } => line(
            out,
            format!("ctx.set_r({}, ({}).wrapping_sub({}));", rd, ru64(rs), ru64(rt)),
        ),

        // --- 64-bit doubleword shifts (results stay full 64-bit) ---
        // DSLL/DSRL by sa (0..31); logical shifts operate on ToU64, arithmetic
        // (DSRA) on ToS64 so bit 63 fills.
        Dsll { rd, rt, sa } => {
            line(out, format!("ctx.set_r({}, ({}) << {});", rd, ru64(rt), sa))
        }
        Dsrl { rd, rt, sa } => {
            line(out, format!("ctx.set_r({}, ({}) >> {});", rd, ru64(rt), sa))
        }
        Dsra { rd, rt, sa } => {
            line(out, format!("ctx.set_r({}, (({}) >> {}) as u64);", rd, rs64(rt), sa))
        }
        // The *32 forms shift by sa + 32 (32..63).
        Dsll32 { rd, rt, sa } => {
            line(out, format!("ctx.set_r({}, ({}) << {});", rd, ru64(rt), sa as u32 + 32))
        }
        Dsrl32 { rd, rt, sa } => {
            line(out, format!("ctx.set_r({}, ({}) >> {});", rd, ru64(rt), sa as u32 + 32))
        }
        Dsra32 { rd, rt, sa } => {
            line(out, format!("ctx.set_r({}, (({}) >> {}) as u64);", rd, rs64(rt), sa as u32 + 32))
        }
        // Variable doubleword shifts: shift count is the low 6 bits of rs (0..63).
        Dsllv { rd, rt, rs } => line(
            out,
            format!("ctx.set_r({}, ({}) << ({} & 63));", rd, ru64(rt), ru64(rs)),
        ),
        Dsrlv { rd, rt, rs } => line(
            out,
            format!("ctx.set_r({}, ({}) >> ({} & 63));", rd, ru64(rt), ru64(rs)),
        ),
        Dsrav { rd, rt, rs } => line(
            out,
            format!("ctx.set_r({}, (({}) >> ({} & 63)) as u64);", rd, rs64(rt), ru64(rs)),
        ),

        // --- 64-bit doubleword mult/div (write HI/LO as full 64-bit) ---
        // DMULT/DMULTU: 64x64 -> 128-bit product; LO = low 64, HI = high 64.
        // Rust's i128/u128 give the full product safely (no unsafe, no
        // pointer tricks) — the typed analogue of N64Recomp's __int128 DMULT.
        Dmult { rs, rt } => line(
            out,
            format!(
                "{{ let p = ({} as i128) * ({} as i128); ctx.lo = p as u64; ctx.hi = (p >> 64) as u64; }}",
                rs64(rs),
                rs64(rt)
            ),
        ),
        Dmultu { rs, rt } => line(
            out,
            format!(
                "{{ let p = ({} as u128) * ({} as u128); ctx.lo = p as u64; ctx.hi = (p >> 64) as u64; }}",
                ru64(rs),
                ru64(rt)
            ),
        ),
        // DDIV: signed 64-bit, including INT64_MIN / -1. The runtime helper
        // traps loudly on the manual-uncertain zero-divisor case.
        Ddiv { rs, rt } => line(
            out,
            format!("ctx.div_s64({}, {});", rs64(rs), rs64(rt)),
        ),
        Ddivu { rs, rt } => line(
            out,
            format!("ctx.div_u64({}, {});", ru64(rs), ru64(rt)),
        ),

        // --- Doubleword loads ---
        Ld { rt, base, off } => mem_fault.load(
            out,
            &format!("mem.load_d(Rdram::eff_addr({}, {}))", r(base), off),
            &format!("mem.try_load_d_translated(ctx, Rdram::eff_addr({}, {}))", r(base), off),
            |value| format!("ctx.set_r({rt}, {value});"),
        ),
        Lld { rt, base, off } => emit_lld(out, *mem_fault, rt, base, off),
        Ldl { rt, base, off } => mem_fault.load(
            out,
            &format!("mem.load_dl(ctx.r({rt}), Rdram::eff_addr({}, {}))", r(base), off),
            &format!("mem.try_load_dl_translated(ctx, ctx.r({rt}), Rdram::eff_addr({}, {}))", r(base), off),
            |value| format!("ctx.set_r({rt}, {value});"),
        ),
        Ldr { rt, base, off } => mem_fault.load(
            out,
            &format!("mem.load_dr(ctx.r({rt}), Rdram::eff_addr({}, {}))", r(base), off),
            &format!("mem.try_load_dr_translated(ctx, ctx.r({rt}), Rdram::eff_addr({}, {}))", r(base), off),
            |value| format!("ctx.set_r({rt}, {value});"),
        ),

        // --- Doubleword stores ---
        Sd { rt, base, off } => mem_fault.store(
            out,
            &format!("mem.store_d(Rdram::eff_addr({}, {}), {});", r(base), off, ru64(rt)),
            &format!("mem.try_store_d_translated(ctx, Rdram::eff_addr({}, {}), {})", r(base), off, ru64(rt)),
        ),
        Sdl { rt, base, off } => mem_fault.store(
            out,
            &format!("mem.store_dl(Rdram::eff_addr({}, {}), {});", r(base), off, ru64(rt)),
            &format!("mem.try_store_dl_translated(ctx, Rdram::eff_addr({}, {}), {})", r(base), off, ru64(rt)),
        ),
        Sdr { rt, base, off } => mem_fault.store(
            out,
            &format!("mem.store_dr(Rdram::eff_addr({}, {}), {});", r(base), off, ru64(rt)),
            &format!("mem.try_store_dr_translated(ctx, Rdram::eff_addr({}, {}), {})", r(base), off, ru64(rt)),
        ),
        Scd { rt, base, off } => emit_sc(out, *mem_fault, rt, base, off, true),

        // ================================================================
        // COP1 / FPU.
        //
        // All FPU register reads/writes go through typed `RecompContext`
        // accessors (`f_s`/`set_f_s` single, `f_d`/`set_f_d` double,
        // `f_bits`/`d_bits` raw) that resolve the FR=0 even/odd pairing
        // internally — the emitter never open-codes the `f_odd[(N-1)*2]`
        // pointer arithmetic the C oracle uses. Semantics are clean-roomed
        // from the MIPS III / VR4300 reference (and cross-checked against the
        // recomp.h CVT_/TRUNC_ macro definitions, which are the ISA facts).
        // ================================================================

        // --- GPR <-> FPR moves ---
        // MFC1: GPR = sign-extend(FPR single low32). Mirrors `(int32_t)f.u32l`.
        Mfc1 { rt, fs } => {
            line(out, format!("ctx.set_r32({}, ctx.f_bits({}) as i32);", rt, fs))
        }
        // MTC1: FPR single low32 = GPR low32 (raw bits).
        Mtc1 { rt, fs } => {
            line(out, format!("ctx.set_f_bits({}, {});", fs, ru32(rt)))
        }
        // DMFC1: GPR = FPR full 64 bits.
        Dmfc1 { rt, fs } => line(out, format!("ctx.set_r({}, ctx.d_bits({}));", rt, fs)),
        // DMTC1: FPR 64 bits = GPR.
        Dmtc1 { rt, fs } => line(out, format!("ctx.set_d_bits({}, {});", fs, ru64(rt))),
        // CFC1/CTC1: typed FCR0/FCR31 access. OoT reads and rewrites FCR31
        // around conversion sequences, including non-nearest RM values.
        Cfc1 { rt, fs } => line(
            out,
            format!("{{ let v = ctx.read_fcr({}); ctx.set_r32({}, v as i32); }}", fs, rt),
        ),
        Ctc1 { rt, fs } => {
            line(out, format!("ctx.write_fcr({}, {});", fs, ru32(rt)));
            let finish = mem_fault.fpu_exception_finish();
            line(
                out,
                format!("if ctx.fcsr_exception_pending() {{ {finish} }}"),
            );
        }

        // --- COP1 loads/stores ---
        Lwc1 { ft, base, off } => mem_fault.load(
            out,
            &format!("mem.load_w(Rdram::eff_addr({}, {}))", r(base), off),
            &format!("mem.try_load_w_translated(ctx, Rdram::eff_addr({}, {}))", r(base), off),
            |value| format!("ctx.set_f_bits({ft}, {value} as u32);"),
        ),
        Swc1 { ft, base, off } => mem_fault.store(
            out,
            &format!("mem.store_w(Rdram::eff_addr({}, {}), ctx.f_bits({ft}));", r(base), off),
            &format!("mem.try_store_w_translated(ctx, Rdram::eff_addr({}, {}), ctx.f_bits({ft}))", r(base), off),
        ),
        Ldc1 { ft, base, off } => mem_fault.load(
            out,
            &format!("mem.load_d(Rdram::eff_addr({}, {}))", r(base), off),
            &format!("mem.try_load_d_translated(ctx, Rdram::eff_addr({}, {}))", r(base), off),
            |value| format!("ctx.set_d_bits({ft}, {value});"),
        ),
        Sdc1 { ft, base, off } => mem_fault.store(
            out,
            &format!("mem.store_d(Rdram::eff_addr({}, {}), ctx.d_bits({ft}));", r(base), off),
            &format!("mem.try_store_d_translated(ctx, Rdram::eff_addr({}, {}), ctx.d_bits({ft}))", r(base), off),
        ),

        // --- Single-precision arithmetic ---
        // Routed through the IEEE soft-float shim so the op honors FCSR.RM and
        // sets the FCSR Cause/Flag bits (`crate::fpu` via the `ctx.fpu_*`
        // helpers). The raw-host `+`/`*`/`.sqrt()` path (round-to-nearest,
        // no flags) is retired.
        // The `fpu_*` shim helpers return `true` when an ENABLED FP exception
        // trapped (destination left unwritten). The whole-function / straight-
        // line lane has no exception-return ABI yet, so it panics loudly on a
        // trap, mirroring the `.expect("MIPS ADD integer overflow")` shape the
        // integer-arithmetic arms use. The bank lane instead short-circuits this
        // via `emit_bank_fpu_trap`, which turns the same `true` into a typed
        // `BlockExit::Fault(CpuException::FloatingPoint)` (ExcCode 15).
        AddS { fd, fs, ft } => line(out, emit_fpu_arith_call(&format!("ctx.fpu_add_s({fd}, {fs}, {ft})"))),
        SubS { fd, fs, ft } => line(out, emit_fpu_arith_call(&format!("ctx.fpu_sub_s({fd}, {fs}, {ft})"))),
        MulS { fd, fs, ft } => line(out, emit_fpu_arith_call(&format!("ctx.fpu_mul_s({fd}, {fs}, {ft})"))),
        DivS { fd, fs, ft } => line(out, emit_fpu_arith_call(&format!("ctx.fpu_div_s({fd}, {fs}, {ft})"))),
        AbsS { fd, fs } => line(out, emit_fpu_arith_call(&format!("ctx.fpu_abs_s({fd}, {fs})"))),
        NegS { fd, fs } => line(out, emit_fpu_arith_call(&format!("ctx.fpu_neg_s({fd}, {fs})"))),
        SqrtS { fd, fs } => line(out, emit_fpu_arith_call(&format!("ctx.fpu_sqrt_s({fd}, {fs})"))),
        // MOV.S is a bit-exact copy (not an arithmetic op): move the raw word.
        MovS { fd, fs } => line(out, format!("ctx.set_f_bits({}, ctx.f_bits({}));", fd, fs)),
        // Conditional moves: pure register copies, never trap (no `if`-guard).
        MovcfS { fd, fs, tf } => line(out, format!("ctx.fpu_movcf_s({fd}, {fs}, {tf});")),
        MovzS { fd, fs, rt } => line(out, format!("ctx.fpu_movz_s({fd}, {fs}, {rt});")),
        MovnS { fd, fs, rt } => line(out, format!("ctx.fpu_movn_s({fd}, {fs}, {rt});")),

        // --- Double-precision arithmetic (routed through the shim). ---
        AddD { fd, fs, ft } => line(out, emit_fpu_arith_call(&format!("ctx.fpu_add_d({fd}, {fs}, {ft})"))),
        SubD { fd, fs, ft } => line(out, emit_fpu_arith_call(&format!("ctx.fpu_sub_d({fd}, {fs}, {ft})"))),
        MulD { fd, fs, ft } => line(out, emit_fpu_arith_call(&format!("ctx.fpu_mul_d({fd}, {fs}, {ft})"))),
        DivD { fd, fs, ft } => line(out, emit_fpu_arith_call(&format!("ctx.fpu_div_d({fd}, {fs}, {ft})"))),
        AbsD { fd, fs } => line(out, emit_fpu_arith_call(&format!("ctx.fpu_abs_d({fd}, {fs})"))),
        NegD { fd, fs } => line(out, emit_fpu_arith_call(&format!("ctx.fpu_neg_d({fd}, {fs})"))),
        SqrtD { fd, fs } => line(out, emit_fpu_arith_call(&format!("ctx.fpu_sqrt_d({fd}, {fs})"))),
        MovD { fd, fs } => line(out, format!("ctx.set_d_bits({}, ctx.d_bits({}));", fd, fs)),
        MovcfD { fd, fs, tf } => line(out, format!("ctx.fpu_movcf_d({fd}, {fs}, {tf});")),
        MovzD { fd, fs, rt } => line(out, format!("ctx.fpu_movz_d({fd}, {fs}, {rt});")),
        MovnD { fd, fs, rt } => line(out, format!("ctx.fpu_movn_d({fd}, {fs}, {rt});")),

        // --- Conversions. Float-to-float and fixed-to-float use shared
        //     integer-only IEEE encoders and typed FCSR results.
        //     The
        //     int destinations write the RAW 32/64 bits of the result into the
        //     FPR (an int-in-FPR is stored as its two's-complement bit pattern,
        //     exactly as the C writes `f.u32l = (int32_t)...`). The int source
        //     of CVT.S.W/CVT.D.W reads the FPR single word AS an i32.

        CvtSW { fd, fs } => emit_fixed_to_float(out, *mem_fault, fd, fs, 'S', 'W'),
        CvtDW { fd, fs } => emit_fixed_to_float(out, *mem_fault, fd, fs, 'D', 'W'),
        CvtSL { fd, fs } => emit_fixed_to_float(out, *mem_fault, fd, fs, 'S', 'L'),
        CvtDL { fd, fs } => emit_fixed_to_float(out, *mem_fault, fd, fs, 'D', 'L'),
        CvtDS { fd, fs } => emit_float_to_float(out, *mem_fault, fd, fs, 'D', 'S'),
        CvtSD { fd, fs } => emit_float_to_float(out, *mem_fault, fd, fs, 'S', 'D'),

        // float/double -> int32 (round to nearest, ties to even = FCSR default).
        // Written as raw bits of the i32 into the FPR single word.
        CvtWS { fd, fs } => emit_fpu_i32(out, *mem_fault, fd, fs, true, None),
        CvtWD { fd, fs } => emit_fpu_i32(out, *mem_fault, fd, fs, false, None),
        // float/double -> int64 (round to nearest).
        CvtLS { fd, fs } => emit_fpu_i64(out, *mem_fault, fd, fs, true, None),
        CvtLD { fd, fs } => emit_fpu_i64(out, *mem_fault, fd, fs, false, None),

        // TRUNC.* uses the typed raw-register helper with fixed RM=1. The
        // helper classifies guest operands and raises FCSR exceptions before
        // the destination is committed; no host-language cast defines the
        // guest out-of-range or NaN behavior.
        TruncWS { fd, fs } => emit_fpu_i32(out, *mem_fault, fd, fs, true, Some(1)),
        TruncWD { fd, fs } => emit_fpu_i32(out, *mem_fault, fd, fs, false, Some(1)),
        TruncLS { fd, fs } => emit_fpu_i64(out, *mem_fault, fd, fs, true, Some(1)),
        TruncLD { fd, fs } => emit_fpu_i64(out, *mem_fault, fd, fs, false, Some(1)),
        RoundWS { fd, fs } => emit_fpu_i32(out, *mem_fault, fd, fs, true, Some(0)),
        RoundWD { fd, fs } => emit_fpu_i32(out, *mem_fault, fd, fs, false, Some(0)),
        RoundLS { fd, fs } => emit_fpu_i64(out, *mem_fault, fd, fs, true, Some(0)),
        RoundLD { fd, fs } => emit_fpu_i64(out, *mem_fault, fd, fs, false, Some(0)),
        CeilWS { fd, fs } => emit_fpu_i32(out, *mem_fault, fd, fs, true, Some(2)),
        CeilWD { fd, fs } => emit_fpu_i32(out, *mem_fault, fd, fs, false, Some(2)),
        CeilLS { fd, fs } => emit_fpu_i64(out, *mem_fault, fd, fs, true, Some(2)),
        CeilLD { fd, fs } => emit_fpu_i64(out, *mem_fault, fd, fs, false, Some(2)),
        FloorWS { fd, fs } => emit_fpu_i32(out, *mem_fault, fd, fs, true, Some(3)),
        FloorWD { fd, fs } => emit_fpu_i32(out, *mem_fault, fd, fs, false, Some(3)),
        FloorLS { fd, fs } => emit_fpu_i64(out, *mem_fault, fd, fs, true, Some(3)),
        FloorLD { fd, fs } => emit_fpu_i64(out, *mem_fault, fd, fs, false, Some(3)),
        // (FLOOR/CEIL/ROUND.W.{S,D} are handled by the unified emit_fpu_i32
        // arms above with the mode arg Some(3)/Some(2)/Some(0); the duplicate
        // inline arms from main's driver branch were removed as unreachable on
        // merge -- the emit_fpu_i32 helper and the merged decoder are the
        // superset, and fpu_oracle.rs verifies the emitted behavior matches.)

        // --- FP compares: set the condition flag (FCSR bit 23). ---
        CEqS { fs, ft } => emit_fpu_compare(out, *mem_fault, true, fs, ft, 2),
        CLtS { fs, ft } => emit_fpu_compare(out, *mem_fault, true, fs, ft, 12),
        CLeS { fs, ft } => emit_fpu_compare(out, *mem_fault, true, fs, ft, 14),
        CEqD { fs, ft } => emit_fpu_compare(out, *mem_fault, false, fs, ft, 2),
        CLtD { fs, ft } => emit_fpu_compare(out, *mem_fault, false, fs, ft, 12),
        CLeD { fs, ft } => emit_fpu_compare(out, *mem_fault, false, fs, ft, 14),
        CCondS { fs, ft, cond } => emit_fpu_compare(out, *mem_fault, true, fs, ft, cond),
        CCondD { fs, ft, cond } => emit_fpu_compare(out, *mem_fault, false, fs, ft, cond),

        // --- COP0 system control ---
        //
        // The typed context owns the modeled COP0 state. Unsupported
        // registers remain loud; the block lane separately expresses ERET as
        // a typed arbitrary-PC transfer.
        Mfc0 { rt, cop0d } => match cop0d {
            9 => match mem_fault {
                MemFault::Panic => {
                    line(out, format!("ctx.set_r32({}, ctx.cop0_count as i32);", rt))
                }
                MemFault::Fault { .. } => line(
                    out,
                    format!(
                        "ctx.set_r32({}, ctx.read_cop0_count_interior(executed) as i32);",
                        rt
                    ),
                ),
            },
            11 => line(out, format!("ctx.set_r32({}, ctx.cop0_compare as i32);", rt)),
            1 if matches!(mem_fault, MemFault::Fault { .. }) => line(
                out,
                format!("ctx.set_r32({}, ctx.read_cop0(1) as i32);", rt),
            ),
            0 | 2 | 3 | 4 | 5 | 6 | 8 | 10 | 12 | 13 | 14 | 18 | 19 | 20 | 30 => line(
                out,
                format!("ctx.set_r32({}, ctx.read_cop0({}) as i32);", rt, cop0d),
            ),
            other => unsupported(
                out,
                format!("unsupported mfc0 from COP0 register {other}"),
            ),
        },
        Mtc0 { rt, cop0d } => match cop0d {
            0 | 2 | 3 | 4 | 5 | 6 | 9 | 10 | 11 | 12 | 13 | 14 | 18 | 19 | 30 => line(
                out,
                format!("ctx.write_cop0({}, {});", cop0d, ru32(rt)),
            ),
            other => unsupported(
                out,
                format!("unsupported mtc0 to COP0 register {other}"),
            ),
        },
        Dmfc0 { rt, cop0d }
            if matches!(mem_fault, MemFault::Fault { .. }) && matches!(cop0d, 8 | 10 | 20) =>
        {
            line(
                out,
                format!("ctx.set_r({}, ctx.read_cop0_64({}));", rt, cop0d),
            )
        }
        Dmtc0 { rt, cop0d }
            if matches!(mem_fault, MemFault::Fault { .. }) && matches!(cop0d, 10 | 20) =>
        {
            line(
                out,
                format!("ctx.write_cop0_64({}, {});", cop0d, r(rt)),
            )
        }
        Dmfc0 { cop0d, .. } => unsupported(
            out,
            format!("unsupported dmfc0 from COP0 register {cop0d}"),
        ),
        Dmtc0 { cop0d, .. } => unsupported(
            out,
            format!("unsupported dmtc0 to COP0 register {cop0d}"),
        ),
        Eret => unsupported(
            out,
            "eret executed in recompiled code: exception return is host/libultra territory"
                .to_owned(),
        ),
        Tlbwi => line(out, "ctx.tlbwi_record();".to_string()),
        Tlbwr => match mem_fault {
            MemFault::Panic => unsupported(
                out,
                "tlbwr in whole-function code requires an instruction clock".to_owned(),
            ),
            MemFault::Fault { .. } => line(out, "ctx.tlbwr_record();".to_string()),
        },
        Tlbp => line(out, "ctx.tlbp_probe();".to_string()),
        Tlbr => line(out, "ctx.tlbr_read();".to_string()),

        // --- Cache / sync: no-ops on a coherent host rdram ---
        Cache { op, .. } => {
            line(out, format!("// cache op {:#04X}: no-op (host rdram is coherent)", op))
        }
        Sync => line(out, "// sync: no-op (single-threaded recompiled context)".to_string()),

        // --- COP2: unused coprocessor, loud trap ---
        Mfc2 { .. } | Mtc2 { .. } | Cfc2 { .. } | Ctc2 { .. } | Dmfc2 { .. }
        | Dmtc2 { .. } | Cop2Op { .. } | Lwc2 { .. } | Ldc2 { .. } | Swc2 { .. }
        | Sdc2 { .. } => unsupported(
            out,
            "COP2 access in recompiled code: COP2 is unused on the N64 and not modeled".to_owned(),
        ),

        // --- Traps ---
        Syscall { code } => line(
            out,
            format!("panic!(\"syscall (code {:#X}) executed in recompiled code\");", code),
        ),
        Break { code } => {
            line(out, format!("panic!(\"break (code {:#X}) executed in recompiled code\");", code))
        }
        Tge { rs, rt, code } => emit_trap(out, &format!("{} >= {}", rs64(rs), rs64(rt)), "tge", code),
        Tgeu { rs, rt, code } => emit_trap(out, &format!("{} >= {}", ru64(rs), ru64(rt)), "tgeu", code),
        Tlt { rs, rt, code } => emit_trap(out, &format!("{} < {}", rs64(rs), rs64(rt)), "tlt", code),
        Tltu { rs, rt, code } => emit_trap(out, &format!("{} < {}", ru64(rs), ru64(rt)), "tltu", code),
        Teq { rs, rt, code } => emit_trap(out, &format!("{} == {}", r(rs), r(rt)), "teq", code),
        Tne { rs, rt, code } => emit_trap(out, &format!("{} != {}", r(rs), r(rt)), "tne", code),
        Tgei { rs, imm } => emit_trap(out, &format!("{} >= {}i64", rs64(rs), imm as i64), "tgei", 0),
        Tgeiu { rs, imm } => emit_trap(out, &format!("{} >= {}u64", ru64(rs), imm as i64 as u64), "tgeiu", 0),
        Tlti { rs, imm } => emit_trap(out, &format!("{} < {}i64", rs64(rs), imm as i64), "tlti", 0),
        Tltiu { rs, imm } => emit_trap(out, &format!("{} < {}u64", ru64(rs), imm as i64 as u64), "tltiu", 0),
        Teqi { rs, imm } => emit_trap(out, &format!("{} == {}i64", rs64(rs), imm as i64), "teqi", 0),
        Tnei { rs, imm } => emit_trap(out, &format!("{} != {}i64", rs64(rs), imm as i64), "tnei", 0),

        // Control transfers are never emitted here.
        other => line(out, format!("compile_error!(\"non-straight op reached emit_straight: {:?}\");", other)),
    }
}
