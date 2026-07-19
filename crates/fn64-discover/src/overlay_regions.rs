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
//! plausible ROM interval and a plausible RDRAM destination VA. This module
//! enumerates that family across the whole ROM.
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
    /// covering the AKI `0x24` record plus neighbours. `min_records = 3` is
    /// the smallest run a single arithmetic accident cannot fake (two records
    /// can share one spurious spacing; three independent plausible triples in
    /// a row do not).
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
            min_records: 3,
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
    pub vram_dest: u32,
}

impl CandidateRecord {
    /// The overlay's ROM byte length. Named `byte_len` rather than `len` so it
    /// reads as a size, not a container length.
    pub fn byte_len(&self) -> u32 {
        self.rom_end - self.rom_start
    }
}

/// A candidate table: where it sits, its shape, and the records it yields.
/// Identity for canonicalization is the ordered ROM-interval set, not the
/// `(offset, field)` phase -- a table read at `+0` with `field_rom_start = 0`
/// and the same table read one word earlier with `field_rom_start = 4`
/// describe the identical overlays and must not both count.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CandidateTable {
    pub table_rom_offset: u32,
    pub record_stride: u32,
    pub field_rom_start: u32,
    pub field_rom_end: u32,
    pub field_vram_dest: u32,
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

            let mut offset = config.min_rom_offset;
            let last_table_start =
                rom_len.saturating_sub(stride.saturating_mul(config.min_records));
            while offset <= last_table_start {
                let read_rec = |base: u32| -> Option<CandidateRecord> {
                    Some(CandidateRecord {
                        rom_start: read_u32_be(rom_bytes, (base + field_rom_start) as usize)?,
                        rom_end: read_u32_be(rom_bytes, (base + field_rom_end) as usize)?,
                        vram_dest: read_u32_be(rom_bytes, (base + field_vram_dest) as usize)?,
                    })
                };

                // Quick reject before growing a run.
                let first = read_rec(offset);
                if !first.is_some_and(|r| record_valid(&r, rom_len, config)) {
                    offset += 4;
                    continue;
                }

                let mut records = Vec::new();
                let mut base = offset;
                while base <= rom_len.saturating_sub(stride) {
                    match read_rec(base) {
                        Some(rec) if record_valid(&rec, rom_len, config) => {
                            records.push(rec);
                            base += stride;
                        }
                        _ => break,
                    }
                }

                if records.len() as u32 >= config.min_records && intervals_non_overlapping(&records)
                {
                    raw.push(CandidateTable {
                        table_rom_offset: offset,
                        record_stride: stride,
                        field_rom_start,
                        field_rom_end,
                        field_vram_dest,
                        records,
                    });
                    // Skip past the consumed run: overlapping sub-runs of the
                    // same table are not independent discoveries.
                    offset = base;
                } else {
                    offset += 4;
                }
            }
            field_rom_start += 4;
        }
    }

    canonicalize(raw)
}

/// Collapse phase aliases: keep one table per distinct interval set, choosing
/// the lowest `table_rom_offset` (then the smallest field layout) as the
/// canonical representative. Deterministic.
fn canonicalize(mut raw: Vec<CandidateTable>) -> Vec<CandidateTable> {
    raw.sort_by(|a, b| {
        a.table_rom_offset
            .cmp(&b.table_rom_offset)
            .then(a.record_stride.cmp(&b.record_stride))
            .then(a.field_rom_start.cmp(&b.field_rom_start))
    });
    let mut seen: BTreeSet<Vec<(u32, u32)>> = BTreeSet::new();
    let mut out = Vec::new();
    for table in raw {
        if seen.insert(table.interval_set()) {
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
    /// Admitted iff `mapped_regions >= min_mapped_regions` (see
    /// [`recover_overlay_regions`]).
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
            BlockTerminator::RanOffEnd => Some(DescriptorCfgFailure::RanOffEnd {
                block_start: block.start_va,
            }),
            BlockTerminator::Branch {
                target,
                fallthrough,
            }
            | BlockTerminator::BranchLikely {
                target,
                fallthrough,
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

/// Admit only hypotheses whose VA interval is unique across distinct region
/// identities. Exact duplicate records from phase/stride aliases are one
/// hypothesis, not conflicts; different source regions sharing any VA byte
/// reject each other symmetrically, independent of iteration order.
fn apply_unique_descriptor_fallbacks<A>(
    admissions: &mut [A],
    raw_mappings: &BTreeSet<MappingHypothesis>,
    fallbacks: Vec<DescriptorFallback>,
    mut region_deltas: impl FnMut(&mut A) -> &mut Vec<Option<(u32, u32)>>,
    mut mapped_regions: impl FnMut(&mut A) -> &mut u32,
) -> BTreeSet<(usize, usize)> {
    let admitted: Vec<_> = fallbacks
        .iter()
        .filter(|candidate| {
            let conflicts_with_raw = raw_mappings.iter().any(|mapping| {
                *mapping != candidate.mapping && mapping.overlaps_va(candidate.mapping)
            });
            let conflicts_with_fallback = fallbacks.iter().any(|other| {
                other.mapping != candidate.mapping && other.mapping.overlaps_va(candidate.mapping)
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

    let mut raw_mappings = BTreeSet::new();
    let mut fallbacks = Vec::new();
    for (admission_index, admission) in admissions.iter().enumerate() {
        if !admission.admitted {
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
    let _ = apply_unique_descriptor_fallbacks(
        &mut admissions,
        &raw_mappings,
        fallbacks,
        |admission| &mut admission.region_deltas,
        |admission| &mut admission.mapped_regions,
    );

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
    pub vrom_min_records: u32,
    pub min_mapped_regions: u32,
    pub candidate_tables: Vec<VromCandidateTable>,
    pub admissions: Vec<VromTableAdmission>,
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
) -> Vec<VromCandidateTable> {
    let mut raw = Vec::new();

    for file_record in &file_table.records {
        let Ok(file_bytes) = file_table.materialize_vrom_range(
            rom_bytes,
            file_record.vrom_start,
            file_record.vrom_end,
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
    assert!(vrom_min_records >= 2, "a one-record run is not a table");
    let file_table = recover_file_table(rom_bytes, file_table_config);
    let candidate_tables = file_table
        .admitted_table
        .as_ref()
        .map_or_else(Vec::new, |table| {
            enumerate_vrom_family_tables(rom_bytes, table, config, vrom_min_records)
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
                    let outcome = admitted_file_table
                        .materialize_vrom_range(rom_bytes, record.rom_start, record.rom_end)
                        .ok()
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
                let region_bytes = match admitted_file_table.materialize_vrom_range(
                    rom_bytes,
                    record.rom_start,
                    record.rom_end,
                ) {
                    Ok(bytes) => bytes,
                    Err(_) => {
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
        vrom_min_records,
        min_mapped_regions,
        candidate_tables,
        admissions,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a big-endian ROM with a planted descriptor table of a chosen
    /// shape. Region bodies are filled with a delta_vote-admissible code
    /// pattern (three distinct `jal`s onto prologues plus enough `lui`) so
    /// the uniqueness filter has something real to admit.
    struct RomBuilder {
        bytes: Vec<u8>,
    }

    impl RomBuilder {
        fn new(len: usize) -> Self {
            Self {
                bytes: vec![0u8; len],
            }
        }
        fn put_u32(&mut self, offset: u32, value: u32) {
            self.bytes[offset as usize..offset as usize + 4].copy_from_slice(&value.to_be_bytes());
        }
        /// Fill `[rom_start, rom_start+len)` with a region mapped to `va_start`
        /// that delta_vote admits: three distinct jals to prologues plus lui.
        fn plant_admissible_region(&mut self, rom_start: u32, len: u32, va_start: u32) {
            self.plant_admissible_bytes(rom_start, len, va_start);
        }
        fn plant_admissible_bytes(&mut self, physical_start: u32, len: u32, va_start: u32) {
            const PROLOGUE: u32 = 0x27bd_ffe0; // addiu $sp,$sp,-0x20
            let lui = 0x3c04_0000 | (va_start >> 16); // lui $a0, hi(va)
            let jal = |target: u32| 0x0c00_0000 | ((target >> 2) & 0x03ff_ffff);
            // Prologues at non-uniform offsets so no second delta aliases.
            let prologues = [0x40u32, 0x90, 0x100];
            self.put_u32(physical_start, jal(va_start + prologues[0]));
            self.put_u32(physical_start + 8, jal(va_start + prologues[1]));
            self.put_u32(physical_start + 16, jal(va_start + prologues[2]));
            for k in 0..4u32 {
                self.put_u32(physical_start + 24 + k * 4, lui);
            }
            for &p in &prologues {
                if p + 4 <= len {
                    self.put_u32(physical_start + p, PROLOGUE);
                }
            }
        }

        fn plant_admissible_and_corroborating_region(
            &mut self,
            physical_start: u32,
            va_start: u32,
        ) {
            let jal = |target: u32| 0x0c00_0000 | ((target >> 2) & 0x03ff_ffff);
            for (offset, word) in [
                (0x00, jal(va_start + 0x40)),
                (0x04, 0),
                (0x08, jal(va_start + 0x90)),
                (0x0c, 0),
                (0x10, jal(va_start + 0x100)),
                (0x14, 0),
                (0x18, 0x3c04_0000 | (va_start >> 16)),
                (0x1c, 0x3c05_0000 | (va_start >> 16)),
                (0x20, 0x3c06_0000 | (va_start >> 16)),
                (0x24, 0x3c07_0000 | (va_start >> 16)),
                (0x28, 0x03e0_0008),
                (0x2c, 0),
            ] {
                self.put_u32(physical_start + offset, word);
            }
            for offset in [0x40, 0x90, 0x100] {
                self.put_u32(physical_start + offset, 0x27bd_ffe0);
                self.put_u32(physical_start + offset + 4, 0x03e0_0008);
                self.put_u32(physical_start + offset + 8, 0);
            }
        }

        /// A small, complete leaf function with no absolute-address evidence:
        /// delta_vote stays open, while descriptor-rooted CFG validation has
        /// an independently decodable return path.
        fn plant_descriptor_corroborating_region(&mut self, physical_start: u32) {
            self.put_u32(physical_start, 0x27bd_ffe0); // addiu $sp,$sp,-0x20
            self.put_u32(physical_start + 4, 0x03e0_0008); // jr $ra
            self.put_u32(physical_start + 8, 0x0000_0000); // delay-slot nop
        }

        fn plant_file_record(
            &mut self,
            offset: u32,
            vrom_start: u32,
            vrom_end: u32,
            rom_start: u32,
        ) {
            self.put_u32(offset, vrom_start);
            self.put_u32(offset + 4, vrom_end);
            self.put_u32(offset + 8, rom_start);
            self.put_u32(offset + 12, 0);
        }
    }

    /// A planted table of a NON-NW4E shape (stride 0x10, fields at +0) is
    /// recovered, and its regions delta_vote-admit.
    #[test]
    fn planted_non_nw4e_table_is_recovered() {
        let mut rom = RomBuilder::new(0x40_000);
        let table_off = 0x2000u32;
        let stride = 0x10u32;
        // Three chained regions.
        let regions = [
            (0x8000u32, 0x8000u32, 0x8010_0000u32),
            (0x10000, 0x6000, 0x8020_0000),
            (0x16000, 0x9000, 0x8030_0000),
        ];
        for (i, &(rom_start, len, va)) in regions.iter().enumerate() {
            let base = table_off + i as u32 * stride;
            rom.put_u32(base, rom_start);
            rom.put_u32(base + 4, rom_start + len);
            rom.put_u32(base + 8, va);
            rom.plant_admissible_region(rom_start, len, va);
        }

        let config = SearchConfig::aki_family();
        let recovery = recover_overlay_regions(&rom.bytes, &config, &DeltaVoteConfig::default(), 2);

        // The planted table is present as a distinct candidate.
        let found = recovery
            .candidate_tables
            .iter()
            .find(|t| {
                t.interval_set() == vec![(0x8000, 0x10000), (0x10000, 0x16000), (0x16000, 0x1f000)]
            })
            .expect("planted table recovered");
        assert_eq!(found.records.len(), 3);
        assert_eq!(found.record_stride, 0x10);

        // Its regions delta_vote-admit and the table is admitted.
        let admission = recovery
            .admissions
            .iter()
            .find(|a| a.table.interval_set() == found.interval_set())
            .unwrap();
        assert_eq!(admission.mapped_regions, 3);
        assert!(admission.admitted);
        assert_eq!(
            admission.region_deltas[0],
            Some((0x8010_0000u32.wrapping_sub(0x8000), 0x8010_0000))
        );
    }

    /// A table whose last record is out of bounds is not extended into it:
    /// the run stops at the last valid record, and if fewer than `min_records`
    /// remain the run is not a table at all.
    #[test]
    fn record_out_of_bounds_is_rejected() {
        let mut rom = RomBuilder::new(0x20_000);
        let table_off = 0x2000u32;
        // Two valid records then one whose rom_end exceeds the ROM.
        rom.put_u32(table_off, 0x4000);
        rom.put_u32(table_off + 4, 0x8000);
        rom.put_u32(table_off + 8, 0x8010_0000);
        rom.put_u32(table_off + 0x10, 0x8000);
        rom.put_u32(table_off + 0x14, 0xc000);
        rom.put_u32(table_off + 0x18, 0x8020_0000);
        // Out-of-bounds rom_end.
        rom.put_u32(table_off + 0x20, 0xc000);
        rom.put_u32(table_off + 0x24, 0x00ff_ffff); // > rom_len
        rom.put_u32(table_off + 0x28, 0x8030_0000);

        let config = SearchConfig::aki_family();
        let tables = enumerate_family_tables(&rom.bytes, &config);
        // The two-record prefix is below min_records=3, so no table with the
        // out-of-bounds interval is admitted anywhere.
        assert!(tables.iter().all(|t| t
            .records
            .iter()
            .all(|r| r.rom_end <= rom.bytes.len() as u32)));
        assert!(tables
            .iter()
            .all(|t| !t.interval_set().contains(&(0xc000, 0x00ff_ffff))));
    }

    /// Two distinct candidate tables both survive the raw family search;
    /// delta_vote admissibility keeps only the one whose regions are real
    /// code. The other (coincidental pointer pairs over zero-filled ROM)
    /// stays a candidate but is NOT admitted.
    #[test]
    fn delta_vote_disambiguates_two_candidate_tables() {
        let mut rom = RomBuilder::new(0x60_000);

        // Table A: real code regions -> delta_vote admits.
        let table_a = 0x2000u32;
        let regions_a = [
            (0x8000u32, 0x8000u32, 0x8010_0000u32),
            (0x10000, 0x6000, 0x8020_0000),
            (0x16000, 0x9000, 0x8030_0000),
        ];
        for (i, &(rs, len, va)) in regions_a.iter().enumerate() {
            let base = table_a + i as u32 * 0x10;
            rom.put_u32(base, rs);
            rom.put_u32(base + 4, rs + len);
            rom.put_u32(base + 8, va);
            rom.plant_admissible_region(rs, len, va);
        }

        // Table B: well-formed records pointing at zero-filled ROM regions
        // (no code) -> delta_vote finds no lui segment / no votes, stays open.
        let table_b = 0x3000u32;
        let regions_b = [
            (0x30000u32, 0x4000u32, 0x8040_0000u32),
            (0x34000, 0x4000, 0x8041_0000),
            (0x38000, 0x4000, 0x8042_0000),
        ];
        for (i, &(rs, len, va)) in regions_b.iter().enumerate() {
            let base = table_b + i as u32 * 0x10;
            rom.put_u32(base, rs);
            rom.put_u32(base + 4, rs + len);
            rom.put_u32(base + 8, va);
            // Deliberately leave the region zero-filled: not code.
        }

        let config = SearchConfig::aki_family();
        let recovery = recover_overlay_regions(&rom.bytes, &config, &DeltaVoteConfig::default(), 2);

        let set_a = vec![(0x8000, 0x10000), (0x10000, 0x16000), (0x16000, 0x1f000)];
        let set_b = vec![(0x30000, 0x34000), (0x34000, 0x38000), (0x38000, 0x3c000)];

        // Both are candidates from the raw search.
        assert!(recovery
            .candidate_tables
            .iter()
            .any(|t| t.interval_set() == set_a));
        assert!(recovery
            .candidate_tables
            .iter()
            .any(|t| t.interval_set() == set_b));

        // delta_vote admits only A.
        let a = recovery
            .admissions
            .iter()
            .find(|x| x.table.interval_set() == set_a)
            .unwrap();
        let b = recovery
            .admissions
            .iter()
            .find(|x| x.table.interval_set() == set_b)
            .unwrap();
        assert!(a.admitted, "code-backed table must admit");
        assert!(!b.admitted, "zero-filled table must not admit");
        assert_eq!(b.mapped_regions, 0);

        // The admitted-intervals accessor returns exactly A's intervals.
        assert_eq!(recovery.admitted_intervals(), set_a);
    }

    #[test]
    fn recovery_is_byte_identical_across_runs() {
        let mut rom = RomBuilder::new(0x40_000);
        let table_off = 0x2000u32;
        let regions = [
            (0x8000u32, 0x8000u32, 0x8010_0000u32),
            (0x10000, 0x6000, 0x8020_0000),
            (0x16000, 0x9000, 0x8030_0000),
        ];
        for (i, &(rs, len, va)) in regions.iter().enumerate() {
            let base = table_off + i as u32 * 0x10;
            rom.put_u32(base, rs);
            rom.put_u32(base + 4, rs + len);
            rom.put_u32(base + 8, va);
            rom.plant_admissible_region(rs, len, va);
        }
        let config = SearchConfig::aki_family();
        let first = recover_overlay_regions(&rom.bytes, &config, &DeltaVoteConfig::default(), 2);
        let second = recover_overlay_regions(&rom.bytes, &config, &DeltaVoteConfig::default(), 2);
        assert_eq!(
            serde_json::to_string(&first).unwrap(),
            serde_json::to_string(&second).unwrap()
        );
    }

    fn descriptor_fallback_fixture(third_va: u32, corroborating_code: bool) -> TableAdmission {
        let mut rom = RomBuilder::new(0x20_000);
        let records = [
            (0x8000u32, 0x400u32, 0x8010_0000u32),
            (0x9000, 0x400, 0x8020_0000),
            (0xa000, 0x400, third_va),
        ];
        for (index, &(rom_start, len, va_start)) in records.iter().enumerate() {
            let descriptor = 0x2000 + index as u32 * 0x10;
            rom.put_u32(descriptor, rom_start);
            rom.put_u32(descriptor + 4, rom_start + len);
            rom.put_u32(descriptor + 8, va_start);
            if index < 2 {
                rom.plant_admissible_region(rom_start, len, va_start);
            } else if corroborating_code {
                rom.plant_descriptor_corroborating_region(rom_start);
            }
        }

        let recovery = recover_overlay_regions(
            &rom.bytes,
            &SearchConfig::vrom_family(),
            &DeltaVoteConfig::default(),
            2,
        );
        recovery
            .admissions
            .into_iter()
            .find(|admission| {
                admission.table.interval_set()
                    == vec![(0x8000, 0x8400), (0x9000, 0x9400), (0xa000, 0xa400)]
            })
            .expect("planted descriptor table recovered")
    }

    #[test]
    fn descriptor_corroborates_delta_open_record() {
        let admission = descriptor_fallback_fixture(0x8030_0000, true);
        assert!(
            admission.admitted,
            "two delta-voted records admit the table"
        );
        assert_eq!(admission.mapped_regions, 3);
        assert_eq!(
            admission.region_deltas[2],
            Some((0x8030_0000u32.wrapping_sub(0xa000), 0x8030_0000))
        );
    }

    #[test]
    fn descriptor_without_region_corroboration_stays_open() {
        let admission = descriptor_fallback_fixture(0x8030_0000, false);
        assert!(admission.admitted);
        assert_eq!(admission.mapped_regions, 2);
        assert_eq!(admission.region_deltas[2], None);
    }

    #[test]
    fn descriptor_overlapping_delta_admitted_va_is_rejected_as_non_unique() {
        let admission = descriptor_fallback_fixture(0x8010_0000, true);
        assert!(admission.admitted);
        assert_eq!(admission.mapped_regions, 2);
        assert_eq!(admission.region_deltas[2], None);
    }

    fn one_delta_mapped_vrom_table(second_va: u32, corroborate_second: bool) -> VromTableAdmission {
        let mut rom = RomBuilder::new(0x20_000);
        for (index, &(vrom_start, vrom_end, physical_start)) in [
            (0x0000, 0x1000, 0x0000),
            (0x1000, 0x2000, 0x4000),
            (0x2000, 0x2400, 0x8000),
            (0x2400, 0x2800, 0x8400),
        ]
        .iter()
        .enumerate()
        {
            rom.plant_file_record(
                0x2000 + index as u32 * 0x10,
                vrom_start,
                vrom_end,
                physical_start,
            );
        }

        let first_va = 0x8010_0000;
        for (index, &(start, end, va)) in [(0x2000, 0x2400, first_va), (0x2400, 0x2800, second_va)]
            .iter()
            .enumerate()
        {
            let descriptor = 0x4000 + index as u32 * 0x10;
            rom.put_u32(descriptor, start);
            rom.put_u32(descriptor + 4, end);
            rom.put_u32(descriptor + 8, va);
        }
        rom.plant_admissible_and_corroborating_region(0x8000, first_va);
        if corroborate_second {
            rom.plant_descriptor_corroborating_region(0x8400);
        }

        recover_vrom_overlay_regions(
            &rom.bytes,
            &SearchConfig::vrom_family(),
            &DeltaVoteConfig::default(),
            &FileTableSearchConfig::n64_family(),
            2,
            2,
        )
        .admissions
        .into_iter()
        .find(|admission| {
            admission.table.interval_set() == vec![(0x2000, 0x2400), (0x2400, 0x2800)]
        })
        .expect("planted VROM descriptor table recovered")
    }

    #[test]
    fn one_delta_mapped_table_with_corroborated_unique_record_is_admitted() {
        let admission = one_delta_mapped_vrom_table(0x8020_0000, true);
        assert!(admission.admitted);
        assert_eq!(admission.mapped_regions, 2);
        assert_eq!(
            admission.region_diagnostics,
            [
                VromRecordMappingDiagnostic::DeltaVoteAndDescriptorCorroborated,
                VromRecordMappingDiagnostic::DescriptorCorroborated,
            ]
        );
    }

    #[test]
    fn one_delta_mapped_table_with_uncorroborated_record_stays_open() {
        let admission = one_delta_mapped_vrom_table(0x8020_0000, false);
        assert!(!admission.admitted);
        assert_eq!(admission.mapped_regions, 1);
        assert!(matches!(
            admission.region_diagnostics[1],
            VromRecordMappingDiagnostic::Open(DescriptorMappingFailure::Rule3Cfg(_))
        ));
    }

    #[test]
    fn one_delta_mapped_table_with_va_conflict_is_rejected() {
        let admission = one_delta_mapped_vrom_table(0x8010_0000, true);
        assert!(!admission.admitted);
        assert_eq!(admission.mapped_regions, 1);
        assert!(admission.region_diagnostics.iter().any(|diagnostic| {
            *diagnostic
                == VromRecordMappingDiagnostic::Open(DescriptorMappingFailure::Rule4VaConflict)
        }));
    }

    #[test]
    fn bounded_short_leaf_is_code_but_same_size_non_code_is_rejected() {
        let mapping = MappingHypothesis {
            source_start: 0x1000,
            source_end: 0x1080,
            va_start: 0x8080_0000,
            va_end: 0x8080_0080,
        };
        let mut leaf = vec![0u8; 0x80];
        leaf[..12].copy_from_slice(&[
            0x27, 0xbd, 0xff, 0xe0, 0x03, 0xe0, 0x00, 0x08, 0x00, 0x00, 0x00, 0x00,
        ]);
        assert_eq!(descriptor_mapping_corroborated(&leaf, mapping), Ok(()));
        assert!(matches!(
            descriptor_mapping_corroborated(&[0u8; 0x80], mapping),
            Err(DescriptorCfgFailure::RanOffEnd { .. })
        ));
    }

    #[test]
    fn corroborated_one_record_gap_is_admitted_but_isolated_triple_is_not_enumerated() {
        let mut rom = RomBuilder::new(0x20_000);
        let files = [
            (0x0000, 0x1000, 0x0000),
            (0x1000, 0x2000, 0x4000),
            (0x2000, 0x2100, 0x8000),
            (0x2100, 0x2200, 0x8100),
            (0x2200, 0x2300, 0x8200),
            (0x2300, 0x2400, 0x8300),
            (0x2400, 0x2500, 0x8400),
            (0x2500, 0x2600, 0x8500),
        ];
        for (index, &(vrom_start, vrom_end, physical_start)) in files.iter().enumerate() {
            rom.plant_file_record(
                0x2800 + index as u32 * 0x10,
                vrom_start,
                vrom_end,
                physical_start,
            );
        }

        let records = [
            (0x100, 0x2000, 0x2100, 0x8080_0000, 0x8000),
            (0x140, 0x2100, 0x2200, 0x8080_0100, 0x8100),
            (0x200, 0x2200, 0x2300, 0x8080_0200, 0x8200),
            (0x300, 0x2300, 0x2400, 0x8080_0300, 0x8300),
            (0x340, 0x2400, 0x2500, 0x8080_0400, 0x8400),
            // This also corroborates as code, but lies outside the envelope
            // formed by the two ordinary constant-stride table fragments.
            (0x500, 0x2500, 0x2600, 0x8080_0500, 0x8500),
        ];
        for &(table_offset, start, end, va, physical_start) in &records {
            rom.put_u32(0x4000 + table_offset, start);
            rom.put_u32(0x4000 + table_offset + 4, end);
            rom.put_u32(0x4000 + table_offset + 8, va);
            rom.plant_descriptor_corroborating_region(physical_start);
        }

        let recovery = recover_vrom_overlay_regions(
            &rom.bytes,
            &SearchConfig::vrom_family(),
            &DeltaVoteConfig::default(),
            &FileTableSearchConfig::n64_family(),
            2,
            2,
        );
        let gap = recovery
            .admissions
            .iter()
            .find(|admission| admission.table.interval_set() == vec![(0x2200, 0x2300)])
            .expect("one-record gap inside the descriptor envelope");
        assert_eq!(gap.table.record_stride, 0);
        assert!(gap.admitted);
        assert_eq!(gap.mapped_regions, 1);
        assert_eq!(
            gap.region_diagnostics,
            [VromRecordMappingDiagnostic::DescriptorCorroborated]
        );
        assert!(recovery
            .candidate_tables
            .iter()
            .all(|table| table.interval_set() != vec![(0x2500, 0x2600)]));
    }

    #[test]
    fn virtual_table_is_resolved_through_recovered_file_table() {
        let mut rom = RomBuilder {
            bytes: vec![0xff; 0x20_000],
        };
        let file_table_offset = 0x2000u32;
        let files = [
            (0x0000, 0x1000, 0x0000),
            (0x1000, 0x2000, 0x4000),
            (0x2000, 0x6000, 0x8000),
            (0x6000, 0xa000, 0xc000),
            (0xa000, 0xe000, 0x10000),
        ];
        for (index, &(vrom_start, vrom_end, physical_start)) in files.iter().enumerate() {
            rom.plant_file_record(
                file_table_offset + index as u32 * 0x10,
                vrom_start,
                vrom_end,
                physical_start,
            );
        }

        let overlays = [
            (0x2000, 0x6000, 0x8000, 0x8010_0000),
            (0x6000, 0xa000, 0xc000, 0x8020_0000),
            (0xa000, 0xe000, 0x10000, 0x8030_0000),
        ];
        for (index, &(vrom_start, vrom_end, physical_start, va_start)) in
            overlays.iter().enumerate()
        {
            let descriptor = 0x4000 + index as u32 * 0x10;
            rom.put_u32(descriptor, vrom_start);
            rom.put_u32(descriptor + 4, vrom_end);
            rom.put_u32(descriptor + 8, va_start);
            rom.plant_admissible_bytes(physical_start, vrom_end - vrom_start, va_start);
        }

        let recovery = recover_vrom_overlay_regions(
            &rom.bytes,
            &SearchConfig::aki_family(),
            &DeltaVoteConfig::default(),
            &FileTableSearchConfig::n64_family(),
            2,
            2,
        );
        let file_table = recovery.file_table.admitted_table.as_ref().unwrap();
        assert_eq!(file_table.table_rom_offset, file_table_offset);
        assert_eq!(file_table.translate_uncompressed(0x1000), Some(0x4000));

        let expected = vec![(0x2000, 0x6000), (0x6000, 0xa000), (0xa000, 0xe000)];
        let admission = recovery
            .admissions
            .iter()
            .find(|admission| admission.table.interval_set() == expected)
            .expect("VROM-located descriptor table recovered");
        assert!(admission.admitted);
        assert_eq!(admission.mapped_regions, 3);
        assert_eq!(recovery.admitted_intervals(), expected);
    }

    #[test]
    fn single_image_file_table_does_not_manufacture_an_overlay_table() {
        let mut rom = RomBuilder {
            bytes: vec![0xff; 0x10_000],
        };
        for (index, &(vrom_start, vrom_end, physical_start)) in [
            (0x0000, 0x1000, 0x0000),
            (0x1000, 0x2000, 0x4000),
            (0x2000, 0x3000, 0x5000),
        ]
        .iter()
        .enumerate()
        {
            rom.plant_file_record(
                0x2000 + index as u32 * 0x10,
                vrom_start,
                vrom_end,
                physical_start,
            );
        }
        let recovery = recover_vrom_overlay_regions(
            &rom.bytes,
            &SearchConfig::aki_family(),
            &DeltaVoteConfig::default(),
            &FileTableSearchConfig::n64_family(),
            2,
            2,
        );
        assert!(recovery.file_table.admitted_table.is_some());
        assert!(recovery.candidate_tables.is_empty());
        assert!(recovery.admitted_intervals().is_empty());
    }
}
