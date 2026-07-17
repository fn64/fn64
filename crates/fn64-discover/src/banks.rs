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
    load_image_table_record_subject, Fact, FactDb, MappingAddressSpace, ProofState, RomAddressSpace,
};
use crate::rom::NormalizedRom;

/// IPL3's fixed boot-copy size on real N64 hardware: the first 0x100000
/// ROM bytes (after the 0x1000-byte header+IPL3 region) are DMA'd to RDRAM
/// starting at the header's entry point. This is a hardware constant, not
/// a discovered value -- see any N64 hardware boot reference (the PI
/// register sequence IPL3 issues is fixed silicon behavior).
pub const BOOT_COPY_ROM_START: u32 = 0x1000;
pub const BOOT_COPY_SIZE: u32 = 0x0010_0000;

/// Name reserved for the always-resident boot/init bank.
pub const BOOT_BANK: &str = "boot";

/// Discover the boot-copy bank from the ROM header alone. This never fails
/// for a normalized ROM (the header was already validated in Phase 1) and
/// is `Proven` immediately: the mapping is a direct read of hardware-fixed
/// header fields, not an inference.
pub fn discover_boot_bank(rom: &NormalizedRom, db: &mut FactDb) {
    let va_start = rom.header.entry_point;
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
             entry point read directly from normalized header, size fixed by N64 hardware boot behavior"
        ),
    });

    db.conclude(
        format!("bank:{BOOT_BANK}"),
        ProofState::Proven,
        vec![mapping, evidence],
        "boot_copy_from_header",
    )
    .expect("boot bank is the first conclusion for this subject; cannot violate monotonicity");
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
#[derive(Debug, Clone, Copy)]
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
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TableLocation {
    pub space: RomAddressSpace,
    pub offset: u32,
}

/// Field offsets for the source interval in one table record.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SourceRangeFields {
    pub space: RomAddressSpace,
    pub field_start: u32,
    pub field_end: u32,
}

/// Address space named by a table record's destination interval.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DestinationSpace {
    PhysicalRom,
    Vram,
}

/// How to obtain a destination interval's exclusive end. `FieldOrSourceLength`
/// models DMA file tables whose zero physical end denotes an uncompressed file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DestinationEnd {
    Field(u32),
    SourceLength,
    FieldOrSourceLength(u32),
}

/// Field offsets for the destination interval in one table record.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DestinationRangeFields {
    pub space: DestinationSpace,
    pub field_start: u32,
    pub end: DestinationEnd,
}

/// A configurable table whose records map one ROM/VROM interval to either a
/// physical ROM file or a VRAM load range. The same shape describes OoT-style
/// file tables and overlay tables; a physical-ROM-to-VRAM shape also subsumes
/// the older AKI descriptor-table form.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LoadImageTableShape {
    pub location: TableLocation,
    pub record_count: u32,
    pub record_stride: u32,
    pub source: SourceRangeFields,
    pub destination: DestinationRangeFields,
}

/// Explicit per-title data for one mapping table. `bank_name` is required for
/// VRAM destinations and absent for VROM-to-physical file tables.
#[derive(Debug, Clone, Copy)]
pub struct LoadImageTableInput {
    pub name: &'static str,
    pub shape: LoadImageTableShape,
    pub bank_name: Option<fn(u32) -> String>,
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
            input.name,
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
                    subject: crate::facts::BankAddr::new(input.name, shape.location.offset),
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
            let record_subject = load_image_table_record_subject(input.name, index);
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
            let bank = input.bank_name.map(|name| name(index));
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
                            bank.as_deref().unwrap_or(input.name),
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
            name: "files",
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
            name: "effects",
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
            bank_name: Some(|index| format!("effect_{index}")),
        };
        let rom = normalize(&rom_bytes).unwrap();
        let mut db = FactDb::new();
        let accepted = scan_load_image_tables(&rom, &[overlay, file_table_input(2)], &mut db);

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
            name: "overlays",
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
            bank_name: Some(|index| format!("overlay_{index}")),
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
}
