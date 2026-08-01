//! Bounded, candidate-only validation of located Rare `rzip` streams.
//!
//! Header location is deliberately separate from validation: two magic bytes
//! are cheap candidate evidence, while exact raw-DEFLATE `StreamEnd` and
//! declared-length agreement prove that one container decodes. This module
//! composes those mechanisms without assigning a runtime address, bank,
//! placement, executable permission, or proof state.
//!
//! Results contain offsets, lengths, digests, and mapping-independent code
//! signals only. Decoded bytes are transient and never enter the result.

use crate::delta_vote::DeltaVoteConfig;
use crate::headered_raw_deflate::{
    materialize_headered_raw_deflate_1173_stream, materialize_headered_raw_deflate_stream,
    HeaderedRawDeflateDecodedStream, HeaderedRawDeflateError, HeaderedRawDeflateLimits,
};
use crate::ledger::{code_like_residue_scan, RESIDUE_SPAN};
use crate::rzip_scan::{scan_rzip_candidates_v1, RzipScanError, RzipScanLimitsV1, RzipVariantV1};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Bounds for one scan-and-validate pass.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RzipVerificationLimitsV1 {
    pub scan: RzipScanLimitsV1,
    pub decoder: HeaderedRawDeflateLimits,
    /// Sum of declared output lengths accepted into verified results.
    /// Candidates that cannot fit are skipped and counted as an explicit
    /// frontier; a large false header cannot prevent later smaller attempts.
    pub max_verified_output_bytes: u64,
}

impl Default for RzipVerificationLimitsV1 {
    fn default() -> Self {
        Self {
            scan: RzipScanLimitsV1::default(),
            decoder: HeaderedRawDeflateLimits::default(),
            max_verified_output_bytes: 64 * 1024 * 1024,
        }
    }
}

/// Mapping-independent code signals over one decoded stream.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RzipCodeEvidenceV1 {
    pub chunks: u32,
    pub code_like_chunks: u32,
    pub code_like_bytes: u64,
    pub aligned_words: u64,
    /// Exact aligned `jr $ra` words over the whole output. Unlike the ledger
    /// scan's `return_sites`, this does not require a delay word within the
    /// same 8 KiB classification chunk.
    pub jr_ra_words: u64,
    pub jal_sites: u64,
    pub prologue_sites: u64,
    pub return_sites: u64,
}

impl RzipCodeEvidenceV1 {
    pub fn contains_code_like_chunk(self) -> bool {
        self.code_like_chunks != 0
    }
}

/// Content-free evidence for one candidate that decoded exactly.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct VerifiedRzipStreamV1 {
    pub variant: RzipVariantV1,
    /// Offsets are relative to the caller-supplied source slice.
    pub source_start: usize,
    pub source_end: usize,
    pub encoded_start: usize,
    pub encoded_end: usize,
    pub output_len: usize,
    pub source_sha256: String,
    pub output_sha256: String,
    pub code: RzipCodeEvidenceV1,
}

/// Why a plausible header did not validate as one exact stream.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RzipDecoderRejectionReasonV1 {
    /// Scanner/decoder limits disagree or the candidate exceeds a decoder
    /// resource bound.
    ResourceLimit,
    /// A scanner-produced candidate did not satisfy the selected header
    /// layout. This indicates composition drift rather than random payload.
    HeaderInvariant,
    DeflateRejected,
    MissingStreamEnd,
    NoForwardProgress,
    OutputLengthMismatch,
    ArithmeticOverflow,
}

/// Distribution and explicit frontiers for one validation pass.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RzipVerificationV1 {
    pub scanned_candidates: usize,
    pub scanner_rejected_headers: usize,
    pub scanner_limit_hit: bool,
    pub attempted_candidates: usize,
    pub decoder_rejected_candidates: usize,
    pub decoder_rejections_by_reason: BTreeMap<RzipDecoderRejectionReasonV1, usize>,
    pub output_budget_skipped_candidates: usize,
    pub verified_output_bytes: u64,
    pub verified_streams: Vec<VerifiedRzipStreamV1>,
}

/// Scan and validate every candidate admitted by the supplied bounds.
///
/// A decoder rejection is expected evidence about a coincidental header, not
/// an error for the whole ROM. Failure to run the bounded scanner is returned.
pub fn verify_rzip_streams_v1(
    source: &[u8],
    limits: RzipVerificationLimitsV1,
) -> Result<RzipVerificationV1, RzipScanError> {
    let scan = scan_rzip_candidates_v1(source, limits.scan)?;
    let mut result = RzipVerificationV1 {
        scanned_candidates: scan.candidates.len(),
        scanner_rejected_headers: scan.rejected_headers,
        scanner_limit_hit: scan.limit_hit,
        ..RzipVerificationV1::default()
    };
    let vote = DeltaVoteConfig::default();

    for candidate in scan.candidates {
        let declared = u64::from(candidate.declared_output_len);
        let Some(projected_output) = result.verified_output_bytes.checked_add(declared) else {
            result.output_budget_skipped_candidates += 1;
            continue;
        };
        if projected_output > limits.max_verified_output_bytes {
            result.output_budget_skipped_candidates += 1;
            continue;
        }

        result.attempted_candidates += 1;
        let decoded = match candidate.variant {
            RzipVariantV1::Headered1172 => {
                materialize_headered_raw_deflate_stream(source, candidate.cursor, limits.decoder)
            }
            RzipVariantV1::Headered1173 => materialize_headered_raw_deflate_1173_stream(
                source,
                candidate.cursor,
                limits.decoder,
            ),
        };
        let decoded = match decoded {
            Ok(decoded) => decoded,
            Err(error) => {
                result.decoder_rejected_candidates += 1;
                *result
                    .decoder_rejections_by_reason
                    .entry(rejection_reason(error))
                    .or_default() += 1;
                continue;
            }
        };

        debug_assert_eq!(
            decoded.stream.declared_output_len,
            candidate.declared_output_len as usize
        );
        result.verified_output_bytes = projected_output;
        result
            .verified_streams
            .push(verified_stream(candidate.variant, decoded, &vote));
    }

    Ok(result)
}

fn rejection_reason(error: HeaderedRawDeflateError) -> RzipDecoderRejectionReasonV1 {
    match error {
        HeaderedRawDeflateError::InputLimitExceeded { .. }
        | HeaderedRawDeflateError::StreamCountLimitExceeded { .. }
        | HeaderedRawDeflateError::StreamOutputLimitExceeded { .. }
        | HeaderedRawDeflateError::AggregateOutputLimitExceeded { .. } => {
            RzipDecoderRejectionReasonV1::ResourceLimit
        }
        HeaderedRawDeflateError::CursorOutsideSource { .. }
        | HeaderedRawDeflateError::ZeroStreamCount
        | HeaderedRawDeflateError::TruncatedHeader { .. }
        | HeaderedRawDeflateError::InvalidMagic { .. }
        | HeaderedRawDeflateError::ZeroDeclaredOutput { .. } => {
            RzipDecoderRejectionReasonV1::HeaderInvariant
        }
        HeaderedRawDeflateError::DeflateRejected { .. } => {
            RzipDecoderRejectionReasonV1::DeflateRejected
        }
        HeaderedRawDeflateError::MissingStreamEnd { .. } => {
            RzipDecoderRejectionReasonV1::MissingStreamEnd
        }
        HeaderedRawDeflateError::NoForwardProgress { .. } => {
            RzipDecoderRejectionReasonV1::NoForwardProgress
        }
        HeaderedRawDeflateError::OutputLengthMismatch { .. } => {
            RzipDecoderRejectionReasonV1::OutputLengthMismatch
        }
        HeaderedRawDeflateError::AggregateOutputOverflow
        | HeaderedRawDeflateError::OutputAllocationOverflow { .. }
        | HeaderedRawDeflateError::DecoderCounterOverflow { .. }
        | HeaderedRawDeflateError::SourceRangeOverflow { .. } => {
            RzipDecoderRejectionReasonV1::ArithmeticOverflow
        }
    }
}

fn verified_stream(
    variant: RzipVariantV1,
    decoded: HeaderedRawDeflateDecodedStream,
    vote: &DeltaVoteConfig,
) -> VerifiedRzipStreamV1 {
    let mut code = RzipCodeEvidenceV1::default();
    code.jr_ra_words = decoded
        .bytes
        .chunks_exact(4)
        .filter(|word| *word == [0x03, 0xe0, 0x00, 0x08])
        .count() as u64;
    for (index, chunk) in decoded.bytes.chunks(RESIDUE_SPAN as usize).enumerate() {
        let address_start = u32::try_from(index)
            .ok()
            .and_then(|index| index.checked_mul(RESIDUE_SPAN))
            .expect("decoded stream is bounded below the u32 address space");
        let (is_code_like, scan) = code_like_residue_scan(chunk, address_start, vote);
        code.chunks += 1;
        code.code_like_chunks += u32::from(is_code_like);
        if is_code_like {
            code.code_like_bytes += chunk.len() as u64;
        }
        code.aligned_words += scan.words as u64;
        code.jal_sites += scan.jal_sites as u64;
        code.prologue_sites += scan.prologue_sites as u64;
        code.return_sites += scan.return_sites as u64;
    }

    VerifiedRzipStreamV1 {
        variant,
        source_start: decoded.stream.source_range.start,
        source_end: decoded.stream.source_range.end,
        encoded_start: decoded.stream.deflate_range.start,
        encoded_end: decoded.stream.deflate_range.end,
        output_len: decoded.bytes.len(),
        source_sha256: decoded.stream.source_sha256,
        output_sha256: decoded.stream.output_sha256,
        code,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use flate2::{write::DeflateEncoder, Compression};
    use std::io::Write;

    fn encode(bytes: &[u8], variant: RzipVariantV1) -> Vec<u8> {
        let mut encoder = DeflateEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(bytes).unwrap();
        let compressed = encoder.finish().unwrap();
        let declared = (bytes.len() as u32).to_be_bytes();
        let mut stream = match variant {
            RzipVariantV1::Headered1172 => {
                let mut header = vec![0x11, 0x72];
                header.extend_from_slice(&declared);
                header
            }
            RzipVariantV1::Headered1173 => {
                let mut header = vec![0x11, 0x73];
                header.extend_from_slice(&declared[1..]);
                header
            }
        };
        stream.extend_from_slice(&compressed);
        stream
    }

    fn limits(source_len: usize) -> RzipVerificationLimitsV1 {
        RzipVerificationLimitsV1 {
            scan: RzipScanLimitsV1 {
                max_input_bytes: source_len,
                max_declared_output_bytes: 0x20_0000,
                max_candidates: 32,
            },
            decoder: HeaderedRawDeflateLimits {
                max_input_bytes: source_len,
                max_stream_output_bytes: 0x20_0000,
                max_aggregate_output_bytes: 0x20_0000,
                max_streams: 1,
            },
            max_verified_output_bytes: 0x20_0000,
        }
    }

    #[test]
    fn validates_both_variants_and_rejects_a_magic_coincidence() {
        let first_output = b"first exact output";
        let second_output = b"second exact output";
        let first = encode(first_output, RzipVariantV1::Headered1172);
        let second = encode(second_output, RzipVariantV1::Headered1173);
        let mut source = vec![0x11, 0x73, 0, 0, 8, 0xff, 0xff];
        let first_start = source.len();
        source.extend_from_slice(&first);
        let second_start = source.len();
        source.extend_from_slice(&second);
        source.extend_from_slice(&vec![0xa5; 1024 * 1024]);

        let result = verify_rzip_streams_v1(&source, limits(source.len())).unwrap();

        assert_eq!(result.scanned_candidates, 3);
        assert_eq!(result.attempted_candidates, 3);
        assert_eq!(result.decoder_rejected_candidates, 1);
        assert_eq!(
            result.decoder_rejections_by_reason,
            BTreeMap::from([(RzipDecoderRejectionReasonV1::DeflateRejected, 1)])
        );
        assert_eq!(result.verified_streams.len(), 2);
        assert_eq!(result.verified_output_bytes, 37);
        assert_eq!(result.verified_streams[0].source_start, first_start);
        assert_eq!(
            result.verified_streams[0].source_end,
            first_start + first.len()
        );
        assert_eq!(result.verified_streams[1].source_start, second_start);
        assert_eq!(
            result.verified_streams[1].source_end,
            second_start + second.len()
        );
        assert!(!serde_json::to_string(&result)
            .unwrap()
            .contains("exact output"));
    }

    #[test]
    fn output_budget_skip_is_explicit_and_does_not_hide_a_later_small_stream() {
        let large = encode(&vec![0x44; 1024], RzipVariantV1::Headered1172);
        let small_output = b"small";
        let small = encode(small_output, RzipVariantV1::Headered1173);
        let source = [large, small].concat();
        let mut bounded = limits(source.len());
        bounded.max_verified_output_bytes = small_output.len() as u64;

        let result = verify_rzip_streams_v1(&source, bounded).unwrap();

        assert_eq!(result.output_budget_skipped_candidates, 1);
        assert_eq!(result.attempted_candidates, 1);
        assert_eq!(result.verified_streams.len(), 1);
        assert_eq!(
            result.verified_streams[0].variant,
            RzipVariantV1::Headered1173
        );
    }

    #[test]
    fn code_evidence_reuses_the_ledgers_chunk_predicate() {
        let mut output = vec![0u8; RESIDUE_SPAN as usize];
        for (offset, word) in [0x27bd_ffe0u32, 0x0c00_0000, 0, 0x03e0_0008, 0]
            .into_iter()
            .enumerate()
        {
            output[offset * 4..offset * 4 + 4].copy_from_slice(&word.to_be_bytes());
        }
        let source = encode(&output, RzipVariantV1::Headered1173);

        let result = verify_rzip_streams_v1(&source, limits(source.len())).unwrap();
        let code = result.verified_streams[0].code;

        assert_eq!(code.chunks, 1);
        assert_eq!(code.code_like_chunks, 1);
        assert_eq!(code.code_like_bytes, u64::from(RESIDUE_SPAN));
        assert_eq!(code.prologue_sites, 1);
        assert_eq!(code.jr_ra_words, 1);
        assert_eq!(code.return_sites, 1);
        assert_eq!(code.jal_sites, 1);
    }
}
