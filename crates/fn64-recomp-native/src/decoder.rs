//! MIPS III (VR4300) instruction decoder: a 32-bit big-endian instruction
//! word -> a typed [`Instruction`] enum.
//!
//! # Clean-room provenance
//!
//! Every encoding below is byte-cited from the **public** MIPS III / VR4300
//! instruction-set reference (the *MIPS IV Instruction Set* manual, Rev 3.2,
//! and the *NEC VR4300 User's Manual*), which document the instruction word
//! layout as public fact. We mirror the *structure* of the MIT N64Recomp
//! decode dispatch (opcode -> SPECIAL/REGIMM/COPz sub-dispatch) but not any
//! of its code; the field extraction here is derived directly from the ISA.
//!
//! ## Instruction word layout (all MIPS instructions are 32 bits, big-endian)
//!
//! ```text
//!  31    26 25   21 20   16 15   11 10    6 5     0
//! +--------+-------+-------+-------+-------+-------+
//! | opcode |  rs   |  rt   |  rd   |  sa   | funct |   R-type (register)
//! +--------+-------+-------+-------+-------+-------+
//! | opcode |  rs   |  rt   |     immediate (16)    |   I-type (immediate)
//! +--------+-------+-----------------------------+
//! | opcode |            target (26)              |   J-type (jump)
//! +--------+-------------------------------------+
//! ```
//!
//! - `opcode` = bits 31..26. When `opcode == 0` the instruction is `SPECIAL`
//!   and the operation is selected by `funct` (bits 5..0). When `opcode == 1`
//!   it is `REGIMM` and the operation is selected by the `rt` field
//!   (bits 20..16). `opcode == 0x10/0x11` are `COP0`/`COP1`.
//! - Registers `rs`/`rt`/`rd` are 5-bit indices (0..31). GPR 0 (`$zero`) is
//!   hardwired to 0.
//! - `sa` (shift amount) = bits 10..6.
//! - `immediate` (I-type) = bits 15..0, sign- or zero-extended per op.
//! - `target` (J-type) = bits 25..0; the branch target is
//!   `(pc_of_delay_slot & 0xF000_0000) | (target << 2)`.

/// A decoded MIPS register index (0..=31). Index 0 is `$zero`.
pub type Reg = u8;

/// The decoded operation plus its typed operands. One variant per ISA op we
/// cover. Unrecognized words decode to [`Instruction::Unknown`] carrying the
/// raw bits (loud failure, never a silent nop).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Instruction {
    /// `sll $zero,$zero,0` (all-zero word) — the canonical MIPS no-op.
    Nop,

    // --- Loads (I-type: base=rs, dest/src=rt, signed 16-bit offset) ---
    /// Load byte (sign-extended). `LB rt, off(base)`.
    Lb { rt: Reg, base: Reg, off: i16 },
    /// Load byte unsigned. `LBU rt, off(base)`.
    Lbu { rt: Reg, base: Reg, off: i16 },
    /// Load halfword (sign-extended). `LH rt, off(base)`.
    Lh { rt: Reg, base: Reg, off: i16 },
    /// Load halfword unsigned. `LHU rt, off(base)`.
    Lhu { rt: Reg, base: Reg, off: i16 },
    /// Load word (sign-extended into the 64-bit GPR). `LW rt, off(base)`.
    Lw { rt: Reg, base: Reg, off: i16 },
    /// Load word left (unaligned). `LWL rt, off(base)`.
    Lwl { rt: Reg, base: Reg, off: i16 },
    /// Load word right (unaligned). `LWR rt, off(base)`.
    Lwr { rt: Reg, base: Reg, off: i16 },

    // --- Stores ---
    /// Store byte. `SB rt, off(base)`.
    Sb { rt: Reg, base: Reg, off: i16 },
    /// Store halfword. `SH rt, off(base)`.
    Sh { rt: Reg, base: Reg, off: i16 },
    /// Store word. `SW rt, off(base)`.
    Sw { rt: Reg, base: Reg, off: i16 },
    /// Store word left. `SWL rt, off(base)`.
    Swl { rt: Reg, base: Reg, off: i16 },
    /// Store word right. `SWR rt, off(base)`.
    Swr { rt: Reg, base: Reg, off: i16 },

    // --- 64-bit doubleword loads/stores (MIPS III) ---
    //
    // Encodings byte-verified against `mips-linux-gnu-as -mips64 -mabi=64`
    // (see the decoder tests): LD=opcode 0x37, SD=0x3F, LDL=0x1A, LDR=0x1B,
    // SDL=0x2C, SDR=0x2D, LLD=0x34, SCD=0x3C.
    /// Load doubleword. `LD rt, off(base)`.
    Ld { rt: Reg, base: Reg, off: i16 },
    /// Store doubleword. `SD rt, off(base)`.
    Sd { rt: Reg, base: Reg, off: i16 },
    /// Load doubleword left (unaligned). `LDL rt, off(base)`.
    Ldl { rt: Reg, base: Reg, off: i16 },
    /// Load doubleword right (unaligned). `LDR rt, off(base)`.
    Ldr { rt: Reg, base: Reg, off: i16 },
    /// Store doubleword left. `SDL rt, off(base)`.
    Sdl { rt: Reg, base: Reg, off: i16 },
    /// Store doubleword right. `SDR rt, off(base)`.
    Sdr { rt: Reg, base: Reg, off: i16 },
    /// Load-linked doubleword. `LLD rt, off(base)`. On the single-threaded
    /// recompilation model (no other processor to invalidate the link) this is
    /// a plain doubleword load; see the emitter note.
    Lld { rt: Reg, base: Reg, off: i16 },
    /// Store-conditional doubleword. `SCD rt, off(base)`. On the
    /// single-threaded recompilation model the conditional store always
    /// succeeds, so it stores the doubleword and sets rt to 1; see the emitter
    /// note.
    Scd { rt: Reg, base: Reg, off: i16 },

    // --- 64-bit doubleword ALU immediate (I-type) ---
    /// Doubleword add immediate (trap on overflow; treated as DADDIU per the
    /// recomp custom of ignoring integer-overflow traps). `DADDI rt, rs, imm`.
    Daddi { rt: Reg, rs: Reg, imm: i16 },
    /// Doubleword add immediate unsigned (no trap). `DADDIU rt, rs, imm`.
    Daddiu { rt: Reg, rs: Reg, imm: i16 },

    // --- 64-bit doubleword ALU register (R-type, SPECIAL) ---
    /// Doubleword add (trap on overflow; treated as DADDU). `DADD rd, rs, rt`.
    Dadd { rd: Reg, rs: Reg, rt: Reg },
    /// Doubleword add unsigned. `DADDU rd, rs, rt`.
    Daddu { rd: Reg, rs: Reg, rt: Reg },
    /// Doubleword subtract (trap on overflow; treated as DSUBU). `DSUB rd, rs, rt`.
    Dsub { rd: Reg, rs: Reg, rt: Reg },
    /// Doubleword subtract unsigned. `DSUBU rd, rs, rt`.
    Dsubu { rd: Reg, rs: Reg, rt: Reg },

    // --- 64-bit doubleword shifts (R-type, SPECIAL) ---
    /// Doubleword shift left logical by `sa` (0..31). `DSLL rd, rt, sa`.
    Dsll { rd: Reg, rt: Reg, sa: u8 },
    /// Doubleword shift right logical by `sa`. `DSRL rd, rt, sa`.
    Dsrl { rd: Reg, rt: Reg, sa: u8 },
    /// Doubleword shift right arithmetic by `sa`. `DSRA rd, rt, sa`.
    Dsra { rd: Reg, rt: Reg, sa: u8 },
    /// Doubleword shift left logical by `sa + 32` (32..63). `DSLL32 rd, rt, sa`.
    Dsll32 { rd: Reg, rt: Reg, sa: u8 },
    /// Doubleword shift right logical by `sa + 32`. `DSRL32 rd, rt, sa`.
    Dsrl32 { rd: Reg, rt: Reg, sa: u8 },
    /// Doubleword shift right arithmetic by `sa + 32`. `DSRA32 rd, rt, sa`.
    Dsra32 { rd: Reg, rt: Reg, sa: u8 },
    /// Doubleword shift left logical variable (by `rs & 63`). `DSLLV rd, rt, rs`.
    Dsllv { rd: Reg, rt: Reg, rs: Reg },
    /// Doubleword shift right logical variable (by `rs & 63`). `DSRLV rd, rt, rs`.
    Dsrlv { rd: Reg, rt: Reg, rs: Reg },
    /// Doubleword shift right arithmetic variable (by `rs & 63`). `DSRAV rd, rt, rs`.
    Dsrav { rd: Reg, rt: Reg, rs: Reg },

    // --- 64-bit doubleword mult/div (R-type, SPECIAL; write HI/LO) ---
    /// Doubleword multiply signed (128-bit product into HI:LO). `DMULT rs, rt`.
    Dmult { rs: Reg, rt: Reg },
    /// Doubleword multiply unsigned. `DMULTU rs, rt`.
    Dmultu { rs: Reg, rt: Reg },
    /// Doubleword divide signed (LO=quotient, HI=remainder). `DDIV rs, rt`.
    Ddiv { rs: Reg, rt: Reg },
    /// Doubleword divide unsigned. `DDIVU rs, rt`.
    Ddivu { rs: Reg, rt: Reg },

    // --- ALU immediate (I-type) ---
    /// Add immediate (trap on overflow; we treat as ADDIU per recomp custom).
    Addi { rt: Reg, rs: Reg, imm: i16 },
    /// Add immediate unsigned (no trap). `ADDIU rt, rs, imm`.
    Addiu { rt: Reg, rs: Reg, imm: i16 },
    /// Set-on-less-than immediate (signed). `SLTI rt, rs, imm`.
    Slti { rt: Reg, rs: Reg, imm: i16 },
    /// Set-on-less-than immediate unsigned. `SLTIU rt, rs, imm`.
    Sltiu { rt: Reg, rs: Reg, imm: i16 },
    /// And immediate (zero-extended). `ANDI rt, rs, imm`.
    Andi { rt: Reg, rs: Reg, imm: u16 },
    /// Or immediate (zero-extended). `ORI rt, rs, imm`.
    Ori { rt: Reg, rs: Reg, imm: u16 },
    /// Xor immediate (zero-extended). `XORI rt, rs, imm`.
    Xori { rt: Reg, rs: Reg, imm: u16 },
    /// Load upper immediate. `LUI rt, imm` (imm << 16, sign-extended).
    Lui { rt: Reg, imm: u16 },

    // --- ALU register (R-type, SPECIAL) ---
    /// Add (trap on overflow; treated as ADD). `ADD rd, rs, rt`.
    Add { rd: Reg, rs: Reg, rt: Reg },
    /// Add unsigned. `ADDU rd, rs, rt`.
    Addu { rd: Reg, rs: Reg, rt: Reg },
    /// Subtract. `SUB rd, rs, rt`.
    Sub { rd: Reg, rs: Reg, rt: Reg },
    /// Subtract unsigned. `SUBU rd, rs, rt`.
    Subu { rd: Reg, rs: Reg, rt: Reg },
    /// Bitwise and. `AND rd, rs, rt`.
    And { rd: Reg, rs: Reg, rt: Reg },
    /// Bitwise or. `OR rd, rs, rt`.
    Or { rd: Reg, rs: Reg, rt: Reg },
    /// Bitwise xor. `XOR rd, rs, rt`.
    Xor { rd: Reg, rs: Reg, rt: Reg },
    /// Bitwise nor. `NOR rd, rs, rt`.
    Nor { rd: Reg, rs: Reg, rt: Reg },
    /// Set-on-less-than (signed). `SLT rd, rs, rt`.
    Slt { rd: Reg, rs: Reg, rt: Reg },
    /// Set-on-less-than unsigned. `SLTU rd, rs, rt`.
    Sltu { rd: Reg, rs: Reg, rt: Reg },

    // --- Shifts (R-type, SPECIAL) ---
    /// Shift left logical (by immediate sa). `SLL rd, rt, sa`.
    Sll { rd: Reg, rt: Reg, sa: u8 },
    /// Shift right logical (by immediate sa). `SRL rd, rt, sa`.
    Srl { rd: Reg, rt: Reg, sa: u8 },
    /// Shift right arithmetic (by immediate sa). `SRA rd, rt, sa`.
    Sra { rd: Reg, rt: Reg, sa: u8 },
    /// Shift left logical variable (by rs). `SLLV rd, rt, rs`.
    Sllv { rd: Reg, rt: Reg, rs: Reg },
    /// Shift right logical variable. `SRLV rd, rt, rs`.
    Srlv { rd: Reg, rt: Reg, rs: Reg },
    /// Shift right arithmetic variable. `SRAV rd, rt, rs`.
    Srav { rd: Reg, rt: Reg, rs: Reg },

    // --- Mult/Div (R-type, SPECIAL; write HI/LO) ---
    /// Multiply signed. `MULT rs, rt`.
    Mult { rs: Reg, rt: Reg },
    /// Multiply unsigned. `MULTU rs, rt`.
    Multu { rs: Reg, rt: Reg },
    /// Divide signed. `DIV rs, rt`.
    Div { rs: Reg, rt: Reg },
    /// Divide unsigned. `DIVU rs, rt`.
    Divu { rs: Reg, rt: Reg },
    /// Move from HI. `MFHI rd`.
    Mfhi { rd: Reg },
    /// Move from LO. `MFLO rd`.
    Mflo { rd: Reg },
    /// Move to HI. `MTHI rs`.
    Mthi { rs: Reg },
    /// Move to LO. `MTLO rs`.
    Mtlo { rs: Reg },

    // --- Branches (I-type; branch-relative 16-bit offset in words) ---
    /// Branch if equal. `BEQ rs, rt, off`.
    Beq { rs: Reg, rt: Reg, off: i16 },
    /// Branch if not equal. `BNE rs, rt, off`.
    Bne { rs: Reg, rt: Reg, off: i16 },
    /// Branch if <= 0. `BLEZ rs, off`.
    Blez { rs: Reg, off: i16 },
    /// Branch if > 0. `BGTZ rs, off`.
    Bgtz { rs: Reg, off: i16 },
    /// Branch if < 0 (REGIMM). `BLTZ rs, off`.
    Bltz { rs: Reg, off: i16 },
    /// Branch if >= 0 (REGIMM). `BGEZ rs, off`.
    Bgez { rs: Reg, off: i16 },
    /// Branch-and-link if < 0 (REGIMM). `BLTZAL rs, off`.
    Bltzal { rs: Reg, off: i16 },
    /// Branch-and-link if >= 0 (REGIMM). `BGEZAL rs, off`.
    Bgezal { rs: Reg, off: i16 },

    // --- Branch-likely variants (nullify delay slot when NOT taken) ---
    /// Branch-likely equal. `BEQL rs, rt, off`.
    Beql { rs: Reg, rt: Reg, off: i16 },
    /// Branch-likely not equal. `BNEL rs, rt, off`.
    Bnel { rs: Reg, rt: Reg, off: i16 },
    /// Branch-likely <= 0. `BLEZL rs, off`.
    Blezl { rs: Reg, off: i16 },
    /// Branch-likely > 0. `BGTZL rs, off`.
    Bgtzl { rs: Reg, off: i16 },
    /// Branch-likely < 0 (REGIMM). `BLTZL rs, off`.
    Bltzl { rs: Reg, off: i16 },
    /// Branch-likely >= 0 (REGIMM). `BGEZL rs, off`.
    Bgezl { rs: Reg, off: i16 },

    // --- Jumps ---
    /// Jump (absolute, 26-bit target). `J target`.
    J { target: u32 },
    /// Jump-and-link (absolute). `JAL target`.
    Jal { target: u32 },
    /// Jump register. `JR rs`.
    Jr { rs: Reg },
    /// Jump-and-link register. `JALR rd, rs` (rd defaults to $ra).
    Jalr { rd: Reg, rs: Reg },

    /// A word we do not (yet) decode. Carries the raw bits so the emitter can
    /// fail loudly instead of silently emitting a nop.
    Unknown { word: u32 },
}

// --- Field extraction (public ISA bit positions) ---

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
            0x00 => Sll { rd: rd(w), rt: rt(w), sa: sa(w) },
            0x02 => Srl { rd: rd(w), rt: rt(w), sa: sa(w) },
            0x03 => Sra { rd: rd(w), rt: rt(w), sa: sa(w) },
            0x04 => Sllv { rd: rd(w), rt: rt(w), rs: rs(w) },
            0x06 => Srlv { rd: rd(w), rt: rt(w), rs: rs(w) },
            0x07 => Srav { rd: rd(w), rt: rt(w), rs: rs(w) },
            // Doubleword variable shifts.
            0x14 => Dsllv { rd: rd(w), rt: rt(w), rs: rs(w) },
            0x16 => Dsrlv { rd: rd(w), rt: rt(w), rs: rs(w) },
            0x17 => Dsrav { rd: rd(w), rt: rt(w), rs: rs(w) },
            // Jumps.
            0x08 => Jr { rs: rs(w) },
            0x09 => Jalr { rd: rd(w), rs: rs(w) },
            // HI/LO moves.
            0x10 => Mfhi { rd: rd(w) },
            0x11 => Mthi { rs: rs(w) },
            0x12 => Mflo { rd: rd(w) },
            0x13 => Mtlo { rs: rs(w) },
            // Mult/Div.
            0x18 => Mult { rs: rs(w), rt: rt(w) },
            0x19 => Multu { rs: rs(w), rt: rt(w) },
            0x1A => Div { rs: rs(w), rt: rt(w) },
            0x1B => Divu { rs: rs(w), rt: rt(w) },
            // Doubleword mult/div.
            0x1C => Dmult { rs: rs(w), rt: rt(w) },
            0x1D => Dmultu { rs: rs(w), rt: rt(w) },
            0x1E => Ddiv { rs: rs(w), rt: rt(w) },
            0x1F => Ddivu { rs: rs(w), rt: rt(w) },
            // ALU register.
            0x20 => Add { rd: rd(w), rs: rs(w), rt: rt(w) },
            0x21 => Addu { rd: rd(w), rs: rs(w), rt: rt(w) },
            0x22 => Sub { rd: rd(w), rs: rs(w), rt: rt(w) },
            0x23 => Subu { rd: rd(w), rs: rs(w), rt: rt(w) },
            0x24 => And { rd: rd(w), rs: rs(w), rt: rt(w) },
            0x25 => Or { rd: rd(w), rs: rs(w), rt: rt(w) },
            0x26 => Xor { rd: rd(w), rs: rs(w), rt: rt(w) },
            0x27 => Nor { rd: rd(w), rs: rs(w), rt: rt(w) },
            0x2A => Slt { rd: rd(w), rs: rs(w), rt: rt(w) },
            0x2B => Sltu { rd: rd(w), rs: rs(w), rt: rt(w) },
            // Doubleword ALU register.
            0x2C => Dadd { rd: rd(w), rs: rs(w), rt: rt(w) },
            0x2D => Daddu { rd: rd(w), rs: rs(w), rt: rt(w) },
            0x2E => Dsub { rd: rd(w), rs: rs(w), rt: rt(w) },
            0x2F => Dsubu { rd: rd(w), rs: rs(w), rt: rt(w) },
            // Doubleword immediate shifts. DSLL/DSRL/DSRA use sa (0..31);
            // the *32 forms add 32 to the shift count (32..63).
            0x38 => Dsll { rd: rd(w), rt: rt(w), sa: sa(w) },
            0x3A => Dsrl { rd: rd(w), rt: rt(w), sa: sa(w) },
            0x3B => Dsra { rd: rd(w), rt: rt(w), sa: sa(w) },
            0x3C => Dsll32 { rd: rd(w), rt: rt(w), sa: sa(w) },
            0x3E => Dsrl32 { rd: rd(w), rt: rt(w), sa: sa(w) },
            0x3F => Dsra32 { rd: rd(w), rt: rt(w), sa: sa(w) },
            _ => Unknown { word: w },
        },
        // REGIMM: dispatch on the rt field (bits 20..16).
        0x01 => match rt(w) {
            0x00 => Bltz { rs: rs(w), off: imm_s(w) },
            0x01 => Bgez { rs: rs(w), off: imm_s(w) },
            0x02 => Bltzl { rs: rs(w), off: imm_s(w) },
            0x03 => Bgezl { rs: rs(w), off: imm_s(w) },
            0x10 => Bltzal { rs: rs(w), off: imm_s(w) },
            0x11 => Bgezal { rs: rs(w), off: imm_s(w) },
            _ => Unknown { word: w },
        },
        // J-type.
        0x02 => J { target: target26(w) },
        0x03 => Jal { target: target26(w) },
        // Branches (I-type).
        0x04 => Beq { rs: rs(w), rt: rt(w), off: imm_s(w) },
        0x05 => Bne { rs: rs(w), rt: rt(w), off: imm_s(w) },
        0x06 => Blez { rs: rs(w), off: imm_s(w) },
        0x07 => Bgtz { rs: rs(w), off: imm_s(w) },
        // ALU immediate.
        0x08 => Addi { rt: rt(w), rs: rs(w), imm: imm_s(w) },
        0x09 => Addiu { rt: rt(w), rs: rs(w), imm: imm_s(w) },
        0x0A => Slti { rt: rt(w), rs: rs(w), imm: imm_s(w) },
        0x0B => Sltiu { rt: rt(w), rs: rs(w), imm: imm_s(w) },
        0x0C => Andi { rt: rt(w), rs: rs(w), imm: imm_u(w) },
        0x0D => Ori { rt: rt(w), rs: rs(w), imm: imm_u(w) },
        0x0E => Xori { rt: rt(w), rs: rs(w), imm: imm_u(w) },
        0x0F => Lui { rt: rt(w), imm: imm_u(w) },
        // Doubleword ALU immediate.
        0x18 => Daddi { rt: rt(w), rs: rs(w), imm: imm_s(w) },
        0x19 => Daddiu { rt: rt(w), rs: rs(w), imm: imm_s(w) },
        // Doubleword unaligned loads.
        0x1A => Ldl { rt: rt(w), base: rs(w), off: imm_s(w) },
        0x1B => Ldr { rt: rt(w), base: rs(w), off: imm_s(w) },
        // Branch-likely.
        0x14 => Beql { rs: rs(w), rt: rt(w), off: imm_s(w) },
        0x15 => Bnel { rs: rs(w), rt: rt(w), off: imm_s(w) },
        0x16 => Blezl { rs: rs(w), off: imm_s(w) },
        0x17 => Bgtzl { rs: rs(w), off: imm_s(w) },
        // Loads.
        0x20 => Lb { rt: rt(w), base: rs(w), off: imm_s(w) },
        0x21 => Lh { rt: rt(w), base: rs(w), off: imm_s(w) },
        0x22 => Lwl { rt: rt(w), base: rs(w), off: imm_s(w) },
        0x23 => Lw { rt: rt(w), base: rs(w), off: imm_s(w) },
        0x24 => Lbu { rt: rt(w), base: rs(w), off: imm_s(w) },
        0x25 => Lhu { rt: rt(w), base: rs(w), off: imm_s(w) },
        0x26 => Lwr { rt: rt(w), base: rs(w), off: imm_s(w) },
        // Stores.
        0x28 => Sb { rt: rt(w), base: rs(w), off: imm_s(w) },
        0x29 => Sh { rt: rt(w), base: rs(w), off: imm_s(w) },
        0x2A => Swl { rt: rt(w), base: rs(w), off: imm_s(w) },
        0x2B => Sw { rt: rt(w), base: rs(w), off: imm_s(w) },
        0x2E => Swr { rt: rt(w), base: rs(w), off: imm_s(w) },
        // Doubleword unaligned stores.
        0x2C => Sdl { rt: rt(w), base: rs(w), off: imm_s(w) },
        0x2D => Sdr { rt: rt(w), base: rs(w), off: imm_s(w) },
        // Load-linked / store-conditional doubleword.
        0x34 => Lld { rt: rt(w), base: rs(w), off: imm_s(w) },
        0x3C => Scd { rt: rt(w), base: rs(w), off: imm_s(w) },
        // Aligned doubleword load/store.
        0x37 => Ld { rt: rt(w), base: rs(w), off: imm_s(w) },
        0x3F => Sd { rt: rt(w), base: rs(w), off: imm_s(w) },
        _ => Unknown { word: w },
    }
}

impl Instruction {
    /// Whether this instruction is a control-transfer whose *following*
    /// instruction is in its delay slot (all branches and jumps on MIPS).
    pub fn has_delay_slot(&self) -> bool {
        use Instruction::*;
        matches!(
            self,
            Beq { .. } | Bne { .. } | Blez { .. } | Bgtz { .. } | Bltz { .. } | Bgez { .. }
                | Bltzal { .. } | Bgezal { .. } | Beql { .. } | Bnel { .. } | Blezl { .. }
                | Bgtzl { .. } | Bltzl { .. } | Bgezl { .. } | J { .. } | Jal { .. }
                | Jr { .. } | Jalr { .. }
        )
    }

    /// Whether this is a *branch-likely* op (nullifies its delay slot when the
    /// branch is NOT taken). Needed because the emitter places the delay-slot
    /// instruction inside the taken-branch block for these.
    pub fn is_branch_likely(&self) -> bool {
        use Instruction::*;
        matches!(
            self,
            Beql { .. } | Bnel { .. } | Blezl { .. } | Bgtzl { .. } | Bltzl { .. } | Bgezl { .. }
        )
    }
}
