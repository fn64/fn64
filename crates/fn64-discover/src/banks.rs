//! Phase 2 (docs/DISCOVER-DESIGN.md "discover load images before
//! functions"): find candidate ROM-to-RDRAM mappings *before* any function
//! discovery runs, so function identity can be bank-qualified from the
//! first instruction.
//!
//! This module implements two mechanical detectors rather than heuristic
//! mapping guesses:
//!
//! 1. **The boot copy.** The admitted standard IPL3 builds DMA a fixed-size
//!    prefix of the ROM (0x100000 bytes for a PI-based CIC boot, starting
//!    at ROM offset 0x1000) to the RDRAM VA determined from the header entry
//!    and the exact IPL3 relocation profile. This is `bank "boot"`, but it
//!    becomes `Proven` only when the complete IPL3 has an exact recognized
//!    identity that establishes its relocation behavior.
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
/// starting at the destination selected by the exact IPL3 build. The source
/// extent is a hardware constant, not a discovered value -- see public N64
/// IPL3/PI boot documentation.
pub const BOOT_COPY_ROM_START: u32 = 0x1000;
pub const BOOT_COPY_SIZE: u32 = 0x0010_0000;

/// Name reserved for the always-resident boot/init bank.
pub const BOOT_BANK: &str = "boot";

/// The 4032-byte IPL3 blob occupies ROM `[0x40, 0x1000)`; which build a
/// cartridge carries decides where its CIC-paired IPL3 relocates the 1 MiB
/// boot copy. CIC-6102 and 6105 builds load at the header entry point;
/// the CIC-6103 build loads at `entry point - 0x100000` (public N64 boot
/// documentation, n64brew "CIC-NUS-610x" / "IPL3"). The digest below was
/// measured directly from a permitted Kirby 64 (US) cartridge dump and later
/// clustered with the permitted Banjo/Kirby 6103-family inputs. The 6102
/// cluster (SM64, GoldenEye, four AKI titles) and 6105 cluster (OoT, MM,
/// Perfect Dark) share their own distinct blobs and a zero delta, and Kirby's
/// decomp places `main` at
/// exactly `entry - 0x100000` (0x80000400), confirming the delta on real
/// data. The 6102/7101 and 6105/7105 SHA-256 values were measured across the
/// permitted local corpus and cross-checked against the matching IPL3 MD5
/// clusters in Dragorn421/n64checksum (CC0-1.0,
/// <https://github.com/Dragorn421/n64checksum>). No other digest inherits
/// their behavior.
///
/// The 6106/7106 and 7102 digests were added later from the same permitted
/// local corpus, which contains five distinct IPL3 blobs rather than three.
/// Both were identified by re-deriving their IPL3 MD5 *and* CRC32 from local
/// ROM bytes and matching the published clusters in Dragorn421/n64checksum
/// (6106/7106 MD5 `6460387749AC0BD925AA5430BC7864FE`, CRC32 `0xACC8580A`;
/// 7102 MD5 `955894C2E40A698BF98A67B78A4E28FA`, CRC32 `0x009E9EA3`); the same
/// procedure reproduced all three digests above, confirming the method.
///
/// Their deltas are proven two independent ways. Documentary: en64
/// <https://en64.shoutwiki.com/wiki/ROM> records that 6101/6102/6105 do not
/// relocate, 6103 subtracts 0x100000, and 6106 subtracts 0x200000, and states
/// 7102 is the PAL counterpart of the non-relocating 6101 type. Empirical:
/// counting boot-copy `jal` targets that land inside
/// `[entry - delta, entry - delta + 0x100000)` selects exactly one delta per
/// ROM -- 0x200000 for the three permitted 6106 cartridges (68.5%, 76.5%, and
/// 77.0% of targets in-bank, against ~0% at the other candidates) and 0 for
/// the permitted 7102 cartridge (88.4%). Running the same procedure on SM64,
/// whose zero delta is already proven above, returns 0 at 86.8%, so the
/// method reproduces a known-good answer before being trusted on new data.
const IPL3_SHA256_CIC_6102_7101: &str =
    "61e88238552c356c23d19409fe5570ee6910419586bc6fc740f638f761adc46e";
const IPL3_SHA256_CIC_6103_7103: &str =
    "bf3620d30817007091ebe9bddd1b88c23b8a0052170b3309cde5b6b4238e45e7";
const IPL3_SHA256_CIC_6105_7105: &str =
    "04b7bc6717a9f0eb724cf927e74ad3876c381cbb280d841736fc5e55580b756b";
const IPL3_SHA256_CIC_6106_7106: &str =
    "36adc40148af56f0d78cd505eb6a90117d1fd6f11c6309e52ed36bc4c6ba340e";
const IPL3_SHA256_CIC_7102: &str =
    "16e062ba8f190c7a712a6bdb34620207299d9be676174cd81d764403df661ad0";

const IPL3_ROM_START: usize = 0x40;
const IPL3_ROM_END: usize = 0x1000;

/// Exact standard IPL3 identity whose boot-copy relocation is known.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RecognizedIpl3 {
    Cic6102Or7101,
    Cic6103Or7103,
    Cic6105Or7105,
    Cic6106Or7106,
    Cic7102,
}

impl RecognizedIpl3 {
    fn load_delta(self) -> u32 {
        match self {
            Self::Cic6102Or7101 | Self::Cic6105Or7105 | Self::Cic7102 => 0,
            Self::Cic6103Or7103 => 0x10_0000,
            Self::Cic6106Or7106 => 0x20_0000,
        }
    }

    fn note(self) -> &'static str {
        match self {
            Self::Cic6102Or7101 => "CIC-6102/7101 IPL3 (loads at header entry)",
            Self::Cic6103Or7103 => "CIC-6103/7103 IPL3 (loads at entry - 0x100000)",
            Self::Cic6105Or7105 => "CIC-6105/7105 IPL3 (loads at header entry)",
            Self::Cic6106Or7106 => "CIC-6106/7106 IPL3 (loads at entry - 0x200000)",
            Self::Cic7102 => "CIC-7102 IPL3 (loads at header entry)",
        }
    }
}

/// Why boot-bank discovery could not establish a mapping.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum BootBankOpenReason {
    TruncatedIpl3 {
        available_bytes: u32,
        required_bytes: u32,
    },
    UnrecognizedIpl3 {
        sha256: String,
    },
    TruncatedBootCopy {
        ipl3: RecognizedIpl3,
        ipl3_sha256: String,
        available_bytes: u32,
        required_bytes: u32,
    },
    InvalidEntrypoint {
        ipl3: RecognizedIpl3,
        ipl3_sha256: String,
        entry_point: u32,
        load_delta: u32,
    },
    InvalidLoadRange {
        ipl3: RecognizedIpl3,
        ipl3_sha256: String,
        va_start: u32,
        byte_length: u32,
    },
}

/// Typed outcome of the IPL3-bound boot-bank proof rule.
#[must_use]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum BootBankDiscovery {
    Proven {
        ipl3: RecognizedIpl3,
        ipl3_sha256: String,
        load_delta: u32,
    },
    Open {
        reason: BootBankOpenReason,
    },
}

fn classify_ipl3_sha256(digest: &str) -> Option<RecognizedIpl3> {
    match digest {
        IPL3_SHA256_CIC_6102_7101 => Some(RecognizedIpl3::Cic6102Or7101),
        IPL3_SHA256_CIC_6103_7103 => Some(RecognizedIpl3::Cic6103Or7103),
        IPL3_SHA256_CIC_6105_7105 => Some(RecognizedIpl3::Cic6105Or7105),
        IPL3_SHA256_CIC_6106_7106 => Some(RecognizedIpl3::Cic6106Or7106),
        IPL3_SHA256_CIC_7102 => Some(RecognizedIpl3::Cic7102),
        _ => None,
    }
}

fn identify_ipl3(rom_bytes: &[u8]) -> Result<(RecognizedIpl3, String), BootBankOpenReason> {
    use sha2::Digest as _;
    if rom_bytes.len() < IPL3_ROM_END {
        return Err(BootBankOpenReason::TruncatedIpl3 {
            available_bytes: rom_bytes.len().saturating_sub(IPL3_ROM_START) as u32,
            required_bytes: (IPL3_ROM_END - IPL3_ROM_START) as u32,
        });
    }
    let mut hasher = sha2::Sha256::new();
    hasher.update(&rom_bytes[IPL3_ROM_START..IPL3_ROM_END]);
    let digest = format!("{:x}", hasher.finalize());
    classify_ipl3_sha256(&digest)
        .map(|identity| (identity, digest.clone()))
        .ok_or(BootBankOpenReason::UnrecognizedIpl3 { sha256: digest })
}

/// Discover the boot-copy bank from the ROM header plus the IPL3 blob.
/// A complete, exactly recognized standard IPL3 produces a `Proven` mapping
/// and entry. A truncated or unknown IPL3 records an explicit `Open` bank
/// conclusion and publishes no guessed mapping or entry.
pub fn discover_boot_bank(rom: &NormalizedRom, db: &mut FactDb) -> BootBankDiscovery {
    let (ipl3, ipl3_sha256) = match identify_ipl3(&rom.bytes) {
        Ok(identified) => identified,
        Err(reason) => {
            return record_boot_open(rom, db, reason);
        }
    };
    publish_boot_bank(rom, db, ipl3, ipl3_sha256)
}

fn publish_boot_bank(
    rom: &NormalizedRom,
    db: &mut FactDb,
    ipl3: RecognizedIpl3,
    ipl3_sha256: String,
) -> BootBankDiscovery {
    let load_delta = ipl3.load_delta();
    let Some(va_start) = rom.header.entry_point.checked_sub(load_delta) else {
        return record_boot_open(
            rom,
            db,
            BootBankOpenReason::InvalidEntrypoint {
                ipl3,
                ipl3_sha256,
                entry_point: rom.header.entry_point,
                load_delta,
            },
        );
    };
    let rom_start = BOOT_COPY_ROM_START;
    let rom_end = rom_start + BOOT_COPY_SIZE;
    if rom.len() < rom_end as usize {
        return record_boot_open(
            rom,
            db,
            BootBankOpenReason::TruncatedBootCopy {
                ipl3,
                ipl3_sha256,
                available_bytes: rom.len().saturating_sub(rom_start as usize) as u32,
                required_bytes: BOOT_COPY_SIZE,
            },
        );
    }
    let Some(va_end) = va_start.checked_add(BOOT_COPY_SIZE) else {
        return record_boot_open(
            rom,
            db,
            BootBankOpenReason::InvalidLoadRange {
                ipl3,
                ipl3_sha256,
                va_start,
                byte_length: BOOT_COPY_SIZE,
            },
        );
    };

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
             {} (SHA-256 {ipl3_sha256})",
            ipl3.note()
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

    BootBankDiscovery::Proven {
        ipl3,
        ipl3_sha256,
        load_delta,
    }
}

fn record_boot_open(
    rom: &NormalizedRom,
    db: &mut FactDb,
    reason: BootBankOpenReason,
) -> BootBankDiscovery {
    let note = match &reason {
        BootBankOpenReason::TruncatedIpl3 {
            available_bytes,
            required_bytes,
        } => format!(
            "boot bank open: IPL3 is truncated ({available_bytes}/{required_bytes} bytes); no load delta inferred"
        ),
        BootBankOpenReason::UnrecognizedIpl3 { sha256 } => format!(
            "boot bank open: IPL3 SHA-256 {sha256} has no admitted relocation behavior; no load delta inferred"
        ),
        BootBankOpenReason::TruncatedBootCopy {
            ipl3,
            ipl3_sha256,
            available_bytes,
            required_bytes,
        } => format!(
            "boot bank open: {} (SHA-256 {ipl3_sha256}) boot-copy source is truncated ({available_bytes}/{required_bytes} bytes); no partial mapping published",
            ipl3.note()
        ),
        BootBankOpenReason::InvalidEntrypoint {
            ipl3,
            ipl3_sha256,
            entry_point,
            load_delta,
        } => format!(
            "boot bank open: header entry 0x{entry_point:08x} cannot apply {} (SHA-256 {ipl3_sha256}) load delta 0x{load_delta:x}; no wrapped mapping published",
            ipl3.note()
        ),
        BootBankOpenReason::InvalidLoadRange {
            ipl3,
            ipl3_sha256,
            va_start,
            byte_length,
        } => format!(
            "boot bank open: {} (SHA-256 {ipl3_sha256}) load range at 0x{va_start:08x} with length 0x{byte_length:x} exceeds the 32-bit address space; no wrapped mapping published",
            ipl3.note()
        ),
    };
    let evidence = db.insert(Fact::Evidence {
        subject: BankAddr::new(BOOT_BANK, rom.header.entry_point),
        note,
    });
    db.conclude(
        format!("bank:{BOOT_BANK}"),
        ProofState::Open,
        vec![evidence],
        "boot_copy_requires_complete_recognized_ipl3",
    )
    .expect("boot bank is the first conclusion for this subject; cannot violate monotonicity");
    BootBankDiscovery::Open { reason }
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
/// Two admitted tables assigning one ROM source to different destinations.
struct DestinationDisagreement {
    source_start: u32,
    source_end: u32,
    first_va: u32,
    second_va: u32,
}

/// The first source interval that two admitted tables place at different VAs,
/// scanned in deterministic order. `None` means every table agrees wherever
/// they overlap, so their records describe one geometry.
///
/// Identical sources must declare identical destinations. Sources that
/// *partially* overlap are a contradiction outright: one ROM byte cannot
/// belong to two differently-based images, and no fragmentation or stride
/// alias produces that shape -- aliases repeat whole records.
fn contradicting_destination(
    admitted: &[&crate::overlay_regions::TableAdmission],
) -> Option<DestinationDisagreement> {
    let mut declared: std::collections::BTreeMap<(u32, u32), u32> =
        std::collections::BTreeMap::new();
    for admission in admitted {
        for record in &admission.table.records {
            match declared.entry((record.rom_start, record.rom_end)) {
                std::collections::btree_map::Entry::Vacant(slot) => {
                    slot.insert(record.vram_dest);
                }
                std::collections::btree_map::Entry::Occupied(slot) => {
                    if *slot.get() != record.vram_dest {
                        return Some(DestinationDisagreement {
                            source_start: record.rom_start,
                            source_end: record.rom_end,
                            first_va: *slot.get(),
                            second_va: record.vram_dest,
                        });
                    }
                }
            }
        }
    }

    let mut intervals: Vec<_> = declared
        .iter()
        .map(|((start, end), va)| (*start, *end, *va))
        .collect();
    intervals.sort_unstable();
    for pair in intervals.windows(2) {
        let (start, end, va) = pair[0];
        let (next_start, next_end, next_va) = pair[1];
        if next_start < end {
            return Some(DestinationDisagreement {
                source_start: start.max(next_start),
                source_end: end.min(next_end),
                first_va: va,
                second_va: next_va,
            });
        }
    }
    None
}

/// One deduplicated overlay record plus the table it was recovered from.
struct MergedOverlayRecord<'a> {
    record: crate::overlay_regions::CandidateRecord,
    delta_outcome: Option<(u32, u32)>,
    table: &'a crate::overlay_regions::CandidateTable,
}

/// Union the admitted tables' records, one entry per distinct source interval,
/// ordered by source so bank indices are deterministic.
///
/// A record resolved by one fragment and left open by another is kept
/// resolved: the outcomes are not in tension once destinations agree, and
/// discarding the resolved one would lose proof that was already earned.
fn merge_admitted_overlay_records<'a>(
    admitted: &[&'a crate::overlay_regions::TableAdmission],
) -> Vec<MergedOverlayRecord<'a>> {
    let mut by_source: std::collections::BTreeMap<(u32, u32), MergedOverlayRecord<'a>> = std::collections::BTreeMap::new();
    for admission in admitted {
        for (record, delta_outcome) in admission.table.records.iter().zip(&admission.region_deltas) {
            let key = (record.rom_start, record.rom_end);
            match by_source.entry(key) {
                std::collections::btree_map::Entry::Vacant(slot) => {
                    slot.insert(MergedOverlayRecord {
                        record: *record,
                        delta_outcome: *delta_outcome,
                        table: &admission.table,
                    });
                }
                std::collections::btree_map::Entry::Occupied(mut slot) => {
                    if slot.get().delta_outcome.is_none() && delta_outcome.is_some() {
                        slot.insert(MergedOverlayRecord {
                            record: *record,
                            delta_outcome: *delta_outcome,
                            table: &admission.table,
                        });
                    }
                }
            }
        }
    }
    by_source.into_values().collect()
}

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

    if admitted.is_empty() {
        db.conclude(
            table_subject,
            ProofState::Open,
            vec![selection],
            "recovered_overlay_table_has_no_unique_admission",
        )
        .expect("first conclusion for recovered overlay table");
        return Vec::new();
    }

    // Several admitted tables are not automatically a contradiction. A
    // descriptor array split across ROM, or read at a stride alias (a 0x40
    // stride sees every other 0x20 record), yields fragments of ONE geometry.
    // The contradiction that matters is a source interval assigned two
    // different destinations, because only then do the tables disagree about
    // where bytes land. Measured across the corpus: Paper Mario's 18 admitted
    // tables cover 232 distinct sources with zero contradicting destinations,
    // as do Mario Party's 8 over 94 -- they fragment, they do not disagree.
    if let Some(conflict) = contradicting_destination(&admitted) {
        let note = db.insert(Fact::Evidence {
            subject: BankAddr::new(table_name, conflict.source_start),
            note: format!(
                "recovered overlay tables disagree: ROM [0x{:x},0x{:x}) is declared at both VA 0x{:08x} and 0x{:08x}",
                conflict.source_start, conflict.source_end, conflict.first_va, conflict.second_va,
            ),
        });
        db.conclude(
            table_subject,
            ProofState::Conflict,
            vec![selection, note],
            "recovered_overlay_tables_disagree_on_destination",
        )
        .expect("first conclusion for recovered overlay table");
        return Vec::new();
    }

    for admission in &admitted {
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
    }

    // One record per distinct source interval, in deterministic source order.
    // Fragments and stride aliases repeat records; the destination-agreement
    // check above already proved every repeat declares the same VA, so the
    // first occurrence is the whole claim. Keeping the resolved delta means a
    // fragment that mapped a record is not lost to one that left it open.
    let merged = merge_admitted_overlay_records(&admitted);

    let mut accepted_banks = Vec::new();
    let mut table_evidence = vec![selection];
    for (index, MergedOverlayRecord { record, delta_outcome, table, .. }) in
        merged.iter().enumerate()
    {
        let (record, delta_outcome) = (record, delta_outcome);
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
    materialize_rom_range_bounded(
        rom,
        db,
        space,
        start,
        end,
        crate::file_table::DEFAULT_MAX_DECODED_VROM_FILE_BYTES,
    )
}

/// Materialize a ROM interval while bounding the complete decoded VROM file
/// that may be allocated to serve a smaller requested slice.
pub fn materialize_rom_range_bounded(
    rom: &NormalizedRom,
    db: &FactDb,
    space: RomAddressSpace,
    start: u32,
    end: u32,
    max_decoded_vrom_file_bytes: usize,
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
    let expected_len = usize::try_from(
        vrom_end
            .checked_sub(vrom_start)
            .ok_or_else(|| "proven VROM file mapping is inverted".to_string())?,
    )
    .map_err(|_| "decoded VROM file length exceeds usize".to_string())?;
    if expected_len > max_decoded_vrom_file_bytes {
        return Err(format!(
            "decoded VROM file length {expected_len} exceeds transient limit {max_decoded_vrom_file_bytes}"
        ));
    }
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
    scan_load_image_tables_bounded(
        rom,
        inputs,
        db,
        crate::file_table::DEFAULT_MAX_DECODED_VROM_FILE_BYTES,
    )
}

/// Scan load-image tables while bounding every complete decoded VROM file
/// used for a virtual table or load image.
pub fn scan_load_image_tables_bounded(
    rom: &NormalizedRom,
    inputs: &[LoadImageTableInput],
    db: &mut FactDb,
    max_decoded_vrom_file_bytes: usize,
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
        let table_bytes = match materialize_rom_range_bounded(
            rom,
            db,
            shape.location.space,
            shape.location.offset,
            table_end,
            max_decoded_vrom_file_bytes,
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
                (_, DestinationSpace::Vram) => materialize_rom_range_bounded(
                    rom,
                    db,
                    shape.source.space,
                    source_start,
                    source_end,
                    max_decoded_vrom_file_bytes,
                )
                .map(|materialized| materialized.backing_evidence),
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
    /// The deterministic loader-input prefix was scanned, but additional
    /// inputs were withheld by [`MAX_STATIC_REQUEST_DMA_INPUTS`].
    pub input_limit_hit: bool,
    /// Boot-image wrapper shapes examined by the candidate-only classifier.
    pub physical_wrapper_candidates_examined: usize,
    /// Shape candidates withheld because CFG/path and inner-callee semantic
    /// authority have not yet been established.
    pub wrapper_semantic_proof_unavailable: usize,
    /// The wrapper candidate scan itself stopped at its work bound.
    pub physical_wrapper_candidate_limit_hit: bool,
    /// Which required dataflow fact each rejected wrapper candidate failed to
    /// establish; the wrapper rule is the dominant geometry frontier, so the
    /// unmet fact is the actionable part of a rejection.
    pub wrapper_shape_rejections: crate::pi_dma::WrapperRejectionCensus,
}

impl StaticRequestDmaReport {
    pub(crate) fn push_open_bounded(&mut self, message: String) {
        if self.open.len() + 1 < MAX_STATIC_REQUEST_DMA_OPEN_ROWS {
            self.open.push(message);
        } else if self.open.len() + 1 == MAX_STATIC_REQUEST_DMA_OPEN_ROWS {
            self.open.push(format!(
                "request-DMA open frontier reached its {}-row reporting bound; additional rows omitted",
                MAX_STATIC_REQUEST_DMA_OPEN_ROWS
            ));
        }
    }
}

/// The largest RDRAM a retail console reaches (Expansion Pak). Used only to
/// bound destination sanity in the slicer; VA truth is judged downstream.
const SCAN_RDRAM_LEN: u32 = 0x0080_0000;
const MAX_STATIC_REQUEST_DMA_BANKS: usize = 4096;
const MAX_STATIC_REQUEST_DMA_INPUTS: usize = 64;
const MAX_STATIC_REQUEST_DMA_OPEN_ROWS: usize = 4096;
const MAX_STATIC_REQUEST_DMA_SCANNED_BYTES: usize = 256 * 1024 * 1024;

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
    scan_static_request_dma_bounded(
        rom,
        inputs,
        db,
        crate::file_table::DEFAULT_MAX_DECODED_VROM_FILE_BYTES,
    )
}

/// [`scan_static_request_dma`] with an explicit complete-file VROM decode
/// cap. A virtual request is not published unless its bytes materialize inside
/// that envelope.
pub fn scan_static_request_dma_bounded(
    rom: &NormalizedRom,
    inputs: &[StaticRequestDmaInput],
    db: &mut FactDb,
    max_decoded_vrom_file_bytes: usize,
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
                report.open.push(format!(
                    "{}: slicer rejected boot image: {error:?}",
                    input.name
                ));
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
                    if let Err(error) = materialize_rom_range_bounded(
                        rom,
                        db,
                        RomAddressSpace::Virtual,
                        device,
                        device_end,
                        max_decoded_vrom_file_bytes,
                    ) {
                        report.open.push(format!(
                            "{}: call at 0x{call_pc:x} VROM range 0x{device:x}..0x{device_end:x} is unavailable within the decode limit: {error}",
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

/// Recover exact whole-file request-DMA loads to a bounded fixed point over
/// every proven, materializable bank.
///
/// Unlike [`scan_static_request_dma_bounded`], this production-auto path does
/// not treat a contained VROM slice as a new load image. Each virtual request
/// must equal exactly one proven file-table record. Newly recovered images are
/// scanned in later rounds, which admits loader calls made by resident code
/// loaded from the boot image without relying on a title-specific call site.
pub fn scan_static_request_dma_fixed_point_bounded(
    rom: &NormalizedRom,
    inputs: &[StaticRequestDmaInput],
    db: &mut FactDb,
    max_decoded_vrom_file_bytes: usize,
) -> StaticRequestDmaReport {
    use crate::loaders::VirtualAddress;
    use std::collections::{BTreeMap, BTreeSet};

    #[derive(Debug, Clone)]
    struct PendingLoad {
        input_index: usize,
        source_bank: String,
        call_pc: u32,
        device: u32,
        device_end: u32,
        va_start: u32,
        va_end: u32,
    }

    type Geometry = (RomAddressSpace, u32, u32, u32, u32);

    let mut report = StaticRequestDmaReport::default();
    if inputs.is_empty() {
        return report;
    }
    let inputs = if inputs.len() > MAX_STATIC_REQUEST_DMA_INPUTS {
        report.input_limit_hit = true;
        report.push_open_bounded(format!(
            "request-DMA fixed point has {} loader inputs; scanning the deterministic first {MAX_STATIC_REQUEST_DMA_INPUTS} and withholding the remainder",
            inputs.len()
        ));
        &inputs[..MAX_STATIC_REQUEST_DMA_INPUTS]
    } else {
        inputs
    };
    let mut exact_vrom_files: BTreeMap<(u32, u32), usize> = BTreeMap::new();
    for (_, fact) in db.proven_vrom_file_mappings() {
        if let Fact::LoadImageTableRecord {
            source_start,
            source_end,
            ..
        } = fact
        {
            *exact_vrom_files
                .entry((*source_start, *source_end))
                .or_default() += 1;
        }
    }

    let mut known_geometries: BTreeSet<Geometry> = db
        .proven_rom_mappings()
        .into_iter()
        .filter_map(|fact| match fact {
            Fact::RomMapping {
                rom_space,
                rom_start,
                rom_end,
                va_start,
                va_end,
                ..
            } => Some((*rom_space, *rom_start, *rom_end, *va_start, *va_end)),
            _ => None,
        })
        .collect();
    let mut scanned_banks: BTreeSet<(String, Geometry)> = BTreeSet::new();
    let mut scanned_bytes = 0usize;
    let mut next_bank_indices = vec![0u32; inputs.len()];

    loop {
        let mut sources: Vec<_> = db
            .proven_rom_mappings()
            .into_iter()
            .filter_map(|fact| match fact {
                Fact::RomMapping {
                    bank,
                    rom_space,
                    rom_start,
                    rom_end,
                    va_start,
                    va_end,
                } => Some((
                    bank.clone(),
                    (*rom_space, *rom_start, *rom_end, *va_start, *va_end),
                )),
                _ => None,
            })
            .filter(|source| !scanned_banks.contains(source))
            .collect();
        sources.sort();
        if sources.is_empty() {
            break;
        }
        if scanned_banks.len().saturating_add(sources.len()) > MAX_STATIC_REQUEST_DMA_BANKS {
            report.push_open_bounded(
                format!(
                    "request-DMA fixed point exceeds its {MAX_STATIC_REQUEST_DMA_BANKS}-bank scan bound"
                ),
            );
            break;
        }

        let mut pending: BTreeMap<Geometry, PendingLoad> = BTreeMap::new();
        for (source_bank, geometry) in sources {
            scanned_banks.insert((source_bank.clone(), geometry));
            let (source_space, source_rom_start, source_rom_end, source_va_start, _) = geometry;
            let materialized = match materialize_rom_range_bounded(
                rom,
                db,
                source_space,
                source_rom_start,
                source_rom_end,
                max_decoded_vrom_file_bytes,
            ) {
                Ok(materialized) => materialized,
                Err(error) => {
                    report.push_open_bounded(
                        format!(
                            "{source_bank}: proven request-DMA scan source is not materializable: {error}"
                        ),
                    );
                    continue;
                }
            };
            scanned_bytes = match scanned_bytes.checked_add(materialized.bytes.len()) {
                Some(total) if total <= MAX_STATIC_REQUEST_DMA_SCANNED_BYTES => total,
                _ => {
                    report.push_open_bounded(
                        format!(
                            "request-DMA fixed point exceeds its {MAX_STATIC_REQUEST_DMA_SCANNED_BYTES}-byte aggregate scan bound"
                        ),
                    );
                    return report;
                }
            };
            let words: Vec<u32> = materialized
                .bytes
                .chunks_exact(4)
                .map(|chunk| u32::from_be_bytes(chunk.try_into().unwrap()))
                .collect();

            for (input_index, input) in inputs.iter().enumerate() {
                let slices = match crate::pi_dma::slice_load_request_calls(
                    &words,
                    VirtualAddress::new(source_va_start),
                    VirtualAddress::new(input.callee_va),
                    SCAN_RDRAM_LEN,
                    input.dram_arg_register,
                    input.device_arg_register,
                    input.size_arg_register,
                ) {
                    Ok(slices) => slices,
                    Err(error) => {
                        report.push_open_bounded(format!(
                            "{}: slicer rejected source bank {source_bank}: {error:?}",
                            input.name
                        ));
                        continue;
                    }
                };
                for slice in slices {
                    let call_pc = slice.call_pc.get();
                    let (Some(candidate), Some(dram_pointer)) =
                        (slice.candidate(), slice.dram_pointer.proven().copied())
                    else {
                        report.push_open_bounded(format!(
                            "{}: call at {source_bank}:0x{call_pc:x} has open operands",
                            input.name
                        ));
                        continue;
                    };
                    let device = candidate.device_address.get();
                    let length = if input.size_is_end_address {
                        match candidate.byte_count.get().checked_sub(device) {
                            Some(length) if length > 0 => length,
                            _ => {
                                report.push_open_bounded(
                                    format!(
                                        "{}: call at {source_bank}:0x{call_pc:x} has an invalid end-address operand",
                                        input.name
                                    ),
                                );
                                continue;
                            }
                        }
                    } else {
                        candidate.byte_count.get()
                    };
                    let va_start = dram_pointer.get();
                    let (Some(device_end), Some(va_end)) =
                        (device.checked_add(length), va_start.checked_add(length))
                    else {
                        report.push_open_bounded(format!(
                            "{}: call at {source_bank}:0x{call_pc:x} has an overflowing range",
                            input.name
                        ));
                        continue;
                    };
                    let target_geometry =
                        (input.device_space, device, device_end, va_start, va_end);
                    if known_geometries.contains(&target_geometry)
                        || pending.contains_key(&target_geometry)
                    {
                        continue;
                    }

                    match input.device_space {
                        RomAddressSpace::Physical => {
                            if device_end as usize > rom.len() {
                                report.push_open_bounded(
                                    format!(
                                        "{}: call at {source_bank}:0x{call_pc:x} physical range 0x{device:x}..0x{device_end:x} exceeds the ROM",
                                        input.name
                                    ),
                                );
                                continue;
                            }
                        }
                        RomAddressSpace::Virtual => {
                            let exact_records = exact_vrom_files
                                .get(&(device, device_end))
                                .copied()
                                .unwrap_or(0);
                            if exact_records != 1 {
                                report.push_open_bounded(
                                    format!(
                                        "{}: call at {source_bank}:0x{call_pc:x} VROM range 0x{device:x}..0x{device_end:x} has {exact_records} exact proven file records; expected exactly one",
                                        input.name
                                    ),
                                );
                                continue;
                            }
                            if let Err(error) = materialize_rom_range_bounded(
                                rom,
                                db,
                                RomAddressSpace::Virtual,
                                device,
                                device_end,
                                max_decoded_vrom_file_bytes,
                            ) {
                                report.push_open_bounded(
                                    format!(
                                        "{}: call at {source_bank}:0x{call_pc:x} exact VROM file 0x{device:x}..0x{device_end:x} is unavailable within the decode limit: {error}",
                                        input.name
                                    ),
                                );
                                continue;
                            }
                        }
                    }
                    pending.insert(
                        target_geometry,
                        PendingLoad {
                            input_index,
                            source_bank: source_bank.clone(),
                            call_pc,
                            device,
                            device_end,
                            va_start,
                            va_end,
                        },
                    );
                }
            }
        }

        if pending.is_empty() {
            continue;
        }
        for (geometry, load) in pending {
            if known_geometries.len() >= MAX_STATIC_REQUEST_DMA_BANKS {
                report.push_open_bounded(
                    format!(
                        "request-DMA fixed point reached its {MAX_STATIC_REQUEST_DMA_BANKS}-mapping bound"
                    ),
                );
                return report;
            }
            let input = &inputs[load.input_index];
            let bank = loop {
                let index = next_bank_indices[load.input_index];
                let Some(next) = index.checked_add(1) else {
                    report.push_open_bounded(format!(
                        "{}: request-DMA bank-name index overflow",
                        input.name
                    ));
                    return report;
                };
                next_bank_indices[load.input_index] = next;
                let candidate = input.bank_name.name(index);
                if !db.facts().iter().any(|fact| {
                        matches!(fact, Fact::RomMapping { bank, .. } if bank.as_str() == candidate)
                    }) {
                        break candidate;
                    }
            };
            let mapping = db.insert(Fact::RomMapping {
                bank: bank.clone(),
                rom_space: input.device_space,
                rom_start: load.device,
                rom_end: load.device_end,
                va_start: load.va_start,
                va_end: load.va_end,
            });
            let evidence = db.insert(Fact::Evidence {
                subject: BankAddr::new(&load.source_bank, load.call_pc),
                note: format!(
                    "exact whole-file request-DMA operands at {}:0x{:x} to {} (0x{:x}): device 0x{:x}+0x{:x} -> VA 0x{:x}; instruction bytes do not prove reachability or completion",
                    load.source_bank,
                    load.call_pc,
                    input.name,
                    input.callee_va,
                    load.device,
                    load.device_end - load.device,
                    load.va_start
                ),
            });
            db.conclude(
                format!("bank:{bank}"),
                ProofState::Proven,
                vec![mapping, evidence],
                "static_request_dma_whole_file_fixed_point",
            )
            .expect("fixed-point request-DMA bank names are freshly generated");
            known_geometries.insert(geometry);
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
    fn recognized_entry_loading_ipl3_publishes_header_mapping() {
        let rom = make_test_rom(0x8000_0400, BOOT_COPY_SIZE as usize + 0x1000);
        let mut db = FactDb::new();
        let outcome = publish_boot_bank(
            &rom,
            &mut db,
            RecognizedIpl3::Cic6102Or7101,
            IPL3_SHA256_CIC_6102_7101.to_string(),
        );
        assert!(matches!(
            outcome,
            BootBankDiscovery::Proven { load_delta: 0, .. }
        ));

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
    fn recognized_ipl3_with_truncated_boot_copy_stays_open() {
        let rom = make_test_rom(0x8000_0400, 0x2000);
        let mut db = FactDb::new();
        let outcome = publish_boot_bank(
            &rom,
            &mut db,
            RecognizedIpl3::Cic6105Or7105,
            IPL3_SHA256_CIC_6105_7105.to_string(),
        );
        assert!(matches!(
            outcome,
            BootBankDiscovery::Open {
                reason: BootBankOpenReason::TruncatedBootCopy { .. }
            }
        ));
        assert_eq!(db.conclusion("bank:boot").unwrap().state, ProofState::Open);
        assert!(db.proven_rom_mappings().is_empty());
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
    fn multiple_admissions_over_disjoint_sources_merge_into_one_geometry() {
        // Several admitted tables are fragments or stride aliases of one
        // descriptor array unless they actually disagree. These two claim
        // disjoint ROM sources at disjoint destinations, so both map.
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

        assert_eq!(banks.len(), 2, "both non-contradicting records must map");
        assert_eq!(db.proven_rom_mappings().len(), 2);
    }

    #[test]
    fn admissions_disagreeing_on_a_destination_still_map_nothing() {
        // The contradiction that matters: one source interval declared at two
        // different VAs. Nothing may be admitted from either table.
        let rom = make_test_rom(0x8000_0400, 0x5000);
        let recovery = recovery_with(vec![
            recovered_table(0x1800, 0x2000, 0x8010_0000, 0x8010_0000),
            recovered_table(0x1900, 0x2000, 0x8020_0000, 0x8020_0000),
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
    fn partially_overlapping_sources_are_a_conflict_not_a_merge() {
        // One ROM byte cannot belong to two differently-based images, and no
        // stride alias produces a partial overlap -- aliases repeat whole
        // records.
        let rom = make_test_rom(0x8000_0400, 0x5000);
        let recovery = recovery_with(vec![
            recovered_table(0x1800, 0x2000, 0x8010_0000, 0x8010_0000),
            recovered_table(0x1900, 0x2800, 0x8020_0000, 0x8020_0000),
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
    }

    #[test]
    fn only_exact_standard_ipl3_hashes_have_relocation_behavior() {
        assert_eq!(
            classify_ipl3_sha256(IPL3_SHA256_CIC_6102_7101),
            Some(RecognizedIpl3::Cic6102Or7101)
        );
        assert_eq!(
            classify_ipl3_sha256(IPL3_SHA256_CIC_6103_7103),
            Some(RecognizedIpl3::Cic6103Or7103)
        );
        assert_eq!(
            classify_ipl3_sha256(IPL3_SHA256_CIC_6105_7105),
            Some(RecognizedIpl3::Cic6105Or7105)
        );
        assert_eq!(
            classify_ipl3_sha256(IPL3_SHA256_CIC_6106_7106),
            Some(RecognizedIpl3::Cic6106Or7106)
        );
        assert_eq!(
            classify_ipl3_sha256(IPL3_SHA256_CIC_7102),
            Some(RecognizedIpl3::Cic7102)
        );
        assert_eq!(RecognizedIpl3::Cic6102Or7101.load_delta(), 0);
        assert_eq!(RecognizedIpl3::Cic6103Or7103.load_delta(), 0x10_0000);
        assert_eq!(RecognizedIpl3::Cic6105Or7105.load_delta(), 0);
        assert_eq!(RecognizedIpl3::Cic6106Or7106.load_delta(), 0x20_0000);
        assert_eq!(RecognizedIpl3::Cic7102.load_delta(), 0);
        assert_eq!(classify_ipl3_sha256(&"00".repeat(32)), None);
    }

    #[test]
    fn unknown_complete_ipl3_records_open_without_mapping_or_entry() {
        let rom = make_test_rom(0x8000_0400, BOOT_COPY_SIZE as usize + 0x1000);
        let mut db = FactDb::new();
        let outcome = discover_boot_bank(&rom, &mut db);

        assert!(matches!(
            outcome,
            BootBankDiscovery::Open {
                reason: BootBankOpenReason::UnrecognizedIpl3 { .. }
            }
        ));
        assert_eq!(db.conclusion("bank:boot").unwrap().state, ProofState::Open);
        assert!(db.proven_rom_mappings().is_empty());
        assert!(db.proven_function_entries(BOOT_BANK).is_empty());
    }

    #[test]
    fn truncated_ipl3_records_typed_open_frontier() {
        let mut bytes = vec![0u8; 0x100];
        bytes[0..4].copy_from_slice(&0x8037_1240u32.to_be_bytes());
        bytes[8..12].copy_from_slice(&0x8000_0400u32.to_be_bytes());
        let rom = normalize(&bytes).expect("header-sized synthetic z64");
        let mut db = FactDb::new();
        let outcome = discover_boot_bank(&rom, &mut db);

        assert_eq!(
            outcome,
            BootBankDiscovery::Open {
                reason: BootBankOpenReason::TruncatedIpl3 {
                    available_bytes: 0xc0,
                    required_bytes: 0xfc0,
                }
            }
        );
        assert_eq!(db.conclusion("bank:boot").unwrap().state, ProofState::Open);
        assert!(db.proven_rom_mappings().is_empty());
    }

    #[test]
    fn relocating_ipl3_rejects_entrypoint_subtraction_underflow() {
        let rom = make_test_rom(0x0000_0400, BOOT_COPY_SIZE as usize + 0x1000);
        let mut db = FactDb::new();
        let outcome = publish_boot_bank(
            &rom,
            &mut db,
            RecognizedIpl3::Cic6103Or7103,
            IPL3_SHA256_CIC_6103_7103.to_string(),
        );

        assert!(matches!(
            outcome,
            BootBankDiscovery::Open {
                reason: BootBankOpenReason::InvalidEntrypoint {
                    entry_point: 0x0000_0400,
                    load_delta: 0x10_0000,
                    ..
                }
            }
        ));
        assert!(db.proven_rom_mappings().is_empty());
    }

    #[test]
    fn entry_loading_ipl3_rejects_address_range_overflow() {
        let rom = make_test_rom(0xfff0_0400, BOOT_COPY_SIZE as usize + 0x1000);
        let mut db = FactDb::new();
        let outcome = publish_boot_bank(
            &rom,
            &mut db,
            RecognizedIpl3::Cic6102Or7101,
            IPL3_SHA256_CIC_6102_7101.to_string(),
        );

        assert!(matches!(
            outcome,
            BootBankDiscovery::Open {
                reason: BootBankOpenReason::InvalidLoadRange {
                    va_start: 0xfff0_0400,
                    byte_length: BOOT_COPY_SIZE,
                    ..
                }
            }
        ));
        assert!(db.proven_rom_mappings().is_empty());
    }
}

/// One mechanically recovered DMA-request routine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecoveredRequestDmaCallee {
    pub callee_va: u32,
    pub corroborated_sites: usize,
    pub resolved_sites: usize,
}

/// What mechanical request-DMA callee recovery admitted and left open.
#[derive(Debug, Default)]
pub struct RequestDmaCalleeRecovery {
    pub admitted: Vec<RecoveredRequestDmaCallee>,
    pub open: Vec<String>,
}

/// Recover, from ROM bytes alone, the routine a game uses to DMA a resident
/// image into RDRAM.
///
/// A resident code image is loaded by an explicit `RequestSync(ram, vrom,
/// size)`-shaped call rather than named by any descriptor table, so its VRAM
/// destination is invisible to table recovery. Rather than cite that routine's
/// address, admit it on machine-checkable evidence: a candidate IS the
/// DMA-request routine when the constant `(vrom, size)` operands recovered at
/// its direct call sites land exactly on file-table records already proven
/// from this ROM.
///
/// The rule is deliberately unforgiving. A candidate with ANY resolved call
/// site whose operands name no proven record is rejected outright: a real
/// loader's arguments describe real files, so one contradiction means the
/// shape matched something else.
pub fn recover_request_dma_callees(
    rom: &NormalizedRom,
    db: &FactDb,
    min_corroborated_sites: usize,
) -> RequestDmaCalleeRecovery {
    use crate::loaders::VirtualAddress;
    use std::collections::{BTreeMap, BTreeSet};

    let mut recovery = RequestDmaCalleeRecovery::default();
    let Some((boot_rom_start, boot_rom_end, boot_va_start)) =
        db.proven_rom_mappings().iter().find_map(|fact| match fact {
            Fact::RomMapping {
                bank,
                rom_start,
                rom_end,
                va_start,
                ..
            } if bank == BOOT_BANK => Some((*rom_start, *rom_end, *va_start)),
            _ => None,
        })
    else {
        recovery
            .open
            .push("boot bank not proven; request-dma callee recovery skipped".to_string());
        return recovery;
    };
    let Some(image) = rom
        .bytes
        .get(boot_rom_start as usize..boot_rom_end as usize)
    else {
        recovery
            .open
            .push("boot bank ROM interval is outside the normalized image".to_string());
        return recovery;
    };
    let words: Vec<u32> = image
        .chunks_exact(4)
        .map(|chunk| u32::from_be_bytes(chunk.try_into().unwrap()))
        .collect();

    // The corroboration set: every (vrom_start, length) a proven file-table
    // record already describes. Recovered operands must hit one exactly.
    let mut records: BTreeSet<(u32, u32)> = BTreeSet::new();
    for (_, fact) in db.proven_vrom_file_mappings() {
        if let Fact::LoadImageTableRecord {
            source_start,
            source_end,
            ..
        } = fact
        {
            if let Some(len) = source_end.checked_sub(*source_start) {
                records.insert((*source_start, len));
            }
        }
    }
    if records.is_empty() {
        recovery
            .open
            .push("no proven file-table records to corroborate against".to_string());
        return recovery;
    }

    // Count direct call sites per target so the scan can skip targets with
    // fewer sites than the rule requires. This is only a bound on work: the
    // admission evidence is the exact record match below, never the count.
    let mut sites_per_target: BTreeMap<u32, usize> = BTreeMap::new();
    for (index, word) in words.iter().enumerate() {
        if word >> 26 != 0x03 {
            continue;
        }
        let pc = boot_va_start.wrapping_add((index as u32) * 4);
        let target = (pc & 0xF000_0000) | ((word & 0x03FF_FFFF) << 2);
        *sites_per_target.entry(target).or_default() += 1;
    }

    for (callee_va, site_count) in sites_per_target {
        if site_count < min_corroborated_sites {
            continue;
        }
        // o32 `RequestSync(ram, vrom, size)`: $a0/$a1/$a2.
        let Ok(slices) = crate::pi_dma::slice_load_request_calls(
            &words,
            VirtualAddress::new(boot_va_start),
            VirtualAddress::new(callee_va),
            SCAN_RDRAM_LEN,
            4,
            5,
            6,
        ) else {
            continue;
        };
        let mut corroborated = 0usize;
        let mut resolved = 0usize;
        let mut contradicted = false;
        for slice in &slices {
            let (Some(device), Some(bytes)) =
                (slice.device_address.proven(), slice.byte_count.proven())
            else {
                continue;
            };
            resolved += 1;
            if records.contains(&(device.get(), bytes.get())) {
                corroborated += 1;
            } else {
                contradicted = true;
                break;
            }
        }
        if contradicted || corroborated < min_corroborated_sites {
            continue;
        }
        recovery.admitted.push(RecoveredRequestDmaCallee {
            callee_va,
            corroborated_sites: corroborated,
            resolved_sites: resolved,
        });
    }
    if recovery.admitted.is_empty() {
        recovery.open.push(
            "no callee's call-site operands corroborated against proven file records".to_string(),
        );
    }
    recovery
}

#[cfg(test)]
mod request_dma_recovery_tests {
    use super::*;
    use crate::rom::normalize;

    /// Place `words` at the start of the IPL3 boot image of a synthetic z64.
    fn rom_with_boot_words(entry: u32, words: &[u32]) -> NormalizedRom {
        let mut buf = vec![0u8; BOOT_COPY_ROM_START as usize + BOOT_COPY_SIZE as usize];
        buf[0..4].copy_from_slice(&0x8037_1240u32.to_be_bytes());
        buf[8..12].copy_from_slice(&entry.to_be_bytes());
        buf[0x20..0x24].copy_from_slice(b"TEST");
        buf[0x3b..0x3f].copy_from_slice(b"CTSE");
        for (index, word) in words.iter().enumerate() {
            let offset = BOOT_COPY_ROM_START as usize + index * 4;
            buf[offset..offset + 4].copy_from_slice(&word.to_be_bytes());
        }
        normalize(&buf).expect("valid synthetic z64")
    }

    /// Boot image that materializes `(ram, vrom, size)` in $a0/$a1/$a2 and
    /// calls `loader`, mirroring an o32 `RequestSync(ram, vrom, size)` site.
    fn boot_calling_loader(vrom: u32, size: u32, loader: u32) -> Vec<u32> {
        vec![
            0x2405_0000 | (vrom & 0xFFFF),               // addiu $a1, $zero, vrom
            0x2406_0000 | (size & 0xFFFF),               // addiu $a2, $zero, size
            0x3C04_8000,                                 // lui   $a0, 0x8000
            0x0C00_0000 | ((loader & 0x0FFF_FFFF) >> 2), // jal   loader
            0x0000_0000,                                 // nop
        ]
    }

    fn request_call(vrom: u32, size: u32, destination: u32, loader: u32) -> Vec<u32> {
        vec![
            0x3c05_0000 | (vrom >> 16),
            0x34a5_0000 | (vrom & 0xffff),
            0x3c06_0000 | (size >> 16),
            0x34c6_0000 | (size & 0xffff),
            0x3c04_0000 | (destination >> 16),
            0x3484_0000 | (destination & 0xffff),
            0x0c00_0000 | ((loader & 0x0fff_ffff) >> 2),
            0,
        ]
    }

    fn fixed_point_rom(boot_words: &[u32], first_file_words: &[u32]) -> NormalizedRom {
        const FIRST_PHYSICAL: usize = 0x102000;
        const SECOND_PHYSICAL: usize = 0x103000;
        let mut buf = vec![0u8; SECOND_PHYSICAL + 0x40];
        buf[0..4].copy_from_slice(&0x8037_1240u32.to_be_bytes());
        buf[8..12].copy_from_slice(&0x8000_0400u32.to_be_bytes());
        buf[0x20..0x24].copy_from_slice(b"TEST");
        buf[0x3b..0x3f].copy_from_slice(b"CTSE");
        for (index, word) in boot_words.iter().enumerate() {
            let offset = BOOT_COPY_ROM_START as usize + index * 4;
            buf[offset..offset + 4].copy_from_slice(&word.to_be_bytes());
        }
        for (index, word) in first_file_words.iter().enumerate() {
            let offset = FIRST_PHYSICAL + index * 4;
            buf[offset..offset + 4].copy_from_slice(&word.to_be_bytes());
        }
        normalize(&buf).expect("valid fixed-point synthetic z64")
    }

    fn add_file_record(
        db: &mut FactDb,
        table: &str,
        index: u32,
        vrom: u32,
        size: u32,
        physical: u32,
    ) {
        let fact = db.insert(Fact::LoadImageTableRecord {
            table: table.to_string(),
            bank: None,
            table_space: RomAddressSpace::Physical,
            table_offset: 0,
            index,
            source_space: crate::facts::MappingAddressSpace::VirtualRom,
            source_start: vrom,
            source_end: vrom + size,
            destination_space: crate::facts::MappingAddressSpace::PhysicalRom,
            destination_start: physical,
            destination_end: physical + size,
        });
        db.conclude(
            load_image_table_record_subject(table, index),
            ProofState::Proven,
            vec![fact],
            "fixed-point test fixture",
        )
        .expect("fresh file record");
    }

    fn fixed_point_input() -> StaticRequestDmaInput {
        StaticRequestDmaInput {
            name: "request_sync".to_string(),
            callee_va: FIXTURE_LOADER,
            dram_arg_register: 4,
            device_arg_register: 5,
            size_arg_register: 6,
            size_is_end_address: false,
            device_space: RomAddressSpace::Virtual,
            bank_name: BankNamePattern::new("request_dma_", 0, ""),
        }
    }

    #[test]
    fn physical_end_address_contract_publishes_the_exact_range() {
        const PHYSICAL_START: u32 = 0x20;
        const PHYSICAL_END: u32 = 0x60;
        const DESTINATION: u32 = 0x8010_0000;
        let rom = rom_with_boot_words(
            0x8000_0400,
            &request_call(PHYSICAL_START, PHYSICAL_END, DESTINATION, FIXTURE_LOADER),
        );
        let mut db = FactDb::new();
        let _ = publish_boot_bank(
            &rom,
            &mut db,
            RecognizedIpl3::Cic6102Or7101,
            IPL3_SHA256_CIC_6102_7101.to_string(),
        );
        let input = StaticRequestDmaInput {
            name: "physical_end".to_string(),
            callee_va: FIXTURE_LOADER,
            dram_arg_register: 4,
            device_arg_register: 5,
            size_arg_register: 6,
            size_is_end_address: true,
            device_space: RomAddressSpace::Physical,
            bank_name: BankNamePattern::new("request_dma_", 0, ""),
        };

        let report = scan_static_request_dma_fixed_point_bounded(&rom, &[input], &mut db, 1024);

        assert_eq!(report.proven_banks, ["request_dma_0"]);
        assert!(db.proven_rom_mappings().iter().any(|fact| matches!(
            fact,
            Fact::RomMapping {
                bank,
                rom_space: RomAddressSpace::Physical,
                rom_start: PHYSICAL_START,
                rom_end: PHYSICAL_END,
                va_start: DESTINATION,
                va_end: 0x8010_0040,
            } if bank == "request_dma_0"
        )));
    }

    fn db_with_proven_record(rom: &NormalizedRom, vrom: u32, size: u32) -> FactDb {
        let mut db = FactDb::new();
        let _outcome = publish_boot_bank(
            rom,
            &mut db,
            RecognizedIpl3::Cic6102Or7101,
            IPL3_SHA256_CIC_6102_7101.to_string(),
        );
        let fact = db.insert(Fact::LoadImageTableRecord {
            table: "t".to_string(),
            bank: None,
            table_space: RomAddressSpace::Physical,
            table_offset: 0,
            index: 0,
            source_space: crate::facts::MappingAddressSpace::VirtualRom,
            source_start: vrom,
            source_end: vrom + size,
            destination_space: crate::facts::MappingAddressSpace::PhysicalRom,
            destination_start: 0,
            destination_end: size,
        });
        db.conclude(
            load_image_table_record_subject("t", 0),
            ProofState::Proven,
            vec![fact],
            "test fixture",
        )
        .expect("fresh conclusion");
        db
    }

    const FIXTURE_VROM: u32 = 0x1060;
    const FIXTURE_SIZE: u32 = 0x63d0;
    const FIXTURE_LOADER: u32 = 0x8000_0500;

    #[test]
    fn whole_file_request_dma_reaches_a_two_hop_fixed_point() {
        const FIRST_VROM: u32 = 0x0020_0000;
        const SECOND_VROM: u32 = 0x0021_0000;
        const FILE_SIZE: u32 = 0x40;
        const FIRST_VA: u32 = 0x8010_0000;
        const SECOND_VA: u32 = 0x8020_0000;
        let rom = fixed_point_rom(
            &request_call(FIRST_VROM, FILE_SIZE, FIRST_VA, FIXTURE_LOADER),
            &request_call(SECOND_VROM, FILE_SIZE, SECOND_VA, FIXTURE_LOADER),
        );
        let mut db = FactDb::new();
        let _ = publish_boot_bank(
            &rom,
            &mut db,
            RecognizedIpl3::Cic6102Or7101,
            IPL3_SHA256_CIC_6102_7101.to_string(),
        );
        add_file_record(&mut db, "files", 0, FIRST_VROM, FILE_SIZE, 0x102000);
        add_file_record(&mut db, "files", 1, SECOND_VROM, FILE_SIZE, 0x103000);

        let report = scan_static_request_dma_fixed_point_bounded(
            &rom,
            &[fixed_point_input()],
            &mut db,
            1024,
        );

        assert_eq!(report.proven_banks, ["request_dma_0", "request_dma_1"]);
        assert!(db.facts().iter().any(|fact| matches!(
            fact,
            Fact::RomMapping {
                bank,
                rom_start: SECOND_VROM,
                rom_end: 0x0021_0040,
                va_start: SECOND_VA,
                va_end: 0x8020_0040,
                ..
            } if bank == "request_dma_1"
        )));
        assert!(db.facts().iter().any(|fact| matches!(
            fact,
            Fact::Evidence { subject, note }
                if subject.bank == "request_dma_0"
                    && subject.pc == FIRST_VA + 24
                    && note.contains("request_dma_0:0x80100018")
        )));
    }

    #[test]
    fn fixed_point_scans_a_deterministic_prefix_when_loader_input_limit_is_hit() {
        const VROM: u32 = 0x0020_0000;
        const FILE_SIZE: u32 = 0x40;
        const VA: u32 = 0x8010_0000;
        let rom = fixed_point_rom(&request_call(VROM, FILE_SIZE, VA, FIXTURE_LOADER), &[]);
        let mut db = FactDb::new();
        let _ = publish_boot_bank(
            &rom,
            &mut db,
            RecognizedIpl3::Cic6102Or7101,
            IPL3_SHA256_CIC_6102_7101.to_string(),
        );
        add_file_record(&mut db, "files", 0, VROM, FILE_SIZE, 0x102000);
        let inputs = vec![fixed_point_input(); MAX_STATIC_REQUEST_DMA_INPUTS + 1];

        let report = scan_static_request_dma_fixed_point_bounded(&rom, &inputs, &mut db, 1024);

        assert!(report.input_limit_hit);
        assert_eq!(report.proven_banks, ["request_dma_0"]);
        assert!(report
            .open
            .iter()
            .any(|row| row.contains("scanning the deterministic first 64")));
    }

    #[test]
    fn fixed_point_rejects_contained_and_ambiguous_vrom_requests() {
        const VROM: u32 = 0x0020_0000;
        const FILE_SIZE: u32 = 0x40;
        let contained_rom = fixed_point_rom(
            &request_call(VROM + 4, FILE_SIZE - 4, 0x8010_0000, FIXTURE_LOADER),
            &[],
        );
        let mut contained_db = FactDb::new();
        let _ = publish_boot_bank(
            &contained_rom,
            &mut contained_db,
            RecognizedIpl3::Cic6102Or7101,
            IPL3_SHA256_CIC_6102_7101.to_string(),
        );
        add_file_record(&mut contained_db, "files", 0, VROM, FILE_SIZE, 0x102000);
        let contained = scan_static_request_dma_fixed_point_bounded(
            &contained_rom,
            &[fixed_point_input()],
            &mut contained_db,
            1024,
        );
        assert!(contained.proven_banks.is_empty());
        assert!(contained
            .open
            .iter()
            .any(|row| row.contains("has 0 exact proven file records")));

        let ambiguous_rom = fixed_point_rom(
            &request_call(VROM, FILE_SIZE, 0x8010_0000, FIXTURE_LOADER),
            &[],
        );
        let mut ambiguous_db = FactDb::new();
        let _ = publish_boot_bank(
            &ambiguous_rom,
            &mut ambiguous_db,
            RecognizedIpl3::Cic6102Or7101,
            IPL3_SHA256_CIC_6102_7101.to_string(),
        );
        add_file_record(&mut ambiguous_db, "files_a", 0, VROM, FILE_SIZE, 0x102000);
        add_file_record(&mut ambiguous_db, "files_b", 0, VROM, FILE_SIZE, 0x103000);
        let ambiguous = scan_static_request_dma_fixed_point_bounded(
            &ambiguous_rom,
            &[fixed_point_input()],
            &mut ambiguous_db,
            1024,
        );
        assert!(ambiguous.proven_banks.is_empty());
        assert!(ambiguous
            .open
            .iter()
            .any(|row| row.contains("has 2 exact proven file records")));
    }

    #[test]
    fn fixed_point_deduplicates_repeated_exact_requests() {
        const VROM: u32 = 0x0020_0000;
        const FILE_SIZE: u32 = 0x40;
        const VA: u32 = 0x8010_0000;
        let call = request_call(VROM, FILE_SIZE, VA, FIXTURE_LOADER);
        let boot_words: Vec<_> = call.iter().chain(&call).copied().collect();
        let rom = fixed_point_rom(&boot_words, &[]);
        let mut db = FactDb::new();
        let _ = publish_boot_bank(
            &rom,
            &mut db,
            RecognizedIpl3::Cic6102Or7101,
            IPL3_SHA256_CIC_6102_7101.to_string(),
        );
        add_file_record(&mut db, "files", 0, VROM, FILE_SIZE, 0x102000);

        let report = scan_static_request_dma_fixed_point_bounded(
            &rom,
            &[fixed_point_input()],
            &mut db,
            1024,
        );

        assert_eq!(report.proven_banks, ["request_dma_0"]);
        assert_eq!(
            db.proven_rom_mappings()
                .into_iter()
                .filter(|fact| matches!(
                    fact,
                    Fact::RomMapping {
                        rom_start: VROM,
                        rom_end: 0x0020_0040,
                        ..
                    }
                ))
                .count(),
            1
        );
    }

    #[test]
    fn request_dma_callee_is_recovered_when_operands_hit_a_proven_record() {
        let rom = rom_with_boot_words(
            0x8000_0400,
            &boot_calling_loader(FIXTURE_VROM, FIXTURE_SIZE, FIXTURE_LOADER),
        );
        let db = db_with_proven_record(&rom, FIXTURE_VROM, FIXTURE_SIZE);

        let recovery = recover_request_dma_callees(&rom, &db, 1);
        assert_eq!(
            recovery.admitted.len(),
            1,
            "exactly the loader should be admitted, got {:?}",
            recovery.admitted
        );
        assert_eq!(recovery.admitted[0].callee_va, FIXTURE_LOADER);
        assert_eq!(recovery.admitted[0].corroborated_sites, 1);
    }

    #[test]
    fn request_dma_callee_is_rejected_when_operands_name_no_proven_record() {
        // Identical call site, but the proven record describes a different
        // length. One contradicting site must reject the candidate outright --
        // this is what stops an arbitrary three-argument call from being
        // mistaken for a loader.
        let rom = rom_with_boot_words(
            0x8000_0400,
            &boot_calling_loader(FIXTURE_VROM, FIXTURE_SIZE, FIXTURE_LOADER),
        );
        let db = db_with_proven_record(&rom, FIXTURE_VROM, FIXTURE_SIZE + 0x10);

        let recovery = recover_request_dma_callees(&rom, &db, 1);
        assert!(
            recovery.admitted.is_empty(),
            "a size that matches no record must not be admitted, got {:?}",
            recovery.admitted
        );
    }

    #[test]
    fn bounded_request_dma_refuses_a_slice_from_an_oversized_vrom_file() {
        const DECODED_FILE_BYTES: u32 = 0x0010_0000;
        const PHYSICAL_START: usize = 0x2000;
        let mut rom = rom_with_boot_words(
            0x8000_0400,
            &boot_calling_loader(FIXTURE_VROM, 4, FIXTURE_LOADER),
        );
        rom.bytes[PHYSICAL_START..PHYSICAL_START + 4].copy_from_slice(b"Yaz0");
        rom.bytes[PHYSICAL_START + 4..PHYSICAL_START + 8]
            .copy_from_slice(&DECODED_FILE_BYTES.to_be_bytes());
        let mut db = FactDb::new();
        let _outcome = publish_boot_bank(
            &rom,
            &mut db,
            RecognizedIpl3::Cic6102Or7101,
            IPL3_SHA256_CIC_6102_7101.to_string(),
        );
        let record = db.insert(Fact::LoadImageTableRecord {
            table: "oversized".to_string(),
            bank: None,
            table_space: RomAddressSpace::Physical,
            table_offset: 0,
            index: 0,
            source_space: crate::facts::MappingAddressSpace::VirtualRom,
            source_start: FIXTURE_VROM,
            source_end: FIXTURE_VROM + DECODED_FILE_BYTES,
            destination_space: crate::facts::MappingAddressSpace::PhysicalRom,
            destination_start: PHYSICAL_START as u32,
            destination_end: (PHYSICAL_START + 16) as u32,
        });
        db.conclude(
            load_image_table_record_subject("oversized", 0),
            ProofState::Proven,
            vec![record],
            "test fixture",
        )
        .expect("fresh conclusion");
        let input = StaticRequestDmaInput {
            name: "oversized_request".to_string(),
            callee_va: FIXTURE_LOADER,
            dram_arg_register: 4,
            device_arg_register: 5,
            size_arg_register: 6,
            size_is_end_address: false,
            device_space: RomAddressSpace::Virtual,
            bank_name: BankNamePattern::new("request_dma_", 0, ""),
        };

        let report = scan_static_request_dma_bounded(&rom, &[input], &mut db, 1024);
        assert!(report.proven_banks.is_empty());
        assert!(report
            .open
            .iter()
            .any(|reason| reason.contains("exceeds transient limit 1024")));
        assert!(!db.proven_rom_mappings().iter().any(
            |fact| matches!(fact, Fact::RomMapping { bank, .. } if bank.starts_with("request_dma_"))
        ));
        crate::harvest::harvest_discovered_candidates_bounded(&rom, &mut db, 1024)
            .expect("the rejected request mapping cannot reach harvest");
    }
}
