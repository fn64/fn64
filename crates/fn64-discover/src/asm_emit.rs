//! GNU `as` assembly-text emission from exact fn64 function owners and the
//! shared recompiler decoder's typed MIPS III IR.
//!
//! This module does not discover boundaries, classify words, or decode the
//! ISA. Its only authoritative geometry is [`ExactFunctionOwner`], and every
//! typed input is checked against [`fn64_recomp_rs::decode`] before emission.
//! Emission consumes caller-supplied words and is therefore independent of
//! whether the exact owner's bytes came from an affine ROM span or a
//! materialized output span.
//! A caller can explicitly retain an embedded or unresolved word as
//! [`AsmWord::Raw`]; it is then emitted numerically without inferring code or
//! a symbol.

use crate::cfg::region_target;
use crate::facts::BankAddr;
use crate::owner_proof::ExactFunctionOwner;
use fn64_recomp_rs::{decode, Instruction};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write;

/// One word in an exact owner's extent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AsmWord {
    /// A shared-decoder instruction, retaining the source word so emission
    /// can preserve encodings that need a numeric fallback.
    Instruction { word: u32, decoded: Instruction },
    /// Proven data, embedded tables, or otherwise unresolved content.
    Raw { word: u32 },
}

impl AsmWord {
    /// Decode through fn64's single ISA authority.
    pub fn decode(word: u32) -> Self {
        Self::Instruction {
            word,
            decoded: decode(word),
        }
    }

    /// Retain a word numerically without claiming that it is an instruction.
    pub fn raw(word: u32) -> Self {
        Self::Raw { word }
    }

    pub fn word(self) -> u32 {
        match self {
            Self::Instruction { word, .. } | Self::Raw { word } => word,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AsmEmitError {
    EmptyOwner {
        entry_pc: u32,
    },
    UnalignedExtent {
        entry_pc: u32,
        va_end: u32,
    },
    WordCount {
        expected: usize,
        actual: usize,
    },
    DecoderMismatch {
        pc: u32,
        word: u32,
        supplied: Instruction,
        decoded: Instruction,
    },
}

impl std::fmt::Display for AsmEmitError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::EmptyOwner { entry_pc } => {
                write!(f, "exact owner at {entry_pc:#010x} has an empty extent")
            }
            Self::UnalignedExtent { entry_pc, va_end } => write!(
                f,
                "exact owner extent [{entry_pc:#010x}, {va_end:#010x}) is not word-aligned"
            ),
            Self::WordCount { expected, actual } => {
                write!(f, "exact owner needs {expected} words, received {actual}")
            }
            Self::DecoderMismatch {
                pc,
                word,
                supplied,
                decoded,
            } => write!(
                f,
                "typed IR mismatch at {pc:#010x} for {word:#010x}: supplied {supplied:?}, shared decoder returned {decoded:?}"
            ),
        }
    }
}

impl std::error::Error for AsmEmitError {}

/// Emit one exact function as big-endian MIPS III GNU `as` text.
///
/// `proven_owners` is the symbol authority. A direct `jal` becomes a
/// deterministic symbol only when its bank-qualified target is an entry in
/// that exact-owner catalog; otherwise its resolved address stays numeric.
pub fn emit_function(
    owner: &ExactFunctionOwner,
    words: &[AsmWord],
    proven_owners: &[ExactFunctionOwner],
) -> Result<String, AsmEmitError> {
    let byte_len = owner.byte_len();
    if byte_len == 0 {
        return Err(AsmEmitError::EmptyOwner {
            entry_pc: owner.entry.pc,
        });
    }
    if !owner.entry.pc.is_multiple_of(4)
        || !owner.va_end.is_multiple_of(4)
        || !byte_len.is_multiple_of(4)
    {
        return Err(AsmEmitError::UnalignedExtent {
            entry_pc: owner.entry.pc,
            va_end: owner.va_end,
        });
    }
    let expected = (byte_len / 4) as usize;
    if words.len() != expected {
        return Err(AsmEmitError::WordCount {
            expected,
            actual: words.len(),
        });
    }

    for (index, item) in words.iter().enumerate() {
        if let AsmWord::Instruction {
            word,
            decoded: supplied,
        } = item
        {
            let decoded = decode(*word);
            if *supplied != decoded {
                return Err(AsmEmitError::DecoderMismatch {
                    pc: owner.entry.pc + index as u32 * 4,
                    word: *word,
                    supplied: *supplied,
                    decoded,
                });
            }
        }
    }

    let symbols: BTreeMap<BankAddr, String> = proven_owners
        .iter()
        .map(|proven| (proven.entry.clone(), function_symbol(&proven.entry)))
        .collect();
    let own_symbol = function_symbol(&owner.entry);
    let labels = branch_labels(owner, words);

    let mut out = String::new();
    writeln!(out, ".set noreorder").unwrap();
    writeln!(out, ".set noat").unwrap();
    writeln!(out, ".set nomacro").unwrap();
    writeln!(out, ".set mips3").unwrap();
    writeln!(out, ".option pic0").unwrap();
    writeln!(out, ".text").unwrap();
    writeln!(out, ".align 2").unwrap();

    for (address, symbol) in &symbols {
        if address.bank == owner.entry.bank && address.pc != owner.entry.pc {
            writeln!(out, ".equ {symbol}, {:#010x}", address.pc).unwrap();
        }
    }

    writeln!(out, ".globl {own_symbol}").unwrap();
    writeln!(out, ".type {own_symbol}, @function").unwrap();
    writeln!(out, "{own_symbol}:").unwrap();

    for (index, item) in words.iter().enumerate() {
        let pc = owner.entry.pc + index as u32 * 4;
        if labels.contains(&pc) {
            writeln!(out, "{}:", local_label(pc)).unwrap();
        }
        match item {
            AsmWord::Raw { word } => emit_raw(&mut out, *word),
            AsmWord::Instruction { word, decoded } => {
                emit_instruction(&mut out, pc, *word, *decoded, owner, &symbols)
            }
        }
    }

    writeln!(out, ".size {own_symbol}, .-{own_symbol}").unwrap();
    Ok(out)
}

/// Emit one contiguous proven-code region as GNU `as` text at its VA.
///
/// Unlike [`emit_function`], this makes **no ownership claim**: the extent is
/// CFG-proven code (one basic block, or a maximal run of contiguous blocks),
/// not a function boundary. Branches inside the region become local labels;
/// every transfer leaving it stays numeric, and no `jal` resolves to a
/// symbol. Used by whole-ROM round-trip verification where code is proven
/// but no exact owner exists.
pub fn emit_code_region(bank: &str, pc: u32, words: &[AsmWord]) -> Result<String, AsmEmitError> {
    let region = ExactFunctionOwner {
        entry: BankAddr::new(bank.to_owned(), pc),
        va_end: pc.wrapping_add((words.len() as u32).wrapping_mul(4)),
        // Emission never reads the backing span; this placeholder cannot
        // reach any consumer because the synthetic owner never leaves here.
        backing: crate::facts::BankBackingSpanV1::RomAffine {
            rom_space: crate::RomAddressSpace::Physical,
            rom_start: 0,
            rom_end: 0,
        },
        block_starts: vec![pc],
    };
    emit_function(&region, words, &[])
}

/// Stable assembler identifier for one bank-qualified exact owner.
pub fn function_symbol(address: &BankAddr) -> String {
    let mut bank = String::with_capacity(address.bank.len() * 2);
    for byte in address.bank.as_bytes() {
        write!(bank, "{byte:02x}").unwrap();
    }
    format!("fn64_b{bank}_{:08x}", address.pc)
}

fn branch_labels(owner: &ExactFunctionOwner, words: &[AsmWord]) -> BTreeSet<u32> {
    words
        .iter()
        .enumerate()
        .filter_map(|(index, item)| {
            let AsmWord::Instruction { decoded, .. } = item else {
                return None;
            };
            let pc = owner.entry.pc + index as u32 * 4;
            let target = branch_offset(*decoded)
                .map(|off| pc.wrapping_add(4).wrapping_add((off as i32 as u32) << 2))?;
            (target >= owner.entry.pc && target < owner.va_end && target.is_multiple_of(4))
                .then_some(target)
        })
        .collect()
}

/// The absolute target of a PC-relative branch at `pc`, or `None` for
/// non-branch instructions. Callers use this to decide whether a branch can
/// be emitted symbolically (target inside the emitted extent) or must be
/// retained numerically — GNU `as` resolves absolute branch operands against
/// section-relative addresses, so a branch out of the emitted extent cannot
/// assemble as a mnemonic.
pub fn branch_target(pc: u32, instruction: Instruction) -> Option<u32> {
    branch_offset(instruction)
        .map(|off| pc.wrapping_add(4).wrapping_add((off as i32 as u32) << 2))
}

fn branch_offset(instruction: Instruction) -> Option<i16> {
    use Instruction::*;
    match instruction {
        Beq { off, .. }
        | Bne { off, .. }
        | Blez { off, .. }
        | Bgtz { off, .. }
        | Bltz { off, .. }
        | Bgez { off, .. }
        | Bltzal { off, .. }
        | Bgezal { off, .. }
        | Bltzall { off, .. }
        | Bgezall { off, .. }
        | Beql { off, .. }
        | Bnel { off, .. }
        | Blezl { off, .. }
        | Bgtzl { off, .. }
        | Bltzl { off, .. }
        | Bgezl { off, .. }
        | Bc1t { off }
        | Bc1f { off }
        | Bc1tl { off }
        | Bc1fl { off }
        | Bc0t { off }
        | Bc0f { off }
        | Bc0tl { off }
        | Bc0fl { off } => Some(off),
        _ => None,
    }
}

fn local_label(pc: u32) -> String {
    format!(".L_{pc:08x}")
}

fn branch_operand(pc: u32, off: i16, owner: &ExactFunctionOwner) -> String {
    let target = pc.wrapping_add(4).wrapping_add((off as i32 as u32) << 2);
    if target >= owner.entry.pc && target < owner.va_end && target.is_multiple_of(4) {
        local_label(target)
    } else {
        format!("{target:#010x}")
    }
}

fn gpr(reg: u8) -> String {
    format!("${reg}")
}

fn fpr(reg: u8) -> String {
    format!("$f{reg}")
}

fn emit_raw(out: &mut String, word: u32) {
    writeln!(out, "    .word {word:#010x}").unwrap();
}

fn emit_instruction(
    out: &mut String,
    pc: u32,
    word: u32,
    instruction: Instruction,
    owner: &ExactFunctionOwner,
    symbols: &BTreeMap<BankAddr, String>,
) {
    use Instruction::*;

    macro_rules! line {
        ($mnemonic:literal) => {
            writeln!(out, concat!("    ", $mnemonic)).unwrap()
        };
        ($mnemonic:literal, $($arg:expr),+ $(,)?) => {
            writeln!(out, concat!("    ", $mnemonic), $($arg),+).unwrap()
        };
    }
    macro_rules! mem {
        ($mnemonic:literal, $rt:expr, $base:expr, $off:expr) => {
            writeln!(
                out,
                concat!("    ", $mnemonic, " {},{}({})"),
                gpr($rt),
                $off,
                gpr($base)
            )
            .unwrap()
        };
    }
    macro_rules! fmem {
        ($mnemonic:literal, $ft:expr, $base:expr, $off:expr) => {
            writeln!(
                out,
                concat!("    ", $mnemonic, " {},{}({})"),
                fpr($ft),
                $off,
                gpr($base)
            )
            .unwrap()
        };
    }
    macro_rules! rrr {
        ($mnemonic:literal, $rd:expr, $rs:expr, $rt:expr) => {
            writeln!(
                out,
                concat!("    ", $mnemonic, " {},{},{}"),
                gpr($rd),
                gpr($rs),
                gpr($rt)
            )
            .unwrap()
        };
    }
    macro_rules! rri {
        ($mnemonic:literal, $rt:expr, $rs:expr, $imm:expr) => {
            writeln!(
                out,
                concat!("    ", $mnemonic, " {},{},{}"),
                gpr($rt),
                gpr($rs),
                $imm
            )
            .unwrap()
        };
    }
    macro_rules! fff {
        ($mnemonic:literal, $fd:expr, $fs:expr, $ft:expr) => {
            writeln!(
                out,
                concat!("    ", $mnemonic, " {},{},{}"),
                fpr($fd),
                fpr($fs),
                fpr($ft)
            )
            .unwrap()
        };
    }
    macro_rules! ff {
        ($mnemonic:literal, $fd:expr, $fs:expr) => {
            writeln!(
                out,
                concat!("    ", $mnemonic, " {},{}"),
                fpr($fd),
                fpr($fs)
            )
            .unwrap()
        };
    }

    match instruction {
        Nop => line!("nop"),
        Lb { rt, base, off } => mem!("lb", rt, base, off),
        Lbu { rt, base, off } => mem!("lbu", rt, base, off),
        Lh { rt, base, off } => mem!("lh", rt, base, off),
        Lhu { rt, base, off } => mem!("lhu", rt, base, off),
        Lw { rt, base, off } => mem!("lw", rt, base, off),
        Lwu { rt, base, off } => mem!("lwu", rt, base, off),
        Lwl { rt, base, off } => mem!("lwl", rt, base, off),
        Lwr { rt, base, off } => mem!("lwr", rt, base, off),
        Sb { rt, base, off } => mem!("sb", rt, base, off),
        Sh { rt, base, off } => mem!("sh", rt, base, off),
        Sw { rt, base, off } => mem!("sw", rt, base, off),
        Swl { rt, base, off } => mem!("swl", rt, base, off),
        Swr { rt, base, off } => mem!("swr", rt, base, off),
        Ld { rt, base, off } => mem!("ld", rt, base, off),
        Sd { rt, base, off } => mem!("sd", rt, base, off),
        Ldl { rt, base, off } => mem!("ldl", rt, base, off),
        Ldr { rt, base, off } => mem!("ldr", rt, base, off),
        Sdl { rt, base, off } => mem!("sdl", rt, base, off),
        Sdr { rt, base, off } => mem!("sdr", rt, base, off),
        Lld { rt, base, off } => mem!("lld", rt, base, off),
        Scd { rt, base, off } => mem!("scd", rt, base, off),
        Ll { rt, base, off } => mem!("ll", rt, base, off),
        Sc { rt, base, off } => mem!("sc", rt, base, off),

        Addi { rt, rs, imm } => rri!("addi", rt, rs, imm),
        Addiu { rt, rs, imm } => rri!("addiu", rt, rs, imm),
        Daddi { rt, rs, imm } => rri!("daddi", rt, rs, imm),
        Daddiu { rt, rs, imm } => rri!("daddiu", rt, rs, imm),
        Slti { rt, rs, imm } => rri!("slti", rt, rs, imm),
        Sltiu { rt, rs, imm } => rri!("sltiu", rt, rs, imm),
        Andi { rt, rs, imm } => rri!("andi", rt, rs, format_args!("{imm:#06x}")),
        Ori { rt, rs, imm } => rri!("ori", rt, rs, format_args!("{imm:#06x}")),
        Xori { rt, rs, imm } => rri!("xori", rt, rs, format_args!("{imm:#06x}")),
        Lui { rt, imm } => line!("lui {},{:#06x}", gpr(rt), imm),

        Add { rd, rs, rt } => rrr!("add", rd, rs, rt),
        Addu { rd, rs, rt } => rrr!("addu", rd, rs, rt),
        Sub { rd, rs, rt } => rrr!("sub", rd, rs, rt),
        Subu { rd, rs, rt } => rrr!("subu", rd, rs, rt),
        Dadd { rd, rs, rt } => rrr!("dadd", rd, rs, rt),
        Daddu { rd, rs, rt } => rrr!("daddu", rd, rs, rt),
        Dsub { rd, rs, rt } => rrr!("dsub", rd, rs, rt),
        Dsubu { rd, rs, rt } => rrr!("dsubu", rd, rs, rt),
        And { rd, rs, rt } => rrr!("and", rd, rs, rt),
        Or { rd, rs, rt } => rrr!("or", rd, rs, rt),
        Xor { rd, rs, rt } => rrr!("xor", rd, rs, rt),
        Nor { rd, rs, rt } => rrr!("nor", rd, rs, rt),
        Slt { rd, rs, rt } => rrr!("slt", rd, rs, rt),
        Sltu { rd, rs, rt } => rrr!("sltu", rd, rs, rt),

        Sll { rd, rt, sa } => line!("sll {},{},{}", gpr(rd), gpr(rt), sa),
        Srl { rd, rt, sa } => line!("srl {},{},{}", gpr(rd), gpr(rt), sa),
        Sra { rd, rt, sa } => line!("sra {},{},{}", gpr(rd), gpr(rt), sa),
        Dsll { rd, rt, sa } => line!("dsll {},{},{}", gpr(rd), gpr(rt), sa),
        Dsrl { rd, rt, sa } => line!("dsrl {},{},{}", gpr(rd), gpr(rt), sa),
        Dsra { rd, rt, sa } => line!("dsra {},{},{}", gpr(rd), gpr(rt), sa),
        Dsll32 { rd, rt, sa } => line!("dsll32 {},{},{}", gpr(rd), gpr(rt), sa),
        Dsrl32 { rd, rt, sa } => line!("dsrl32 {},{},{}", gpr(rd), gpr(rt), sa),
        Dsra32 { rd, rt, sa } => line!("dsra32 {},{},{}", gpr(rd), gpr(rt), sa),
        Sllv { rd, rt, rs } => rrr!("sllv", rd, rt, rs),
        Srlv { rd, rt, rs } => rrr!("srlv", rd, rt, rs),
        Srav { rd, rt, rs } => rrr!("srav", rd, rt, rs),
        Dsllv { rd, rt, rs } => rrr!("dsllv", rd, rt, rs),
        Dsrlv { rd, rt, rs } => rrr!("dsrlv", rd, rt, rs),
        Dsrav { rd, rt, rs } => rrr!("dsrav", rd, rt, rs),

        Mult { rs, rt } => line!("mult {},{}", gpr(rs), gpr(rt)),
        Multu { rs, rt } => line!("multu {},{}", gpr(rs), gpr(rt)),
        Dmult { rs, rt } => line!("dmult {},{}", gpr(rs), gpr(rt)),
        Dmultu { rs, rt } => line!("dmultu {},{}", gpr(rs), gpr(rt)),
        // GNU `as` treats two-operand divide as a checked macro on some MIPS
        // modes. The raw form is the faithful representation here.
        Div { .. } | Divu { .. } | Ddiv { .. } | Ddivu { .. } => emit_raw(out, word),
        Mfhi { rd } => line!("mfhi {}", gpr(rd)),
        Mflo { rd } => line!("mflo {}", gpr(rd)),
        Mthi { rs } => line!("mthi {}", gpr(rs)),
        Mtlo { rs } => line!("mtlo {}", gpr(rs)),

        Beq { rs, rt, off } => line!(
            "beq {},{},{}",
            gpr(rs),
            gpr(rt),
            branch_operand(pc, off, owner)
        ),
        Bne { rs, rt, off } => line!(
            "bne {},{},{}",
            gpr(rs),
            gpr(rt),
            branch_operand(pc, off, owner)
        ),
        Blez { rs, off } => line!("blez {},{}", gpr(rs), branch_operand(pc, off, owner)),
        Bgtz { rs, off } => line!("bgtz {},{}", gpr(rs), branch_operand(pc, off, owner)),
        Bltz { rs, off } => line!("bltz {},{}", gpr(rs), branch_operand(pc, off, owner)),
        Bgez { rs, off } => line!("bgez {},{}", gpr(rs), branch_operand(pc, off, owner)),
        Bltzal { rs, off } => line!("bltzal {},{}", gpr(rs), branch_operand(pc, off, owner)),
        Bgezal { rs, off } => line!("bgezal {},{}", gpr(rs), branch_operand(pc, off, owner)),
        Bltzall { rs, off } => line!("bltzall {},{}", gpr(rs), branch_operand(pc, off, owner)),
        Bgezall { rs, off } => line!("bgezall {},{}", gpr(rs), branch_operand(pc, off, owner)),
        Beql { rs, rt, off } => line!(
            "beql {},{},{}",
            gpr(rs),
            gpr(rt),
            branch_operand(pc, off, owner)
        ),
        Bnel { rs, rt, off } => line!(
            "bnel {},{},{}",
            gpr(rs),
            gpr(rt),
            branch_operand(pc, off, owner)
        ),
        Blezl { rs, off } => line!("blezl {},{}", gpr(rs), branch_operand(pc, off, owner)),
        Bgtzl { rs, off } => line!("bgtzl {},{}", gpr(rs), branch_operand(pc, off, owner)),
        Bltzl { rs, off } => line!("bltzl {},{}", gpr(rs), branch_operand(pc, off, owner)),
        Bgezl { rs, off } => line!("bgezl {},{}", gpr(rs), branch_operand(pc, off, owner)),

        J { target } => line!("j {:#010x}", region_target(pc, target)),
        Jal { target } => {
            let target_pc = region_target(pc, target);
            let address = BankAddr::new(owner.entry.bank.clone(), target_pc);
            if let Some(symbol) = symbols.get(&address) {
                line!("jal {}", symbol);
            } else {
                line!("jal {:#010x}", target_pc);
            }
        }
        Jr { rs } => line!("jr {}", gpr(rs)),
        Jalr { rd, rs } => line!("jalr {},{}", gpr(rd), gpr(rs)),

        Mfc1 { rt, fs } => line!("mfc1 {},{}", gpr(rt), fpr(fs)),
        Mtc1 { rt, fs } => line!("mtc1 {},{}", gpr(rt), fpr(fs)),
        Dmfc1 { rt, fs } => line!("dmfc1 {},{}", gpr(rt), fpr(fs)),
        Dmtc1 { rt, fs } => line!("dmtc1 {},{}", gpr(rt), fpr(fs)),
        Cfc1 { rt, fs } => line!("cfc1 {},${}", gpr(rt), fs),
        Ctc1 { rt, fs } => line!("ctc1 {},${}", gpr(rt), fs),
        Lwc1 { ft, base, off } => fmem!("lwc1", ft, base, off),
        Swc1 { ft, base, off } => fmem!("swc1", ft, base, off),
        Ldc1 { ft, base, off } => fmem!("ldc1", ft, base, off),
        Sdc1 { ft, base, off } => fmem!("sdc1", ft, base, off),

        AddS { fd, fs, ft } => fff!("add.s", fd, fs, ft),
        SubS { fd, fs, ft } => fff!("sub.s", fd, fs, ft),
        MulS { fd, fs, ft } => fff!("mul.s", fd, fs, ft),
        DivS { fd, fs, ft } => fff!("div.s", fd, fs, ft),
        AbsS { fd, fs } => ff!("abs.s", fd, fs),
        NegS { fd, fs } => ff!("neg.s", fd, fs),
        SqrtS { fd, fs } => ff!("sqrt.s", fd, fs),
        MovS { fd, fs } => ff!("mov.s", fd, fs),
        MovcfS { fd, fs, tf } => line!(
            "{} {},{},$fcc0",
            if tf { "movt.s" } else { "movf.s" },
            fpr(fd),
            fpr(fs)
        ),
        MovzS { fd, fs, rt } => line!("movz.s {},{},{}", fpr(fd), fpr(fs), gpr(rt)),
        MovnS { fd, fs, rt } => line!("movn.s {},{},{}", fpr(fd), fpr(fs), gpr(rt)),
        AddD { fd, fs, ft } => fff!("add.d", fd, fs, ft),
        SubD { fd, fs, ft } => fff!("sub.d", fd, fs, ft),
        MulD { fd, fs, ft } => fff!("mul.d", fd, fs, ft),
        DivD { fd, fs, ft } => fff!("div.d", fd, fs, ft),
        AbsD { fd, fs } => ff!("abs.d", fd, fs),
        NegD { fd, fs } => ff!("neg.d", fd, fs),
        SqrtD { fd, fs } => ff!("sqrt.d", fd, fs),
        MovD { fd, fs } => ff!("mov.d", fd, fs),
        MovcfD { fd, fs, tf } => line!(
            "{} {},{},$fcc0",
            if tf { "movt.d" } else { "movf.d" },
            fpr(fd),
            fpr(fs)
        ),
        MovzD { fd, fs, rt } => line!("movz.d {},{},{}", fpr(fd), fpr(fs), gpr(rt)),
        MovnD { fd, fs, rt } => line!("movn.d {},{},{}", fpr(fd), fpr(fs), gpr(rt)),

        CvtSW { fd, fs } => ff!("cvt.s.w", fd, fs),
        CvtDW { fd, fs } => ff!("cvt.d.w", fd, fs),
        CvtSD { fd, fs } => ff!("cvt.s.d", fd, fs),
        CvtDS { fd, fs } => ff!("cvt.d.s", fd, fs),
        CvtSL { fd, fs } => ff!("cvt.s.l", fd, fs),
        CvtDL { fd, fs } => ff!("cvt.d.l", fd, fs),
        CvtWS { fd, fs } => ff!("cvt.w.s", fd, fs),
        CvtWD { fd, fs } => ff!("cvt.w.d", fd, fs),
        CvtLS { fd, fs } => ff!("cvt.l.s", fd, fs),
        CvtLD { fd, fs } => ff!("cvt.l.d", fd, fs),
        TruncWS { fd, fs } => ff!("trunc.w.s", fd, fs),
        TruncWD { fd, fs } => ff!("trunc.w.d", fd, fs),
        TruncLS { fd, fs } => ff!("trunc.l.s", fd, fs),
        TruncLD { fd, fs } => ff!("trunc.l.d", fd, fs),
        RoundWS { fd, fs } => ff!("round.w.s", fd, fs),
        CeilWS { fd, fs } => ff!("ceil.w.s", fd, fs),
        FloorWS { fd, fs } => ff!("floor.w.s", fd, fs),
        RoundLS { fd, fs } => ff!("round.l.s", fd, fs),
        CeilLS { fd, fs } => ff!("ceil.l.s", fd, fs),
        FloorLS { fd, fs } => ff!("floor.l.s", fd, fs),
        RoundWD { fd, fs } => ff!("round.w.d", fd, fs),
        CeilWD { fd, fs } => ff!("ceil.w.d", fd, fs),
        FloorWD { fd, fs } => ff!("floor.w.d", fd, fs),
        RoundLD { fd, fs } => ff!("round.l.d", fd, fs),
        CeilLD { fd, fs } => ff!("ceil.l.d", fd, fs),
        FloorLD { fd, fs } => ff!("floor.l.d", fd, fs),

        CEqS { fs, ft } => line!("c.eq.s {},{}", fpr(fs), fpr(ft)),
        CLtS { fs, ft } => line!("c.lt.s {},{}", fpr(fs), fpr(ft)),
        CLeS { fs, ft } => line!("c.le.s {},{}", fpr(fs), fpr(ft)),
        CEqD { fs, ft } => line!("c.eq.d {},{}", fpr(fs), fpr(ft)),
        CLtD { fs, ft } => line!("c.lt.d {},{}", fpr(fs), fpr(ft)),
        CLeD { fs, ft } => line!("c.le.d {},{}", fpr(fs), fpr(ft)),
        CCondS { fs, ft, cond } => line!("c.{}.s {},{}", fp_condition(cond), fpr(fs), fpr(ft)),
        CCondD { fs, ft, cond } => line!("c.{}.d {},{}", fp_condition(cond), fpr(fs), fpr(ft)),
        Bc1t { off } => line!("bc1t {}", branch_operand(pc, off, owner)),
        Bc1f { off } => line!("bc1f {}", branch_operand(pc, off, owner)),
        Bc1tl { off } => line!("bc1tl {}", branch_operand(pc, off, owner)),
        Bc1fl { off } => line!("bc1fl {}", branch_operand(pc, off, owner)),

        Mfc0 { rt, cop0d } => line!("mfc0 {},${}", gpr(rt), cop0d),
        Mtc0 { rt, cop0d } => line!("mtc0 {},${}", gpr(rt), cop0d),
        Dmfc0 { rt, cop0d } => line!("dmfc0 {},${}", gpr(rt), cop0d),
        Dmtc0 { rt, cop0d } => line!("dmtc0 {},${}", gpr(rt), cop0d),
        Bc0f { off } => line!("bc0f {}", branch_operand(pc, off, owner)),
        Bc0t { off } => line!("bc0t {}", branch_operand(pc, off, owner)),
        Bc0fl { off } => line!("bc0fl {}", branch_operand(pc, off, owner)),
        Bc0tl { off } => line!("bc0tl {}", branch_operand(pc, off, owner)),
        Eret => line!("eret"),
        Tlbwi => line!("tlbwi"),
        Tlbwr => line!("tlbwr"),
        Tlbp => line!("tlbp"),
        Tlbr => line!("tlbr"),
        Cache { op, base, off } => line!("cache {},{}({})", op, off, gpr(base)),
        Sync if word == 0x0000_000f => line!("sync"),
        Sync => emit_raw(out, word),

        Mfc2 { rt, rd } => line!("mfc2 {},${}", gpr(rt), rd),
        Mtc2 { rt, rd } => line!("mtc2 {},${}", gpr(rt), rd),
        Cfc2 { rt, rd } => line!("cfc2 {},${}", gpr(rt), rd),
        Ctc2 { rt, rd } => line!("ctc2 {},${}", gpr(rt), rd),
        Dmfc2 { rt, rd } => line!("dmfc2 {},${}", gpr(rt), rd),
        Dmtc2 { rt, rd } => line!("dmtc2 {},${}", gpr(rt), rd),
        Lwc2 { rt, base, off } => mem!("lwc2", rt, base, off),
        Ldc2 { rt, base, off } => mem!("ldc2", rt, base, off),
        Swc2 { rt, base, off } => mem!("swc2", rt, base, off),
        Sdc2 { rt, base, off } => mem!("sdc2", rt, base, off),

        Tge { .. }
        | Tgeu { .. }
        | Tlt { .. }
        | Tltu { .. }
        | Teq { .. }
        | Tne { .. }
        | Tgei { .. }
        | Tgeiu { .. }
        | Tlti { .. }
        | Tltiu { .. }
        | Teqi { .. }
        | Tnei { .. }
        | Cop2Op { .. } => emit_raw(out, word),
        // GNU `as` accepts a narrower diagnostic operand than the shared
        // decoder retains for these encodings. Numeric emission preserves
        // all source bits without pretending the operand was representable.
        Syscall { .. } | Break { .. } => emit_raw(out, word),
        Unknown { .. } => emit_raw(out, word),
    }
}

fn fp_condition(cond: u8) -> &'static str {
    const NAMES: [&str; 16] = [
        "f", "un", "eq", "ueq", "olt", "ult", "ole", "ule", "sf", "ngle", "seq", "ngl", "lt",
        "nge", "le", "ngt",
    ];
    NAMES[usize::from(cond & 0x0f)]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::facts::{BankBackingSpanV1, RomAddressSpace};

    fn owner(start: u32, word_count: u32) -> ExactFunctionOwner {
        ExactFunctionOwner {
            entry: BankAddr::new("test", start),
            va_end: start + word_count * 4,
            backing: BankBackingSpanV1::RomAffine {
                rom_space: RomAddressSpace::Physical,
                rom_start: 0,
                rom_end: word_count * 4,
            },
            block_starts: vec![start],
        }
    }

    #[test]
    fn code_region_emits_without_ownership_claims() {
        let words = [
            AsmWord::decode(0x1100_0001), // beq $8,$0,+2   (in-region -> label)
            AsmWord::decode(0x0000_0000),
            AsmWord::raw(0x0001_7cd0),    // embedded table word, retained numerically
            AsmWord::decode(0x1100_2000), // beq $8,$0,+0x2001 (leaves region -> numeric)
            AsmWord::decode(0x0000_0000),
        ];
        let text = emit_code_region("test", 0x8000_1000, &words).unwrap();
        assert!(text.contains("    beq $8,$0,.L_80001008\n"));
        assert!(text.contains(".L_80001008:\n"));
        assert!(text.contains("    .word 0x00017cd0\n"));
        // The out-of-region branch keeps a numeric operand: callers must
        // retain such words raw before assembling, and this emission shape
        // is what makes that requirement visible.
        assert!(text.contains("    beq $8,$0,0x80009010\n"));
    }

    #[test]
    fn branch_target_reports_the_absolute_destination() {
        let AsmWord::Instruction { decoded, .. } = AsmWord::decode(0x1100_0001) else {
            panic!("beq decodes as an instruction");
        };
        assert_eq!(branch_target(0x8000_1000, decoded), Some(0x8000_1008));
        let AsmWord::Instruction { decoded, .. } = AsmWord::decode(0x012a_4020) else {
            panic!("add decodes as an instruction");
        };
        assert_eq!(branch_target(0x8000_1000, decoded), None);
    }

    #[test]
    fn integer_load_branch_and_symbolic_call_emit_as_instructions() {
        // Deliberately not 16-byte aligned: owner VAs are only word-aligned,
        // and the round trip must not inherit GNU ld's default text padding.
        let mut function = owner(0x8000_1004, 6);
        // Words cross this API explicitly, so their storage coordinates must
        // not limit otherwise identical assembly emission.
        function.backing = BankBackingSpanV1::Materialized {
            receipt_sha256: "11".repeat(32),
            output_start: 0,
            output_end: 24,
        };
        let callee = owner(0x8000_2000, 1);
        let words = [
            AsmWord::decode(0x012a_4020), // add $8,$9,$10
            AsmWord::decode(0x8d28_0010), // lw $8,16($9)
            AsmWord::decode(0x1100_0001), // beq $8,$0,.L_80001010
            AsmWord::decode(0x0000_0000),
            AsmWord::decode(0x0c00_0800), // jal 0x80002000
            AsmWord::decode(0x0000_0000),
        ];
        let text = emit_function(&function, &words, &[function.clone(), callee.clone()]).unwrap();
        assert!(text.contains("    add $8,$9,$10\n"));
        assert!(text.contains("    lw $8,16($9)\n"));
        assert!(text.contains("    beq $8,$0,.L_80001014\n"));
        assert!(text.contains(".L_80001014:\n"));
        assert!(text.contains(&format!("    jal {}\n", function_symbol(&callee.entry))));

        let toolchain_present = std::process::Command::new("mips-linux-gnu-as")
            .arg("--version")
            .output()
            .is_ok();
        if !toolchain_present {
            eprintln!("skipping GNU-as byte round trip: mips-linux-gnu-as is unavailable");
            return;
        }
        let temp = std::env::temp_dir().join(format!("fn64-asm-emit-test-{}", std::process::id()));
        std::fs::create_dir(&temp).unwrap();
        let source = temp.join("function.s");
        let object = temp.join("function.o");
        let linked = temp.join("function.elf");
        let binary = temp.join("function.bin");
        let linker_script = temp.join("function.ld");
        std::fs::write(&source, text).unwrap();
        std::fs::write(
            &linker_script,
            "SECTIONS { .text 0x80001004 : SUBALIGN(4) { *(.text) } }\n",
        )
        .unwrap();
        let assemble = std::process::Command::new("mips-linux-gnu-as")
            .args(["-EB", "-mips3", "-32", "-G", "0", "-o"])
            .arg(&object)
            .arg(&source)
            .output()
            .unwrap();
        assert!(
            assemble.status.success(),
            "assembler failed: {}",
            String::from_utf8_lossy(&assemble.stderr)
        );
        let link = std::process::Command::new("mips-linux-gnu-ld")
            .args(["-EB", "-m", "elf32btsmip", "-T"])
            .arg(&linker_script)
            .arg("-o")
            .arg(&linked)
            .arg(&object)
            .output()
            .unwrap();
        assert!(
            link.status.success(),
            "linker failed: {}",
            String::from_utf8_lossy(&link.stderr)
        );
        let extract = std::process::Command::new("mips-linux-gnu-objcopy")
            .args(["-O", "binary", "-j", ".text"])
            .arg(&linked)
            .arg(&binary)
            .output()
            .unwrap();
        assert!(
            extract.status.success(),
            "objcopy failed: {}",
            String::from_utf8_lossy(&extract.stderr)
        );
        let mut assembled = std::fs::read(&binary).unwrap();
        let expected: Vec<u8> = words
            .iter()
            .flat_map(|item| item.word().to_be_bytes())
            .collect();
        assembled.truncate(expected.len());
        let _ = std::fs::remove_dir_all(&temp);
        assert_eq!(assembled, expected);
    }

    #[test]
    fn unresolved_word_is_numeric() {
        let function = owner(0x8000_1000, 1);
        let text = emit_function(&function, &[AsmWord::raw(0xdead_beef)], &[]).unwrap();
        assert!(text.contains("    .word 0xdeadbeef\n"));
        assert!(!text.contains("dead_beef"));
    }

    #[test]
    fn rejects_ir_not_produced_by_shared_decoder() {
        let function = owner(0x8000_1000, 1);
        let error = emit_function(
            &function,
            &[AsmWord::Instruction {
                word: 0,
                decoded: Instruction::Add {
                    rd: 1,
                    rs: 2,
                    rt: 3,
                },
            }],
            &[],
        )
        .unwrap_err();
        assert!(matches!(error, AsmEmitError::DecoderMismatch { .. }));
    }
}
