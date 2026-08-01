//! Bounded location of Rare `rzip` container headers in ROM bytes.
//!
//! [`headered_raw_deflate`](crate::headered_raw_deflate) decodes an explicitly
//! addressed sequence; it never scans, so it has no caller until something
//! supplies a cursor. This module is that something: it reports *candidate*
//! header sites and nothing else.
//!
//! A candidate is a magic match whose declared output length is in range. It
//! is not a proof that a stream is present, that it decodes, or that its
//! output is code — the two magic bytes occur by chance roughly every 64 KiB
//! of high-entropy data, so this scanner necessarily proposes far more sites
//! than exist. Only the decoder's exact `StreamEnd` and declared-length checks
//! separate a real container from a coincidence.
//!
//! Two on-ROM variants, both carrying plain raw DEFLATE. The header layouts
//! below were derived from `src/lib/rzip_c.c` in the MIT-licensed
//! `perfect-dark-pc-port/perfect_dark` decompilation, then re-verified against
//! permitted local ROM bytes rather than taken on trust:
//!
//! | variant | header | declared length |
//! |---|---|---|
//! | `0x1172` | 6 bytes | 4-byte big-endian at `[2..6]` |
//! | `0x1173` | 5 bytes | 3-byte big-endian at `[2..5]` |
//!
//! The measurement matters because that reference also describes a *runtime*
//! `1172` entry point taking a two-byte header with no length at all. That
//! form decodes zero streams from ROM images; the six-byte form decodes 3,035
//! in Banjo-Kazooie. The on-ROM layout is the one recorded here.

use serde::{Deserialize, Serialize};

/// `0x1172`: six-byte header, four-byte big-endian declared output length.
const MAGIC_1172: [u8; 2] = [0x11, 0x72];
const HEADER_LEN_1172: usize = 6;

/// `0x1173`: five-byte header, three-byte big-endian declared output length.
const MAGIC_1173: [u8; 2] = [0x11, 0x73];
const HEADER_LEN_1173: usize = 5;

/// Which container a candidate header claims to be.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RzipVariantV1 {
    /// Six-byte header, four-byte big-endian length.
    Headered1172,
    /// Five-byte header, three-byte big-endian length.
    Headered1173,
}

impl RzipVariantV1 {
    /// Bytes from the magic to the first DEFLATE byte.
    pub const fn header_len(self) -> usize {
        match self {
            Self::Headered1172 => HEADER_LEN_1172,
            Self::Headered1173 => HEADER_LEN_1173,
        }
    }

    const fn magic(self) -> [u8; 2] {
        match self {
            Self::Headered1172 => MAGIC_1172,
            Self::Headered1173 => MAGIC_1173,
        }
    }
}

/// One candidate container header. Offsets are relative to the scanned slice.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct RzipCandidateV1 {
    pub variant: RzipVariantV1,
    /// Offset of the magic — the cursor a decoder is given.
    pub cursor: usize,
    /// Output length the header declares. Unverified until decode.
    pub declared_output_len: u32,
}

/// Bounds on one scan. Every field is a hard ceiling, never a heuristic.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RzipScanLimitsV1 {
    /// Largest slice this scanner will read.
    pub max_input_bytes: usize,
    /// Reject a header declaring more output than this. Matches
    /// [`crate::headered_raw_deflate::HeaderedRawDeflateLimits`] so the
    /// scanner never proposes a site the decoder must refuse on size alone.
    pub max_declared_output_bytes: u32,
    /// Stop after this many candidates. A truncated scan is a frontier, not
    /// proven absence, so [`RzipScanV1::limit_hit`] records it.
    pub max_candidates: usize,
}

impl Default for RzipScanLimitsV1 {
    fn default() -> Self {
        Self {
            max_input_bytes: 64 * 1024 * 1024,
            max_declared_output_bytes: 64 * 1024 * 1024,
            max_candidates: 65_536,
        }
    }
}

/// Result of one bounded scan.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct RzipScanV1 {
    /// Candidates in ascending cursor order.
    pub candidates: Vec<RzipCandidateV1>,
    /// Magic matches rejected because the declared length was zero or over
    /// the cap. Retained so "no candidates" is distinguishable from "every
    /// match was implausible".
    pub rejected_headers: usize,
    /// The candidate cap stopped the scan before the slice ended.
    pub limit_hit: bool,
}

/// Why a scan could not run at all.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RzipScanError {
    InputLimitExceeded { bytes: usize, limit: usize },
}

impl std::fmt::Display for RzipScanError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InputLimitExceeded { bytes, limit } => {
                write!(formatter, "rzip scan input {bytes} exceeds its {limit}-byte bound")
            }
        }
    }
}

impl std::error::Error for RzipScanError {}

/// Locate candidate `rzip` headers in `source`.
///
/// Scans every byte offset, not just aligned ones: a container may sit at any
/// offset inside a file-table record, and a two-byte magic is cheap to test.
/// Overlapping candidates are all reported — the decoder decides.
///
/// Candidates are proposals. A caller must decode each one and keep only those
/// that reach `StreamEnd` at exactly the declared length; this function
/// deliberately performs no inflation so a scan can never be mistaken for
/// evidence that a container exists.
pub fn scan_rzip_candidates_v1(
    source: &[u8],
    limits: RzipScanLimitsV1,
) -> Result<RzipScanV1, RzipScanError> {
    if source.len() > limits.max_input_bytes {
        return Err(RzipScanError::InputLimitExceeded {
            bytes: source.len(),
            limit: limits.max_input_bytes,
        });
    }

    let mut scan = RzipScanV1::default();
    for cursor in 0..source.len().saturating_sub(1) {
        let magic = [source[cursor], source[cursor + 1]];
        let variant = if magic == MAGIC_1172 {
            RzipVariantV1::Headered1172
        } else if magic == MAGIC_1173 {
            RzipVariantV1::Headered1173
        } else {
            continue;
        };

        let Some(declared_output_len) = declared_output_len(source, cursor, variant) else {
            // Truncated header at the tail of the slice: not a rejection of a
            // plausible container, just no room for one.
            continue;
        };
        if declared_output_len == 0 || declared_output_len > limits.max_declared_output_bytes {
            scan.rejected_headers += 1;
            continue;
        }

        if scan.candidates.len() == limits.max_candidates {
            scan.limit_hit = true;
            break;
        }
        scan.candidates.push(RzipCandidateV1 {
            variant,
            cursor,
            declared_output_len,
        });
    }
    Ok(scan)
}

/// Read the declared output length, or `None` when the header is truncated.
fn declared_output_len(source: &[u8], cursor: usize, variant: RzipVariantV1) -> Option<u32> {
    let end = cursor.checked_add(variant.header_len())?;
    let header = source.get(cursor..end)?;
    debug_assert_eq!([header[0], header[1]], variant.magic());
    Some(match variant {
        RzipVariantV1::Headered1172 => u32::from_be_bytes(header[2..6].try_into().ok()?),
        RzipVariantV1::Headered1173 => {
            (u32::from(header[2]) << 16) | (u32::from(header[3]) << 8) | u32::from(header[4])
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn header_1172(declared: u32) -> Vec<u8> {
        let mut bytes = vec![0x11, 0x72];
        bytes.extend_from_slice(&declared.to_be_bytes());
        bytes
    }

    fn header_1173(declared: u32) -> Vec<u8> {
        let mut bytes = vec![0x11, 0x73];
        bytes.extend_from_slice(&declared.to_be_bytes()[1..]);
        bytes
    }

    #[test]
    fn locates_both_variants_with_their_own_header_lengths() {
        let mut rom = vec![0u8; 8];
        rom.extend(header_1172(0x1234));
        rom.extend([0xaa; 4]);
        let cursor_1173 = rom.len();
        rom.extend(header_1173(0x5678));
        rom.extend([0xbb; 4]);

        let scan = scan_rzip_candidates_v1(&rom, RzipScanLimitsV1::default()).unwrap();
        assert_eq!(
            scan.candidates,
            vec![
                RzipCandidateV1 {
                    variant: RzipVariantV1::Headered1172,
                    cursor: 8,
                    declared_output_len: 0x1234,
                },
                RzipCandidateV1 {
                    variant: RzipVariantV1::Headered1173,
                    cursor: cursor_1173,
                    declared_output_len: 0x5678,
                },
            ]
        );
        assert!(!scan.limit_hit);
    }

    #[test]
    fn the_two_variants_read_different_length_widths() {
        // Same trailing bytes, different magic: 1172 reads four bytes and
        // 1173 reads three. Confusing them silently mis-sizes every stream.
        let tail = [0x00, 0x01, 0x02, 0x03];
        let mut a = vec![0x11, 0x72];
        a.extend_from_slice(&tail);
        let mut b = vec![0x11, 0x73];
        b.extend_from_slice(&tail);

        let scan_a = scan_rzip_candidates_v1(&a, RzipScanLimitsV1::default()).unwrap();
        let scan_b = scan_rzip_candidates_v1(&b, RzipScanLimitsV1::default()).unwrap();
        assert_eq!(scan_a.candidates[0].declared_output_len, 0x0001_0203);
        assert_eq!(scan_b.candidates[0].declared_output_len, 0x0001_02);
    }

    #[test]
    fn implausible_declared_lengths_are_counted_not_proposed() {
        let mut rom = header_1172(0);
        rom.extend(header_1172(u32::MAX));
        let limits = RzipScanLimitsV1 {
            max_declared_output_bytes: 0x1000,
            ..RzipScanLimitsV1::default()
        };
        let scan = scan_rzip_candidates_v1(&rom, limits).unwrap();
        assert!(scan.candidates.is_empty());
        assert_eq!(scan.rejected_headers, 2);
    }

    #[test]
    fn a_truncated_trailing_header_is_not_a_rejection() {
        // Two magic bytes at the very end have no room for a length. That is
        // absence of a header, not a header that failed a plausibility test.
        let rom = vec![0x00, 0x11, 0x72];
        let scan = scan_rzip_candidates_v1(&rom, RzipScanLimitsV1::default()).unwrap();
        assert!(scan.candidates.is_empty());
        assert_eq!(scan.rejected_headers, 0);
    }

    #[test]
    fn the_candidate_cap_reports_a_frontier_rather_than_truncating_silently() {
        let mut rom = Vec::new();
        for _ in 0..4 {
            rom.extend(header_1172(0x40));
        }
        let limits = RzipScanLimitsV1 {
            max_candidates: 2,
            ..RzipScanLimitsV1::default()
        };
        let scan = scan_rzip_candidates_v1(&rom, limits).unwrap();
        assert_eq!(scan.candidates.len(), 2);
        assert!(scan.limit_hit);
    }

    #[test]
    fn unaligned_headers_are_found() {
        // Containers sit at arbitrary offsets inside file-table records, so an
        // aligned-only scan would miss them.
        let mut rom = vec![0xff];
        rom.extend(header_1173(0x20));
        let scan = scan_rzip_candidates_v1(&rom, RzipScanLimitsV1::default()).unwrap();
        assert_eq!(scan.candidates.len(), 1);
        assert_eq!(scan.candidates[0].cursor, 1);
    }

    #[test]
    fn an_oversized_input_is_refused_before_scanning() {
        let rom = vec![0u8; 32];
        let limits = RzipScanLimitsV1 {
            max_input_bytes: 16,
            ..RzipScanLimitsV1::default()
        };
        assert_eq!(
            scan_rzip_candidates_v1(&rom, limits),
            Err(RzipScanError::InputLimitExceeded {
                bytes: 32,
                limit: 16
            })
        );
    }
}
