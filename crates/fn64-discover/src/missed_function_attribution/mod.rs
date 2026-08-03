//! Supervised, answer-key-after-the-fact attribution for function discovery.
//!
//! [`ColdAttributionIndexBuilder`] consumes only cold discovery snapshots.
//! Answer rows are admitted later by [`attribute_known_functions`], so the
//! answer key can explain misses without becoming detector input. Raw answer
//! ROM coordinates intentionally carry no [`RomAddressSpace`]; only a unique
//! cold mapping may assign one.

use crate::cfg::WordClass;
use crate::facts::{
    function_entry_subject, BankAddr, CandidateDetector, Fact, ProofState, RomAddressSpace,
};
use crate::grade_candidates::{AddressedPhysicalEntryV2, ScopedCandidateIdentitiesV3};
use crate::owner_proof::OwnerAssessment;
use crate::snapshot::{OwnerBlockerKind, ProgramSnapshotV1};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};

pub const MISSED_FUNCTION_ATTRIBUTION_SCHEMA_V1: u32 = 1;
pub const MAX_COLD_FACT_ROWS: u64 = 4_000_000;
pub const MAX_COLD_WORD_ROWS: u64 = 16_000_000;
pub const MAX_ANSWER_SECTIONS: u64 = 1_048_576;
pub const MAX_ANSWER_ROWS: u64 = 2_000_000;
pub const MAX_CANDIDATE_ROWS: u64 = 2_000_000;
pub const KNOWN_FUNCTION_ATTRIBUTION_ENVELOPE_SCHEMA_V2: u32 = 2;
pub const KNOWN_FUNCTION_ATTRIBUTION_ALGORITHM_V2: &str = "fn64.known-function-attribution.v2";

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionDomain {
    Vr4300,
    Rsp,
    Cic,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AnswerRowKind {
    Function,
    Alias,
    ZeroSizeMarker,
}

/// One answer-key section. `rom_start` is deliberately an unqualified raw
/// coordinate: dumps do not reliably say whether it is physical ROM or VROM.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AnswerSectionV1 {
    pub raw_ordinal: u64,
    pub name: String,
    pub execution_domain: ExecutionDomain,
    pub rom_start: u32,
    pub vram_start: u32,
    pub size: u32,
}

/// One raw answer row. Aliases and zero-size linker markers are retained as
/// rows instead of being deduplicated away. `section_raw_ordinal` refers to
/// [`AnswerSectionV1::raw_ordinal`], never its position in a vector.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AnswerFunctionV1 {
    pub raw_ordinal: u64,
    pub section_raw_ordinal: u64,
    pub name: String,
    pub vram: u32,
    pub size: u32,
    pub kind: AnswerRowKind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AttributionError {
    InvalidMapping {
        bank: String,
    },
    DuplicateSectionOrdinal {
        raw_ordinal: u64,
    },
    DuplicateFunctionOrdinal {
        raw_ordinal: u64,
    },
    MissingSection {
        function_ordinal: u64,
        section_ordinal: u64,
    },
    FunctionBeforeSection {
        function_ordinal: u64,
    },
    FunctionOutsideSection {
        function_ordinal: u64,
    },
    ArithmeticOverflow {
        context: &'static str,
        raw_ordinal: u64,
    },
    CounterOverflow {
        counter: &'static str,
    },
    LimitExceeded {
        resource: &'static str,
        count: u64,
        limit: u64,
    },
    Serialization(String),
    InvalidReport(String),
}

impl std::fmt::Display for AttributionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidMapping { bank } => write!(f, "invalid ROM mapping for bank {bank:?}"),
            Self::DuplicateSectionOrdinal { raw_ordinal } => {
                write!(f, "duplicate answer section ordinal {raw_ordinal}")
            }
            Self::DuplicateFunctionOrdinal { raw_ordinal } => {
                write!(f, "duplicate answer function ordinal {raw_ordinal}")
            }
            Self::MissingSection {
                function_ordinal,
                section_ordinal,
            } => write!(
                f,
                "answer function {function_ordinal} names missing section {section_ordinal}"
            ),
            Self::FunctionBeforeSection { function_ordinal } => write!(
                f,
                "answer function {function_ordinal} starts before its section"
            ),
            Self::FunctionOutsideSection { function_ordinal } => write!(
                f,
                "answer function {function_ordinal} extends outside its section"
            ),
            Self::ArithmeticOverflow {
                context,
                raw_ordinal,
            } => {
                write!(f, "{context} overflows for answer row {raw_ordinal}")
            }
            Self::CounterOverflow { counter } => write!(f, "counter {counter} overflows u64"),
            Self::LimitExceeded {
                resource,
                count,
                limit,
            } => write!(f, "{resource} count {count} exceeds limit {limit}"),
            Self::Serialization(error) => write!(f, "canonical report serialization: {error}"),
            Self::InvalidReport(error) => write!(f, "invalid attribution report: {error}"),
        }
    }
}

impl std::error::Error for AttributionError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RelationKindV1 {
    Direct,
    Resolved,
    Table,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OwnerStateV1 {
    Proven,
    Candidate,
    Ambiguous,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResolvedMappingV1 {
    pub rom_space: RomAddressSpace,
    pub rom: u32,
    pub bank: String,
    pub vram: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClaimObservationV1 {
    pub detector: CandidateDetector,
    pub proposed_states: Vec<ProofState>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OwnerObservationV1 {
    pub state: OwnerStateV1,
    pub blocker_kinds: Vec<OwnerBlockerKind>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AttributionObservationsV1 {
    pub mappings: Vec<ResolvedMappingV1>,
    pub claims: Vec<ClaimObservationV1>,
    pub conclusion_states: Vec<ProofState>,
    pub word_classes: Vec<WordClass>,
    pub owners: Vec<OwnerObservationV1>,
    pub incoming_relations: Vec<RelationKindV1>,
    pub candidate_detectors: Vec<CandidateDetector>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MissReasonV1 {
    NoMapping,
    AmbiguousMapping,
    ExactCandidateNotPromoted,
    ProvenCodeNoEntry,
    CandidateCodeNoEntry,
    MappedUnreached,
    NoRelation,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
#[serde(deny_unknown_fields)]
pub enum AnswerAttributionStatusV1 {
    CandidateMatched,
    Missed { primary_reason: MissReasonV1 },
    NotDiscoverableMarker,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AnswerAttributionV1 {
    pub function: AnswerFunctionV1,
    pub execution_domain: ExecutionDomain,
    pub raw_rom: u32,
    pub status: AnswerAttributionStatusV1,
    pub observations: AttributionObservationsV1,
    pub mechanism_cluster_key: String,
    pub instance_cluster_key: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CandidateStatusV1 {
    CandidateMatched,
    AmbiguousAnswerMapping,
    Interior,
    Gap,
    Outside,
    Ungradable,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[serde(deny_unknown_fields)]
pub enum CandidateAccountingIdentityV1 {
    Addressed { entry: AddressedPhysicalEntryV2 },
    Ungradable { address: BankAddr },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CandidateDetectorSourcesV1 {
    pub detector: CandidateDetector,
    pub sources: Vec<AddressedPhysicalEntryV2>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CandidateAttributionV1 {
    pub identity: CandidateAccountingIdentityV1,
    pub combined: bool,
    pub detectors: Vec<CandidateDetector>,
    pub detector_sources: Vec<CandidateDetectorSourcesV1>,
    pub status: CandidateStatusV1,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CandidateAccountingTotalsV1 {
    /// Unique union of combined, per-detector, and ungradable identities.
    pub denominator: u64,
    pub gradable: u64,
    pub ungradable: u64,
    pub combined: u64,
    pub per_detector_only: u64,
    pub candidate_matched: u64,
    pub ambiguous_answer_mapping: u64,
    pub interior: u64,
    pub gap: u64,
    pub outside: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AttributionTotalsV1 {
    pub raw_rows: u64,
    pub nonzero_rows: u64,
    pub distinct_bodies: u64,
    pub alias_rows: u64,
    pub marker_rows: u64,
    pub candidate_matched_rows: u64,
    pub missed_rows: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DomainTotalsV1 {
    pub execution_domain: ExecutionDomain,
    pub totals: AttributionTotalsV1,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AttributionReportV1 {
    pub schema_version: u32,
    /// Canonical raw-section catalog, sorted by ordinal. Rows refer here by
    /// ordinal so cluster consumers retain names and extents without copying
    /// them into every function row.
    pub sections: Vec<AnswerSectionV1>,
    pub rows: Vec<AnswerAttributionV1>,
    pub candidate_statuses: Vec<CandidateAttributionV1>,
    pub candidate_totals: CandidateAccountingTotalsV1,
    pub totals: AttributionTotalsV1,
    pub per_domain: Vec<DomainTotalsV1>,
    /// SHA-256 of the canonical JSON serialization of every preceding field.
    pub canonical_sha256: String,
}

/// Strict artifact envelope shared by the producer and the independent
/// validation mode. The digest strings are the exact identities supplied by
/// the cold workspace and answer-key admission boundary.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AttributionEnvelopeV2 {
    pub schema_version: u32,
    pub algorithm: String,
    pub normalized_rom_sha256: String,
    pub cold_workspace_manifest_sha256: String,
    pub cold_candidate_identities_v3_sha256: String,
    pub answer_key_sha256: String,
    pub answer_key_execution_domain: ExecutionDomain,
    pub report: AttributionReportV1,
}

#[derive(Debug, Clone, Copy)]
pub struct AttributionEnvelopeBindingsV2<'a> {
    pub normalized_rom_sha256: &'a str,
    pub cold_workspace_manifest_sha256: &'a str,
    pub cold_candidate_identities_v3_sha256: &'a str,
    pub answer_key_sha256: &'a str,
}

#[derive(Serialize)]
struct CanonicalAttributionReportV1<'a> {
    schema_version: u32,
    sections: &'a [AnswerSectionV1],
    rows: &'a [AnswerAttributionV1],
    candidate_statuses: &'a [CandidateAttributionV1],
    candidate_totals: &'a CandidateAccountingTotalsV1,
    totals: &'a AttributionTotalsV1,
    per_domain: &'a [DomainTotalsV1],
}

fn canonical_attribution_report_digest(
    report: &AttributionReportV1,
) -> Result<String, AttributionError> {
    let canonical = CanonicalAttributionReportV1 {
        schema_version: report.schema_version,
        sections: &report.sections,
        rows: &report.rows,
        candidate_statuses: &report.candidate_statuses,
        candidate_totals: &report.candidate_totals,
        totals: &report.totals,
        per_domain: &report.per_domain,
    };
    let bytes = serde_json::to_vec(&canonical)
        .map_err(|error| AttributionError::Serialization(error.to_string()))?;
    Ok(format!("{:x}", Sha256::digest(bytes)))
}

fn invalid_report(detail: impl Into<String>) -> AttributionError {
    AttributionError::InvalidReport(detail.into())
}

fn is_lower_hex_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn strictly_sorted_unique<T: Ord>(values: &[T]) -> bool {
    values.windows(2).all(|pair| pair[0] < pair[1])
}

/// Parse and independently validate a produced attribution envelope. This
/// deliberately recomputes every counter and every row status derivable from
/// the retained evidence; a self-consistent replacement digest is not enough
/// to admit a mutated report.
pub fn validate_attribution_envelope_json_v2(
    bytes: &[u8],
    bindings: AttributionEnvelopeBindingsV2<'_>,
) -> Result<AttributionEnvelopeV2, AttributionError> {
    let envelope: AttributionEnvelopeV2 = serde_json::from_slice(bytes)
        .map_err(|error| invalid_report(format!("typed JSON parse failed: {error}")))?;
    if envelope.schema_version != KNOWN_FUNCTION_ATTRIBUTION_ENVELOPE_SCHEMA_V2
        || envelope.algorithm != KNOWN_FUNCTION_ATTRIBUTION_ALGORITHM_V2
        || envelope.answer_key_execution_domain != ExecutionDomain::Unknown
    {
        return Err(invalid_report("unsupported envelope identity"));
    }
    for (actual, expected, label) in [
        (
            envelope.normalized_rom_sha256.as_str(),
            bindings.normalized_rom_sha256,
            "normalized ROM",
        ),
        (
            envelope.cold_workspace_manifest_sha256.as_str(),
            bindings.cold_workspace_manifest_sha256,
            "cold manifest",
        ),
        (
            envelope.cold_candidate_identities_v3_sha256.as_str(),
            bindings.cold_candidate_identities_v3_sha256,
            "candidate identity",
        ),
        (
            envelope.answer_key_sha256.as_str(),
            bindings.answer_key_sha256,
            "answer key",
        ),
    ] {
        if !is_lower_hex_sha256(actual) || actual != expected {
            return Err(invalid_report(format!("{label} digest mismatch")));
        }
    }
    validate_attribution_report_v1(&envelope.report)?;
    Ok(envelope)
}

pub fn validate_attribution_report_v1(
    report: &AttributionReportV1,
) -> Result<(), AttributionError> {
    if report.schema_version != MISSED_FUNCTION_ATTRIBUTION_SCHEMA_V1 {
        return Err(invalid_report("unsupported inner schema"));
    }
    for (label, count, limit) in [
        ("sections", report.sections.len(), MAX_ANSWER_SECTIONS),
        ("rows", report.rows.len(), MAX_ANSWER_ROWS),
        (
            "candidate statuses",
            report.candidate_statuses.len(),
            MAX_CANDIDATE_ROWS,
        ),
    ] {
        if u64::try_from(count).map_err(|_| invalid_report("row count overflow"))? > limit {
            return Err(invalid_report(format!(
                "{label} exceed the validation limit"
            )));
        }
    }
    if !is_lower_hex_sha256(&report.canonical_sha256)
        || canonical_attribution_report_digest(report)? != report.canonical_sha256
    {
        return Err(invalid_report("inner canonical digest mismatch"));
    }
    if !strictly_sorted_unique(
        &report
            .sections
            .iter()
            .map(|section| section.raw_ordinal)
            .collect::<Vec<_>>(),
    ) {
        return Err(invalid_report("sections are not canonical"));
    }
    if !strictly_sorted_unique(
        &report
            .rows
            .iter()
            .map(|row| row.function.raw_ordinal)
            .collect::<Vec<_>>(),
    ) {
        return Err(invalid_report("answer rows are not canonical"));
    }
    if !strictly_sorted_unique(
        &report
            .candidate_statuses
            .iter()
            .map(|candidate| &candidate.identity)
            .collect::<Vec<_>>(),
    ) {
        return Err(invalid_report("candidate statuses are not canonical"));
    }
    if !strictly_sorted_unique(
        &report
            .per_domain
            .iter()
            .map(|domain| domain.execution_domain)
            .collect::<Vec<_>>(),
    ) {
        return Err(invalid_report("domain totals are not canonical"));
    }

    let sections: BTreeMap<_, _> = report
        .sections
        .iter()
        .map(|section| (section.raw_ordinal, section))
        .collect();
    let combined: BTreeSet<_> = report
        .candidate_statuses
        .iter()
        .filter(|candidate| candidate.combined)
        .filter_map(|candidate| match candidate.identity {
            CandidateAccountingIdentityV1::Addressed { entry } => Some(entry),
            CandidateAccountingIdentityV1::Ungradable { .. } => None,
        })
        .collect();
    let mut totals = AttributionTotalsV1::default();
    let mut per_domain = BTreeMap::<ExecutionDomain, AttributionTotalsV1>::new();
    let mut distinct = BTreeSet::new();
    let mut distinct_by_domain = BTreeMap::<ExecutionDomain, BTreeSet<(u32, u32)>>::new();
    let mut unique_starts = BTreeSet::new();
    let mut ambiguous_starts = BTreeSet::new();
    for row in &report.rows {
        let section = sections
            .get(&row.function.section_raw_ordinal)
            .ok_or_else(|| invalid_report("row names a missing section"))?;
        if row.execution_domain != section.execution_domain
            || row.function.name.is_empty()
            || row.function.name.len() > 4096
        {
            return Err(invalid_report("row metadata disagrees with its section"));
        }
        let offset = row
            .function
            .vram
            .checked_sub(section.vram_start)
            .ok_or_else(|| invalid_report("row precedes its section"))?;
        let expected_raw_rom = section
            .rom_start
            .checked_add(offset)
            .ok_or_else(|| invalid_report("row ROM coordinate overflows"))?;
        let function_end = offset
            .checked_add(row.function.size)
            .ok_or_else(|| invalid_report("row extent overflows"))?;
        if expected_raw_rom != row.raw_rom || function_end > section.size {
            return Err(invalid_report("row extent disagrees with its section"));
        }
        let observations = &row.observations;
        if !strictly_sorted_unique(&observations.mappings)
            || !strictly_sorted_unique(&observations.conclusion_states)
            || !strictly_sorted_unique(&observations.word_classes)
            || !strictly_sorted_unique(&observations.owners)
            || !strictly_sorted_unique(&observations.incoming_relations)
            || !strictly_sorted_unique(&observations.candidate_detectors)
        {
            return Err(invalid_report("row observations are not canonical"));
        }
        let claim_detectors = observations
            .claims
            .iter()
            .map(|claim| claim.detector)
            .collect::<Vec<_>>();
        if !strictly_sorted_unique(&claim_detectors)
            || observations
                .claims
                .iter()
                .any(|claim| !strictly_sorted_unique(&claim.proposed_states))
        {
            return Err(invalid_report("row claims are not canonical"));
        }
        let marker = row.function.kind == AnswerRowKind::ZeroSizeMarker || row.function.size == 0;
        let addressed = observations
            .mappings
            .iter()
            .map(|mapping| AddressedPhysicalEntryV2 {
                rom_space: mapping.rom_space,
                rom: mapping.rom,
                vram: mapping.vram,
            })
            .collect::<BTreeSet<_>>();
        let expected_status = if marker {
            AnswerAttributionStatusV1::NotDiscoverableMarker
        } else if observations.mappings.is_empty() {
            AnswerAttributionStatusV1::Missed {
                primary_reason: MissReasonV1::NoMapping,
            }
        } else if observations.mappings.len() != 1 {
            AnswerAttributionStatusV1::Missed {
                primary_reason: MissReasonV1::AmbiguousMapping,
            }
        } else if addressed.iter().any(|entry| combined.contains(entry)) {
            AnswerAttributionStatusV1::CandidateMatched
        } else if !observations.candidate_detectors.is_empty() {
            AnswerAttributionStatusV1::Missed {
                primary_reason: MissReasonV1::ExactCandidateNotPromoted,
            }
        } else if observations.word_classes.contains(&WordClass::ProvenCode) {
            AnswerAttributionStatusV1::Missed {
                primary_reason: MissReasonV1::ProvenCodeNoEntry,
            }
        } else if observations
            .word_classes
            .contains(&WordClass::CandidateCode)
        {
            AnswerAttributionStatusV1::Missed {
                primary_reason: MissReasonV1::CandidateCodeNoEntry,
            }
        } else if !observations.incoming_relations.is_empty() {
            AnswerAttributionStatusV1::Missed {
                primary_reason: MissReasonV1::MappedUnreached,
            }
        } else {
            AnswerAttributionStatusV1::Missed {
                primary_reason: MissReasonV1::NoRelation,
            }
        };
        if row.status != expected_status
            || row.mechanism_cluster_key != mechanism_key(row.execution_domain, &row.status)
            || row.instance_cluster_key != instance_key(&row.mechanism_cluster_key, observations)?
        {
            return Err(invalid_report(
                "row status or cluster identity is inconsistent",
            ));
        }
        if !marker {
            if observations.mappings.len() == 1 {
                unique_starts.extend(addressed.iter().copied());
            } else if observations.mappings.len() > 1 {
                ambiguous_starts.extend(addressed.iter().copied());
            }
            distinct.insert((row.raw_rom, row.function.vram));
            distinct_by_domain
                .entry(row.execution_domain)
                .or_default()
                .insert((row.raw_rom, row.function.vram));
        }
        add_row_totals(&mut totals, row)?;
        add_row_totals(per_domain.entry(row.execution_domain).or_default(), row)?;
    }
    totals.distinct_bodies = u64::try_from(distinct.len())
        .map_err(|_| invalid_report("distinct-body count overflows"))?;
    for (domain, bodies) in distinct_by_domain {
        per_domain.entry(domain).or_default().distinct_bodies = u64::try_from(bodies.len())
            .map_err(|_| invalid_report("domain distinct-body count overflows"))?;
    }
    let expected_domains = per_domain
        .into_iter()
        .map(|(execution_domain, totals)| DomainTotalsV1 {
            execution_domain,
            totals,
        })
        .collect::<Vec<_>>();
    if report.totals != totals || report.per_domain != expected_domains {
        return Err(invalid_report("row totals are inconsistent"));
    }

    let mut candidate_totals = CandidateAccountingTotalsV1 {
        denominator: u64::try_from(report.candidate_statuses.len())
            .map_err(|_| invalid_report("candidate count overflows"))?,
        ..CandidateAccountingTotalsV1::default()
    };
    for candidate in &report.candidate_statuses {
        if !strictly_sorted_unique(&candidate.detectors) {
            return Err(invalid_report("candidate detectors are not canonical"));
        }
        let source_detectors = candidate
            .detector_sources
            .iter()
            .map(|sources| sources.detector)
            .collect::<Vec<_>>();
        if !strictly_sorted_unique(&source_detectors)
            || candidate.detector_sources.iter().any(|sources| {
                !candidate.detectors.contains(&sources.detector)
                    || !strictly_sorted_unique(&sources.sources)
            })
        {
            return Err(invalid_report("candidate provenance is not canonical"));
        }
        match (&candidate.identity, candidate.status) {
            (CandidateAccountingIdentityV1::Ungradable { .. }, CandidateStatusV1::Ungradable) => {}
            (CandidateAccountingIdentityV1::Ungradable { .. }, _) => {
                return Err(invalid_report("ungradable identity has a gradable status"));
            }
            (CandidateAccountingIdentityV1::Addressed { entry }, CandidateStatusV1::Ungradable) => {
                return Err(invalid_report(format!(
                    "addressed identity {entry:?} is marked ungradable"
                )));
            }
            (
                CandidateAccountingIdentityV1::Addressed { entry },
                CandidateStatusV1::CandidateMatched,
            ) if !unique_starts.contains(entry) => {
                return Err(invalid_report(
                    "candidate match is not a unique known start",
                ));
            }
            (
                CandidateAccountingIdentityV1::Addressed { entry },
                CandidateStatusV1::AmbiguousAnswerMapping,
            ) if !ambiguous_starts.contains(entry) => {
                return Err(invalid_report(
                    "ambiguous candidate has no ambiguous known start",
                ));
            }
            _ => {}
        }
        match candidate.status {
            CandidateStatusV1::CandidateMatched => candidate_totals.candidate_matched += 1,
            CandidateStatusV1::AmbiguousAnswerMapping => {
                candidate_totals.ambiguous_answer_mapping += 1
            }
            CandidateStatusV1::Interior => candidate_totals.interior += 1,
            CandidateStatusV1::Gap => candidate_totals.gap += 1,
            CandidateStatusV1::Outside => candidate_totals.outside += 1,
            CandidateStatusV1::Ungradable => candidate_totals.ungradable += 1,
        }
        if candidate.status != CandidateStatusV1::Ungradable {
            candidate_totals.gradable += 1;
        }
        if candidate.combined {
            candidate_totals.combined += 1;
        } else {
            candidate_totals.per_detector_only += 1;
        }
    }
    if report.candidate_totals != candidate_totals {
        return Err(invalid_report("candidate totals are inconsistent"));
    }
    Ok(())
}

/// Rebuild the report from the sealed cold index, its exact candidate receipt,
/// and the independently parsed answer key. Neither the report's answer rows
/// nor its classification totals become an oracle for validation.
pub fn validate_attribution_report_against_cold_v1(
    report: &AttributionReportV1,
    index: &ColdAttributionIndex,
    identities: &ScopedCandidateIdentitiesV3,
    sections: &[AnswerSectionV1],
    functions: &[AnswerFunctionV1],
) -> Result<(), AttributionError> {
    validate_attribution_report_v1(report)?;
    let rebuilt = attribute_known_functions(index, identities, sections, functions)?;
    if rebuilt != *report {
        return Err(invalid_report(
            "report differs from exact cold-input reconstruction",
        ));
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct CompactAddr {
    bank: u32,
    pc: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct CompactMapping {
    bank: u32,
    rom_space: RomAddressSpace,
    rom_start: u32,
    rom_end: u32,
    va_start: u32,
    va_end: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct CompactClaim {
    address: CompactAddr,
    detector: CandidateDetector,
    state: ProofState,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct CompactOwner {
    address: CompactAddr,
    state: OwnerStateV1,
    blockers: Vec<OwnerBlockerKind>,
}

/// Incremental cold-only collector. Banks are interned once; repeated facts
/// from per-bank projected snapshots collapse in ordered sets.
#[derive(Debug, Default)]
pub struct ColdAttributionIndexBuilder {
    ingested_fact_rows: u64,
    ingested_word_rows: u64,
    bank_ids: BTreeMap<String, u32>,
    banks: Vec<String>,
    mappings: BTreeSet<CompactMapping>,
    claims: BTreeSet<CompactClaim>,
    conclusions: BTreeMap<CompactAddr, BTreeSet<ProofState>>,
    words: BTreeMap<CompactAddr, BTreeSet<WordClass>>,
    owners: BTreeSet<CompactOwner>,
    relations: BTreeMap<CompactAddr, BTreeSet<RelationKindV1>>,
}

impl ColdAttributionIndexBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    fn intern_bank(&mut self, bank: &str) -> Result<u32, AttributionError> {
        if let Some(id) = self.bank_ids.get(bank) {
            return Ok(*id);
        }
        let id =
            u32::try_from(self.banks.len()).map_err(|_| AttributionError::CounterOverflow {
                counter: "interned_banks",
            })?;
        self.banks.push(bank.to_owned());
        self.bank_ids.insert(bank.to_owned(), id);
        Ok(id)
    }

    fn addr(&mut self, address: &BankAddr) -> Result<CompactAddr, AttributionError> {
        Ok(CompactAddr {
            bank: self.intern_bank(&address.bank)?,
            pc: address.pc,
        })
    }

    pub fn ingest_snapshot(
        &mut self,
        snapshot: &ProgramSnapshotV1,
    ) -> Result<(), AttributionError> {
        let fact_rows = u64::try_from(snapshot.facts.facts().len()).map_err(|_| {
            AttributionError::CounterOverflow {
                counter: "cold_fact_rows",
            }
        })?;
        let next_fact_rows = self.ingested_fact_rows.checked_add(fact_rows).ok_or(
            AttributionError::CounterOverflow {
                counter: "cold_fact_rows",
            },
        )?;
        if next_fact_rows > MAX_COLD_FACT_ROWS {
            return Err(AttributionError::LimitExceeded {
                resource: "cold_fact_rows",
                count: next_fact_rows,
                limit: MAX_COLD_FACT_ROWS,
            });
        }
        let word_rows = snapshot.banks.iter().try_fold(0u64, |sum, bank| {
            let rows = u64::try_from(bank.closure.cfg.word_class.len()).map_err(|_| {
                AttributionError::CounterOverflow {
                    counter: "cold_word_rows",
                }
            })?;
            sum.checked_add(rows)
                .ok_or(AttributionError::CounterOverflow {
                    counter: "cold_word_rows",
                })
        })?;
        let next_word_rows = self.ingested_word_rows.checked_add(word_rows).ok_or(
            AttributionError::CounterOverflow {
                counter: "cold_word_rows",
            },
        )?;
        if next_word_rows > MAX_COLD_WORD_ROWS {
            return Err(AttributionError::LimitExceeded {
                resource: "cold_word_rows",
                count: next_word_rows,
                limit: MAX_COLD_WORD_ROWS,
            });
        }
        self.ingested_fact_rows = next_fact_rows;
        self.ingested_word_rows = next_word_rows;
        for fact in snapshot.facts.facts() {
            match fact {
                Fact::RomMapping {
                    bank,
                    rom_space,
                    rom_start,
                    rom_end,
                    va_start,
                    va_end,
                } => {
                    if !snapshot
                        .facts
                        .conclusion(&format!("bank:{bank}"))
                        .is_some_and(|conclusion| conclusion.state == ProofState::Proven)
                    {
                        continue;
                    }
                    let rom_len = rom_end.checked_sub(*rom_start);
                    let va_len = va_end.checked_sub(*va_start);
                    if rom_len.is_none() || rom_len != va_len || rom_len == Some(0) {
                        return Err(AttributionError::InvalidMapping { bank: bank.clone() });
                    }
                    let bank_id = self.intern_bank(bank)?;
                    self.mappings.insert(CompactMapping {
                        bank: bank_id,
                        rom_space: *rom_space,
                        rom_start: *rom_start,
                        rom_end: *rom_end,
                        va_start: *va_start,
                        va_end: *va_end,
                    });
                }
                Fact::FunctionEntryClaim {
                    target,
                    detector,
                    proposed_state,
                    ..
                } => {
                    let address = self.addr(target)?;
                    self.claims.insert(CompactClaim {
                        address,
                        detector: *detector,
                        state: *proposed_state,
                    });
                    if let Some(conclusion) =
                        snapshot.facts.conclusion(&function_entry_subject(target))
                    {
                        self.conclusions
                            .entry(address)
                            .or_default()
                            .insert(conclusion.state);
                    }
                }
                Fact::DirectCall { target, .. } => {
                    let address = self.addr(target)?;
                    self.relations
                        .entry(address)
                        .or_default()
                        .insert(RelationKindV1::Direct);
                }
                Fact::ResolvedCall { target, .. } => {
                    let address = self.addr(target)?;
                    self.relations
                        .entry(address)
                        .or_default()
                        .insert(RelationKindV1::Resolved);
                }
                Fact::TableEntry { target, .. } => {
                    let address = self.addr(target)?;
                    self.relations
                        .entry(address)
                        .or_default()
                        .insert(RelationKindV1::Table);
                }
                _ => {}
            }
        }

        for bank in &snapshot.banks {
            let bank_id = self.intern_bank(&bank.input.bank)?;
            for (&pc, &class) in &bank.closure.cfg.word_class {
                self.words
                    .entry(CompactAddr { bank: bank_id, pc })
                    .or_default()
                    .insert(class);
            }
            for assessment in &bank.owner_proof.assessments {
                let address = self.addr(assessment.entry())?;
                let (state, mut blockers) = match assessment {
                    OwnerAssessment::Proven { .. } => (OwnerStateV1::Proven, Vec::new()),
                    OwnerAssessment::Candidate { frontier } => (
                        OwnerStateV1::Candidate,
                        frontier
                            .blockers
                            .iter()
                            .map(OwnerBlockerKind::from)
                            .collect(),
                    ),
                    OwnerAssessment::Ambiguous { frontier } => (
                        OwnerStateV1::Ambiguous,
                        frontier
                            .blockers
                            .iter()
                            .map(OwnerBlockerKind::from)
                            .collect(),
                    ),
                };
                blockers.sort_unstable();
                blockers.dedup();
                self.owners.insert(CompactOwner {
                    address,
                    state,
                    blockers,
                });
            }
        }
        Ok(())
    }

    pub fn finalize(self) -> Result<ColdAttributionIndex, AttributionError> {
        // Interning keeps ingestion compact, but ingestion order is not
        // authority. Remap IDs by bank name so snapshot order cannot change
        // report ordering or its canonical digest.
        let mut banks = self.banks.clone();
        banks.sort();
        let canonical_ids: BTreeMap<_, _> = banks
            .iter()
            .enumerate()
            .map(|(index, bank)| {
                u32::try_from(index)
                    .map(|index| (bank.clone(), index))
                    .map_err(|_| AttributionError::CounterOverflow {
                        counter: "canonical_banks",
                    })
            })
            .collect::<Result<_, _>>()?;
        let remap = self
            .banks
            .iter()
            .map(|bank| canonical_ids[bank])
            .collect::<Vec<_>>();
        let remap_addr = |address: CompactAddr| CompactAddr {
            bank: remap[address.bank as usize],
            pc: address.pc,
        };

        let mut mappings = self
            .mappings
            .into_iter()
            .map(|mapping| CompactMapping {
                bank: remap[mapping.bank as usize],
                ..mapping
            })
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>();
        mappings.sort_by_key(|mapping| {
            (
                mapping.rom_start,
                mapping.rom_end,
                mapping.rom_space,
                mapping.va_start,
                mapping.bank,
            )
        });
        let mut mapping_prefix_max_end = Vec::with_capacity(mappings.len());
        let mut max_end = 0u32;
        for mapping in &mappings {
            max_end = max_end.max(mapping.rom_end);
            mapping_prefix_max_end.push(max_end);
        }
        let mut claims =
            BTreeMap::<CompactAddr, BTreeMap<CandidateDetector, BTreeSet<ProofState>>>::new();
        for claim in self.claims {
            claims
                .entry(remap_addr(claim.address))
                .or_default()
                .entry(claim.detector)
                .or_default()
                .insert(claim.state);
        }
        let mut owners = BTreeMap::<CompactAddr, BTreeSet<OwnerObservationV1>>::new();
        for owner in self.owners {
            owners
                .entry(remap_addr(owner.address))
                .or_default()
                .insert(OwnerObservationV1 {
                    state: owner.state,
                    blocker_kinds: owner.blockers,
                });
        }
        let mut conclusions = BTreeMap::<CompactAddr, BTreeSet<ProofState>>::new();
        for (address, states) in self.conclusions {
            conclusions
                .entry(remap_addr(address))
                .or_default()
                .extend(states);
        }
        let mut words = BTreeMap::<CompactAddr, BTreeSet<WordClass>>::new();
        for (address, classes) in self.words {
            words
                .entry(remap_addr(address))
                .or_default()
                .extend(classes);
        }
        let mut relations = BTreeMap::<CompactAddr, BTreeSet<RelationKindV1>>::new();
        for (address, kinds) in self.relations {
            relations
                .entry(remap_addr(address))
                .or_default()
                .extend(kinds);
        }
        Ok(ColdAttributionIndex {
            banks,
            mappings,
            mapping_prefix_max_end,
            claims,
            conclusions,
            words,
            owners,
            relations,
        })
    }
}

#[derive(Debug)]
pub struct ColdAttributionIndex {
    banks: Vec<String>,
    mappings: Vec<CompactMapping>,
    mapping_prefix_max_end: Vec<u32>,
    claims: BTreeMap<CompactAddr, BTreeMap<CandidateDetector, BTreeSet<ProofState>>>,
    conclusions: BTreeMap<CompactAddr, BTreeSet<ProofState>>,
    words: BTreeMap<CompactAddr, BTreeSet<WordClass>>,
    owners: BTreeMap<CompactAddr, BTreeSet<OwnerObservationV1>>,
    relations: BTreeMap<CompactAddr, BTreeSet<RelationKindV1>>,
}

impl ColdAttributionIndex {
    fn resolve(&self, raw_rom: u32, vram: u32) -> Vec<(CompactAddr, ResolvedMappingV1)> {
        let mut result = BTreeSet::new();
        let mut index = self
            .mappings
            .partition_point(|mapping| mapping.rom_start <= raw_rom);
        while index != 0 {
            index -= 1;
            if self.mapping_prefix_max_end[index] <= raw_rom {
                break;
            }
            let mapping = &self.mappings[index];
            if raw_rom < mapping.rom_start
                || raw_rom >= mapping.rom_end
                || vram < mapping.va_start
                || vram >= mapping.va_end
                || raw_rom - mapping.rom_start != vram - mapping.va_start
            {
                continue;
            }
            let bank = self.banks[mapping.bank as usize].clone();
            result.insert((
                CompactAddr {
                    bank: mapping.bank,
                    pc: vram,
                },
                ResolvedMappingV1 {
                    rom_space: mapping.rom_space,
                    rom: raw_rom,
                    bank,
                    vram,
                },
            ));
        }
        result.into_iter().collect()
    }

    fn observations(
        &self,
        resolved: &[(CompactAddr, ResolvedMappingV1)],
        candidate_detectors_by_identity: &BTreeMap<
            AddressedPhysicalEntryV2,
            BTreeSet<CandidateDetector>,
        >,
    ) -> AttributionObservationsV1 {
        let physical: BTreeSet<_> = resolved
            .iter()
            .map(|(_, mapping)| AddressedPhysicalEntryV2 {
                rom_space: mapping.rom_space,
                rom: mapping.rom,
                vram: mapping.vram,
            })
            .collect();
        let mut claims: BTreeMap<CandidateDetector, BTreeSet<ProofState>> = BTreeMap::new();
        let mut conclusion_states = BTreeSet::new();
        let mut word_classes = BTreeSet::new();
        let mut owners = BTreeSet::new();
        let mut relations = BTreeSet::new();
        for (address, _) in resolved {
            if let Some(address_claims) = self.claims.get(address) {
                for (detector, states) in address_claims {
                    claims
                        .entry(*detector)
                        .or_default()
                        .extend(states.iter().copied());
                }
            }
            conclusion_states.extend(self.conclusions.get(address).into_iter().flatten().copied());
            word_classes.extend(self.words.get(address).into_iter().flatten().copied());
            relations.extend(self.relations.get(address).into_iter().flatten().copied());
            owners.extend(self.owners.get(address).into_iter().flatten().cloned());
        }
        let mut candidate_detectors = BTreeSet::new();
        for identity in physical {
            candidate_detectors.extend(
                candidate_detectors_by_identity
                    .get(&identity)
                    .into_iter()
                    .flatten()
                    .copied(),
            );
        }
        AttributionObservationsV1 {
            mappings: resolved
                .iter()
                .map(|(_, mapping)| mapping.clone())
                .collect(),
            claims: claims
                .into_iter()
                .map(|(detector, proposed_states)| ClaimObservationV1 {
                    detector,
                    proposed_states: proposed_states.into_iter().collect(),
                })
                .collect(),
            conclusion_states: conclusion_states.into_iter().collect(),
            word_classes: word_classes.into_iter().collect(),
            owners: owners.into_iter().collect(),
            incoming_relations: relations.into_iter().collect(),
            candidate_detectors: candidate_detectors.into_iter().collect(),
        }
    }

    fn intersect_extent(
        &self,
        raw_start: u32,
        raw_end: u32,
        vram_start: u32,
        vram_end: u32,
    ) -> Vec<AnswerExtent> {
        let mut result = BTreeSet::new();
        let mut index = self
            .mappings
            .partition_point(|mapping| mapping.rom_start < raw_end);
        while index != 0 {
            index -= 1;
            if self.mapping_prefix_max_end[index] <= raw_start {
                break;
            }
            let mapping = &self.mappings[index];
            let overlap_start = raw_start.max(mapping.rom_start);
            let overlap_end = raw_end.min(mapping.rom_end);
            if overlap_start >= overlap_end {
                continue;
            }
            let Some(mapping_vram_start) = mapping
                .va_start
                .checked_add(overlap_start - mapping.rom_start)
            else {
                continue;
            };
            let Some(answer_vram_start) = vram_start.checked_add(overlap_start - raw_start) else {
                continue;
            };
            if mapping_vram_start != answer_vram_start || mapping_vram_start >= vram_end {
                continue;
            }
            let length = overlap_end - overlap_start;
            let Some(overlap_vram_end) = mapping_vram_start.checked_add(length) else {
                continue;
            };
            let clipped_vram_end = overlap_vram_end.min(vram_end);
            let clipped_length = clipped_vram_end - mapping_vram_start;
            result.insert(AnswerExtent {
                rom_space: mapping.rom_space,
                rom_start: overlap_start,
                rom_end: overlap_start + clipped_length,
                vram_start: mapping_vram_start,
                vram_end: clipped_vram_end,
            });
        }
        result.into_iter().collect()
    }
}

fn checked_inc(value: &mut u64, counter: &'static str) -> Result<(), AttributionError> {
    *value = value
        .checked_add(1)
        .ok_or(AttributionError::CounterOverflow { counter })?;
    Ok(())
}

fn add_row_totals(
    totals: &mut AttributionTotalsV1,
    row: &AnswerAttributionV1,
) -> Result<(), AttributionError> {
    checked_inc(&mut totals.raw_rows, "raw_rows")?;
    if row.function.kind == AnswerRowKind::ZeroSizeMarker || row.function.size == 0 {
        checked_inc(&mut totals.marker_rows, "marker_rows")?;
    } else {
        checked_inc(&mut totals.nonzero_rows, "nonzero_rows")?;
        if row.function.kind == AnswerRowKind::Alias {
            checked_inc(&mut totals.alias_rows, "alias_rows")?;
        }
        match row.status {
            AnswerAttributionStatusV1::CandidateMatched => {
                checked_inc(&mut totals.candidate_matched_rows, "candidate_matched_rows")?
            }
            AnswerAttributionStatusV1::Missed { .. } => {
                checked_inc(&mut totals.missed_rows, "missed_rows")?
            }
            AnswerAttributionStatusV1::NotDiscoverableMarker => {}
        }
    }
    Ok(())
}

fn mechanism_key(domain: ExecutionDomain, status: &AnswerAttributionStatusV1) -> String {
    let reason = match status {
        AnswerAttributionStatusV1::CandidateMatched => "candidate_matched",
        AnswerAttributionStatusV1::NotDiscoverableMarker => "marker",
        AnswerAttributionStatusV1::Missed { primary_reason } => match primary_reason {
            MissReasonV1::NoMapping => "no_mapping",
            MissReasonV1::AmbiguousMapping => "ambiguous_mapping",
            MissReasonV1::ExactCandidateNotPromoted => "exact_candidate_not_promoted",
            MissReasonV1::ProvenCodeNoEntry => "proven_code_no_entry",
            MissReasonV1::CandidateCodeNoEntry => "candidate_code_no_entry",
            MissReasonV1::MappedUnreached => "mapped_unreached",
            MissReasonV1::NoRelation => "no_relation",
        },
    };
    format!("{:?}:{reason}", domain).to_ascii_lowercase()
}

fn instance_key(
    mechanism: &str,
    observations: &AttributionObservationsV1,
) -> Result<String, AttributionError> {
    // Addresses and names are intentionally excluded: the instance groups
    // rows sharing the same evidentiary shape, not merely the same location.
    let categorical = (
        observations
            .mappings
            .iter()
            .map(|m| m.rom_space)
            .collect::<Vec<_>>(),
        &observations.claims,
        &observations.conclusion_states,
        &observations.word_classes,
        &observations.owners,
        &observations.incoming_relations,
        &observations.candidate_detectors,
    );
    let bytes = serde_json::to_vec(&categorical)
        .map_err(|error| AttributionError::Serialization(error.to_string()))?;
    Ok(format!("{mechanism}:{:x}", Sha256::digest(bytes)))
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct AnswerExtent {
    rom_space: RomAddressSpace,
    rom_start: u32,
    rom_end: u32,
    vram_start: u32,
    vram_end: u32,
}

#[derive(Debug, Default)]
struct AlignedIntervalIndex {
    ranges: BTreeMap<(RomAddressSpace, i64), Vec<(u32, u32)>>,
}

impl AlignedIntervalIndex {
    fn insert(&mut self, extent: &AnswerExtent) {
        let delta = i64::from(extent.vram_start) - i64::from(extent.rom_start);
        self.ranges
            .entry((extent.rom_space, delta))
            .or_default()
            .push((extent.rom_start, extent.rom_end));
    }

    fn finalize(&mut self) {
        for ranges in self.ranges.values_mut() {
            ranges.sort_unstable();
            let mut merged: Vec<(u32, u32)> = Vec::with_capacity(ranges.len());
            for &(start, end) in ranges.iter() {
                if let Some(last) = merged.last_mut().filter(|last| start <= last.1) {
                    last.1 = last.1.max(end);
                } else {
                    merged.push((start, end));
                }
            }
            *ranges = merged;
        }
    }

    fn contains(&self, candidate: AddressedPhysicalEntryV2, strict_start: bool) -> bool {
        let delta = i64::from(candidate.vram) - i64::from(candidate.rom);
        let Some(ranges) = self.ranges.get(&(candidate.rom_space, delta)) else {
            return false;
        };
        let index = ranges.partition_point(|(start, _)| *start <= candidate.rom);
        index != 0 && {
            let (start, end) = ranges[index - 1];
            candidate.rom < end && (!strict_start || candidate.rom > start)
        }
    }
}

pub fn attribute_known_functions(
    index: &ColdAttributionIndex,
    identities: &ScopedCandidateIdentitiesV3,
    sections: &[AnswerSectionV1],
    functions: &[AnswerFunctionV1],
) -> Result<AttributionReportV1, AttributionError> {
    for (resource, count, limit) in [
        ("answer_sections", sections.len(), MAX_ANSWER_SECTIONS),
        ("answer_rows", functions.len(), MAX_ANSWER_ROWS),
    ] {
        let count = u64::try_from(count)
            .map_err(|_| AttributionError::CounterOverflow { counter: resource })?;
        if count > limit {
            return Err(AttributionError::LimitExceeded {
                resource,
                count,
                limit,
            });
        }
    }
    let mut receipt_rows = u64::try_from(
        identities
            .combined_candidates
            .len()
            .checked_add(identities.combined_ungradable.len())
            .ok_or(AttributionError::CounterOverflow {
                counter: "candidate_receipt_rows",
            })?,
    )
    .map_err(|_| AttributionError::CounterOverflow {
        counter: "candidate_receipt_rows",
    })?;
    for detector in &identities.per_detector {
        for count in [
            detector.candidates.len(),
            detector.ungradable.len(),
            detector.provenance.len(),
        ] {
            receipt_rows = receipt_rows
                .checked_add(u64::try_from(count).map_err(|_| {
                    AttributionError::CounterOverflow {
                        counter: "candidate_receipt_rows",
                    }
                })?)
                .ok_or(AttributionError::CounterOverflow {
                    counter: "candidate_receipt_rows",
                })?;
        }
        for provenance in &detector.provenance {
            receipt_rows = receipt_rows
                .checked_add(u64::try_from(provenance.sources.len()).map_err(|_| {
                    AttributionError::CounterOverflow {
                        counter: "candidate_receipt_rows",
                    }
                })?)
                .ok_or(AttributionError::CounterOverflow {
                    counter: "candidate_receipt_rows",
                })?;
        }
    }
    if receipt_rows > MAX_CANDIDATE_ROWS {
        return Err(AttributionError::LimitExceeded {
            resource: "candidate_receipt_rows",
            count: receipt_rows,
            limit: MAX_CANDIDATE_ROWS,
        });
    }
    let mut section_by_ordinal = BTreeMap::new();
    for section in sections {
        if section_by_ordinal
            .insert(section.raw_ordinal, section)
            .is_some()
        {
            return Err(AttributionError::DuplicateSectionOrdinal {
                raw_ordinal: section.raw_ordinal,
            });
        }
    }
    let mut function_ordinals = BTreeSet::new();
    let combined: BTreeSet<_> = identities.combined_candidates.iter().copied().collect();
    let mut candidate_detectors_by_identity =
        BTreeMap::<AddressedPhysicalEntryV2, BTreeSet<CandidateDetector>>::new();
    for detector in &identities.per_detector {
        for candidate in &detector.candidates {
            candidate_detectors_by_identity
                .entry(*candidate)
                .or_default()
                .insert(detector.detector);
        }
    }
    let mut rows = Vec::with_capacity(functions.len());
    let mut body_index = AlignedIntervalIndex::default();
    let mut section_index = AlignedIntervalIndex::default();
    let mut uniquely_mapped_known_starts = BTreeSet::new();
    let mut ambiguously_mapped_known_starts = BTreeSet::new();
    let mut distinct_bodies = BTreeSet::<(u32, u32)>::new();
    let mut distinct_by_domain: BTreeMap<ExecutionDomain, BTreeSet<(u32, u32)>> = BTreeMap::new();

    for section in sections {
        let raw_end = section.rom_start.checked_add(section.size).ok_or(
            AttributionError::ArithmeticOverflow {
                context: "section ROM extent",
                raw_ordinal: section.raw_ordinal,
            },
        )?;
        let vram_end = section.vram_start.checked_add(section.size).ok_or(
            AttributionError::ArithmeticOverflow {
                context: "section VRAM extent",
                raw_ordinal: section.raw_ordinal,
            },
        )?;
        for extent in
            index.intersect_extent(section.rom_start, raw_end, section.vram_start, vram_end)
        {
            section_index.insert(&extent);
        }
    }

    for function in functions {
        if !function_ordinals.insert(function.raw_ordinal) {
            return Err(AttributionError::DuplicateFunctionOrdinal {
                raw_ordinal: function.raw_ordinal,
            });
        }
        let section = section_by_ordinal
            .get(&function.section_raw_ordinal)
            .ok_or(AttributionError::MissingSection {
                function_ordinal: function.raw_ordinal,
                section_ordinal: function.section_raw_ordinal,
            })?;
        let offset = function.vram.checked_sub(section.vram_start).ok_or(
            AttributionError::FunctionBeforeSection {
                function_ordinal: function.raw_ordinal,
            },
        )?;
        let raw_rom =
            section
                .rom_start
                .checked_add(offset)
                .ok_or(AttributionError::ArithmeticOverflow {
                    context: "function ROM coordinate",
                    raw_ordinal: function.raw_ordinal,
                })?;
        let function_end =
            offset
                .checked_add(function.size)
                .ok_or(AttributionError::ArithmeticOverflow {
                    context: "function extent",
                    raw_ordinal: function.raw_ordinal,
                })?;
        if function_end > section.size {
            return Err(AttributionError::FunctionOutsideSection {
                function_ordinal: function.raw_ordinal,
            });
        }

        let resolved = index.resolve(raw_rom, function.vram);
        let observations = index.observations(&resolved, &candidate_detectors_by_identity);
        let addressed: BTreeSet<_> = observations
            .mappings
            .iter()
            .map(|mapping| AddressedPhysicalEntryV2 {
                rom_space: mapping.rom_space,
                rom: mapping.rom,
                vram: mapping.vram,
            })
            .collect();
        let status = if function.kind == AnswerRowKind::ZeroSizeMarker || function.size == 0 {
            AnswerAttributionStatusV1::NotDiscoverableMarker
        } else if resolved.is_empty() {
            AnswerAttributionStatusV1::Missed {
                primary_reason: MissReasonV1::NoMapping,
            }
        } else if resolved.len() != 1 {
            AnswerAttributionStatusV1::Missed {
                primary_reason: MissReasonV1::AmbiguousMapping,
            }
        } else if addressed.iter().any(|entry| combined.contains(entry)) {
            AnswerAttributionStatusV1::CandidateMatched
        } else if !observations.candidate_detectors.is_empty() {
            AnswerAttributionStatusV1::Missed {
                primary_reason: MissReasonV1::ExactCandidateNotPromoted,
            }
        } else if observations.word_classes.contains(&WordClass::ProvenCode) {
            AnswerAttributionStatusV1::Missed {
                primary_reason: MissReasonV1::ProvenCodeNoEntry,
            }
        } else if observations
            .word_classes
            .contains(&WordClass::CandidateCode)
        {
            AnswerAttributionStatusV1::Missed {
                primary_reason: MissReasonV1::CandidateCodeNoEntry,
            }
        } else if !observations.incoming_relations.is_empty() {
            AnswerAttributionStatusV1::Missed {
                primary_reason: MissReasonV1::MappedUnreached,
            }
        } else {
            AnswerAttributionStatusV1::Missed {
                primary_reason: MissReasonV1::NoRelation,
            }
        };
        let mechanism_cluster_key = mechanism_key(section.execution_domain, &status);
        let instance_cluster_key = instance_key(&mechanism_cluster_key, &observations)?;
        if function.kind != AnswerRowKind::ZeroSizeMarker && function.size != 0 {
            if resolved.len() == 1 {
                uniquely_mapped_known_starts.extend(addressed.iter().copied());
            } else if resolved.len() > 1 {
                ambiguously_mapped_known_starts.extend(addressed.iter().copied());
            }
            distinct_bodies.insert((raw_rom, function.vram));
            distinct_by_domain
                .entry(section.execution_domain)
                .or_default()
                .insert((raw_rom, function.vram));
            let raw_end =
                raw_rom
                    .checked_add(function.size)
                    .ok_or(AttributionError::ArithmeticOverflow {
                        context: "function ROM extent",
                        raw_ordinal: function.raw_ordinal,
                    })?;
            let vram_end = function.vram.checked_add(function.size).ok_or(
                AttributionError::ArithmeticOverflow {
                    context: "function VRAM extent",
                    raw_ordinal: function.raw_ordinal,
                },
            )?;
            for extent in index.intersect_extent(raw_rom, raw_end, function.vram, vram_end) {
                body_index.insert(&extent);
            }
        }
        rows.push(AnswerAttributionV1 {
            function: function.clone(),
            execution_domain: section.execution_domain,
            raw_rom,
            status,
            observations,
            mechanism_cluster_key,
            instance_cluster_key,
        });
    }
    rows.sort_by_key(|row| row.function.raw_ordinal);
    body_index.finalize();
    section_index.finalize();

    #[derive(Default)]
    struct CandidateAccumulator {
        combined: bool,
        detectors: BTreeSet<CandidateDetector>,
        sources: BTreeMap<CandidateDetector, BTreeSet<AddressedPhysicalEntryV2>>,
    }
    let mut candidate_union =
        BTreeMap::<CandidateAccountingIdentityV1, CandidateAccumulator>::new();
    for candidate in &identities.combined_candidates {
        candidate_union
            .entry(CandidateAccountingIdentityV1::Addressed { entry: *candidate })
            .or_default()
            .combined = true;
    }
    for address in &identities.combined_ungradable {
        candidate_union
            .entry(CandidateAccountingIdentityV1::Ungradable {
                address: address.clone(),
            })
            .or_default()
            .combined = true;
    }
    for detector in &identities.per_detector {
        for candidate in &detector.candidates {
            candidate_union
                .entry(CandidateAccountingIdentityV1::Addressed { entry: *candidate })
                .or_default()
                .detectors
                .insert(detector.detector);
        }
        for address in &detector.ungradable {
            candidate_union
                .entry(CandidateAccountingIdentityV1::Ungradable {
                    address: address.clone(),
                })
                .or_default()
                .detectors
                .insert(detector.detector);
        }
        for provenance in &detector.provenance {
            let accumulator = candidate_union
                .entry(CandidateAccountingIdentityV1::Addressed {
                    entry: provenance.candidate,
                })
                .or_default();
            accumulator.detectors.insert(detector.detector);
            accumulator
                .sources
                .entry(detector.detector)
                .or_default()
                .extend(provenance.sources.iter().copied());
        }
    }

    let mut candidate_totals = CandidateAccountingTotalsV1::default();
    candidate_totals.denominator =
        u64::try_from(candidate_union.len()).map_err(|_| AttributionError::CounterOverflow {
            counter: "candidate_denominator",
        })?;
    let mut candidate_statuses = Vec::with_capacity(candidate_union.len());
    for (identity, accumulator) in candidate_union {
        let status = match &identity {
            CandidateAccountingIdentityV1::Ungradable { .. } => CandidateStatusV1::Ungradable,
            CandidateAccountingIdentityV1::Addressed { entry } => {
                if ambiguously_mapped_known_starts.contains(entry) {
                    CandidateStatusV1::AmbiguousAnswerMapping
                } else if uniquely_mapped_known_starts.contains(entry) {
                    CandidateStatusV1::CandidateMatched
                } else if body_index.contains(*entry, true) {
                    CandidateStatusV1::Interior
                } else if section_index.contains(*entry, false) {
                    CandidateStatusV1::Gap
                } else {
                    CandidateStatusV1::Outside
                }
            }
        };
        match status {
            CandidateStatusV1::CandidateMatched => {
                checked_inc(&mut candidate_totals.candidate_matched, "candidate_matched")?
            }
            CandidateStatusV1::AmbiguousAnswerMapping => checked_inc(
                &mut candidate_totals.ambiguous_answer_mapping,
                "ambiguous_answer_mapping",
            )?,
            CandidateStatusV1::Interior => {
                checked_inc(&mut candidate_totals.interior, "candidate_interior")?
            }
            CandidateStatusV1::Gap => checked_inc(&mut candidate_totals.gap, "candidate_gap")?,
            CandidateStatusV1::Outside => {
                checked_inc(&mut candidate_totals.outside, "candidate_outside")?
            }
            CandidateStatusV1::Ungradable => {
                checked_inc(&mut candidate_totals.ungradable, "candidate_ungradable")?
            }
        }
        if !matches!(status, CandidateStatusV1::Ungradable) {
            checked_inc(&mut candidate_totals.gradable, "candidate_gradable")?;
        }
        if accumulator.combined {
            checked_inc(&mut candidate_totals.combined, "candidate_combined")?;
        } else {
            checked_inc(
                &mut candidate_totals.per_detector_only,
                "candidate_per_detector_only",
            )?;
        }
        candidate_statuses.push(CandidateAttributionV1 {
            identity,
            combined: accumulator.combined,
            detectors: accumulator.detectors.into_iter().collect(),
            detector_sources: accumulator
                .sources
                .into_iter()
                .map(|(detector, sources)| CandidateDetectorSourcesV1 {
                    detector,
                    sources: sources.into_iter().collect(),
                })
                .collect(),
            status,
        });
    }

    let mut totals = AttributionTotalsV1::default();
    let mut per_domain_map = BTreeMap::<ExecutionDomain, AttributionTotalsV1>::new();
    for row in &rows {
        add_row_totals(&mut totals, row)?;
        add_row_totals(per_domain_map.entry(row.execution_domain).or_default(), row)?;
    }
    totals.distinct_bodies =
        u64::try_from(distinct_bodies.len()).map_err(|_| AttributionError::CounterOverflow {
            counter: "distinct_bodies",
        })?;
    for (domain, distinct) in &distinct_by_domain {
        per_domain_map.entry(*domain).or_default().distinct_bodies = u64::try_from(distinct.len())
            .map_err(|_| AttributionError::CounterOverflow {
                counter: "per_domain.distinct_bodies",
            })?;
    }
    let per_domain = per_domain_map
        .into_iter()
        .map(|(execution_domain, totals)| DomainTotalsV1 {
            execution_domain,
            totals,
        })
        .collect::<Vec<_>>();
    let mut canonical_sections = sections.to_vec();
    canonical_sections.sort_by_key(|section| section.raw_ordinal);

    let mut report = AttributionReportV1 {
        schema_version: MISSED_FUNCTION_ATTRIBUTION_SCHEMA_V1,
        sections: canonical_sections,
        rows,
        candidate_statuses,
        candidate_totals,
        totals,
        per_domain,
        canonical_sha256: String::new(),
    };
    report.canonical_sha256 = canonical_attribution_report_digest(&report)?;
    Ok(report)
}

#[cfg(test)]
mod tests;
