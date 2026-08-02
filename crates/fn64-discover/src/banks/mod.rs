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
                "uniquely admitted ROM-only descriptor table at 0x{:x}, record {index}: ROM [0x{:x},0x{:x}) -> normalized descriptor VA 0x{:08x} (field={}); {delta_note}",
                table.table_rom_offset,
                record.rom_start,
                record.rom_end,
                record.vram_dest,
                table.destination_field.label(),
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

mod request_dma;
pub use request_dma::*;

#[cfg(test)]
mod tests;
