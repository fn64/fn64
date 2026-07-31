//! Grading-only precision/recall cross-check for Phase 3 candidates.
//!
//! The answer key is parsed only by this module and is never reachable from
//! any detector. Candidate identity is translated through discovery's own
//! `RomMapping` facts into `(ROM offset, runtime VA)` before comparison, so
//! overlapping overlay VAs cannot earn false matches by address alone.

use crate::facts::{
    function_entry_subject, BankAddr, CandidateDetector, Fact, FactDb, FunctionEntryEvidence,
    ProofState, RomAddressSpace,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};

pub const SCOPED_CANDIDATE_IDENTITY_SCHEMA_V1: u32 = 1;
pub const SCOPED_CANDIDATE_IDENTITY_SCHEMA_V2: u32 = 2;
pub const SCOPED_CANDIDATE_IDENTITY_SCHEMA_V3: u32 = 3;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct PhysicalEntry {
    pub rom: u32,
    pub vram: u32,
}

/// Physical source identities retained for one detector candidate.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CandidatePhysicalProvenanceV1 {
    pub candidate: PhysicalEntry,
    pub sources: Vec<PhysicalEntry>,
}

/// Canonical physical identities produced by one detector in a scoped run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DetectorCandidateIdentitiesV1 {
    pub detector: CandidateDetector,
    pub candidates: Vec<PhysicalEntry>,
    pub provenance: Vec<CandidatePhysicalProvenanceV1>,
    /// Bank-qualified identities that could not translate through exactly one
    /// scoped mapping. They remain explicit rather than disappearing from an
    /// otherwise equal digest.
    pub ungradable: Vec<BankAddr>,
}

/// Answer-key-independent candidate identity receipt.
///
/// All collections are sorted before construction, so serialization and the
/// digest are canonical for a fixed fact database and bank scope.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScopedCandidateIdentitiesV1 {
    pub schema_version: u32,
    pub per_detector: Vec<DetectorCandidateIdentitiesV1>,
    pub combined_candidates: Vec<PhysicalEntry>,
    pub combined_ungradable: Vec<BankAddr>,
}

impl ScopedCandidateIdentitiesV1 {
    pub fn digest_sha256(&self) -> String {
        let bytes = serde_json::to_vec(self).expect("candidate identity receipt serializes");
        format!("{:x}", Sha256::digest(bytes))
    }
}

/// ROM-address-space-qualified candidate identity.
///
/// Physical cartridge offsets and virtual-ROM offsets are different domains,
/// even when their numeric offsets and runtime VAs happen to match. V1 is
/// retained for compatibility; new diagnostic consumers should use V2.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct AddressedPhysicalEntryV2 {
    pub rom_space: RomAddressSpace,
    pub rom: u32,
    pub vram: u32,
}

/// Address-space-qualified source identities retained for one candidate.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CandidatePhysicalProvenanceV2 {
    pub candidate: AddressedPhysicalEntryV2,
    pub sources: Vec<AddressedPhysicalEntryV2>,
}

/// Canonical V2 identities produced by one detector in a scoped run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DetectorCandidateIdentitiesV2 {
    pub detector: CandidateDetector,
    pub candidates: Vec<AddressedPhysicalEntryV2>,
    pub provenance: Vec<CandidatePhysicalProvenanceV2>,
    pub ungradable: Vec<BankAddr>,
}

/// Answer-key-independent, address-space-safe candidate identity receipt.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScopedCandidateIdentitiesV2 {
    pub schema_version: u32,
    pub per_detector: Vec<DetectorCandidateIdentitiesV2>,
    pub combined_candidates: Vec<AddressedPhysicalEntryV2>,
    pub combined_ungradable: Vec<BankAddr>,
}

impl ScopedCandidateIdentitiesV2 {
    pub fn digest_sha256(&self) -> String {
        let bytes = serde_json::to_vec(self).expect("candidate identity receipt serializes");
        format!("{:x}", Sha256::digest(bytes))
    }
}

/// V3 keeps the address-space-qualified V2 row geometry and adds the
/// composition-derived semantic-callable detector to the closed denominator.
pub type ScopedCandidateIdentitiesV3 = ScopedCandidateIdentitiesV2;

#[derive(Debug, Clone)]
pub struct CandidateAnswerKey {
    pub entries: BTreeSet<PhysicalEntry>,
    pub function_count: usize,
    pub section_count: usize,
    multiplicity: BTreeMap<PhysicalEntry, usize>,
    extents: Vec<FunctionExtent>,
}

#[derive(Debug, Clone, Copy)]
struct FunctionExtent {
    rom_start: u32,
    rom_end: u32,
    vram_start: u32,
    vram_end: u32,
}

#[derive(Debug, Deserialize)]
struct SymbolsDoc {
    #[serde(default)]
    section: Vec<SectionDoc>,
}

#[derive(Debug, Deserialize)]
struct SectionDoc {
    name: String,
    rom: u32,
    vram: u32,
    size: u32,
    #[serde(default)]
    functions: Vec<FunctionDoc>,
}

#[derive(Debug, Deserialize)]
struct FunctionDoc {
    name: String,
    vram: u32,
    size: u32,
}

/// Parse the zeldaret-derived `dump.toml` used by the existing OoT recompile
/// loader. Malformed extents fail loudly; silently skipping a key row would
/// inflate recall.
pub fn parse_symbol_dump(text: &str) -> Result<CandidateAnswerKey, String> {
    let doc: SymbolsDoc = toml::from_str(text).map_err(|error| error.to_string())?;
    let section_count = doc.section.len();
    let mut entries = BTreeSet::new();
    let mut multiplicity = BTreeMap::new();
    let mut function_count = 0usize;
    let mut extents = Vec::new();

    for section in doc.section {
        for function in section.functions {
            function_count += 1;
            let offset = function.vram.checked_sub(section.vram).ok_or_else(|| {
                format!(
                    "function {:?} at 0x{:08x} precedes section {:?} at 0x{:08x}",
                    function.name, function.vram, section.name, section.vram
                )
            })?;
            let function_end = offset
                .checked_add(function.size)
                .ok_or_else(|| format!("function {:?} extent overflows u32", function.name))?;
            if function_end > section.size {
                return Err(format!(
                    "function {:?} extent [0x{:x},0x{:x}) lies outside section {:?} size 0x{:x}",
                    function.name, offset, function_end, section.name, section.size
                ));
            }
            let rom = section
                .rom
                .checked_add(offset)
                .ok_or_else(|| format!("function {:?} ROM address overflows u32", function.name))?;
            let entry = PhysicalEntry {
                rom,
                vram: function.vram,
            };
            entries.insert(entry);
            *multiplicity.entry(entry).or_insert(0) += 1;
            extents.push(FunctionExtent {
                rom_start: rom,
                rom_end: rom.checked_add(function.size).ok_or_else(|| {
                    format!("function {:?} ROM extent overflows u32", function.name)
                })?,
                vram_start: function.vram,
                vram_end: function.vram.checked_add(function.size).ok_or_else(|| {
                    format!("function {:?} VRAM extent overflows u32", function.name)
                })?,
            });
        }
    }

    Ok(CandidateAnswerKey {
        entries,
        function_count,
        section_count,
        multiplicity,
        extents,
    })
}

#[derive(Debug, Clone)]
pub struct PrecisionRecall {
    pub candidates: usize,
    pub true_positives: usize,
    pub false_positives: usize,
    /// Answer-key function rows recalled. This may exceed
    /// `true_positives` when aliases share one physical entry.
    pub recalled_functions: usize,
    pub false_negatives: usize,
}

impl PrecisionRecall {
    pub fn precision(&self) -> f64 {
        if self.candidates == 0 {
            0.0
        } else {
            self.true_positives as f64 / self.candidates as f64
        }
    }

    pub fn recall(&self) -> f64 {
        let total = self.recalled_functions + self.false_negatives;
        if total == 0 {
            0.0
        } else {
            self.recalled_functions as f64 / total as f64
        }
    }
}

#[derive(Debug, Clone)]
pub struct DetectorGrade {
    pub detector: CandidateDetector,
    pub metrics: PrecisionRecall,
    /// Positive claims that could not be translated through exactly one
    /// discovered bank mapping. They count neither as matches nor candidates;
    /// the count is explicit so a broken mapping cannot hide in the grade.
    pub ungradable: usize,
    pub false_positive_breakdown: FalsePositiveBreakdown,
}

/// Grading-only explanation of false candidates. These categories never feed
/// discovery; they identify whether the target was an interior label/gap and
/// whether any detector source instruction lies in an answer-key function.
#[derive(Debug, Clone, Default)]
pub struct FalsePositiveBreakdown {
    pub target_interior: usize,
    pub target_outside_functions: usize,
    pub source_inside_function: usize,
    pub source_outside_functions: usize,
    pub samples: Vec<FalsePositiveSample>,
}

#[derive(Debug, Clone)]
pub struct FalsePositiveSample {
    pub target: PhysicalEntry,
    pub sources: Vec<PhysicalEntry>,
}

#[derive(Debug, Clone)]
pub struct CandidateGradeReport {
    pub answer_key_total: usize,
    pub per_detector: Vec<DetectorGrade>,
    pub combined: PrecisionRecall,
    pub combined_ungradable: usize,
}

#[derive(Debug, Clone, Copy)]
struct Mapping {
    rom_space: RomAddressSpace,
    rom_start: u32,
    rom_end: u32,
    va_start: u32,
    va_end: u32,
}

/// Grade every positive provider independently and the merged non-conflict
/// candidate set as a union. Answer-key rows never influence the candidates.
pub fn grade_candidates(db: &FactDb, key: &CandidateAnswerKey) -> CandidateGradeReport {
    grade_candidates_scoped(db, key, |_| true)
}

/// Grade only claims and mappings belonging to the selected banks.
///
/// This keeps comparative gates honest when a full discovery composition
/// contains additional, independently proven load images. A claim is included
/// only when its target and every bank named by its typed evidence are in
/// scope; excluded banks cannot contribute cross-bank candidates, ungradable
/// claims, or source attribution.
pub fn grade_candidates_scoped(
    db: &FactDb,
    key: &CandidateAnswerKey,
    include_bank: impl Fn(&str) -> bool,
) -> CandidateGradeReport {
    let identities = scoped_candidate_identities_v1(db, include_bank);
    let per_detector = identities
        .per_detector
        .iter()
        .map(|row| {
            let candidates: BTreeSet<_> = row.candidates.iter().copied().collect();
            let sources: BTreeMap<_, _> = row
                .provenance
                .iter()
                .map(|provenance| {
                    (
                        provenance.candidate,
                        provenance.sources.iter().copied().collect(),
                    )
                })
                .collect();
            DetectorGrade {
                detector: row.detector,
                metrics: metrics(&candidates, key),
                ungradable: row.ungradable.len(),
                false_positive_breakdown: false_positive_breakdown(
                    &candidates,
                    Some(&sources),
                    key,
                ),
            }
        })
        .collect();
    let combined_candidates = identities.combined_candidates.iter().copied().collect();

    CandidateGradeReport {
        answer_key_total: key.function_count,
        per_detector,
        combined: metrics(&combined_candidates, key),
        combined_ungradable: identities.combined_ungradable.len(),
    }
}

/// Build a canonical scoped candidate receipt without consulting an answer
/// key. This is the pre-grading equality oracle for A/B experiments.
pub fn scoped_candidate_identities_v1(
    db: &FactDb,
    include_bank: impl Fn(&str) -> bool,
) -> ScopedCandidateIdentitiesV1 {
    let mappings = mappings_by_bank(db, &include_bank);
    let mut per_detector_candidates: BTreeMap<CandidateDetector, BTreeSet<PhysicalEntry>> =
        BTreeMap::from([
            (CandidateDetector::JalTarget, BTreeSet::new()),
            (CandidateDetector::IndirectCallTarget, BTreeSet::new()),
            (CandidateDetector::ProloguePattern, BTreeSet::new()),
            (CandidateDetector::TableDerived, BTreeSet::new()),
        ]);
    let mut per_detector_ungradable: BTreeMap<CandidateDetector, BTreeSet<BankAddr>> =
        BTreeMap::new();
    let mut per_detector_sources: BTreeMap<
        CandidateDetector,
        BTreeMap<PhysicalEntry, BTreeSet<PhysicalEntry>>,
    > = BTreeMap::new();
    let mut combined_targets: BTreeSet<BankAddr> = BTreeSet::new();

    for fact in db.facts() {
        let Fact::FunctionEntryClaim {
            target,
            detector,
            evidence,
            proposed_state,
        } = fact
        else {
            continue;
        };
        if *detector == CandidateDetector::SemanticCallableArgument {
            continue;
        }
        if !include_bank(&target.bank) || !evidence_in_scope(evidence, &include_bank) {
            continue;
        }
        if !is_positive(*proposed_state) {
            continue;
        }
        per_detector_candidates.entry(*detector).or_default();
        match translate(target.bank.as_str(), target.pc, &mappings) {
            Some(entry) => {
                per_detector_candidates
                    .entry(*detector)
                    .or_default()
                    .insert(entry);
                let source = match fact {
                    Fact::FunctionEntryClaim {
                        evidence:
                            FunctionEntryEvidence::DirectJal { call_site }
                            | FunctionEntryEvidence::ResolvedJalr { call_site, .. }
                            | FunctionEntryEvidence::ExhaustiveIndirectCall { call_site, .. }
                            | FunctionEntryEvidence::SemanticCallableArgument { call_site, .. },
                        ..
                    } => translate(&call_site.bank, call_site.pc, &mappings),
                    Fact::FunctionEntryClaim {
                        evidence: FunctionEntryEvidence::HandlerTablePointer { source_slot, .. },
                        ..
                    } => translate(&source_slot.bank, source_slot.pc, &mappings),
                    _ => None,
                };
                if let Some(source) = source {
                    per_detector_sources
                        .entry(*detector)
                        .or_default()
                        .entry(entry)
                        .or_default()
                        .insert(source);
                }
            }
            None => {
                per_detector_ungradable
                    .entry(*detector)
                    .or_default()
                    .insert(target.clone());
            }
        }
        if db
            .conclusion(&function_entry_subject(target))
            .is_some_and(|conclusion| is_positive(conclusion.state))
        {
            combined_targets.insert(target.clone());
        }
    }

    let per_detector = per_detector_candidates
        .into_iter()
        .map(|(detector, candidates)| {
            let provenance = per_detector_sources
                .remove(&detector)
                .unwrap_or_default()
                .into_iter()
                .map(|(candidate, sources)| CandidatePhysicalProvenanceV1 {
                    candidate,
                    sources: sources.into_iter().collect(),
                })
                .collect();
            DetectorCandidateIdentitiesV1 {
                detector,
                candidates: candidates.into_iter().collect(),
                provenance,
                ungradable: per_detector_ungradable
                    .remove(&detector)
                    .unwrap_or_default()
                    .into_iter()
                    .collect(),
            }
        })
        .collect();

    let mut combined_candidates = BTreeSet::new();
    let mut combined_ungradable = BTreeSet::new();
    for target in combined_targets {
        match translate(&target.bank, target.pc, &mappings) {
            Some(entry) => {
                combined_candidates.insert(entry);
            }
            None => {
                combined_ungradable.insert(target);
            }
        }
    }

    ScopedCandidateIdentitiesV1 {
        schema_version: SCOPED_CANDIDATE_IDENTITY_SCHEMA_V1,
        per_detector,
        combined_candidates: combined_candidates.into_iter().collect(),
        combined_ungradable: combined_ungradable.into_iter().collect(),
    }
}

/// Build the canonical scoped V2 receipt without consulting an answer key.
///
/// This mirrors [`scoped_candidate_identities_v1`] while retaining the ROM
/// coordinate system in every translated target and provenance identity.
pub fn scoped_candidate_identities_v2(
    db: &FactDb,
    include_bank: impl Fn(&str) -> bool,
) -> ScopedCandidateIdentitiesV2 {
    scoped_candidate_identities_addressed(
        db,
        include_bank,
        SCOPED_CANDIDATE_IDENTITY_SCHEMA_V2,
        false,
    )
}

/// Build the canonical V3 receipt, including typed semantic callable-entry
/// authority rederived during byte-verified snapshot composition.
pub fn scoped_candidate_identities_v3(
    db: &FactDb,
    include_bank: impl Fn(&str) -> bool,
) -> ScopedCandidateIdentitiesV3 {
    scoped_candidate_identities_addressed(
        db,
        include_bank,
        SCOPED_CANDIDATE_IDENTITY_SCHEMA_V3,
        true,
    )
}

fn scoped_candidate_identities_addressed(
    db: &FactDb,
    include_bank: impl Fn(&str) -> bool,
    schema_version: u32,
    include_semantic: bool,
) -> ScopedCandidateIdentitiesV2 {
    let mappings = mappings_by_bank(db, &include_bank);
    let mut per_detector_candidates: BTreeMap<
        CandidateDetector,
        BTreeSet<AddressedPhysicalEntryV2>,
    > = BTreeMap::from([
        (CandidateDetector::HardwareEntrypoint, BTreeSet::new()),
        (CandidateDetector::JalTarget, BTreeSet::new()),
        (CandidateDetector::IndirectCallTarget, BTreeSet::new()),
        (CandidateDetector::ProloguePattern, BTreeSet::new()),
        (CandidateDetector::ArgumentHomeSpillLeaf, BTreeSet::new()),
        (CandidateDetector::TableDerived, BTreeSet::new()),
    ]);
    if include_semantic {
        per_detector_candidates
            .insert(CandidateDetector::SemanticCallableArgument, BTreeSet::new());
    }
    let mut per_detector_ungradable: BTreeMap<CandidateDetector, BTreeSet<BankAddr>> =
        BTreeMap::new();
    let mut per_detector_sources: BTreeMap<
        CandidateDetector,
        BTreeMap<AddressedPhysicalEntryV2, BTreeSet<AddressedPhysicalEntryV2>>,
    > = BTreeMap::new();
    let mut combined_targets: BTreeSet<BankAddr> = BTreeSet::new();

    for fact in db.facts() {
        let Fact::FunctionEntryClaim {
            target,
            detector,
            evidence,
            proposed_state,
        } = fact
        else {
            continue;
        };
        if *detector == CandidateDetector::SemanticCallableArgument && !include_semantic {
            continue;
        }
        if !include_bank(&target.bank) || !evidence_in_scope(evidence, &include_bank) {
            continue;
        }
        if !is_positive(*proposed_state) {
            continue;
        }
        per_detector_candidates.entry(*detector).or_default();
        match translate_v2(target.bank.as_str(), target.pc, &mappings) {
            Some(entry) => {
                per_detector_candidates
                    .entry(*detector)
                    .or_default()
                    .insert(entry);
                let source = match fact {
                    Fact::FunctionEntryClaim {
                        evidence:
                            FunctionEntryEvidence::DirectJal { call_site }
                            | FunctionEntryEvidence::ResolvedJalr { call_site, .. }
                            | FunctionEntryEvidence::ExhaustiveIndirectCall { call_site, .. }
                            | FunctionEntryEvidence::SemanticCallableArgument { call_site, .. },
                        ..
                    } => translate_v2(&call_site.bank, call_site.pc, &mappings),
                    Fact::FunctionEntryClaim {
                        evidence: FunctionEntryEvidence::HandlerTablePointer { source_slot, .. },
                        ..
                    } => translate_v2(&source_slot.bank, source_slot.pc, &mappings),
                    _ => None,
                };
                if let Some(source) = source {
                    per_detector_sources
                        .entry(*detector)
                        .or_default()
                        .entry(entry)
                        .or_default()
                        .insert(source);
                }
            }
            None => {
                per_detector_ungradable
                    .entry(*detector)
                    .or_default()
                    .insert(target.clone());
            }
        }
        if db
            .conclusion(&function_entry_subject(target))
            .is_some_and(|conclusion| is_positive(conclusion.state))
        {
            combined_targets.insert(target.clone());
        }
    }

    let per_detector = per_detector_candidates
        .into_iter()
        .map(|(detector, candidates)| {
            let provenance = per_detector_sources
                .remove(&detector)
                .unwrap_or_default()
                .into_iter()
                .map(|(candidate, sources)| CandidatePhysicalProvenanceV2 {
                    candidate,
                    sources: sources.into_iter().collect(),
                })
                .collect();
            DetectorCandidateIdentitiesV2 {
                detector,
                candidates: candidates.into_iter().collect(),
                provenance,
                ungradable: per_detector_ungradable
                    .remove(&detector)
                    .unwrap_or_default()
                    .into_iter()
                    .collect(),
            }
        })
        .collect();

    let mut combined_candidates = BTreeSet::new();
    let mut combined_ungradable = BTreeSet::new();
    for target in combined_targets {
        match translate_v2(&target.bank, target.pc, &mappings) {
            Some(entry) => {
                combined_candidates.insert(entry);
            }
            None => {
                combined_ungradable.insert(target);
            }
        }
    }

    ScopedCandidateIdentitiesV2 {
        schema_version,
        per_detector,
        combined_candidates: combined_candidates.into_iter().collect(),
        combined_ungradable: combined_ungradable.into_iter().collect(),
    }
}

fn contains(extent: &FunctionExtent, entry: PhysicalEntry) -> bool {
    entry.rom >= extent.rom_start
        && entry.rom < extent.rom_end
        && entry.vram >= extent.vram_start
        && entry.vram < extent.vram_end
}

fn false_positive_breakdown(
    candidates: &BTreeSet<PhysicalEntry>,
    sources: Option<&BTreeMap<PhysicalEntry, BTreeSet<PhysicalEntry>>>,
    key: &CandidateAnswerKey,
) -> FalsePositiveBreakdown {
    let mut breakdown = FalsePositiveBreakdown::default();
    for candidate in candidates.difference(&key.entries) {
        if key
            .extents
            .iter()
            .any(|extent| contains(extent, *candidate))
        {
            breakdown.target_interior += 1;
        } else {
            breakdown.target_outside_functions += 1;
        }
        let source_inside = sources
            .and_then(|sources| sources.get(candidate))
            .is_some_and(|sources| {
                sources
                    .iter()
                    .any(|source| key.extents.iter().any(|extent| contains(extent, *source)))
            });
        if source_inside {
            breakdown.source_inside_function += 1;
        } else {
            breakdown.source_outside_functions += 1;
        }
        if breakdown.samples.len() < 12 {
            breakdown.samples.push(FalsePositiveSample {
                target: *candidate,
                sources: sources
                    .and_then(|sources| sources.get(candidate))
                    .map_or_else(Vec::new, |sources| sources.iter().copied().collect()),
            });
        }
    }
    breakdown
}

fn is_positive(state: ProofState) -> bool {
    matches!(
        state,
        ProofState::Candidate | ProofState::Supported | ProofState::Proven
    )
}

fn evidence_in_scope(
    evidence: &FunctionEntryEvidence,
    include_bank: &impl Fn(&str) -> bool,
) -> bool {
    match evidence {
        FunctionEntryEvidence::RomHeaderEntrypoint => true,
        FunctionEntryEvidence::DirectJal { call_site } => include_bank(&call_site.bank),
        FunctionEntryEvidence::ResolvedJalr {
            call_site,
            construction_start,
        } => include_bank(&call_site.bank) && include_bank(&construction_start.bank),
        FunctionEntryEvidence::ExhaustiveIndirectCall {
            call_site,
            memory_sources,
            ..
        } => {
            include_bank(&call_site.bank)
                && memory_sources
                    .iter()
                    .all(|source| include_bank(&source.bank))
        }
        FunctionEntryEvidence::SemanticCallableArgument {
            call_site,
            callee,
            contract,
            ..
        } => {
            include_bank(&call_site.bank)
                && include_bank(&callee.bank)
                && match contract {
                    crate::facts::SemanticCallableContract::OsCreateThread => true,
                    crate::facts::SemanticCallableContract::ArgumentToJalr { jalr_sites } => {
                        jalr_sites.iter().all(|site| include_bank(&site.bank))
                    }
                    crate::facts::SemanticCallableContract::CallbackRegistry {
                        dispatcher,
                        callback_store_site,
                        list_insert_site,
                        jalr_site,
                    } => [dispatcher, callback_store_site, list_insert_site, jalr_site]
                        .into_iter()
                        .all(|site| include_bank(&site.bank)),
                }
        }
        FunctionEntryEvidence::Prologue {
            stack_adjust,
            corroborating_site,
            ..
        } => include_bank(&stack_adjust.bank) && include_bank(&corroborating_site.bank),
        FunctionEntryEvidence::ArgumentHomeSpillLeaf {
            predecessor_return,
            spill_site,
            return_site,
            ..
        } => {
            include_bank(&predecessor_return.bank)
                && include_bank(&spill_site.bank)
                && include_bank(&return_site.bank)
        }
        FunctionEntryEvidence::TableEntry { table, .. } => include_bank(&table.bank),
        FunctionEntryEvidence::HandlerTablePointer {
            table_base,
            source_slot,
            ..
        } => include_bank(&table_base.bank) && include_bank(&source_slot.bank),
    }
}

fn mappings_by_bank(
    db: &FactDb,
    include_bank: &impl Fn(&str) -> bool,
) -> BTreeMap<String, Vec<Mapping>> {
    let mut out: BTreeMap<String, Vec<Mapping>> = BTreeMap::new();
    for fact in db.proven_rom_mappings() {
        let Fact::RomMapping {
            bank,
            rom_space,
            rom_start,
            rom_end,
            va_start,
            va_end,
            ..
        } = fact
        else {
            unreachable!("proven_rom_mappings returned non-mapping")
        };
        if !include_bank(bank) {
            continue;
        }
        out.entry(bank.clone()).or_default().push(Mapping {
            rom_space: *rom_space,
            rom_start: *rom_start,
            rom_end: *rom_end,
            va_start: *va_start,
            va_end: *va_end,
        });
    }
    out
}

fn translate_v2(
    bank: &str,
    pc: u32,
    mappings: &BTreeMap<String, Vec<Mapping>>,
) -> Option<AddressedPhysicalEntryV2> {
    let matches: BTreeSet<AddressedPhysicalEntryV2> = mappings
        .get(bank)?
        .iter()
        .filter(|mapping| pc >= mapping.va_start && pc < mapping.va_end)
        .filter_map(|mapping| {
            let offset = pc.checked_sub(mapping.va_start)?;
            let rom = mapping.rom_start.checked_add(offset)?;
            (rom < mapping.rom_end).then_some(AddressedPhysicalEntryV2 {
                rom_space: mapping.rom_space,
                rom,
                vram: pc,
            })
        })
        .collect();
    (matches.len() == 1).then(|| *matches.iter().next().unwrap())
}

fn translate(
    bank: &str,
    pc: u32,
    mappings: &BTreeMap<String, Vec<Mapping>>,
) -> Option<PhysicalEntry> {
    let matches: BTreeSet<PhysicalEntry> = mappings
        .get(bank)?
        .iter()
        .filter(|mapping| pc >= mapping.va_start && pc < mapping.va_end)
        .filter_map(|mapping| {
            let offset = pc.checked_sub(mapping.va_start)?;
            let rom = mapping.rom_start.checked_add(offset)?;
            (rom < mapping.rom_end).then_some(PhysicalEntry { rom, vram: pc })
        })
        .collect();
    (matches.len() == 1).then(|| *matches.iter().next().unwrap())
}

fn metrics(candidates: &BTreeSet<PhysicalEntry>, key: &CandidateAnswerKey) -> PrecisionRecall {
    let true_positives = candidates.intersection(&key.entries).count();
    let recalled_functions = candidates
        .iter()
        .filter_map(|entry| key.multiplicity.get(entry))
        .sum();
    PrecisionRecall {
        candidates: candidates.len(),
        true_positives,
        false_positives: candidates.len() - true_positives,
        recalled_functions,
        false_negatives: key.function_count - recalled_functions,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::facts::{BankAddr, CandidateDetector, FunctionEntryEvidence, ProloguePattern};

    const SAMPLE: &str = r#"
[[section]]
name = "boot"
rom = 0x1000
vram = 0x80000400
size = 0x20
functions = [
  { name = "a", vram = 0x80000400, size = 0x10 },
  { name = "b", vram = 0x80000410, size = 0x10 },
]
"#;

    #[test]
    fn parses_physical_function_identities() {
        let key = parse_symbol_dump(SAMPLE).unwrap();
        assert_eq!(key.section_count, 1);
        assert_eq!(key.function_count, 2);
        assert_eq!(key.entries.len(), 2);
        assert!(key.entries.contains(&PhysicalEntry {
            rom: 0x1010,
            vram: 0x8000_0410,
        }));
    }

    #[test]
    fn grades_each_detector_and_combined_without_va_aliasing() {
        let key = parse_symbol_dump(SAMPLE).unwrap();
        let mut db = FactDb::new();
        let mapping = db.insert(Fact::RomMapping {
            bank: "boot".into(),
            rom_space: crate::facts::RomAddressSpace::Physical,
            rom_start: 0x1000,
            rom_end: 0x1020,
            va_start: 0x8000_0400,
            va_end: 0x8000_0420,
        });
        db.conclude("bank:boot", ProofState::Proven, vec![mapping], "test")
            .unwrap();

        for (pc, detector, evidence) in [
            (
                0x8000_0400,
                CandidateDetector::JalTarget,
                FunctionEntryEvidence::DirectJal {
                    call_site: BankAddr::new("boot", 0x8000_0410),
                },
            ),
            (
                0x8000_0408,
                CandidateDetector::ProloguePattern,
                FunctionEntryEvidence::Prologue {
                    stack_adjust: BankAddr::new("boot", 0x8000_0408),
                    frame_size: 0x20,
                    pattern: ProloguePattern::SavesReturnAddress,
                    corroborating_site: BankAddr::new("boot", 0x8000_040c),
                },
            ),
        ] {
            let target = BankAddr::new("boot", pc);
            let fact = db.insert(Fact::FunctionEntryClaim {
                target: target.clone(),
                detector,
                evidence,
                proposed_state: ProofState::Candidate,
            });
            db.conclude(
                function_entry_subject(&target),
                ProofState::Candidate,
                vec![fact],
                "test",
            )
            .unwrap();
        }

        let report = grade_candidates(&db, &key);
        let jal = report
            .per_detector
            .iter()
            .find(|grade| grade.detector == CandidateDetector::JalTarget)
            .unwrap();
        assert_eq!(jal.metrics.true_positives, 1);
        assert_eq!(jal.metrics.false_positives, 0);
        assert_eq!(report.combined.true_positives, 1);
        assert_eq!(report.combined.false_positives, 1);
        assert_eq!(report.combined.false_negatives, 1);
    }

    #[test]
    fn scoped_grade_excludes_an_unrelated_proven_load_image() {
        let key = parse_symbol_dump(SAMPLE).unwrap();
        let mut db = FactDb::new();
        for (bank, rom_start, va_start) in [
            ("boot", 0x1000, 0x8000_0400),
            ("request_dma_0", 0x2000, 0x8010_0000),
        ] {
            let mapping = db.insert(Fact::RomMapping {
                bank: bank.into(),
                rom_space: crate::facts::RomAddressSpace::Physical,
                rom_start,
                rom_end: rom_start + 0x20,
                va_start,
                va_end: va_start + 0x20,
            });
            db.conclude(
                format!("bank:{bank}"),
                ProofState::Proven,
                vec![mapping],
                "test",
            )
            .unwrap();
            let target = BankAddr::new(bank, va_start);
            let claim = db.insert(Fact::FunctionEntryClaim {
                target: target.clone(),
                detector: CandidateDetector::ProloguePattern,
                evidence: FunctionEntryEvidence::Prologue {
                    stack_adjust: target.clone(),
                    frame_size: 0x20,
                    pattern: ProloguePattern::SavesReturnAddress,
                    corroborating_site: target.clone(),
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
        }

        let full = grade_candidates(&db, &key);
        let boot_only = grade_candidates_scoped(&db, &key, |bank| bank == "boot");
        assert_eq!(full.combined.candidates, 2);
        assert_eq!(full.combined.false_positives, 1);
        assert_eq!(boot_only.combined.candidates, 1);
        assert_eq!(boot_only.combined.true_positives, 1);
        assert_eq!(boot_only.combined.false_positives, 0);
        assert_eq!(boot_only.combined_ungradable, 0);
    }

    #[test]
    fn scoped_grade_excludes_cross_bank_evidence_from_outside_the_scope() {
        let key = parse_symbol_dump(SAMPLE).unwrap();
        let mut db = FactDb::new();
        let mapping = db.insert(Fact::RomMapping {
            bank: "boot".into(),
            rom_space: crate::facts::RomAddressSpace::Physical,
            rom_start: 0x1000,
            rom_end: 0x1020,
            va_start: 0x8000_0400,
            va_end: 0x8000_0420,
        });
        db.conclude("bank:boot", ProofState::Proven, vec![mapping], "test")
            .unwrap();
        let target = BankAddr::new("boot", 0x8000_0410);
        let claim = db.insert(Fact::FunctionEntryClaim {
            target: target.clone(),
            detector: CandidateDetector::JalTarget,
            evidence: FunctionEntryEvidence::DirectJal {
                call_site: BankAddr::new("request_dma_0", 0x8010_0000),
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

        assert_eq!(grade_candidates(&db, &key).combined.candidates, 1);
        let scoped = grade_candidates_scoped(&db, &key, |bank| bank == "boot");
        assert_eq!(scoped.combined.candidates, 0);
        assert_eq!(scoped.combined.false_negatives, 2);
    }

    #[test]
    fn candidate_identity_digest_covers_physical_provenance() {
        fn db_with_call_site(call_pc: u32) -> FactDb {
            let mut db = FactDb::new();
            let mapping = db.insert(Fact::RomMapping {
                bank: "boot".into(),
                rom_space: crate::facts::RomAddressSpace::Physical,
                rom_start: 0x1000,
                rom_end: 0x1020,
                va_start: 0x8000_0400,
                va_end: 0x8000_0420,
            });
            db.conclude("bank:boot", ProofState::Proven, vec![mapping], "test")
                .unwrap();
            let target = BankAddr::new("boot", 0x8000_0410);
            let claim = db.insert(Fact::FunctionEntryClaim {
                target: target.clone(),
                detector: CandidateDetector::JalTarget,
                evidence: FunctionEntryEvidence::DirectJal {
                    call_site: BankAddr::new("boot", call_pc),
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
            db
        }

        let first = scoped_candidate_identities_v1(&db_with_call_site(0x8000_0400), |_| true);
        let repeated = scoped_candidate_identities_v1(&db_with_call_site(0x8000_0400), |_| true);
        let different_source =
            scoped_candidate_identities_v1(&db_with_call_site(0x8000_0404), |_| true);
        assert_eq!(first, repeated);
        assert_eq!(first.digest_sha256(), repeated.digest_sha256());
        assert_eq!(
            first.combined_candidates,
            different_source.combined_candidates
        );
        assert_ne!(first, different_source);
        assert_ne!(first.digest_sha256(), different_source.digest_sha256());
    }

    #[test]
    fn candidate_identity_receipt_retains_ungradable_identities() {
        let mut db = FactDb::new();
        let target = BankAddr::new("open_bank", 0x8010_0000);
        let claim = db.insert(Fact::FunctionEntryClaim {
            target: target.clone(),
            detector: CandidateDetector::HardwareEntrypoint,
            evidence: FunctionEntryEvidence::RomHeaderEntrypoint,
            proposed_state: ProofState::Candidate,
        });
        db.conclude(
            function_entry_subject(&target),
            ProofState::Candidate,
            vec![claim],
            "test",
        )
        .unwrap();

        let identities = scoped_candidate_identities_v1(&db, |_| true);
        assert_eq!(identities.combined_ungradable, [target.clone()]);
        let hardware = identities
            .per_detector
            .iter()
            .find(|row| row.detector == CandidateDetector::HardwareEntrypoint)
            .unwrap();
        assert_eq!(hardware.ungradable, [target]);
        assert!(hardware.candidates.is_empty());
    }

    fn insert_v2_jal_candidate(
        db: &mut FactDb,
        bank: &str,
        rom_space: RomAddressSpace,
        call_pc: u32,
    ) {
        let mapping = db.insert(Fact::RomMapping {
            bank: bank.into(),
            rom_space,
            rom_start: 0x1000,
            rom_end: 0x1020,
            va_start: 0x8000_0400,
            va_end: 0x8000_0420,
        });
        db.conclude(
            format!("bank:{bank}"),
            ProofState::Proven,
            vec![mapping],
            "test",
        )
        .unwrap();
        let target = BankAddr::new(bank, 0x8000_0410);
        let claim = db.insert(Fact::FunctionEntryClaim {
            target: target.clone(),
            detector: CandidateDetector::JalTarget,
            evidence: FunctionEntryEvidence::DirectJal {
                call_site: BankAddr::new(bank, call_pc),
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
    }

    #[test]
    fn v2_keeps_equal_numeric_physical_and_virtual_identities_distinct() {
        let mut both = FactDb::new();
        insert_v2_jal_candidate(
            &mut both,
            "physical",
            RomAddressSpace::Physical,
            0x8000_0400,
        );
        insert_v2_jal_candidate(&mut both, "virtual", RomAddressSpace::Virtual, 0x8000_0400);

        let v1 = scoped_candidate_identities_v1(&both, |_| true);
        let v2 = scoped_candidate_identities_v2(&both, |_| true);
        assert_eq!(v1.combined_candidates.len(), 1);
        assert_eq!(v2.schema_version, SCOPED_CANDIDATE_IDENTITY_SCHEMA_V2);
        assert_eq!(
            v2.combined_candidates,
            [
                AddressedPhysicalEntryV2 {
                    rom_space: RomAddressSpace::Physical,
                    rom: 0x1010,
                    vram: 0x8000_0410,
                },
                AddressedPhysicalEntryV2 {
                    rom_space: RomAddressSpace::Virtual,
                    rom: 0x1010,
                    vram: 0x8000_0410,
                },
            ]
        );

        let mut physical_only = FactDb::new();
        insert_v2_jal_candidate(
            &mut physical_only,
            "physical",
            RomAddressSpace::Physical,
            0x8000_0400,
        );
        let physical_receipt = scoped_candidate_identities_v2(&physical_only, |_| true);
        let mut virtual_only = FactDb::new();
        insert_v2_jal_candidate(
            &mut virtual_only,
            "physical",
            RomAddressSpace::Virtual,
            0x8000_0400,
        );
        let virtual_receipt = scoped_candidate_identities_v2(&virtual_only, |_| true);
        assert_eq!(
            physical_receipt.combined_candidates[0].rom,
            virtual_receipt.combined_candidates[0].rom
        );
        assert_eq!(
            physical_receipt.combined_candidates[0].vram,
            virtual_receipt.combined_candidates[0].vram
        );
        assert_ne!(physical_receipt, virtual_receipt);
        assert_ne!(
            physical_receipt.digest_sha256(),
            virtual_receipt.digest_sha256()
        );
        assert_ne!(v2.digest_sha256(), physical_receipt.digest_sha256());
    }

    #[test]
    fn v3_adds_semantic_callable_authority_without_changing_v2() {
        let mut db = FactDb::new();
        insert_v2_jal_candidate(&mut db, "resident", RomAddressSpace::Physical, 0x8000_0400);
        let target = BankAddr::new("resident", 0x8000_0410);
        let semantic = db.insert(Fact::FunctionEntryClaim {
            target: target.clone(),
            detector: CandidateDetector::SemanticCallableArgument,
            evidence: FunctionEntryEvidence::SemanticCallableArgument {
                call_site: BankAddr::new("resident", 0x8000_0404),
                callee: BankAddr::new("resident", 0x8000_040c),
                pointer_register: 6,
                contract: crate::facts::SemanticCallableContract::OsCreateThread,
            },
            proposed_state: ProofState::Proven,
        });
        db.conclude(
            function_entry_subject(&target),
            ProofState::Proven,
            vec![semantic],
            "test",
        )
        .unwrap();

        let v2 = scoped_candidate_identities_v2(&db, |_| true);
        assert_eq!(v2.schema_version, SCOPED_CANDIDATE_IDENTITY_SCHEMA_V2);
        assert!(!v2
            .per_detector
            .iter()
            .any(|row| { row.detector == CandidateDetector::SemanticCallableArgument }));

        let v3 = scoped_candidate_identities_v3(&db, |_| true);
        assert_eq!(v3.schema_version, SCOPED_CANDIDATE_IDENTITY_SCHEMA_V3);
        let row = v3
            .per_detector
            .iter()
            .find(|row| row.detector == CandidateDetector::SemanticCallableArgument)
            .unwrap();
        assert_eq!(row.candidates.len(), 1);
        assert_eq!(row.provenance.len(), 1);
        assert_eq!(row.provenance[0].sources[0].vram, 0x8000_0404);
    }

    #[test]
    fn v2_receipt_is_deterministic_and_hashes_addressed_provenance() {
        let mut first_db = FactDb::new();
        insert_v2_jal_candidate(
            &mut first_db,
            "overlay",
            RomAddressSpace::Virtual,
            0x8000_0400,
        );
        let first = scoped_candidate_identities_v2(&first_db, |_| true);
        let repeated = scoped_candidate_identities_v2(&first_db, |_| true);
        assert_eq!(first, repeated);
        assert_eq!(first.digest_sha256(), repeated.digest_sha256());

        let jal = first
            .per_detector
            .iter()
            .find(|row| row.detector == CandidateDetector::JalTarget)
            .unwrap();
        assert_eq!(jal.provenance.len(), 1);
        assert_eq!(
            jal.provenance[0].sources,
            [AddressedPhysicalEntryV2 {
                rom_space: RomAddressSpace::Virtual,
                rom: 0x1000,
                vram: 0x8000_0400,
            }]
        );

        let mut different_source_db = FactDb::new();
        insert_v2_jal_candidate(
            &mut different_source_db,
            "overlay",
            RomAddressSpace::Virtual,
            0x8000_0404,
        );
        let different_source = scoped_candidate_identities_v2(&different_source_db, |_| true);
        assert_eq!(
            first.combined_candidates,
            different_source.combined_candidates
        );
        assert_ne!(first.digest_sha256(), different_source.digest_sha256());
    }

    #[test]
    fn v2_handler_table_provenance_hashes_the_exact_source_slot() {
        fn receipt(source_pc: u32) -> ScopedCandidateIdentitiesV2 {
            let mut db = FactDb::new();
            let mapping = db.insert(Fact::RomMapping {
                bank: "overlay".into(),
                rom_space: RomAddressSpace::Virtual,
                rom_start: 0x1000,
                rom_end: 0x1020,
                va_start: 0x8000_0400,
                va_end: 0x8000_0420,
            });
            db.conclude("bank:overlay", ProofState::Proven, vec![mapping], "test")
                .unwrap();
            let target = BankAddr::new("overlay", 0x8000_0410);
            let claim = db.insert(Fact::FunctionEntryClaim {
                target: target.clone(),
                detector: CandidateDetector::TableDerived,
                evidence: FunctionEntryEvidence::HandlerTablePointer {
                    table_base: BankAddr::new("overlay", 0x8000_0400),
                    source_slot: BankAddr::new("overlay", source_pc),
                    slot_ordinal: (source_pc - 0x8000_0400) / 4,
                    stride_words: 1,
                    run_length: 4,
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
            scoped_candidate_identities_v2(&db, |_| true)
        }

        let first = receipt(0x8000_0400);
        let different_slot = receipt(0x8000_0404);
        let table = first
            .per_detector
            .iter()
            .find(|row| row.detector == CandidateDetector::TableDerived)
            .unwrap();
        assert_eq!(
            table.provenance[0].sources,
            [AddressedPhysicalEntryV2 {
                rom_space: RomAddressSpace::Virtual,
                rom: 0x1000,
                vram: 0x8000_0400,
            }]
        );
        assert_eq!(
            first.combined_candidates,
            different_slot.combined_candidates
        );
        assert_ne!(first.digest_sha256(), different_slot.digest_sha256());
    }

    #[test]
    fn v2_receipt_retains_ungradable_identities() {
        let mut db = FactDb::new();
        let target = BankAddr::new("open_bank", 0x8010_0000);
        let claim = db.insert(Fact::FunctionEntryClaim {
            target: target.clone(),
            detector: CandidateDetector::HardwareEntrypoint,
            evidence: FunctionEntryEvidence::RomHeaderEntrypoint,
            proposed_state: ProofState::Candidate,
        });
        db.conclude(
            function_entry_subject(&target),
            ProofState::Candidate,
            vec![claim],
            "test",
        )
        .unwrap();

        let identities = scoped_candidate_identities_v2(&db, |_| true);
        assert_eq!(identities.combined_ungradable, [target.clone()]);
        let hardware = identities
            .per_detector
            .iter()
            .find(|row| row.detector == CandidateDetector::HardwareEntrypoint)
            .unwrap();
        assert_eq!(hardware.ungradable, [target]);
        assert!(hardware.candidates.is_empty());
    }
}
