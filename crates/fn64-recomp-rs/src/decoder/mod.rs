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
/// recognize. Unrecognized/reserved words decode to [`Instruction::Unknown`] carrying the
/// raw bits (loud failure, never a silent nop).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Instruction {
    /// `sll $zero,$zero,0` (all-zero word) — the canonical MIPS no-op.
    Nop,

    // --- Loads (I-type: base=rs, dest/src=rt, signed 16-bit offset) ---
    /// Load byte (sign-extended). `LB rt, off(base)`.
    Lb {
        rt: Reg,
        base: Reg,
        off: i16,
    },
    /// Load byte unsigned. `LBU rt, off(base)`.
    Lbu {
        rt: Reg,
        base: Reg,
        off: i16,
    },
    /// Load halfword (sign-extended). `LH rt, off(base)`.
    Lh {
        rt: Reg,
        base: Reg,
        off: i16,
    },
    /// Load halfword unsigned. `LHU rt, off(base)`.
    Lhu {
        rt: Reg,
        base: Reg,
        off: i16,
    },
    /// Load word (sign-extended into the 64-bit GPR). `LW rt, off(base)`.
    Lw {
        rt: Reg,
        base: Reg,
        off: i16,
    },
    /// Load word unsigned (zero-extended into the 64-bit GPR). `LWU rt, off(base)`.
    Lwu {
        rt: Reg,
        base: Reg,
        off: i16,
    },
    /// Load word left (unaligned). `LWL rt, off(base)`.
    Lwl {
        rt: Reg,
        base: Reg,
        off: i16,
    },
    /// Load word right (unaligned). `LWR rt, off(base)`.
    Lwr {
        rt: Reg,
        base: Reg,
        off: i16,
    },

    // --- Stores ---
    /// Store byte. `SB rt, off(base)`.
    Sb {
        rt: Reg,
        base: Reg,
        off: i16,
    },
    /// Store halfword. `SH rt, off(base)`.
    Sh {
        rt: Reg,
        base: Reg,
        off: i16,
    },
    /// Store word. `SW rt, off(base)`.
    Sw {
        rt: Reg,
        base: Reg,
        off: i16,
    },
    /// Store word left. `SWL rt, off(base)`.
    Swl {
        rt: Reg,
        base: Reg,
        off: i16,
    },
    /// Store word right. `SWR rt, off(base)`.
    Swr {
        rt: Reg,
        base: Reg,
        off: i16,
    },

    // --- 64-bit doubleword loads/stores (MIPS III) ---
    //
    // Encodings byte-verified against `mips-linux-gnu-as -mips64 -mabi=64`
    // (see the decoder tests): LD=opcode 0x37, SD=0x3F, LDL=0x1A, LDR=0x1B,
    // SDL=0x2C, SDR=0x2D, LLD=0x34, SCD=0x3C.
    /// Load doubleword. `LD rt, off(base)`.
    Ld {
        rt: Reg,
        base: Reg,
        off: i16,
    },
    /// Store doubleword. `SD rt, off(base)`.
    Sd {
        rt: Reg,
        base: Reg,
        off: i16,
    },
    /// Load doubleword left (unaligned). `LDL rt, off(base)`.
    Ldl {
        rt: Reg,
        base: Reg,
        off: i16,
    },
    /// Load doubleword right (unaligned). `LDR rt, off(base)`.
    Ldr {
        rt: Reg,
        base: Reg,
        off: i16,
    },
    /// Store doubleword left. `SDL rt, off(base)`.
    Sdl {
        rt: Reg,
        base: Reg,
        off: i16,
    },
    /// Store doubleword right. `SDR rt, off(base)`.
    Sdr {
        rt: Reg,
        base: Reg,
        off: i16,
    },
    /// Load-linked doubleword. `LLD rt, off(base)`.
    Lld {
        rt: Reg,
        base: Reg,
        off: i16,
    },
    /// Store-conditional doubleword. `SCD rt, off(base)`.
    Scd {
        rt: Reg,
        base: Reg,
        off: i16,
    },
    /// Load-linked word. `LL rt, off(base)`.
    Ll {
        rt: Reg,
        base: Reg,
        off: i16,
    },
    /// Store-conditional word. `SC rt, off(base)`.
    Sc {
        rt: Reg,
        base: Reg,
        off: i16,
    },

    // --- 64-bit doubleword ALU immediate (I-type) ---
    /// Doubleword add immediate (trap on overflow). `DADDI rt, rs, imm`.
    Daddi {
        rt: Reg,
        rs: Reg,
        imm: i16,
    },
    /// Doubleword add immediate unsigned (no trap). `DADDIU rt, rs, imm`.
    Daddiu {
        rt: Reg,
        rs: Reg,
        imm: i16,
    },

    // --- 64-bit doubleword ALU register (R-type, SPECIAL) ---
    /// Doubleword add (trap on overflow). `DADD rd, rs, rt`.
    Dadd {
        rd: Reg,
        rs: Reg,
        rt: Reg,
    },
    /// Doubleword add unsigned. `DADDU rd, rs, rt`.
    Daddu {
        rd: Reg,
        rs: Reg,
        rt: Reg,
    },
    /// Doubleword subtract (trap on overflow). `DSUB rd, rs, rt`.
    Dsub {
        rd: Reg,
        rs: Reg,
        rt: Reg,
    },
    /// Doubleword subtract unsigned. `DSUBU rd, rs, rt`.
    Dsubu {
        rd: Reg,
        rs: Reg,
        rt: Reg,
    },

    // --- 64-bit doubleword shifts (R-type, SPECIAL) ---
    /// Doubleword shift left logical by `sa` (0..31). `DSLL rd, rt, sa`.
    Dsll {
        rd: Reg,
        rt: Reg,
        sa: u8,
    },
    /// Doubleword shift right logical by `sa`. `DSRL rd, rt, sa`.
    Dsrl {
        rd: Reg,
        rt: Reg,
        sa: u8,
    },
    /// Doubleword shift right arithmetic by `sa`. `DSRA rd, rt, sa`.
    Dsra {
        rd: Reg,
        rt: Reg,
        sa: u8,
    },
    /// Doubleword shift left logical by `sa + 32` (32..63). `DSLL32 rd, rt, sa`.
    Dsll32 {
        rd: Reg,
        rt: Reg,
        sa: u8,
    },
    /// Doubleword shift right logical by `sa + 32`. `DSRL32 rd, rt, sa`.
    Dsrl32 {
        rd: Reg,
        rt: Reg,
        sa: u8,
    },
    /// Doubleword shift right arithmetic by `sa + 32`. `DSRA32 rd, rt, sa`.
    Dsra32 {
        rd: Reg,
        rt: Reg,
        sa: u8,
    },
    /// Doubleword shift left logical variable (by `rs & 63`). `DSLLV rd, rt, rs`.
    Dsllv {
        rd: Reg,
        rt: Reg,
        rs: Reg,
    },
    /// Doubleword shift right logical variable (by `rs & 63`). `DSRLV rd, rt, rs`.
    Dsrlv {
        rd: Reg,
        rt: Reg,
        rs: Reg,
    },
    /// Doubleword shift right arithmetic variable (by `rs & 63`). `DSRAV rd, rt, rs`.
    Dsrav {
        rd: Reg,
        rt: Reg,
        rs: Reg,
    },

    // --- 64-bit doubleword mult/div (R-type, SPECIAL; write HI/LO) ---
    /// Doubleword multiply signed (128-bit product into HI:LO). `DMULT rs, rt`.
    Dmult {
        rs: Reg,
        rt: Reg,
    },
    /// Doubleword multiply unsigned. `DMULTU rs, rt`.
    Dmultu {
        rs: Reg,
        rt: Reg,
    },
    /// Doubleword divide signed (LO=quotient, HI=remainder). `DDIV rs, rt`.
    Ddiv {
        rs: Reg,
        rt: Reg,
    },
    /// Doubleword divide unsigned. `DDIVU rs, rt`.
    Ddivu {
        rs: Reg,
        rt: Reg,
    },

    // --- ALU immediate (I-type) ---
    /// Add immediate (trap on overflow).
    Addi {
        rt: Reg,
        rs: Reg,
        imm: i16,
    },
    /// Add immediate unsigned (no trap). `ADDIU rt, rs, imm`.
    Addiu {
        rt: Reg,
        rs: Reg,
        imm: i16,
    },
    /// Set-on-less-than immediate (signed). `SLTI rt, rs, imm`.
    Slti {
        rt: Reg,
        rs: Reg,
        imm: i16,
    },
    /// Set-on-less-than immediate unsigned. `SLTIU rt, rs, imm`.
    Sltiu {
        rt: Reg,
        rs: Reg,
        imm: i16,
    },
    /// And immediate (zero-extended). `ANDI rt, rs, imm`.
    Andi {
        rt: Reg,
        rs: Reg,
        imm: u16,
    },
    /// Or immediate (zero-extended). `ORI rt, rs, imm`.
    Ori {
        rt: Reg,
        rs: Reg,
        imm: u16,
    },
    /// Xor immediate (zero-extended). `XORI rt, rs, imm`.
    Xori {
        rt: Reg,
        rs: Reg,
        imm: u16,
    },
    /// Load upper immediate. `LUI rt, imm` (imm << 16, sign-extended).
    Lui {
        rt: Reg,
        imm: u16,
    },

    // --- ALU register (R-type, SPECIAL) ---
    /// Add (trap on overflow). `ADD rd, rs, rt`.
    Add {
        rd: Reg,
        rs: Reg,
        rt: Reg,
    },
    /// Add unsigned. `ADDU rd, rs, rt`.
    Addu {
        rd: Reg,
        rs: Reg,
        rt: Reg,
    },
    /// Subtract. `SUB rd, rs, rt`.
    Sub {
        rd: Reg,
        rs: Reg,
        rt: Reg,
    },
    /// Subtract unsigned. `SUBU rd, rs, rt`.
    Subu {
        rd: Reg,
        rs: Reg,
        rt: Reg,
    },
    /// Bitwise and. `AND rd, rs, rt`.
    And {
        rd: Reg,
        rs: Reg,
        rt: Reg,
    },
    /// Bitwise or. `OR rd, rs, rt`.
    Or {
        rd: Reg,
        rs: Reg,
        rt: Reg,
    },
    /// Bitwise xor. `XOR rd, rs, rt`.
    Xor {
        rd: Reg,
        rs: Reg,
        rt: Reg,
    },
    /// Bitwise nor. `NOR rd, rs, rt`.
    Nor {
        rd: Reg,
        rs: Reg,
        rt: Reg,
    },
    /// Set-on-less-than (signed). `SLT rd, rs, rt`.
    Slt {
        rd: Reg,
        rs: Reg,
        rt: Reg,
    },
    /// Set-on-less-than unsigned. `SLTU rd, rs, rt`.
    Sltu {
        rd: Reg,
        rs: Reg,
        rt: Reg,
    },

    // --- Shifts (R-type, SPECIAL) ---
    /// Shift left logical (by immediate sa). `SLL rd, rt, sa`.
    Sll {
        rd: Reg,
        rt: Reg,
        sa: u8,
    },
    /// Shift right logical (by immediate sa). `SRL rd, rt, sa`.
    Srl {
        rd: Reg,
        rt: Reg,
        sa: u8,
    },
    /// Shift right arithmetic (by immediate sa). `SRA rd, rt, sa`.
    Sra {
        rd: Reg,
        rt: Reg,
        sa: u8,
    },
    /// Shift left logical variable (by rs). `SLLV rd, rt, rs`.
    Sllv {
        rd: Reg,
        rt: Reg,
        rs: Reg,
    },
    /// Shift right logical variable. `SRLV rd, rt, rs`.
    Srlv {
        rd: Reg,
        rt: Reg,
        rs: Reg,
    },
    /// Shift right arithmetic variable. `SRAV rd, rt, rs`.
    Srav {
        rd: Reg,
        rt: Reg,
        rs: Reg,
    },

    // --- Mult/Div (R-type, SPECIAL; write HI/LO) ---
    /// Multiply signed. `MULT rs, rt`.
    Mult {
        rs: Reg,
        rt: Reg,
    },
    /// Multiply unsigned. `MULTU rs, rt`.
    Multu {
        rs: Reg,
        rt: Reg,
    },
    /// Divide signed. `DIV rs, rt`.
    Div {
        rs: Reg,
        rt: Reg,
    },
    /// Divide unsigned. `DIVU rs, rt`.
    Divu {
        rs: Reg,
        rt: Reg,
    },
    /// Move from HI. `MFHI rd`.
    Mfhi {
        rd: Reg,
    },
    /// Move from LO. `MFLO rd`.
    Mflo {
        rd: Reg,
    },
    /// Move to HI. `MTHI rs`.
    Mthi {
        rs: Reg,
    },
    /// Move to LO. `MTLO rs`.
    Mtlo {
        rs: Reg,
    },

    // --- Branches (I-type; branch-relative 16-bit offset in words) ---
    /// Branch if equal. `BEQ rs, rt, off`.
    Beq {
        rs: Reg,
        rt: Reg,
        off: i16,
    },
    /// Branch if not equal. `BNE rs, rt, off`.
    Bne {
        rs: Reg,
        rt: Reg,
        off: i16,
    },
    /// Branch if <= 0. `BLEZ rs, off`.
    Blez {
        rs: Reg,
        off: i16,
    },
    /// Branch if > 0. `BGTZ rs, off`.
    Bgtz {
        rs: Reg,
        off: i16,
    },
    /// Branch if < 0 (REGIMM). `BLTZ rs, off`.
    Bltz {
        rs: Reg,
        off: i16,
    },
    /// Branch if >= 0 (REGIMM). `BGEZ rs, off`.
    Bgez {
        rs: Reg,
        off: i16,
    },
    /// Branch-and-link if < 0 (REGIMM). `BLTZAL rs, off`.
    Bltzal {
        rs: Reg,
        off: i16,
    },
    /// Branch-and-link if >= 0 (REGIMM). `BGEZAL rs, off`.
    Bgezal {
        rs: Reg,
        off: i16,
    },
    /// Branch-and-link-likely if < 0. `BLTZALL rs, off`.
    Bltzall {
        rs: Reg,
        off: i16,
    },
    /// Branch-and-link-likely if >= 0. `BGEZALL rs, off`.
    Bgezall {
        rs: Reg,
        off: i16,
    },

    // --- Branch-likely variants (nullify delay slot when NOT taken) ---
    /// Branch-likely equal. `BEQL rs, rt, off`.
    Beql {
        rs: Reg,
        rt: Reg,
        off: i16,
    },
    /// Branch-likely not equal. `BNEL rs, rt, off`.
    Bnel {
        rs: Reg,
        rt: Reg,
        off: i16,
    },
    /// Branch-likely <= 0. `BLEZL rs, off`.
    Blezl {
        rs: Reg,
        off: i16,
    },
    /// Branch-likely > 0. `BGTZL rs, off`.
    Bgtzl {
        rs: Reg,
        off: i16,
    },
    /// Branch-likely < 0 (REGIMM). `BLTZL rs, off`.
    Bltzl {
        rs: Reg,
        off: i16,
    },
    /// Branch-likely >= 0 (REGIMM). `BGEZL rs, off`.
    Bgezl {
        rs: Reg,
        off: i16,
    },

    // --- Jumps ---
    /// Jump (absolute, 26-bit target). `J target`.
    J {
        target: u32,
    },
    /// Jump-and-link (absolute). `JAL target`.
    Jal {
        target: u32,
    },
    /// Jump register. `JR rs`.
    Jr {
        rs: Reg,
    },
    /// Jump-and-link register. `JALR rd, rs` (`rd=$ra` is an assembler default,
    /// not a decoder substitution).
    Jalr {
        rd: Reg,
        rs: Reg,
    },

    // ================================================================
    // COP1 / FPU (opcode 0x11, plus the dedicated COP1 load/store opcodes).
    //
    // Field layout for opcode 0x11 (byte-cited from the MIPS III / VR4300
    // reference): bits 25..21 = `fmt` (the sub-op / format selector, in the
    // `rs` position), 20..16 = `ft`, 15..11 = `fs`, 10..6 = `fd`, 5..0 =
    // `funct`. `fmt` values: MFC1=0x00, DMFC1=0x01, CFC1=0x02, MTC1=0x04,
    // DMTC1=0x05, CTC1=0x06, BC1=0x08, S=0x10, D=0x11, W=0x14, L=0x15.
    //
    // We name FPU register indices `fd`/`fs`/`ft` (5-bit, 0..31) to keep them
    // distinct from the GPR `Reg`s in the same struct.
    // ================================================================

    // --- COP1 moves between GPR and FPR (fmt sub-dispatch of opcode 0x11) ---
    /// Move word from COP1: `MFC1 rt, fs` — GPR rt = sign-extend(FPR fs low32).
    Mfc1 {
        rt: Reg,
        fs: Reg,
    },
    /// Move word to COP1: `MTC1 rt, fs` — FPR fs low32 = GPR rt low32.
    Mtc1 {
        rt: Reg,
        fs: Reg,
    },
    /// Doubleword move from COP1: `DMFC1 rt, fs` — GPR rt = FPR fs full 64 bits.
    Dmfc1 {
        rt: Reg,
        fs: Reg,
    },
    /// Doubleword move to COP1: `DMTC1 rt, fs` — FPR fs 64 bits = GPR rt.
    Dmtc1 {
        rt: Reg,
        fs: Reg,
    },
    /// Move control word from COP1: `CFC1 rt, fs` (reads FCR; fs is the
    /// control-register index, 0 or 31 in practice).
    Cfc1 {
        rt: Reg,
        fs: Reg,
    },
    /// Move control word to COP1: `CTC1 rt, fs`.
    Ctc1 {
        rt: Reg,
        fs: Reg,
    },

    // --- COP1 loads/stores (dedicated main opcodes) ---
    /// Load word to COP1: `LWC1 ft, off(base)` — FPR ft low32 = mem word.
    Lwc1 {
        ft: Reg,
        base: Reg,
        off: i16,
    },
    /// Store word from COP1: `SWC1 ft, off(base)`.
    Swc1 {
        ft: Reg,
        base: Reg,
        off: i16,
    },
    /// Load doubleword to COP1: `LDC1 ft, off(base)` — FPR ft 64 bits = mem dword.
    Ldc1 {
        ft: Reg,
        base: Reg,
        off: i16,
    },
    /// Store doubleword from COP1: `SDC1 ft, off(base)`.
    Sdc1 {
        ft: Reg,
        base: Reg,
        off: i16,
    },

    // --- Single-precision (fmt = S = 0x10) arithmetic (funct in 5..0) ---
    /// `ADD.S fd, fs, ft`.
    AddS {
        fd: Reg,
        fs: Reg,
        ft: Reg,
    },
    /// `SUB.S fd, fs, ft`.
    SubS {
        fd: Reg,
        fs: Reg,
        ft: Reg,
    },
    /// `MUL.S fd, fs, ft`.
    MulS {
        fd: Reg,
        fs: Reg,
        ft: Reg,
    },
    /// `DIV.S fd, fs, ft`.
    DivS {
        fd: Reg,
        fs: Reg,
        ft: Reg,
    },
    /// `ABS.S fd, fs`.
    AbsS {
        fd: Reg,
        fs: Reg,
    },
    /// `NEG.S fd, fs`.
    NegS {
        fd: Reg,
        fs: Reg,
    },
    /// `SQRT.S fd, fs`.
    SqrtS {
        fd: Reg,
        fs: Reg,
    },
    /// `MOV.S fd, fs` (bit-exact copy of the 32-bit register).
    MovS {
        fd: Reg,
        fs: Reg,
    },
    /// `MOVF.S fd, fs, cc` / `MOVT.S fd, fs, cc` — copy fs->fd (single) iff the
    /// FPU condition flag equals `tf`. `tf` = true for MOVT (move-if-true),
    /// false for MOVF (move-if-false). No rounding, no IEEE exception.
    MovcfS {
        fd: Reg,
        fs: Reg,
        tf: bool,
    },
    /// `MOVZ.S fd, fs, rt` — copy fs->fd (single) iff GPR `rt` == 0.
    MovzS {
        fd: Reg,
        fs: Reg,
        rt: Reg,
    },
    /// `MOVN.S fd, fs, rt` — copy fs->fd (single) iff GPR `rt` != 0.
    MovnS {
        fd: Reg,
        fs: Reg,
        rt: Reg,
    },

    // --- Double-precision (fmt = D = 0x11) arithmetic ---
    /// `ADD.D fd, fs, ft`.
    AddD {
        fd: Reg,
        fs: Reg,
        ft: Reg,
    },
    /// `SUB.D fd, fs, ft`.
    SubD {
        fd: Reg,
        fs: Reg,
        ft: Reg,
    },
    /// `MUL.D fd, fs, ft`.
    MulD {
        fd: Reg,
        fs: Reg,
        ft: Reg,
    },
    /// `DIV.D fd, fs, ft`.
    DivD {
        fd: Reg,
        fs: Reg,
        ft: Reg,
    },
    /// `ABS.D fd, fs`.
    AbsD {
        fd: Reg,
        fs: Reg,
    },
    /// `NEG.D fd, fs`.
    NegD {
        fd: Reg,
        fs: Reg,
    },
    /// `SQRT.D fd, fs`.
    SqrtD {
        fd: Reg,
        fs: Reg,
    },
    /// `MOV.D fd, fs` (bit-exact 64-bit copy).
    MovD {
        fd: Reg,
        fs: Reg,
    },
    /// `MOVF.D fd, fs, cc` / `MOVT.D fd, fs, cc` — copy fs->fd (double) iff the
    /// FPU condition flag equals `tf`. See [`Instruction::MovcfS`].
    MovcfD {
        fd: Reg,
        fs: Reg,
        tf: bool,
    },
    /// `MOVZ.D fd, fs, rt` — copy fs->fd (double) iff GPR `rt` == 0.
    MovzD {
        fd: Reg,
        fs: Reg,
        rt: Reg,
    },
    /// `MOVN.D fd, fs, rt` — copy fs->fd (double) iff GPR `rt` != 0.
    MovnD {
        fd: Reg,
        fs: Reg,
        rt: Reg,
    },

    // --- Conversions. Naming: `Cvt<To><From>`. `W`=32-bit int, `L`=64-bit int,
    //     `S`=single float, `D`=double float. `Trunc*` = round-toward-zero. ---
    /// `CVT.S.W fd, fs` — int32 -> single.
    CvtSW {
        fd: Reg,
        fs: Reg,
    },
    /// `CVT.D.W fd, fs` — int32 -> double.
    CvtDW {
        fd: Reg,
        fs: Reg,
    },
    /// `CVT.S.D fd, fs` — double -> single.
    CvtSD {
        fd: Reg,
        fs: Reg,
    },
    /// `CVT.D.S fd, fs` — single -> double.
    CvtDS {
        fd: Reg,
        fs: Reg,
    },
    /// `CVT.S.L fd, fs` — int64 -> single.
    CvtSL {
        fd: Reg,
        fs: Reg,
    },
    /// `CVT.D.L fd, fs` — int64 -> double.
    CvtDL {
        fd: Reg,
        fs: Reg,
    },
    /// `CVT.W.S fd, fs` — single -> int32 (rounds per FCSR mode).
    CvtWS {
        fd: Reg,
        fs: Reg,
    },
    /// `CVT.W.D fd, fs` — double -> int32 (rounds per FCSR mode).
    CvtWD {
        fd: Reg,
        fs: Reg,
    },
    /// `CVT.L.S fd, fs` — single -> int64 (rounds per FCSR mode).
    CvtLS {
        fd: Reg,
        fs: Reg,
    },
    /// `CVT.L.D fd, fs` — double -> int64 (rounds per FCSR mode).
    CvtLD {
        fd: Reg,
        fs: Reg,
    },
    /// `TRUNC.W.S fd, fs` — single -> int32, toward zero.
    TruncWS {
        fd: Reg,
        fs: Reg,
    },
    /// `TRUNC.W.D fd, fs` — double -> int32, toward zero.
    TruncWD {
        fd: Reg,
        fs: Reg,
    },
    /// `TRUNC.L.S fd, fs` — single -> int64, toward zero.
    TruncLS {
        fd: Reg,
        fs: Reg,
    },
    /// `TRUNC.L.D fd, fs` — double -> int64, toward zero.
    TruncLD {
        fd: Reg,
        fs: Reg,
    },
    /// `ROUND.W.S fd, fs` — nearest, ties to even.
    RoundWS {
        fd: Reg,
        fs: Reg,
    },
    /// `CEIL.W.S fd, fs` — toward +infinity.
    CeilWS {
        fd: Reg,
        fs: Reg,
    },
    /// `FLOOR.W.S fd, fs` — toward -infinity.
    FloorWS {
        fd: Reg,
        fs: Reg,
    },
    /// `ROUND.L.S fd, fs`.
    RoundLS {
        fd: Reg,
        fs: Reg,
    },
    /// `CEIL.L.S fd, fs`.
    CeilLS {
        fd: Reg,
        fs: Reg,
    },
    /// `FLOOR.L.S fd, fs`.
    FloorLS {
        fd: Reg,
        fs: Reg,
    },
    /// `ROUND.W.D fd, fs`.
    RoundWD {
        fd: Reg,
        fs: Reg,
    },
    /// `CEIL.W.D fd, fs`.
    CeilWD {
        fd: Reg,
        fs: Reg,
    },
    /// `FLOOR.W.D fd, fs`.
    FloorWD {
        fd: Reg,
        fs: Reg,
    },
    /// `ROUND.L.D fd, fs`.
    RoundLD {
        fd: Reg,
        fs: Reg,
    },
    /// `CEIL.L.D fd, fs`.
    CeilLD {
        fd: Reg,
        fs: Reg,
    },
    /// `FLOOR.L.D fd, fs`.
    FloorLD {
        fd: Reg,
        fs: Reg,
    },

    // --- FP compares: set the FP condition flag (FCSR bit 23). `fmt`
    //     distinguishes S (0x10) vs D (0x11); `funct` picks the predicate.
    //     All sixteen funct values 0x30..0x3F are decoded. ---
    /// `C.EQ.S fs, ft` — set condition = (fs == ft).
    CEqS {
        fs: Reg,
        ft: Reg,
    },
    /// `C.LT.S fs, ft` — set condition = (fs < ft).
    CLtS {
        fs: Reg,
        ft: Reg,
    },
    /// `C.LE.S fs, ft` — set condition = (fs <= ft).
    CLeS {
        fs: Reg,
        ft: Reg,
    },
    /// `C.EQ.D fs, ft`.
    CEqD {
        fs: Reg,
        ft: Reg,
    },
    /// `C.LT.D fs, ft`.
    CLtD {
        fs: Reg,
        ft: Reg,
    },
    /// `C.LE.D fs, ft`.
    CLeD {
        fs: Reg,
        ft: Reg,
    },
    /// Any other documented `C.cond.S`; `cond` is funct bits 3..0.
    CCondS {
        fs: Reg,
        ft: Reg,
        cond: u8,
    },
    /// Any other documented `C.cond.D`; `cond` is funct bits 3..0.
    CCondD {
        fs: Reg,
        ft: Reg,
        cond: u8,
    },

    // --- COP1 conditional branches (fmt = BC1 = 0x08; ft bit0 = tf, bit1 = nd). ---
    /// `BC1T off` — branch if FP condition flag is set.
    Bc1t {
        off: i16,
    },
    /// `BC1F off` — branch if FP condition flag is clear.
    Bc1f {
        off: i16,
    },
    /// `BC1TL off` — branch-likely if flag set.
    Bc1tl {
        off: i16,
    },
    /// `BC1FL off` — branch-likely if flag clear.
    Bc1fl {
        off: i16,
    },

    // --- COP0 system control (opcode 0x10) ---
    //
    // The N64 CPU's system coprocessor. On a recompiled title almost all COP0
    // register state (Status/Cause/EPC/the TLB) is owned by the libultra host,
    // not the game — so most of these are privileged ops the recompiled body
    // should never execute, and are emitted as **loud traps**, never a silent
    // nop. The two that generated code can legitimately reach are Count/Compare
    // moves (the guts of `osGetCount`/`osSetTimer`), which read/write real
    // context state. `cop0d` is the register index (rd field, bits 15..11).
    /// Move-from COP0. `MFC0 rt, cop0d` (32-bit, sign-extended into GPR).
    Mfc0 {
        rt: Reg,
        cop0d: u8,
    },
    /// Move-to COP0. `MTC0 rt, cop0d` (32-bit).
    Mtc0 {
        rt: Reg,
        cop0d: u8,
    },
    /// Doubleword move-from COP0. `DMFC0 rt, cop0d` (64-bit).
    Dmfc0 {
        rt: Reg,
        cop0d: u8,
    },
    /// Doubleword move-to COP0. `DMTC0 rt, cop0d` (64-bit).
    Dmtc0 {
        rt: Reg,
        cop0d: u8,
    },
    /// Branch on the COP0 condition bit (Status.CH on VR4300).
    Bc0f {
        off: i16,
    },
    Bc0t {
        off: i16,
    },
    Bc0fl {
        off: i16,
    },
    Bc0tl {
        off: i16,
    },
    /// Exception return. `ERET` — privileged; returns from an interrupt/exception.
    Eret,
    /// Write indexed TLB entry. `TLBWI` — privileged MMU op.
    Tlbwi,
    /// Write random TLB entry. `TLBWR` — privileged MMU op.
    Tlbwr,
    /// Probe TLB for matching entry. `TLBP` — privileged MMU op.
    Tlbp,
    /// Read indexed TLB entry. `TLBR` — privileged MMU op.
    Tlbr,

    // --- Cache / synchronization ---
    /// Cache operation. `CACHE op, off(base)`. On a recompiled title the host
    /// rdram is already coherent, so this is a semantic no-op (emitted with a
    /// comment), matching how every N64 static/dynamic recompiler treats it.
    Cache {
        op: u8,
        base: Reg,
        off: i16,
    },
    /// Store-ordering barrier. `SYNC` — a no-op in a single-threaded recompiled
    /// context (no store buffer to drain).
    Sync,

    // --- COP2 (unused coprocessor) stubs (opcode 0x12) ---
    //
    // COP2 is not wired to anything on the N64; libultra never uses it and no
    // ordinary game touches it. We decode the move ops so an unexpected COP2
    // instruction is a *named loud trap* rather than a bare `Unknown` word.
    /// Move-from COP2. `MFC2 rt, rd`.
    Mfc2 {
        rt: Reg,
        rd: Reg,
    },
    /// Move-to COP2. `MTC2 rt, rd`.
    Mtc2 {
        rt: Reg,
        rd: Reg,
    },
    /// Move control-from COP2. `CFC2 rt, rd`.
    Cfc2 {
        rt: Reg,
        rd: Reg,
    },
    /// Move control-to COP2. `CTC2 rt, rd`.
    Ctc2 {
        rt: Reg,
        rd: Reg,
    },
    /// Doubleword move-from COP2. `DMFC2 rt, rd` (loud unusable-coprocessor trap).
    Dmfc2 {
        rt: Reg,
        rd: Reg,
    },
    /// Doubleword move-to COP2. `DMTC2 rt, rd`.
    Dmtc2 {
        rt: Reg,
        rd: Reg,
    },
    /// Any COP2 branch or coprocessor operation, retained as a named trap.
    Cop2Op {
        word: u32,
    },
    /// COP2 load/store primary opcodes, all unusable on the N64.
    Lwc2 {
        rt: Reg,
        base: Reg,
        off: i16,
    },
    Ldc2 {
        rt: Reg,
        base: Reg,
        off: i16,
    },
    Swc2 {
        rt: Reg,
        base: Reg,
        off: i16,
    },
    Sdc2 {
        rt: Reg,
        base: Reg,
        off: i16,
    },

    // --- Integer conditional traps (MIPS III, full-width GPR operands) ---
    Tge {
        rs: Reg,
        rt: Reg,
        code: u16,
    },
    Tgeu {
        rs: Reg,
        rt: Reg,
        code: u16,
    },
    Tlt {
        rs: Reg,
        rt: Reg,
        code: u16,
    },
    Tltu {
        rs: Reg,
        rt: Reg,
        code: u16,
    },
    Teq {
        rs: Reg,
        rt: Reg,
        code: u16,
    },
    Tne {
        rs: Reg,
        rt: Reg,
        code: u16,
    },
    Tgei {
        rs: Reg,
        imm: i16,
    },
    Tgeiu {
        rs: Reg,
        imm: i16,
    },
    Tlti {
        rs: Reg,
        imm: i16,
    },
    Tltiu {
        rs: Reg,
        imm: i16,
    },
    Teqi {
        rs: Reg,
        imm: i16,
    },
    Tnei {
        rs: Reg,
        imm: i16,
    },

    // --- Traps (SPECIAL) ---
    /// System call. `SYSCALL code` — raises an exception; emitted as a loud trap.
    Syscall {
        code: u32,
    },
    /// Breakpoint. `BREAK code` — raises an exception; emitted as a loud trap.
    Break {
        code: u32,
    },

    /// A word we do not (yet) decode. Carries the raw bits so the emitter can
    /// fail loudly instead of silently emitting a nop.
    Unknown {
        word: u32,
    },
}

// --- Field extraction (public ISA bit positions) ---

mod dispatch;
pub use dispatch::decode;

impl Instruction {
    /// Whether executing this instruction requires kernel mode or Status.CU0.
    /// Keeping the complete decoded COP0 family here prevents either
    /// arbitrary-PC lane from accidentally admitting a new shape unguarded.
    pub const fn requires_cop0(&self) -> bool {
        use Instruction::*;
        matches!(
            self,
            Mfc0 { .. }
                | Dmfc0 { .. }
                | Mtc0 { .. }
                | Dmtc0 { .. }
                | Bc0f { .. }
                | Bc0t { .. }
                | Bc0fl { .. }
                | Bc0tl { .. }
                | Eret
                | Tlbwi
                | Tlbwr
                | Tlbp
                | Tlbr
        )
    }

    /// Whether executing this instruction requires Status.CU1. Keeping this
    /// classification beside the decoder makes the block emitter's
    /// coprocessor guard exhaustive across arithmetic, moves, memory, compare,
    /// conversion, and branch families.
    pub const fn requires_cop1(&self) -> bool {
        use Instruction::*;
        matches!(
            self,
            Mfc1 { .. }
                | Mtc1 { .. }
                | Dmfc1 { .. }
                | Dmtc1 { .. }
                | Cfc1 { .. }
                | Ctc1 { .. }
                | Lwc1 { .. }
                | Swc1 { .. }
                | Ldc1 { .. }
                | Sdc1 { .. }
                | AddS { .. }
                | SubS { .. }
                | MulS { .. }
                | DivS { .. }
                | AbsS { .. }
                | NegS { .. }
                | SqrtS { .. }
                | MovS { .. }
                | MovcfS { .. }
                | MovzS { .. }
                | MovnS { .. }
                | AddD { .. }
                | SubD { .. }
                | MulD { .. }
                | DivD { .. }
                | AbsD { .. }
                | NegD { .. }
                | SqrtD { .. }
                | MovD { .. }
                | MovcfD { .. }
                | MovzD { .. }
                | MovnD { .. }
                | CvtSW { .. }
                | CvtDW { .. }
                | CvtSD { .. }
                | CvtDS { .. }
                | CvtSL { .. }
                | CvtDL { .. }
                | CvtWS { .. }
                | CvtWD { .. }
                | CvtLS { .. }
                | CvtLD { .. }
                | TruncWS { .. }
                | TruncWD { .. }
                | TruncLS { .. }
                | TruncLD { .. }
                | RoundWS { .. }
                | CeilWS { .. }
                | FloorWS { .. }
                | RoundLS { .. }
                | CeilLS { .. }
                | FloorLS { .. }
                | RoundWD { .. }
                | CeilWD { .. }
                | FloorWD { .. }
                | RoundLD { .. }
                | CeilLD { .. }
                | FloorLD { .. }
                | CEqS { .. }
                | CLtS { .. }
                | CLeS { .. }
                | CEqD { .. }
                | CLtD { .. }
                | CLeD { .. }
                | CCondS { .. }
                | CCondD { .. }
                | Bc1t { .. }
                | Bc1f { .. }
                | Bc1tl { .. }
                | Bc1fl { .. }
        )
    }

    /// Whether this instruction is a control-transfer whose *following*
    /// instruction is in its delay slot (all branches and jumps on MIPS).
    pub fn has_delay_slot(&self) -> bool {
        use Instruction::*;
        matches!(
            self,
            Beq { .. }
                | Bne { .. }
                | Blez { .. }
                | Bgtz { .. }
                | Bltz { .. }
                | Bgez { .. }
                | Bltzal { .. }
                | Bgezal { .. }
                | Bltzall { .. }
                | Bgezall { .. }
                | Beql { .. }
                | Bnel { .. }
                | Blezl { .. }
                | Bgtzl { .. }
                | Bltzl { .. }
                | Bgezl { .. }
                | J { .. }
                | Jal { .. }
                | Jr { .. }
                | Jalr { .. }
                | Bc1t { .. }
                | Bc1f { .. }
                | Bc1tl { .. }
                | Bc1fl { .. }
                | Bc0t { .. }
                | Bc0f { .. }
                | Bc0tl { .. }
                | Bc0fl { .. }
        )
    }

    /// Whether this is a *branch-likely* op (nullifies its delay slot when the
    /// branch is NOT taken). Needed because the emitter places the delay-slot
    /// instruction inside the taken-branch block for these.
    pub fn is_branch_likely(&self) -> bool {
        use Instruction::*;
        matches!(
            self,
            Beql { .. }
                | Bnel { .. }
                | Blezl { .. }
                | Bgtzl { .. }
                | Bltzl { .. }
                | Bgezl { .. }
                | Bltzall { .. }
                | Bgezall { .. }
                | Bc1tl { .. }
                | Bc1fl { .. }
                | Bc0tl { .. }
                | Bc0fl { .. }
        )
    }
}
