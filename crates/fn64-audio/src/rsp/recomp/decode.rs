//! Hand-rolled, pure-Rust RSP instruction decoder: a 32-bit instruction word
//! → a typed [`Instr`] enum.
//!
//! ## Provenance (clean-room)
//!
//! Every opcode/funct/field encoding below is byte-cited from the **public**
//! MIPS-I ISA and the community RSP references, cross-checked against the
//! **MIT-licensed** rabbitizer *encoding tables* (the `.inc` data tables in
//! `N64RecompSource/lib/rabbitizer/tables/tables/instr_id/rsp/`, and the
//! bit-field getters in `rabbitizer/include/instructions/RabbitizerInstructionRsp.h`).
//! Those are data/field-layout tables (MIT), not the GPL `librecomp`
//! implementation — no GPL RSP/VU *implementation* header was read.
//!
//! Bit-field layout (all standard MIPS-I plus the RSP CP2 extensions), cited
//! from `RabbitizerInstructionRsp.h`:
//! - `op   = word[31:26]`, `rs = word[25:21]`, `rt = word[20:16]`,
//!   `rd = word[15:11]`, `sa = word[10:6]`, `funct = word[5:0]`,
//!   `imm = word[15:0]`, `target = word[25:0]`.
//! - CP2 vector regs: `vt = word[20:16]`, `vs = word[15:11]`,
//!   `vd = word[10:6]` (GET_vt/GET_vs/GET_vd).
//! - `elementhigh = word[24:21]` (the compute-op `<e>`; GET_elementhigh),
//!   `elementlow  = word[10:7]`  (the load/store element; GET_elementlow),
//!   `de          = word[15:11]` (VMOV/VRCP dest element; GET_de),
//!   `index       = word[10:7]`  (LTV/STV / VRND index; GET_index),
//!   vector `offset = word[6:0]` (signed 7-bit; GET_OFFSET_VECTOR_RAW).
//! - COP2 sub-op (`mfc2/mtc2/cfc2/ctc2`) is in `rs = word[25:21]`
//!   (rsp_cop2.inc: 0x00 mfc2, 0x02 cfc2, 0x04 mtc2, 0x06 ctc2).
//! - Vector load sub-op (LWC2, op=0x32) / store (SWC2, op=0x3A) is in
//!   `rd = word[15:11]` (rsp_normal_lwc2.inc / rsp_normal_swc2.inc).
//! - VU compute funct is `word[5:0]` (rsp_cop2_vu.inc).
//!
//! Anything the audio ucode could contain that is NOT decoded here returns
//! [`Instr::Unknown`], which the emitter turns into a loud compile-time trap
//! — never a silent skip.

use crate::rsp::ops::VuOp;

/// A decoded RSP instruction. Scalar MIPS-subset ops carry their register /
/// immediate fields; CP2 compute ops carry a [`VuOp`] plus the operand
/// registers and element field; vector load/store and CP2 moves carry their
/// own fields. `Unknown` preserves the raw word so a trap can name it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Instr {
    /// `nop` (encoded as `sll r0, r0, 0`; also any all-zero word).
    Nop,

    // --- Scalar ALU, register form (SPECIAL) ---
    /// `rd = rs OP rt` (add/addu/sub/subu/and/or/xor/nor/slt/sltu).
    AluReg {
        op: AluRegOp,
        rd: u8,
        rs: u8,
        rt: u8,
    },
    /// `rd = rt << sa` etc. (sll/srl/sra).
    Shift { op: ShiftOp, rd: u8, rt: u8, sa: u8 },
    /// `rd = rt << (rs & 31)` etc. (sllv/srlv/srav).
    ShiftVar { op: ShiftOp, rd: u8, rt: u8, rs: u8 },
    /// `movz`/`movn`: `rd = rs` conditional on `rt == 0` / `rt != 0`.
    CondMove {
        on_zero: bool,
        rd: u8,
        rs: u8,
        rt: u8,
    },

    // --- Scalar ALU, immediate form ---
    /// `rt = rs OP imm` (addi/addiu/slti/sltiu/andi/ori/xori).
    AluImm {
        op: AluImmOp,
        rt: u8,
        rs: u8,
        imm: u16,
    },
    /// `rt = imm << 16` (lui).
    Lui { rt: u8, imm: u16 },

    // --- Loads / stores (scalar, DMEM) ---
    /// `rt = MEM[rs + off]` (lb/lbu/lh/lhu/lw).
    Load {
        op: LoadOp,
        rt: u8,
        base: u8,
        off: i16,
    },
    /// `MEM[rs + off] = rt` (sb/sh/sw).
    Store {
        op: StoreOp,
        rt: u8,
        base: u8,
        off: i16,
    },

    // --- Branches / jumps ---
    /// `beq`/`bne`: compare rs,rt; branch to `target` (absolute IMEM word addr).
    Branch {
        op: BranchOp,
        rs: u8,
        rt: u8,
        target: u16,
    },
    /// `blez`/`bgtz`/`bltz`/`bgez`: compare rs to 0; branch to `target`.
    BranchZ { op: BranchZOp, rs: u8, target: u16 },
    /// `j target` (absolute jump within IMEM).
    Jump { target: u16 },
    /// `jal target`: link into r31, jump.
    Jal { target: u16, ret: u16 },
    /// `jr rs` (indirect jump; `is_link`=false).
    Jr { rs: u8 },
    /// `jalr rd, rs`: link `ret` into rd, indirect-jump to rs.
    Jalr { rd: u8, rs: u8, ret: u16 },

    /// `break` — the "ucode task finished" terminator.
    Break,

    // --- CP0 (RSP status / DMA / semaphore) ---
    /// `mfc0 rt, cop0d` — read an RSP CP0 register.
    Mfc0 { rt: u8, cop0: u8 },
    /// `mtc0 cop0d, rt` — write an RSP CP0 register (DMA regs, status, …).
    Mtc0 { rt: u8, cop0: u8 },

    // --- CP2 scalar transfers ---
    /// `mfc2 rt, vs[elem]` — read a lane of a vector reg into a scalar reg.
    Mfc2 { rt: u8, vs: u8, elem: u8 },
    /// `mtc2 rt, vs[elem]` — write a scalar reg into a lane of a vector reg.
    Mtc2 { rt: u8, vs: u8, elem: u8 },
    /// `cfc2 rt, cd` — read a VU control reg (VCO/VCC/VCE) into a scalar reg.
    Cfc2 { rt: u8, cd: u8 },
    /// `ctc2 rt, cd` — write a scalar reg into a VU control reg.
    Ctc2 { rt: u8, cd: u8 },

    // --- CP2 vector load / store ---
    /// A vector load (`lqv`/`ldv`/`llv`/`lsv`/`lbv`/`lpv`/`luv`/`lhv`/`lfv`/`lrv`/`ltv`).
    VLoad {
        op: VLoadOp,
        vt: u8,
        elem: u8,
        base: u8,
        off: i16,
    },
    /// A vector store (`sqv`/`sdv`/`slv`/`ssv`/`sbv`/`spv`/`suv`/`shv`/`sfv`/`srv`/`swv`/`stv`).
    VStore {
        op: VStoreOp,
        vt: u8,
        elem: u8,
        base: u8,
        off: i16,
    },

    // --- CP2 compute (the 47 VU ops the framework implements) ---
    /// A CP2 compute op. Carries the [`VuOp`] plus the raw operand fields; the
    /// emitter resolves them via the op's [`crate::rsp::ops::operand_shape`].
    Vu {
        op: VuOp,
        vd: u8,
        vs: u8,
        vt: u8,
        e: u8,
        de: u8,
    },

    /// An instruction word this decoder does not recognize. The raw word and
    /// its IMEM address are preserved so the emitter can emit a loud,
    /// self-identifying trap (never a silent skip).
    Unknown { word: u32 },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AluRegOp {
    Add,
    Addu,
    Sub,
    Subu,
    And,
    Or,
    Xor,
    Nor,
    Slt,
    Sltu,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShiftOp {
    Sll,
    Srl,
    Sra,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AluImmOp {
    Addi,
    Addiu,
    Slti,
    Sltiu,
    Andi,
    Ori,
    Xori,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LoadOp {
    Lb,
    Lbu,
    Lh,
    Lhu,
    Lw,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StoreOp {
    Sb,
    Sh,
    Sw,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BranchOp {
    Beq,
    Bne,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BranchZOp {
    Blez,
    Bgtz,
    Bltz,
    Bgez,
}

/// Vector load sub-ops (LWC2 `rd` field), rsp_normal_lwc2.inc.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VLoadOp {
    Lbv,
    Lsv,
    Llv,
    Ldv,
    Lqv,
    Lrv,
    Lpv,
    Luv,
    Lhv,
    Lfv,
    Ltv,
}

/// Vector store sub-ops (SWC2 `rd` field), rsp_normal_swc2.inc.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VStoreOp {
    Sbv,
    Ssv,
    Slv,
    Sdv,
    Sqv,
    Srv,
    Spv,
    Suv,
    Shv,
    Sfv,
    Swv,
    Stv,
}

// --- Field extractors (byte-cited bit positions, see module doc) ---
#[inline]
fn op(word: u32) -> u32 {
    (word >> 26) & 0x3F
}
#[inline]
fn rs(word: u32) -> u8 {
    ((word >> 21) & 0x1F) as u8
}
#[inline]
fn rt(word: u32) -> u8 {
    ((word >> 16) & 0x1F) as u8
}
#[inline]
fn rd(word: u32) -> u8 {
    ((word >> 11) & 0x1F) as u8
}
#[inline]
fn sa(word: u32) -> u8 {
    ((word >> 6) & 0x1F) as u8
}
#[inline]
fn funct(word: u32) -> u32 {
    word & 0x3F
}
#[inline]
fn imm(word: u32) -> u16 {
    (word & 0xFFFF) as u16
}
#[inline]
fn target26(word: u32) -> u32 {
    word & 0x03FF_FFFF
}

// CP2 register fields (GET_vt/GET_vs/GET_vd).
#[inline]
fn vt(word: u32) -> u8 {
    ((word >> 16) & 0x1F) as u8
}
#[inline]
fn vs(word: u32) -> u8 {
    ((word >> 11) & 0x1F) as u8
}
#[inline]
fn vd(word: u32) -> u8 {
    ((word >> 6) & 0x1F) as u8
}
/// GET_elementhigh: word[24:21], the CP2-compute `<e>` field.
#[inline]
fn element_high(word: u32) -> u8 {
    ((word >> 21) & 0xF) as u8
}
/// GET_elementlow: word[10:7], the load/store element field.
#[inline]
fn element_low(word: u32) -> u8 {
    ((word >> 7) & 0xF) as u8
}
/// GET_de: word[15:11], the VMOV/VRCP destination-element field.
#[inline]
fn de(word: u32) -> u8 {
    ((word >> 11) & 0x1F) as u8
}
/// GET_index: word[10:7], the LTV/STV / VRND index field.
#[inline]
fn index(word: u32) -> u8 {
    ((word >> 7) & 0xF) as u8
}
/// GET_OFFSET_VECTOR_RAW: word[6:0], the signed 7-bit vector offset.
#[inline]
fn vec_offset_raw(word: u32) -> u32 {
    word & 0x7F
}

/// The absolute IMEM word-address a branch/jump targets. RSP IMEM is 0x1000
/// bytes; the recompiler masks branch targets with `rsp_mem_mask = 0x1FFF`
/// (`rsp_recomp.cpp:18`), which we reproduce so an IMEM-relative label is
/// stable. `pc` is the address of the branch instruction itself.
fn branch_target(pc: u32, imm: u16) -> u16 {
    // Sign-extend the 16-bit immediate, shift by 2, add to the delay-slot PC.
    let delta = (imm as i16 as i32) << 2;
    let dest = (pc.wrapping_add(4) as i32).wrapping_add(delta) as u32;
    (dest & 0x1FFF) as u16
}

/// The absolute IMEM word-address a `j`/`jal` targets (26-bit field << 2,
/// masked into RSP mem range).
fn jump_target(word: u32) -> u16 {
    ((target26(word) << 2) & 0x1FFF) as u16
}

/// Decode one 32-bit big-endian RSP instruction word at IMEM address `pc`.
///
/// `pc` is only used to resolve PC-relative branch/jump-and-link targets into
/// absolute IMEM word addresses; the returned [`Instr`] is otherwise
/// position-independent.
pub fn decode(word: u32, pc: u32) -> Instr {
    if word == 0 {
        return Instr::Nop;
    }
    match op(word) {
        0x00 => decode_special(word, pc),
        0x01 => decode_regimm(word, pc),
        0x02 => Instr::Jump {
            target: jump_target(word),
        },
        0x03 => Instr::Jal {
            target: jump_target(word),
            ret: ((pc.wrapping_add(8)) & 0x1FFF) as u16,
        },
        0x04 => Instr::Branch {
            op: BranchOp::Beq,
            rs: rs(word),
            rt: rt(word),
            target: branch_target(pc, imm(word)),
        },
        0x05 => Instr::Branch {
            op: BranchOp::Bne,
            rs: rs(word),
            rt: rt(word),
            target: branch_target(pc, imm(word)),
        },
        0x06 => Instr::BranchZ {
            op: BranchZOp::Blez,
            rs: rs(word),
            target: branch_target(pc, imm(word)),
        },
        0x07 => Instr::BranchZ {
            op: BranchZOp::Bgtz,
            rs: rs(word),
            target: branch_target(pc, imm(word)),
        },
        0x08 => alu_imm(AluImmOp::Addi, word),
        0x09 => alu_imm(AluImmOp::Addiu, word),
        0x0A => alu_imm(AluImmOp::Slti, word),
        0x0B => alu_imm(AluImmOp::Sltiu, word),
        0x0C => alu_imm(AluImmOp::Andi, word),
        0x0D => alu_imm(AluImmOp::Ori, word),
        0x0E => alu_imm(AluImmOp::Xori, word),
        0x0F => Instr::Lui {
            rt: rt(word),
            imm: imm(word),
        },
        0x10 => decode_cop0(word),
        0x12 => decode_cop2(word),
        0x20 => load(LoadOp::Lb, word),
        0x21 => load(LoadOp::Lh, word),
        0x23 => load(LoadOp::Lw, word),
        0x24 => load(LoadOp::Lbu, word),
        0x25 => load(LoadOp::Lhu, word),
        0x28 => store(StoreOp::Sb, word),
        0x29 => store(StoreOp::Sh, word),
        0x2B => store(StoreOp::Sw, word),
        0x32 => decode_lwc2(word),
        0x3A => decode_swc2(word),
        _ => Instr::Unknown { word },
    }
}

fn decode_special(word: u32, pc: u32) -> Instr {
    match funct(word) {
        0x00 => {
            if word == 0 {
                Instr::Nop
            } else {
                Instr::Shift {
                    op: ShiftOp::Sll,
                    rd: rd(word),
                    rt: rt(word),
                    sa: sa(word),
                }
            }
        }
        0x02 => Instr::Shift {
            op: ShiftOp::Srl,
            rd: rd(word),
            rt: rt(word),
            sa: sa(word),
        },
        0x03 => Instr::Shift {
            op: ShiftOp::Sra,
            rd: rd(word),
            rt: rt(word),
            sa: sa(word),
        },
        0x04 => Instr::ShiftVar {
            op: ShiftOp::Sll,
            rd: rd(word),
            rt: rt(word),
            rs: rs(word),
        },
        0x06 => Instr::ShiftVar {
            op: ShiftOp::Srl,
            rd: rd(word),
            rt: rt(word),
            rs: rs(word),
        },
        0x07 => Instr::ShiftVar {
            op: ShiftOp::Sra,
            rd: rd(word),
            rt: rt(word),
            rs: rs(word),
        },
        0x08 => Instr::Jr { rs: rs(word) },
        0x09 => Instr::Jalr {
            rd: rd(word),
            rs: rs(word),
            ret: ((pc.wrapping_add(8)) & 0x1FFF) as u16,
        },
        0x0A => Instr::CondMove {
            on_zero: true,
            rd: rd(word),
            rs: rs(word),
            rt: rt(word),
        },
        0x0B => Instr::CondMove {
            on_zero: false,
            rd: rd(word),
            rs: rs(word),
            rt: rt(word),
        },
        0x0D => Instr::Break,
        0x20 => alu_reg(AluRegOp::Add, word),
        0x21 => alu_reg(AluRegOp::Addu, word),
        0x22 => alu_reg(AluRegOp::Sub, word),
        0x23 => alu_reg(AluRegOp::Subu, word),
        0x24 => alu_reg(AluRegOp::And, word),
        0x25 => alu_reg(AluRegOp::Or, word),
        0x26 => alu_reg(AluRegOp::Xor, word),
        0x27 => alu_reg(AluRegOp::Nor, word),
        0x2A => alu_reg(AluRegOp::Slt, word),
        0x2B => alu_reg(AluRegOp::Sltu, word),
        _ => Instr::Unknown { word },
    }
}

fn decode_regimm(word: u32, pc: u32) -> Instr {
    match rt(word) {
        0x00 => Instr::BranchZ {
            op: BranchZOp::Bltz,
            rs: rs(word),
            target: branch_target(pc, imm(word)),
        },
        0x01 => Instr::BranchZ {
            op: BranchZOp::Bgez,
            rs: rs(word),
            target: branch_target(pc, imm(word)),
        },
        _ => Instr::Unknown { word },
    }
}

fn decode_cop0(word: u32) -> Instr {
    // COP0 sub-op is in the rs field: 0x00 mfc0, 0x04 mtc0 (rsp_cop0.inc).
    match rs(word) {
        0x00 => Instr::Mfc0 {
            rt: rt(word),
            cop0: rd(word),
        },
        0x04 => Instr::Mtc0 {
            rt: rt(word),
            cop0: rd(word),
        },
        _ => Instr::Unknown { word },
    }
}

fn decode_cop2(word: u32) -> Instr {
    // COP2: if bit 25 is set it's a compute op (the "CO" bit); otherwise the
    // rs field selects mfc2/cfc2/mtc2/ctc2 (rsp_cop2.inc: 0/2/4/6).
    if (word >> 25) & 1 == 1 {
        return decode_vu(word);
    }
    match rs(word) {
        0x00 => Instr::Mfc2 {
            rt: rt(word),
            vs: vs(word),
            elem: element_low(word),
        },
        0x02 => Instr::Cfc2 {
            rt: rt(word),
            cd: rd(word),
        },
        0x04 => Instr::Mtc2 {
            rt: rt(word),
            vs: vs(word),
            elem: element_low(word),
        },
        0x06 => Instr::Ctc2 {
            rt: rt(word),
            cd: rd(word),
        },
        _ => Instr::Unknown { word },
    }
}

fn decode_vu(word: u32) -> Instr {
    let f = funct(word);
    let vu = match f {
        0x00 => VuOp::Vmulf,
        0x01 => VuOp::Vmulu,
        0x02 => VuOp::Vrndp,
        0x03 => VuOp::Vmulq,
        0x04 => VuOp::Vmudl,
        0x05 => VuOp::Vmudm,
        0x06 => VuOp::Vmudn,
        0x07 => VuOp::Vmudh,
        0x08 => VuOp::Vmacf,
        0x09 => VuOp::Vmacu,
        0x0A => VuOp::Vrndn,
        0x0B => VuOp::Vmacq,
        0x0C => VuOp::Vmadl,
        0x0D => VuOp::Vmadm,
        0x0E => VuOp::Vmadn,
        0x0F => VuOp::Vmadh,
        0x10 => VuOp::Vadd,
        0x11 => VuOp::Vsub,
        0x13 => VuOp::Vabs,
        0x14 => VuOp::Vaddc,
        0x15 => VuOp::Vsubc,
        0x1D => VuOp::Vsar,
        0x20 => VuOp::Vlt,
        0x21 => VuOp::Veq,
        0x22 => VuOp::Vne,
        0x23 => VuOp::Vge,
        0x24 => VuOp::Vcl,
        0x25 => VuOp::Vch,
        0x26 => VuOp::Vcr,
        0x27 => VuOp::Vmrg,
        0x28 => VuOp::Vand,
        0x29 => VuOp::Vnand,
        0x2A => VuOp::Vor,
        0x2B => VuOp::Vnor,
        0x2C => VuOp::Vxor,
        0x2D => VuOp::Vnxor,
        0x30 => VuOp::Vrcp,
        0x31 => VuOp::Vrcpl,
        0x32 => VuOp::Vrcph,
        0x33 => VuOp::Vmov,
        0x34 => VuOp::Vrsq,
        0x35 => VuOp::Vrsql,
        0x36 => VuOp::Vrsqh,
        0x37 => VuOp::Vnop,
        _ => return Instr::Unknown { word },
    };
    Instr::Vu {
        op: vu,
        vd: vd(word),
        vs: vs(word),
        vt: vt(word),
        e: element_high(word),
        de: de(word) & 7,
    }
}

fn decode_lwc2(word: u32) -> Instr {
    let vop = match rd(word) {
        0x00 => VLoadOp::Lbv,
        0x01 => VLoadOp::Lsv,
        0x02 => VLoadOp::Llv,
        0x03 => VLoadOp::Ldv,
        0x04 => VLoadOp::Lqv,
        0x05 => VLoadOp::Lrv,
        0x06 => VLoadOp::Lpv,
        0x07 => VLoadOp::Luv,
        0x08 => VLoadOp::Lhv,
        0x09 => VLoadOp::Lfv,
        0x0B => VLoadOp::Ltv,
        _ => return Instr::Unknown { word },
    };
    // LTV uses the index field (word[10:7]) instead of an element.
    let elem = if matches!(vop, VLoadOp::Ltv) {
        index(word)
    } else {
        element_low(word)
    };
    Instr::VLoad {
        op: vop,
        vt: vt(word),
        elem,
        base: rs(word),
        off: sign_extend_vec_offset(vec_offset_raw(word)),
    }
}

fn decode_swc2(word: u32) -> Instr {
    let vop = match rd(word) {
        0x00 => VStoreOp::Sbv,
        0x01 => VStoreOp::Ssv,
        0x02 => VStoreOp::Slv,
        0x03 => VStoreOp::Sdv,
        0x04 => VStoreOp::Sqv,
        0x05 => VStoreOp::Srv,
        0x06 => VStoreOp::Spv,
        0x07 => VStoreOp::Suv,
        0x08 => VStoreOp::Shv,
        0x09 => VStoreOp::Sfv,
        0x0A => VStoreOp::Swv,
        0x0B => VStoreOp::Stv,
        _ => return Instr::Unknown { word },
    };
    let elem = if matches!(vop, VStoreOp::Stv) {
        index(word)
    } else {
        element_low(word)
    };
    Instr::VStore {
        op: vop,
        vt: vt(word),
        elem,
        base: rs(word),
        off: sign_extend_vec_offset(vec_offset_raw(word)),
    }
}

/// Sign-extend the 7-bit vector offset. This is the RAW element count, NOT yet
/// scaled by the op's element size — the runtime scales it per op (LQV×16,
/// LDV×8, LLV×4, LSV×2, LBV×1, …) so the scale stays with the semantics.
fn sign_extend_vec_offset(raw: u32) -> i16 {
    // ((int8_t)(raw << 1)) >> 1 sign-extends a 7-bit field (rsp_recomp.cpp's
    // Imm7 handling, line ~333). We keep it as an i16 element count.
    (((raw << 1) as u8 as i8) >> 1) as i16
}

fn alu_reg(op: AluRegOp, word: u32) -> Instr {
    Instr::AluReg {
        op,
        rd: rd(word),
        rs: rs(word),
        rt: rt(word),
    }
}

fn alu_imm(op: AluImmOp, word: u32) -> Instr {
    Instr::AluImm {
        op,
        rt: rt(word),
        rs: rs(word),
        imm: imm(word),
    }
}

fn load(op: LoadOp, word: u32) -> Instr {
    Instr::Load {
        op,
        rt: rt(word),
        base: rs(word),
        off: imm(word) as i16,
    }
}

fn store(op: StoreOp, word: u32) -> Instr {
    Instr::Store {
        op,
        rt: rt(word),
        base: rs(word),
        off: imm(word) as i16,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Known-good words taken from OoT's aspMainText (big-endian), decoded by
    // hand from the byte-cited encodings above.

    #[test]
    fn ori_first_word_of_aspmain() {
        // aspMainText[0] big-endian bytes c0 0f 0a 34 -> word 0xc00f0a34?
        // The incbin is big-endian: bytes [34 0a 0f c0] form 0x340a0fc0.
        // op = 0x340a0fc0 >> 26 = 0x0D (ori); rt = 10, rs = 0, imm = 0x0fc0.
        let w = 0x340a_0fc0;
        assert_eq!(
            decode(w, 0),
            Instr::AluImm {
                op: AluImmOp::Ori,
                rt: 10,
                rs: 0,
                imm: 0x0fc0,
            }
        );
    }

    #[test]
    fn lw_decodes_base_offset() {
        // lw $2, 0x18($10):  op=0x23, rt=2, base=10, off=0x18
        // 100011 01010 00010 0000000000011000
        let w = (0x23 << 26) | (10 << 21) | (2 << 16) | 0x0018;
        assert_eq!(
            decode(w, 0),
            Instr::Load {
                op: LoadOp::Lw,
                rt: 2,
                base: 10,
                off: 0x18
            }
        );
    }

    #[test]
    fn vmudn_e_and_regs() {
        // A CP2 compute op: op=0x12, CO bit (25) set, funct=0x06 (vmudn).
        // e (word[24:21]) = 3, vt=8, vs=9, vd=10.
        let w = (0x12u32 << 26)
            | (1 << 25)
            | (3 << 21) // element_high
            | (8 << 16) // vt
            | (9 << 11) // vs
            | (10 << 6) // vd
            | 0x06; // funct vmudn
                    // `de` (word[15:11]) shares bits with `vs`, so for vs=9 the decoded
                    // de = 9 & 7 = 1. (The `de` field only matters for VMOV/VRCP/VRSQ;
                    // for a Vd,Vs,Vt op like VMUDN the emitter ignores it.)
        assert_eq!(
            decode(w, 0),
            Instr::Vu {
                op: VuOp::Vmudn,
                vd: 10,
                vs: 9,
                vt: 8,
                e: 3,
                de: 9 & 7,
            }
        );
    }

    #[test]
    fn lqv_decodes_element_and_offset() {
        // lqv: op=0x32 (LWC2), rd=0x04 (lqv), vt=4, base=2, elem=0, off raw=1.
        let w = (0x32u32 << 26)
            | (2 << 21) // base
            | (4 << 16) // vt
            | (0x04 << 11) // lwc2 subop lqv
            | (0 << 7) // element
            | 0x01; // offset raw = 1
        assert_eq!(
            decode(w, 0),
            Instr::VLoad {
                op: VLoadOp::Lqv,
                vt: 4,
                elem: 0,
                base: 2,
                off: 1
            }
        );
    }

    #[test]
    fn sqv_store_decodes() {
        // sqv: op=0x3A (SWC2), rd=0x04 (sqv), vt=5, base=3.
        let w = (0x3Au32 << 26) | (3 << 21) | (5 << 16) | (0x04 << 11);
        assert_eq!(
            decode(w, 0),
            Instr::VStore {
                op: VStoreOp::Sqv,
                vt: 5,
                elem: 0,
                base: 3,
                off: 0
            }
        );
    }

    #[test]
    fn mtc0_dma_write_reg() {
        // mtc0: op=0x10, rs=0x04, rt=8, cop0=rd=5.
        let w = (0x10u32 << 26) | (0x04 << 21) | (8 << 16) | (5 << 11);
        assert_eq!(decode(w, 0), Instr::Mtc0 { rt: 8, cop0: 5 });
    }

    #[test]
    fn break_decodes() {
        // op=0x00 special, funct=0x0D.
        let w = 0x0000_000D;
        assert_eq!(decode(w, 0), Instr::Break);
    }

    #[test]
    fn beq_branch_target_pc_relative() {
        // beq $2,$3, +2 words (imm=1): op=0x04, rs=2, rt=3, imm=1.
        // target = (pc+4) + (1<<2) = pc+8. At pc=0x40 -> 0x48.
        let w = (0x04u32 << 26) | (2 << 21) | (3 << 16) | 0x0001;
        assert_eq!(
            decode(w, 0x40),
            Instr::Branch {
                op: BranchOp::Beq,
                rs: 2,
                rt: 3,
                target: 0x48
            }
        );
    }

    #[test]
    fn nop_and_zero_word() {
        assert_eq!(decode(0, 0), Instr::Nop);
    }

    #[test]
    fn unknown_word_is_preserved() {
        // op = 0x3F is not a decoded opcode.
        let w = 0xFC00_0000;
        assert_eq!(decode(w, 0), Instr::Unknown { word: w });
    }
}
