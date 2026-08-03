//! Byte-level accounting for a whole ROM.
//!
//! Coverage has been reported as "% of ROM bytes mapped", which is close to
//! meaningless: most of a ROM is textures, audio and models that will never be
//! code. Measured, OoT maps 26% of its bytes and WWF No Mercy 7% -- and nobody
//! could say whether that is 95% of the code or 30% of it, because nothing knew
//! what the other 74-93% contained.
//!
//! This module answers a smaller, answerable question instead: **is every byte
//! ACCOUNTED FOR?** Each byte carries a typed claim or is explicitly
//! unaccounted, and the unaccounted count is a number that can be driven down
//! and argued about. Accounting for a byte is not the same as understanding it;
//! "this span is high-entropy asset data" is an account, not a claim about
//! meaning.
//!
//! Nothing here promotes anything. The ledger reads facts a composition already
//! produced and classifies the residue; it never concludes a mapping.

use crate::delta_vote::{infer_region_delta, DeltaVoteConfig, RegionScanStats};
use crate::facts::{Fact, FactDb};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// How a span of ROM is accounted for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SpanClass {
    /// Inside a proven `RomMapping` -- a composition already claims it.
    Mapped,
    /// The ROM header and IPL3 blob, whose extent is fixed by hardware.
    HeaderAndIpl3,
    /// A single repeated byte. Padding or erased space; carries no content.
    Padding,
    /// Carries a known compression-container magic.
    Container,
    /// Contains function prologues and calls: MIPS code that no mapping covers.
    /// This is the class that matters -- undiscovered code.
    CodeLike,
    /// High byte-entropy with no code evidence: compressed or asset payload.
    HighEntropy,
    /// Examined, carries content, and shows NO code evidence at a resolution
    /// where the code test is measured to fire on 92-100% of genuine code.
    /// That measurement is what licenses a positive name here rather than a
    /// shrug: level data, uncompressed textures, audio tables.
    StructuredData,
    /// Reached the end of classification without matching anything. Should be
    /// zero; a nonzero count is a bug in the ledger, not a property of the ROM.
    Unclassified,
}

impl SpanClass {
    pub fn label(self) -> &'static str {
        match self {
            Self::Mapped => "mapped",
            Self::HeaderAndIpl3 => "header_and_ipl3",
            Self::Padding => "padding",
            Self::Container => "container",
            Self::CodeLike => "code_like",
            Self::HighEntropy => "high_entropy",
            Self::StructuredData => "structured_data",
            Self::Unclassified => "unclassified",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct LedgerSpan {
    pub rom_start: u32,
    pub rom_end: u32,
    pub class: SpanClass,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RomLedger {
    pub total_bytes: u64,
    pub spans: Vec<LedgerSpan>,
    /// Bytes per class. Sums to `total_bytes` by construction -- that identity
    /// is the whole point and is asserted in a test.
    pub bytes_by_class: BTreeMap<String, u64>,
}

impl RomLedger {
    pub fn bytes(&self, class: SpanClass) -> u64 {
        self.bytes_by_class
            .get(class.label())
            .copied()
            .unwrap_or_default()
    }

    /// Bytes carrying MIPS code that no composition maps. The number to drive
    /// toward zero: unlike "% of ROM covered", it does not count assets against
    /// the score.
    ///
    /// A FLOOR whose gap is measured rather than guessed: the code test misses
    /// roughly 0-8% of genuine code chunks at this resolution (100% of OoT's
    /// boot extent detected, 92% of Majora's Mask's).
    pub fn undiscovered_code_bytes(&self) -> u64 {
        self.bytes(SpanClass::CodeLike)
    }
}

/// Resolution of the residue scan. Small enough that a single overlay is not
/// averaged into megabytes of surrounding assets, large enough to hold the
/// prologues and calls a code test needs.
pub(crate) const RESIDUE_SPAN: u32 = 0x2000;

/// Hardware-fixed: the header plus the IPL3 blob, before the boot copy.
const IPL3_END: u32 = 0x1000;

const CONTAINER_MAGICS: [&[u8]; 3] = [b"Yaz0", b"MIO0", &[0x1f, 0x8b]];

fn is_padding(bytes: &[u8]) -> bool {
    bytes
        .first()
        .is_some_and(|first| bytes.iter().all(|b| b == first))
}

fn has_container_magic(bytes: &[u8]) -> bool {
    CONTAINER_MAGICS
        .iter()
        .any(|magic| bytes.starts_with(magic))
}

/// Shannon entropy over the byte histogram, in bits per byte. Compressed and
/// most asset payloads sit near 8; code and structured data sit well below.
fn entropy_bits(bytes: &[u8]) -> f64 {
    if bytes.is_empty() {
        return 0.0;
    }
    let mut histogram = [0u32; 256];
    for byte in bytes {
        histogram[*byte as usize] += 1;
    }
    let len = bytes.len() as f64;
    -histogram
        .iter()
        .filter(|count| **count > 0)
        .map(|count| {
            let p = f64::from(*count) / len;
            p * p.log2()
        })
        .sum::<f64>()
}

/// Entropy at or above which a span with no code evidence is called asset or
/// compressed payload. Named rather than buried: 7.5 bits/byte is comfortably
/// above structured data and below the ~7.99 of well-compressed content, and a
/// span misfiled here is still ACCOUNTED, which is the property that matters.
const HIGH_ENTROPY_BITS: f64 = 7.5;

fn classify_residue(bytes: &[u8], rom_start: u32, vote: &DeltaVoteConfig) -> SpanClass {
    if is_padding(bytes) {
        return SpanClass::Padding;
    }
    if has_container_magic(bytes) {
        return SpanClass::Container;
    }
    // Reuse delta_vote's scan rather than inventing a second decoder: a span
    // holding function prologues AND calls is code, and those are exactly the
    // signals delta_vote already extracts and is tested on.
    let (is_code_like, _) = code_like_residue_scan(bytes, rom_start, vote);
    // Every function returns; data does not. Requiring RETURNS, not merely a
    // prologue, is what stops one coincidental `addiu $sp,$sp,-N` admitting a
    // whole span of asset bytes.
    //
    // Measured: OoT's known boot code shows 33-65 returns per 8 KiB and WCW's
    // two real images 666 and 667 across their extents, while the eight spans a
    // prologue-presence test wrongly called code show ZERO returns against
    // 43-126 `jal`s -- the latter being just the chance rate, since opcode 0x03
    // matches one word in 64.
    //
    // The ratio guard is the point: a lone return is as coincidental as a lone
    // prologue. Real code pairs them, because every function has both.
    if is_code_like {
        return SpanClass::CodeLike;
    }
    if entropy_bits(bytes) >= HIGH_ENTROPY_BITS {
        return SpanClass::HighEntropy;
    }
    // Positively typed, not a shrug. The code test above is measured to fire on
    // 92-100% of genuine code at this resolution (tests/ledger_code_sensitivity),
    // so a span reaching here is content that is not code.
    SpanClass::StructuredData
}

/// Run the ledger's measured code predicate without assigning a mapping or
/// proof state. Evaluated-image diagnostics use this exact predicate rather
/// than growing a second notion of "looks like code."
pub(crate) fn code_like_residue_scan(
    bytes: &[u8],
    address_start: u32,
    vote: &DeltaVoteConfig,
) -> (bool, RegionScanStats) {
    let scan = infer_region_delta(bytes, address_start, &[], vote).scan;
    let has_functions = scan.return_sites > 0
        && scan.prologue_sites > 0
        && scan.return_sites * 4 >= scan.prologue_sites
        && scan.prologue_sites * 4 >= scan.return_sites;
    (has_functions && scan.jal_sites > 0, scan)
}

/// Account for every byte of a ROM.
///
/// Proven `RomMapping` facts claim their extents; the header and IPL3 are
/// hardware-fixed; everything else is examined at [`RESIDUE_SPAN`] resolution
/// and typed. Adjacent spans of the same class merge, so the output is a
/// partition and not a histogram of windows.
pub fn build_ledger(rom_bytes: &[u8], facts: &FactDb) -> RomLedger {
    let total = rom_bytes.len() as u32;
    let vote = DeltaVoteConfig::default();

    // Byte-indexed class map. Later writes never overwrite an earlier claim, so
    // a proven mapping always wins over a guess about the same bytes.
    let mut claimed: Vec<Option<SpanClass>> = vec![None; total as usize];
    let claim = |start: u32, end: u32, class: SpanClass, claimed: &mut Vec<Option<SpanClass>>| {
        for slot in claimed
            .iter_mut()
            .take(end.min(total) as usize)
            .skip(start.min(total) as usize)
        {
            slot.get_or_insert(class);
        }
    };

    claim(0, IPL3_END, SpanClass::HeaderAndIpl3, &mut claimed);
    for fact in facts.proven_rom_mappings() {
        if let Fact::RomMapping {
            rom_start, rom_end, ..
        } = fact
        {
            claim(*rom_start, *rom_end, SpanClass::Mapped, &mut claimed);
        }
    }

    // Walk MAXIMAL UNCLAIMED RUNS rather than fixed-offset windows. A window
    // that merely starts inside a claim would otherwise be skipped whole,
    // silently leaving its unclaimed tail unexamined -- which is precisely the
    // hole a ledger exists to make impossible.
    let mut offset = 0u32;
    while offset < total {
        if claimed[offset as usize].is_some() {
            offset += 1;
            continue;
        }
        let run_start = offset;
        while offset < total && claimed[offset as usize].is_none() {
            offset += 1;
        }
        let run_end = offset;
        // Classify the run in bounded chunks so one overlay is not averaged
        // into megabytes of surrounding assets.
        let mut chunk = run_start;
        while chunk < run_end {
            let end = chunk.saturating_add(RESIDUE_SPAN).min(run_end);
            let class = classify_residue(&rom_bytes[chunk as usize..end as usize], chunk, &vote);
            claim(chunk, end, class, &mut claimed);
            chunk = end;
        }
    }

    // Merge the byte map into a span partition.
    let mut spans: Vec<LedgerSpan> = Vec::new();
    let mut bytes_by_class: BTreeMap<String, u64> = BTreeMap::new();
    for (index, slot) in claimed.iter().enumerate() {
        let class = slot.unwrap_or(SpanClass::Unclassified);
        *bytes_by_class.entry(class.label().to_string()).or_default() += 1;
        match spans.last_mut() {
            Some(previous) if previous.class == class && previous.rom_end == index as u32 => {
                previous.rom_end += 1;
            }
            _ => spans.push(LedgerSpan {
                rom_start: index as u32,
                rom_end: index as u32 + 1,
                class,
            }),
        }
    }

    RomLedger {
        total_bytes: total as u64,
        spans,
        bytes_by_class,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::facts::{ProofState, RomAddressSpace};

    fn rom_with(bytes: Vec<u8>) -> Vec<u8> {
        let mut rom = vec![0u8; IPL3_END as usize];
        rom.extend(bytes);
        rom
    }

    #[test]
    fn every_byte_is_accounted_exactly_once() {
        // The identity the ledger exists to provide: classes partition the ROM.
        // Without it, "unaccounted bytes" is not a number anyone can trust.
        let rom = rom_with(vec![0u8; 0x4000]);
        let ledger = build_ledger(&rom, &FactDb::new());
        let summed: u64 = ledger.bytes_by_class.values().sum();
        assert_eq!(summed, ledger.total_bytes);
        assert_eq!(ledger.total_bytes, rom.len() as u64);
        // And the span list covers the ROM contiguously with no gap or overlap.
        let mut cursor = 0u32;
        for span in &ledger.spans {
            assert_eq!(span.rom_start, cursor, "gap or overlap at 0x{cursor:x}");
            cursor = span.rom_end;
        }
        assert_eq!(cursor as u64, ledger.total_bytes);
    }

    #[test]
    fn a_proven_mapping_outranks_a_guess_about_the_same_bytes() {
        // Residue typing is a heuristic; a composition's proven mapping is not.
        // The heuristic must never overwrite the claim.
        let mut rom = rom_with(vec![0xffu8; 0x4000]);
        // Make the mapped region look like padding, which the residue pass
        // would otherwise classify as such.
        for byte in rom.iter_mut().skip(IPL3_END as usize) {
            *byte = 0x00;
        }
        let mut facts = FactDb::new();
        let mapping = facts.insert(Fact::RomMapping {
            bank: "code".to_string(),
            rom_space: RomAddressSpace::Physical,
            rom_start: IPL3_END,
            rom_end: IPL3_END + 0x2000,
            va_start: 0x8000_0000,
            va_end: 0x8000_2000,
        });
        facts
            .conclude("bank:code", ProofState::Proven, vec![mapping], "test")
            .unwrap();

        let ledger = build_ledger(&rom, &facts);
        assert_eq!(ledger.bytes(SpanClass::Mapped), 0x2000);
        assert!(
            ledger
                .spans
                .iter()
                .any(|span| span.rom_start == IPL3_END && span.class == SpanClass::Mapped),
            "the proven mapping was overwritten by residue classification"
        );
    }

    #[test]
    fn padding_and_entropy_are_distinguished() {
        let mut body = vec![0u8; 0x2000];
        // A second span of incompressible bytes.
        let mut state = 0x1234_5678u32;
        body.extend((0..0x2000).map(|_| {
            state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            (state >> 24) as u8
        }));
        let ledger = build_ledger(&rom_with(body), &FactDb::new());
        assert_eq!(ledger.bytes(SpanClass::Padding), 0x2000);
        assert_eq!(ledger.bytes(SpanClass::HighEntropy), 0x2000);
    }
}
