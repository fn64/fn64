//! Instruction-word field extraction and the opcode dispatch tree.
//!
//! Split from the decoder module body; the enum and its inherent impl stay
//! in `mod.rs`. Everything here is private except [`decode`], which the
//! parent re-exports so `decoder::decode` remains the public path.

use super::{Instruction, Reg};

#[inline]
fn opcode(w: u32) -> u32 {
    (w >> 26) & 0x3F
}
#[inline]
fn rs(w: u32) -> Reg {
    ((w >> 21) & 0x1F) as Reg
}
#[inline]
fn rt(w: u32) -> Reg {
    ((w >> 16) & 0x1F) as Reg
}
#[inline]
fn rd(w: u32) -> Reg {
    ((w >> 11) & 0x1F) as Reg
}
#[inline]
fn sa(w: u32) -> u8 {
    ((w >> 6) & 0x1F) as u8
}
#[inline]
fn funct(w: u32) -> u32 {
    w & 0x3F
}
#[inline]
fn imm_u(w: u32) -> u16 {
    (w & 0xFFFF) as u16
}
#[inline]
fn imm_s(w: u32) -> i16 {
    (w & 0xFFFF) as u16 as i16
}
#[inline]
fn target26(w: u32) -> u32 {
    w & 0x03FF_FFFF
}
/// The COP0 register index a MFC0/MTC0/DMFC0/DMTC0 targets: the `rd` field
/// (bits 15..11), matching `rabbitizer`'s `Get_cop0d()` in N64Recomp.
#[inline]
fn cop0d(w: u32) -> u8 {
    ((w >> 11) & 0x1F) as u8
}
/// The 20-bit `code` field of SYSCALL/BREAK (bits 25..6). Diagnostic only.
#[inline]
fn code20(w: u32) -> u32 {
    (w >> 6) & 0x000F_FFFF
}
/// The 10-bit diagnostic code on SPECIAL trap instructions (bits 15..6).
#[inline]
fn code10(w: u32) -> u16 {
    ((w >> 6) & 0x03FF) as u16
}
/// The 5-bit cache-operation selector of CACHE (the `rt` field, bits 20..16).
#[inline]
fn cache_op(w: u32) -> u8 {
    ((w >> 16) & 0x1F) as u8
}

// --- COP1 field extraction. The `fmt` sub-op selector occupies the same bits
// as `rs` (25..21); `ft`/`fs`/`fd` occupy the `rt`/`rd`/`sa` positions but are
// FPU register indices. Named separately for clarity at the decode site.
#[inline]
fn fmt(w: u32) -> u32 {
    (w >> 21) & 0x1F
}
#[inline]
fn ft(w: u32) -> Reg {
    ((w >> 16) & 0x1F) as Reg
}
#[inline]
fn fs(w: u32) -> Reg {
    ((w >> 11) & 0x1F) as Reg
}
#[inline]
fn fd(w: u32) -> Reg {
    ((w >> 6) & 0x1F) as Reg
}

/// Decode a single 32-bit MIPS instruction word.
///
/// The word is interpreted as already host-endian (caller reads the ROM/RAM
/// big-endian word into a `u32`). Returns [`Instruction::Unknown`] for any
/// encoding not in the covered subset.
pub fn decode(w: u32) -> Instruction {
    use Instruction::*;

    if w == 0 {
        return Nop;
    }

    match opcode(w) {
        // SPECIAL: dispatch on funct (bits 5..0).
        0x00 => match funct(w) {
            // Shifts by immediate sa. (SLL $0,$0,0 is caught above as Nop.)
            0x00 => Sll {
                rd: rd(w),
                rt: rt(w),
                sa: sa(w),
            },
            0x02 => Srl {
                rd: rd(w),
                rt: rt(w),
                sa: sa(w),
            },
            0x03 => Sra {
                rd: rd(w),
                rt: rt(w),
                sa: sa(w),
            },
            0x04 => Sllv {
                rd: rd(w),
                rt: rt(w),
                rs: rs(w),
            },
            0x06 => Srlv {
                rd: rd(w),
                rt: rt(w),
                rs: rs(w),
            },
            0x07 => Srav {
                rd: rd(w),
                rt: rt(w),
                rs: rs(w),
            },
            // Doubleword variable shifts.
            0x14 => Dsllv {
                rd: rd(w),
                rt: rt(w),
                rs: rs(w),
            },
            0x16 => Dsrlv {
                rd: rd(w),
                rt: rt(w),
                rs: rs(w),
            },
            0x17 => Dsrav {
                rd: rd(w),
                rt: rt(w),
                rs: rs(w),
            },
            // Jumps.
            0x08 => Jr { rs: rs(w) },
            0x09 => Jalr {
                rd: rd(w),
                rs: rs(w),
            },
            // Traps + sync (SPECIAL funct 0x0C/0x0D/0x0F).
            0x0C => Syscall { code: code20(w) },
            0x0D => Break { code: code20(w) },
            0x0F => Sync,
            // HI/LO moves.
            0x10 => Mfhi { rd: rd(w) },
            0x11 => Mthi { rs: rs(w) },
            0x12 => Mflo { rd: rd(w) },
            0x13 => Mtlo { rs: rs(w) },
            // Mult/Div.
            0x18 => Mult {
                rs: rs(w),
                rt: rt(w),
            },
            0x19 => Multu {
                rs: rs(w),
                rt: rt(w),
            },
            0x1A => Div {
                rs: rs(w),
                rt: rt(w),
            },
            0x1B => Divu {
                rs: rs(w),
                rt: rt(w),
            },
            // Doubleword mult/div.
            0x1C => Dmult {
                rs: rs(w),
                rt: rt(w),
            },
            0x1D => Dmultu {
                rs: rs(w),
                rt: rt(w),
            },
            0x1E => Ddiv {
                rs: rs(w),
                rt: rt(w),
            },
            0x1F => Ddivu {
                rs: rs(w),
                rt: rt(w),
            },
            // ALU register.
            0x20 => Add {
                rd: rd(w),
                rs: rs(w),
                rt: rt(w),
            },
            0x21 => Addu {
                rd: rd(w),
                rs: rs(w),
                rt: rt(w),
            },
            0x22 => Sub {
                rd: rd(w),
                rs: rs(w),
                rt: rt(w),
            },
            0x23 => Subu {
                rd: rd(w),
                rs: rs(w),
                rt: rt(w),
            },
            0x24 => And {
                rd: rd(w),
                rs: rs(w),
                rt: rt(w),
            },
            0x25 => Or {
                rd: rd(w),
                rs: rs(w),
                rt: rt(w),
            },
            0x26 => Xor {
                rd: rd(w),
                rs: rs(w),
                rt: rt(w),
            },
            0x27 => Nor {
                rd: rd(w),
                rs: rs(w),
                rt: rt(w),
            },
            0x2A => Slt {
                rd: rd(w),
                rs: rs(w),
                rt: rt(w),
            },
            0x2B => Sltu {
                rd: rd(w),
                rs: rs(w),
                rt: rt(w),
            },
            // Doubleword ALU register.
            0x2C => Dadd {
                rd: rd(w),
                rs: rs(w),
                rt: rt(w),
            },
            0x2D => Daddu {
                rd: rd(w),
                rs: rs(w),
                rt: rt(w),
            },
            0x2E => Dsub {
                rd: rd(w),
                rs: rs(w),
                rt: rt(w),
            },
            0x2F => Dsubu {
                rd: rd(w),
                rs: rs(w),
                rt: rt(w),
            },
            // Conditional traps; code is bits 15..6 (MIPS IV ISA A-39).
            0x30 => Tge {
                rs: rs(w),
                rt: rt(w),
                code: code10(w),
            },
            0x31 => Tgeu {
                rs: rs(w),
                rt: rt(w),
                code: code10(w),
            },
            0x32 => Tlt {
                rs: rs(w),
                rt: rt(w),
                code: code10(w),
            },
            0x33 => Tltu {
                rs: rs(w),
                rt: rt(w),
                code: code10(w),
            },
            0x34 => Teq {
                rs: rs(w),
                rt: rt(w),
                code: code10(w),
            },
            0x36 => Tne {
                rs: rs(w),
                rt: rt(w),
                code: code10(w),
            },
            // Doubleword immediate shifts. DSLL/DSRL/DSRA use sa (0..31);
            // the *32 forms add 32 to the shift count (32..63).
            0x38 => Dsll {
                rd: rd(w),
                rt: rt(w),
                sa: sa(w),
            },
            0x3A => Dsrl {
                rd: rd(w),
                rt: rt(w),
                sa: sa(w),
            },
            0x3B => Dsra {
                rd: rd(w),
                rt: rt(w),
                sa: sa(w),
            },
            0x3C => Dsll32 {
                rd: rd(w),
                rt: rt(w),
                sa: sa(w),
            },
            0x3E => Dsrl32 {
                rd: rd(w),
                rt: rt(w),
                sa: sa(w),
            },
            0x3F => Dsra32 {
                rd: rd(w),
                rt: rt(w),
                sa: sa(w),
            },
            _ => Unknown { word: w },
        },
        // REGIMM: dispatch on the rt field (bits 20..16).
        0x01 => match rt(w) {
            0x00 => Bltz {
                rs: rs(w),
                off: imm_s(w),
            },
            0x01 => Bgez {
                rs: rs(w),
                off: imm_s(w),
            },
            0x02 => Bltzl {
                rs: rs(w),
                off: imm_s(w),
            },
            0x03 => Bgezl {
                rs: rs(w),
                off: imm_s(w),
            },
            0x08 => Tgei {
                rs: rs(w),
                imm: imm_s(w),
            },
            0x09 => Tgeiu {
                rs: rs(w),
                imm: imm_s(w),
            },
            0x0A => Tlti {
                rs: rs(w),
                imm: imm_s(w),
            },
            0x0B => Tltiu {
                rs: rs(w),
                imm: imm_s(w),
            },
            0x0C => Teqi {
                rs: rs(w),
                imm: imm_s(w),
            },
            0x0E => Tnei {
                rs: rs(w),
                imm: imm_s(w),
            },
            0x10 => Bltzal {
                rs: rs(w),
                off: imm_s(w),
            },
            0x11 => Bgezal {
                rs: rs(w),
                off: imm_s(w),
            },
            0x12 => Bltzall {
                rs: rs(w),
                off: imm_s(w),
            },
            0x13 => Bgezall {
                rs: rs(w),
                off: imm_s(w),
            },
            _ => Unknown { word: w },
        },
        // COP1 (FPU): opcode 0x11, sub-dispatched on `fmt` (bits 25..21).
        0x11 => decode_cop1(w),
        // COP0 (opcode 0x10): sub-dispatch on the rs/format field (bits 25..21).
        // rs bit 25 (the "CO" bit) selects the funct-encoded ops (ERET/TLB*).
        0x10 => {
            let fmt = rs(w);
            let instruction = if fmt & 0x10 != 0 {
                // CO=1: TLB / ERET, selected by funct (bits 5..0).
                match funct(w) {
                    0x01 => Tlbr,
                    0x02 => Tlbwi,
                    0x06 => Tlbwr,
                    0x08 => Tlbp,
                    0x18 => Eret,
                    _ => Unknown { word: w },
                }
            } else {
                match fmt {
                    0x00 => Mfc0 {
                        rt: rt(w),
                        cop0d: cop0d(w),
                    },
                    0x01 => Dmfc0 {
                        rt: rt(w),
                        cop0d: cop0d(w),
                    },
                    0x04 => Mtc0 {
                        rt: rt(w),
                        cop0d: cop0d(w),
                    },
                    0x05 => Dmtc0 {
                        rt: rt(w),
                        cop0d: cop0d(w),
                    },
                    0x08 => match rt(w) & 0x3 {
                        0 => Bc0f { off: imm_s(w) },
                        1 => Bc0t { off: imm_s(w) },
                        2 => Bc0fl { off: imm_s(w) },
                        3 => Bc0tl { off: imm_s(w) },
                        _ => unreachable!(),
                    },
                    _ => Unknown { word: w },
                }
            };
            assert!(
                matches!(instruction, Unknown { .. }) || instruction.requires_cop0(),
                "recognized COP0 decode omitted the mandatory authority classification: \
                 word={w:#010x} instruction={instruction:?}"
            );
            instruction
        }
        // COP2 (opcode 0x12): move ops, sub-dispatched on the rs/format field.
        0x12 => match rs(w) {
            0x00 => Mfc2 {
                rt: rt(w),
                rd: rd(w),
            },
            0x01 => Dmfc2 {
                rt: rt(w),
                rd: rd(w),
            },
            0x02 => Cfc2 {
                rt: rt(w),
                rd: rd(w),
            },
            0x04 => Mtc2 {
                rt: rt(w),
                rd: rd(w),
            },
            0x05 => Dmtc2 {
                rt: rt(w),
                rd: rd(w),
            },
            0x06 => Ctc2 {
                rt: rt(w),
                rd: rd(w),
            },
            _ => Cop2Op { word: w },
        },
        // J-type.
        0x02 => J {
            target: target26(w),
        },
        0x03 => Jal {
            target: target26(w),
        },
        // Branches (I-type).
        0x04 => Beq {
            rs: rs(w),
            rt: rt(w),
            off: imm_s(w),
        },
        0x05 => Bne {
            rs: rs(w),
            rt: rt(w),
            off: imm_s(w),
        },
        0x06 => Blez {
            rs: rs(w),
            off: imm_s(w),
        },
        0x07 => Bgtz {
            rs: rs(w),
            off: imm_s(w),
        },
        // ALU immediate.
        0x08 => Addi {
            rt: rt(w),
            rs: rs(w),
            imm: imm_s(w),
        },
        0x09 => Addiu {
            rt: rt(w),
            rs: rs(w),
            imm: imm_s(w),
        },
        0x0A => Slti {
            rt: rt(w),
            rs: rs(w),
            imm: imm_s(w),
        },
        0x0B => Sltiu {
            rt: rt(w),
            rs: rs(w),
            imm: imm_s(w),
        },
        0x0C => Andi {
            rt: rt(w),
            rs: rs(w),
            imm: imm_u(w),
        },
        0x0D => Ori {
            rt: rt(w),
            rs: rs(w),
            imm: imm_u(w),
        },
        0x0E => Xori {
            rt: rt(w),
            rs: rs(w),
            imm: imm_u(w),
        },
        0x0F => Lui {
            rt: rt(w),
            imm: imm_u(w),
        },
        // Doubleword ALU immediate.
        0x18 => Daddi {
            rt: rt(w),
            rs: rs(w),
            imm: imm_s(w),
        },
        0x19 => Daddiu {
            rt: rt(w),
            rs: rs(w),
            imm: imm_s(w),
        },
        // Doubleword unaligned loads.
        0x1A => Ldl {
            rt: rt(w),
            base: rs(w),
            off: imm_s(w),
        },
        0x1B => Ldr {
            rt: rt(w),
            base: rs(w),
            off: imm_s(w),
        },
        // Branch-likely.
        0x14 => Beql {
            rs: rs(w),
            rt: rt(w),
            off: imm_s(w),
        },
        0x15 => Bnel {
            rs: rs(w),
            rt: rt(w),
            off: imm_s(w),
        },
        0x16 => Blezl {
            rs: rs(w),
            off: imm_s(w),
        },
        0x17 => Bgtzl {
            rs: rs(w),
            off: imm_s(w),
        },
        // Loads.
        0x20 => Lb {
            rt: rt(w),
            base: rs(w),
            off: imm_s(w),
        },
        0x21 => Lh {
            rt: rt(w),
            base: rs(w),
            off: imm_s(w),
        },
        0x22 => Lwl {
            rt: rt(w),
            base: rs(w),
            off: imm_s(w),
        },
        0x23 => Lw {
            rt: rt(w),
            base: rs(w),
            off: imm_s(w),
        },
        0x24 => Lbu {
            rt: rt(w),
            base: rs(w),
            off: imm_s(w),
        },
        0x25 => Lhu {
            rt: rt(w),
            base: rs(w),
            off: imm_s(w),
        },
        0x26 => Lwr {
            rt: rt(w),
            base: rs(w),
            off: imm_s(w),
        },
        0x27 => Lwu {
            rt: rt(w),
            base: rs(w),
            off: imm_s(w),
        },
        // Stores.
        0x28 => Sb {
            rt: rt(w),
            base: rs(w),
            off: imm_s(w),
        },
        0x29 => Sh {
            rt: rt(w),
            base: rs(w),
            off: imm_s(w),
        },
        0x2A => Swl {
            rt: rt(w),
            base: rs(w),
            off: imm_s(w),
        },
        0x2B => Sw {
            rt: rt(w),
            base: rs(w),
            off: imm_s(w),
        },
        0x2E => Swr {
            rt: rt(w),
            base: rs(w),
            off: imm_s(w),
        },
        // Doubleword unaligned stores.
        0x2C => Sdl {
            rt: rt(w),
            base: rs(w),
            off: imm_s(w),
        },
        0x2D => Sdr {
            rt: rt(w),
            base: rs(w),
            off: imm_s(w),
        },
        // Cache operation (I-type: base=rs, op=rt field, signed offset).
        0x2F => Cache {
            op: cache_op(w),
            base: rs(w),
            off: imm_s(w),
        },
        // Load-linked / store-conditional word and doubleword.
        0x30 => Ll {
            rt: rt(w),
            base: rs(w),
            off: imm_s(w),
        },
        0x38 => Sc {
            rt: rt(w),
            base: rs(w),
            off: imm_s(w),
        },
        0x34 => Lld {
            rt: rt(w),
            base: rs(w),
            off: imm_s(w),
        },
        0x3C => Scd {
            rt: rt(w),
            base: rs(w),
            off: imm_s(w),
        },
        // Aligned doubleword load/store.
        0x37 => Ld {
            rt: rt(w),
            base: rs(w),
            off: imm_s(w),
        },
        0x3F => Sd {
            rt: rt(w),
            base: rs(w),
            off: imm_s(w),
        },
        // COP1 loads/stores (dedicated main opcodes).
        0x31 => Lwc1 {
            ft: rt(w),
            base: rs(w),
            off: imm_s(w),
        },
        0x35 => Ldc1 {
            ft: rt(w),
            base: rs(w),
            off: imm_s(w),
        },
        0x39 => Swc1 {
            ft: rt(w),
            base: rs(w),
            off: imm_s(w),
        },
        0x3D => Sdc1 {
            ft: rt(w),
            base: rs(w),
            off: imm_s(w),
        },
        // COP2 memory encodings are architectural even though COP2 is absent.
        0x32 => Lwc2 {
            rt: rt(w),
            base: rs(w),
            off: imm_s(w),
        },
        0x36 => Ldc2 {
            rt: rt(w),
            base: rs(w),
            off: imm_s(w),
        },
        0x3A => Swc2 {
            rt: rt(w),
            base: rs(w),
            off: imm_s(w),
        },
        0x3E => Sdc2 {
            rt: rt(w),
            base: rs(w),
            off: imm_s(w),
        },
        _ => Unknown { word: w },
    }
}

/// Decode a COP1 (opcode 0x11) instruction, sub-dispatched on the `fmt` field.
///
/// Clean-room from the MIPS III / VR4300 reference: the `fmt` values below are
/// the documented COP1 format/sub-op encodings (MFC1=0, DMFC1=1, CFC1=2,
/// MTC1=4, DMTC1=5, CTC1=6, BC1=8, S=0x10, D=0x11, W=0x14, L=0x15). Within the
/// S/D formats the `funct` field (5..0) picks the operation; the conversion
/// and compare `funct` values are likewise documented facts.
fn decode_cop1(w: u32) -> Instruction {
    use Instruction::*;
    match fmt(w) {
        // GPR<->FPR moves.
        0x00 => Mfc1 {
            rt: ft(w),
            fs: fs(w),
        },
        0x01 => Dmfc1 {
            rt: ft(w),
            fs: fs(w),
        },
        0x02 => Cfc1 {
            rt: ft(w),
            fs: fs(w),
        },
        0x04 => Mtc1 {
            rt: ft(w),
            fs: fs(w),
        },
        0x05 => Dmtc1 {
            rt: ft(w),
            fs: fs(w),
        },
        0x06 => Ctc1 {
            rt: ft(w),
            fs: fs(w),
        },
        // BC1: ft field carries tf (bit 0) and nd (bit 1, branch-likely).
        0x08 => match ft(w) & 0x3 {
            0x00 => Bc1f { off: imm_s(w) },
            0x01 => Bc1t { off: imm_s(w) },
            0x02 => Bc1fl { off: imm_s(w) },
            0x03 => Bc1tl { off: imm_s(w) },
            _ => Unknown { word: w },
        },
        // Single-precision format.
        0x10 => decode_cop1_s(w),
        // Double-precision format.
        0x11 => decode_cop1_d(w),
        // Fixed-point-source conversions: W (int32 source) and L (int64 source).
        0x14 => match funct(w) {
            0x20 => CvtSW {
                fd: fd(w),
                fs: fs(w),
            },
            0x21 => CvtDW {
                fd: fd(w),
                fs: fs(w),
            },
            _ => Unknown { word: w },
        },
        0x15 => match funct(w) {
            0x20 => CvtSL {
                fd: fd(w),
                fs: fs(w),
            },
            0x21 => CvtDL {
                fd: fd(w),
                fs: fs(w),
            },
            _ => Unknown { word: w },
        },
        _ => Unknown { word: w },
    }
}

/// Single-precision (fmt = S = 0x10) `funct` sub-dispatch.
fn decode_cop1_s(w: u32) -> Instruction {
    use Instruction::*;
    match funct(w) {
        0x00 => AddS {
            fd: fd(w),
            fs: fs(w),
            ft: ft(w),
        },
        0x01 => SubS {
            fd: fd(w),
            fs: fs(w),
            ft: ft(w),
        },
        0x02 => MulS {
            fd: fd(w),
            fs: fs(w),
            ft: ft(w),
        },
        0x03 => DivS {
            fd: fd(w),
            fs: fs(w),
            ft: ft(w),
        },
        0x04 => SqrtS {
            fd: fd(w),
            fs: fs(w),
        },
        0x05 => AbsS {
            fd: fd(w),
            fs: fs(w),
        },
        0x06 => MovS {
            fd: fd(w),
            fs: fs(w),
        },
        0x07 => NegS {
            fd: fd(w),
            fs: fs(w),
        },
        // Conditional moves. MOVF/MOVT share funct 0x11; the `tf` bit is bit 16
        // (the low bit of the ft field). MOVZ=0x12, MOVN=0x13 name a GPR in ft.
        0x11 => MovcfS {
            fd: fd(w),
            fs: fs(w),
            tf: (w >> 16) & 1 != 0,
        },
        0x12 => MovzS {
            fd: fd(w),
            fs: fs(w),
            rt: ft(w),
        },
        0x13 => MovnS {
            fd: fd(w),
            fs: fs(w),
            rt: ft(w),
        },
        0x08 => RoundLS {
            fd: fd(w),
            fs: fs(w),
        },
        0x09 => TruncLS {
            fd: fd(w),
            fs: fs(w),
        },
        0x0A => CeilLS {
            fd: fd(w),
            fs: fs(w),
        },
        0x0B => FloorLS {
            fd: fd(w),
            fs: fs(w),
        },
        0x0C => RoundWS {
            fd: fd(w),
            fs: fs(w),
        },
        0x0D => TruncWS {
            fd: fd(w),
            fs: fs(w),
        },
        0x0E => CeilWS {
            fd: fd(w),
            fs: fs(w),
        },
        0x0F => FloorWS {
            fd: fd(w),
            fs: fs(w),
        },
        0x21 => CvtDS {
            fd: fd(w),
            fs: fs(w),
        },
        0x24 => CvtWS {
            fd: fd(w),
            fs: fs(w),
        },
        0x25 => CvtLS {
            fd: fd(w),
            fs: fs(w),
        },
        0x32 => CEqS {
            fs: fs(w),
            ft: ft(w),
        },
        0x3C => CLtS {
            fs: fs(w),
            ft: ft(w),
        },
        0x3E => CLeS {
            fs: fs(w),
            ft: ft(w),
        },
        0x30..=0x3F => CCondS {
            fs: fs(w),
            ft: ft(w),
            cond: (funct(w) & 0x0F) as u8,
        },
        _ => Unknown { word: w },
    }
}

/// Double-precision (fmt = D = 0x11) `funct` sub-dispatch.
fn decode_cop1_d(w: u32) -> Instruction {
    use Instruction::*;
    match funct(w) {
        0x00 => AddD {
            fd: fd(w),
            fs: fs(w),
            ft: ft(w),
        },
        0x01 => SubD {
            fd: fd(w),
            fs: fs(w),
            ft: ft(w),
        },
        0x02 => MulD {
            fd: fd(w),
            fs: fs(w),
            ft: ft(w),
        },
        0x03 => DivD {
            fd: fd(w),
            fs: fs(w),
            ft: ft(w),
        },
        0x04 => SqrtD {
            fd: fd(w),
            fs: fs(w),
        },
        0x05 => AbsD {
            fd: fd(w),
            fs: fs(w),
        },
        0x06 => MovD {
            fd: fd(w),
            fs: fs(w),
        },
        0x07 => NegD {
            fd: fd(w),
            fs: fs(w),
        },
        0x11 => MovcfD {
            fd: fd(w),
            fs: fs(w),
            tf: (w >> 16) & 1 != 0,
        },
        0x12 => MovzD {
            fd: fd(w),
            fs: fs(w),
            rt: ft(w),
        },
        0x13 => MovnD {
            fd: fd(w),
            fs: fs(w),
            rt: ft(w),
        },
        0x08 => RoundLD {
            fd: fd(w),
            fs: fs(w),
        },
        0x09 => TruncLD {
            fd: fd(w),
            fs: fs(w),
        },
        0x0A => CeilLD {
            fd: fd(w),
            fs: fs(w),
        },
        0x0B => FloorLD {
            fd: fd(w),
            fs: fs(w),
        },
        0x0C => RoundWD {
            fd: fd(w),
            fs: fs(w),
        },
        0x0D => TruncWD {
            fd: fd(w),
            fs: fs(w),
        },
        0x0E => CeilWD {
            fd: fd(w),
            fs: fs(w),
        },
        0x0F => FloorWD {
            fd: fd(w),
            fs: fs(w),
        },
        0x20 => CvtSD {
            fd: fd(w),
            fs: fs(w),
        },
        0x24 => CvtWD {
            fd: fd(w),
            fs: fs(w),
        },
        0x25 => CvtLD {
            fd: fd(w),
            fs: fs(w),
        },
        0x32 => CEqD {
            fs: fs(w),
            ft: ft(w),
        },
        0x3C => CLtD {
            fs: fs(w),
            ft: ft(w),
        },
        0x3E => CLeD {
            fs: fs(w),
            ft: ft(w),
        },
        0x30..=0x3F => CCondD {
            fs: fs(w),
            ft: ft(w),
            cond: (funct(w) & 0x0F) as u8,
        },
        _ => Unknown { word: w },
    }
}
