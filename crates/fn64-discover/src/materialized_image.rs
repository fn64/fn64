//! Candidate-only re-derivation of typed evaluated-image receipts.
//!
//! Sources are re-read through the normalized-ROM materializer. Physical
//! sources therefore come from the normalized ROM itself, while virtual-ROM
//! sources require exactly one proven file backing in the supplied fact
//! database. The result deliberately has no serialization or conversion into
//! a fact, bank backing, conclusion, or authority capability.

use crate::banks::materialize_rom_range_bounded;
use crate::facts::{
    evaluated_image_receipt_sha256_v1, BankBackingSpanV1, EvaluatedImageReceiptV1, Fact, FactDb,
    MaterializationEvaluatorV1, MaterializedByteRangeV1, MaterializedImageSourceV1,
    MaterializedImageStreamV1, MaterializedImageSuffixV1, ProofState, RomAddressSpace,
};
use crate::headered_raw_deflate::{
    materialize_headered_raw_deflate_sequence, HeaderedRawDeflateError, HeaderedRawDeflateLimits,
};
use crate::NormalizedRom;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};

const HARD_MAX_BYTES: usize = 64 * 1024 * 1024;
const HARD_MAX_STREAMS: usize = 4096;

/// Caller-selected resource envelope, bounded again by fixed hard ceilings.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MaterializedImageLimitsV1 {
    pub max_source_bytes: usize,
    pub max_decoded_vrom_file_bytes: usize,
    pub max_stream_output_bytes: usize,
    pub max_aggregate_output_bytes: usize,
    pub max_streams: usize,
}

impl Default for MaterializedImageLimitsV1 {
    fn default() -> Self {
        Self {
            max_source_bytes: HARD_MAX_BYTES,
            max_decoded_vrom_file_bytes: HARD_MAX_BYTES,
            max_stream_output_bytes: HARD_MAX_BYTES,
            max_aggregate_output_bytes: HARD_MAX_BYTES,
            max_streams: HARD_MAX_STREAMS,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MaterializedImageLimitKindV1 {
    SourceBytes,
    DecodedVromFileBytes,
    StreamOutputBytes,
    AggregateOutputBytes,
    Streams,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MaterializedImageErrorV1 {
    ZeroLimit {
        kind: MaterializedImageLimitKindV1,
    },
    HardLimitExceeded {
        kind: MaterializedImageLimitKindV1,
        value: usize,
        hard_limit: usize,
    },
    EmptyOrInvertedSource {
        start: u32,
        end: u32,
    },
    SourceCursorOutside {
        cursor: u32,
        source_len: u32,
    },
    SourceExtentLimitExceeded {
        bytes: usize,
        limit: usize,
    },
    SourceMaterialization {
        reason: String,
    },
    SourceLengthMismatch {
        expected: usize,
        actual: usize,
    },
    StreamCountConversion {
        stream_count: u32,
    },
    Decoder(HeaderedRawDeflateError),
    FieldExceedsU32 {
        field: &'static str,
        value: usize,
    },
    ReceiptMismatch {
        expected_sha256: String,
        actual_sha256: String,
    },
}

impl From<HeaderedRawDeflateError> for MaterializedImageErrorV1 {
    fn from(error: HeaderedRawDeflateError) -> Self {
        Self::Decoder(error)
    }
}

impl std::fmt::Display for MaterializedImageErrorV1 {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ZeroLimit { kind } => write!(formatter, "{kind:?} limit must be nonzero"),
            Self::HardLimitExceeded {
                kind,
                value,
                hard_limit,
            } => write!(
                formatter,
                "{kind:?} limit {value} exceeds hard ceiling {hard_limit}"
            ),
            Self::EmptyOrInvertedSource { start, end } => write!(
                formatter,
                "materialized-image source [0x{start:x},0x{end:x}) is empty or inverted"
            ),
            Self::SourceCursorOutside { cursor, source_len } => write!(
                formatter,
                "source cursor 0x{cursor:x} exceeds source length 0x{source_len:x}"
            ),
            Self::SourceExtentLimitExceeded { bytes, limit } => write!(
                formatter,
                "source extent {bytes} exceeds caller limit {limit}"
            ),
            Self::SourceMaterialization { reason } => {
                write!(formatter, "source materialization failed: {reason}")
            }
            Self::SourceLengthMismatch { expected, actual } => write!(
                formatter,
                "source materialization returned {actual} bytes; expected {expected}"
            ),
            Self::StreamCountConversion { stream_count } => write!(
                formatter,
                "stream count {stream_count} cannot be represented on this platform"
            ),
            Self::Decoder(error) => write!(formatter, "raw-DEFLATE evaluator failed: {error:?}"),
            Self::FieldExceedsU32 { field, value } => {
                write!(formatter, "{field} value {value} exceeds u32")
            }
            Self::ReceiptMismatch {
                expected_sha256,
                actual_sha256,
            } => write!(
                formatter,
                "re-derived receipt {actual_sha256} does not match expected {expected_sha256}"
            ),
        }
    }
}

impl std::error::Error for MaterializedImageErrorV1 {}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MaterializedBackingFactsRequirementV1 {
    VirtualRom,
    EvaluatedImage,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MaterializedBackingSpanErrorV1 {
    InvalidGeometry,
    SpanLimitExceeded {
        bytes: usize,
        limit: usize,
    },
    FactsRequired {
        requirement: MaterializedBackingFactsRequirementV1,
    },
    RomMaterialization {
        rom_space: RomAddressSpace,
        rom_start: u32,
        rom_end: u32,
        reason: String,
    },
    MissingEvaluatedImageReceipt {
        receipt_sha256: String,
    },
    AmbiguousEvaluatedImageReceipt {
        receipt_sha256: String,
        count: usize,
    },
    EvaluatedImageRederivation {
        receipt_sha256: String,
        error: MaterializedImageErrorV1,
    },
}

/// Operation-local cache of fully re-derived evaluated-image outputs.
///
/// The cache is deliberately neither serializable nor authoritative. Every
/// span lookup still proves the exact bank/VA/receipt association from the
/// supplied [`FactDb`]; caching only avoids repeating the deterministic
/// evaluator for another span of the same receipt during one operation.
#[derive(Debug, Default)]
pub struct MaterializedBackingSpanCacheV1 {
    evaluated_outputs: BTreeMap<String, Vec<u8>>,
}

/// Re-derived candidate bytes and their content-only receipt.
///
/// This type intentionally does not implement `Serialize` and exposes no
/// conversion to discovery authority.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MaterializedImageEvaluationV1 {
    receipt: EvaluatedImageReceiptV1,
    bytes: Vec<u8>,
    source_backing_evidence: Vec<usize>,
}

impl MaterializedImageEvaluationV1 {
    pub fn receipt(&self) -> &EvaluatedImageReceiptV1 {
        &self.receipt
    }

    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    /// Fact indices required to re-read a virtual-ROM source. Physical-ROM
    /// sources have no backing facts and therefore return an empty slice.
    pub fn source_backing_evidence(&self) -> &[usize] {
        &self.source_backing_evidence
    }
}

/// Evaluate one explicit source/evaluator recipe without assigning placement
/// or proof state.
pub fn evaluate_materialized_image_v1(
    rom: &NormalizedRom,
    facts: &FactDb,
    source: &MaterializedImageSourceV1,
    evaluator: &MaterializationEvaluatorV1,
    limits: MaterializedImageLimitsV1,
) -> Result<MaterializedImageEvaluationV1, MaterializedImageErrorV1> {
    validate_limits(limits)?;
    let source_len_u32 = source.rom_end.checked_sub(source.rom_start).ok_or(
        MaterializedImageErrorV1::EmptyOrInvertedSource {
            start: source.rom_start,
            end: source.rom_end,
        },
    )?;
    if source_len_u32 == 0 {
        return Err(MaterializedImageErrorV1::EmptyOrInvertedSource {
            start: source.rom_start,
            end: source.rom_end,
        });
    }
    if source.cursor > source_len_u32 {
        return Err(MaterializedImageErrorV1::SourceCursorOutside {
            cursor: source.cursor,
            source_len: source_len_u32,
        });
    }
    let source_len =
        usize::try_from(source_len_u32).map_err(|_| MaterializedImageErrorV1::FieldExceedsU32 {
            field: "source extent",
            value: usize::MAX,
        })?;
    if source_len > limits.max_source_bytes {
        return Err(MaterializedImageErrorV1::SourceExtentLimitExceeded {
            bytes: source_len,
            limit: limits.max_source_bytes,
        });
    }
    let materialized = materialize_rom_range_bounded(
        rom,
        facts,
        source.rom_space,
        source.rom_start,
        source.rom_end,
        limits.max_decoded_vrom_file_bytes,
    )
    .map_err(|reason| MaterializedImageErrorV1::SourceMaterialization { reason })?;
    if materialized.bytes.len() != source_len {
        return Err(MaterializedImageErrorV1::SourceLengthMismatch {
            expected: source_len,
            actual: materialized.bytes.len(),
        });
    }
    let source_cursor =
        usize::try_from(source.cursor).map_err(|_| MaterializedImageErrorV1::FieldExceedsU32 {
            field: "source cursor",
            value: usize::MAX,
        })?;

    let sequence = match evaluator {
        MaterializationEvaluatorV1::HeaderedRawDeflateSequenceV1 { stream_count } => {
            let stream_count = usize::try_from(*stream_count).map_err(|_| {
                MaterializedImageErrorV1::StreamCountConversion {
                    stream_count: *stream_count,
                }
            })?;
            materialize_headered_raw_deflate_sequence(
                &materialized.bytes,
                source_cursor,
                stream_count,
                HeaderedRawDeflateLimits {
                    max_input_bytes: limits.max_source_bytes,
                    max_stream_output_bytes: limits.max_stream_output_bytes,
                    max_aggregate_output_bytes: limits.max_aggregate_output_bytes,
                    max_streams: limits.max_streams,
                },
            )?
        }
    };

    let streams = sequence
        .streams
        .into_iter()
        .map(|stream| {
            Ok(MaterializedImageStreamV1 {
                source_range: narrow_range("stream source range", stream.source_range)?,
                encoded_range: narrow_range("stream encoded range", stream.deflate_range)?,
                output_range: narrow_range("stream output range", stream.output_range)?,
                declared_output_len: narrow("declared output length", stream.declared_output_len)?,
                source_sha256: stream.source_sha256,
                output_sha256: stream.output_sha256,
            })
        })
        .collect::<Result<Vec<_>, MaterializedImageErrorV1>>()?;
    let trailing_suffix = MaterializedImageSuffixV1 {
        offset: narrow("trailing suffix offset", sequence.trailing_suffix.offset)?,
        len: narrow("trailing suffix length", sequence.trailing_suffix.len)?,
        sha256: sequence.trailing_suffix.sha256,
    };
    let output_len = narrow("aggregate output length", sequence.bytes.len())?;
    let receipt = EvaluatedImageReceiptV1 {
        evaluator: evaluator.clone(),
        source: source.clone(),
        source_sha256: sha256(&materialized.bytes),
        output_len,
        output_sha256: sha256(&sequence.bytes),
        streams,
        trailing_suffix,
    };
    Ok(MaterializedImageEvaluationV1 {
        receipt,
        bytes: sequence.bytes,
        source_backing_evidence: materialized.backing_evidence,
    })
}

/// Re-evaluate an expected receipt from normalized ROM and FactDb inputs and
/// require every typed field to agree exactly.
pub fn rederive_materialized_image_v1(
    rom: &NormalizedRom,
    facts: &FactDb,
    expected: &EvaluatedImageReceiptV1,
    limits: MaterializedImageLimitsV1,
) -> Result<MaterializedImageEvaluationV1, MaterializedImageErrorV1> {
    let actual =
        evaluate_materialized_image_v1(rom, facts, &expected.source, &expected.evaluator, limits)?;
    if actual.receipt != *expected {
        return Err(MaterializedImageErrorV1::ReceiptMismatch {
            expected_sha256: crate::facts::evaluated_image_receipt_sha256_v1(expected),
            actual_sha256: crate::facts::evaluated_image_receipt_sha256_v1(&actual.receipt),
        });
    }
    Ok(actual)
}

/// Reconstruct one typed bank subspan without changing proof state.
///
/// Physical spans slice the normalized ROM directly. Virtual spans resolve
/// through proven VROM file records. Evaluated spans require one exact receipt
/// under a proven bank conclusion, re-derive its complete output, cache that
/// output by receipt identity, and only then return the checked subspan.
#[allow(clippy::too_many_arguments)]
pub fn materialize_backing_span_v1(
    rom: &NormalizedRom,
    facts: Option<&FactDb>,
    bank: &str,
    va_start: u32,
    va_end: u32,
    backing: &BankBackingSpanV1,
    limits: MaterializedImageLimitsV1,
    cache: &mut MaterializedBackingSpanCacheV1,
) -> Result<Vec<u8>, MaterializedBackingSpanErrorV1> {
    let va_len = va_end
        .checked_sub(va_start)
        .filter(|length| *length != 0)
        .ok_or(MaterializedBackingSpanErrorV1::InvalidGeometry)?;
    let backing_len = match backing {
        BankBackingSpanV1::RomAffine {
            rom_start, rom_end, ..
        } => rom_end.checked_sub(*rom_start),
        BankBackingSpanV1::Materialized {
            output_start,
            output_end,
            ..
        } => output_end.checked_sub(*output_start),
    }
    .filter(|length| *length == va_len)
    .ok_or(MaterializedBackingSpanErrorV1::InvalidGeometry)?;
    let span_bytes = usize::try_from(backing_len)
        .map_err(|_| MaterializedBackingSpanErrorV1::InvalidGeometry)?;
    if span_bytes > limits.max_aggregate_output_bytes {
        return Err(MaterializedBackingSpanErrorV1::SpanLimitExceeded {
            bytes: span_bytes,
            limit: limits.max_aggregate_output_bytes,
        });
    }

    match backing {
        BankBackingSpanV1::RomAffine {
            rom_space: RomAddressSpace::Physical,
            rom_start,
            rom_end,
        } => {
            let start = usize::try_from(*rom_start)
                .map_err(|_| MaterializedBackingSpanErrorV1::InvalidGeometry)?;
            let end = usize::try_from(*rom_end)
                .map_err(|_| MaterializedBackingSpanErrorV1::InvalidGeometry)?;
            rom.bytes
                .get(start..end)
                .map(<[u8]>::to_vec)
                .ok_or_else(|| MaterializedBackingSpanErrorV1::RomMaterialization {
                    rom_space: RomAddressSpace::Physical,
                    rom_start: *rom_start,
                    rom_end: *rom_end,
                    reason: "physical ROM span is outside the normalized image".to_owned(),
                })
        }
        BankBackingSpanV1::RomAffine {
            rom_space: RomAddressSpace::Virtual,
            rom_start,
            rom_end,
        } => {
            let facts = facts.ok_or(MaterializedBackingSpanErrorV1::FactsRequired {
                requirement: MaterializedBackingFactsRequirementV1::VirtualRom,
            })?;
            materialize_rom_range_bounded(
                rom,
                facts,
                RomAddressSpace::Virtual,
                *rom_start,
                *rom_end,
                limits.max_decoded_vrom_file_bytes,
            )
            .map(|materialized| materialized.bytes)
            .map_err(
                |reason| MaterializedBackingSpanErrorV1::RomMaterialization {
                    rom_space: RomAddressSpace::Virtual,
                    rom_start: *rom_start,
                    rom_end: *rom_end,
                    reason,
                },
            )
        }
        BankBackingSpanV1::Materialized {
            receipt_sha256,
            output_start,
            output_end,
        } => {
            let facts = facts.ok_or(MaterializedBackingSpanErrorV1::FactsRequired {
                requirement: MaterializedBackingFactsRequirementV1::EvaluatedImage,
            })?;
            let proven_conclusion = facts
                .conclusion(&format!("bank:{bank}"))
                .filter(|conclusion| conclusion.state == ProofState::Proven);
            let receipts = facts
                .facts()
                .iter()
                .enumerate()
                .filter_map(|(index, fact)| {
                    let Fact::EvaluatedImage {
                        bank: fact_bank,
                        va_start: image_va_start,
                        va_end: image_va_end,
                        receipt,
                    } = fact
                    else {
                        return None;
                    };
                    let image_len = image_va_end.checked_sub(*image_va_start)?;
                    (proven_conclusion
                        .is_some_and(|conclusion| conclusion.justified_by.contains(&index))
                        && fact_bank == bank
                        && receipt.output_len == image_len
                        && evaluated_image_receipt_sha256_v1(receipt) == *receipt_sha256
                        && image_va_start.checked_add(*output_start) == Some(va_start)
                        && image_va_start.checked_add(*output_end) == Some(va_end))
                    .then_some(receipt.clone())
                })
                .collect::<BTreeSet<_>>();
            let receipt = match receipts.len() {
                0 => {
                    return Err(
                        MaterializedBackingSpanErrorV1::MissingEvaluatedImageReceipt {
                            receipt_sha256: receipt_sha256.clone(),
                        },
                    );
                }
                1 => receipts.into_iter().next().expect("one receipt exists"),
                count => {
                    return Err(
                        MaterializedBackingSpanErrorV1::AmbiguousEvaluatedImageReceipt {
                            receipt_sha256: receipt_sha256.clone(),
                            count,
                        },
                    );
                }
            };
            if !cache.evaluated_outputs.contains_key(receipt_sha256) {
                let evaluation = rederive_materialized_image_v1(rom, facts, &receipt, limits)
                    .map_err(|error| {
                        MaterializedBackingSpanErrorV1::EvaluatedImageRederivation {
                            receipt_sha256: receipt_sha256.clone(),
                            error,
                        }
                    })?;
                cache
                    .evaluated_outputs
                    .insert(receipt_sha256.clone(), evaluation.bytes().to_vec());
            }
            let start = usize::try_from(*output_start)
                .map_err(|_| MaterializedBackingSpanErrorV1::InvalidGeometry)?;
            let end = usize::try_from(*output_end)
                .map_err(|_| MaterializedBackingSpanErrorV1::InvalidGeometry)?;
            cache
                .evaluated_outputs
                .get(receipt_sha256)
                .expect("receipt output was cached after exact re-derivation")
                .get(start..end)
                .map(<[u8]>::to_vec)
                .ok_or(MaterializedBackingSpanErrorV1::InvalidGeometry)
        }
    }
}

fn validate_limits(limits: MaterializedImageLimitsV1) -> Result<(), MaterializedImageErrorV1> {
    for (kind, value, hard_limit) in [
        (
            MaterializedImageLimitKindV1::SourceBytes,
            limits.max_source_bytes,
            HARD_MAX_BYTES,
        ),
        (
            MaterializedImageLimitKindV1::DecodedVromFileBytes,
            limits.max_decoded_vrom_file_bytes,
            HARD_MAX_BYTES,
        ),
        (
            MaterializedImageLimitKindV1::StreamOutputBytes,
            limits.max_stream_output_bytes,
            HARD_MAX_BYTES,
        ),
        (
            MaterializedImageLimitKindV1::AggregateOutputBytes,
            limits.max_aggregate_output_bytes,
            HARD_MAX_BYTES,
        ),
        (
            MaterializedImageLimitKindV1::Streams,
            limits.max_streams,
            HARD_MAX_STREAMS,
        ),
    ] {
        if value == 0 {
            return Err(MaterializedImageErrorV1::ZeroLimit { kind });
        }
        if value > hard_limit {
            return Err(MaterializedImageErrorV1::HardLimitExceeded {
                kind,
                value,
                hard_limit,
            });
        }
    }
    Ok(())
}

fn narrow(field: &'static str, value: usize) -> Result<u32, MaterializedImageErrorV1> {
    u32::try_from(value).map_err(|_| MaterializedImageErrorV1::FieldExceedsU32 { field, value })
}

fn narrow_range(
    field: &'static str,
    range: crate::headered_raw_deflate::RelativeByteRange,
) -> Result<MaterializedByteRangeV1, MaterializedImageErrorV1> {
    Ok(MaterializedByteRangeV1 {
        start: narrow(field, range.start)?,
        end: narrow(field, range.end)?,
    })
}

fn sha256(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::facts::{
        load_image_table_record_subject, Fact, MappingAddressSpace, ProofState, RomAddressSpace,
    };
    use crate::normalize;
    use flate2::{write::DeflateEncoder, Compression};
    use std::io::Write;

    fn stream(bytes: &[u8]) -> Vec<u8> {
        let mut encoder = DeflateEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(bytes).unwrap();
        let compressed = encoder.finish().unwrap();
        let mut result = Vec::with_capacity(6 + compressed.len());
        result.extend_from_slice(&[0x11, 0x72]);
        result.extend_from_slice(&(bytes.len() as u32).to_be_bytes());
        result.extend_from_slice(&compressed);
        result
    }

    fn synthetic_rom(payload: &[u8], offset: usize) -> NormalizedRom {
        let mut bytes = vec![0; (offset + payload.len() + 3) & !3];
        bytes[0..4].copy_from_slice(&0x8037_1240u32.to_be_bytes());
        bytes[8..12].copy_from_slice(&0x8000_0400u32.to_be_bytes());
        bytes[offset..offset + payload.len()].copy_from_slice(payload);
        normalize(&bytes).unwrap()
    }

    fn limits() -> MaterializedImageLimitsV1 {
        MaterializedImageLimitsV1 {
            max_source_bytes: 4096,
            max_decoded_vrom_file_bytes: 4096,
            max_stream_output_bytes: 1024,
            max_aggregate_output_bytes: 2048,
            max_streams: 4,
        }
    }

    #[test]
    fn physical_source_is_deterministic_and_receipt_serializes_no_bytes() {
        let first_output = b"first synthetic output";
        let second_output = b"second synthetic output";
        let output = [first_output.as_slice(), second_output.as_slice()].concat();
        let prefix = [0xaa, 0xbb];
        let suffix = [0xde, 0xad, 0xbe, 0xef];
        let first_encoded = stream(first_output);
        let second_encoded = stream(second_output);
        let source_bytes = [
            prefix.as_slice(),
            first_encoded.as_slice(),
            second_encoded.as_slice(),
            suffix.as_slice(),
        ]
        .concat();
        let rom = synthetic_rom(&source_bytes, 0x80);
        let source = MaterializedImageSourceV1 {
            rom_space: RomAddressSpace::Physical,
            rom_start: 0x80,
            rom_end: 0x80 + source_bytes.len() as u32,
            cursor: prefix.len() as u32,
        };
        let evaluator =
            MaterializationEvaluatorV1::HeaderedRawDeflateSequenceV1 { stream_count: 2 };

        let first =
            evaluate_materialized_image_v1(&rom, &FactDb::new(), &source, &evaluator, limits())
                .unwrap();
        let second =
            evaluate_materialized_image_v1(&rom, &FactDb::new(), &source, &evaluator, limits())
                .unwrap();
        assert_eq!(first, second);
        assert_eq!(first.bytes(), output);
        assert_eq!(first.receipt().source_sha256, sha256(&source_bytes));
        assert_eq!(first.receipt().output_sha256, sha256(&output));
        assert_eq!(first.receipt().streams.len(), 2);
        assert_eq!(first.receipt().streams[0].source_range.start, 2);
        assert_eq!(
            first.receipt().streams[1].source_range.start,
            2 + first_encoded.len() as u32
        );
        assert_eq!(
            first.receipt().streams[1].output_range.start,
            first_output.len() as u32
        );
        assert_eq!(first.receipt().trailing_suffix.len, suffix.len() as u32);
        assert!(first.source_backing_evidence().is_empty());
        let wire = serde_json::to_string(first.receipt()).unwrap();
        assert!(!wire.contains("bytes"));
        assert!(!wire.contains("synthetic candidate output"));
        assert_eq!(
            rederive_materialized_image_v1(&rom, &FactDb::new(), first.receipt(), limits())
                .unwrap(),
            first
        );
    }

    #[test]
    fn virtual_source_is_re_read_through_proven_fact_backing() {
        let output = b"virtual source output";
        let encoded = stream(output);
        let rom = synthetic_rom(&encoded, 0x100);
        let mut facts = FactDb::new();
        let record = facts.insert(Fact::LoadImageTableRecord {
            table: "files".to_owned(),
            bank: None,
            table_space: RomAddressSpace::Physical,
            table_offset: 0x40,
            index: 0,
            source_space: MappingAddressSpace::VirtualRom,
            source_start: 0x2000,
            source_end: 0x2000 + encoded.len() as u32,
            destination_space: MappingAddressSpace::PhysicalRom,
            destination_start: 0x100,
            destination_end: 0x100 + encoded.len() as u32,
        });
        facts
            .conclude(
                load_image_table_record_subject("files", 0),
                ProofState::Proven,
                vec![record],
                "synthetic file backing",
            )
            .unwrap();
        let source = MaterializedImageSourceV1 {
            rom_space: RomAddressSpace::Virtual,
            rom_start: 0x2000,
            rom_end: 0x2000 + encoded.len() as u32,
            cursor: 0,
        };
        let result = evaluate_materialized_image_v1(
            &rom,
            &facts,
            &source,
            &MaterializationEvaluatorV1::HeaderedRawDeflateSequenceV1 { stream_count: 1 },
            limits(),
        )
        .unwrap();
        assert_eq!(result.bytes(), output);
        assert_eq!(result.source_backing_evidence(), &[record]);

        let missing = FactDb::new();
        assert!(matches!(
            evaluate_materialized_image_v1(
                &rom,
                &missing,
                &source,
                &MaterializationEvaluatorV1::HeaderedRawDeflateSequenceV1 { stream_count: 1 },
                limits(),
            ),
            Err(MaterializedImageErrorV1::SourceMaterialization { .. })
        ));
        assert!(missing.facts().is_empty());
    }

    #[test]
    fn backing_span_rejects_an_uncited_same_bank_evaluated_image() {
        let output = b"uncited output";
        let encoded = stream(output);
        let rom = synthetic_rom(&encoded, 0x80);
        let source = MaterializedImageSourceV1 {
            rom_space: RomAddressSpace::Physical,
            rom_start: 0x80,
            rom_end: 0x80 + encoded.len() as u32,
            cursor: 0,
        };
        let evaluation = evaluate_materialized_image_v1(
            &rom,
            &FactDb::new(),
            &source,
            &MaterializationEvaluatorV1::HeaderedRawDeflateSequenceV1 { stream_count: 1 },
            limits(),
        )
        .unwrap();
        let mut facts = FactDb::new();
        let affine = facts.insert(Fact::RomMapping {
            bank: "bank".into(),
            rom_space: RomAddressSpace::Physical,
            rom_start: 0x80,
            rom_end: 0x80 + output.len() as u32,
            va_start: 0x8000_0000,
            va_end: 0x8000_0000 + output.len() as u32,
        });
        facts.insert(Fact::EvaluatedImage {
            bank: "bank".into(),
            va_start: 0x8000_0000,
            va_end: 0x8000_0000 + output.len() as u32,
            receipt: evaluation.receipt().clone(),
        });
        facts
            .conclude(
                "bank:bank",
                ProofState::Proven,
                vec![affine],
                "only affine evidence is proven",
            )
            .unwrap();
        let backing = BankBackingSpanV1::Materialized {
            receipt_sha256: evaluated_image_receipt_sha256_v1(evaluation.receipt()),
            output_start: 0,
            output_end: output.len() as u32,
        };

        assert!(matches!(
            materialize_backing_span_v1(
                &rom,
                Some(&facts),
                "bank",
                0x8000_0000,
                0x8000_0000 + output.len() as u32,
                &backing,
                limits(),
                &mut MaterializedBackingSpanCacheV1::default(),
            ),
            Err(MaterializedBackingSpanErrorV1::MissingEvaluatedImageReceipt { .. })
        ));
    }

    #[test]
    fn tampering_and_resource_bounds_fail_closed() {
        let output = b"tamper target";
        let encoded = stream(output);
        let rom = synthetic_rom(&encoded, 0x80);
        let source = MaterializedImageSourceV1 {
            rom_space: RomAddressSpace::Physical,
            rom_start: 0x80,
            rom_end: 0x80 + encoded.len() as u32,
            cursor: 0,
        };
        let evaluator =
            MaterializationEvaluatorV1::HeaderedRawDeflateSequenceV1 { stream_count: 1 };
        let result =
            evaluate_materialized_image_v1(&rom, &FactDb::new(), &source, &evaluator, limits())
                .unwrap();

        let mut tampered = result.receipt().clone();
        tampered.streams[0].output_sha256 = "00".repeat(32);
        assert!(matches!(
            rederive_materialized_image_v1(&rom, &FactDb::new(), &tampered, limits()),
            Err(MaterializedImageErrorV1::ReceiptMismatch { .. })
        ));

        let mut bounded = limits();
        bounded.max_source_bytes = encoded.len() - 1;
        assert_eq!(
            evaluate_materialized_image_v1(&rom, &FactDb::new(), &source, &evaluator, bounded,),
            Err(MaterializedImageErrorV1::SourceExtentLimitExceeded {
                bytes: encoded.len(),
                limit: encoded.len() - 1,
            })
        );
        let mut invalid = limits();
        invalid.max_streams = HARD_MAX_STREAMS + 1;
        assert_eq!(
            evaluate_materialized_image_v1(&rom, &FactDb::new(), &source, &evaluator, invalid,),
            Err(MaterializedImageErrorV1::HardLimitExceeded {
                kind: MaterializedImageLimitKindV1::Streams,
                value: HARD_MAX_STREAMS + 1,
                hard_limit: HARD_MAX_STREAMS,
            })
        );
    }
}
