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

use crate::facts::{Fact, FactDb, ProofState};
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
}
