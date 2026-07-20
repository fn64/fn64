//! ROM-bound external evidence for discovery facts that have not yet been
//! inferred mechanically. The manifest is data, never executable callbacks,
//! and the normalized ROM digest is checked before any claim is consumed.

use crate::banks::{self, BankNamePattern, DescriptorTableShape, LoadImageTableInput};
use crate::facts::{executable_range_subject, BankAddr, Fact, FactDb, ProofState};
use crate::rom::NormalizedRom;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub const EVIDENCE_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EvidenceManifest {
    pub schema_version: u32,
    /// SHA-256 of the normalized big-endian ROM, not the source file's byte
    /// order. A manifest for one revision can never silently apply to another.
    pub rom_sha256: String,
    #[serde(default)]
    pub descriptor_tables: Vec<DescriptorTableEvidence>,
    #[serde(default)]
    pub load_image_tables: Vec<LoadImageTableInput>,
    #[serde(default)]
    pub executable_ranges: Vec<ExecutableRangeEvidence>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DescriptorTableEvidence {
    pub name: String,
    pub source: String,
    pub shape: DescriptorTableShape,
    pub bank_name: BankNamePattern,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExecutableRangeEvidence {
    pub bank: String,
    pub va_start: u32,
    pub va_end: u32,
    /// Human-readable provenance such as a loader backward slice, PI DMA
    /// trace identifier, or a prior clean-room analysis artifact.
    pub source: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EvidenceError {
    Toml(String),
    UnsupportedSchema { found: u32 },
    RomDigestMismatch { expected: String, actual: String },
    EmptySource { subject: String },
    InvalidExecutableRange { subject: String, reason: String },
}

impl std::fmt::Display for EvidenceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Toml(error) => write!(f, "invalid evidence manifest TOML: {error}"),
            Self::UnsupportedSchema { found } => write!(
                f,
                "unsupported discovery evidence schema {found}; expected {EVIDENCE_SCHEMA_VERSION}"
            ),
            Self::RomDigestMismatch { expected, actual } => write!(
                f,
                "evidence manifest is bound to normalized ROM SHA-256 {expected}, got {actual}"
            ),
            Self::EmptySource { subject } => {
                write!(
                    f,
                    "evidence manifest claim {subject:?} has no provenance source"
                )
            }
            Self::InvalidExecutableRange { subject, reason } => {
                write!(f, "invalid executable range {subject}: {reason}")
            }
        }
    }
}

impl std::error::Error for EvidenceError {}

impl EvidenceManifest {
    pub fn from_toml(text: &str) -> Result<Self, EvidenceError> {
        toml::from_str(text).map_err(|error| EvidenceError::Toml(error.to_string()))
    }

    pub fn validate_identity(&self, rom: &NormalizedRom) -> Result<(), EvidenceError> {
        if self.schema_version != EVIDENCE_SCHEMA_VERSION {
            return Err(EvidenceError::UnsupportedSchema {
                found: self.schema_version,
            });
        }
        if self.rom_sha256 != rom.sha256 {
            return Err(EvidenceError::RomDigestMismatch {
                expected: self.rom_sha256.clone(),
                actual: rom.sha256.clone(),
            });
        }
        for table in &self.descriptor_tables {
            require_source(&table.name, &table.source)?;
        }
        for range in &self.executable_ranges {
            require_source(
                &executable_range_subject(&range.bank, range.va_start, range.va_end),
                &range.source,
            )?;
        }
        Ok(())
    }
}

fn require_source(subject: &str, source: &str) -> Result<(), EvidenceError> {
    if source.trim().is_empty() {
        return Err(EvidenceError::EmptySource {
            subject: subject.to_string(),
        });
    }
    Ok(())
}

/// Apply only Phase-2 mapping claims. This is separate from executable ranges
/// so all ranges are validated against the complete mapping set before code
/// harvesting begins.
pub fn apply_mapping_evidence(
    rom: &NormalizedRom,
    manifest: &EvidenceManifest,
    db: &mut FactDb,
) -> Result<(), EvidenceError> {
    manifest.validate_identity(rom)?;
    for table in &manifest.descriptor_tables {
        let pattern = &table.bank_name;
        banks::scan_descriptor_table(rom, table.shape, |index| pattern.name(index), db);
        db.insert(Fact::Evidence {
            subject: BankAddr::new(&table.name, table.shape.table_rom_offset),
            note: format!(
                "external manifest descriptor-table source: {}",
                table.source
            ),
        });
    }
    banks::scan_load_image_tables(rom, &manifest.load_image_tables, db);
    Ok(())
}

/// Validate and record executable intervals. Ranges must be word-aligned,
/// non-overlapping within a bank, and contained in exactly one proven load
/// image. These checks prevent a stale or overly broad manifest from turning
/// arbitrary mapped data into code candidates.
pub fn apply_executable_evidence(
    manifest: &EvidenceManifest,
    db: &mut FactDb,
) -> Result<(), EvidenceError> {
    let mut prior_by_bank: BTreeMap<&str, Vec<(u32, u32)>> = BTreeMap::new();
    for range in &manifest.executable_ranges {
        let subject = executable_range_subject(&range.bank, range.va_start, range.va_end);
        if range.va_end <= range.va_start {
            return Err(invalid_range(&subject, "range is empty or inverted"));
        }
        if !range.va_start.is_multiple_of(4) || !range.va_end.is_multiple_of(4) {
            return Err(invalid_range(
                &subject,
                "range is not instruction-word aligned",
            ));
        }
        let containing_mappings = db
            .proven_rom_mappings()
            .into_iter()
            .filter(|fact| {
                matches!(
                    fact,
                    Fact::RomMapping {
                        bank,
                        rom_start,
                        rom_end,
                        va_start,
                        ..
                    } if bank == &range.bank
                        && range.va_start >= *va_start
                        && range.va_end
                            <= va_start.saturating_add(rom_end.saturating_sub(*rom_start))
                )
            })
            .count();
        if containing_mappings != 1 {
            return Err(invalid_range(
                &subject,
                &format!(
                    "contained in {containing_mappings} proven load images; expected exactly one"
                ),
            ));
        }
        let prior = prior_by_bank.entry(&range.bank).or_default();
        if prior
            .iter()
            .any(|(start, end)| range.va_start < *end && range.va_end > *start)
        {
            return Err(invalid_range(
                &subject,
                "overlaps another executable range for this bank",
            ));
        }
        prior.push((range.va_start, range.va_end));

        let fact = db.insert(Fact::ExecutableRange {
            bank: range.bank.clone(),
            va_start: range.va_start,
            va_end: range.va_end,
        });
        let provenance = db.insert(Fact::Evidence {
            subject: BankAddr::new(&range.bank, range.va_start),
            note: format!(
                "external manifest executable-range source: {}",
                range.source
            ),
        });
        db.conclude(
            subject,
            ProofState::Proven,
            vec![fact, provenance],
            "rom_bound_external_executable_range",
        )
        .expect("new executable-range subject cannot violate monotonicity");
    }
    Ok(())
}

fn invalid_range(subject: &str, reason: &str) -> EvidenceError {
    EvidenceError::InvalidExecutableRange {
        subject: subject.to_string(),
        reason: reason.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::facts::{CandidateDetector, Fact};

    fn test_rom() -> Vec<u8> {
        let mut bytes = vec![0u8; 0x3000];
        bytes[0..4].copy_from_slice(&0x8037_1240u32.to_be_bytes());
        bytes[8..12].copy_from_slice(&0x8000_0400u32.to_be_bytes());
        bytes[0x20..0x2c].copy_from_slice(b"EVIDENCE ROM");
        bytes[0x3b..0x3f].copy_from_slice(b"CTSE");
        for offset in [0x1000usize, 0x1100] {
            for (index, word) in [0x27bd_ffe0u32, 0xafbf_001c, 0x03e0_0008, 0x27bd_0020]
                .into_iter()
                .enumerate()
            {
                let start = offset + index * 4;
                bytes[start..start + 4].copy_from_slice(&word.to_be_bytes());
            }
        }
        bytes
    }

    fn manifest_for(rom: &NormalizedRom) -> EvidenceManifest {
        EvidenceManifest {
            schema_version: EVIDENCE_SCHEMA_VERSION,
            rom_sha256: rom.sha256.clone(),
            descriptor_tables: vec![],
            load_image_tables: vec![],
            executable_ranges: vec![ExecutableRangeEvidence {
                bank: banks::BOOT_BANK.to_string(),
                va_start: 0x8000_0400,
                va_end: 0x8000_0410,
                source: "synthetic test text interval".to_string(),
            }],
        }
    }

    #[test]
    fn manifest_round_trips_as_toml_data() {
        let rom = crate::rom::normalize(&test_rom()).unwrap();
        let manifest = manifest_for(&rom);
        let text = toml::to_string_pretty(&manifest).unwrap();
        assert_eq!(EvidenceManifest::from_toml(&text).unwrap(), manifest);
        assert!(text.contains("rom_sha256"));
        assert!(!text.contains("fn("));
    }

    #[test]
    fn digest_mismatch_is_rejected_before_evidence_is_used() {
        let bytes = test_rom();
        let rom = crate::rom::normalize(&bytes).unwrap();
        let mut manifest = manifest_for(&rom);
        manifest.rom_sha256 = "00".repeat(32);
        let error = crate::run_discovery_with_manifest(&bytes, &manifest).unwrap_err();
        assert!(matches!(
            error,
            crate::DiscoveryError::Evidence(EvidenceError::RomDigestMismatch { .. })
        ));
    }

    #[test]
    fn executable_range_prevents_mapped_data_from_becoming_code_candidates() {
        let bytes = test_rom();
        let rom = crate::rom::normalize(&bytes).unwrap();
        let (_, unrestricted) = crate::run_discovery(&bytes, None).unwrap();
        let (_, restricted) =
            crate::run_discovery_with_manifest(&bytes, &manifest_for(&rom)).unwrap();

        let prologue_pcs = |db: &FactDb| {
            db.facts()
                .iter()
                .filter_map(|fact| match fact {
                    Fact::FunctionEntryClaim {
                        target,
                        detector: CandidateDetector::ProloguePattern,
                        ..
                    } => Some(target.pc),
                    _ => None,
                })
                .collect::<Vec<_>>()
        };
        assert!(prologue_pcs(&unrestricted).contains(&0x8000_0500));
        assert_eq!(prologue_pcs(&restricted), vec![0x8000_0400]);
    }

    #[test]
    fn executable_range_must_be_rom_backed_and_non_overlapping() {
        let bytes = test_rom();
        let rom = crate::rom::normalize(&bytes).unwrap();
        let mut manifest = manifest_for(&rom);
        manifest.executable_ranges.push(ExecutableRangeEvidence {
            bank: banks::BOOT_BANK.to_string(),
            va_start: 0x8000_0408,
            va_end: 0x8000_0420,
            source: "overlapping synthetic interval".to_string(),
        });
        let error = crate::run_discovery_with_manifest(&bytes, &manifest).unwrap_err();
        assert!(matches!(
            error,
            crate::DiscoveryError::Evidence(EvidenceError::InvalidExecutableRange { .. })
        ));
    }
}
