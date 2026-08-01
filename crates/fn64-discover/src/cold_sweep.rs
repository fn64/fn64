//! Cold, path-free discovery measurement for one ROM.
//!
//! The caller supplies only ROM bytes. No donor, answer key, trace, manifest,
//! or game-specific configuration enters [`measure_cold_rom`]. Snapshot
//! composition is attempted from the facts returned by
//! [`crate::run_discovery_auto`]; a composition frontier stays explicit and
//! never masquerades as a zero-unsupported scoreboard.

use crate::closure::{ClosureScoreboard, DestinationClass, DestinationReason};
use crate::ledger::build_ledger;
use crate::snapshot::{
    compose_materialized_banks_validated_v2_with_limits, MultiBankCompositionLimits,
};
use crate::snapshot_inputs::{
    prepare_snapshot_banks_with_limits, PrepareSnapshotBanksError, PrepareSnapshotBanksLimits,
};
use crate::stage1_effects::{scan_stage1_effects, summarize_stage1_effects, Stage1EffectSummaryV1};
use crate::{
    run_discovery_auto_with_limits, AutoDiscoveryLimits, DiscoveryStrategy, RomRejectReason,
    StrategyOutcome,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fmt;

pub const COLD_ROM_RECEIPT_SCHEMA_V2: &str = "fn64.cold-rom-measurement.v2";
pub const COLD_ROM_MAX_INPUT_BYTES: usize = 64 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ColdSweepError {
    RomTooLarge { bytes: usize, limit: usize },
    RomRejected(RomRejectReason),
}

impl fmt::Display for ColdSweepError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::RomTooLarge { bytes, limit } => {
                write!(
                    formatter,
                    "ROM input is {bytes} bytes, exceeding the {limit}-byte cold-sweep bound"
                )
            }
            Self::RomRejected(error) => error.fmt(formatter),
        }
    }
}

impl std::error::Error for ColdSweepError {}

impl From<RomRejectReason> for ColdSweepError {
    fn from(error: RomRejectReason) -> Self {
        Self::RomRejected(error)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ColdSweepLimitsV2 {
    pub max_rom_input_bytes: u64,
    pub max_decoded_vrom_file_bytes: u64,
    pub max_banks: u64,
    pub max_aggregate_materialized_bytes: u64,
    pub max_projected_fact_rows: u64,
    pub max_projected_fact_bytes: u64,
    pub max_cross_bank_authority_records: u64,
}

impl ColdSweepLimitsV2 {
    pub const fn fixed() -> Self {
        Self {
            max_rom_input_bytes: COLD_ROM_MAX_INPUT_BYTES as u64,
            max_decoded_vrom_file_bytes: COLD_ROM_MAX_INPUT_BYTES as u64,
            max_banks: 4096,
            max_aggregate_materialized_bytes: COLD_ROM_MAX_INPUT_BYTES as u64,
            max_projected_fact_rows: 4_000_000,
            max_projected_fact_bytes: 256 * 1024 * 1024,
            max_cross_bank_authority_records: 1_048_576,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ColdCompositionBlockerV2 {
    NoProvenMappings,
    BankPreparationRejected,
    SnapshotCompositionRejected,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, tag = "status", rename_all = "snake_case")]
pub enum ColdClosureMeasurementV2 {
    Measured { scoreboard: ClosureScoreboard },
    Open { blocker: ColdCompositionBlockerV2 },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ColdStage1EffectBlockerV2 {
    CompositionUnavailable,
    SnapshotBankMissing,
    ScanRejected,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, tag = "status", rename_all = "snake_case")]
pub enum ColdStage1EffectMeasurementV2 {
    Measured { summary: Stage1EffectSummaryV1 },
    Open { blocker: ColdStage1EffectBlockerV2 },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ColdRomMeasurementV2 {
    pub schema: String,
    pub limits: ColdSweepLimitsV2,
    pub normalized_rom_sha256: String,
    pub selected_strategy: DiscoveryStrategy,
    pub strategy_outcomes: Vec<StrategyOutcome>,
    pub fact_count: usize,
    pub overlay_relocation_fact_count: usize,
    pub proven_bank_count: usize,
    pub closure: ColdClosureMeasurementV2,
    pub stage1_effects: ColdStage1EffectMeasurementV2,
    pub ledger_total_bytes: u64,
    pub ledger_code_like_floor_bytes: u64,
    pub ledger_bytes_by_class: BTreeMap<String, u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ColdRomReceiptV2 {
    pub measurement: ColdRomMeasurementV2,
    pub receipt_sha256: String,
}

/// Non-authoritative diagnostic text kept outside the sealed, path-free
/// receipt. It explains why composition remained open without making error
/// formatting part of the receipt identity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ColdRomRunV2 {
    pub receipt: ColdRomReceiptV2,
    pub composition_diagnostic: Option<String>,
}

pub fn measure_cold_rom(rom_bytes: &[u8]) -> Result<ColdRomRunV2, ColdSweepError> {
    validate_rom_input_len(rom_bytes.len())?;
    let limits = ColdSweepLimitsV2::fixed();
    let auto = run_discovery_auto_with_limits(
        rom_bytes,
        AutoDiscoveryLimits {
            vrom_materialization: crate::file_table::VromMaterializationLimits {
                max_decoded_file_bytes: limits.max_decoded_vrom_file_bytes as usize,
            },
        },
    )?;

    let proven_bank_count = auto.facts.proven_rom_mappings().len();
    let (closure, stage1_effects, composition_diagnostic) = match prepare_snapshot_banks_with_limits(
        &auto.rom,
        &auto.facts,
        PrepareSnapshotBanksLimits {
            max_banks: limits.max_banks as usize,
            max_aggregate_rom_bytes: limits.max_aggregate_materialized_bytes,
            max_decoded_vrom_file_bytes: limits.max_decoded_vrom_file_bytes as usize,
        },
    ) {
        Ok(prepared) => {
            let inputs = prepared.materialized_inputs();
            match compose_materialized_banks_validated_v2_with_limits(
                &auto.rom,
                &auto.facts,
                &inputs,
                MultiBankCompositionLimits {
                    max_projected_fact_rows: limits.max_projected_fact_rows,
                    max_projected_fact_bytes: limits.max_projected_fact_bytes,
                    max_aggregate_materialized_bytes: limits.max_aggregate_materialized_bytes,
                    max_cross_bank_authority_records: limits.max_cross_bank_authority_records,
                },
            ) {
                Ok(composed) => {
                    let mut reports = Vec::with_capacity(inputs.len());
                    let mut effect_error = None;
                    for input in &inputs {
                        let matching_banks = composed
                            .snapshots()
                            .iter()
                            .flat_map(|snapshot| &snapshot.banks)
                            .filter(|bank| {
                                bank.input.bank == input.bank
                                    && bank.input.va_start == input.va_start
                                    && bank.input.va_end
                                        == input.va_start.saturating_add(input.bytes.len() as u32)
                            })
                            .collect::<Vec<_>>();
                        let [bank] = matching_banks.as_slice() else {
                            effect_error = Some((
                                ColdStage1EffectBlockerV2::SnapshotBankMissing,
                                format!(
                                    "stage-1 effect scan found {} snapshot banks for {}",
                                    matching_banks.len(),
                                    input.bank
                                ),
                            ));
                            break;
                        };
                        match scan_stage1_effects(
                            input.bank,
                            input.bytes,
                            input.va_start,
                            &bank.authority_closure.cfg,
                        ) {
                            Ok(report) => reports.push(report),
                            Err(error) => {
                                effect_error = Some((
                                    ColdStage1EffectBlockerV2::ScanRejected,
                                    error.to_string(),
                                ));
                                break;
                            }
                        }
                    }
                    let (stage1_effects, diagnostic) = effect_error.map_or_else(
                        || {
                            (
                                ColdStage1EffectMeasurementV2::Measured {
                                    summary: summarize_stage1_effects(&reports),
                                },
                                None,
                            )
                        },
                        |(blocker, diagnostic)| {
                            (
                                ColdStage1EffectMeasurementV2::Open { blocker },
                                Some(diagnostic),
                            )
                        },
                    );
                    (
                        ColdClosureMeasurementV2::Measured {
                            scoreboard: crate::closure::scoreboard(composed.snapshots()),
                        },
                        stage1_effects,
                        diagnostic,
                    )
                }
                Err(error) => (
                    ColdClosureMeasurementV2::Open {
                        blocker: ColdCompositionBlockerV2::SnapshotCompositionRejected,
                    },
                    ColdStage1EffectMeasurementV2::Open {
                        blocker: ColdStage1EffectBlockerV2::CompositionUnavailable,
                    },
                    Some(error.to_string()),
                ),
            }
        }
        Err(PrepareSnapshotBanksError::NoProvenMappings) => (
            ColdClosureMeasurementV2::Open {
                blocker: ColdCompositionBlockerV2::NoProvenMappings,
            },
            ColdStage1EffectMeasurementV2::Open {
                blocker: ColdStage1EffectBlockerV2::CompositionUnavailable,
            },
            Some(PrepareSnapshotBanksError::NoProvenMappings.to_string()),
        ),
        Err(error) => (
            ColdClosureMeasurementV2::Open {
                blocker: ColdCompositionBlockerV2::BankPreparationRejected,
            },
            ColdStage1EffectMeasurementV2::Open {
                blocker: ColdStage1EffectBlockerV2::CompositionUnavailable,
            },
            Some(error.to_string()),
        ),
    };
    let ledger = build_ledger(&auto.rom.bytes, &auto.facts);
    let overlay_relocation_fact_count = auto
        .facts
        .facts()
        .iter()
        .filter(|fact| matches!(fact, crate::facts::Fact::OverlayRelocation { .. }))
        .count();
    let measurement = ColdRomMeasurementV2 {
        schema: COLD_ROM_RECEIPT_SCHEMA_V2.to_owned(),
        limits,
        normalized_rom_sha256: auto.rom.sha256,
        selected_strategy: auto.selected,
        strategy_outcomes: auto.outcomes,
        fact_count: auto.facts.facts().len(),
        overlay_relocation_fact_count,
        proven_bank_count,
        closure,
        stage1_effects,
        ledger_total_bytes: ledger.total_bytes,
        ledger_code_like_floor_bytes: ledger.undiscovered_code_bytes(),
        ledger_bytes_by_class: ledger.bytes_by_class,
    };
    let encoded = serde_json::to_vec(&measurement)
        .expect("cold ROM measurement contains only serializable closed types");
    let receipt_sha256 = format!("{:x}", Sha256::digest(encoded));
    Ok(ColdRomRunV2 {
        receipt: ColdRomReceiptV2 {
            measurement,
            receipt_sha256,
        },
        composition_diagnostic,
    })
}

fn validate_rom_input_len(bytes: usize) -> Result<(), ColdSweepError> {
    if bytes > COLD_ROM_MAX_INPUT_BYTES {
        Err(ColdSweepError::RomTooLarge {
            bytes,
            limit: COLD_ROM_MAX_INPUT_BYTES,
        })
    } else {
        Ok(())
    }
}

impl ColdRomReceiptV2 {
    pub fn verify(&self) -> Result<(), String> {
        if self.measurement.schema != COLD_ROM_RECEIPT_SCHEMA_V2 {
            return Err(format!(
                "cold receipt schema must be {COLD_ROM_RECEIPT_SCHEMA_V2}, got {}",
                self.measurement.schema
            ));
        }
        if self.measurement.limits != ColdSweepLimitsV2::fixed() {
            return Err("cold receipt resource envelope differs from schema v2".to_owned());
        }
        validate_lowercase_sha256(
            &self.measurement.normalized_rom_sha256,
            "normalized ROM SHA-256",
        )?;
        validate_lowercase_sha256(&self.receipt_sha256, "receipt SHA-256")?;
        if let ColdClosureMeasurementV2::Measured { scoreboard } = &self.measurement.closure {
            if scoreboard.per_class.len() != DestinationClass::ALL.len()
                || DestinationClass::ALL
                    .into_iter()
                    .any(|class| !scoreboard.per_class.contains_key(class.label()))
            {
                return Err("cold receipt closure omits a destination tier".to_owned());
            }
            if scoreboard.per_reason.len() != DestinationReason::ALL.len()
                || DestinationReason::ALL
                    .into_iter()
                    .any(|reason| !scoreboard.per_reason.contains_key(reason.label()))
            {
                return Err("cold receipt closure omits a destination-reason bucket".to_owned());
            }
            let total = DestinationClass::ALL
                .into_iter()
                .try_fold(0u64, |total, class| {
                    total.checked_add(scoreboard.tally(class).destinations)
                });
            let Some(total) = total else {
                return Err("cold receipt closure tier total overflows u64".to_owned());
            };
            if total != scoreboard.total_destinations
                || scoreboard.unsupported
                    != scoreboard.tally(DestinationClass::Unsupported).destinations
                || scoreboard.dynamic_mips
                    != scoreboard.tally(DestinationClass::DynamicMips).destinations
            {
                return Err("cold receipt closure totals are internally inconsistent".to_owned());
            }
            let reason_total = DestinationReason::ALL
                .into_iter()
                .try_fold(0u64, |total, reason| {
                    total.checked_add(scoreboard.reason_count(reason))
                });
            let Some(reason_total) = reason_total else {
                return Err("cold receipt destination-reason total overflows u64".to_owned());
            };
            if reason_total != scoreboard.total_destinations {
                return Err(
                    "cold receipt destination reasons do not sum to total destinations".to_owned(),
                );
            }
            let reason_tier_destinations = [
                (
                    DestinationClass::ExactAot,
                    scoreboard.reason_count(DestinationReason::InExactOwner),
                ),
                (
                    DestinationClass::BlockAot,
                    scoreboard.reason_count(DestinationReason::InProvenBlock),
                ),
                (
                    DestinationClass::DynamicMips,
                    [
                        DestinationReason::OpenIndirectSite,
                        DestinationReason::BoundedIndirectSite,
                        DestinationReason::MappedNotProvenCode,
                        DestinationReason::ProvenCodeNoOwner,
                    ]
                    .into_iter()
                    .try_fold(0u64, |total, reason| {
                        total.checked_add(scoreboard.reason_count(reason))
                    })
                    .ok_or_else(|| {
                        "cold receipt dynamic-MIPS reason total overflows u64".to_owned()
                    })?,
                ),
                (
                    DestinationClass::Unsupported,
                    [
                        DestinationReason::IntoProvenData,
                        DestinationReason::OutsideAllMappings,
                    ]
                    .into_iter()
                    .try_fold(0u64, |total, reason| {
                        total.checked_add(scoreboard.reason_count(reason))
                    })
                    .ok_or_else(|| {
                        "cold receipt unsupported reason total overflows u64".to_owned()
                    })?,
                ),
            ];
            if reason_tier_destinations
                .into_iter()
                .any(|(class, reasons)| reasons != scoreboard.tally(class).destinations)
            {
                return Err(
                    "cold receipt destination reasons disagree with destination tiers".to_owned(),
                );
            }
        }
        let ledger_total = self
            .measurement
            .ledger_bytes_by_class
            .values()
            .copied()
            .try_fold(0u64, u64::checked_add)
            .ok_or_else(|| "cold receipt ledger class total overflows u64".to_owned())?;
        if ledger_total != self.measurement.ledger_total_bytes {
            return Err("cold receipt ledger classes do not sum to total bytes".to_owned());
        }
        if self.measurement.ledger_code_like_floor_bytes
            != self
                .measurement
                .ledger_bytes_by_class
                .get(crate::ledger::SpanClass::CodeLike.label())
                .copied()
                .unwrap_or_default()
        {
            return Err(
                "cold receipt code-like floor disagrees with the ledger code-like bucket"
                    .to_owned(),
            );
        }
        let encoded = serde_json::to_vec(&self.measurement)
            .map_err(|error| format!("serializing cold measurement for verification: {error}"))?;
        let expected = format!("{:x}", Sha256::digest(encoded));
        if self.receipt_sha256 != expected {
            return Err(format!(
                "cold receipt digest mismatch: expected {expected}, got {}",
                self.receipt_sha256
            ));
        }
        Ok(())
    }
}

fn validate_lowercase_sha256(value: &str, label: &str) -> Result<(), String> {
    if value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        Ok(())
    } else {
        Err(format!("{label} must be 64 lowercase hexadecimal digits"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::closure::ClassTally;

    fn synthetic_rom() -> Vec<u8> {
        let mut bytes = vec![0u8; 0x3000];
        bytes[0..4].copy_from_slice(&0x8037_1240u32.to_be_bytes());
        bytes[8..12].copy_from_slice(&0x8000_0400u32.to_be_bytes());
        bytes[0x20..0x24].copy_from_slice(b"TEST");
        bytes[0x3b..0x3f].copy_from_slice(b"CTSE");
        bytes
    }

    #[test]
    fn unknown_ipl3_stays_open_instead_of_reporting_zero_unsupported() {
        let run = measure_cold_rom(&synthetic_rom()).unwrap();
        assert!(matches!(
            &run.receipt.measurement.closure,
            ColdClosureMeasurementV2::Open {
                blocker: ColdCompositionBlockerV2::NoProvenMappings
            }
        ));
        assert_eq!(run.receipt.measurement.proven_bank_count, 0);
        assert_eq!(
            run.receipt.measurement.ledger_code_like_floor_bytes,
            run.receipt
                .measurement
                .ledger_bytes_by_class
                .get(crate::ledger::SpanClass::CodeLike.label())
                .copied()
                .unwrap_or_default()
        );
        run.receipt.verify().unwrap();
        let encoded = serde_json::to_string(&run.receipt).unwrap();
        assert!(!encoded.contains("unsupported_event_count"));
    }

    #[test]
    fn receipt_is_path_free_and_deterministic_for_ten_runs() {
        let rom = synthetic_rom();
        let first_run = measure_cold_rom(&rom).unwrap();
        let expected_digest = format!(
            "{:x}",
            Sha256::digest(serde_json::to_vec(&first_run.receipt.measurement).unwrap())
        );
        assert_eq!(first_run.receipt.receipt_sha256, expected_digest);
        first_run.receipt.verify().unwrap();
        let first = serde_json::to_vec(&first_run.receipt).unwrap();
        for _ in 1..10 {
            let next = serde_json::to_vec(&measure_cold_rom(&rom).unwrap().receipt).unwrap();
            assert_eq!(next, first);
        }
        let text = String::from_utf8(first).unwrap();
        assert!(!text.contains("/Users/"));
        assert!(!text.contains(".z64"));
    }

    #[test]
    fn oversized_input_is_rejected_without_allocating_it() {
        assert!(matches!(
            validate_rom_input_len(COLD_ROM_MAX_INPUT_BYTES + 4),
            Err(ColdSweepError::RomTooLarge { .. })
        ));
    }

    #[test]
    fn receipt_rejects_digest_tampering_and_unknown_fields() {
        let mut receipt = measure_cold_rom(&synthetic_rom()).unwrap().receipt;
        receipt.receipt_sha256 = "0".repeat(64);
        assert!(receipt.verify().unwrap_err().contains("digest mismatch"));

        let mut value = serde_json::to_value(receipt).unwrap();
        value
            .as_object_mut()
            .unwrap()
            .insert("unknown".to_owned(), serde_json::Value::Null);
        assert!(serde_json::from_value::<ColdRomReceiptV2>(value).is_err());
    }

    fn reseal(receipt: &mut ColdRomReceiptV2) {
        receipt.receipt_sha256 = format!(
            "{:x}",
            Sha256::digest(serde_json::to_vec(&receipt.measurement).unwrap())
        );
    }

    #[test]
    fn receipt_rejects_resealed_relational_tampering() {
        let mut reason_sum = measure_cold_rom(&synthetic_rom()).unwrap().receipt;
        let ColdClosureMeasurementV2::Open { .. } = reason_sum.measurement.closure else {
            panic!("synthetic ROM should retain an open closure");
        };

        // Exercise the measured-closure verifier with a complete but inconsistent
        // scoreboard; recomputing the digest proves the relational check, rather
        // than the outer checksum, rejects it.
        let mut per_class = BTreeMap::new();
        for class in DestinationClass::ALL {
            per_class.insert(class.label().to_owned(), ClassTally::default());
        }
        per_class.insert(
            DestinationClass::BlockAot.label().to_owned(),
            ClassTally {
                destinations: 1,
                bytes: 4,
            },
        );
        let mut per_reason = BTreeMap::new();
        for reason in DestinationReason::ALL {
            per_reason.insert(reason.label().to_owned(), 0);
        }
        per_reason.insert(DestinationReason::InExactOwner.label().to_owned(), 1);
        reason_sum.measurement.closure = ColdClosureMeasurementV2::Measured {
            scoreboard: ClosureScoreboard {
                total_destinations: 1,
                per_class,
                per_reason,
                unsupported: 0,
                dynamic_mips: 0,
            },
        };
        reseal(&mut reason_sum);
        assert!(reason_sum
            .verify()
            .unwrap_err()
            .contains("reasons disagree"));

        let mut reason_total = reason_sum.clone();
        let ColdClosureMeasurementV2::Measured { scoreboard } =
            &mut reason_total.measurement.closure
        else {
            unreachable!()
        };
        scoreboard
            .per_reason
            .insert(DestinationReason::InExactOwner.label().to_owned(), 2);
        reseal(&mut reason_total);
        assert!(reason_total.verify().unwrap_err().contains("do not sum"));

        let mut code_like = measure_cold_rom(&synthetic_rom()).unwrap().receipt;
        code_like.measurement.ledger_code_like_floor_bytes = code_like
            .measurement
            .ledger_code_like_floor_bytes
            .saturating_add(4);
        reseal(&mut code_like);
        assert!(code_like
            .verify()
            .unwrap_err()
            .contains("code-like floor disagrees"));
    }
}
