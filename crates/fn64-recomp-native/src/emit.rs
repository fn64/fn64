//! The typed Rust emitter: a decoded MIPS function -> one Rust `fn` body that
//! operates on the typed [`crate::runtime::RecompContext`] + [`crate::runtime::Rdram`].
//!
//! # Structure (mirrors N64Recomp, emits Rust not C)
//!
//! N64Recomp emits one C function per MIPS function, with `goto L_XXXX` labels
//! at every branch target and the delay-slot instruction duplicated into the
//! taken path (see `recompilation.cpp::process_instruction`). C `goto` has no
//! safe Rust equivalent, so we emit the same control-flow graph as a
//! **labelled dispatch loop**:
//!
//! ```text
//! let mut pc = <entry>;
//! 'run: loop { match pc {
//!     0x…00 => { <ops> ; pc = 0x…04; }          // straight-line fall-through
//!     0x…10 => { <ops> ;                          // a branch site:
//!         <delay-slot ops>                         //   delay slot runs first
//!         if <cond> { pc = 0x…40; } else { pc = 0x…18; }
//!     }
//!     0x…44 => { <ops> ; return; }               // jr $ra
//!     _ => unreachable!(),
//! } }
//! ```
//!
//! Every basic-block boundary (a branch target, or the instruction after a
//! branch) starts a new `match` arm keyed by the instruction's vram. This is
//! the delay-slot rule made explicit: the branch arm executes the delay slot,
//! then assigns `pc`, so the delay slot always runs exactly once regardless of
//! whether the branch is taken. Branch-likely ops put the delay slot only in
//! the taken assignment.
//!
//! The emitted code contains **no `unsafe`, no pointer casts** — only typed
//! `RecompContext`/`Rdram` method calls. That is the property `-native` exists
//! to guarantee.

use crate::decoder::{decode, Instruction, Reg};
use std::collections::BTreeSet;
use std::fmt::Write;

/// One MIPS function to recompile: its name, its start vram, and its words
/// (big-endian instruction words already read into `u32`s).
pub struct FuncInput<'a> {
    pub name: &'a str,
    pub vram: u32,
    pub words: &'a [u32],
}

/// Emit a complete Rust module: the `fn <name>` plus a doc header. The emitted
/// function has signature `fn <name>(ctx: &mut RecompContext, mem: &mut Rdram)`.
///
/// Indirect calls (`jal`/`jalr`/`jr $t` tail calls) are emitted as a call
/// through a supplied dispatch closure; for this foundation slice we emit them
/// as a `lookup(target)(ctx, mem)` call so the shape is proven, matching
/// N64Recomp's `LOOKUP_FUNC`. Straight-line leaf functions (the oracle target)
/// need none of that.
pub fn emit_function(func: &FuncInput) -> String {
    let base = func.vram;
    let words = func.words;

    // 1. Decode every word.
    let instrs: Vec<Instruction> = words.iter().map(|&w| decode(w)).collect();

    // 2. Find all basic-block leaders: the entry, every branch/jump target
    //    that lands inside this function, and every instruction that follows a
    //    branch (fall-through target). This is the CFG-leader set that decides
    //    which vrams get their own `match` arm.
    let mut leaders: BTreeSet<u32> = BTreeSet::new();
    leaders.insert(base);
    let func_end = base + (words.len() as u32) * 4;
    for (i, instr) in instrs.iter().enumerate() {
        let vram = base + (i as u32) * 4;
        if let Some(target) = branch_target(instr, vram) {
            if target >= base && target < func_end {
                leaders.insert(target);
            }
        }
        if instr.has_delay_slot() {
            // The instruction two words down (after the delay slot) is a
            // fall-through leader.
            let after = vram + 8;
            if after >= base && after < func_end {
                leaders.insert(after);
            }
        }
    }

    // 3. Emit.
    let mut out = String::new();
    let _ = writeln!(
        out,
        "// Recompiled from MIPS function `{}` @ {:#010X} ({} instructions).",
        func.name,
        base,
        words.len()
    );
    let _ = writeln!(out, "// Emitted by fn64-recomp-native (typed Rust, no unsafe).");
    // A leaf function may not touch memory (or, degenerately, registers); the
    // fixed ABI signature keeps both params, so allow the unused-var lint per
    // function rather than second-guessing which params a body references.
    let _ = writeln!(out, "#[allow(unused_variables)]");
    let _ = writeln!(
        out,
        "pub fn {}(ctx: &mut RecompContext, mem: &mut Rdram) {{",
        func.name
    );
    let _ = writeln!(out, "    let mut pc: u32 = {:#010X};", base);
    let _ = writeln!(out, "    'run: loop {{ match pc {{");

    let mut i = 0;
    while i < instrs.len() {
        let vram = base + (i as u32) * 4;
        if leaders.contains(&vram) {
            let _ = writeln!(out, "        {:#010X} => {{", vram);
        }

        let instr = instrs[i];
        // Emit the original instruction as a comment (N64Recomp does this too).
        let _ = writeln!(out, "            // {:#010X}: {:?}", vram, instr);

        if instr.has_delay_slot() {
            // The next word is the delay slot. Emit the control transfer,
            // which consumes the delay slot.
            let delay = instrs.get(i + 1).copied();
            let delay_vram = vram + 4;
            emit_control_transfer(&mut out, instr, vram, delay, delay_vram, base, func_end);
            // Skip the delay-slot word; it was handled by the transfer.
            i += 2;
            // Close the arm if the next vram is a leader (new arm) or we ran off
            // the end.
            let _ = writeln!(out, "        }}");
            continue;
        } else {
            emit_straight(&mut out, instr, vram);
        }

        // Fall-through: if the NEXT instruction starts a new block, close this
        // arm with an explicit `pc =` assignment to it. Otherwise let the arm
        // continue straight-lining into the next instruction.
        let next_vram = vram + 4;
        let next_is_leader = leaders.contains(&next_vram) && next_vram < func_end;
        if next_is_leader {
            let _ = writeln!(out, "            pc = {:#010X};", next_vram);
            let _ = writeln!(out, "        }}");
        } else if next_vram >= func_end {
            // This straight-line instruction is the LAST word of the function
            // (e.g. a padding `nop` sitting after a `jr $ra` return, which is
            // common alignment tail). Its arm was opened but has no successor
            // to fall through to, so close it explicitly — otherwise the block
            // dangles into the `_ =>` catch-all and the emitted Rust has an
            // unbalanced brace. Assign `pc` past the function so the loop's
            // `_ =>` arm is the (unreachable) terminator.
            let _ = writeln!(out, "            pc = {:#010X};", next_vram);
            let _ = writeln!(out, "        }}");
        }
        i += 1;
    }

    let _ = writeln!(out, "        _ => unreachable!(\"jumped to unmapped vram {{:#X}}\", pc),");
    let _ = writeln!(out, "    }} }}");
    let _ = writeln!(out, "}}");
    out
}

/// The (in-function) target vram of a branch/jump instruction, if it has a
/// statically known one. `jr`/`jalr` (register-indirect) return `None`.
fn branch_target(instr: &Instruction, vram: u32) -> Option<u32> {
    use Instruction::*;
    let rel = |off: i16| vram.wrapping_add(4).wrapping_add((off as i32 as u32) << 2);
    match *instr {
        Beq { off, .. } | Bne { off, .. } | Beql { off, .. } | Bnel { off, .. } => Some(rel(off)),
        Blez { off, .. } | Bgtz { off, .. } | Blezl { off, .. } | Bgtzl { off, .. } => {
            Some(rel(off))
        }
        Bltz { off, .. } | Bgez { off, .. } | Bltzl { off, .. } | Bgezl { off, .. } => {
            Some(rel(off))
        }
        Bltzal { off, .. } | Bgezal { off, .. } => Some(rel(off)),
        // Absolute jumps: target = (delay_slot_pc & 0xF0000000) | (target << 2).
        J { target } | Jal { target } => {
            Some((vram.wrapping_add(4) & 0xF000_0000) | (target << 2))
        }
        _ => None,
    }
}

/// Register read expression as typed Rust. `$zero` folds to a literal `0`.
fn r(idx: Reg) -> String {
    if idx == 0 {
        "0i64 as u64".to_string()
    } else {
        format!("ctx.r({})", idx)
    }
}

/// Register read as `i32` (low word, signed).
fn rs32(idx: Reg) -> String {
    if idx == 0 {
        "0i32".to_string()
    } else {
        format!("ctx.r_s32({})", idx)
    }
}

/// Register read as `u32` (low word, unsigned).
fn ru32(idx: Reg) -> String {
    if idx == 0 {
        "0u32".to_string()
    } else {
        format!("ctx.r_u32({})", idx)
    }
}

/// Register read as `i64` (full register, signed) — the `SIGNED(reg)`/`ToS64`
/// operand for SLT/SLTI and the single-operand branches, which MIPS III
/// evaluates on the whole 64-bit register.
fn rs64(idx: Reg) -> String {
    if idx == 0 {
        "0i64".to_string()
    } else {
        format!("ctx.r_s64({})", idx)
    }
}

/// Register read as `u64` (full register, unsigned) — the `ToU64` operand for
/// SLTU/SLTIU.
fn ru64(idx: Reg) -> String {
    if idx == 0 {
        "0u64".to_string()
    } else {
        format!("ctx.r_u64({})", idx)
    }
}

/// Emit a straight-line (non-control-transfer) instruction as typed Rust.
fn emit_straight(out: &mut String, instr: Instruction, _vram: u32) {
    use Instruction::*;
    let line = |out: &mut String, s: String| {
        let _ = writeln!(out, "            {}", s);
    };
    match instr {
        Nop => line(out, "// nop".to_string()),

        // --- ALU immediate (results are 32-bit, sign-extended into GPR) ---
        Addi { rt, rs, imm } | Addiu { rt, rs, imm } => line(
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
            line(out, format!("ctx.set_r32({}, {:#X}i32);", rt, (imm as i32) << 16))
        }

        // --- ALU register ---
        Add { rd, rs, rt } | Addu { rd, rs, rt } => line(
            out,
            format!("ctx.set_r32({}, ({}).wrapping_add({}));", rd, rs32(rs), rs32(rt)),
        ),
        Sub { rd, rs, rt } | Subu { rd, rs, rt } => line(
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
            format!(
                "{{ let a = {}; let b = {}; if b != 0 {{ ctx.lo = a.wrapping_div(b) as i64 as u64; ctx.hi = a.wrapping_rem(b) as i64 as u64; }} }}",
                rs32(rs),
                rs32(rt)
            ),
        ),
        Divu { rs, rt } => line(
            out,
            format!(
                "{{ let a = {}; let b = {}; if b != 0 {{ ctx.lo = (a / b) as i32 as i64 as u64; ctx.hi = (a % b) as i32 as i64 as u64; }} }}",
                ru32(rs),
                ru32(rt)
            ),
        ),
        Mfhi { rd } => line(out, format!("ctx.set_r({}, ctx.hi);", rd)),
        Mflo { rd } => line(out, format!("ctx.set_r({}, ctx.lo);", rd)),
        Mthi { rs } => line(out, format!("ctx.hi = {};", r(rs))),
        Mtlo { rs } => line(out, format!("ctx.lo = {};", r(rs))),

        // --- Loads ---
        Lw { rt, base, off } => line(
            out,
            format!("ctx.set_r32({}, mem.load_w(Rdram::eff_addr({}, {})));", rt, r(base), off),
        ),
        Lh { rt, base, off } => line(
            out,
            format!(
                "ctx.set_r32({}, mem.load_h(Rdram::eff_addr({}, {})) as i32);",
                rt,
                r(base),
                off
            ),
        ),
        Lhu { rt, base, off } => line(
            out,
            format!(
                "ctx.set_r({}, mem.load_hu(Rdram::eff_addr({}, {})) as u64);",
                rt,
                r(base),
                off
            ),
        ),
        Lb { rt, base, off } => line(
            out,
            format!(
                "ctx.set_r32({}, mem.load_b(Rdram::eff_addr({}, {})) as i32);",
                rt,
                r(base),
                off
            ),
        ),
        Lbu { rt, base, off } => line(
            out,
            format!(
                "ctx.set_r({}, mem.load_bu(Rdram::eff_addr({}, {})) as u64);",
                rt,
                r(base),
                off
            ),
        ),
        Lwl { rt, base, off } => line(
            out,
            format!(
                "ctx.set_r32({}, mem.load_wl(ctx.r({}), Rdram::eff_addr({}, {})));",
                rt,
                rt,
                r(base),
                off
            ),
        ),
        Lwr { rt, base, off } => line(
            out,
            format!(
                "ctx.set_r32({}, mem.load_wr(ctx.r({}), Rdram::eff_addr({}, {})));",
                rt,
                rt,
                r(base),
                off
            ),
        ),

        // --- Stores ---
        Sw { rt, base, off } => line(
            out,
            format!("mem.store_w(Rdram::eff_addr({}, {}), {});", r(base), off, ru32(rt)),
        ),
        Sh { rt, base, off } => line(
            out,
            format!("mem.store_h(Rdram::eff_addr({}, {}), {} as u16);", r(base), off, ru32(rt)),
        ),
        Sb { rt, base, off } => line(
            out,
            format!("mem.store_b(Rdram::eff_addr({}, {}), {} as u8);", r(base), off, ru32(rt)),
        ),
        Swl { rt, base, off } => line(
            out,
            format!("mem.store_wl(Rdram::eff_addr({}, {}), {});", r(base), off, ru32(rt)),
        ),
        Swr { rt, base, off } => line(
            out,
            format!("mem.store_wr(Rdram::eff_addr({}, {}), {});", r(base), off, ru32(rt)),
        ),

        // --- COP0 system control ---
        //
        // Count (reg 9) and Compare (reg 11) are the only COP0 registers a
        // recompiled body legitimately touches (via osGetCount / the timer
        // path); they live in the typed context as real state. Every other
        // COP0 register (Status/Cause/EPC/…) is libultra-managed, and the raw
        // TLB / ERET ops are privileged — those become loud traps, never a
        // silent nop, so a game that unexpectedly executes one fails audibly.
        Mfc0 { rt, cop0d } => match cop0d {
            9 => line(out, format!("ctx.set_r32({}, ctx.cop0_count as i32);", rt)),
            11 => line(out, format!("ctx.set_r32({}, ctx.cop0_compare as i32);", rt)),
            other => line(
                out,
                format!(
                    "panic!(\"unsupported mfc0 from COP0 register {} (libultra-managed); \
                     fn64-recomp-native only models Count(9)/Compare(11)\");",
                    other
                ),
            ),
        },
        Mtc0 { rt, cop0d } => match cop0d {
            9 => line(out, format!("ctx.cop0_count = {};", ru32(rt))),
            11 => line(out, format!("ctx.cop0_compare = {};", ru32(rt))),
            other => line(
                out,
                format!(
                    "panic!(\"unsupported mtc0 to COP0 register {} (libultra-managed); \
                     fn64-recomp-native only models Count(9)/Compare(11)\");",
                    other
                ),
            ),
        },
        Dmfc0 { cop0d, .. } => line(
            out,
            format!(
                "panic!(\"unsupported dmfc0 from COP0 register {} (64-bit privileged access)\");",
                cop0d
            ),
        ),
        Dmtc0 { cop0d, .. } => line(
            out,
            format!(
                "panic!(\"unsupported dmtc0 to COP0 register {} (64-bit privileged access)\");",
                cop0d
            ),
        ),
        Eret => line(
            out,
            "panic!(\"eret executed in recompiled code: exception return is host/libultra territory\");"
                .to_string(),
        ),
        Tlbwi => line(out, "panic!(\"tlbwi: TLB is host-managed, not modeled\");".to_string()),
        Tlbwr => line(out, "panic!(\"tlbwr: TLB is host-managed, not modeled\");".to_string()),
        Tlbp => line(out, "panic!(\"tlbp: TLB is host-managed, not modeled\");".to_string()),
        Tlbr => line(out, "panic!(\"tlbr: TLB is host-managed, not modeled\");".to_string()),

        // --- Cache / sync: no-ops on a coherent host rdram ---
        Cache { op, .. } => {
            line(out, format!("// cache op {:#04X}: no-op (host rdram is coherent)", op))
        }
        Sync => line(out, "// sync: no-op (single-threaded recompiled context)".to_string()),

        // --- COP2: unused coprocessor, loud trap ---
        Mfc2 { .. } | Mtc2 { .. } | Cfc2 { .. } | Ctc2 { .. } => line(
            out,
            "panic!(\"COP2 access in recompiled code: COP2 is unused on the N64 and not modeled\");"
                .to_string(),
        ),

        // --- Traps ---
        Syscall { code } => line(
            out,
            format!("panic!(\"syscall (code {:#X}) executed in recompiled code\");", code),
        ),
        Break { code } => {
            line(out, format!("panic!(\"break (code {:#X}) executed in recompiled code\");", code))
        }

        // Control transfers are never emitted here.
        other => line(out, format!("compile_error!(\"non-straight op reached emit_straight: {:?}\");", other)),
    }
}

/// Emit a control transfer (branch/jump) plus its delay slot. The delay slot
/// runs first (unconditionally for normal branches; only when taken for
/// branch-likely). Then `pc` is assigned to the successor block.
fn emit_control_transfer(
    out: &mut String,
    instr: Instruction,
    vram: u32,
    delay: Option<Instruction>,
    delay_vram: u32,
    _base: u32,
    _func_end: u32,
) {
    use Instruction::*;
    let target = branch_target(&instr, vram);
    let fallthrough = delay_vram + 4;

    let emit_delay = |out: &mut String| {
        if let Some(d) = delay {
            let _ = writeln!(out, "            // delay: {:#010X}: {:?}", delay_vram, d);
            emit_straight(out, d, delay_vram);
        }
    };

    // Condition expression (Rust bool) for conditional branches.
    let cond = |instr: &Instruction| -> Option<String> {
        Some(match *instr {
            Beq { rs, rt, .. } => format!("{} == {}", r(rs), r(rt)),
            Bne { rs, rt, .. } => format!("{} != {}", r(rs), r(rt)),
            Beql { rs, rt, .. } => format!("{} == {}", r(rs), r(rt)),
            Bnel { rs, rt, .. } => format!("{} != {}", r(rs), r(rt)),
            Blez { rs, .. } | Blezl { rs, .. } => format!("{} <= 0", rs64(rs)),
            Bgtz { rs, .. } | Bgtzl { rs, .. } => format!("{} > 0", rs64(rs)),
            Bltz { rs, .. } | Bltzl { rs, .. } | Bltzal { rs, .. } => format!("{} < 0", rs64(rs)),
            Bgez { rs, .. } | Bgezl { rs, .. } | Bgezal { rs, .. } => format!("{} >= 0", rs64(rs)),
            _ => return None,
        })
    };

    match instr {
        Jr { rs } => {
            // `jr $ra` is a return; any other register is an indirect tail call.
            emit_delay(out);
            if rs == 31 {
                let _ = writeln!(out, "            return;");
            } else {
                let _ = writeln!(
                    out,
                    "            lookup(ctx.r_u32({}))(ctx, mem); return;",
                    rs
                );
            }
        }
        J { .. } => {
            emit_delay(out);
            let t = target.unwrap();
            let _ = writeln!(out, "            pc = {:#010X}; continue 'run;", t);
        }
        Jal { .. } => {
            // Link: $ra = address after the delay slot.
            let _ = writeln!(out, "            ctx.set_r32(31, {:#X}i32);", fallthrough as i32);
            emit_delay(out);
            let t = target.unwrap();
            let _ = writeln!(out, "            lookup({:#010X})(ctx, mem);", t);
            let _ = writeln!(out, "            pc = {:#010X}; continue 'run;", fallthrough);
        }
        Jalr { rd, rs } => {
            let link = if rd == 0 { 31 } else { rd };
            let _ = writeln!(out, "            ctx.set_r32({}, {:#X}i32);", link, fallthrough as i32);
            emit_delay(out);
            let _ = writeln!(out, "            lookup(ctx.r_u32({}))(ctx, mem);", rs);
            let _ = writeln!(out, "            pc = {:#010X}; continue 'run;", fallthrough);
        }
        Bltzal { .. } | Bgezal { .. } => {
            // Conditional branch-and-link.
            let c = cond(&instr).unwrap();
            let t = target.unwrap();
            let _ = writeln!(out, "            let _take = {};", c);
            let _ = writeln!(out, "            ctx.set_r32(31, {:#X}i32);", fallthrough as i32);
            emit_delay(out);
            let _ = writeln!(
                out,
                "            pc = if _take {{ {:#010X} }} else {{ {:#010X} }}; continue 'run;",
                t, fallthrough
            );
        }
        _ if instr.is_branch_likely() => {
            // Branch-likely: delay slot is executed ONLY when the branch is
            // taken. Evaluate condition, then run delay slot inside the taken
            // arm.
            let c = cond(&instr).unwrap();
            let t = target.unwrap();
            let _ = writeln!(out, "            if {} {{", c);
            emit_delay(out);
            let _ = writeln!(out, "                pc = {:#010X};", t);
            let _ = writeln!(out, "            }} else {{");
            let _ = writeln!(out, "                pc = {:#010X};", fallthrough);
            let _ = writeln!(out, "            }} continue 'run;");
        }
        _ => {
            // Normal conditional branch: delay slot runs unconditionally.
            let c = cond(&instr).unwrap();
            let t = target.unwrap();
            let _ = writeln!(out, "            let _take = {};", c);
            emit_delay(out);
            let _ = writeln!(
                out,
                "            pc = if _take {{ {:#010X} }} else {{ {:#010X} }}; continue 'run;",
                t, fallthrough
            );
        }
    }
}
