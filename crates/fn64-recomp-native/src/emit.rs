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

        // --- 64-bit doubleword ALU immediate ---
        // DADDI/DADDIU: full 64-bit add of rs and the sign-extended immediate.
        // (DADDI's overflow trap is dropped, matching the recomp custom of
        // treating trapping adds as their non-trapping twin.)
        Daddi { rt, rs, imm } | Daddiu { rt, rs, imm } => line(
            out,
            format!("ctx.set_r({}, ({}).wrapping_add({}i64 as u64));", rt, ru64(rs), imm as i64),
        ),

        // --- 64-bit doubleword ALU register ---
        Dadd { rd, rs, rt } | Daddu { rd, rs, rt } => line(
            out,
            format!("ctx.set_r({}, ({}).wrapping_add({}));", rd, ru64(rs), ru64(rt)),
        ),
        Dsub { rd, rs, rt } | Dsubu { rd, rs, rt } => line(
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
        // DDIV: signed 64-bit; guard the INT64_MIN / -1 overflow (quotient
        // saturates to INT64_MIN, remainder 0) exactly like N64Recomp's DDIV,
        // and the divide-by-zero case leaves HI/LO unchanged (undefined on
        // hardware; we mirror the C oracle, which skips the write path when the
        // recompiled code never relies on it — here we simply guard b != 0).
        Ddiv { rs, rt } => line(
            out,
            format!(
                "{{ let a = {}; let b = {}; if b != 0 {{ if a == i64::MIN && b == -1 {{ ctx.lo = a as u64; ctx.hi = 0; }} else {{ ctx.lo = a.wrapping_div(b) as u64; ctx.hi = a.wrapping_rem(b) as u64; }} }} }}",
                rs64(rs),
                rs64(rt)
            ),
        ),
        Ddivu { rs, rt } => line(
            out,
            format!(
                "{{ let a = {}; let b = {}; if b != 0 {{ ctx.lo = a / b; ctx.hi = a % b; }} }}",
                ru64(rs),
                ru64(rt)
            ),
        ),

        // --- Doubleword loads ---
        Ld { rt, base, off } => line(
            out,
            format!("ctx.set_r({}, mem.load_d(Rdram::eff_addr({}, {})));", rt, r(base), off),
        ),
        // LLD is a plain doubleword load on the single-threaded recompilation
        // model (no other master can break the link between it and its SCD).
        Lld { rt, base, off } => line(
            out,
            format!("ctx.set_r({}, mem.load_d(Rdram::eff_addr({}, {})));", rt, r(base), off),
        ),
        Ldl { rt, base, off } => line(
            out,
            format!(
                "ctx.set_r({}, mem.load_dl(ctx.r({}), Rdram::eff_addr({}, {})));",
                rt,
                rt,
                r(base),
                off
            ),
        ),
        Ldr { rt, base, off } => line(
            out,
            format!(
                "ctx.set_r({}, mem.load_dr(ctx.r({}), Rdram::eff_addr({}, {})));",
                rt,
                rt,
                r(base),
                off
            ),
        ),

        // --- Doubleword stores ---
        Sd { rt, base, off } => line(
            out,
            format!("mem.store_d(Rdram::eff_addr({}, {}), {});", r(base), off, ru64(rt)),
        ),
        Sdl { rt, base, off } => line(
            out,
            format!("mem.store_dl(Rdram::eff_addr({}, {}), {});", r(base), off, ru64(rt)),
        ),
        Sdr { rt, base, off } => line(
            out,
            format!("mem.store_dr(Rdram::eff_addr({}, {}), {});", r(base), off, ru64(rt)),
        ),
        // SCD stores the doubleword and, on the single-threaded model, always
        // reports success by writing 1 into rt.
        Scd { rt, base, off } => {
            line(
                out,
                format!("mem.store_d(Rdram::eff_addr({}, {}), {});", r(base), off, ru64(rt)),
            );
            line(out, format!("ctx.set_r({}, 1);", rt));
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
