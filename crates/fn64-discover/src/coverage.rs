//! Coverage metrics kept separate from function-entry precision/recall.
//! Physical ROM coverage, logical load-image coverage, executable coverage,
//! and function-entry proof states answer different questions and must never
//! be collapsed into one percentage.

use crate::facts::{
    executable_range_subject, function_entry_subject, Fact, FactDb, MappingAddressSpace,
    ProofState, RomAddressSpace,
};
use crate::owner_proof::{OwnerAssessment, OwnerBlocker, OwnerProofReport};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OwnerProofRunState {
    NotRun,
    Run,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OwnerBlockerCount {
    pub blocker: OwnerBlocker,
    pub assessments: u64,
}

/// Exact-owner coverage is deliberately separate from function-entry
/// coverage. An authoritative entry does not imply that its end address is
/// known, and candidate geometry never contributes to `exact_owner_bytes`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OwnerProofCoverage {
    pub state: OwnerProofRunState,
    pub analyzed_banks: u64,
    pub assessed_entries: u64,
    pub exact_owners: u64,
    pub candidate_owners: u64,
    pub ambiguous_owners: u64,
    pub exact_owner_bytes: u64,
    pub blockers: Vec<OwnerBlockerCount>,
}

impl OwnerProofCoverage {
    fn not_run() -> Self {
        Self {
            state: OwnerProofRunState::NotRun,
            analyzed_banks: 0,
            assessed_entries: 0,
            exact_owners: 0,
            candidate_owners: 0,
            ambiguous_owners: 0,
            exact_owner_bytes: 0,
            blockers: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OwnerProofCoverageError {
    NoAssessments,
    ReportBankMismatch {
        report_bank: String,
        entry_bank: String,
        entry_pc: u32,
    },
    DuplicateAssessment {
        bank: String,
        entry_pc: u32,
    },
    InvalidExactExtent {
        bank: String,
        entry_pc: u32,
        va_end: u32,
        rom_start: u32,
        rom_end: u32,
    },
    UnresolvedOwners {
        candidates: u64,
        ambiguous: u64,
    },
}

impl std::fmt::Display for OwnerProofCoverageError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoAssessments => write!(f, "owner proof produced no assessments"),
            Self::ReportBankMismatch {
                report_bank,
                entry_bank,
                entry_pc,
            } => write!(
                f,
                "owner report bank {report_bank:?} contains entry {entry_bank}:0x{entry_pc:08x}"
            ),
            Self::DuplicateAssessment { bank, entry_pc } => write!(
                f,
                "owner {bank}:0x{entry_pc:08x} was assessed more than once"
            ),
            Self::InvalidExactExtent {
                bank,
                entry_pc,
                va_end,
                rom_start,
                rom_end,
            } => write!(
                f,
                "exact owner {bank}:0x{entry_pc:08x} has invalid VA/ROM extents ending at 0x{va_end:08x} and [0x{rom_start:08x},0x{rom_end:08x})"
            ),
            Self::UnresolvedOwners {
                candidates,
                ambiguous,
            } => write!(
                f,
                "owner proof has {candidates} candidate and {ambiguous} ambiguous assessments"
            ),
        }
    }
}

impl std::error::Error for OwnerProofCoverageError {}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CoverageReport {
    pub total_rom_bytes: u64,
    /// Unique physical ROM bytes directly named by proven load images.
    pub direct_physical_load_bytes: u64,
    /// Unique physical ROM bytes known to back VROM files. This may include
    /// non-code assets and is therefore not executable coverage.
    pub known_file_backing_bytes: u64,
    /// Sum of proven load-image byte lengths in their logical bank identity.
    /// Aliased overlays count separately because `(bank, pc)` identities do.
    pub logical_load_image_bytes: u64,
    /// Sum of proven executable intervals per bank, unioned within each bank.
    pub executable_bytes: u64,
    pub mapped_banks: u64,
    pub executable_banks: u64,
    pub function_entries_by_state: BTreeMap<ProofState, u64>,
    pub function_owners: OwnerProofCoverage,
}

pub fn report(total_rom_bytes: usize, db: &FactDb) -> CoverageReport {
    report_base(total_rom_bytes, db, OwnerProofCoverage::not_run())
}

/// Build coverage after the Phase 5 proof boundary has actually run. Reports
/// are validated before aggregation so duplicate entries, cross-bank mixups,
/// or malformed deserialized extents cannot inflate exact-owner coverage.
pub fn report_with_owner_proofs(
    total_rom_bytes: usize,
    db: &FactDb,
    reports: &[OwnerProofReport],
) -> Result<CoverageReport, OwnerProofCoverageError> {
    let owner_coverage = owner_proof_coverage(reports)?;
    Ok(report_base(total_rom_bytes, db, owner_coverage))
}

/// Acceptance gate for consumers that require emitter-ready ownership for
/// every assessed entry. The returned coverage still retains blocker details
/// when the caller chooses to report rather than gate an unresolved run.
pub fn require_all_owners_exact(
    reports: &[OwnerProofReport],
) -> Result<OwnerProofCoverage, OwnerProofCoverageError> {
    let coverage = owner_proof_coverage(reports)?;
    if coverage.assessed_entries == 0 {
        return Err(OwnerProofCoverageError::NoAssessments);
    }
    if coverage.candidate_owners != 0 || coverage.ambiguous_owners != 0 {
        return Err(OwnerProofCoverageError::UnresolvedOwners {
            candidates: coverage.candidate_owners,
            ambiguous: coverage.ambiguous_owners,
        });
    }
    Ok(coverage)
}

pub fn owner_proof_coverage(
    reports: &[OwnerProofReport],
) -> Result<OwnerProofCoverage, OwnerProofCoverageError> {
    let mut banks = BTreeSet::new();
    let mut entries = BTreeSet::new();
    let mut exact_owners = 0u64;
    let mut candidate_owners = 0u64;
    let mut ambiguous_owners = 0u64;
    let mut exact_owner_bytes = 0u64;
    let mut blockers = BTreeMap::<OwnerBlocker, u64>::new();

    for report in reports {
        banks.insert(report.bank.clone());
        for assessment in &report.assessments {
            let entry = assessment.entry();
            if entry.bank != report.bank {
                return Err(OwnerProofCoverageError::ReportBankMismatch {
                    report_bank: report.bank.clone(),
                    entry_bank: entry.bank.clone(),
                    entry_pc: entry.pc,
                });
            }
            if !entries.insert((entry.bank.clone(), entry.pc)) {
                return Err(OwnerProofCoverageError::DuplicateAssessment {
                    bank: entry.bank.clone(),
                    entry_pc: entry.pc,
                });
            }
            match assessment {
                OwnerAssessment::Proven { owner } => {
                    let Some(va_len) = owner.va_end.checked_sub(owner.entry.pc) else {
                        return Err(invalid_exact(owner));
                    };
                    let Some(rom_len) = owner.rom_end.checked_sub(owner.rom_start) else {
                        return Err(invalid_exact(owner));
                    };
                    if va_len == 0 || va_len != rom_len {
                        return Err(invalid_exact(owner));
                    }
                    exact_owners += 1;
                    exact_owner_bytes += u64::from(va_len);
                }
                OwnerAssessment::Candidate { frontier } => {
                    candidate_owners += 1;
                    for blocker in &frontier.blockers {
                        *blockers.entry(blocker.clone()).or_default() += 1;
                    }
                }
                OwnerAssessment::Ambiguous { frontier } => {
                    ambiguous_owners += 1;
                    for blocker in &frontier.blockers {
                        *blockers.entry(blocker.clone()).or_default() += 1;
                    }
                }
            }
        }
    }

    Ok(OwnerProofCoverage {
        state: OwnerProofRunState::Run,
        analyzed_banks: banks.len() as u64,
        assessed_entries: entries.len() as u64,
        exact_owners,
        candidate_owners,
        ambiguous_owners,
        exact_owner_bytes,
        blockers: blockers
            .into_iter()
            .map(|(blocker, assessments)| OwnerBlockerCount {
                blocker,
                assessments,
            })
            .collect(),
    })
}

fn invalid_exact(owner: &crate::owner_proof::ExactFunctionOwner) -> OwnerProofCoverageError {
    OwnerProofCoverageError::InvalidExactExtent {
        bank: owner.entry.bank.clone(),
        entry_pc: owner.entry.pc,
        va_end: owner.va_end,
        rom_start: owner.rom_start,
        rom_end: owner.rom_end,
    }
}

fn report_base(
    total_rom_bytes: usize,
    db: &FactDb,
    function_owners: OwnerProofCoverage,
) -> CoverageReport {
    let mut direct_physical = Vec::new();
    let mut logical_mappings = BTreeSet::new();
    let mut mapped_banks = BTreeSet::new();
    for fact in db.proven_rom_mappings() {
        let Fact::RomMapping {
            bank,
            rom_space,
            rom_start,
            rom_end,
            va_start,
            va_end,
        } = fact
        else {
            unreachable!()
        };
        mapped_banks.insert(bank.clone());
        logical_mappings.insert((
            bank.clone(),
            *rom_space,
            *rom_start,
            *rom_end,
            *va_start,
            *va_end,
        ));
        if *rom_space == RomAddressSpace::Physical {
            direct_physical.push((*rom_start, *rom_end));
        }
    }

    let file_backing = db
        .proven_vrom_file_mappings()
        .into_iter()
        .filter_map(|(_, fact)| match fact {
            Fact::LoadImageTableRecord {
                destination_space: MappingAddressSpace::PhysicalRom,
                destination_start,
                destination_end,
                ..
            } => Some((*destination_start, *destination_end)),
            _ => None,
        })
        .collect::<Vec<_>>();

    let mut executable_by_bank: BTreeMap<String, Vec<(u32, u32)>> = BTreeMap::new();
    for fact in db.facts() {
        let Fact::ExecutableRange {
            bank,
            va_start,
            va_end,
        } = fact
        else {
            continue;
        };
        if db
            .conclusion(&executable_range_subject(bank, *va_start, *va_end))
            .is_some_and(|conclusion| conclusion.state == ProofState::Proven)
        {
            executable_by_bank
                .entry(bank.clone())
                .or_default()
                .push((*va_start, *va_end));
        }
    }

    let mut entries = BTreeMap::new();
    let mut seen_entries = BTreeSet::new();
    for fact in db.facts() {
        let Fact::FunctionEntryClaim { target, .. } = fact else {
            continue;
        };
        if !seen_entries.insert(target.clone()) {
            continue;
        }
        if let Some(conclusion) = db.conclusion(&function_entry_subject(target)) {
            *entries.entry(conclusion.state).or_insert(0) += 1;
        }
    }

    CoverageReport {
        total_rom_bytes: total_rom_bytes as u64,
        direct_physical_load_bytes: union_len(direct_physical),
        known_file_backing_bytes: union_len(file_backing),
        logical_load_image_bytes: logical_mappings
            .iter()
            .map(|(_, _, start, end, _, _)| end.saturating_sub(*start) as u64)
            .sum(),
        executable_bytes: executable_by_bank
            .values()
            .map(|ranges| union_len(ranges.clone()))
            .sum(),
        mapped_banks: mapped_banks.len() as u64,
        executable_banks: executable_by_bank.len() as u64,
        function_entries_by_state: entries,
        function_owners,
    }
}

fn union_len(mut ranges: Vec<(u32, u32)>) -> u64 {
    ranges.retain(|(start, end)| end > start);
    ranges.sort_unstable();
    let mut total = 0u64;
    let mut current: Option<(u32, u32)> = None;
    for (start, end) in ranges {
        match current {
            None => current = Some((start, end)),
            Some((current_start, current_end)) if start <= current_end => {
                current = Some((current_start, current_end.max(end)));
            }
            Some((current_start, current_end)) => {
                total += current_end.saturating_sub(current_start) as u64;
                current = Some((start, end));
            }
        }
    }
    if let Some((start, end)) = current {
        total += end.saturating_sub(start) as u64;
    }
    total
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::facts::{BankAddr, CandidateDetector, FunctionEntryEvidence};
    use crate::owner_proof::{
        ExactFunctionOwner, OwnerAssessment, OwnerFrontier, OwnerProofReport,
    };

    #[test]
    fn coverage_keeps_physical_aliases_logical_banks_and_entries_separate() {
        let mut db = FactDb::new();
        for bank in ["a", "b"] {
            let mapping = db.insert(Fact::RomMapping {
                bank: bank.to_string(),
                rom_space: RomAddressSpace::Physical,
                rom_start: 0x100,
                rom_end: 0x200,
                va_start: 0x8000_0000,
                va_end: 0x8000_0100,
            });
            db.conclude(
                format!("bank:{bank}"),
                ProofState::Proven,
                vec![mapping],
                "test",
            )
            .unwrap();
            let executable = db.insert(Fact::ExecutableRange {
                bank: bank.to_string(),
                va_start: 0x8000_0000,
                va_end: 0x8000_0080,
            });
            db.conclude(
                executable_range_subject(bank, 0x8000_0000, 0x8000_0080),
                ProofState::Proven,
                vec![executable],
                "test",
            )
            .unwrap();
        }
        let target = BankAddr::new("a", 0x8000_0000);
        let claim = db.insert(Fact::FunctionEntryClaim {
            target: target.clone(),
            detector: CandidateDetector::JalTarget,
            evidence: FunctionEntryEvidence::DirectJal {
                call_site: BankAddr::new("a", 0x8000_0040),
            },
            proposed_state: ProofState::Candidate,
        });
        db.conclude(
            function_entry_subject(&target),
            ProofState::Candidate,
            vec![claim],
            "test",
        )
        .unwrap();

        let report = report(0x1000, &db);
        assert_eq!(report.direct_physical_load_bytes, 0x100);
        assert_eq!(report.logical_load_image_bytes, 0x200);
        assert_eq!(report.executable_bytes, 0x100);
        assert_eq!(report.mapped_banks, 2);
        assert_eq!(report.function_entries_by_state[&ProofState::Candidate], 1);
        assert_eq!(report.function_owners.state, OwnerProofRunState::NotRun);
        assert_eq!(
            serde_json::to_value(&report).unwrap()["function_owners"]["state"],
            "not_run"
        );
    }

    fn exact(entry_pc: u32, byte_len: u32) -> OwnerAssessment {
        OwnerAssessment::Proven {
            owner: ExactFunctionOwner {
                entry: BankAddr::new("bank", entry_pc),
                va_end: entry_pc + byte_len,
                rom_space: RomAddressSpace::Physical,
                rom_start: 0x1000 + entry_pc - 0x8000_0000,
                rom_end: 0x1000 + entry_pc - 0x8000_0000 + byte_len,
                block_starts: vec![entry_pc],
            },
        }
    }

    #[test]
    fn owner_coverage_counts_only_exact_extents_and_retains_blockers() {
        let reports = [OwnerProofReport {
            bank: "bank".into(),
            assessments: vec![
                exact(0x8000_0000, 12),
                OwnerAssessment::Candidate {
                    frontier: OwnerFrontier {
                        entry: BankAddr::new("bank", 0x8000_0010),
                        proposed_va_end: Some(0x8000_0020),
                        blockers: vec![OwnerBlocker::BankNotProven],
                    },
                },
                OwnerAssessment::Ambiguous {
                    frontier: OwnerFrontier {
                        entry: BankAddr::new("bank", 0x8000_0020),
                        proposed_va_end: Some(0x8000_0030),
                        blockers: vec![OwnerBlocker::PartitionOverlap {
                            other_root: 0x8000_0010,
                        }],
                    },
                },
            ],
        }];

        let coverage = owner_proof_coverage(&reports).unwrap();
        assert_eq!(coverage.state, OwnerProofRunState::Run);
        assert_eq!(coverage.assessed_entries, 3);
        assert_eq!(coverage.exact_owners, 1);
        assert_eq!(coverage.candidate_owners, 1);
        assert_eq!(coverage.ambiguous_owners, 1);
        assert_eq!(coverage.exact_owner_bytes, 12);
        assert_eq!(coverage.blockers.len(), 2);
        let report = report_with_owner_proofs(0x1000, &FactDb::new(), &reports).unwrap();
        assert_eq!(report.function_owners, coverage);
        assert_eq!(
            serde_json::to_value(&report).unwrap()["function_owners"]["blockers"]
                .as_array()
                .unwrap()
                .len(),
            2
        );
        assert_eq!(
            require_all_owners_exact(&reports),
            Err(OwnerProofCoverageError::UnresolvedOwners {
                candidates: 1,
                ambiguous: 1,
            })
        );
    }

    #[test]
    fn exact_owner_gate_rejects_empty_and_duplicate_inputs() {
        assert_eq!(
            require_all_owners_exact(&[]),
            Err(OwnerProofCoverageError::NoAssessments)
        );

        let reports = [
            OwnerProofReport {
                bank: "bank".into(),
                assessments: vec![exact(0x8000_0000, 8)],
            },
            OwnerProofReport {
                bank: "bank".into(),
                assessments: vec![exact(0x8000_0000, 8)],
            },
        ];
        assert_eq!(
            require_all_owners_exact(&reports),
            Err(OwnerProofCoverageError::DuplicateAssessment {
                bank: "bank".into(),
                entry_pc: 0x8000_0000,
            })
        );
    }

    #[test]
    fn exact_owner_gate_accepts_valid_proven_extents() {
        let reports = [OwnerProofReport {
            bank: "bank".into(),
            assessments: vec![exact(0x8000_0000, 8), exact(0x8000_0008, 12)],
        }];
        let coverage = require_all_owners_exact(&reports).unwrap();
        assert_eq!(coverage.exact_owners, 2);
        assert_eq!(coverage.exact_owner_bytes, 20);
        assert!(coverage.blockers.is_empty());
    }
}
