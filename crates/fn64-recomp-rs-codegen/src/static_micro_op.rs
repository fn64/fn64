//! Canonical `static-micro-op` artifact construction.
//!
//! This module is intentionally separate from the typed-Rust emitter. It
//! turns already admitted, bank-qualified instruction spans into a compact
//! transport artifact; selecting or executing that artifact is a runtime
//! concern.
//!
//! Both wires are entirely big-endian. V1 spans contain only owned records. V2
//! adds a canonical presence byte and at most one final delay-only lookahead;
//! the header instruction count remains the owned-word count. Spans are
//! ordered lexicographically by `(bank, vram)` and may not overlap within a
//! bank.

use fn64_recomp_rs::{
    static_micro_op_format_source_receipt_v1, static_micro_op_format_source_receipt_v2,
    AdmittedStaticMicroOpProgramV1, AdmittedStaticMicroOpProgramV2, BankId,
    StaticMicroOpPackErrorV1, StaticMicroOpRecordV1, STATIC_MICRO_OP_HEADER_V1_BYTES,
    STATIC_MICRO_OP_MAGIC_V1, STATIC_MICRO_OP_MAGIC_V2, STATIC_MICRO_OP_RECORD_V1_BYTES,
    STATIC_MICRO_OP_SPAN_HEADER_V1_BYTES,
};
use sha2::{Digest, Sha256};

const HEADER_LEN: usize = STATIC_MICRO_OP_HEADER_V1_BYTES;
const SPAN_HEADER_LEN: usize = STATIC_MICRO_OP_SPAN_HEADER_V1_BYTES;

pub use fn64_recomp_rs::STATIC_MICRO_OP_PACK_SCHEMA_V1;
pub const STATIC_MICRO_OP_PACKER_SOURCE_SCHEMA_V1: &str = "fn64.static-micro-op-packer-source.v1";
pub const STATIC_MICRO_OP_PACKER_SOURCE_SCHEMA_V2: &str = "fn64.static-micro-op-packer-source.v2";
pub const STATIC_MICRO_OP_PACKER_SOURCE_SCHEMA_V3: &str = "fn64.static-micro-op-packer-source.v3";

/// Exact codegen-side source identity, independent of the typed-Rust emitter
/// source receipt.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StaticMicroOpPackerSourceReceiptV1 {
    source_sha256: [u8; 32],
}

impl StaticMicroOpPackerSourceReceiptV1 {
    pub const fn schema(self) -> &'static str {
        STATIC_MICRO_OP_PACKER_SOURCE_SCHEMA_V1
    }

    pub const fn source_sha256(self) -> [u8; 32] {
        self.source_sha256
    }
}

pub fn static_micro_op_packer_source_receipt_v1() -> StaticMicroOpPackerSourceReceiptV1 {
    let source = include_bytes!("static_micro_op.rs");
    let mut hasher = Sha256::new();
    hasher.update(b"fn64:static-micro-op-packer-source:v1:");
    hasher.update((source.len() as u64).to_be_bytes());
    hasher.update(source);
    StaticMicroOpPackerSourceReceiptV1 {
        source_sha256: hasher.finalize().into(),
    }
}

/// Packer source identity co-bound to the runtime-owned record/decoder format
/// mapping that determines the encoded opcode and flags.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StaticMicroOpPackerSourceReceiptV2 {
    source_sha256: [u8; 32],
    format_source_sha256: [u8; 32],
}

impl StaticMicroOpPackerSourceReceiptV2 {
    pub const fn schema(self) -> &'static str {
        STATIC_MICRO_OP_PACKER_SOURCE_SCHEMA_V2
    }

    pub const fn source_sha256(self) -> [u8; 32] {
        self.source_sha256
    }

    pub const fn format_source_sha256(self) -> [u8; 32] {
        self.format_source_sha256
    }
}

pub fn static_micro_op_packer_source_receipt_v2() -> StaticMicroOpPackerSourceReceiptV2 {
    let format_source_sha256 = static_micro_op_format_source_receipt_v1().source_sha256();
    let source = include_bytes!("static_micro_op.rs");
    let mut hasher = Sha256::new();
    hasher.update(b"fn64:static-micro-op-packer-source:v2:");
    hasher.update((source.len() as u64).to_be_bytes());
    hasher.update(source);
    hasher.update(format_source_sha256);
    StaticMicroOpPackerSourceReceiptV2 {
        source_sha256: hasher.finalize().into(),
        format_source_sha256,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StaticMicroOpPackerSourceReceiptV3 {
    source_sha256: [u8; 32],
    format_source_sha256: [u8; 32],
}

impl StaticMicroOpPackerSourceReceiptV3 {
    pub const fn schema(self) -> &'static str {
        STATIC_MICRO_OP_PACKER_SOURCE_SCHEMA_V3
    }

    pub const fn source_sha256(self) -> [u8; 32] {
        self.source_sha256
    }

    pub const fn format_source_sha256(self) -> [u8; 32] {
        self.format_source_sha256
    }
}

pub fn static_micro_op_packer_source_receipt_v3() -> StaticMicroOpPackerSourceReceiptV3 {
    let format_source_sha256 = static_micro_op_format_source_receipt_v2().source_sha256();
    let source = include_bytes!("static_micro_op.rs");
    let mut hasher = Sha256::new();
    hasher.update(b"fn64:static-micro-op-packer-source:v3:");
    hasher.update((source.len() as u64).to_be_bytes());
    hasher.update(source);
    hasher.update(format_source_sha256);
    StaticMicroOpPackerSourceReceiptV3 {
        source_sha256: hasher.finalize().into(),
        format_source_sha256,
    }
}

/// One ordered executable span admitted to a static-micro-op artifact.
#[derive(Clone, Copy, Debug)]
pub struct StaticMicroOpSpanInput<'a> {
    pub bank: BankId,
    pub vram: u32,
    pub words: &'a [u32],
}

#[derive(Clone, Copy, Debug)]
pub struct StaticMicroOpSpanInputV2<'a> {
    pub bank: BankId,
    pub vram: u32,
    pub words: &'a [u32],
    /// One affine word needed only as the final owned control's delay. It is
    /// encoded and live-verified but is not an owned instruction or entry.
    pub delay_lookahead: Option<u32>,
}

/// Canonical encoded artifact plus its content-silent inventory.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StaticMicroOpPackV1 {
    bytes: Vec<u8>,
    span_count: u32,
    instruction_count: u64,
    body_sha256: [u8; 32],
}

impl StaticMicroOpPackV1 {
    pub const fn schema(&self) -> &'static str {
        STATIC_MICRO_OP_PACK_SCHEMA_V1
    }

    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    pub const fn span_count(&self) -> u32 {
        self.span_count
    }

    pub const fn instruction_count(&self) -> u64 {
        self.instruction_count
    }

    /// Digest of every canonical span header and encoded micro-op record.
    pub const fn body_sha256(&self) -> [u8; 32] {
        self.body_sha256
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StaticMicroOpPackV2 {
    bytes: Vec<u8>,
    span_count: u32,
    instruction_count: u64,
    body_sha256: [u8; 32],
}

impl StaticMicroOpPackV2 {
    pub const fn schema(&self) -> &'static str {
        fn64_recomp_rs::STATIC_MICRO_OP_PACK_SCHEMA_V2
    }

    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    pub const fn span_count(&self) -> u32 {
        self.span_count
    }

    pub const fn instruction_count(&self) -> u64 {
        self.instruction_count
    }

    pub const fn body_sha256(&self) -> [u8; 32] {
        self.body_sha256
    }
}

pub type StaticMicroOpPackError = StaticMicroOpPackErrorV1;

/// Pack ordered, disjoint bank-qualified spans into canonical v1 bytes.
pub fn pack_static_micro_ops_v1(
    spans: &[StaticMicroOpSpanInput<'_>],
) -> Result<StaticMicroOpPackV1, StaticMicroOpPackError> {
    let span_count =
        u32::try_from(spans.len()).map_err(|_| StaticMicroOpPackError::CountOverflow)?;
    let mut instruction_count = 0u64;
    let mut body_len = 0usize;
    let mut previous: Option<(BankId, u32, u32)> = None;

    for span in spans {
        validate_span(span, previous)?;
        let word_count =
            u32::try_from(span.words.len()).map_err(|_| StaticMicroOpPackError::CountOverflow)?;
        let byte_len =
            word_count
                .checked_mul(4)
                .ok_or(StaticMicroOpPackError::AddressOverflow {
                    bank: span.bank,
                    vram: span.vram,
                })?;
        let end =
            span.vram
                .checked_add(byte_len)
                .ok_or(StaticMicroOpPackError::AddressOverflow {
                    bank: span.bank,
                    vram: span.vram,
                })?;
        instruction_count = instruction_count
            .checked_add(u64::from(word_count))
            .ok_or(StaticMicroOpPackError::CountOverflow)?;
        body_len = body_len
            .checked_add(SPAN_HEADER_LEN)
            .and_then(|len| {
                len.checked_add(
                    span.words
                        .len()
                        .checked_mul(STATIC_MICRO_OP_RECORD_V1_BYTES)?,
                )
            })
            .ok_or(StaticMicroOpPackError::CountOverflow)?;
        previous = Some((span.bank, span.vram, end));
    }

    let mut body = Vec::with_capacity(body_len);
    for span in spans {
        body.extend_from_slice(&span.bank.get().to_be_bytes());
        body.extend_from_slice(&span.vram.to_be_bytes());
        body.extend_from_slice(&(span.words.len() as u32).to_be_bytes());
        for &word in span.words {
            body.extend_from_slice(&StaticMicroOpRecordV1::from_word(word).to_bytes());
        }
    }
    let body_sha256: [u8; 32] = Sha256::digest(&body).into();
    let mut bytes = Vec::with_capacity(HEADER_LEN + body.len());
    bytes.extend_from_slice(STATIC_MICRO_OP_MAGIC_V1);
    bytes.extend_from_slice(&span_count.to_be_bytes());
    bytes.extend_from_slice(&instruction_count.to_be_bytes());
    bytes.extend_from_slice(&body_sha256);
    bytes.extend_from_slice(&body);

    let artifact = StaticMicroOpPackV1 {
        bytes,
        span_count,
        instruction_count,
        body_sha256,
    };
    validate_static_micro_op_pack_v1(artifact.bytes())?;
    Ok(artifact)
}

/// Validate framing, canonical geometry, every record, counts, and digest.
pub fn validate_static_micro_op_pack_v1(
    bytes: &[u8],
) -> Result<StaticMicroOpPackV1, StaticMicroOpPackError> {
    let admitted = AdmittedStaticMicroOpProgramV1::from_bytes(bytes)?;
    Ok(StaticMicroOpPackV1 {
        bytes: bytes.to_vec(),
        span_count: admitted.span_count(),
        instruction_count: admitted.instruction_count(),
        body_sha256: admitted.body_sha256(),
    })
}

pub fn pack_static_micro_ops_v2(
    spans: &[StaticMicroOpSpanInputV2<'_>],
) -> Result<StaticMicroOpPackV2, StaticMicroOpPackError> {
    let span_count =
        u32::try_from(spans.len()).map_err(|_| StaticMicroOpPackError::CountOverflow)?;
    let mut instruction_count = 0u64;
    let mut body = Vec::new();
    let mut previous = None;
    for span in spans {
        let v1 = StaticMicroOpSpanInput {
            bank: span.bank,
            vram: span.vram,
            words: span.words,
        };
        validate_span(&v1, previous)?;
        let word_count =
            u32::try_from(span.words.len()).map_err(|_| StaticMicroOpPackError::CountOverflow)?;
        let end = span
            .vram
            .checked_add(word_count.checked_mul(4).ok_or(
                StaticMicroOpPackError::AddressOverflow {
                    bank: span.bank,
                    vram: span.vram,
                },
            )?)
            .ok_or(StaticMicroOpPackError::AddressOverflow {
                bank: span.bank,
                vram: span.vram,
            })?;
        instruction_count = instruction_count
            .checked_add(u64::from(word_count))
            .ok_or(StaticMicroOpPackError::CountOverflow)?;
        body.extend_from_slice(&span.bank.get().to_be_bytes());
        body.extend_from_slice(&span.vram.to_be_bytes());
        body.extend_from_slice(&word_count.to_be_bytes());
        body.push(u8::from(span.delay_lookahead.is_some()));
        for &word in span.words {
            body.extend_from_slice(&StaticMicroOpRecordV1::from_word(word).to_bytes());
        }
        if let Some(word) = span.delay_lookahead {
            body.extend_from_slice(&StaticMicroOpRecordV1::from_word(word).to_bytes());
        }
        previous = Some((span.bank, span.vram, end));
    }
    let body_sha256: [u8; 32] = Sha256::digest(&body).into();
    let mut bytes = Vec::with_capacity(HEADER_LEN + body.len());
    bytes.extend_from_slice(STATIC_MICRO_OP_MAGIC_V2);
    bytes.extend_from_slice(&span_count.to_be_bytes());
    bytes.extend_from_slice(&instruction_count.to_be_bytes());
    bytes.extend_from_slice(&body_sha256);
    bytes.extend_from_slice(&body);
    validate_static_micro_op_pack_v2(&bytes)
}

pub fn validate_static_micro_op_pack_v2(
    bytes: &[u8],
) -> Result<StaticMicroOpPackV2, StaticMicroOpPackError> {
    let admitted = AdmittedStaticMicroOpProgramV2::from_bytes(bytes)?;
    Ok(StaticMicroOpPackV2 {
        bytes: bytes.to_vec(),
        span_count: admitted.span_count(),
        instruction_count: admitted.instruction_count(),
        body_sha256: admitted.body_sha256(),
    })
}

fn validate_span(
    span: &StaticMicroOpSpanInput<'_>,
    previous: Option<(BankId, u32, u32)>,
) -> Result<(), StaticMicroOpPackError> {
    if span.words.is_empty() {
        return Err(StaticMicroOpPackError::EmptySpan {
            bank: span.bank,
            vram: span.vram,
        });
    }
    let word_count =
        u32::try_from(span.words.len()).map_err(|_| StaticMicroOpPackError::CountOverflow)?;
    let end =
        span.vram
            .checked_add(word_count.checked_mul(4).ok_or(
                StaticMicroOpPackError::AddressOverflow {
                    bank: span.bank,
                    vram: span.vram,
                },
            )?)
            .ok_or(StaticMicroOpPackError::AddressOverflow {
                bank: span.bank,
                vram: span.vram,
            })?;
    validate_geometry(span.bank, span.vram, end, previous)
}

fn validate_geometry(
    bank: BankId,
    vram: u32,
    end: u32,
    previous: Option<(BankId, u32, u32)>,
) -> Result<(), StaticMicroOpPackError> {
    if vram & 3 != 0 {
        return Err(StaticMicroOpPackError::UnalignedStart { bank, vram });
    }
    if let Some((previous_bank, previous_vram, previous_end)) = previous {
        if (bank, vram) < (previous_bank, previous_vram) {
            return Err(StaticMicroOpPackError::OutOfOrder {
                previous_bank,
                previous_vram,
                bank,
                vram,
            });
        }
        if bank == previous_bank && vram < previous_end {
            return Err(StaticMicroOpPackError::Overlap {
                bank,
                previous_end,
                vram,
            });
        }
    }
    debug_assert!(end >= vram);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const BANK_A: BankId = BankId::new(1);
    const BANK_B: BankId = BankId::new(2);

    fn inputs<'a>(a: &'a [u32], b: &'a [u32]) -> [StaticMicroOpSpanInput<'a>; 2] {
        [
            StaticMicroOpSpanInput {
                bank: BANK_A,
                vram: 0x8000_0000,
                words: a,
            },
            StaticMicroOpSpanInput {
                bank: BANK_B,
                vram: 0x8000_0000,
                words: b,
            },
        ]
    }

    #[test]
    fn pack_is_deterministic_and_round_trips_counts_and_digest() {
        let spans = inputs(&[0, 0x2402_0001], &[0x03E0_0008, 0]);
        let first = pack_static_micro_ops_v1(&spans).unwrap();
        let second = pack_static_micro_ops_v1(&spans).unwrap();
        assert_eq!(first, second);
        assert_eq!(first.span_count(), 2);
        assert_eq!(first.instruction_count(), 4);
        assert_eq!(first.schema(), STATIC_MICRO_OP_PACK_SCHEMA_V1);
        assert_ne!(first.body_sha256(), [0; 32]);
        assert_eq!(
            validate_static_micro_op_pack_v1(first.bytes()).unwrap(),
            first
        );
    }

    #[test]
    fn packer_source_receipt_is_separate_and_nonzero() {
        let receipt = static_micro_op_packer_source_receipt_v1();
        assert_eq!(receipt.schema(), STATIC_MICRO_OP_PACKER_SOURCE_SCHEMA_V1);
        assert_ne!(receipt.source_sha256(), [0; 32]);
        assert_eq!(receipt, static_micro_op_packer_source_receipt_v1());

        let bound = static_micro_op_packer_source_receipt_v2();
        assert_eq!(bound.schema(), STATIC_MICRO_OP_PACKER_SOURCE_SCHEMA_V2);
        assert_ne!(bound.source_sha256(), [0; 32]);
        assert_eq!(
            bound.format_source_sha256(),
            static_micro_op_format_source_receipt_v1().source_sha256()
        );

        let v2_bound = static_micro_op_packer_source_receipt_v3();
        assert_eq!(v2_bound.schema(), STATIC_MICRO_OP_PACKER_SOURCE_SCHEMA_V3);
        assert_ne!(v2_bound.source_sha256(), [0; 32]);
        assert_eq!(
            v2_bound.format_source_sha256(),
            static_micro_op_format_source_receipt_v2().source_sha256()
        );
    }

    #[test]
    fn rejects_unaligned_out_of_order_overlap_and_overflow() {
        let word = [0u32];
        for (spans, expected) in [
            (
                vec![StaticMicroOpSpanInput {
                    bank: BANK_A,
                    vram: 2,
                    words: &word,
                }],
                StaticMicroOpPackError::UnalignedStart {
                    bank: BANK_A,
                    vram: 2,
                },
            ),
            (
                vec![
                    StaticMicroOpSpanInput {
                        bank: BANK_B,
                        vram: 0,
                        words: &word,
                    },
                    StaticMicroOpSpanInput {
                        bank: BANK_A,
                        vram: 0,
                        words: &word,
                    },
                ],
                StaticMicroOpPackError::OutOfOrder {
                    previous_bank: BANK_B,
                    previous_vram: 0,
                    bank: BANK_A,
                    vram: 0,
                },
            ),
            (
                vec![
                    StaticMicroOpSpanInput {
                        bank: BANK_A,
                        vram: 0,
                        words: &word,
                    },
                    StaticMicroOpSpanInput {
                        bank: BANK_A,
                        vram: 0,
                        words: &word,
                    },
                ],
                StaticMicroOpPackError::Overlap {
                    bank: BANK_A,
                    previous_end: 4,
                    vram: 0,
                },
            ),
            (
                vec![StaticMicroOpSpanInput {
                    bank: BANK_A,
                    vram: u32::MAX - 3,
                    words: &word,
                }],
                StaticMicroOpPackError::AddressOverflow {
                    bank: BANK_A,
                    vram: u32::MAX - 3,
                },
            ),
        ] {
            assert_eq!(pack_static_micro_ops_v1(&spans).unwrap_err(), expected);
        }
    }

    #[test]
    fn runtime_admission_rejects_missing_delay_but_preserves_control_shaped_words() {
        let missing = [0x03e0_0008];
        assert_eq!(
            pack_static_micro_ops_v1(&[StaticMicroOpSpanInput {
                bank: BANK_A,
                vram: 0x8000_1000,
                words: &missing,
            }])
            .unwrap_err(),
            StaticMicroOpPackError::MissingDelaySlot {
                bank: BANK_A,
                pc: 0x8000_1000,
            }
        );

        let nested = [0x1000_0001, 0x1000_0000, 0];
        let packed = pack_static_micro_ops_v1(&[StaticMicroOpSpanInput {
            bank: BANK_A,
            vram: 0x8000_1000,
            words: &nested,
        }])
        .expect("control-shaped words remain valid arbitrary entries");
        assert_eq!(packed.instruction_count(), 3);
    }

    #[test]
    fn v2_lookahead_is_canonical_and_not_an_owned_instruction() {
        let final_branch = [0x1000_0000];
        let packed = pack_static_micro_ops_v2(&[StaticMicroOpSpanInputV2 {
            bank: BANK_A,
            vram: 0x8000_1000,
            words: &final_branch,
            delay_lookahead: Some(0),
        }])
        .unwrap();
        assert_eq!(
            packed.schema(),
            fn64_recomp_rs::STATIC_MICRO_OP_PACK_SCHEMA_V2
        );
        assert_eq!(packed.instruction_count(), 1);
        assert_eq!(
            validate_static_micro_op_pack_v2(packed.bytes()).unwrap(),
            packed
        );

        assert_eq!(
            pack_static_micro_ops_v2(&[StaticMicroOpSpanInputV2 {
                bank: BANK_A,
                vram: 0x8000_1000,
                words: &[0],
                delay_lookahead: Some(0),
            }])
            .unwrap_err(),
            StaticMicroOpPackError::UnexpectedDelayLookahead {
                bank: BANK_A,
                pc: 0x8000_1000,
            }
        );

        let mut invalid_tag = packed.bytes().to_vec();
        invalid_tag[HEADER_LEN + 16] = 2;
        let digest: [u8; 32] = Sha256::digest(&invalid_tag[HEADER_LEN..]).into();
        invalid_tag[20..52].copy_from_slice(&digest);
        assert_eq!(
            validate_static_micro_op_pack_v2(&invalid_tag).unwrap_err(),
            StaticMicroOpPackError::InvalidLookaheadTag {
                span_index: 0,
                actual: 2,
            }
        );
    }

    #[test]
    fn validator_rejects_truncation_trailing_bytes_and_corruption() {
        let spans = inputs(&[0], &[1]);
        let packed = pack_static_micro_ops_v1(&spans).unwrap();
        assert_eq!(
            validate_static_micro_op_pack_v1(&packed.bytes()[..HEADER_LEN - 1]).unwrap_err(),
            StaticMicroOpPackError::Truncated
        );

        let mut trailing = packed.bytes().to_vec();
        trailing.push(0);
        assert_eq!(
            validate_static_micro_op_pack_v1(&trailing).unwrap_err(),
            StaticMicroOpPackError::DigestMismatch
        );

        let mut corrupt = packed.bytes().to_vec();
        corrupt[HEADER_LEN + SPAN_HEADER_LEN] ^= 0x80;
        assert_eq!(
            validate_static_micro_op_pack_v1(&corrupt).unwrap_err(),
            StaticMicroOpPackError::DigestMismatch
        );
    }

    #[test]
    fn valid_digest_does_not_hide_trailing_or_invalid_record_bytes() {
        let spans = inputs(&[0], &[1]);
        let packed = pack_static_micro_ops_v1(&spans).unwrap();

        let mut trailing = packed.bytes().to_vec();
        trailing.push(0);
        let digest: [u8; 32] = Sha256::digest(&trailing[HEADER_LEN..]).into();
        trailing[20..52].copy_from_slice(&digest);
        assert_eq!(
            validate_static_micro_op_pack_v1(&trailing).unwrap_err(),
            StaticMicroOpPackError::TrailingBytes
        );

        let mut invalid = packed.bytes().to_vec();
        invalid[HEADER_LEN + SPAN_HEADER_LEN] = 0xFF;
        let digest: [u8; 32] = Sha256::digest(&invalid[HEADER_LEN..]).into();
        invalid[20..52].copy_from_slice(&digest);
        assert!(matches!(
            validate_static_micro_op_pack_v1(&invalid),
            Err(StaticMicroOpPackError::InvalidRecord {
                span_index: 0,
                word_index: 0,
                ..
            })
        ));

        let mut truncated = packed.bytes()[..packed.bytes().len() - 1].to_vec();
        let digest: [u8; 32] = Sha256::digest(&truncated[HEADER_LEN..]).into();
        truncated[20..52].copy_from_slice(&digest);
        assert_eq!(
            validate_static_micro_op_pack_v1(&truncated).unwrap_err(),
            StaticMicroOpPackError::Truncated
        );

        let mut wrong_count = packed.bytes().to_vec();
        wrong_count[12..20].copy_from_slice(&3u64.to_be_bytes());
        assert_eq!(
            validate_static_micro_op_pack_v1(&wrong_count).unwrap_err(),
            StaticMicroOpPackError::CountMismatch {
                header: 3,
                observed: 2,
            }
        );
    }
}
