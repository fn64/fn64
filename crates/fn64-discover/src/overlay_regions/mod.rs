//! Mechanical recovery of candidate overlay ROM regions from ROM bytes alone.
//!
//! # The problem this closes
//!
//! [`crate::delta_vote`] recovers an overlay's VA delta from its ROM region,
//! but it needs the region `[rom_start, rom_end)` as input. For NW4E that
//! interval came from the byte-verified descriptor table in
//! [`crate::aki_reference`] at a hard-coded ROM offset. NWXE's overlays were
//! reported "absent" (`gate_delta_vote`: "NWXE not graded -- overlay ROM
//! intervals require a descriptor table ... that has not been byte-verified")
//! and the coverage ladder capped NWXE recall at 28% because of it. This
//! module removes the hard-coded offset: it *searches* the ROM for a table of
//! the NW4E descriptor FAMILY and returns the overlay intervals it implies.
//!
//! # The shape being generalized
//!
//! NW4E's descriptor table (ROM `0x539a0`, 5 records, stride `0x24`, fields at
//! record-relative `+0x00` rom_start, `+0x04` rom_end, `+0x08` vram_dest) is a
//! *shape*, not a location. The family is any `(stride, field_rom_start,
//! field_rom_end, field_vram_dest)` whose consecutive records carry a
//! plausible ROM interval and a plausible RDRAM destination field. The field
//! may hold either the destination start or its exclusive end; both typed
//! interpretations are enumerated and delta agreement admits at most the
//! matching geometry. This module enumerates that family across the whole ROM.
//!
//! # Discipline: ENUMERATE, VALIDATE, ADMIT only on uniqueness
//!
//! This is deliberately *not* a loose region scanner (the aligned-pointer-run
//! heuristic was rejected at 3.10% precision; a raw region-score threshold was
//! rejected too). Candidate tables are enumerated, each is validated by
//! bounded self-consistency constraints, phase aliases that describe the
//! identical interval set are canonicalized to one, and -- when more than one
//! *distinct* interval set survives -- [`crate::delta_vote`] admissibility is
//! the uniqueness filter that breaks the ambiguity. A table whose regions
//! delta_vote cannot map is not admitted merely because its records parse.
//! An open record may use its descriptor destination as a mapping hypothesis
//! only when a CFG rooted at that destination decodes without a blocker, every
//! reachable PC-relative branch stays in the proposed interval, and no
//! distinct admitted or corroborated record overlaps its VA interval. The
//! ordinary mapped-region floor remains the default table rule. A table below
//! that floor is admitted only when every record is independently
//! descriptor-corroborated and its resulting VA interval is globally unique;
//! partial corroboration never mints a table. One-record gaps are enumerated
//! only inside an already-established descriptor-family envelope, because an
//! arbitrary isolated pointer triple can otherwise alias real resident code.
//!
//! Every function here is a pure function of the ROM bytes and the search
//! configuration: no I/O, no randomness, byte-identical output across runs.
//!
//! # VROM-located descriptor tables
//!
//! [`recover_vrom_overlay_regions`] is a separate two-stage path for engines
//! whose descriptor table itself lives in VROM. It first invokes
//! [`crate::file_table`] to recover one physical
//! `(vrom_start,vrom_end,rom_start,rom_end)` table, then materializes each
//! VROM file and applies the same adjacent source-range/destination family and
//! `delta_vote` validation. Keeping this result separate is load-bearing: the
//! established physical [`recover_overlay_regions`] output and AKI gates are
//! unchanged, while every VROM table location is explicitly typed as virtual.
//!
//! # The PI-DMA route, and why it does not provide these regions
//!
//! [`crate::pi_dma`] slices `osPiStartDma`/`osEPiStartDma` calls for constant
//! (device, RDRAM, length) triples. That route recovers overlay regions only
//! when the DMA source address is an *immediate* in the caller. AKI overlay
//! loaders are table-driven: the copy routine reads `rom_start`/`rom_end`/
//! `vram_dest` out of a descriptor record through registers (see the NW4E
//! `record_loader` at VA `0x8000073c`, called with nine record fields --
//! `aki_reference`), so the device/length operands are register-derived from
//! the loop, not immediates, and pi_dma leaves them as explicit blockers. The
//! descriptor-table route recovers exactly those triples directly. `gate_
//! overlay_regions` states this cross-check rather than wiring a pi_dma pass
//! that structurally cannot resolve a table-driven copy.
//!
//! # Downstream proof boundary
//!
//! An admitted delta remains candidate mapping evidence in isolation.
//! [`crate::banks::scan_recovered_overlay_regions`] requires exactly one
//! admitted table and exact agreement between each record's delta-derived VA
//! and its independently parsed descriptor destination before producing a
//! proven bank-qualified load image. Multiple admissions, open deltas, and
//! destination disagreements remain conflict/open facts.

use crate::cfg::{build_cfg, BlockTerminator, WordClass};
use crate::delta_vote::{infer_region_delta, DeltaVoteConfig, DeltaVoteOutcome};
use crate::file_table::{
    recover_file_table, CandidateFileTable, FileTableRecovery, FileTableSearchConfig,
    VromMaterializationError, VromMaterializationLimits,
};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// Bounds and predicates the family search validates each record against.
/// Every field is part of the reported result's meaning; a gate must print
/// the configuration it searched with.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SearchConfig {
    /// Lowest ROM offset a record's `rom_start`/`rom_end` may name. The ROM
    /// header and IPL live below this; an overlay never starts inside them.
    pub min_rom_offset: u32,
    /// Smallest overlay length accepted (a code region, not a stub field).
    pub min_region_len: u32,
    /// Largest overlay length accepted. Guards against a pair of unrelated
    /// large pointers manufacturing a giant "region".
    pub max_region_len: u32,
    /// Inclusive lower bound of the plausible RDRAM destination VA window
    /// (cached kseg0 RDRAM).
    pub vram_lo: u32,
    /// Exclusive upper bound of the plausible RDRAM destination VA window.
    pub vram_hi: u32,
    /// Record strides to try, in bytes (each a multiple of 4, >= 12).
    pub strides: Vec<u32>,
    /// Minimum number of consecutive valid records for a run to be a table.
    pub min_records: u32,
}

impl SearchConfig {
    /// The default family search: an 8 MiB RDRAM window (`0x8000_0000 ..
    /// 0x8080_0000`), overlay lengths from 4 KiB to 2 MiB, and strides
    /// covering the AKI `0x24` record plus neighbours. `min_records = 2` is
    /// the smallest run the family search will consider: a two-record table is
    /// a real shape (a game with exactly two mutually exclusive overlays --
    /// measured on WCW/nWo Revenge -- has nothing longer to offer), and two
    /// records CAN share one spurious spacing, so the floor alone is not the
    /// evidence. What keeps a coincidental pair out is downstream: every
    /// record must decode as code at its declared VA
    /// ([`descriptor_mapping_corroborated`]), and a table is admitted only
    /// when `min_mapped_regions` of its records map uniquely -- callers
    /// derive that floor from this one, so a 2-record table must map BOTH.
    pub fn aki_family() -> Self {
        Self {
            min_rom_offset: 0x1000,
            min_region_len: 0x1000,
            max_region_len: 0x0020_0000,
            vram_lo: 0x8000_0000,
            vram_hi: 0x8080_0000,
            strides: vec![
                0x0c, 0x10, 0x14, 0x18, 0x1c, 0x20, 0x24, 0x28, 0x2c, 0x30, 0x38, 0x40,
            ],
            min_records: 2,
        }
    }

    /// The same descriptor fields and region-size constraints in a VROM
    /// source domain. Overlay link addresses are not necessarily resident
    /// physical-RDRAM addresses: relocation-capable N64 linkers commonly use
    /// the wider `0x8000_0000..0x8100_0000` kseg0 link window even though the
    /// loaded image occupies less physical RDRAM at runtime. `delta_vote`
    /// still has to derive the unique address for every admitted region.
    pub fn vrom_family() -> Self {
        let mut config = Self::aki_family();
        // Small effect/utility overlays remain real code images. The 0x80
        // floor includes bounded leaf images while descriptor-rooted CFG
        // validation, not the 4 KiB AKI size floor, discriminates code.
        config.min_region_len = 0x80;
        config.vram_hi = 0x8100_0000;
        config
    }
}

/// One parsed record's ROM interval and destination VA, before the table it
/// belongs to is judged.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct CandidateRecord {
    pub rom_start: u32,
    pub rom_end: u32,
    /// Normalized destination start. [`CandidateTable::destination_field`]
    /// records whether the source field held this value directly or held the
    /// corresponding exclusive end.
    pub vram_dest: u32,
}

impl CandidateRecord {
    /// The overlay's ROM byte length. Named `byte_len` rather than `len` so it
    /// reads as a size, not a container length.
    pub fn byte_len(&self) -> u32 {
        self.rom_end - self.rom_start
    }
}

/// Meaning of the third address field in a physical overlay descriptor.
///
/// Both layouts occur in ROMs. Keeping the interpretation on the candidate
/// table prevents an end address from being silently treated as a start while
/// downstream records continue to consume one normalized destination start.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DestinationFieldSemantics {
    #[default]
    Start,
    ExclusiveEnd,
}

impl DestinationFieldSemantics {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Start => "start",
            Self::ExclusiveEnd => "exclusive_end",
        }
    }
}

/// A candidate table: where it sits, its shape, and the records it yields.
/// Identity for canonicalization is the destination-field semantics plus the
/// normalized record geometry, not the `(offset, field)` phase -- a table read
/// at `+0` with `field_rom_start = 0` and the same table read one word earlier
/// with `field_rom_start = 4` describe the identical overlays and must not both
/// count.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CandidateTable {
    pub table_rom_offset: u32,
    pub record_stride: u32,
    pub field_rom_start: u32,
    pub field_rom_end: u32,
    pub field_vram_dest: u32,
    #[serde(default)]
    pub destination_field: DestinationFieldSemantics,
    pub records: Vec<CandidateRecord>,
}

impl CandidateTable {
    /// The ordered ROM-interval set this table proposes -- its canonical
    /// identity.
    pub fn interval_set(&self) -> Vec<(u32, u32)> {
        let mut intervals: Vec<(u32, u32)> = self
            .records
            .iter()
            .map(|r| (r.rom_start, r.rom_end))
            .collect();
        intervals.sort_unstable();
        intervals.dedup();
        intervals
    }
}

/// Per-record predicate: in-bounds, ordered, code-sized, plausible RDRAM
/// destination. Pure; no promotion.
fn record_valid(rec: &CandidateRecord, rom_len: u32, config: &SearchConfig) -> bool {
    rec.rom_start >= config.min_rom_offset
        && rec.rom_end > rec.rom_start
        && rec.rom_end <= rom_len
        && rec.rom_start.is_multiple_of(4)
        && rec.rom_end.is_multiple_of(4)
        && rec.byte_len() >= config.min_region_len
        && rec.byte_len() <= config.max_region_len
        && rec.vram_dest >= config.vram_lo
        && rec.vram_dest < config.vram_hi
        && rec.vram_dest.is_multiple_of(4)
}

fn normalize_record_destination(
    mut record: CandidateRecord,
    semantics: DestinationFieldSemantics,
) -> Option<CandidateRecord> {
    let byte_len = record.rom_end.checked_sub(record.rom_start)?;
    record.vram_dest = match semantics {
        DestinationFieldSemantics::Start => record.vram_dest,
        DestinationFieldSemantics::ExclusiveEnd => record.vram_dest.checked_sub(byte_len)?,
    };
    Some(record)
}

/// Records are non-overlapping when, sorted by start, no interval reaches into
/// the next. Disjoint or exactly-abutting are both accepted (AKI tables chain
/// `rom_end[i] == rom_start[i+1]`); a true overlap is rejected.
fn intervals_non_overlapping(records: &[CandidateRecord]) -> bool {
    let mut intervals: Vec<(u32, u32)> = records.iter().map(|r| (r.rom_start, r.rom_end)).collect();
    intervals.sort_unstable();
    intervals.windows(2).all(|w| w[0].1 <= w[1].0)
}

fn read_u32_be(bytes: &[u8], offset: usize) -> Option<u32> {
    let slice = bytes.get(offset..offset + 4)?;
    Some(u32::from_be_bytes(slice.try_into().ok()?))
}

/// Enumerate every family table in `rom_bytes` and return one
/// [`CandidateTable`] per *distinct* proposed interval set (phase aliases
/// collapsed). Deterministic order: by table offset, then stride, then field
/// layout.
///
/// This is the raw enumeration -- it does not yet apply the delta_vote
/// uniqueness filter (see [`recover_overlay_regions`]). Its job is to find
/// everything the family shape admits so the filter has the full candidate set
/// to disambiguate.
pub fn enumerate_family_tables(rom_bytes: &[u8], config: &SearchConfig) -> Vec<CandidateTable> {
    let rom_len = rom_bytes.len() as u32;
    let mut raw: Vec<CandidateTable> = Vec::new();

    // A candidate record's three adjacent words do not depend on the table
    // stride or field phase. Index the sparse set of valid triples once. The
    // exhaustive search below still visits every stride/phase combination,
    // but it no longer rereads and revalidates the whole ROM for each one.
    let last_record_start = rom_bytes.len().saturating_sub(12);
    let valid_record_offsets = (config.min_rom_offset as usize..=last_record_start)
        .step_by(4)
        .filter_map(|offset| {
            let raw = CandidateRecord {
                rom_start: read_u32_be(rom_bytes, offset)?,
                rom_end: read_u32_be(rom_bytes, offset + 4)?,
                vram_dest: read_u32_be(rom_bytes, offset + 8)?,
            };
            [
                DestinationFieldSemantics::Start,
                DestinationFieldSemantics::ExclusiveEnd,
            ]
            .into_iter()
            .filter_map(|semantics| normalize_record_destination(raw, semantics))
            .any(|record| record_valid(&record, rom_len, config))
            .then_some(offset as u32)
        })
        .collect::<Vec<_>>();
    let read_valid_record =
        |offset: u32, semantics: DestinationFieldSemantics| -> Option<CandidateRecord> {
            let offset = offset as usize;
            let raw = CandidateRecord {
                rom_start: read_u32_be(rom_bytes, offset)?,
                rom_end: read_u32_be(rom_bytes, offset + 4)?,
                vram_dest: read_u32_be(rom_bytes, offset + 8)?,
            };
            let record = normalize_record_destination(raw, semantics)?;
            record_valid(&record, rom_len, config).then_some(record)
        };

    for &stride in &config.strides {
        if stride < 12 || !stride.is_multiple_of(4) {
            continue;
        }
        // rom_end must immediately follow rom_start (adjacent u32 fields), and
        // vram_dest must immediately follow rom_end -- the AKI family layout.
        // The field_rom_start slides across the record so a table whose first
        // field is not a rom_start is still found at the right phase.
        let max_field_start = stride - 12; // room for start, end, vram
        let mut field_rom_start = 0u32;
        while field_rom_start <= max_field_start {
            let field_rom_end = field_rom_start + 4;
            let field_vram_dest = field_rom_start + 8;

            for destination_field in [
                DestinationFieldSemantics::Start,
                DestinationFieldSemantics::ExclusiveEnd,
            ] {
                let last_table_start =
                    rom_len.saturating_sub(stride.saturating_mul(config.min_records));
                let mut next_offset = config.min_rom_offset;
                for &record_offset in &valid_record_offsets {
                    let Some(offset) = record_offset.checked_sub(field_rom_start) else {
                        continue;
                    };
                    if offset < next_offset || offset > last_table_start {
                        continue;
                    }

                    let mut records = Vec::new();
                    let mut base = offset;
                    while base <= rom_len.saturating_sub(stride) {
                        match read_valid_record(base + field_rom_start, destination_field) {
                            Some(rec) => {
                                records.push(rec);
                                base += stride;
                            }
                            _ => break,
                        }
                    }

                    if records.len() as u32 >= config.min_records
                        && intervals_non_overlapping(&records)
                    {
                        raw.push(CandidateTable {
                            table_rom_offset: offset,
                            record_stride: stride,
                            field_rom_start,
                            field_rom_end,
                            field_vram_dest,
                            destination_field,
                            records,
                        });
                        // Skip past the consumed run: overlapping sub-runs of
                        // the same table are not independent discoveries.
                        next_offset = base;
                    } else {
                        next_offset = offset + 4;
                    }
                }
            }
            field_rom_start += 4;
        }
    }

    canonicalize(raw)
}

/// Collapse phase aliases: keep one table per distinct normalized geometry and
/// destination-field semantics, choosing the lowest `table_rom_offset` (then
/// the smallest field layout) as the canonical representative. Deterministic.
fn canonicalize(mut raw: Vec<CandidateTable>) -> Vec<CandidateTable> {
    raw.sort_by(|a, b| {
        a.table_rom_offset
            .cmp(&b.table_rom_offset)
            .then(a.record_stride.cmp(&b.record_stride))
            .then(a.field_rom_start.cmp(&b.field_rom_start))
    });
    let mut seen: BTreeSet<(DestinationFieldSemantics, Vec<(u32, u32, u32)>)> = BTreeSet::new();
    let mut out = Vec::new();
    for table in raw {
        let geometry = table
            .records
            .iter()
            .map(|record| (record.rom_start, record.rom_end, record.vram_dest))
            .collect();
        if seen.insert((table.destination_field, geometry)) {
            out.push(table);
        }
    }
    // Final order: by first proposed interval, so the report is stable and
    // independent of which phase won the alias race.
    out.sort_by_key(|table| table.interval_set());
    out
}

/// Why a candidate table was or was not admitted, with the delta_vote outcome
/// per region so "admitted" and "rejected" are both measurements.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TableAdmission {
    pub table: CandidateTable,
    /// One entry per region: `Some((delta, va_start))` when delta_vote
    /// admitted a unique mapping or the descriptor hypothesis was
    /// independently corroborated and unique, `None` when it stayed open.
    pub region_deltas: Vec<Option<(u32, u32)>>,
    /// Regions mapped by delta_vote or the descriptor-corroborated fallback.
    pub mapped_regions: u32,
    /// Admitted iff `mapped_regions >= min_mapped_regions` under the selected
    /// destination-field semantics (see [`recover_overlay_regions`]). Exact
    /// delta agreement selects between competing interpretations; a family
    /// with no agreement retains the established start-address candidate path,
    /// which still cannot produce a proven mapping without per-record
    /// agreement downstream.
    pub admitted: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct MappingHypothesis {
    source_start: u32,
    source_end: u32,
    va_start: u32,
    va_end: u32,
}

impl MappingHypothesis {
    fn from_descriptor(record: CandidateRecord, config: &SearchConfig) -> Option<Self> {
        let va_end = record.vram_dest.checked_add(record.byte_len())?;
        (record.vram_dest >= config.vram_lo && va_end <= config.vram_hi).then_some(Self {
            source_start: record.rom_start,
            source_end: record.rom_end,
            va_start: record.vram_dest,
            va_end,
        })
    }

    fn from_delta(record: CandidateRecord, va_start: u32) -> Option<Self> {
        Some(Self {
            source_start: record.rom_start,
            source_end: record.rom_end,
            va_start,
            va_end: va_start.checked_add(record.byte_len())?,
        })
    }

    fn overlaps_va(self, other: Self) -> bool {
        self.va_start < other.va_end && other.va_start < self.va_end
    }
}

/// The precise rule-(3) blocker for a descriptor-rooted CFG. Kept typed so a
/// caller can report why a record stayed open without re-running discovery or
/// consulting a grading key.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DescriptorCfgFailure {
    EmptyOrMisalignedRegion,
    SourceLengthMismatch,
    NoDecodedCode,
    EmptyCfg,
    InvalidInstruction { pc: u32, word: u32 },
    MissingDelaySlot { control_pc: u32 },
    RanOffEnd { block_start: u32 },
    OutOfRangeTarget { block_start: u32, target: u32 },
    OutOfRangeFallthrough { block_start: u32, next: u32 },
}

/// Which descriptor-corroboration rule left a VROM record open.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DescriptorMappingFailure {
    /// The source interval did not materialize through exactly one recovered
    /// file-table record.
    Rule1SourceMaterialization,
    /// The aligned descriptor destination and checked end did not fit wholly
    /// inside the configured link-VA window.
    Rule2DestinationRange,
    /// Recursive descent from the descriptor VA did not establish a bounded,
    /// blocker-free real-code CFG.
    Rule3Cfg(DescriptorCfgFailure),
    /// A distinct mapping hypothesis overlapped the proposed VA interval.
    Rule4VaConflict,
}

/// Per-record mapping outcome for the VROM path. `region_diagnostics` stays
/// parallel to `region_deltas`, making every open record attributable to one
/// failed corroboration rule.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum VromRecordMappingDiagnostic {
    DeltaVote,
    DeltaVoteAndDescriptorCorroborated,
    DescriptorCorroborated,
    /// This record passed on its own, but a sibling in the same below-floor
    /// table did not. Partial descriptor evidence never mints a table.
    DescriptorCorroboratedTableIncomplete,
    Open(DescriptorMappingFailure),
}

/// Validate the descriptor-derived mapping without using delta_vote or a
/// grading key. The descriptor supplies only the root VA; the region's own
/// reachable instructions must establish a complete, blocker-free CFG.
#[doc(hidden)]
pub fn descriptor_mapping_corroborated_probe(
    region_bytes: &[u8],
    rom_start: u32,
    rom_end: u32,
    vram_dest: u32,
) -> Result<(), DescriptorCfgFailure> {
    descriptor_mapping_corroborated(
        region_bytes,
        MappingHypothesis {
            source_start: rom_start,
            source_end: rom_end,
            va_start: vram_dest,
            va_end: vram_dest.wrapping_add(rom_end - rom_start),
        },
    )
}

fn descriptor_mapping_corroborated(
    region_bytes: &[u8],
    hypothesis: MappingHypothesis,
) -> Result<(), DescriptorCfgFailure> {
    if region_bytes.is_empty() || !region_bytes.len().is_multiple_of(4) {
        return Err(DescriptorCfgFailure::EmptyOrMisalignedRegion);
    }
    if region_bytes.len() != (hypothesis.source_end - hypothesis.source_start) as usize {
        return Err(DescriptorCfgFailure::SourceLengthMismatch);
    }

    let cfg = build_cfg(
        "descriptor_mapping_hypothesis",
        region_bytes,
        hypothesis.va_start,
        &[hypothesis.va_start],
    );
    let in_range = |va: u32| {
        va >= hypothesis.va_start
            && va < hypothesis.va_end
            && (va - hypothesis.va_start).is_multiple_of(4)
    };
    let has_code = cfg
        .word_class
        .values()
        .any(|class| *class == WordClass::ProvenCode);
    if !has_code {
        return Err(DescriptorCfgFailure::NoDecodedCode);
    }
    if cfg.blocks.is_empty() {
        return Err(DescriptorCfgFailure::EmptyCfg);
    }
    for block in &cfg.blocks {
        let failure = match &block.terminator {
            BlockTerminator::InvalidInstruction { pc, word } => {
                Some(DescriptorCfgFailure::InvalidInstruction {
                    pc: *pc,
                    word: *word,
                })
            }
            BlockTerminator::MissingDelaySlot { control_pc } => {
                Some(DescriptorCfgFailure::MissingDelaySlot {
                    control_pc: *control_pc,
                })
            }
            BlockTerminator::RanOffEnd
            | BlockTerminator::DataFence { .. }
            | BlockTerminator::SelfReferentialBranch { .. } => {
                Some(DescriptorCfgFailure::RanOffEnd {
                    block_start: block.start_va,
                })
            }
            BlockTerminator::Branch {
                target,
                fallthrough,
                ..
            }
            | BlockTerminator::BranchLikely {
                target,
                fallthrough,
                ..
            } => {
                if !in_range(*target) {
                    Some(DescriptorCfgFailure::OutOfRangeTarget {
                        block_start: block.start_va,
                        target: *target,
                    })
                } else if !in_range(*fallthrough) {
                    Some(DescriptorCfgFailure::OutOfRangeFallthrough {
                        block_start: block.start_va,
                        next: *fallthrough,
                    })
                } else {
                    None
                }
            }
            BlockTerminator::Call { next, .. } | BlockTerminator::Fallthrough { next } => {
                (!in_range(*next)).then_some(DescriptorCfgFailure::OutOfRangeFallthrough {
                    block_start: block.start_va,
                    next: *next,
                })
            }
            BlockTerminator::Tail { .. }
            | BlockTerminator::Return
            | BlockTerminator::Indirect { .. }
            | BlockTerminator::ResolvedIndirect { .. }
            | BlockTerminator::Trap => None,
        };
        if let Some(failure) = failure {
            return Err(failure);
        }
    }
    Ok(())
}

#[derive(Debug, Clone, Copy)]
struct DescriptorFallback {
    admission_index: usize,
    record_index: usize,
    mapping: MappingHypothesis,
}

/// Distinct ROM sources that must converge on one destination before the
/// overlap is read as a reused RAM slot rather than a contradiction. Two
/// regions sharing a VA is ordinary ambiguity; many independent ROM images
/// declaring one destination is a loader design.
const MIN_SWAPPED_SOURCES_PER_DESTINATION: usize = 8;

/// Destinations-to-sources ratio below which the shape is a reused slot.
/// A swapping engine concentrates many sources onto few destinations; a table
/// of coincidental pointer pairs scatters instead.
const MAX_SWAPPED_DESTINATION_SHARE: usize = 4;

/// Whether these hypotheses describe an overlay-swapping engine: many distinct
/// ROM images declaring few destinations, at least one of which is claimed by
/// many sources.
///
/// Deliberately strict. Coincidental descriptor tables produce scattered
/// destinations, so requiring heavy concentration keeps VA uniqueness as the
/// rule everywhere it is the right rule. Every candidate here has already
/// passed [`descriptor_mapping_corroborated`], so each region decodes as code
/// at its declared VA -- this only decides whether their *overlap* is fatal.
fn overlapping_destination_is_engine_shape(fallbacks: &[DescriptorFallback]) -> bool {
    let mut sources_by_destination: BTreeMap<u32, BTreeSet<(u32, u32)>> = BTreeMap::new();
    for fallback in fallbacks {
        sources_by_destination
            .entry(fallback.mapping.va_start)
            .or_default()
            .insert((fallback.mapping.source_start, fallback.mapping.source_end));
    }
    concentrated_swap_shape(&sources_by_destination)
}

/// The concentration test alone: many distinct ROM images declaring few
/// destinations, at least one claimed by many sources.
fn concentrated_swap_shape(
    sources_by_destination: &BTreeMap<u32, BTreeSet<(u32, u32)>>,
) -> bool {
    let distinct_sources: usize = sources_by_destination
        .values()
        .map(|sources| sources.len())
        .sum();
    let busiest = sources_by_destination
        .values()
        .map(|sources| sources.len())
        .max()
        .unwrap_or(0);
    busiest >= MIN_SWAPPED_SOURCES_PER_DESTINATION
        && distinct_sources >= sources_by_destination.len() * MAX_SWAPPED_DESTINATION_SHARE
}

/// Whether two mappings' ROM sources are immediate neighbours -- one ends
/// exactly where the other begins. Swap-pair siblings tile a contiguous span;
/// an unrelated image that merely lands on the same VA does not.
fn sources_abut(left: MappingHypothesis, right: MappingHypothesis) -> bool {
    left.source_end == right.source_start || right.source_end == left.source_start
}

/// [`overlapping_destination_is_engine_shape`] over both evidence sets: the
/// descriptor fallbacks and the raw delta-vote mappings. A swap pair splits
/// its evidence across the two (one sibling votes, the other falls back), so
/// concentration and contiguity must be measured on the union.
/// Concentration OR small-swap-pair contiguity, measured across both
/// evidence sets.
///
/// A game with exactly two mutually exclusive overlays concentrates 2 sources
/// on 1 destination -- an order of magnitude under
/// [`MIN_SWAPPED_SOURCES_PER_DESTINATION`], so concentration cannot carry the
/// evidence at that size and contiguity does instead: coincidental pointer
/// pairs do not abut, while a partitioned ROM span whose pieces each decode as
/// code at one shared VA is a swap pair rather than a contradiction (measured:
/// WCW/nWo Revenge, ROM 0x3c770..0x834a0 and 0x834a0..0xdac50, both declaring
/// 0x80090000).
fn overlapping_destination_is_engine_shape_across(
    fallbacks: &[DescriptorFallback],
    raw_mappings: &BTreeSet<MappingHypothesis>,
) -> bool {
    let mut sources_by_destination: BTreeMap<u32, BTreeSet<(u32, u32)>> = BTreeMap::new();
    for fallback in fallbacks {
        sources_by_destination
            .entry(fallback.mapping.va_start)
            .or_default()
            .insert((fallback.mapping.source_start, fallback.mapping.source_end));
    }
    for mapping in raw_mappings {
        sources_by_destination
            .entry(mapping.va_start)
            .or_default()
            .insert((mapping.source_start, mapping.source_end));
    }
    let distinct_sources: usize = sources_by_destination
        .values()
        .map(|sources| sources.len())
        .sum();
    let busiest = sources_by_destination
        .values()
        .map(|sources| sources.len())
        .max()
        .unwrap_or(0);
    if busiest >= MIN_SWAPPED_SOURCES_PER_DESTINATION
        && distinct_sources >= sources_by_destination.len() * MAX_SWAPPED_DESTINATION_SHARE
    {
        return true;
    }
    small_contiguous_swap_pair(&sources_by_destination)
}

/// Whether one destination is claimed only by ROM sources that exactly tile a
/// contiguous span -- each source's end is the next source's start.
///
/// Deliberately narrow: every source for the destination must participate (a
/// stray extra claimant means the overlap is ambiguity again), the sources
/// must be non-degenerate, and the tiling must be exact. Each region has
/// already passed [`descriptor_mapping_corroborated`], so it decodes as code
/// at the declared VA; this only decides whether the shared destination is a
/// design or a contradiction.
fn small_contiguous_swap_pair(
    sources_by_destination: &BTreeMap<u32, BTreeSet<(u32, u32)>>,
) -> bool {
    sources_by_destination.values().any(|sources| {
        if sources.len() < 2 {
            return false;
        }
        let ordered: Vec<(u32, u32)> = sources.iter().copied().collect();
        ordered.iter().all(|(start, end)| end > start)
            && ordered
                .windows(2)
                .all(|pair| pair[0].1 == pair[1].0)
    })
}

/// Admit only hypotheses whose VA interval is unique across distinct region
/// identities. Exact duplicate records from phase/stride aliases are one
/// hypothesis, not conflicts; different source regions sharing any VA byte
/// reject each other symmetrically, independent of iteration order.
///
/// VA uniqueness is the wrong test for an overlay-*swapping* engine, which
/// loads many ROM images into one reused RAM slot at different times. There
/// the descriptor's own `vram_dest` is the authority and overlap is the
/// design, not a contradiction: measured on Paper Mario (PAL), 319 of 322
/// records in admitted tables declare the same `0x80240000` destination, so
/// symmetric rejection discards every one of them.
///
/// [`overlapping_destination_is_engine_shape`] recognizes that case. The
/// safety property is unchanged for every other engine: a hypothesis is still
/// admitted only when its region decodes as code at the declared VA, and a
/// *raw* delta-vote mapping still wins over any conflicting fallback.
fn apply_unique_descriptor_fallbacks<A>(
    admissions: &mut [A],
    raw_mappings: &BTreeSet<MappingHypothesis>,
    fallbacks: Vec<DescriptorFallback>,
    region_deltas: impl FnMut(&mut A) -> &mut Vec<Option<(u32, u32)>>,
    mapped_regions: impl FnMut(&mut A) -> &mut u32,
) -> BTreeSet<(usize, usize)> {
    apply_unique_descriptor_fallbacks_with_swap_pairs(
        admissions,
        raw_mappings,
        fallbacks,
        region_deltas,
        mapped_regions,
        false,
    )
}

/// [`apply_unique_descriptor_fallbacks`], with the small-swap-pair exemption
/// selectable.
///
/// The exemption is offered only on the physical descriptor path. The VROM
/// path resolves its own file table first, so an abutting source pair there is
/// ordinary adjacency in a file list rather than evidence of a reused slot,
/// and its stricter VA-uniqueness rule (`Rule4VaConflict`) stands unchanged.
fn apply_unique_descriptor_fallbacks_with_swap_pairs<A>(
    admissions: &mut [A],
    raw_mappings: &BTreeSet<MappingHypothesis>,
    fallbacks: Vec<DescriptorFallback>,
    mut region_deltas: impl FnMut(&mut A) -> &mut Vec<Option<(u32, u32)>>,
    mut mapped_regions: impl FnMut(&mut A) -> &mut u32,
    allow_small_swap_pairs: bool,
) -> BTreeSet<(usize, usize)> {
    // Shape detection spans BOTH evidence sets. A two-overlay swap pair
    // typically has one record win a raw delta vote while its sibling stays
    // open, so a fallbacks-only view sees a single claimant and can never
    // recognize the pair (measured on WCW/nWo Revenge: record 1 votes, record
    // 2 falls back, both declare 0x80090000).
    let swapping_engine = if allow_small_swap_pairs {
        overlapping_destination_is_engine_shape_across(&fallbacks, raw_mappings)
    } else {
        overlapping_destination_is_engine_shape(&fallbacks)
    };
    let admitted: Vec<_> = fallbacks
        .iter()
        .filter(|candidate| {
            // A delta-vote mapping is independently derived evidence, so it
            // outranks a declared destination -- except where the overlap IS
            // the design. In a swap pair the raw mapping and the declared
            // destination agree about the slot and differ only in which ROM
            // image occupies it, so treating the raw claim as exclusive
            // discards the sibling that shares it by construction.
            let conflicts_with_raw = raw_mappings.iter().any(|mapping| {
                *mapping != candidate.mapping
                    && mapping.overlaps_va(candidate.mapping)
                    // ...unless the two ROM images abut. A swap pair's members
                    // tile one contiguous ROM span and share one slot by
                    // design, so the raw claim does not contradict its
                    // sibling; an unrelated source landing on the same VA is
                    // still a contradiction and still rejects.
                    && !(allow_small_swap_pairs && swapping_engine && sources_abut(*mapping, candidate.mapping))
            });
            let conflicts_with_fallback = !swapping_engine
                && fallbacks.iter().any(|other| {
                    other.mapping != candidate.mapping
                        && other.mapping.overlaps_va(candidate.mapping)
                });
            !conflicts_with_raw && !conflicts_with_fallback
        })
        .collect();

    let admitted_keys: BTreeSet<_> = admitted
        .iter()
        .map(|candidate| (candidate.admission_index, candidate.record_index))
        .collect();
    for candidate in admitted {
        let admission = &mut admissions[candidate.admission_index];
        let slot = &mut region_deltas(admission)[candidate.record_index];
        if slot.is_none() {
            *slot = Some((
                candidate
                    .mapping
                    .va_start
                    .wrapping_sub(candidate.mapping.source_start),
                candidate.mapping.va_start,
            ));
            *mapped_regions(admission) += 1;
        }
    }
    admitted_keys
}

/// The full result: the raw family candidates, then the delta_vote-filtered
/// admissions, so a grader sees the before/after tightening the discipline
/// requires (never ship a loose scanner).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OverlayRecovery {
    pub config: SearchConfig,
    pub delta_config: DeltaVoteConfig,
    pub min_mapped_regions: u32,
    /// Distinct candidate tables from the family search (phase aliases
    /// already collapsed).
    pub candidate_tables: Vec<CandidateTable>,
    /// Each candidate table judged by delta_vote admissibility.
    pub admissions: Vec<TableAdmission>,
}

impl OverlayRecovery {
    /// The overlay ROM intervals from admitted tables only -- the regions a
    /// downstream phase (or `gate_delta_vote`) may consume. Deterministic
    /// order.
    pub fn admitted_intervals(&self) -> Vec<(u32, u32)> {
        let mut intervals: Vec<(u32, u32)> = self
            .admissions
            .iter()
            .filter(|a| a.admitted)
            .flat_map(|a| a.table.interval_set())
            .collect();
        intervals.sort_unstable();
        intervals.dedup();
        intervals
    }
}

/// Recover candidate overlay regions from ROM bytes alone, then tighten with
/// delta_vote admissibility as the uniqueness filter.
///
/// `min_mapped_regions` is how many of a table's regions delta_vote must
/// uniquely map for the table to be admitted. When the family search returns
/// exactly one distinct candidate table this filter is a corroboration (the
/// single table is reported with its delta_vote outcomes); when it returns
/// more than one, the filter is what disambiguates them -- a table of
/// coincidental pointer pairs will not have regions that decode as MIPS whose
/// distinct `jal` targets land on prologues under one unique delta.
///
/// Pure function of the ROM bytes and configuration.
pub fn recover_overlay_regions(
    rom_bytes: &[u8],
    config: &SearchConfig,
    delta_config: &DeltaVoteConfig,
    min_mapped_regions: u32,
) -> OverlayRecovery {
    let candidate_tables = enumerate_family_tables(rom_bytes, config);

    admit_overlay_region_tables(
        rom_bytes,
        config,
        delta_config,
        min_mapped_regions,
        candidate_tables,
    )
}

/// Apply delta-vote admission to an already enumerated physical descriptor
/// family. Keeping enumeration and admission separately callable lets build
/// tooling reuse or profile the exhaustive ROM scan without weakening the
/// proof rule: the candidates remain explicit inputs and the returned report
/// retains the complete searched configuration.
pub fn admit_overlay_region_tables(
    rom_bytes: &[u8],
    config: &SearchConfig,
    delta_config: &DeltaVoteConfig,
    min_mapped_regions: u32,
    candidate_tables: Vec<CandidateTable>,
) -> OverlayRecovery {
    let mut admissions: Vec<TableAdmission> = candidate_tables
        .iter()
        .map(|table| {
            let mut region_deltas = Vec::with_capacity(table.records.len());
            let mut mapped = 0u32;
            for rec in &table.records {
                let outcome = rom_bytes
                    .get(rec.rom_start as usize..rec.rom_end as usize)
                    .map(|region| infer_region_delta(region, rec.rom_start, &[], delta_config));
                match outcome.map(|r| r.outcome) {
                    Some(DeltaVoteOutcome::Admitted { delta, va_start }) => {
                        mapped += 1;
                        region_deltas.push(Some((delta, va_start)));
                    }
                    _ => region_deltas.push(None),
                }
            }
            TableAdmission {
                admitted: mapped >= min_mapped_regions,
                mapped_regions: mapped,
                region_deltas,
                table: table.clone(),
            }
        })
        .collect();

    // Select the raw field interpretation before descriptor-CFG fallback.
    // Otherwise the start and end hypotheses for the same source interval
    // overlap by construction and reject each other as non-unique. Exact
    // delta agreement is independent of the descriptor field and therefore
    // breaks that ambiguity. A nonzero tie has no unique interpretation and
    // admits neither. With no delta agreement at all, retain the established
    // start-address interpretation so descriptor-only corroboration keeps its
    // prior proof boundary; an end-address layout requires positive evidence.
    let agreements = |semantics| {
        admissions
            .iter()
            .filter(|admission| admission.table.destination_field == semantics)
            .flat_map(|admission| admission.table.records.iter().zip(&admission.region_deltas))
            .filter_map(|(record, outcome)| {
                outcome
                    .is_some_and(|(_, va_start)| va_start == record.vram_dest)
                    .then_some((record.rom_start, record.rom_end, record.vram_dest))
            })
            .collect::<BTreeSet<_>>()
    };
    let start_agreements = agreements(DestinationFieldSemantics::Start);
    let end_agreements = agreements(DestinationFieldSemantics::ExclusiveEnd);
    let selected_destination_field = match start_agreements.len().cmp(&end_agreements.len()) {
        std::cmp::Ordering::Greater => Some(DestinationFieldSemantics::Start),
        std::cmp::Ordering::Less => Some(DestinationFieldSemantics::ExclusiveEnd),
        std::cmp::Ordering::Equal if start_agreements.is_empty() => {
            Some(DestinationFieldSemantics::Start)
        }
        std::cmp::Ordering::Equal => None,
    };

    // Descriptor corroboration is offered to EVERY candidate table, not only
    // to tables delta_vote already admitted. Gating it on `admitted` was a
    // deadlock: `admitted` is computed from delta-vote outcomes alone, so a
    // table whose regions all stay open can never reach the fallback that
    // would resolve them. Measured on Hot Wheels - Turbo Racing, where 0 of
    // 136 records vote but 53 decode as code at their declared destination:
    // all 21 tables scored `mapped_regions = 0` and the 53 were discarded.
    //
    // This widens what counts as corroboration, not the admission bar itself
    // -- a region still only counts once it decodes as code at the declared
    // VA, and `min_mapped_regions` is re-checked below against the total.
    let mut raw_mappings = BTreeSet::new();
    let mut fallbacks = Vec::new();
    for (admission_index, admission) in admissions.iter().enumerate() {
        if Some(admission.table.destination_field) != selected_destination_field {
            continue;
        }
        for (record_index, (record, outcome)) in admission
            .table
            .records
            .iter()
            .zip(&admission.region_deltas)
            .enumerate()
        {
            match *outcome {
                Some((_, va_start)) => {
                    if let Some(mapping) = MappingHypothesis::from_delta(*record, va_start) {
                        raw_mappings.insert(mapping);
                    }
                }
                None => {
                    let Some(mapping) = MappingHypothesis::from_descriptor(*record, config) else {
                        continue;
                    };
                    let Some(region_bytes) =
                        rom_bytes.get(record.rom_start as usize..record.rom_end as usize)
                    else {
                        continue;
                    };
                    if descriptor_mapping_corroborated(region_bytes, mapping).is_ok() {
                        fallbacks.push(DescriptorFallback {
                            admission_index,
                            record_index,
                            mapping,
                        });
                    }
                }
            }
        }
    }
    let _ = apply_unique_descriptor_fallbacks_with_swap_pairs(
        &mut admissions,
        &raw_mappings,
        fallbacks,
        |admission| &mut admission.region_deltas,
        |admission| &mut admission.mapped_regions,
        true,
    );

    // Admission was decided before the fallbacks ran, so re-derive it against
    // the regions now corroborated. Fragment and stride aliases can each hold
    // only a subset of the matching records; preserve the ordinary per-
    // fragment mapped-region floor within the selected interpretation.
    for admission in &mut admissions {
        admission.admitted = admission.mapped_regions >= min_mapped_regions
            && Some(admission.table.destination_field) == selected_destination_field;
    }

    // A fully-mapped table outranks a partially-mapped one. Lowering the
    // record floor to admit genuine two-overlay games also lets a short table
    // clear the bar with records left open, and a weaker table competing with
    // a complete one is not ambiguity to preserve -- downstream
    // (`scan_recovered_overlay_regions`) requires exactly ONE admitted table,
    // so a spurious partial admission silently costs the whole game its
    // overlays (measured on WWF WrestleMania 2000: the real 4-record table
    // maps 4/4 while a 3-record neighbour maps 2/3, and admitting both dropped
    // NWXE recall by 122 functions).
    //
    // Completeness, not length, is the discriminator: a short table that maps
    // every record it declares is exactly the two-overlay shape this floor
    // exists to admit.
    let any_complete = admissions
        .iter()
        .any(|admission| admission.admitted && admission.mapped_regions as usize == admission.table.records.len());
    if any_complete {
        for admission in &mut admissions {
            if admission.mapped_regions as usize != admission.table.records.len() {
                admission.admitted = false;
            }
        }
    }

    OverlayRecovery {
        config: config.clone(),
        delta_config: *delta_config,
        min_mapped_regions,
        candidate_tables,
        admissions,
    }
}

/// A descriptor-family table whose location is in VROM. Its records retain
/// logical VROM source intervals; only the location field differs from the
/// physical [`CandidateTable`] path above.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VromCandidateTable {
    pub table_vrom_offset: u32,
    pub record_stride: u32,
    pub field_rom_start: u32,
    pub field_rom_end: u32,
    pub field_vram_dest: u32,
    pub records: Vec<CandidateRecord>,
}

impl VromCandidateTable {
    pub fn interval_set(&self) -> Vec<(u32, u32)> {
        let mut intervals: Vec<_> = self
            .records
            .iter()
            .map(|record| (record.rom_start, record.rom_end))
            .collect();
        intervals.sort_unstable();
        intervals.dedup();
        intervals
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VromTableAdmission {
    pub table: VromCandidateTable,
    pub region_deltas: Vec<Option<(u32, u32)>>,
    /// Parallel per-record reason path. An open mapping always names the
    /// failed descriptor rule; callers never have to infer it from a score.
    pub region_diagnostics: Vec<VromRecordMappingDiagnostic>,
    pub mapped_regions: u32,
    /// Engine-independent ordinary evidence minimum supplied by the caller.
    /// A below-floor table needs complete descriptor corroboration and global
    /// uniqueness instead.
    pub required_mapped_regions: u32,
    pub admitted: bool,
}

/// Result of the two-stage VROM path. The physical AKI recovery remains a
/// separate, unchanged result so callers cannot accidentally reinterpret a
/// physical table location as VROM (or perturb its existing gate output).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VromOverlayRecovery {
    pub file_table: FileTableRecovery,
    pub config: SearchConfig,
    pub delta_config: DeltaVoteConfig,
    /// Complete-file decode cap applied to every recovered VROM source.
    pub materialization_limits: VromMaterializationLimits,
    /// Distinct recovered files withheld because their complete Yaz0 output
    /// exceeded the configured transient decode cap.
    pub decoded_file_limit_hits: Vec<DecodedFileLimitHit>,
    pub vrom_min_records: u32,
    pub min_mapped_regions: u32,
    pub candidate_tables: Vec<VromCandidateTable>,
    pub admissions: Vec<VromTableAdmission>,
}

/// One visible resource frontier from VROM overlay recovery.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct DecodedFileLimitHit {
    pub vrom_start: u32,
    pub vrom_end: u32,
    pub decoded_file_bytes: usize,
    pub max_decoded_file_bytes: usize,
}

fn materialize_vrom_for_recovery(
    file_table: &CandidateFileTable,
    rom_bytes: &[u8],
    start: u32,
    end: u32,
    limits: VromMaterializationLimits,
    limit_hits: &mut BTreeSet<DecodedFileLimitHit>,
) -> Option<Vec<u8>> {
    match file_table.materialize_vrom_range_diagnostic_with_limits(rom_bytes, start, end, limits) {
        Ok(bytes) => Some(bytes),
        Err(VromMaterializationError::DecodedFileLimitExceeded {
            vrom_start,
            vrom_end,
            decoded_file_bytes,
            max_decoded_file_bytes,
        }) => {
            limit_hits.insert(DecodedFileLimitHit {
                vrom_start,
                vrom_end,
                decoded_file_bytes,
                max_decoded_file_bytes,
            });
            None
        }
        Err(VromMaterializationError::Unavailable { .. }) => None,
    }
}

impl VromOverlayRecovery {
    pub fn admitted_intervals(&self) -> Vec<(u32, u32)> {
        let mut intervals: Vec<_> = self
            .admissions
            .iter()
            .filter(|admission| admission.admitted)
            .flat_map(|admission| admission.table.interval_set())
            .collect();
        intervals.sort_unstable();
        intervals.dedup();
        intervals
    }
}

fn vrom_record_valid(
    record: &CandidateRecord,
    file_table: &CandidateFileTable,
    config: &SearchConfig,
) -> bool {
    record.rom_start >= config.min_rom_offset
        && record.rom_end > record.rom_start
        && record.rom_start.is_multiple_of(4)
        && record.rom_end.is_multiple_of(4)
        && record.byte_len() >= config.min_region_len
        && record.byte_len() <= config.max_region_len
        && file_table.contains_vrom_range(record.rom_start, record.rom_end)
        && record.vram_dest >= config.vram_lo
        && record.vram_dest < config.vram_hi
        && record.vram_dest.is_multiple_of(4)
}

fn enumerate_vrom_family_tables(
    rom_bytes: &[u8],
    file_table: &CandidateFileTable,
    config: &SearchConfig,
    vrom_min_records: u32,
    materialization_limits: VromMaterializationLimits,
    limit_hits: &mut BTreeSet<DecodedFileLimitHit>,
) -> Vec<VromCandidateTable> {
    let mut raw = Vec::new();

    for file_record in &file_table.records {
        let Some(file_bytes) = materialize_vrom_for_recovery(
            file_table,
            rom_bytes,
            file_record.vrom_start,
            file_record.vrom_end,
            materialization_limits,
            limit_hits,
        ) else {
            continue;
        };
        let file_len = file_bytes.len() as u32;

        // The three semantic fields are adjacent. Enumerate each plausible
        // triple once, then form constant-stride runs from this sparse map;
        // rescanning every decompressed byte once per stride/field phase would
        // multiply work without adding a distinct interval hypothesis. The
        // canonical VROM location is the first parsed source field (phase 0).
        let mut valid_triples = BTreeMap::new();
        let mut fields_offset = 0u32;
        while fields_offset <= file_len.saturating_sub(12) {
            let record = CandidateRecord {
                rom_start: read_u32_be(&file_bytes, fields_offset as usize).unwrap(),
                rom_end: read_u32_be(&file_bytes, (fields_offset + 4) as usize).unwrap(),
                vram_dest: read_u32_be(&file_bytes, (fields_offset + 8) as usize).unwrap(),
            };
            if vrom_record_valid(&record, file_table, config) {
                valid_triples.insert(fields_offset, record);
            }
            fields_offset += 4;
        }

        let mut covered_triples = BTreeSet::new();

        for &stride in &config.strides {
            if stride < 12 || !stride.is_multiple_of(4) {
                continue;
            }
            for &start in valid_triples.keys() {
                if start
                    .checked_sub(stride)
                    .is_some_and(|previous| valid_triples.contains_key(&previous))
                {
                    continue;
                }
                let mut records = Vec::new();
                let mut record_offsets = Vec::new();
                let mut offset = start;
                while let Some(record) = valid_triples.get(&offset) {
                    records.push(*record);
                    record_offsets.push(offset);
                    let Some(next) = offset.checked_add(stride) else {
                        break;
                    };
                    offset = next;
                }
                if records.len() as u32 >= vrom_min_records && intervals_non_overlapping(&records) {
                    covered_triples.extend(record_offsets);
                    raw.push(VromCandidateTable {
                        table_vrom_offset: file_record.vrom_start + start,
                        record_stride: stride,
                        field_rom_start: 0,
                        field_rom_end: 4,
                        field_vram_dest: 8,
                        records,
                    });
                }
            }
        }

        // A valid triple between the first and last records of established
        // constant-stride tables is a one-record gap candidate. Keeping the
        // surrounding table-family envelope is load-bearing: an arbitrary
        // code-bearing file can contain a coincidental source/end/VA triple
        // whose destination also decodes, so rules 1-4 alone do not prove
        // that an isolated triple outside any descriptor family is a table.
        if let (Some(&first), Some(&last)) = (
            covered_triples.iter().next(),
            covered_triples.iter().next_back(),
        ) {
            for (&offset, &record) in valid_triples.range(first..=last) {
                if covered_triples.contains(&offset) {
                    continue;
                }
                raw.push(VromCandidateTable {
                    table_vrom_offset: file_record.vrom_start + offset,
                    record_stride: 0,
                    field_rom_start: 0,
                    field_rom_end: 4,
                    field_vram_dest: 8,
                    records: vec![record],
                });
            }
        }
    }

    raw.sort_by(|a, b| {
        a.table_vrom_offset
            .cmp(&b.table_vrom_offset)
            .then(a.record_stride.cmp(&b.record_stride))
            .then(a.field_rom_start.cmp(&b.field_rom_start))
    });
    let mut seen = BTreeSet::new();
    let mut canonical = Vec::new();
    for table in raw {
        if seen.insert(table.interval_set()) {
            canonical.push(table);
        }
    }
    canonical.sort_by_key(VromCandidateTable::interval_set);
    canonical
}

/// Recover the physical file table, resolve every materializable VROM file,
/// and run the descriptor-family search over those virtual table locations.
///
/// `vrom_min_records` is the ordinary constant-stride run floor; physical AKI
/// callers retain their established three-record family minimum. A one-record
/// gap inside an established descriptor-family envelope is enumerated
/// separately and can pass only through complete descriptor corroboration and
/// global VA uniqueness. `min_mapped_regions` is the ordinary delta-vote
/// admission floor; a table below it is admitted only when every record passes
/// the stricter descriptor route.
pub fn recover_vrom_overlay_regions(
    rom_bytes: &[u8],
    config: &SearchConfig,
    delta_config: &DeltaVoteConfig,
    file_table_config: &FileTableSearchConfig,
    vrom_min_records: u32,
    min_mapped_regions: u32,
) -> VromOverlayRecovery {
    recover_vrom_overlay_regions_with_limits(
        rom_bytes,
        config,
        delta_config,
        file_table_config,
        vrom_min_records,
        min_mapped_regions,
        VromMaterializationLimits::default(),
    )
}

/// [`recover_vrom_overlay_regions`] with an explicit complete-file decode
/// cap. A file beyond the cap contributes no descriptor candidates or mapping
/// evidence; recovery remains open rather than allocating its declared size.
pub fn recover_vrom_overlay_regions_with_limits(
    rom_bytes: &[u8],
    config: &SearchConfig,
    delta_config: &DeltaVoteConfig,
    file_table_config: &FileTableSearchConfig,
    vrom_min_records: u32,
    min_mapped_regions: u32,
    materialization_limits: VromMaterializationLimits,
) -> VromOverlayRecovery {
    assert!(vrom_min_records >= 2, "a one-record run is not a table");
    let file_table = recover_file_table(rom_bytes, file_table_config);
    let mut decoded_file_limit_hits = BTreeSet::new();
    let candidate_tables = file_table
        .admitted_table
        .as_ref()
        .map_or_else(Vec::new, |table| {
            enumerate_vrom_family_tables(
                rom_bytes,
                table,
                config,
                vrom_min_records,
                materialization_limits,
                &mut decoded_file_limit_hits,
            )
        });
    let mut admissions: Vec<VromTableAdmission> = candidate_tables
        .iter()
        .map(|table| {
            let admitted_file_table = file_table
                .admitted_table
                .as_ref()
                .expect("VROM candidates require one admitted file table");
            let mut mapped_regions = 0u32;
            let mut region_diagnostics = Vec::with_capacity(table.records.len());
            let region_deltas = table
                .records
                .iter()
                .map(|record| {
                    let outcome = materialize_vrom_for_recovery(
                        admitted_file_table,
                        rom_bytes,
                        record.rom_start,
                        record.rom_end,
                        materialization_limits,
                        &mut decoded_file_limit_hits,
                    )
                    .map(|bytes| {
                        infer_region_delta(&bytes, record.rom_start, &[], delta_config).outcome
                    });
                    match outcome {
                        Some(DeltaVoteOutcome::Admitted { delta, va_start }) => {
                            mapped_regions += 1;
                            region_diagnostics.push(VromRecordMappingDiagnostic::DeltaVote);
                            Some((delta, va_start))
                        }
                        _ => {
                            // Replaced with the precise rule below. Rule 1 is
                            // the fail-closed default if no admitted file table
                            // is available to evaluate the record further.
                            region_diagnostics.push(VromRecordMappingDiagnostic::Open(
                                DescriptorMappingFailure::Rule1SourceMaterialization,
                            ));
                            None
                        }
                    }
                })
                .collect();
            let required_mapped_regions = min_mapped_regions;
            VromTableAdmission {
                admitted: mapped_regions >= required_mapped_regions,
                mapped_regions,
                required_mapped_regions,
                region_deltas,
                region_diagnostics,
                table: table.clone(),
            }
        })
        .collect();

    if let Some(admitted_file_table) = &file_table.admitted_table {
        let initially_admitted: Vec<_> = admissions
            .iter()
            .map(|admission| admission.admitted)
            .collect();
        let mut table_evidence = vec![Vec::new(); admissions.len()];

        // Evaluate every delta-open record in an ordinary admission. For a
        // below-floor table, evaluate every record: its delta-voted records
        // must independently agree with descriptor corroboration too.
        for admission_index in 0..admissions.len() {
            for record_index in 0..admissions[admission_index].table.records.len() {
                let below_floor = !initially_admitted[admission_index];
                if !below_floor && admissions[admission_index].region_deltas[record_index].is_some()
                {
                    continue;
                }
                let record = admissions[admission_index].table.records[record_index];
                let region_bytes = match materialize_vrom_for_recovery(
                    admitted_file_table,
                    rom_bytes,
                    record.rom_start,
                    record.rom_end,
                    materialization_limits,
                    &mut decoded_file_limit_hits,
                ) {
                    Some(bytes) => bytes,
                    None => {
                        admissions[admission_index].region_diagnostics[record_index] =
                            VromRecordMappingDiagnostic::Open(
                                DescriptorMappingFailure::Rule1SourceMaterialization,
                            );
                        continue;
                    }
                };
                let Some(mapping) = MappingHypothesis::from_descriptor(record, config) else {
                    admissions[admission_index].region_diagnostics[record_index] =
                        VromRecordMappingDiagnostic::Open(
                            DescriptorMappingFailure::Rule2DestinationRange,
                        );
                    continue;
                };
                match descriptor_mapping_corroborated(&region_bytes, mapping) {
                    Ok(()) => {
                        if let Some((_, va_start)) =
                            admissions[admission_index].region_deltas[record_index]
                        {
                            if MappingHypothesis::from_delta(record, va_start) != Some(mapping) {
                                admissions[admission_index].region_diagnostics[record_index] =
                                    VromRecordMappingDiagnostic::Open(
                                        DescriptorMappingFailure::Rule4VaConflict,
                                    );
                                continue;
                            }
                            admissions[admission_index].region_diagnostics[record_index] =
                                VromRecordMappingDiagnostic::DeltaVoteAndDescriptorCorroborated;
                        } else {
                            admissions[admission_index].region_diagnostics[record_index] =
                                VromRecordMappingDiagnostic::DescriptorCorroborated;
                        }
                        table_evidence[admission_index].push(DescriptorFallback {
                            admission_index,
                            record_index,
                            mapping,
                        });
                    }
                    Err(failure) => {
                        admissions[admission_index].region_diagnostics[record_index] =
                            VromRecordMappingDiagnostic::Open(DescriptorMappingFailure::Rule3Cfg(
                                failure,
                            ));
                    }
                }
            }
        }

        let table_eligible: Vec<_> = admissions
            .iter()
            .enumerate()
            .map(|(admission_index, admission)| {
                if initially_admitted[admission_index] {
                    return true;
                }
                table_evidence[admission_index].len() == admission.table.records.len()
            })
            .collect();

        // Only ordinary admissions and fully corroborated below-floor tables
        // participate in global uniqueness. An incomplete, rejected table
        // cannot manufacture a conflict that suppresses a sound mapping.
        let mut raw_mappings = BTreeSet::new();
        let mut fallbacks = Vec::new();
        for (admission_index, admission) in admissions.iter().enumerate() {
            if !table_eligible[admission_index] {
                continue;
            }
            if initially_admitted[admission_index] {
                for (record, outcome) in
                    admission.table.records.iter().zip(&admission.region_deltas)
                {
                    if let Some((_, va_start)) = *outcome {
                        if let Some(mapping) = MappingHypothesis::from_delta(*record, va_start) {
                            raw_mappings.insert(mapping);
                        }
                    }
                }
            }
            fallbacks.extend(table_evidence[admission_index].iter().copied());
        }

        let accepted = apply_unique_descriptor_fallbacks(
            &mut admissions,
            &raw_mappings,
            fallbacks.clone(),
            |admission| &mut admission.region_deltas,
            |admission| &mut admission.mapped_regions,
        );

        for candidate in &fallbacks {
            if !accepted.contains(&(candidate.admission_index, candidate.record_index)) {
                admissions[candidate.admission_index].region_diagnostics[candidate.record_index] =
                    VromRecordMappingDiagnostic::Open(DescriptorMappingFailure::Rule4VaConflict);
            }
        }

        for admission_index in 0..admissions.len() {
            if initially_admitted[admission_index] {
                continue;
            }
            let fully_mapped = admissions[admission_index]
                .region_deltas
                .iter()
                .all(Option::is_some);
            let all_unique = table_evidence[admission_index].iter().all(|candidate| {
                accepted.contains(&(candidate.admission_index, candidate.record_index))
            });
            admissions[admission_index].admitted =
                table_eligible[admission_index] && fully_mapped && all_unique;
            if !admissions[admission_index].admitted {
                for diagnostic in &mut admissions[admission_index].region_diagnostics {
                    if matches!(
                        diagnostic,
                        VromRecordMappingDiagnostic::DescriptorCorroborated
                            | VromRecordMappingDiagnostic::DeltaVoteAndDescriptorCorroborated
                    ) {
                        *diagnostic =
                            VromRecordMappingDiagnostic::DescriptorCorroboratedTableIncomplete;
                    }
                }
            }
        }
    }

    VromOverlayRecovery {
        file_table,
        config: config.clone(),
        delta_config: *delta_config,
        materialization_limits,
        decoded_file_limit_hits: decoded_file_limit_hits.into_iter().collect(),
        vrom_min_records,
        min_mapped_regions,
        candidate_tables,
        admissions,
    }
}

#[cfg(test)]
mod tests;
