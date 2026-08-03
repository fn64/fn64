//! Stable, non-executing records for the first static-micro-op representation.
//!
//! The dense AOT lane currently emits one large Rust body per admitted word.
//! This record separates immutable program data from the future shared
//! executor: the exact raw word remains available for live-image verification
//! and operand extraction, while `opcode` is a stable dispatch key. This module
//! deliberately provides no execution path and grants no `production-aot`
//! authority.

use std::fmt;

use crate::decoder::{decode, Instruction};
use sha2::{Digest, Sha256};

pub const STATIC_MICRO_OP_RECORD_V1_BYTES: usize = 8;
pub const STATIC_MICRO_OP_FORMAT_SOURCE_SCHEMA_V1: &str = "fn64.static-micro-op-format-source.v1";
pub const STATIC_MICRO_OP_FORMAT_SOURCE_SCHEMA_V2: &str = "fn64.static-micro-op-format-source.v2";
pub const STATIC_MICRO_OP_PACK_SCHEMA_V1: &str = "fn64.static-micro-op-pack.v1";
pub const STATIC_MICRO_OP_PACK_SCHEMA_V2: &str = "fn64.static-micro-op-pack.v2";
pub const STATIC_MICRO_OP_MAGIC_V1: &[u8; 8] = b"FN64SM01";
pub const STATIC_MICRO_OP_MAGIC_V2: &[u8; 8] = b"FN64SM02";
pub const STATIC_MICRO_OP_HEADER_V1_BYTES: usize = 8 + 4 + 8 + 32;
pub const STATIC_MICRO_OP_SPAN_HEADER_V1_BYTES: usize = 8 + 4 + 4;
pub const STATIC_MICRO_OP_SPAN_HEADER_V2_BYTES: usize = 8 + 4 + 4 + 1;

pub const STATIC_MICRO_OP_OPCODE_RESERVED_INSTRUCTION_V1: u16 = 205;

const FLAG_DELAY_SLOT: u8 = 1 << 0;
const FLAG_BRANCH_LIKELY: u8 = 1 << 1;
const FLAG_REQUIRES_COP0: u8 = 1 << 2;
const FLAG_REQUIRES_COP1: u8 = 1 << 3;

/// Source identity for the stable record mapping and the decoder it consumes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StaticMicroOpFormatSourceReceiptV1 {
    source_sha256: [u8; 32],
}

impl StaticMicroOpFormatSourceReceiptV1 {
    pub const fn schema(self) -> &'static str {
        STATIC_MICRO_OP_FORMAT_SOURCE_SCHEMA_V1
    }

    pub const fn source_sha256(self) -> [u8; 32] {
        self.source_sha256
    }
}

pub fn static_micro_op_format_source_receipt_v1() -> StaticMicroOpFormatSourceReceiptV1 {
    let sources: &[(&[u8], &[u8])] = &[
        (
            b"src/decoder/dispatch.rs",
            include_bytes!("decoder/dispatch.rs"),
        ),
        (
            b"src/decoder/mod.rs",
            include_bytes!("decoder/mod.rs"),
        ),
        (
            b"src/static_micro_op.rs",
            include_bytes!("static_micro_op.rs"),
        ),
    ];
    let mut hasher = Sha256::new();
    hasher.update(b"fn64:static-micro-op-format-source:v1:");
    for (label, source) in sources {
        hasher.update((label.len() as u64).to_be_bytes());
        hasher.update(label);
        hasher.update((source.len() as u64).to_be_bytes());
        hasher.update(source);
    }
    StaticMicroOpFormatSourceReceiptV1 {
        source_sha256: hasher.finalize().into(),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StaticMicroOpFormatSourceReceiptV2 {
    source_sha256: [u8; 32],
}

impl StaticMicroOpFormatSourceReceiptV2 {
    pub const fn schema(self) -> &'static str {
        STATIC_MICRO_OP_FORMAT_SOURCE_SCHEMA_V2
    }

    pub const fn source_sha256(self) -> [u8; 32] {
        self.source_sha256
    }
}

pub fn static_micro_op_format_source_receipt_v2() -> StaticMicroOpFormatSourceReceiptV2 {
    let sources: &[(&[u8], &[u8])] = &[
        (
            b"src/decoder/dispatch.rs",
            include_bytes!("decoder/dispatch.rs"),
        ),
        (
            b"src/decoder/mod.rs",
            include_bytes!("decoder/mod.rs"),
        ),
        (
            b"src/static_micro_op.rs",
            include_bytes!("static_micro_op.rs"),
        ),
    ];
    let mut hasher = Sha256::new();
    hasher.update(b"fn64:static-micro-op-format-source:v2:");
    for (label, source) in sources {
        hasher.update((label.len() as u64).to_be_bytes());
        hasher.update(label);
        hasher.update((source.len() as u64).to_be_bytes());
        hasher.update(source);
    }
    StaticMicroOpFormatSourceReceiptV2 {
        source_sha256: hasher.finalize().into(),
    }
}

/// One canonical static-micro-op record.
///
/// The in-memory layout is fixed at eight bytes, but artifact producers must
/// use [`Self::to_bytes`] rather than native layout so byte order is canonical
/// on every host.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StaticMicroOpRecordV1 {
    pub expected_raw_word: u32,
    pub opcode: u16,
    pub flags: u8,
    pub reserved: u8,
}

const _: [(); STATIC_MICRO_OP_RECORD_V1_BYTES] = [(); std::mem::size_of::<StaticMicroOpRecordV1>()];

impl StaticMicroOpRecordV1 {
    pub fn from_word(expected_raw_word: u32) -> Self {
        let instruction = decode(expected_raw_word);
        Self {
            expected_raw_word,
            opcode: opcode_v1(instruction),
            flags: flags_v1(instruction),
            reserved: 0,
        }
    }

    pub const fn to_bytes(self) -> [u8; STATIC_MICRO_OP_RECORD_V1_BYTES] {
        let word = self.expected_raw_word.to_be_bytes();
        let opcode = self.opcode.to_be_bytes();
        [
            word[0],
            word[1],
            word[2],
            word[3],
            opcode[0],
            opcode[1],
            self.flags,
            self.reserved,
        ]
    }

    pub fn from_bytes(
        bytes: [u8; STATIC_MICRO_OP_RECORD_V1_BYTES],
    ) -> Result<Self, StaticMicroOpRecordErrorV1> {
        let record = Self {
            expected_raw_word: u32::from_be_bytes(bytes[0..4].try_into().unwrap()),
            opcode: u16::from_be_bytes(bytes[4..6].try_into().unwrap()),
            flags: bytes[6],
            reserved: bytes[7],
        };
        record.validate()?;
        Ok(record)
    }

    pub fn validate(self) -> Result<(), StaticMicroOpRecordErrorV1> {
        if self.reserved != 0 {
            return Err(StaticMicroOpRecordErrorV1::ReservedNonzero {
                actual: self.reserved,
            });
        }
        let expected = Self::from_word(self.expected_raw_word);
        if self.opcode != expected.opcode {
            return Err(StaticMicroOpRecordErrorV1::OpcodeMismatch {
                expected: expected.opcode,
                actual: self.opcode,
            });
        }
        if self.flags != expected.flags {
            return Err(StaticMicroOpRecordErrorV1::FlagsMismatch {
                expected: expected.flags,
                actual: self.flags,
            });
        }
        Ok(())
    }

    pub const fn has_delay_slot(self) -> bool {
        self.flags & FLAG_DELAY_SLOT != 0
    }

    pub const fn is_branch_likely(self) -> bool {
        self.flags & FLAG_BRANCH_LIKELY != 0
    }

    pub const fn requires_cop0(self) -> bool {
        self.flags & FLAG_REQUIRES_COP0 != 0
    }

    pub const fn requires_cop1(self) -> bool {
        self.flags & FLAG_REQUIRES_COP1 != 0
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StaticMicroOpRecordErrorV1 {
    ReservedNonzero { actual: u8 },
    OpcodeMismatch { expected: u16, actual: u16 },
    FlagsMismatch { expected: u8, actual: u8 },
}

impl fmt::Display for StaticMicroOpRecordErrorV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            Self::ReservedNonzero { actual } => write!(
                formatter,
                "static-micro-op.v1 reserved byte must be zero, observed {actual:#04x}"
            ),
            Self::OpcodeMismatch { expected, actual } => write!(
                formatter,
                "static-micro-op.v1 opcode {actual} does not match decoded opcode {expected}"
            ),
            Self::FlagsMismatch { expected, actual } => write!(
                formatter,
                "static-micro-op.v1 flags {actual:#04x} do not match decoded flags {expected:#04x}"
            ),
        }
    }
}

impl std::error::Error for StaticMicroOpRecordErrorV1 {}

fn flags_v1(instruction: Instruction) -> u8 {
    (u8::from(instruction.has_delay_slot()) * FLAG_DELAY_SLOT)
        | (u8::from(instruction.is_branch_likely()) * FLAG_BRANCH_LIKELY)
        | (u8::from(instruction.requires_cop0()) * FLAG_REQUIRES_COP0)
        | (u8::from(instruction.requires_cop1()) * FLAG_REQUIRES_COP1)
}

// This match is intentionally exhaustive. Adding a decoded instruction must
// fail compilation until its stable static-micro-op dispatch key is assigned.
fn opcode_v1(instruction: Instruction) -> u16 {
    match instruction {
        Instruction::Nop => 0,
        Instruction::Lb { .. } => 1,
        Instruction::Lbu { .. } => 2,
        Instruction::Lh { .. } => 3,
        Instruction::Lhu { .. } => 4,
        Instruction::Lw { .. } => 5,
        Instruction::Lwu { .. } => 6,
        Instruction::Lwl { .. } => 7,
        Instruction::Lwr { .. } => 8,
        Instruction::Sb { .. } => 9,
        Instruction::Sh { .. } => 10,
        Instruction::Sw { .. } => 11,
        Instruction::Swl { .. } => 12,
        Instruction::Swr { .. } => 13,
        Instruction::Ld { .. } => 14,
        Instruction::Sd { .. } => 15,
        Instruction::Ldl { .. } => 16,
        Instruction::Ldr { .. } => 17,
        Instruction::Sdl { .. } => 18,
        Instruction::Sdr { .. } => 19,
        Instruction::Lld { .. } => 20,
        Instruction::Scd { .. } => 21,
        Instruction::Ll { .. } => 22,
        Instruction::Sc { .. } => 23,
        Instruction::Daddi { .. } => 24,
        Instruction::Daddiu { .. } => 25,
        Instruction::Dadd { .. } => 26,
        Instruction::Daddu { .. } => 27,
        Instruction::Dsub { .. } => 28,
        Instruction::Dsubu { .. } => 29,
        Instruction::Dsll { .. } => 30,
        Instruction::Dsrl { .. } => 31,
        Instruction::Dsra { .. } => 32,
        Instruction::Dsll32 { .. } => 33,
        Instruction::Dsrl32 { .. } => 34,
        Instruction::Dsra32 { .. } => 35,
        Instruction::Dsllv { .. } => 36,
        Instruction::Dsrlv { .. } => 37,
        Instruction::Dsrav { .. } => 38,
        Instruction::Dmult { .. } => 39,
        Instruction::Dmultu { .. } => 40,
        Instruction::Ddiv { .. } => 41,
        Instruction::Ddivu { .. } => 42,
        Instruction::Addi { .. } => 43,
        Instruction::Addiu { .. } => 44,
        Instruction::Slti { .. } => 45,
        Instruction::Sltiu { .. } => 46,
        Instruction::Andi { .. } => 47,
        Instruction::Ori { .. } => 48,
        Instruction::Xori { .. } => 49,
        Instruction::Lui { .. } => 50,
        Instruction::Add { .. } => 51,
        Instruction::Addu { .. } => 52,
        Instruction::Sub { .. } => 53,
        Instruction::Subu { .. } => 54,
        Instruction::And { .. } => 55,
        Instruction::Or { .. } => 56,
        Instruction::Xor { .. } => 57,
        Instruction::Nor { .. } => 58,
        Instruction::Slt { .. } => 59,
        Instruction::Sltu { .. } => 60,
        Instruction::Sll { .. } => 61,
        Instruction::Srl { .. } => 62,
        Instruction::Sra { .. } => 63,
        Instruction::Sllv { .. } => 64,
        Instruction::Srlv { .. } => 65,
        Instruction::Srav { .. } => 66,
        Instruction::Mult { .. } => 67,
        Instruction::Multu { .. } => 68,
        Instruction::Div { .. } => 69,
        Instruction::Divu { .. } => 70,
        Instruction::Mfhi { .. } => 71,
        Instruction::Mflo { .. } => 72,
        Instruction::Mthi { .. } => 73,
        Instruction::Mtlo { .. } => 74,
        Instruction::Beq { .. } => 75,
        Instruction::Bne { .. } => 76,
        Instruction::Blez { .. } => 77,
        Instruction::Bgtz { .. } => 78,
        Instruction::Bltz { .. } => 79,
        Instruction::Bgez { .. } => 80,
        Instruction::Bltzal { .. } => 81,
        Instruction::Bgezal { .. } => 82,
        Instruction::Bltzall { .. } => 83,
        Instruction::Bgezall { .. } => 84,
        Instruction::Beql { .. } => 85,
        Instruction::Bnel { .. } => 86,
        Instruction::Blezl { .. } => 87,
        Instruction::Bgtzl { .. } => 88,
        Instruction::Bltzl { .. } => 89,
        Instruction::Bgezl { .. } => 90,
        Instruction::J { .. } => 91,
        Instruction::Jal { .. } => 92,
        Instruction::Jr { .. } => 93,
        Instruction::Jalr { .. } => 94,
        Instruction::Mfc1 { .. } => 95,
        Instruction::Mtc1 { .. } => 96,
        Instruction::Dmfc1 { .. } => 97,
        Instruction::Dmtc1 { .. } => 98,
        Instruction::Cfc1 { .. } => 99,
        Instruction::Ctc1 { .. } => 100,
        Instruction::Lwc1 { .. } => 101,
        Instruction::Swc1 { .. } => 102,
        Instruction::Ldc1 { .. } => 103,
        Instruction::Sdc1 { .. } => 104,
        Instruction::AddS { .. } => 105,
        Instruction::SubS { .. } => 106,
        Instruction::MulS { .. } => 107,
        Instruction::DivS { .. } => 108,
        Instruction::AbsS { .. } => 109,
        Instruction::NegS { .. } => 110,
        Instruction::SqrtS { .. } => 111,
        Instruction::MovS { .. } => 112,
        Instruction::MovcfS { .. } => 113,
        Instruction::MovzS { .. } => 114,
        Instruction::MovnS { .. } => 115,
        Instruction::AddD { .. } => 116,
        Instruction::SubD { .. } => 117,
        Instruction::MulD { .. } => 118,
        Instruction::DivD { .. } => 119,
        Instruction::AbsD { .. } => 120,
        Instruction::NegD { .. } => 121,
        Instruction::SqrtD { .. } => 122,
        Instruction::MovD { .. } => 123,
        Instruction::MovcfD { .. } => 124,
        Instruction::MovzD { .. } => 125,
        Instruction::MovnD { .. } => 126,
        Instruction::CvtSW { .. } => 127,
        Instruction::CvtDW { .. } => 128,
        Instruction::CvtSD { .. } => 129,
        Instruction::CvtDS { .. } => 130,
        Instruction::CvtSL { .. } => 131,
        Instruction::CvtDL { .. } => 132,
        Instruction::CvtWS { .. } => 133,
        Instruction::CvtWD { .. } => 134,
        Instruction::CvtLS { .. } => 135,
        Instruction::CvtLD { .. } => 136,
        Instruction::TruncWS { .. } => 137,
        Instruction::TruncWD { .. } => 138,
        Instruction::TruncLS { .. } => 139,
        Instruction::TruncLD { .. } => 140,
        Instruction::RoundWS { .. } => 141,
        Instruction::CeilWS { .. } => 142,
        Instruction::FloorWS { .. } => 143,
        Instruction::RoundLS { .. } => 144,
        Instruction::CeilLS { .. } => 145,
        Instruction::FloorLS { .. } => 146,
        Instruction::RoundWD { .. } => 147,
        Instruction::CeilWD { .. } => 148,
        Instruction::FloorWD { .. } => 149,
        Instruction::RoundLD { .. } => 150,
        Instruction::CeilLD { .. } => 151,
        Instruction::FloorLD { .. } => 152,
        Instruction::CEqS { .. } => 153,
        Instruction::CLtS { .. } => 154,
        Instruction::CLeS { .. } => 155,
        Instruction::CEqD { .. } => 156,
        Instruction::CLtD { .. } => 157,
        Instruction::CLeD { .. } => 158,
        Instruction::CCondS { .. } => 159,
        Instruction::CCondD { .. } => 160,
        Instruction::Bc1t { .. } => 161,
        Instruction::Bc1f { .. } => 162,
        Instruction::Bc1tl { .. } => 163,
        Instruction::Bc1fl { .. } => 164,
        Instruction::Mfc0 { .. } => 165,
        Instruction::Mtc0 { .. } => 166,
        Instruction::Dmfc0 { .. } => 167,
        Instruction::Dmtc0 { .. } => 168,
        Instruction::Bc0f { .. } => 169,
        Instruction::Bc0t { .. } => 170,
        Instruction::Bc0fl { .. } => 171,
        Instruction::Bc0tl { .. } => 172,
        Instruction::Eret => 173,
        Instruction::Tlbwi => 174,
        Instruction::Tlbwr => 175,
        Instruction::Tlbp => 176,
        Instruction::Tlbr => 177,
        Instruction::Cache { .. } => 178,
        Instruction::Sync => 179,
        Instruction::Mfc2 { .. } => 180,
        Instruction::Mtc2 { .. } => 181,
        Instruction::Cfc2 { .. } => 182,
        Instruction::Ctc2 { .. } => 183,
        Instruction::Dmfc2 { .. } => 184,
        Instruction::Dmtc2 { .. } => 185,
        Instruction::Cop2Op { .. } => 186,
        Instruction::Lwc2 { .. } => 187,
        Instruction::Ldc2 { .. } => 188,
        Instruction::Swc2 { .. } => 189,
        Instruction::Sdc2 { .. } => 190,
        Instruction::Tge { .. } => 191,
        Instruction::Tgeu { .. } => 192,
        Instruction::Tlt { .. } => 193,
        Instruction::Tltu { .. } => 194,
        Instruction::Teq { .. } => 195,
        Instruction::Tne { .. } => 196,
        Instruction::Tgei { .. } => 197,
        Instruction::Tgeiu { .. } => 198,
        Instruction::Tlti { .. } => 199,
        Instruction::Tltiu { .. } => 200,
        Instruction::Teqi { .. } => 201,
        Instruction::Tnei { .. } => 202,
        Instruction::Syscall { .. } => 203,
        Instruction::Break { .. } => 204,
        Instruction::Unknown { .. } => STATIC_MICRO_OP_OPCODE_RESERVED_INSTRUCTION_V1,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_bytes_round_trip_and_keep_exact_raw_word() {
        let record = StaticMicroOpRecordV1::from_word(0x5000_0001); // beql
        assert_eq!(record.to_bytes()[0..4], 0x5000_0001u32.to_be_bytes());
        assert!(record.has_delay_slot());
        assert!(record.is_branch_likely());
        assert_eq!(
            StaticMicroOpRecordV1::from_bytes(record.to_bytes()),
            Ok(record)
        );
        assert_eq!(record.to_bytes().len(), STATIC_MICRO_OP_RECORD_V1_BYTES);
    }

    #[test]
    fn flags_are_derived_from_the_shared_decoder() {
        for word in [
            0x0000_0000, // nop
            0x1000_0001, // beq
            0x5000_0001, // beql
            0x4002_4800, // mfc0
            0x4402_2000, // mfc1
            0x7801_2345, // reserved
        ] {
            let instruction = decode(word);
            let record = StaticMicroOpRecordV1::from_word(word);
            assert_eq!(record.has_delay_slot(), instruction.has_delay_slot());
            assert_eq!(record.is_branch_likely(), instruction.is_branch_likely());
            assert_eq!(record.requires_cop0(), instruction.requires_cop0());
            assert_eq!(record.requires_cop1(), instruction.requires_cop1());
            assert_eq!(record.validate(), Ok(()));
        }
    }

    #[test]
    fn reserved_instruction_has_an_explicit_dispatch_key() {
        let record = StaticMicroOpRecordV1::from_word(0x7801_2345);
        assert!(matches!(
            decode(record.expected_raw_word),
            Instruction::Unknown { .. }
        ));
        assert_eq!(
            record.opcode,
            STATIC_MICRO_OP_OPCODE_RESERVED_INSTRUCTION_V1
        );
    }

    #[test]
    fn malformed_records_fail_loudly() {
        let canonical = StaticMicroOpRecordV1::from_word(0x5000_0001);

        let mut reserved = canonical.to_bytes();
        reserved[7] = 1;
        assert_eq!(
            StaticMicroOpRecordV1::from_bytes(reserved),
            Err(StaticMicroOpRecordErrorV1::ReservedNonzero { actual: 1 })
        );

        let mut opcode = canonical.to_bytes();
        opcode[5] ^= 1;
        assert!(matches!(
            StaticMicroOpRecordV1::from_bytes(opcode),
            Err(StaticMicroOpRecordErrorV1::OpcodeMismatch { .. })
        ));

        let mut flags = canonical.to_bytes();
        flags[6] ^= FLAG_BRANCH_LIKELY;
        assert!(matches!(
            StaticMicroOpRecordV1::from_bytes(flags),
            Err(StaticMicroOpRecordErrorV1::FlagsMismatch { .. })
        ));
    }

    #[test]
    fn format_receipt_binds_mapping_and_decoder_sources() {
        let receipt = static_micro_op_format_source_receipt_v1();
        assert_eq!(receipt.schema(), STATIC_MICRO_OP_FORMAT_SOURCE_SCHEMA_V1);
        assert_ne!(receipt.source_sha256(), [0; 32]);
        assert_eq!(receipt, static_micro_op_format_source_receipt_v1());
        let receipt_v2 = static_micro_op_format_source_receipt_v2();
        assert_eq!(receipt_v2.schema(), STATIC_MICRO_OP_FORMAT_SOURCE_SCHEMA_V2);
        assert_ne!(receipt_v2.source_sha256(), [0; 32]);
        assert_eq!(receipt_v2, static_micro_op_format_source_receipt_v2());
    }
}
