//! Where a ROM's executable code lives: one resident bank, or several spans.
//!
//! [`DiscoveryStrategy::BootBankOnly`] is a single verdict covering two very
//! different ROMs. One genuinely has no overlays, so the verdict is COMPLETE
//! and there is nothing further to recover. The other has overlays this build
//! cannot find, which is a real miss. Nothing distinguished them, so planning
//! counted every zero-candidate ROM as outstanding work.
//!
//! Measured over the 287-ROM corpus, that mattered by a factor of ten:
//!
//! ```text
//!   NO_CANDIDATES and single/mostly-single bank   208   COMPLETE
//!   NO_CANDIDATES and genuinely MULTI_SPAN         21   real misses
//! ```
//!
//! This is a property of the ROM, not of the current search, so it stays true
//! as discovery improves -- unlike a candidate/admitted count, which measures
//! this build's capability. Recording both is what lets a later run tell
//! "we got better" apart from "the ROM was always like this".
//!
//! [`DiscoveryStrategy::BootBankOnly`]: crate::DiscoveryStrategy::BootBankOnly

use serde::{Deserialize, Serialize};

/// `jr $ra`, the cheapest structural proxy for "compiled code is here".
///
/// Every non-leaf MIPS function ends with one, and a full 32-bit pattern makes
/// coincidental matches in data rare. This deliberately does not decode
/// instructions: the question is only where code is dense, not what it does.
const JR_RA: [u8; 4] = [0x03, 0xe0, 0x00, 0x08];

/// Gap that separates two spans. Generous on purpose -- a 256 KiB hole of
/// data, tables or padding inside one bank of code is ordinary, while a real
/// overlay region sits much further out.
const SPAN_GAP_BYTES: usize = 0x40000;

/// At or above this share in one span, the ROM is single-bank: there is no
/// overlay geometry to recover and a `BootBankOnly` verdict is complete.
const SINGLE_BANK_CONCENTRATION: f64 = 0.95;

/// Below `SINGLE_BANK_CONCENTRATION` but at or above this, the tail is small
/// enough that it is usually data or a stub rather than a code overlay.
const MOSTLY_SINGLE_BANK_CONCENTRATION: f64 = 0.80;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CodeSpanClass {
    /// No `jr $ra` at all. Not a normal N64 game image; says nothing about
    /// overlays either way.
    NoCodeFound,
    /// Effectively all code in one span. Overlay discovery finding nothing
    /// here is a COMPLETE result.
    SingleBank,
    /// One dominant span with a small tail.
    MostlySingleBank,
    /// Code genuinely spread across spans. Overlay discovery finding nothing
    /// here is a MISS worth investigating.
    MultiSpan,
}

impl CodeSpanClass {
    /// Whether "no overlay table found" is CONSISTENT with the ROM's code
    /// layout, rather than evidence something was missed.
    ///
    /// # This is a weak signal, not a proof of absence
    ///
    /// Measured counter-example: WWF WrestleMania 2000 reports `SingleBank` at
    /// concentration 1.00 (2,484 `jr $ra`, one span `[0x878,0x13d1c8)`) and it
    /// *does* have overlays that discovery recovers. Its overlay images are
    /// loaded from ROM offsets INSIDE that same contiguous span, so nothing in
    /// the code-density profile distinguishes them from the resident bank.
    ///
    /// So a true return means only "the ROM does not look multi-span", which
    /// is one input to a judgement, never the judgement itself. It is safe in
    /// the aggregate -- it correctly separated 208 complete ROMs from 21 real
    /// misses across the corpus -- and unsafe as a per-ROM verdict.
    ///
    /// [`Self::MultiSpan`] is the informative direction: distant code the
    /// overlay search did not account for is a genuine gap.
    pub fn absent_overlays_are_expected(self) -> bool {
        matches!(self, Self::SingleBank | Self::MostlySingleBank)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CodeSpanLocality {
    pub class: CodeSpanClass,
    pub jr_ra_sites: usize,
    pub span_count: usize,
    /// Share of all `jr $ra` sites in the largest span, 0.0 when none exist.
    pub largest_span_concentration: f64,
    /// Byte bounds of the largest span, inclusive of its last site.
    pub largest_span: Option<(usize, usize)>,
}

/// Measure code-span locality over normalized (big-endian) ROM bytes.
pub fn measure_code_span_locality(rom_bytes: &[u8]) -> CodeSpanLocality {
    let sites: Vec<usize> = (0..rom_bytes.len().saturating_sub(4))
        .step_by(4)
        .filter(|&offset| rom_bytes[offset..offset + 4] == JR_RA)
        .collect();

    let Some(&first) = sites.first() else {
        return CodeSpanLocality {
            class: CodeSpanClass::NoCodeFound,
            jr_ra_sites: 0,
            span_count: 0,
            largest_span_concentration: 0.0,
            largest_span: None,
        };
    };

    let mut spans: Vec<(usize, usize, usize)> = Vec::new();
    let (mut span_start, mut span_end, mut count) = (first, first, 0usize);
    for &site in &sites {
        if site - span_end > SPAN_GAP_BYTES {
            spans.push((span_start, span_end, count));
            span_start = site;
            count = 0;
        }
        span_end = site;
        count += 1;
    }
    spans.push((span_start, span_end, count));

    let largest = spans
        .iter()
        .copied()
        .max_by_key(|&(_, _, count)| count)
        .expect("at least one span exists when sites is non-empty");
    let concentration = largest.2 as f64 / sites.len() as f64;
    let class = if concentration >= SINGLE_BANK_CONCENTRATION {
        CodeSpanClass::SingleBank
    } else if concentration >= MOSTLY_SINGLE_BANK_CONCENTRATION {
        CodeSpanClass::MostlySingleBank
    } else {
        CodeSpanClass::MultiSpan
    };

    CodeSpanLocality {
        class,
        jr_ra_sites: sites.len(),
        span_count: spans.len(),
        largest_span_concentration: concentration,
        largest_span: Some((largest.0, largest.1)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rom_with_jr_ra_at(offsets: &[usize], len: usize) -> Vec<u8> {
        let mut bytes = vec![0u8; len];
        for &offset in offsets {
            bytes[offset..offset + 4].copy_from_slice(&JR_RA);
        }
        bytes
    }

    #[test]
    fn empty_rom_reports_no_code_and_is_not_treated_as_complete() {
        let locality = measure_code_span_locality(&vec![0u8; 0x1000]);
        assert_eq!(locality.class, CodeSpanClass::NoCodeFound);
        assert_eq!(locality.jr_ra_sites, 0);
        // Inconclusive must NOT read as "no overlays expected", or a ROM we
        // failed to parse would silently count as complete.
        assert!(!locality.class.absent_overlays_are_expected());
    }

    #[test]
    fn one_dense_span_is_single_bank() {
        let offsets: Vec<usize> = (0..100).map(|index| 0x1000 + index * 0x10).collect();
        let locality = measure_code_span_locality(&rom_with_jr_ra_at(&offsets, 0x100000));
        assert_eq!(locality.class, CodeSpanClass::SingleBank);
        assert_eq!(locality.span_count, 1);
        assert_eq!(locality.jr_ra_sites, 100);
        assert!(locality.class.absent_overlays_are_expected());
    }

    #[test]
    fn two_far_apart_equal_spans_are_multi_span() {
        let mut offsets: Vec<usize> = (0..50).map(|index| 0x1000 + index * 0x10).collect();
        offsets.extend((0..50).map(|index| 0x400000 + index * 0x10));
        let locality = measure_code_span_locality(&rom_with_jr_ra_at(&offsets, 0x800000));
        assert_eq!(locality.class, CodeSpanClass::MultiSpan);
        assert_eq!(locality.span_count, 2);
        assert!((locality.largest_span_concentration - 0.5).abs() < 1e-9);
        // The whole point: a missing overlay table here IS a miss.
        assert!(!locality.class.absent_overlays_are_expected());
    }

    #[test]
    fn a_small_distant_tail_stays_mostly_single_bank() {
        let mut offsets: Vec<usize> = (0..90).map(|index| 0x1000 + index * 0x10).collect();
        offsets.extend((0..10).map(|index| 0x400000 + index * 0x10));
        let locality = measure_code_span_locality(&rom_with_jr_ra_at(&offsets, 0x800000));
        assert_eq!(locality.class, CodeSpanClass::MostlySingleBank);
        assert!((locality.largest_span_concentration - 0.9).abs() < 1e-9);
    }

    #[test]
    fn single_bank_does_not_prove_overlays_are_absent() {
        // WM2000's real shape: every jr $ra in one contiguous span, yet the
        // ROM has overlays discovery recovers -- their images live INSIDE that
        // span. This pins the limitation so `absent_overlays_are_expected` is
        // never mistaken for a proof of absence.
        let offsets: Vec<usize> = (0..200).map(|index| 0x878 + index * 0x40).collect();
        let locality = measure_code_span_locality(&rom_with_jr_ra_at(&offsets, 0x200000));
        assert_eq!(locality.class, CodeSpanClass::SingleBank);
        assert!((locality.largest_span_concentration - 1.0).abs() < 1e-9);
        // True here, and yet overlays exist in the real ROM with this shape.
        assert!(locality.class.absent_overlays_are_expected());
    }

    #[test]
    fn a_gap_below_the_threshold_does_not_split_a_span() {
        // 0x30000 < SPAN_GAP_BYTES: ordinary data hole inside one bank.
        let offsets = [0x1000, 0x1004, 0x31004, 0x31008];
        let locality = measure_code_span_locality(&rom_with_jr_ra_at(&offsets, 0x100000));
        assert_eq!(locality.span_count, 1);
        assert_eq!(locality.class, CodeSpanClass::SingleBank);
    }
}
