//! Coverage metrics kept separate from function-entry precision/recall.
//! Physical ROM coverage, logical load-image coverage, executable coverage,
//! and function-entry proof states answer different questions and must never
//! be collapsed into one percentage.

use crate::block_pack::BlockPackV1;
use crate::facts::{
    executable_range_subject, function_entry_subject, BankBackingSpanV1, BankBackingV1, Fact,
    FactDb, MappingAddressSpace, ProofState, RomAddressSpace,
};
use crate::owner_proof::{OwnerAssessment, OwnerBlocker, OwnerProofReport};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
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
        backing: BankBackingSpanV1,
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
                backing,
            } => write!(
                f,
                "exact owner {bank}:0x{entry_pc:08x} has invalid VA/backing extents ending at 0x{va_end:08x} and {backing:?}"
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
                    let Some(backing_len) = backing_span_len(&owner.backing) else {
                        return Err(invalid_exact(owner));
                    };
                    if va_len == 0 || va_len != backing_len {
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
        backing: owner.backing.clone(),
    }
}

fn backing_span_len(backing: &BankBackingSpanV1) -> Option<u32> {
    match backing {
        BankBackingSpanV1::RomAffine {
            rom_start, rom_end, ..
        } => rom_end.checked_sub(*rom_start),
        BankBackingSpanV1::Materialized {
            output_start,
            output_end,
            ..
        } => output_end.checked_sub(*output_start),
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
    for image in db.proven_bank_images() {
        mapped_banks.insert(image.bank.clone());
        if let BankBackingV1::RomAffine {
            rom_space: RomAddressSpace::Physical,
            rom_start,
            rom_end,
        } = &image.backing
        {
            direct_physical.push((*rom_start, *rom_end));
        }
        logical_mappings.insert(image);
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
            .map(|image| image.va_end.saturating_sub(image.va_start) as u64)
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

/// Deterministic summary of a [`BlockPackV1`]: how many independent blocks it
/// carries, how many aligned instruction words those blocks cover, and a
/// content digest that folds every block identity and byte hash. The digest is
/// derived only from pack contents (never a timestamp or ordering accident),
/// so two byte-identical packs produce the same digest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackCoverage {
    /// Hash-wire schema. V3 replaces flat ROM coordinates with typed backing.
    pub digest_schema_version: u32,
    /// The independently versioned BlockPack wire schema summarized here.
    pub schema_version: u32,
    pub banks: u64,
    pub blocks: u64,
    /// Aligned 32-bit instruction words across all packed blocks. Each block's
    /// word count is the typed backing span length divided by four; block
    /// geometry is validated four-byte-aligned when the pack is emitted.
    pub words: u64,
    pub digest: String,
}

pub const PACK_COVERAGE_DIGEST_SCHEMA_V3: u32 = 3;

/// Fold a pack into a deterministic coverage summary. The digest commits to the
/// schema version, the normalized ROM identity, and every packed block's
/// `(bank, bank_id, start_va, end_va, backing, bytes_sha256, terminator)`.
/// Banks and blocks are already emitted in stable order by the BlockPack
/// emitters; this reads them in that order.
pub fn pack_coverage(pack: &BlockPackV1) -> PackCoverage {
    let mut hasher = Sha256::new();
    hasher.update(b"fn64:pack-coverage:v3\n");
    hasher.update(pack.schema_version.to_be_bytes());
    hasher.update(pack.normalized_rom_sha256.as_bytes());
    hasher.update(b"\n");
    let mut blocks = 0u64;
    let mut words = 0u64;
    for bank in &pack.banks {
        hasher.update(b"bank\n");
        hasher.update(bank.bank.as_bytes());
        hasher.update(b"\n");
        hasher.update(bank.bank_id.to_be_bytes());
        for block in &bank.blocks {
            blocks += 1;
            words += u64::from(backing_span_len(&block.backing).unwrap_or(0)) / 4;
            hasher.update(block.start_va.to_be_bytes());
            hasher.update(block.end_va.to_be_bytes());
            match &block.backing {
                BankBackingSpanV1::RomAffine {
                    rom_space,
                    rom_start,
                    rom_end,
                } => {
                    hasher.update([0]);
                    hasher.update([match rom_space {
                        RomAddressSpace::Physical => 0,
                        RomAddressSpace::Virtual => 1,
                    }]);
                    hasher.update(rom_start.to_be_bytes());
                    hasher.update(rom_end.to_be_bytes());
                }
                BankBackingSpanV1::Materialized {
                    receipt_sha256,
                    output_start,
                    output_end,
                } => {
                    hasher.update([1]);
                    hasher.update((receipt_sha256.len() as u64).to_be_bytes());
                    hasher.update(receipt_sha256.as_bytes());
                    hasher.update(output_start.to_be_bytes());
                    hasher.update(output_end.to_be_bytes());
                }
            }
            hasher.update(block.bytes_sha256.as_bytes());
            hasher.update(b"\n");
            hasher.update(format!("{:?}", block.terminator).as_bytes());
            hasher.update(b"\n");
        }
    }
    PackCoverage {
        digest_schema_version: PACK_COVERAGE_DIGEST_SCHEMA_V3,
        schema_version: pack.schema_version,
        banks: pack.banks.len() as u64,
        blocks,
        words,
        digest: hasher
            .finalize()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect(),
    }
}

/// Identity of the ROM a coverage report was produced from, rendered verbatim
/// so a report line is self-describing without any global state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RomIdentity {
    pub label: String,
    pub sha256: String,
}

/// Render a coverage report as deterministic text lines. Ordering is fixed,
/// every quantity is an integer printed in base-10 or fixed-width hex, and no
/// timestamps, floats, or locale-sensitive formatting appear. Feeding the same
/// report and pack coverage always yields the same `Vec<String>`.
///
/// The metric ladder (docs/DISCOVER-PLAN.md) is deliberately kept as separate
/// lines: physical, logical, executable, owner, entry-conclusion, and pack
/// coverage answer different questions and must never be collapsed into a
/// single percentage. Measured coverage is not proof: a mapped or executable
/// byte count reports what evidence established, not that the interval is
/// authoritative for emission.
pub fn render_report(
    identity: &RomIdentity,
    phase: &str,
    report: &CoverageReport,
    pack: Option<&PackCoverage>,
) -> Vec<String> {
    let mut lines = Vec::new();
    lines.push(format!("rom {} sha256={}", identity.label, identity.sha256));
    lines.push(format!("phase {phase}"));
    lines.push(format!("physical_rom_bytes {}", report.total_rom_bytes));
    lines.push(format!(
        "physical_assigned_bytes direct_load={} known_file={}",
        report.direct_physical_load_bytes, report.known_file_backing_bytes
    ));
    lines.push(format!(
        "logical_load_image_bytes {} mapped_banks={}",
        report.logical_load_image_bytes, report.mapped_banks
    ));
    lines.push(format!(
        "executable_bytes {} executable_banks={}",
        report.executable_bytes, report.executable_banks
    ));

    // Entry-conclusion counts across every proof state, always printed for
    // every state so an absent state reads as an explicit zero, not an omission.
    let entries = &report.function_entries_by_state;
    let state_count = |state: ProofState| entries.get(&state).copied().unwrap_or(0);
    lines.push(format!(
        "entry_conclusions open={} candidate={} supported={} rejected={} conflict={} proven={}",
        state_count(ProofState::Open),
        state_count(ProofState::Candidate),
        state_count(ProofState::Supported),
        state_count(ProofState::Rejected),
        state_count(ProofState::Conflict),
        state_count(ProofState::Proven),
    ));

    let owners = &report.function_owners;
    match owners.state {
        OwnerProofRunState::NotRun => {
            lines.push("owner_proof not_run".to_string());
        }
        OwnerProofRunState::Run => {
            lines.push(format!(
                "owner_proof run banks={} assessed={} exact={} candidate={} ambiguous={} exact_bytes={}",
                owners.analyzed_banks,
                owners.assessed_entries,
                owners.exact_owners,
                owners.candidate_owners,
                owners.ambiguous_owners,
                owners.exact_owner_bytes,
            ));
            for blocker in &owners.blockers {
                lines.push(format!(
                    "owner_blocker {} {:?}",
                    blocker.assessments, blocker.blocker
                ));
            }
        }
    }

    match pack {
        None => lines.push("pack none".to_string()),
        Some(pack) => lines.push(format!(
            "pack schema={} digest_schema={} banks={} blocks={} words={} digest={}",
            pack.schema_version,
            pack.digest_schema_version,
            pack.banks,
            pack.blocks,
            pack.words,
            pack.digest
        )),
    }

    lines
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::block_pack::{BlockPackV1, PackedBankV1, PackedBlockV1};
    use crate::cfg::BlockTerminator;
    use crate::facts::{
        BankAddr, CandidateDetector, FunctionEntryEvidence, MaterializationEvaluatorV1,
        MaterializedImageSourceV1, MaterializedImageSuffixV1,
    };
    use crate::owner_proof::{
        ExactFunctionOwner, OwnerAssessment, OwnerFrontier, OwnerProofReport,
    };

    #[test]
    fn coverage_keeps_physical_aliases_logical_banks_and_entries_separate() {
        let mut db = FactDb::new();
        for bank in ["a", "b"] {
            let mapping = db.insert(Fact::RomMapping {
                bank: bank.to_string(),
                rom_space: if bank == "a" {
                    RomAddressSpace::Physical
                } else {
                    RomAddressSpace::Virtual
                },
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
        let evaluated = db.insert(Fact::EvaluatedImage {
            bank: "materialized".into(),
            va_start: 0x8010_0000,
            va_end: 0x8010_0040,
            receipt: crate::facts::EvaluatedImageReceiptV1 {
                evaluator: MaterializationEvaluatorV1::HeaderedRawDeflateSequenceV1 {
                    stream_count: 0,
                },
                source: MaterializedImageSourceV1 {
                    rom_space: RomAddressSpace::Physical,
                    rom_start: 0x400,
                    rom_end: 0x440,
                    cursor: 0,
                },
                source_sha256: "1".repeat(64),
                output_len: 0x40,
                output_sha256: "2".repeat(64),
                streams: Vec::new(),
                trailing_suffix: MaterializedImageSuffixV1 {
                    offset: 0,
                    len: 0x40,
                    sha256: "3".repeat(64),
                },
            },
        });
        db.conclude(
            "bank:materialized",
            ProofState::Proven,
            vec![evaluated],
            "test",
        )
        .unwrap();
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
        assert_eq!(report.logical_load_image_bytes, 0x240);
        assert_eq!(report.executable_bytes, 0x100);
        assert_eq!(report.mapped_banks, 3);
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
                backing: BankBackingSpanV1::RomAffine {
                    rom_space: RomAddressSpace::Physical,
                    rom_start: 0x1000 + entry_pc - 0x8000_0000,
                    rom_end: 0x1000 + entry_pc - 0x8000_0000 + byte_len,
                },
                block_starts: vec![entry_pc],
            },
        }
    }

    fn exact_materialized(entry_pc: u32, byte_len: u32) -> OwnerAssessment {
        OwnerAssessment::Proven {
            owner: ExactFunctionOwner {
                entry: BankAddr::new("bank", entry_pc),
                va_end: entry_pc + byte_len,
                backing: BankBackingSpanV1::Materialized {
                    receipt_sha256: "f".repeat(64),
                    output_start: entry_pc - 0x8000_0000,
                    output_end: entry_pc - 0x8000_0000 + byte_len,
                },
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
            assessments: vec![exact(0x8000_0000, 8), exact_materialized(0x8000_0008, 12)],
        }];
        let coverage = require_all_owners_exact(&reports).unwrap();
        assert_eq!(coverage.exact_owners, 2);
        assert_eq!(coverage.exact_owner_bytes, 20);
        assert!(coverage.blockers.is_empty());
    }

    fn synthetic_report() -> CoverageReport {
        let mut entries = BTreeMap::new();
        entries.insert(ProofState::Proven, 7);
        entries.insert(ProofState::Candidate, 3);
        CoverageReport {
            total_rom_bytes: 0x0080_0000,
            direct_physical_load_bytes: 0x0010_0000,
            known_file_backing_bytes: 0x0020_0000,
            logical_load_image_bytes: 0x0030_0000,
            executable_bytes: 0x0004_0000,
            mapped_banks: 4,
            executable_banks: 2,
            function_entries_by_state: entries,
            function_owners: OwnerProofCoverage::not_run(),
        }
    }

    fn synthetic_pack() -> BlockPackV1 {
        BlockPackV1 {
            schema_version: 1,
            normalized_rom_sha256: "a".repeat(64),
            banks: vec![PackedBankV1 {
                bank: "boot".into(),
                bank_id: 0xdead_beef,
                blocks: vec![
                    PackedBlockV1 {
                        start_va: 0x8000_0000,
                        end_va: 0x8000_0010,
                        backing: BankBackingSpanV1::RomAffine {
                            rom_space: RomAddressSpace::Physical,
                            rom_start: 0x1000,
                            rom_end: 0x1010,
                        },
                        bytes_sha256: "b".repeat(64),
                        terminator: BlockTerminator::Return,
                    },
                    PackedBlockV1 {
                        start_va: 0x8000_0010,
                        end_va: 0x8000_0020,
                        backing: BankBackingSpanV1::Materialized {
                            receipt_sha256: "d".repeat(64),
                            output_start: 0x10,
                            output_end: 0x20,
                        },
                        bytes_sha256: "c".repeat(64),
                        terminator: BlockTerminator::Fallthrough { next: 0x8000_0020 },
                    },
                ],
            }],
        }
    }

    #[test]
    fn render_emits_exact_stable_lines_without_pack() {
        let report = synthetic_report();
        let identity = RomIdentity {
            label: "SYNTH".into(),
            sha256: "d".repeat(64),
        };
        let lines = render_report(&identity, "phase-3-harvest", &report, None);
        assert_eq!(
            lines,
            vec![
                format!("rom SYNTH sha256={}", "d".repeat(64)),
                "phase phase-3-harvest".to_string(),
                "physical_rom_bytes 8388608".to_string(),
                "physical_assigned_bytes direct_load=1048576 known_file=2097152".to_string(),
                "logical_load_image_bytes 3145728 mapped_banks=4".to_string(),
                "executable_bytes 262144 executable_banks=2".to_string(),
                "entry_conclusions open=0 candidate=3 supported=0 rejected=0 conflict=0 proven=7"
                    .to_string(),
                "owner_proof not_run".to_string(),
                "pack none".to_string(),
            ]
        );
    }

    #[test]
    fn render_emits_owner_and_pack_lines_when_present() {
        let mut report = synthetic_report();
        report.function_owners = OwnerProofCoverage {
            state: OwnerProofRunState::Run,
            analyzed_banks: 1,
            assessed_entries: 3,
            exact_owners: 1,
            candidate_owners: 1,
            ambiguous_owners: 1,
            exact_owner_bytes: 12,
            blockers: vec![
                OwnerBlockerCount {
                    blocker: OwnerBlocker::BankNotProven,
                    assessments: 2,
                },
                OwnerBlockerCount {
                    blocker: OwnerBlocker::PartitionOverlap { other_root: 0x40 },
                    assessments: 1,
                },
            ],
        };
        let identity = RomIdentity {
            label: "SYNTH".into(),
            sha256: "d".repeat(64),
        };
        let pack = pack_coverage(&synthetic_pack());
        let lines = render_report(&identity, "phase-5-owner", &report, Some(&pack));
        assert_eq!(
            lines[7],
            "owner_proof run banks=1 assessed=3 exact=1 candidate=1 ambiguous=1 exact_bytes=12"
        );
        assert_eq!(lines[8], "owner_blocker 2 BankNotProven");
        assert_eq!(
            lines[9],
            "owner_blocker 1 PartitionOverlap { other_root: 64 }"
        );
        assert_eq!(
            lines[10],
            format!(
                "pack schema=1 digest_schema=3 banks=1 blocks=2 words=8 digest={}",
                pack.digest
            )
        );
    }

    #[test]
    fn pack_coverage_counts_words_and_is_digest_stable() {
        let pack = synthetic_pack();
        let a = pack_coverage(&pack);
        let b = pack_coverage(&pack);
        assert_eq!(a, b);
        assert_eq!(a.banks, 1);
        assert_eq!(a.blocks, 2);
        assert_eq!(a.words, 8);
        assert_eq!(a.digest.len(), 64);

        // A changed block byte hash must move the digest.
        let mut mutated = pack;
        mutated.banks[0].blocks[0].bytes_sha256 = "e".repeat(64);
        assert_ne!(pack_coverage(&mutated).digest, a.digest);

        let mut backing_changed = synthetic_pack();
        backing_changed.banks[0].blocks[0].backing = BankBackingSpanV1::RomAffine {
            rom_space: RomAddressSpace::Virtual,
            rom_start: 0x1000,
            rom_end: 0x1010,
        };
        assert_ne!(pack_coverage(&backing_changed).digest, a.digest);

        let mut receipt_changed = synthetic_pack();
        let BankBackingSpanV1::Materialized { receipt_sha256, .. } =
            &mut receipt_changed.banks[0].blocks[1].backing
        else {
            panic!("synthetic second block must be materialized")
        };
        *receipt_sha256 = "e".repeat(64);
        assert_ne!(pack_coverage(&receipt_changed).digest, a.digest);
        assert_eq!(a.digest_schema_version, PACK_COVERAGE_DIGEST_SCHEMA_V3);
    }
}
