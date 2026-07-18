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
        // Small effect/utility overlays remain real code images; delta_vote,
        // not a 4 KiB AKI size floor, is the discriminating evidence.
        config.min_region_len = 0x100;
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
    /// admitted a unique mapping, `None` when it stayed open.
    pub region_deltas: Vec<Option<(u32, u32)>>,
    /// Regions delta_vote uniquely mapped.
    pub mapped_regions: u32,
    /// Admitted iff `mapped_regions >= min_mapped_regions` (see
    /// [`recover_overlay_regions`]).
    pub admitted: bool,
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

    let admissions = candidate_tables
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
    pub mapped_regions: u32,
    /// Engine-independent evidence minimum supplied by the caller, matching
    /// the unchanged physical descriptor-family path.
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
                let mut offset = start;
                while let Some(record) = valid_triples.get(&offset) {
                    records.push(*record);
                    let Some(next) = offset.checked_add(stride) else {
                        break;
                    };
                    offset = next;
                }
                if records.len() as u32 >= vrom_min_records && intervals_non_overlapping(&records) {
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
/// `vrom_min_records` is explicit because two records are enough only when
/// both independently satisfy `delta_vote`; physical AKI callers retain their
/// established three-record family minimum. `min_mapped_regions` has exactly
/// the same meaning as [`recover_overlay_regions`].
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
    let admissions = candidate_tables
        .iter()
        .map(|table| {
            let admitted_file_table = file_table
                .admitted_table
                .as_ref()
                .expect("VROM candidates require one admitted file table");
            let mut mapped_regions = 0u32;
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
                            Some((delta, va_start))
                        }
                        _ => None,
                    }
                })
                .collect();
            let required_mapped_regions = min_mapped_regions;
            VromTableAdmission {
                admitted: mapped_regions >= required_mapped_regions,
                mapped_regions,
                required_mapped_regions,
                region_deltas,
                table: table.clone(),
            }
        })
        .collect();

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
