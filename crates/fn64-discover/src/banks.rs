//! Phase 2 (docs/DISCOVER-DESIGN.md "discover load images before
//! functions"): find candidate ROM-to-RDRAM mappings *before* any function
//! discovery runs, so function identity can be bank-qualified from the
//! first instruction.
//!
//! This module implements the two detectors that are mechanical hardware
//! facts rather than heuristics for every N64 ROM:
//!
//! 1. **The boot copy.** IPL3 always DMAs a fixed-size prefix of the ROM
//!    (hardware behavior: 0x100000 bytes for a PI-based CIC boot, starting
//!    at ROM offset 0x1000) to the RDRAM VA named by the header's entry
//!    point field. This is `bank "boot"` and requires no scanning -- the
//!    header supplies every field directly.
//! 2. **Repeated-load VA ranges from an overlay descriptor table.** Many
//!    AKI-family engines (see faki-tools' NW4E ground truth) and other N64
//!    titles keep a fixed-shape table of `(rom_start, rom_end, vram_dest,
//!    ...)` records used by a loader dispatcher. [`scan_descriptor_table`]
//!    takes an explicit table location + record shape (never guessed) and
//!    turns each record into a `RomMapping` candidate fact.
//!
//! Both feed `FactDb::conclude` so downstream consumers see one proof
//! state per bank rather than re-deriving mapping validity themselves.

use crate::facts::{
    function_entry_subject, load_image_table_record_subject, BankAddr, CandidateDetector, Fact,
    FactDb, FunctionEntryEvidence, MappingAddressSpace, ProofState, RomAddressSpace,
};
use crate::rom::NormalizedRom;
use serde::{Deserialize, Serialize};

/// IPL3's fixed boot-copy size on real N64 hardware: the first 0x100000
/// ROM bytes (after the 0x1000-byte header+IPL3 region) are DMA'd to RDRAM
/// starting at the header's entry point. This is a hardware constant, not
/// a discovered value -- see any N64 hardware boot reference (the PI
/// register sequence IPL3 issues is fixed silicon behavior).
pub const BOOT_COPY_ROM_START: u32 = 0x1000;
pub const BOOT_COPY_SIZE: u32 = 0x0010_0000;

/// Name reserved for the always-resident boot/init bank.
pub const BOOT_BANK: &str = "boot";

/// The 4032-byte IPL3 blob occupies ROM `[0x40, 0x1000)`; which build a
/// cartridge carries decides where its CIC-paired IPL3 relocates the 1 MiB
/// boot copy. CIC-6102 and 6105 builds load at the header entry point;
/// the CIC-6103 build loads at `entry point - 0x100000` (public N64 boot
/// documentation, n64brew "CIC-NUS-610x" / "IPL3"). The digest below was
/// measured directly from a Kirby 64 (US) cartridge dump — the one 6103
/// title in the local corpus — and cross-checked by clustering IPL3
/// digests across 14 local ROMs: the 6102 cluster (SM64, GoldenEye, four
/// AKI titles) and 6105 cluster (OoT, MM, Perfect Dark) share their own
/// distinct blobs and a zero delta, and Kirby's decomp places `main` at
/// exactly `entry - 0x100000` (0x80000400), confirming the delta on real
/// data. An unrecognized IPL3 keeps the zero-delta reading — the behavior
/// for every non-6103 blob observed so far — rather than guessing.
const IPL3_SHA256_CIC_6103: &str =
    "bf3620d30817007091ebe9bddd1b88c23b8a0052170b3309cde5b6b4238e45e7";

const IPL3_ROM_START: usize = 0x40;
const IPL3_ROM_END: usize = 0x1000;

fn boot_load_delta(rom_bytes: &[u8]) -> (u32, &'static str) {
    use sha2::Digest as _;
    if rom_bytes.len() < IPL3_ROM_END {
        return (0, "header-only (ROM too short for IPL3)");
    }
    let mut hasher = sha2::Sha256::new();
    hasher.update(&rom_bytes[IPL3_ROM_START..IPL3_ROM_END]);
    let digest = format!("{:x}", hasher.finalize());
    if digest == IPL3_SHA256_CIC_6103 {
        (0x10_0000, "CIC-6103 IPL3 (loads at entry - 0x100000)")
    } else {
        (0, "entry-loading IPL3 (6102/6105-class or unrecognized)")
    }
}

/// Discover the boot-copy bank from the ROM header plus the IPL3 blob.
/// This never fails for a normalized ROM (the header was already validated
/// in Phase 1) and is `Proven` immediately: the mapping is a direct read
/// of hardware-fixed header fields and the hardware-fixed relocation
/// behavior of the identified IPL3 build, not an inference.
pub fn discover_boot_bank(rom: &NormalizedRom, db: &mut FactDb) {
    let (load_delta, ipl3_note) = boot_load_delta(&rom.bytes);
    let va_start = rom.header.entry_point.wrapping_sub(load_delta);
    let rom_start = BOOT_COPY_ROM_START;
    let rom_end = rom_start
        .saturating_add(BOOT_COPY_SIZE)
        .min(rom.len() as u32);
    let va_end = va_start.saturating_add(rom_end - rom_start);

    let mapping = db.insert(Fact::RomMapping {
        bank: BOOT_BANK.to_string(),
        rom_space: RomAddressSpace::Physical,
        rom_start,
        rom_end,
        va_start,
        va_end,
    });
    let evidence = db.insert(Fact::Evidence {
        subject: crate::facts::BankAddr::new(BOOT_BANK, va_start),
        note: format!(
            "IPL3 boot copy: ROM [0x{rom_start:x}, 0x{rom_end:x}) -> VA [0x{va_start:x}, 0x{va_end:x}); \
             entry point read directly from normalized header, size fixed by N64 hardware boot behavior; \
             {ipl3_note}"
        ),
    });

    db.conclude(
        format!("bank:{BOOT_BANK}"),
        ProofState::Proven,
        vec![mapping, evidence],
        "boot_copy_from_header",
    )
    .expect("boot bank is the first conclusion for this subject; cannot violate monotonicity");

    let entry = BankAddr::new(BOOT_BANK, va_start);
    let entry_fact = db.insert(Fact::FunctionEntryClaim {
        target: entry.clone(),
        detector: CandidateDetector::HardwareEntrypoint,
        evidence: FunctionEntryEvidence::RomHeaderEntrypoint,
        proposed_state: ProofState::Proven,
    });
    db.conclude(
        function_entry_subject(&entry),
        ProofState::Proven,
        vec![mapping, evidence, entry_fact],
        "rom_header_entry_after_ipl3_boot_copy",
    )
    .expect("boot entry is the first conclusion for this subject; cannot violate monotonicity");
}

/// One fixed-shape descriptor-table record location, in ROM-record-field
/// order. This module does not scan for a table location by itself --
/// the table's ROM offset and record shape must be supplied by the
/// caller as an explicit, cited claim (e.g. from prior RE, like NW4E's
/// documented table at ROM 0x0539a0), matching the design doc's
/// "overlay descriptor tables" candidate source. Treating an unverified
/// table location as ground truth would violate the "no guessed symbol
/// file" discipline -- so this function accepts the location as an input,
/// records exactly where it came from, and only promotes what parses
/// consistently within it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct DescriptorTableShape {
    /// ROM byte offset of the first record.
    pub table_rom_offset: u32,
    /// Number of records to read.
    pub record_count: u32,
    /// Byte stride between records.
    pub record_stride: u32,
    /// Offset within a record of the big-endian u32 ROM start field.
    pub field_rom_start: u32,
    /// Offset within a record of the big-endian u32 ROM end field
    /// (exclusive).
    pub field_rom_end: u32,
    /// Offset within a record of the big-endian u32 destination VA field.
    pub field_vram_dest: u32,
}

/// One parsed descriptor-table record before it is judged consistent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DescriptorRecord {
    pub index: u32,
    pub rom_start: u32,
    pub rom_end: u32,
    pub vram_dest: u32,
}

/// Read `shape`'s records from `rom` and return them in table order,
/// without yet judging validity. Returns `None` for a record if any of
/// its three fields would read out of the normalized ROM's bounds --
/// that is Phase-1-level malformed input, not a per-record proof
/// decision, so it is surfaced to the caller rather than silently
/// dropped.
pub fn read_descriptor_records(
    rom: &NormalizedRom,
    shape: DescriptorTableShape,
) -> Vec<Option<DescriptorRecord>> {
    (0..shape.record_count)
        .map(|i| {
            let base = shape.table_rom_offset + i * shape.record_stride;
            let rom_start = rom.read_u32((base + shape.field_rom_start) as usize)?;
            let rom_end = rom.read_u32((base + shape.field_rom_end) as usize)?;
            let vram_dest = rom.read_u32((base + shape.field_vram_dest) as usize)?;
            Some(DescriptorRecord {
                index: i,
                rom_start,
                rom_end,
                vram_dest,
            })
        })
        .collect()
}

/// Judge each record and, if it passes the bounded self-consistency
/// checks below, add a `RomMapping` fact and a `Proven` conclusion.
/// A record is accepted only if:
///
/// - both ROM fields parsed (in-bounds reads),
/// - `rom_end > rom_start` (non-empty, non-inverted interval),
/// - the ROM interval fits inside the normalized ROM's own bounds,
/// - the implied VA interval is well-formed (`vram_dest` is nonzero and
///   the interval length matches the ROM interval length by construction).
///
/// A record that fails any check is **not** silently dropped: it gets an
/// explicit `Rejected` conclusion citing which check failed, so the
/// unresolved/rejected frontier stays visible per the design doc's
/// classification discipline. Every record present in the table produces
/// exactly one conclusion, accepted or not.
pub fn scan_descriptor_table(
    rom: &NormalizedRom,
    shape: DescriptorTableShape,
    bank_name: impl Fn(u32) -> String,
    db: &mut FactDb,
) -> Vec<String> {
    let records = read_descriptor_records(rom, shape);
    let mut accepted_banks = Vec::new();

    for (i, record) in records.into_iter().enumerate() {
        let idx = i as u32;
        let bank = bank_name(idx);
        let subject = format!("bank:{bank}");

        let Some(rec) = record else {
            db.conclude(
                &subject,
                ProofState::Open,
                vec![],
                "descriptor_table_record_out_of_bounds",
            )
            .expect("first conclusion for this subject");
            continue;
        };

        let rom_len = rom.len() as u32;
        let well_formed = rec.rom_end > rec.rom_start
            && rec.rom_end <= rom_len
            && rec.rom_start <= rom_len
            && rec.vram_dest != 0;

        let evidence = db.insert(Fact::Evidence {
            subject: crate::facts::BankAddr::new(&bank, rec.vram_dest),
            note: format!(
                "descriptor table record {idx} at ROM offset 0x{:x}: rom=[0x{:x},0x{:x}) vram_dest=0x{:x}",
                shape.table_rom_offset + idx * shape.record_stride,
                rec.rom_start,
                rec.rom_end,
                rec.vram_dest
            ),
        });

        if !well_formed {
            db.conclude(
                &subject,
                ProofState::Rejected,
                vec![evidence],
                "descriptor_table_record_malformed",
            )
            .expect("first conclusion for this subject");
            continue;
        }

        let va_start = rec.vram_dest;
        let va_end = va_start + (rec.rom_end - rec.rom_start);
        let mapping = db.insert(Fact::RomMapping {
            bank: bank.clone(),
            rom_space: RomAddressSpace::Physical,
            rom_start: rec.rom_start,
            rom_end: rec.rom_end,
            va_start,
            va_end,
        });
        db.conclude(
            &subject,
            ProofState::Proven,
            vec![mapping, evidence],
            "descriptor_table_self_consistent_record",
        )
        .expect("first conclusion for this subject");
        accepted_banks.push(bank);
    }

    accepted_banks
}

/// Location of a table in either physical cartridge ROM or the VROM
/// namespace resolved by an earlier file-table record.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct TableLocation {
    pub space: RomAddressSpace,
    pub offset: u32,
}

/// Field offsets for the source interval in one table record.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceRangeFields {
    pub space: RomAddressSpace,
    pub field_start: u32,
    pub field_end: u32,
}

/// Address space named by a table record's destination interval.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DestinationSpace {
    PhysicalRom,
    Vram,
}

/// How to obtain a destination interval's exclusive end. `FieldOrSourceLength`
/// models DMA file tables whose zero physical end denotes an uncompressed file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DestinationEnd {
    Field(u32),
    SourceLength,
    FieldOrSourceLength(u32),
}

/// Field offsets for the destination interval in one table record.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct DestinationRangeFields {
    pub space: DestinationSpace,
    pub field_start: u32,
    pub end: DestinationEnd,
}

/// A configurable table whose records map one ROM/VROM interval to either a
/// physical ROM file or a VRAM load range. The same shape describes OoT-style
/// file tables and overlay tables; a physical-ROM-to-VRAM shape also subsumes
/// the older AKI descriptor-table form.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct LoadImageTableShape {
    pub location: TableLocation,
    pub record_count: u32,
    pub record_stride: u32,
    pub source: SourceRangeFields,
    pub destination: DestinationRangeFields,
}

/// Deterministic, serializable bank naming for records in one table. Keeping
/// this as data rather than a function pointer lets the same validated input
/// come from an inferred fact pack or an external ROM-bound manifest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BankNamePattern {
    pub prefix: String,
    #[serde(default)]
    pub suffix: String,
    #[serde(default)]
    pub index_base: u32,
}

impl BankNamePattern {
    pub fn new(prefix: impl Into<String>, index_base: u32, suffix: impl Into<String>) -> Self {
        Self {
            prefix: prefix.into(),
            suffix: suffix.into(),
            index_base,
        }
    }

    pub fn name(&self, index: u32) -> String {
        format!("{}{}{}", self.prefix, index + self.index_base, self.suffix)
    }
}

/// Explicit per-title data for one mapping table. `bank_name` is required for
/// VRAM destinations and absent for VROM-to-physical file tables.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LoadImageTableInput {
    pub name: String,
    pub shape: LoadImageTableShape,
    pub bank_name: Option<BankNamePattern>,
}

/// Turn one uniquely admitted ROM-only overlay-table recovery into the same
/// proven bank-qualified load-image representation as an explicitly located
/// descriptor table.
///
/// Recovery is authoritative only when exactly one distinct table survives.
/// Within that table, a record becomes a proven mapping only when its own
/// delta vote is admitted and the inferred VA exactly equals the destination
/// independently parsed from the descriptor record. A delta vote by itself
/// remains candidate evidence; the agreement of two separately derived
/// fields under a unique table admission is the proof rule.
pub fn scan_recovered_overlay_regions(
    rom: &NormalizedRom,
    recovery: &crate::overlay_regions::OverlayRecovery,
    table_name: &str,
    bank_name: &BankNamePattern,
    db: &mut FactDb,
) -> Vec<String> {
    assert!(
        !table_name.trim().is_empty(),
        "recovered overlay table name must not be empty"
    );

    let admitted: Vec<_> = recovery
        .admissions
        .iter()
        .filter(|admission| admission.admitted)
        .collect();
    let table_subject = format!("load-image-table:{table_name}");
    let selection = db.insert(Fact::Evidence {
        subject: BankAddr::new(
            table_name,
            admitted
                .first()
                .map_or(0, |admission| admission.table.table_rom_offset),
        ),
        note: format!(
            "ROM-only descriptor-family recovery: {} distinct candidate table(s), {} admitted by delta_vote",
            recovery.candidate_tables.len(),
            admitted.len()
        ),
    });

    let [admission] = admitted.as_slice() else {
        let (state, rule) = if admitted.is_empty() {
            (
                ProofState::Open,
                "recovered_overlay_table_has_no_unique_admission",
            )
        } else {
            (
                ProofState::Conflict,
                "recovered_overlay_table_has_multiple_admissions",
            )
        };
        db.conclude(table_subject, state, vec![selection], rule)
            .expect("first conclusion for recovered overlay table");
        return Vec::new();
    };

    assert_eq!(
        admission.table.records.len(),
        admission.region_deltas.len(),
        "overlay recovery must report one delta outcome per record"
    );
    assert_eq!(
        admission.mapped_regions as usize,
        admission
            .region_deltas
            .iter()
            .filter(|delta| delta.is_some())
            .count(),
        "overlay recovery mapped_regions must match its delta outcomes"
    );

    let table = &admission.table;
    let mut accepted_banks = Vec::new();
    let mut table_evidence = vec![selection];
    for (index, (record, delta_outcome)) in table
        .records
        .iter()
        .zip(&admission.region_deltas)
        .enumerate()
    {
        let index = index as u32;
        let bank = bank_name.name(index);
        let record_subject = load_image_table_record_subject(table_name, index);
        let Some(byte_len) = record.rom_end.checked_sub(record.rom_start) else {
            conclude_record_and_bank(
                db,
                &record_subject,
                Some(&bank),
                ProofState::Rejected,
                vec![selection],
                "recovered_overlay_record_inverted",
            );
            continue;
        };
        let destination_end = record.vram_dest.checked_add(byte_len);
        let interval_valid = byte_len != 0
            && record.rom_end <= rom.len() as u32
            && record.rom_start.is_multiple_of(4)
            && record.rom_end.is_multiple_of(4)
            && record.vram_dest.is_multiple_of(4)
            && destination_end.is_some();
        let destination_end = destination_end.unwrap_or(record.vram_dest);

        let record_fact = db.insert(Fact::LoadImageTableRecord {
            table: table_name.to_string(),
            bank: Some(bank.clone()),
            table_space: RomAddressSpace::Physical,
            table_offset: table.table_rom_offset,
            index,
            source_space: MappingAddressSpace::PhysicalRom,
            source_start: record.rom_start,
            source_end: record.rom_end,
            destination_space: MappingAddressSpace::Vram,
            destination_start: record.vram_dest,
            destination_end,
        });
        let delta_note = match delta_outcome {
            Some((delta, va_start)) => {
                format!("delta=0x{delta:08x}, inferred VA=0x{va_start:08x}")
            }
            None => "delta_vote remained open".to_string(),
        };
        let provenance = db.insert(Fact::Evidence {
            subject: BankAddr::new(&bank, record.vram_dest),
            note: format!(
                "uniquely admitted ROM-only descriptor table at 0x{:x}, record {index}: ROM [0x{:x},0x{:x}) -> descriptor VA 0x{:08x}; {delta_note}",
                table.table_rom_offset,
                record.rom_start,
                record.rom_end,
                record.vram_dest,
            ),
        });
        let mut evidence = vec![selection, record_fact, provenance];
        table_evidence.extend([record_fact, provenance]);

        if !interval_valid {
            conclude_record_and_bank(
                db,
                &record_subject,
                Some(&bank),
                ProofState::Rejected,
                evidence,
                "recovered_overlay_record_malformed",
            );
            continue;
        }

        let Some((delta, va_start)) = *delta_outcome else {
            conclude_record_and_bank(
                db,
                &record_subject,
                Some(&bank),
                ProofState::Open,
                evidence,
                "recovered_overlay_record_delta_open",
            );
            continue;
        };
        if record.rom_start.wrapping_add(delta) != va_start || va_start != record.vram_dest {
            conclude_record_and_bank(
                db,
                &record_subject,
                Some(&bank),
                ProofState::Conflict,
                evidence,
                "recovered_overlay_delta_conflicts_with_descriptor_destination",
            );
            continue;
        }

        let mapping = db.insert(Fact::RomMapping {
            bank: bank.clone(),
            rom_space: RomAddressSpace::Physical,
            rom_start: record.rom_start,
            rom_end: record.rom_end,
            va_start,
            va_end: destination_end,
        });
        evidence.push(mapping);
        db.conclude(
            &record_subject,
            ProofState::Proven,
            evidence.clone(),
            "unique_recovered_overlay_record_with_matching_delta_and_destination",
        )
        .expect("first conclusion for recovered overlay record");
        db.conclude(
            format!("bank:{bank}"),
            ProofState::Proven,
            evidence,
            "unique_recovered_overlay_record_with_matching_delta_and_destination",
        )
        .expect("first conclusion for recovered overlay bank");
        surface_mapping_conflicts(db, record_fact);
        if db
            .conclusion(&format!("bank:{bank}"))
            .is_some_and(|conclusion| conclusion.state == ProofState::Proven)
        {
            accepted_banks.push(bank);
        }
    }

    db.conclude(
        table_subject,
        ProofState::Proven,
        table_evidence,
        "unique_recovered_overlay_table_admission",
    )
    .expect("first conclusion for recovered overlay table");
    accepted_banks
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MaterializedRomRange {
    pub bytes: Vec<u8>,
    pub backing_evidence: Vec<usize>,
}

/// Materialize a physical-ROM or VROM interval. VROM bytes are accepted only
/// through exactly one proven file-table record; compressed files must carry
/// a valid Yaz0 stream whose declared output length matches that record.
pub fn materialize_rom_range(
    rom: &NormalizedRom,
    db: &FactDb,
    space: RomAddressSpace,
    start: u32,
    end: u32,
) -> Result<MaterializedRomRange, String> {
    if end <= start {
        return Err(format!("empty or inverted range [0x{start:x},0x{end:x})"));
    }
    if space == RomAddressSpace::Physical {
        let bytes = rom
            .bytes
            .get(start as usize..end as usize)
            .ok_or_else(|| {
                format!(
                    "physical ROM range [0x{start:x},0x{end:x}) exceeds normalized ROM length 0x{:x}",
                    rom.len()
                )
            })?
            .to_vec();
        return Ok(MaterializedRomRange {
            bytes,
            backing_evidence: vec![],
        });
    }

    let mut matches = Vec::new();
    for (fact_index, fact) in db.proven_vrom_file_mappings() {
        let Fact::LoadImageTableRecord {
            source_start,
            source_end,
            destination_start,
            destination_end,
            ..
        } = fact
        else {
            unreachable!("proven_vrom_file_mappings returned another fact kind")
        };
        if start >= *source_start && end <= *source_end {
            matches.push((
                fact_index,
                *source_start,
                *source_end,
                *destination_start,
                *destination_end,
            ));
        }
    }
    if matches.len() != 1 {
        return Err(format!(
            "VROM range [0x{start:x},0x{end:x}) has {} proven physical file mappings; expected exactly one",
            matches.len()
        ));
    }
    let (fact_index, vrom_start, vrom_end, physical_start, physical_end) = matches[0];
    let physical = rom
        .bytes
        .get(physical_start as usize..physical_end as usize)
        .ok_or_else(|| {
            format!(
                "file backing [0x{physical_start:x},0x{physical_end:x}) exceeds normalized ROM length 0x{:x}",
                rom.len()
            )
        })?;
    let expected_len = (vrom_end - vrom_start) as usize;
    let file = if physical.starts_with(b"Yaz0") {
        decompress_yaz0(physical, expected_len)?
    } else {
        if physical.len() != expected_len {
            return Err(format!(
                "non-Yaz0 file backing length 0x{:x} does not match VROM length 0x{expected_len:x}",
                physical.len()
            ));
        }
        physical.to_vec()
    };
    let relative_start = (start - vrom_start) as usize;
    let relative_end = (end - vrom_start) as usize;
    Ok(MaterializedRomRange {
        bytes: file[relative_start..relative_end].to_vec(),
        backing_evidence: vec![fact_index],
    })
}

/// Scan explicitly supplied table shapes in deterministic dependency order:
/// physical tables first (normally the VROM file table), then tables stored
/// inside VROM files. Every parseable record becomes typed evidence and a
/// proof-state conclusion; malformed, unbacked, or conflicting records remain
/// visible instead of being dropped.
pub fn scan_load_image_tables(
    rom: &NormalizedRom,
    inputs: &[LoadImageTableInput],
    db: &mut FactDb,
) -> Vec<String> {
    let mut ordered: Vec<_> = inputs.iter().collect();
    ordered.sort_by_key(|input| {
        (
            input.shape.location.space == RomAddressSpace::Virtual,
            input.name.as_str(),
        )
    });
    let mut accepted_banks = Vec::new();

    for input in ordered {
        let shape = input.shape;
        let max_field = [
            shape.source.field_start,
            shape.source.field_end,
            shape.destination.field_start,
            match shape.destination.end {
                DestinationEnd::Field(field) | DestinationEnd::FieldOrSourceLength(field) => field,
                DestinationEnd::SourceLength => shape.destination.field_start,
            },
        ]
        .into_iter()
        .max()
        .unwrap();
        let table_len = shape
            .record_count
            .saturating_sub(1)
            .saturating_mul(shape.record_stride)
            .saturating_add(max_field)
            .saturating_add(4);
        let table_end = shape.location.offset.saturating_add(table_len);
        let table_bytes = match materialize_rom_range(
            rom,
            db,
            shape.location.space,
            shape.location.offset,
            table_end,
        ) {
            Ok(materialized) => materialized,
            Err(error) => {
                let evidence = db.insert(Fact::Evidence {
                    subject: crate::facts::BankAddr::new(&input.name, shape.location.offset),
                    note: format!("table bytes unavailable: {error}"),
                });
                db.conclude(
                    format!("load-image-table:{}", input.name),
                    ProofState::Open,
                    vec![evidence],
                    "load_image_table_bytes_unavailable",
                )
                .expect("first conclusion for this table");
                continue;
            }
        };

        for index in 0..shape.record_count {
            let record_subject = load_image_table_record_subject(&input.name, index);
            let base = (index * shape.record_stride) as usize;
            let read = |field: u32| -> Option<u32> {
                let start = base.checked_add(field as usize)?;
                let bytes = table_bytes.bytes.get(start..start + 4)?;
                Some(u32::from_be_bytes(bytes.try_into().unwrap()))
            };
            let Some(source_start) = read(shape.source.field_start) else {
                db.conclude(
                    &record_subject,
                    ProofState::Open,
                    table_bytes.backing_evidence.clone(),
                    "load_image_table_record_out_of_bounds",
                )
                .expect("first conclusion for this record");
                continue;
            };
            let Some(source_end) = read(shape.source.field_end) else {
                db.conclude(
                    &record_subject,
                    ProofState::Open,
                    table_bytes.backing_evidence.clone(),
                    "load_image_table_record_out_of_bounds",
                )
                .expect("first conclusion for this record");
                continue;
            };
            let Some(destination_start) = read(shape.destination.field_start) else {
                db.conclude(
                    &record_subject,
                    ProofState::Open,
                    table_bytes.backing_evidence.clone(),
                    "load_image_table_record_out_of_bounds",
                )
                .expect("first conclusion for this record");
                continue;
            };
            let source_len = source_end.saturating_sub(source_start);
            let destination_end = match shape.destination.end {
                DestinationEnd::Field(field) => read(field),
                DestinationEnd::SourceLength => destination_start.checked_add(source_len),
                DestinationEnd::FieldOrSourceLength(field) => read(field).and_then(|value| {
                    if value == 0 {
                        destination_start.checked_add(source_len)
                    } else {
                        Some(value)
                    }
                }),
            };
            let Some(destination_end) = destination_end else {
                db.conclude(
                    &record_subject,
                    ProofState::Open,
                    table_bytes.backing_evidence.clone(),
                    "load_image_table_destination_end_unavailable",
                )
                .expect("first conclusion for this record");
                continue;
            };
            let bank = input.bank_name.as_ref().map(|pattern| pattern.name(index));
            let source_space = match shape.source.space {
                RomAddressSpace::Physical => MappingAddressSpace::PhysicalRom,
                RomAddressSpace::Virtual => MappingAddressSpace::VirtualRom,
            };
            let destination_space = match shape.destination.space {
                DestinationSpace::PhysicalRom => MappingAddressSpace::PhysicalRom,
                DestinationSpace::Vram => MappingAddressSpace::Vram,
            };
            let record = db.insert(Fact::LoadImageTableRecord {
                table: input.name.to_string(),
                bank: bank.clone(),
                table_space: shape.location.space,
                table_offset: shape.location.offset,
                index,
                source_space,
                source_start,
                source_end,
                destination_space,
                destination_start,
                destination_end,
            });
            let mut evidence = table_bytes.backing_evidence.clone();
            evidence.push(record);

            let interval_well_formed = source_end > source_start
                && destination_end > destination_start
                && destination_start != 0;
            if !interval_well_formed {
                conclude_record_and_bank(
                    db,
                    &record_subject,
                    bank.as_deref(),
                    ProofState::Rejected,
                    evidence,
                    "load_image_table_record_malformed",
                );
                continue;
            }

            let destination_len = destination_end - destination_start;
            if shape.destination.space == DestinationSpace::Vram && destination_len < source_len {
                conclude_record_and_bank(
                    db,
                    &record_subject,
                    bank.as_deref(),
                    ProofState::Conflict,
                    evidence,
                    "load_image_destination_shorter_than_source",
                );
                continue;
            }

            let backing = match (shape.source.space, shape.destination.space) {
                (RomAddressSpace::Virtual, DestinationSpace::PhysicalRom) => {
                    validate_file_record(rom, source_len, destination_start, destination_end)
                        .map(|()| vec![])
                }
                (_, DestinationSpace::Vram) => {
                    materialize_rom_range(rom, db, shape.source.space, source_start, source_end)
                        .map(|materialized| materialized.backing_evidence)
                }
                (RomAddressSpace::Physical, DestinationSpace::PhysicalRom) => Err(
                    "physical-ROM to physical-ROM table is not a load-image/file mapping".into(),
                ),
            };
            let backing = match backing {
                Ok(backing) => backing,
                Err(error) => {
                    let unavailable = db.insert(Fact::Evidence {
                        subject: crate::facts::BankAddr::new(
                            bank.as_deref().unwrap_or(&input.name),
                            source_start,
                        ),
                        note: format!(
                            "{} record {index} source [0x{source_start:x},0x{source_end:x}) unavailable: {error}",
                            input.name
                        ),
                    });
                    evidence.push(unavailable);
                    conclude_record_and_bank(
                        db,
                        &record_subject,
                        bank.as_deref(),
                        ProofState::Open,
                        evidence,
                        "load_image_source_bytes_unavailable",
                    );
                    continue;
                }
            };
            evidence.extend(backing);

            if shape.destination.space == DestinationSpace::Vram {
                let Some(bank) = bank.clone() else {
                    db.conclude(
                        &record_subject,
                        ProofState::Open,
                        evidence,
                        "load_image_table_missing_bank_namer",
                    )
                    .expect("first conclusion for this record");
                    continue;
                };
                let mapping = db.insert(Fact::RomMapping {
                    bank: bank.clone(),
                    rom_space: shape.source.space,
                    rom_start: source_start,
                    rom_end: source_end,
                    va_start: destination_start,
                    va_end: destination_end,
                });
                evidence.push(mapping);
                db.conclude(
                    &record_subject,
                    ProofState::Proven,
                    evidence.clone(),
                    "load_image_table_self_consistent_record",
                )
                .expect("first conclusion for this record");
                db.conclude(
                    format!("bank:{bank}"),
                    ProofState::Proven,
                    evidence,
                    "load_image_table_self_consistent_record",
                )
                .expect("first conclusion for this bank");
                accepted_banks.push(bank);
            } else {
                db.conclude(
                    &record_subject,
                    ProofState::Proven,
                    evidence,
                    "vrom_file_table_self_consistent_record",
                )
                .expect("first conclusion for this record");
            }

            surface_mapping_conflicts(db, record);
        }
    }

    accepted_banks.retain(|bank| {
        db.conclusion(&format!("bank:{bank}"))
            .is_some_and(|conclusion| conclusion.state == ProofState::Proven)
    });
    accepted_banks
}

fn conclude_record_and_bank(
    db: &mut FactDb,
    record_subject: &str,
    bank: Option<&str>,
    state: ProofState,
    evidence: Vec<usize>,
    rule: &str,
) {
    db.conclude(record_subject, state, evidence.clone(), rule)
        .expect("first conclusion for this record");
    if let Some(bank) = bank {
        db.conclude(format!("bank:{bank}"), state, evidence, rule)
            .expect("first conclusion for this bank");
    }
}

/// A cited claim locating a game's load-request wrapper inside the proven
/// boot image: the callee's entry VA and which argument registers carry the
/// destination pointer, device address, and byte count (from the game's own
/// calling convention, e.g. MM boot's `DmaMgr_RequestAsync(req, ram, vrom,
/// size, ...)`). `device_space` declares what namespace the device operand
/// uses: `Physical` for raw cartridge offsets, `Virtual` for VROM that a DMA
/// manager translates — the latter is only accepted when the recovered range
/// sits inside exactly one already-proven VROM file mapping. The claim says
/// where to look; the boot image's instruction bytes still have to yield
/// fully constant operands, or the site stays an open frontier.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StaticRequestDmaInput {
    pub name: String,
    pub callee_va: u32,
    pub dram_arg_register: u8,
    pub device_arg_register: u8,
    pub size_arg_register: u8,
    /// When set, the size register carries the EXCLUSIVE END device address
    /// instead of a byte count (SM64's `dma_read(dest, srcStart, srcEnd)`
    /// shape); the length is `end - device`, rejected unless positive.
    #[serde(default)]
    pub size_is_end_address: bool,
    pub device_space: RomAddressSpace,
    pub bank_name: BankNamePattern,
}

/// What a request-DMA scan proved and what it left open, for gate reports.
#[derive(Debug, Default)]
pub struct StaticRequestDmaReport {
    pub proven_banks: Vec<String>,
    pub open: Vec<String>,
}

/// The largest RDRAM a retail console reaches (Expansion Pak). Used only to
/// bound destination sanity in the slicer; VA truth is judged downstream.
const SCAN_RDRAM_LEN: u32 = 0x0080_0000;

/// Recover load-image mappings from static operands at direct calls to a
/// cited request wrapper within the proven boot image. Each fully constant
/// (destination, device, size) triple that passes its declared-space
/// validation becomes a `Proven` bank mapping; every other call site is
/// reported open, never guessed. Reachability and completion are recorded as
/// unproven in the evidence note, matching `pi_dma`'s honesty contract.
pub fn scan_static_request_dma(
    rom: &NormalizedRom,
    inputs: &[StaticRequestDmaInput],
    db: &mut FactDb,
) -> StaticRequestDmaReport {
    use crate::loaders::VirtualAddress;
    use std::collections::BTreeSet;

    let mut report = StaticRequestDmaReport::default();
    if inputs.is_empty() {
        return report;
    }
    let boot = db.proven_rom_mappings().iter().find_map(|fact| match fact {
        Fact::RomMapping {
            bank,
            rom_start,
            rom_end,
            va_start,
            ..
        } if bank == BOOT_BANK => Some((*rom_start, *rom_end, *va_start)),
        _ => None,
    });
    let Some((boot_rom_start, boot_rom_end, boot_va_start)) = boot else {
        report
            .open
            .push("boot bank not proven; request-dma scan skipped".to_string());
        return report;
    };
    let words: Vec<u32> = rom.bytes[boot_rom_start as usize..boot_rom_end as usize]
        .chunks_exact(4)
        .map(|chunk| u32::from_be_bytes(chunk.try_into().unwrap()))
        .collect();

    for input in inputs {
        let slices = match crate::pi_dma::slice_load_request_calls(
            &words,
            VirtualAddress::new(boot_va_start),
            VirtualAddress::new(input.callee_va),
            SCAN_RDRAM_LEN,
            input.dram_arg_register,
            input.device_arg_register,
            input.size_arg_register,
        ) {
            Ok(slices) => slices,
            Err(error) => {
                report
                    .open
                    .push(format!("{}: slicer rejected boot image: {error:?}", input.name));
                continue;
            }
        };
        if slices.is_empty() {
            report.open.push(format!(
                "{}: no direct calls to cited callee 0x{:x} in the boot image",
                input.name, input.callee_va
            ));
            continue;
        }
        let mut seen: BTreeSet<(u32, u32, u32)> = BTreeSet::new();
        let mut index = 0u32;
        for slice in slices {
            let call_pc = slice.call_pc.get();
            let (Some(candidate), Some(dram_pointer)) =
                (slice.candidate(), slice.dram_pointer.proven().copied())
            else {
                report.open.push(format!(
                    "{}: call at 0x{call_pc:x} has open operands",
                    input.name
                ));
                continue;
            };
            let device = candidate.device_address.get();
            // In end-address mode the slicer's byte_count carries the raw
            // end operand (its rdram bound check then over-reserves by the
            // device offset — a conservative ceiling, never an undercheck).
            let length = if input.size_is_end_address {
                match candidate.byte_count.get().checked_sub(device) {
                    Some(length) if length > 0 => length,
                    _ => {
                        report.open.push(format!(
                            "{}: call at 0x{call_pc:x} end address 0x{:x} is not \
                             beyond device start 0x{device:x}",
                            input.name,
                            candidate.byte_count.get()
                        ));
                        continue;
                    }
                }
            } else {
                candidate.byte_count.get()
            };
            let va_start = dram_pointer.get();
            if !seen.insert((device, va_start, length)) {
                continue;
            }
            let (Some(device_end), Some(va_end)) =
                (device.checked_add(length), va_start.checked_add(length))
            else {
                report.open.push(format!(
                    "{}: call at 0x{call_pc:x} has an overflowing range",
                    input.name
                ));
                continue;
            };
            match input.device_space {
                RomAddressSpace::Physical => {
                    if device_end as usize > rom.len() {
                        report.open.push(format!(
                            "{}: call at 0x{call_pc:x} physical range \
                             0x{device:x}..0x{device_end:x} exceeds the ROM",
                            input.name
                        ));
                        continue;
                    }
                }
                RomAddressSpace::Virtual => {
                    let containing = db
                        .proven_vrom_file_mappings()
                        .iter()
                        .filter(|(_, fact)| {
                            matches!(fact, Fact::LoadImageTableRecord {
                                source_start,
                                source_end,
                                ..
                            } if device >= *source_start && device_end <= *source_end)
                        })
                        .count();
                    if containing != 1 {
                        report.open.push(format!(
                            "{}: call at 0x{call_pc:x} VROM range \
                             0x{device:x}..0x{device_end:x} has {containing} proven file \
                             mappings; expected exactly one",
                            input.name
                        ));
                        continue;
                    }
                }
            }
            let bank = input.bank_name.name(index);
            index += 1;
            let mapping = db.insert(Fact::RomMapping {
                bank: bank.clone(),
                rom_space: input.device_space,
                rom_start: device,
                rom_end: device_end,
                va_start,
                va_end,
            });
            let evidence = db.insert(Fact::Evidence {
                subject: BankAddr::new(&bank, va_start),
                note: format!(
                    "static request-DMA operands at call 0x{call_pc:x} to cited {} \
                     (0x{:x}): device 0x{device:x}+0x{length:x} -> VA 0x{va_start:x}; \
                     instruction bytes do not prove reachability or completion",
                    input.name, input.callee_va
                ),
            });
            db.conclude(
                format!("bank:{bank}"),
                ProofState::Proven,
                vec![mapping, evidence],
                "static_request_dma_operands",
            )
            .expect("request-dma bank names are freshly generated");
            report.proven_banks.push(bank);
        }
    }
    report
}

fn validate_file_record(
    rom: &NormalizedRom,
    vrom_len: u32,
    physical_start: u32,
    physical_end: u32,
) -> Result<(), String> {
    let physical = rom
        .bytes
        .get(physical_start as usize..physical_end as usize)
        .ok_or_else(|| "physical file interval is outside normalized ROM".to_string())?;
    if physical.starts_with(b"Yaz0") {
        let declared = physical
            .get(4..8)
            .map(|bytes| u32::from_be_bytes(bytes.try_into().unwrap()))
            .ok_or_else(|| "truncated Yaz0 header".to_string())?;
        if declared != vrom_len {
            return Err("Yaz0 declared size does not match VROM interval".into());
        }
    } else if physical.len() != vrom_len as usize {
        return Err("uncompressed physical and VROM lengths differ".into());
    }
    Ok(())
}

fn surface_mapping_conflicts(db: &mut FactDb, new_index: usize) {
    let Fact::LoadImageTableRecord {
        table: new_table,
        bank: new_bank,
        index: new_record,
        source_space: new_source_space,
        source_start: new_source_start,
        source_end: new_source_end,
        destination_space: new_destination_space,
        destination_start: new_destination_start,
        destination_end: new_destination_end,
        ..
    } = &db.facts()[new_index]
    else {
        unreachable!()
    };
    let new_values = (
        new_table.clone(),
        new_bank.clone(),
        *new_record,
        *new_source_space,
        *new_source_start,
        *new_source_end,
        *new_destination_space,
        *new_destination_start,
        *new_destination_end,
    );
    let mut conflicts = Vec::new();
    for (old_index, fact) in db.facts()[..new_index].iter().enumerate() {
        let Fact::LoadImageTableRecord {
            table,
            bank,
            index,
            source_space,
            source_start,
            source_end,
            destination_space,
            destination_start,
            destination_end,
            ..
        } = fact
        else {
            continue;
        };
        if !db
            .conclusion(&load_image_table_record_subject(table, *index))
            .is_some_and(|conclusion| {
                matches!(conclusion.state, ProofState::Proven | ProofState::Conflict)
            })
        {
            continue;
        }
        let exact = *source_space == new_values.3
            && *source_start == new_values.4
            && *source_end == new_values.5
            && *destination_space == new_values.6
            && *destination_start == new_values.7
            && *destination_end == new_values.8;
        if exact || *source_space != new_values.3 || *destination_space != new_values.6 {
            continue;
        }
        let source_overlap = *source_start < new_values.5 && new_values.4 < *source_end;
        let destination_overlap =
            *destination_start < new_values.8 && new_values.7 < *destination_end;
        let conflicts_here = if *destination_space == MappingAddressSpace::PhysicalRom {
            source_overlap
        } else {
            source_overlap && destination_overlap
        };
        if conflicts_here {
            conflicts.push((old_index, table.clone(), bank.clone(), *index));
        }
    }

    for (old_index, old_table, old_bank, old_record) in conflicts {
        let evidence = vec![old_index, new_index];
        db.conclude(
            load_image_table_record_subject(&old_table, old_record),
            ProofState::Conflict,
            evidence.clone(),
            "overlapping_load_image_table_records",
        )
        .expect("proven record may transition to conflict");
        db.conclude(
            load_image_table_record_subject(&new_values.0, new_values.2),
            ProofState::Conflict,
            evidence.clone(),
            "overlapping_load_image_table_records",
        )
        .expect("proven record may transition to conflict");
        for bank in [old_bank.as_deref(), new_values.1.as_deref()]
            .into_iter()
            .flatten()
        {
            db.conclude(
                format!("bank:{bank}"),
                ProofState::Conflict,
                evidence.clone(),
                "overlapping_load_image_table_records",
            )
            .expect("proven bank may transition to conflict");
        }
    }
}

/// Decode the bounded Yaz0 stream shape implemented by the allowed
/// N64Recomp-generated `Yaz0_DecompressImpl` C used for the gate profile.
/// Input and output reads are checked here because discovery must surface a
/// malformed backing file rather than inheriting the game's trusted-input
/// assumptions.
fn decompress_yaz0(input: &[u8], expected_len: usize) -> Result<Vec<u8>, String> {
    if input.len() < 16 || &input[..4] != b"Yaz0" {
        return Err("missing or truncated Yaz0 header".into());
    }
    let declared = u32::from_be_bytes(input[4..8].try_into().unwrap()) as usize;
    if declared != expected_len {
        return Err(format!(
            "Yaz0 output length 0x{declared:x} does not match expected VROM length 0x{expected_len:x}"
        ));
    }
    let mut source = 16usize;
    let mut output = Vec::with_capacity(expected_len);
    let mut control = 0u8;
    let mut bits_left = 0u8;
    while output.len() < expected_len {
        if bits_left == 0 {
            control = *input
                .get(source)
                .ok_or_else(|| "Yaz0 stream ended before next control byte".to_string())?;
            source += 1;
            bits_left = 8;
        }
        if control & 0x80 != 0 {
            output.push(
                *input
                    .get(source)
                    .ok_or_else(|| "Yaz0 literal exceeds input".to_string())?,
            );
            source += 1;
        } else {
            let first = *input
                .get(source)
                .ok_or_else(|| "Yaz0 back-reference exceeds input".to_string())?;
            let second = *input
                .get(source + 1)
                .ok_or_else(|| "Yaz0 back-reference exceeds input".to_string())?;
            source += 2;
            let distance = (((first & 0x0f) as usize) << 8) | second as usize;
            if distance >= output.len() {
                return Err("Yaz0 back-reference precedes output start".into());
            }
            let mut length = (first >> 4) as usize;
            if length == 0 {
                length = *input
                    .get(source)
                    .ok_or_else(|| "Yaz0 extended length exceeds input".to_string())?
                    as usize
                    + 0x12;
                source += 1;
            } else {
                length += 2;
            }
            let copy_start = output.len() - distance - 1;
            for offset in 0..length {
                if output.len() == expected_len {
                    break;
                }
                let byte = output[copy_start + offset];
                output.push(byte);
            }
        }
        control <<= 1;
        bits_left -= 1;
    }
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rom::normalize;

    fn make_test_rom(entry: u32, extra_len: usize) -> NormalizedRom {
        let mut buf = vec![0u8; 0x1000 + extra_len];
        buf[0..4].copy_from_slice(&0x8037_1240u32.to_be_bytes());
        buf[8..12].copy_from_slice(&entry.to_be_bytes());
        buf[0x20..0x24].copy_from_slice(b"TEST");
        buf[0x3b..0x3f].copy_from_slice(b"CTSE");
        normalize(&buf).expect("valid synthetic z64")
    }

    #[test]
    fn boot_bank_reads_directly_from_header_no_scanning() {
        let rom = make_test_rom(0x8000_0400, BOOT_COPY_SIZE as usize + 0x1000);
        let mut db = FactDb::new();
        discover_boot_bank(&rom, &mut db);

        let concl = db.conclusion("bank:boot").expect("boot bank concluded");
        assert_eq!(concl.state, ProofState::Proven);

        let mapping = db
            .facts()
            .iter()
            .find(|f| matches!(f, Fact::RomMapping { bank, .. } if bank == BOOT_BANK))
            .expect("rom mapping fact present");
        match mapping {
            Fact::RomMapping {
                rom_start,
                va_start,
                ..
            } => {
                assert_eq!(*rom_start, BOOT_COPY_ROM_START);
                assert_eq!(*va_start, 0x8000_0400);
            }
            _ => unreachable!(),
        }
    }

    #[test]
    fn boot_bank_clamps_to_short_roms_without_panicking() {
        // A tiny synthetic ROM shorter than a full 0x100000 boot copy must
        // not panic or read out of bounds -- clamp rom_end to actual length.
        let rom = make_test_rom(0x8000_0400, 0x2000);
        let mut db = FactDb::new();
        discover_boot_bank(&rom, &mut db);
        let mapping = db
            .facts()
            .iter()
            .find(|f| matches!(f, Fact::RomMapping { bank, .. } if bank == BOOT_BANK))
            .unwrap();
        match mapping {
            Fact::RomMapping { rom_end, .. } => assert!(*rom_end as usize <= rom.len()),
            _ => unreachable!(),
        }
    }

    fn write_record(buf: &mut [u8], base: usize, rom_start: u32, rom_end: u32, vram: u32) {
        buf[base..base + 4].copy_from_slice(&rom_start.to_be_bytes());
        buf[base + 4..base + 8].copy_from_slice(&rom_end.to_be_bytes());
        buf[base + 8..base + 12].copy_from_slice(&vram.to_be_bytes());
    }

    #[test]
    fn descriptor_table_accepts_well_formed_records_and_rejects_malformed() {
        let mut rom_bytes = vec![0u8; 0x1000 + 0x10000];
        rom_bytes[0..4].copy_from_slice(&0x8037_1240u32.to_be_bytes());
        rom_bytes[8..12].copy_from_slice(&0x8000_0400u32.to_be_bytes());
        rom_bytes[0x20..0x24].copy_from_slice(b"TEST");
        rom_bytes[0x3b..0x3f].copy_from_slice(b"CTSE");

        let table_off = 0x2000usize;
        // record 0: well-formed
        write_record(&mut rom_bytes, table_off, 0x3000, 0x4000, 0x8010_0000);
        // record 1: inverted interval (rom_end < rom_start) -- malformed
        write_record(
            &mut rom_bytes,
            table_off + 0x10,
            0x5000,
            0x4500,
            0x8020_0000,
        );
        // record 2: zero vram_dest -- malformed
        write_record(&mut rom_bytes, table_off + 0x20, 0x6000, 0x7000, 0);

        let rom = normalize(&rom_bytes).unwrap();
        let mut db = FactDb::new();
        let shape = DescriptorTableShape {
            table_rom_offset: table_off as u32,
            record_count: 3,
            record_stride: 0x10,
            field_rom_start: 0,
            field_rom_end: 4,
            field_vram_dest: 8,
        };
        let accepted = scan_descriptor_table(&rom, shape, |i| format!("overlay_{i}"), &mut db);

        assert_eq!(accepted, vec!["overlay_0".to_string()]);
        assert_eq!(
            db.conclusion("bank:overlay_0").unwrap().state,
            ProofState::Proven
        );
        assert_eq!(
            db.conclusion("bank:overlay_1").unwrap().state,
            ProofState::Rejected
        );
        assert_eq!(
            db.conclusion("bank:overlay_2").unwrap().state,
            ProofState::Rejected
        );
    }

    #[test]
    fn descriptor_table_out_of_bounds_record_is_open_not_dropped() {
        let mut rom_bytes = vec![0u8; 0x1000 + 0x100];
        rom_bytes[0..4].copy_from_slice(&0x8037_1240u32.to_be_bytes());
        rom_bytes[8..12].copy_from_slice(&0x8000_0400u32.to_be_bytes());
        rom_bytes[0x20..0x24].copy_from_slice(b"TEST");
        rom_bytes[0x3b..0x3f].copy_from_slice(b"CTSE");
        let rom = normalize(&rom_bytes).unwrap();

        let mut db = FactDb::new();
        let shape = DescriptorTableShape {
            table_rom_offset: 0x2000, // beyond this tiny ROM
            record_count: 1,
            record_stride: 0x10,
            field_rom_start: 0,
            field_rom_end: 4,
            field_vram_dest: 8,
        };
        scan_descriptor_table(&rom, shape, |i| format!("overlay_{i}"), &mut db);
        assert_eq!(
            db.conclusion("bank:overlay_0").unwrap().state,
            ProofState::Open
        );
    }

    #[test]
    fn descriptor_table_scan_is_byte_identical_across_runs() {
        let mut rom_bytes = vec![0u8; 0x1000 + 0x10000];
        rom_bytes[0..4].copy_from_slice(&0x8037_1240u32.to_be_bytes());
        rom_bytes[8..12].copy_from_slice(&0x8000_0400u32.to_be_bytes());
        rom_bytes[0x20..0x24].copy_from_slice(b"TEST");
        rom_bytes[0x3b..0x3f].copy_from_slice(b"CTSE");
        write_record(&mut rom_bytes, 0x2000, 0x3000, 0x4000, 0x8010_0000);
        let rom = normalize(&rom_bytes).unwrap();
        let shape = DescriptorTableShape {
            table_rom_offset: 0x2000,
            record_count: 1,
            record_stride: 0x10,
            field_rom_start: 0,
            field_rom_end: 4,
            field_vram_dest: 8,
        };

        let mut db_a = FactDb::new();
        scan_descriptor_table(&rom, shape, |i| format!("overlay_{i}"), &mut db_a);
        let mut db_b = FactDb::new();
        scan_descriptor_table(&rom, shape, |i| format!("overlay_{i}"), &mut db_b);

        let json_a = serde_json::to_string(&db_a).unwrap();
        let json_b = serde_json::to_string(&db_b).unwrap();
        assert_eq!(json_a, json_b, "repeated generation must be byte-identical");
    }

    fn write_u32(bytes: &mut [u8], offset: usize, value: u32) {
        bytes[offset..offset + 4].copy_from_slice(&value.to_be_bytes());
    }

    fn literal_yaz0(bytes: &[u8]) -> Vec<u8> {
        let mut encoded = Vec::with_capacity(16 + bytes.len() + bytes.len().div_ceil(8));
        encoded.extend_from_slice(b"Yaz0");
        encoded.extend_from_slice(&(bytes.len() as u32).to_be_bytes());
        encoded.extend_from_slice(&[0; 8]);
        for chunk in bytes.chunks(8) {
            encoded.push(0xff);
            encoded.extend_from_slice(chunk);
        }
        encoded
    }

    fn file_table_input(count: u32) -> LoadImageTableInput {
        LoadImageTableInput {
            name: "files".to_string(),
            shape: LoadImageTableShape {
                location: TableLocation {
                    space: RomAddressSpace::Physical,
                    offset: 0x2000,
                },
                record_count: count,
                record_stride: 0x10,
                source: SourceRangeFields {
                    space: RomAddressSpace::Virtual,
                    field_start: 0,
                    field_end: 4,
                },
                destination: DestinationRangeFields {
                    space: DestinationSpace::PhysicalRom,
                    field_start: 8,
                    end: DestinationEnd::FieldOrSourceLength(0xc),
                },
            },
            bank_name: None,
        }
    }

    #[test]
    fn generalized_tables_resolve_vrom_table_and_yaz0_load_image() {
        let mut rom_bytes = vec![0u8; 0x6000];
        write_u32(&mut rom_bytes, 0, 0x8037_1240);
        write_u32(&mut rom_bytes, 8, 0x8000_0400);
        rom_bytes[0x20..0x24].copy_from_slice(b"TEST");
        rom_bytes[0x3b..0x3f].copy_from_slice(b"CTSE");

        let mut overlay_table = vec![0u8; 0x20];
        write_u32(&mut overlay_table, 0, 0x0002_0000);
        write_u32(&mut overlay_table, 4, 0x0002_0010);
        write_u32(&mut overlay_table, 8, 0x8080_0000);
        write_u32(&mut overlay_table, 0xc, 0x8080_0020);
        let compressed_table = literal_yaz0(&overlay_table);
        rom_bytes[0x3000..0x3000 + compressed_table.len()].copy_from_slice(&compressed_table);

        let overlay_bytes: Vec<u8> = (0..0x10).map(|value| value as u8).collect();
        rom_bytes[0x4000..0x4010].copy_from_slice(&overlay_bytes);

        write_u32(&mut rom_bytes, 0x2000, 0x0001_0000);
        write_u32(&mut rom_bytes, 0x2004, 0x0001_0020);
        write_u32(&mut rom_bytes, 0x2008, 0x3000);
        write_u32(
            &mut rom_bytes,
            0x200c,
            0x3000 + compressed_table.len() as u32,
        );
        write_u32(&mut rom_bytes, 0x2010, 0x0002_0000);
        write_u32(&mut rom_bytes, 0x2014, 0x0002_0010);
        write_u32(&mut rom_bytes, 0x2018, 0x4000);
        write_u32(&mut rom_bytes, 0x201c, 0);

        let overlay = LoadImageTableInput {
            name: "effects".to_string(),
            shape: LoadImageTableShape {
                location: TableLocation {
                    space: RomAddressSpace::Virtual,
                    offset: 0x0001_0000,
                },
                record_count: 1,
                record_stride: 0x20,
                source: SourceRangeFields {
                    space: RomAddressSpace::Virtual,
                    field_start: 0,
                    field_end: 4,
                },
                destination: DestinationRangeFields {
                    space: DestinationSpace::Vram,
                    field_start: 8,
                    end: DestinationEnd::Field(0xc),
                },
            },
            bank_name: Some(BankNamePattern::new("effect_", 0, "")),
        };
        let rom = normalize(&rom_bytes).unwrap();
        let mut db = FactDb::new();
        let accepted =
            scan_load_image_tables(&rom, &[overlay.clone(), file_table_input(2)], &mut db);

        assert_eq!(accepted, ["effect_0"]);
        assert_eq!(
            db.conclusion("bank:effect_0").unwrap().state,
            ProofState::Proven
        );
        let record = db
            .facts()
            .iter()
            .find(|fact| {
                matches!(
                    fact,
                    Fact::LoadImageTableRecord { table, index: 0, .. }
                        if table == "effects"
                )
            })
            .expect("typed table/record evidence");
        assert!(matches!(
            record,
            Fact::LoadImageTableRecord {
                source_start: 0x0002_0000,
                destination_start: 0x8080_0000,
                ..
            }
        ));
        let materialized = materialize_rom_range(
            &rom,
            &db,
            RomAddressSpace::Virtual,
            0x0002_0000,
            0x0002_0010,
        )
        .unwrap();
        assert_eq!(materialized.bytes, overlay_bytes);

        let mut repeated = FactDb::new();
        scan_load_image_tables(&rom, &[overlay, file_table_input(2)], &mut repeated);
        assert_eq!(
            serde_json::to_string(&db).unwrap(),
            serde_json::to_string(&repeated).unwrap(),
            "generalized table discovery must be byte-identical"
        );
    }

    #[test]
    fn overlapping_source_and_vram_images_surface_as_conflict() {
        let mut rom_bytes = vec![0u8; 0x6000];
        write_u32(&mut rom_bytes, 0, 0x8037_1240);
        write_u32(&mut rom_bytes, 8, 0x8000_0400);
        rom_bytes[0x20..0x24].copy_from_slice(b"TEST");
        rom_bytes[0x3b..0x3f].copy_from_slice(b"CTSE");
        for (base, source_start, source_end, vram_start, vram_end) in [
            (0x2000, 0x4000, 0x4020, 0x8010_0000, 0x8010_0020),
            (0x2010, 0x4010, 0x4030, 0x8010_0010, 0x8010_0030),
        ] {
            write_u32(&mut rom_bytes, base, source_start);
            write_u32(&mut rom_bytes, base + 4, source_end);
            write_u32(&mut rom_bytes, base + 8, vram_start);
            write_u32(&mut rom_bytes, base + 0xc, vram_end);
        }
        let input = LoadImageTableInput {
            name: "overlays".to_string(),
            shape: LoadImageTableShape {
                location: TableLocation {
                    space: RomAddressSpace::Physical,
                    offset: 0x2000,
                },
                record_count: 2,
                record_stride: 0x10,
                source: SourceRangeFields {
                    space: RomAddressSpace::Physical,
                    field_start: 0,
                    field_end: 4,
                },
                destination: DestinationRangeFields {
                    space: DestinationSpace::Vram,
                    field_start: 8,
                    end: DestinationEnd::Field(0xc),
                },
            },
            bank_name: Some(BankNamePattern::new("overlay_", 0, "")),
        };
        let rom = normalize(&rom_bytes).unwrap();
        let mut db = FactDb::new();
        scan_load_image_tables(&rom, &[input], &mut db);

        for index in 0..2 {
            assert_eq!(
                db.conclusion(&format!("bank:overlay_{index}"))
                    .unwrap()
                    .state,
                ProofState::Conflict
            );
            assert_eq!(
                db.conclusion(&load_image_table_record_subject("overlays", index))
                    .unwrap()
                    .state,
                ProofState::Conflict
            );
        }
        assert!(db.proven_rom_mappings().is_empty());
    }

    fn recovered_table(
        table_rom_offset: u32,
        rom_start: u32,
        vram_dest: u32,
        inferred_va: u32,
    ) -> crate::overlay_regions::TableAdmission {
        let table = crate::overlay_regions::CandidateTable {
            table_rom_offset,
            record_stride: 0x24,
            field_rom_start: 0x18,
            field_rom_end: 0x1c,
            field_vram_dest: 0x20,
            records: vec![crate::overlay_regions::CandidateRecord {
                rom_start,
                rom_end: rom_start + 0x1000,
                vram_dest,
            }],
        };
        crate::overlay_regions::TableAdmission {
            table,
            region_deltas: vec![Some((inferred_va.wrapping_sub(rom_start), inferred_va))],
            mapped_regions: 1,
            admitted: true,
        }
    }

    fn recovery_with(
        admissions: Vec<crate::overlay_regions::TableAdmission>,
    ) -> crate::overlay_regions::OverlayRecovery {
        crate::overlay_regions::OverlayRecovery {
            config: crate::overlay_regions::SearchConfig::aki_family(),
            delta_config: crate::delta_vote::DeltaVoteConfig::default(),
            min_mapped_regions: 1,
            candidate_tables: admissions
                .iter()
                .map(|admission| admission.table.clone())
                .collect(),
            admissions,
        }
    }

    #[test]
    fn unique_recovered_table_with_matching_delta_proves_load_image() {
        let rom = make_test_rom(0x8000_0400, 0x5000);
        let recovery = recovery_with(vec![recovered_table(
            0x1800,
            0x2000,
            0x8010_0000,
            0x8010_0000,
        )]);
        let mut db = FactDb::new();
        let banks = scan_recovered_overlay_regions(
            &rom,
            &recovery,
            "recovered_overlays",
            &BankNamePattern::new("overlay_", 0, ""),
            &mut db,
        );

        assert_eq!(banks, ["overlay_0"]);
        assert_eq!(
            db.conclusion("bank:overlay_0").unwrap().state,
            ProofState::Proven
        );
        assert_eq!(
            db.conclusion(&load_image_table_record_subject("recovered_overlays", 0))
                .unwrap()
                .state,
            ProofState::Proven
        );
        assert!(db.facts().iter().any(|fact| matches!(
            fact,
            Fact::RomMapping {
                bank,
                rom_start: 0x2000,
                rom_end: 0x3000,
                va_start: 0x8010_0000,
                va_end: 0x8010_1000,
                ..
            } if bank == "overlay_0"
        )));
    }

    #[test]
    fn recovered_delta_disagreeing_with_descriptor_stays_conflict() {
        let rom = make_test_rom(0x8000_0400, 0x5000);
        let recovery = recovery_with(vec![recovered_table(
            0x1800,
            0x2000,
            0x8010_0000,
            0x8010_1000,
        )]);
        let mut db = FactDb::new();
        let banks = scan_recovered_overlay_regions(
            &rom,
            &recovery,
            "recovered_overlays",
            &BankNamePattern::new("overlay_", 0, ""),
            &mut db,
        );

        assert!(banks.is_empty());
        assert_eq!(
            db.conclusion("bank:overlay_0").unwrap().state,
            ProofState::Conflict
        );
        assert!(db.proven_rom_mappings().is_empty());
    }

    #[test]
    fn multiple_recovered_table_admissions_map_nothing() {
        let rom = make_test_rom(0x8000_0400, 0x5000);
        let recovery = recovery_with(vec![
            recovered_table(0x1800, 0x2000, 0x8010_0000, 0x8010_0000),
            recovered_table(0x1900, 0x3000, 0x8020_0000, 0x8020_0000),
        ]);
        let mut db = FactDb::new();
        let banks = scan_recovered_overlay_regions(
            &rom,
            &recovery,
            "recovered_overlays",
            &BankNamePattern::new("overlay_", 0, ""),
            &mut db,
        );

        assert!(banks.is_empty());
        assert_eq!(
            db.conclusion("load-image-table:recovered_overlays")
                .unwrap()
                .state,
            ProofState::Conflict
        );
        assert!(db.proven_rom_mappings().is_empty());
    }

    #[test]
    fn unrecognized_ipl3_keeps_entry_loading_delta() {
        // Any blob that is not the measured 6103 IPL3 must keep the
        // zero-delta reading, including a ROM too short to hold IPL3 at
        // all — never a guessed relocation.
        let (delta, _) = super::boot_load_delta(&[0u8; IPL3_ROM_END]);
        assert_eq!(delta, 0);
        let (delta, note) = super::boot_load_delta(&[0u8; 0x100]);
        assert_eq!(delta, 0);
        assert!(note.contains("too short"));
    }
}
