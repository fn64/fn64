//! Typed representative-scenario matrix over deterministic release reports.
//!
//! The manifest contains only the immutable project profile and evidence
//! identities, never ROM bytes, captured output, or caller-authored coverage.
//! Dynamic evidence requires schema-v28 report series; coverage is derived
//! from each validated report before it is compared with the fixed profile.

use crate::platform_certification::{
    PlatformCertificationError, VerifiedRt64PlatformCaseAuthority,
};
use crate::{
    verify_release_evidence_series, ArtifactDigest, ArtifactKind, CertificationProfileIdentity,
    CertificationRequirementClass, CertificationRequirementRef, ClosurePath, ClosurePathStatus,
    DeterministicDigest, ExecutionDestinationEvidence, ExecutionDestinationSource,
    FramebufferObservationSource, FullParityV1, ParsedUnsupportedJournal, ReleaseCartridgeSave,
    ReleaseControllerPort, ReleaseEnvironmentEvidence, ReleaseGateReport, ReleaseGraphicsApi,
    ReleaseGraphicsExecutionPolicy, ReleaseHostPlatform, ReleaseMicrocodeFamily,
    ReleaseObservationGeometry, ReleaseRendererEvidence, ReleaseRomClass, ReleaseRomEvidence,
    ReleaseTvRegion, ReleaseWindowsFamily, ReportSeriesError, RspRdpEvidence,
    RspRdpObservationKindEvidence, Rt64PlatformCase, Rt64PlatformTarget,
    VerifiedPrivateReleaseSeries, VerifiedRt64PlatformCaseSeries, LIVE_MINIMUM_CLOSURE_PATHS,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    fmt,
};

pub const RELEASE_MATRIX_SCHEMA: &str = "fn64.release-matrix.v5";
pub const VERIFIED_RELEASE_MATRIX_SCHEMA: &str = "fn64.verified-release-matrix.v18";
pub const INCOMPLETE_RELEASE_MATRIX_SCHEMA: &str = "fn64.release-matrix-incomplete.v7";
pub const VERIFIED_ROM_CLASS_AUTHORITY_SCHEMA: &str = "fn64.verified-rom-class-authority.v1";
pub const RELEASE_MATRIX_REPORT_COUNT: usize = 10;
pub const RELEASE_MATRIX_MAX_SCENARIOS: usize = 64;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum ReleasePlatform {
    #[serde(rename = "macos_arm64")]
    MacosArm64,
    #[serde(rename = "linux_x86_64")]
    LinuxX86_64,
    #[serde(rename = "windows_x86_64")]
    WindowsX86_64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum ControllerFeature {
    #[serde(rename = "standard_controller")]
    StandardController,
    #[serde(rename = "controller_pak")]
    ControllerPak,
    #[serde(rename = "rumble_pak")]
    RumblePak,
    #[serde(rename = "transfer_pak")]
    TransferPak,
    #[serde(rename = "voice_recognition_unit")]
    VoiceRecognitionUnit,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum SaveFeature {
    #[serde(rename = "no_cartridge_save")]
    NoCartridgeSave,
    #[serde(rename = "eeprom_4_kbit")]
    Eeprom4Kbit,
    #[serde(rename = "eeprom_16_kbit")]
    Eeprom16Kbit,
    #[serde(rename = "sram_32_kib")]
    Sram32Kib,
    #[serde(rename = "flash_ram_128_kib")]
    FlashRam128Kib,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum RendererFeature {
    #[serde(rename = "reference_lle_accuracy")]
    ReferenceLleAccuracy,
    #[serde(rename = "rt64_lle_accuracy")]
    Rt64LleAccuracy,
    #[serde(rename = "rt64_post_vi_capture")]
    Rt64PostViCapture,
    #[serde(rename = "rt64_replacement_packs")]
    Rt64ReplacementPacks,
}

/// Executable-entry authority carried by every report in one representative
/// scenario. This prevents a manifest label from substituting one program
/// lane for another after the report has been captured.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum ProgramFeature {
    #[serde(rename = "native_archive")]
    NativeArchive,
    #[serde(rename = "typed_observed_function")]
    TypedObservedFunction,
    #[serde(rename = "typed_block")]
    TypedBlock,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MicrocodeFeature {
    Fast3d,
    F3dex,
    F3dlx,
    F3dlxRej,
    F3dex2,
    F3dex2NoN,
    F3dex2Rej,
    F3dlx2Rej,
    S2dex,
    S2dex2,
    L3dex,
    L3dex2,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct CertifiedMicrocodeIdentity {
    text_sha256: [u8; 32],
    family: ReleaseMicrocodeFamily,
}

/// Immutable digest-to-family adjudication for matrix-v5/v15 certification.
///
/// Runtime/backend admission remains host-configurable because it selects an
/// optimization, not certification. Public-microcode denominator credit is
/// intentionally empty until allowed-source digest provenance is reviewed and
/// lands in a new, schema-versioned project catalog.
const CERTIFIED_PUBLIC_MICROCODE_CATALOG_V1: &[CertifiedMicrocodeIdentity] = &[];

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RspRdpMechanismFeature {
    DramDpc,
    XbusDpc,
    ImemReplacement,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReleaseMatrixCoverage {
    /// Empty unless an admission-bound private run contract authorizes the
    /// report's exact ROM class. A report field or scenario label alone never
    /// populates this dimension.
    pub rom_classes: Vec<ReleaseRomClass>,
    /// Empty for synthetic, unclassified-header, or region-free evidence.
    /// Only a fixed destination code co-bound to device and renderer TV
    /// authority can satisfy a profile TV-region row.
    pub tv_regions: Vec<ReleaseTvRegion>,
    pub platforms: Vec<ReleasePlatform>,
    pub controllers: Vec<ControllerFeature>,
    pub saves: Vec<SaveFeature>,
    pub renderers: Vec<RendererFeature>,
    pub programs: Vec<ProgramFeature>,
    /// Empty is valid evidence that no publicly admitted family was reached;
    /// it satisfies no profile requirement.
    pub microcodes: Vec<MicrocodeFeature>,
    /// Empty is valid evidence that none of the required mechanisms committed.
    pub rsp_rdp_mechanisms: Vec<RspRdpMechanismFeature>,
}

/// Canonical retained proof that one opaque, revalidated private release
/// series authorized the ROM class attached to the same report series.
///
/// This record deliberately retains identities rather than private paths. Its
/// own digest is the evidence identity assigned to the ROM-class profile row,
/// so an incomplete matrix still binds the authority that earned that credit.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VerifiedRomClassAuthority {
    pub schema: String,
    pub contract_schema: String,
    pub contract_sha256: String,
    pub receipt_schema: String,
    pub receipt_sha256: String,
    pub runner_executable_sha256: String,
    pub purpose: String,
    pub report_scenario: String,
    pub input_sha256: String,
    pub input_bytes: u64,
    pub rom_class: ReleaseRomClass,
    pub guest_cycle: u64,
    pub expected_execution_source: ExecutionDestinationSource,
    pub child_executable_sha256: String,
    pub semantic_report_sha256: String,
    pub run_event_sha256s: Vec<String>,
    pub authority_sha256: String,
}

impl VerifiedRomClassAuthority {
    fn recompute_authority_sha256(&self) -> String {
        let mut wire = Vec::new();
        wire.extend_from_slice(b"fn64.verified-rom-class-authority.v1\0");
        push_bytes(&mut wire, self.schema.as_bytes());
        push_bytes(&mut wire, self.contract_schema.as_bytes());
        push_bytes(&mut wire, self.contract_sha256.as_bytes());
        push_bytes(&mut wire, self.receipt_schema.as_bytes());
        push_bytes(&mut wire, self.receipt_sha256.as_bytes());
        push_bytes(&mut wire, self.runner_executable_sha256.as_bytes());
        push_bytes(&mut wire, self.purpose.as_bytes());
        push_bytes(&mut wire, self.report_scenario.as_bytes());
        push_bytes(&mut wire, self.input_sha256.as_bytes());
        wire.extend_from_slice(&self.input_bytes.to_be_bytes());
        push_bytes(&mut wire, self.rom_class.wire_name().as_bytes());
        wire.extend_from_slice(&self.guest_cycle.to_be_bytes());
        push_execution_source(&mut wire, &self.expected_execution_source);
        push_bytes(&mut wire, self.child_executable_sha256.as_bytes());
        push_bytes(&mut wire, self.semantic_report_sha256.as_bytes());
        wire.extend_from_slice(&(self.run_event_sha256s.len() as u64).to_be_bytes());
        for run_event_sha256 in &self.run_event_sha256s {
            push_bytes(&mut wire, run_event_sha256.as_bytes());
        }
        hex(&Sha256::digest(wire))
    }

    fn verify_integrity(&self, id: &str) -> Result<(), ReleaseMatrixError> {
        if self.schema != VERIFIED_ROM_CLASS_AUTHORITY_SCHEMA {
            return Err(ReleaseMatrixError::InvalidRomClassAuthority {
                id: id.to_owned(),
                detail: format!("unsupported authority schema {:?}", self.schema),
            });
        }
        if self.contract_schema != crate::PRIVATE_RELEASE_RUN_CONTRACT_SCHEMA {
            return Err(ReleaseMatrixError::InvalidRomClassAuthority {
                id: id.to_owned(),
                detail: format!("unsupported contract schema {:?}", self.contract_schema),
            });
        }
        if self.receipt_schema != crate::PRIVATE_RELEASE_SERIES_RECEIPT_SCHEMA {
            return Err(ReleaseMatrixError::InvalidRomClassAuthority {
                id: id.to_owned(),
                detail: format!("unsupported receipt schema {:?}", self.receipt_schema),
            });
        }
        for (field, value) in [
            ("contract_sha256", self.contract_sha256.as_str()),
            ("receipt_sha256", self.receipt_sha256.as_str()),
            (
                "runner_executable_sha256",
                self.runner_executable_sha256.as_str(),
            ),
            ("input_sha256", self.input_sha256.as_str()),
            (
                "child_executable_sha256",
                self.child_executable_sha256.as_str(),
            ),
            (
                "semantic_report_sha256",
                self.semantic_report_sha256.as_str(),
            ),
            ("authority_sha256", self.authority_sha256.as_str()),
        ] {
            validate_sha256(id, field, value)?;
        }
        validate_execution_source_identity(id, &self.expected_execution_source)?;
        if !matches!(self.purpose.as_str(), "full_rom" | "combined")
            || self.rom_class == ReleaseRomClass::Unclassified
        {
            return Err(ReleaseMatrixError::InvalidRomClassAuthority {
                id: id.to_owned(),
                detail: "ROM-class authority must be a classified full_rom or combined contract"
                    .to_owned(),
            });
        }
        if self.run_event_sha256s.len() != RELEASE_MATRIX_REPORT_COUNT {
            return Err(ReleaseMatrixError::InvalidRomClassAuthority {
                id: id.to_owned(),
                detail: format!(
                    "ROM-class authority retains {} run-event identities; exactly {} are required",
                    self.run_event_sha256s.len(),
                    RELEASE_MATRIX_REPORT_COUNT
                ),
            });
        }
        let mut unique_run_events = BTreeSet::new();
        for run_event_sha256 in &self.run_event_sha256s {
            validate_sha256(id, "authority.run_event_sha256s[]", run_event_sha256)?;
            if !unique_run_events.insert(run_event_sha256) {
                return Err(ReleaseMatrixError::InvalidRomClassAuthority {
                    id: id.to_owned(),
                    detail: format!(
                        "ROM-class authority repeats run-event identity {run_event_sha256}"
                    ),
                });
            }
        }
        let recomputed = self.recompute_authority_sha256();
        if self.authority_sha256 != recomputed {
            return Err(ReleaseMatrixError::InvalidRomClassAuthority {
                id: id.to_owned(),
                detail: format!(
                    "authority digest mismatch: stored {}, recomputed {recomputed}",
                    self.authority_sha256
                ),
            });
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReleaseMatrixScenario {
    /// Stable diagnostic key. Evidence is associated by `report_scenario`, not
    /// by a caller-provided command-line assignment.
    pub id: String,
    /// Exact scenario string bound by every schema-v28 report in this series.
    pub report_scenario: String,
    /// Exact private-input identity bound by every report; no input bytes are stored.
    pub input_sha256: String,
    pub report_sha256: String,
    /// Canonical digest over this declaration and its exact v20 evidence IDs.
    pub declaration_sha256: String,
}

impl ReleaseMatrixScenario {
    /// Recompute the declaration digest under the v5 evidence-only wire.
    pub fn recompute_declaration_sha256(&self) -> String {
        let mut wire = Vec::new();
        wire.extend_from_slice(b"fn64.release-matrix.scenario.v5\0");
        push_bytes(&mut wire, self.id.as_bytes());
        push_bytes(&mut wire, self.report_scenario.as_bytes());
        push_bytes(&mut wire, self.input_sha256.as_bytes());
        push_bytes(&mut wire, self.report_sha256.as_bytes());
        hex(&Sha256::digest(wire))
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReleaseMatrixManifest {
    pub schema: String,
    pub profile: CertificationProfileIdentity,
    pub scenarios: Vec<ReleaseMatrixScenario>,
}

impl ReleaseMatrixManifest {
    /// Canonical identity for the complete policy declaration and its bound
    /// per-scenario evidence identities. No ROM or captured bytes enter it.
    pub fn recompute_manifest_sha256(&self) -> String {
        let mut wire = Vec::new();
        wire.extend_from_slice(b"fn64.release-matrix.manifest.v5\0");
        push_bytes(&mut wire, self.schema.as_bytes());
        push_bytes(&mut wire, self.profile.schema.as_bytes());
        push_bytes(&mut wire, self.profile.definition_sha256.as_bytes());
        let mut scenarios: Vec<_> = self.scenarios.iter().collect();
        scenarios.sort_by(|left, right| left.id.cmp(&right.id));
        wire.extend_from_slice(&(scenarios.len() as u32).to_be_bytes());
        for scenario in scenarios {
            push_bytes(&mut wire, scenario.id.as_bytes());
            push_bytes(&mut wire, scenario.declaration_sha256.as_bytes());
        }
        hex(&Sha256::digest(wire))
    }
}

/// Presentation proof carried by the live fixed-cycle capture that produced
/// the scenario reports.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PresentationBoundaryEvidence {
    /// Capture consumed the opaque device-scheduled VI boundary at this cycle.
    CommittedViBoundary,
    /// The RT64 post-VI envelope additionally named this exact presentation cycle.
    ExactPostViCapture,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VerifiedMatrixScenario {
    pub id: String,
    pub count: usize,
    pub report_sha256: String,
    pub report_scenario: String,
    pub input_sha256: String,
    pub rom: Option<ReleaseRomEvidence>,
    pub rom_class_authority: Option<VerifiedRomClassAuthority>,
    /// Coverage assignments retained from the verified manifest. These make
    /// the retained matrix independently auditable without the manifest.
    pub coverage: ReleaseMatrixCoverage,
    pub declaration_sha256: String,
    pub guest_cycle: u64,
    pub fixed_cycle_digest: DeterministicDigest,
    pub observations: ReleaseObservationGeometry,
    pub environment: ReleaseEnvironmentEvidence,
    pub closure_paths: u64,
    /// Exact destination sequence and canonical unique/count summary retained
    /// from the verified v28 series.
    pub execution_destinations: ExecutionDestinationEvidence,
    /// Complete schema-v28 RSP/RDP observation stream retained for independent
    /// report reconstruction and coverage derivation.
    pub rsp_rdp: RspRdpEvidence,
    pub unsupported_instrumentation: crate::UnsupportedInstrumentationEvidence,
    /// Exact canonical closure ledger retained from the verified v28 series.
    /// A count alone cannot prove which feature-specific operation paths ran.
    pub closure: Vec<ClosurePath>,
    pub unsupported_events: u64,
    pub unsupported_journal_schema: String,
    pub bound_journals: usize,
    /// Canonical caller-supplied execution identities in retained run order.
    pub run_event_sha256s: Vec<String>,
    pub presentation_boundary: PresentationBoundaryEvidence,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VerifiedReleaseMatrix {
    pub schema: String,
    pub manifest_sha256: String,
    pub profile: CertificationProfileIdentity,
    pub total_reports: usize,
    pub scenarios: Vec<VerifiedMatrixScenario>,
    /// Opaque local process authorities projected into retained integrity
    /// evidence. Deserializing these records cannot recreate the capability.
    pub platform_case_authorities: Vec<VerifiedRt64PlatformCaseAuthority>,
    /// Every project-owned profile requirement and the exact validated
    /// evidence declarations that satisfy it. A complete retained matrix has
    /// no unassigned profile member.
    pub assignments: Vec<CertificationRequirementAssignment>,
    /// Canonical digest over this retained verification result.
    pub verification_sha256: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CertificationRequirementAssignment {
    pub requirement: CertificationRequirementRef,
    pub evidence_sha256s: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IncompleteReleaseMatrix {
    pub schema: String,
    pub manifest_sha256: String,
    pub profile: CertificationProfileIdentity,
    pub verified_scenarios: usize,
    pub verified_reports: usize,
    pub unsupported_instrumentation: crate::UnsupportedInstrumentationEvidence,
    pub platform_case_authorities: Vec<VerifiedRt64PlatformCaseAuthority>,
    pub satisfied: Vec<CertificationRequirementAssignment>,
    pub missing: Vec<CertificationRequirementRef>,
    pub assessment_sha256: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", content = "result", rename_all = "snake_case")]
pub enum ReleaseMatrixVerification {
    Complete(VerifiedReleaseMatrix),
    Incomplete(IncompleteReleaseMatrix),
}

impl IncompleteReleaseMatrix {
    pub fn verify_integrity(&self) -> Result<(), ReleaseMatrixError> {
        if self.schema != INCOMPLETE_RELEASE_MATRIX_SCHEMA {
            return Err(ReleaseMatrixError::UnsupportedIncompleteSchema(
                self.schema.clone(),
            ));
        }
        let profile = self
            .profile
            .verify()
            .map_err(ReleaseMatrixError::InvalidCertificationProfile)?;
        self.unsupported_instrumentation
            .verify_current()
            .map_err(ReleaseMatrixError::InvalidUnsupportedInstrumentation)?;
        validate_sha256(
            "incomplete-matrix",
            "manifest_sha256",
            &self.manifest_sha256,
        )?;
        if self.verified_scenarios == 0
            || self.verified_scenarios > RELEASE_MATRIX_MAX_SCENARIOS
            || self.verified_reports != self.verified_scenarios * RELEASE_MATRIX_REPORT_COUNT
        {
            return Err(ReleaseMatrixError::InvalidIncompleteCounts {
                scenarios: self.verified_scenarios,
                reports: self.verified_reports,
            });
        }
        validate_assignment_partition(profile, &self.satisfied, &self.missing, false)?;
        validate_incomplete_platform_authority_assignments(
            &self.platform_case_authorities,
            &self.satisfied,
        )?;
        if self.missing.is_empty() {
            return Err(ReleaseMatrixError::IncompleteWithoutMissing);
        }
        validate_sha256(
            "incomplete-matrix",
            "assessment_sha256",
            &self.assessment_sha256,
        )?;
        let recomputed = incomplete_matrix_sha256(self);
        if self.assessment_sha256 != recomputed {
            return Err(ReleaseMatrixError::IncompleteIntegrityMismatch {
                stored: self.assessment_sha256.clone(),
                recomputed,
            });
        }
        Ok(())
    }
}

fn validate_incomplete_platform_authority_assignments(
    authorities: &[VerifiedRt64PlatformCaseAuthority],
    assignments: &[CertificationRequirementAssignment],
) -> Result<(), ReleaseMatrixError> {
    let mut seen = BTreeSet::new();
    for authority in authorities {
        authority
            .verify_integrity()
            .map_err(|source| ReleaseMatrixError::InvalidPlatformSeriesAuthority { source })?;
        if !seen.insert((authority.target, authority.case)) {
            return Err(ReleaseMatrixError::DuplicatePlatformSeriesAuthority {
                target: authority.target.id().to_owned(),
                case: authority.case.id().to_owned(),
            });
        }
        let id = format!("{}/{}", authority.target.id(), authority.case.id());
        let assignment = assignments.iter().find(|assignment| {
            assignment.requirement.class() == CertificationRequirementClass::Rt64TargetCase
                && assignment.requirement.id() == id
        });
        if assignment.is_none_or(|assignment| {
            assignment.evidence_sha256s != [authority.authority_sha256.clone()]
        }) {
            return Err(ReleaseMatrixError::PlatformAuthorityAssignmentMismatch {
                target: authority.target.id().to_owned(),
                case: authority.case.id().to_owned(),
            });
        }
    }
    for assignment in assignments.iter().filter(|assignment| {
        assignment.requirement.class() == CertificationRequirementClass::Rt64TargetCase
    }) {
        let id = assignment.requirement.id();
        let authority = authorities
            .iter()
            .find(|authority| id == format!("{}/{}", authority.target.id(), authority.case.id()));
        if authority.is_none_or(|authority| {
            assignment.evidence_sha256s != [authority.authority_sha256.clone()]
        }) {
            let (target, case) = id.split_once('/').unwrap_or((id, ""));
            return Err(ReleaseMatrixError::PlatformAuthorityAssignmentMismatch {
                target: target.to_owned(),
                case: case.to_owned(),
            });
        }
    }
    Ok(())
}

impl VerifiedReleaseMatrix {
    pub fn verify_integrity(&self) -> Result<(), ReleaseMatrixError> {
        if self.schema != VERIFIED_RELEASE_MATRIX_SCHEMA {
            return Err(ReleaseMatrixError::UnsupportedVerifiedSchema(
                self.schema.clone(),
            ));
        }
        let profile = self
            .profile
            .verify()
            .map_err(ReleaseMatrixError::InvalidCertificationProfile)?;
        if self.scenarios.is_empty() || self.scenarios.len() > RELEASE_MATRIX_MAX_SCENARIOS {
            return Err(ReleaseMatrixError::ScenarioCount {
                minimum: 1,
                maximum: RELEASE_MATRIX_MAX_SCENARIOS,
                actual: self.scenarios.len(),
            });
        }
        validate_sha256("verified-matrix", "manifest_sha256", &self.manifest_sha256)?;

        let mut ids = BTreeSet::new();
        let mut report_scenarios = BTreeSet::new();
        let mut matrix_run_events = BTreeSet::new();
        let expected_artifacts = BTreeSet::from([
            ArtifactKind::Framebuffer,
            ArtifactKind::Audio,
            ArtifactKind::Memory,
            ArtifactKind::DeviceState,
            ArtifactKind::TimingTrace,
        ]);
        for scenario in &self.scenarios {
            scenario
                .unsupported_instrumentation
                .verify_current()
                .map_err(ReleaseMatrixError::InvalidUnsupportedInstrumentation)?;
            validate_id(&scenario.id)?;
            if !ids.insert(scenario.id.clone()) {
                return Err(ReleaseMatrixError::DuplicateScenarioId(scenario.id.clone()));
            }
            if scenario.report_scenario.is_empty()
                || scenario.report_scenario.len() > 256
                || scenario.report_scenario.chars().any(char::is_control)
            {
                return Err(ReleaseMatrixError::InvalidReportScenario {
                    id: scenario.id.clone(),
                });
            }
            if !report_scenarios.insert(scenario.report_scenario.clone()) {
                return Err(ReleaseMatrixError::DuplicateReportScenario(
                    scenario.report_scenario.clone(),
                ));
            }
            validate_sha256(&scenario.id, "input_sha256", &scenario.input_sha256)?;
            validate_sha256(&scenario.id, "report_sha256", &scenario.report_sha256)?;
            validate_coverage(&scenario.id, &scenario.coverage, None)?;
            let declaration = retained_scenario_declaration(scenario);
            validate_coverage_cardinality(&scenario.id, &scenario.coverage)?;
            validate_sha256(
                &scenario.id,
                "declaration_sha256",
                &scenario.declaration_sha256,
            )?;
            let recomputed_declaration = declaration.recompute_declaration_sha256();
            if scenario.declaration_sha256 != recomputed_declaration {
                return Err(ReleaseMatrixError::DeclarationDigestMismatch {
                    id: scenario.id.clone(),
                    stored: scenario.declaration_sha256.clone(),
                    recomputed: recomputed_declaration,
                });
            }
            scenario.observations.validate().map_err(|source| {
                ReleaseMatrixError::InvalidVerifiedObservations {
                    id: scenario.id.clone(),
                    source,
                }
            })?;
            scenario
                .execution_destinations
                .verify_integrity()
                .map_err(|source| ReleaseMatrixError::InvalidVerifiedDestinations {
                    id: scenario.id.clone(),
                    source,
                })?;
            let expected_boundary = presentation_boundary(&scenario.observations);
            if scenario.presentation_boundary != expected_boundary {
                return Err(ReleaseMatrixError::VerifiedPresentationMismatch {
                    id: scenario.id.clone(),
                    stored: scenario.presentation_boundary,
                    expected: expected_boundary,
                });
            }
            if scenario.unsupported_journal_schema != "fn64.unsupported-journal.v3"
                || scenario.bound_journals != scenario.count
            {
                return Err(ReleaseMatrixError::VerifiedJournalBinding {
                    id: scenario.id.clone(),
                    schema: scenario.unsupported_journal_schema.clone(),
                    reports: scenario.count,
                    journals: scenario.bound_journals,
                });
            }
            let mut unique_run_events = BTreeSet::new();
            for run_event_sha256 in &scenario.run_event_sha256s {
                validate_sha256(&scenario.id, "run_event_sha256s", run_event_sha256)?;
                unique_run_events.insert(run_event_sha256);
                if !matrix_run_events.insert(run_event_sha256) {
                    return Err(ReleaseMatrixError::DuplicateRunEventIdentity {
                        id: scenario.id.clone(),
                        run_event_sha256: run_event_sha256.clone(),
                    });
                }
            }
            if scenario.run_event_sha256s.len() != scenario.count
                || unique_run_events.len() != scenario.count
            {
                return Err(ReleaseMatrixError::VerifiedRunEventIdentities {
                    id: scenario.id.clone(),
                    reports: scenario.count,
                    identities: scenario.run_event_sha256s.len(),
                    unique: unique_run_events.len(),
                });
            }
            if scenario.count != RELEASE_MATRIX_REPORT_COUNT {
                return Err(ReleaseMatrixError::VerifiedScenarioReportCount {
                    id: scenario.id.clone(),
                    expected: RELEASE_MATRIX_REPORT_COUNT,
                    actual: scenario.count,
                });
            }
            if scenario.guest_cycle != scenario.fixed_cycle_digest.guest_cycle {
                return Err(ReleaseMatrixError::VerifiedCycleMismatch {
                    id: scenario.id.clone(),
                    scenario_cycle: scenario.guest_cycle,
                    digest_cycle: scenario.fixed_cycle_digest.guest_cycle,
                });
            }
            let artifacts: BTreeSet<_> = scenario
                .fixed_cycle_digest
                .artifacts
                .iter()
                .map(|artifact| artifact.kind)
                .collect();
            if artifacts != expected_artifacts
                || scenario.fixed_cycle_digest.artifacts.len() != expected_artifacts.len()
            {
                return Err(ReleaseMatrixError::VerifiedArtifactSet {
                    id: scenario.id.clone(),
                });
            }
            scenario
                .fixed_cycle_digest
                .verify_integrity()
                .and_then(|()| {
                    crate::release_gate::validate_artifact_observation_bytes(
                        &scenario.fixed_cycle_digest,
                        &scenario.observations,
                    )
                })
                .map_err(|source| ReleaseMatrixError::InvalidVerifiedDigest {
                    id: scenario.id.clone(),
                    source,
                })?;
            if let FramebufferObservationSource::PostViSwapchain {
                backend_identity, ..
            } = &scenario.observations.framebuffer.source
            {
                let ReleaseRendererEvidence::Rt64 { graphics_api, .. } =
                    &scenario.environment.renderer
                else {
                    return Err(ReleaseMatrixError::RendererEnvironmentMismatch {
                        id: scenario.id.clone(),
                    });
                };
                if crate::render_evidence::validate_authoritative_rt64_backend_identity(
                    backend_identity,
                    scenario.environment.platform,
                    *graphics_api,
                )
                .is_err()
                {
                    return Err(ReleaseMatrixError::NonAuthoritativeRt64Identity {
                        id: scenario.id.clone(),
                        backend_identity: backend_identity.clone(),
                    });
                }
            }
            let observed_closure_paths = scenario.closure.len() as u64;
            if scenario.closure_paths != observed_closure_paths {
                return Err(ReleaseMatrixError::VerifiedClosurePathCountMismatch {
                    id: scenario.id.clone(),
                    stored: scenario.closure_paths,
                    observed: observed_closure_paths,
                });
            }
            validate_retained_closure(&scenario.id, &scenario.closure)?;
            validate_feature_operation_paths(&scenario.id, &scenario.coverage, &scenario.closure)?;
            let observed_unsupported = scenario
                .closure
                .iter()
                .map(|path| path.unsupported.len() as u64)
                .sum();
            if scenario.unsupported_events != observed_unsupported {
                return Err(ReleaseMatrixError::VerifiedUnsupportedEventCountMismatch {
                    id: scenario.id.clone(),
                    stored: scenario.unsupported_events,
                    observed: observed_unsupported,
                });
            }
            if scenario.unsupported_events != 0 {
                return Err(ReleaseMatrixError::VerifiedUnsupportedEvents {
                    id: scenario.id.clone(),
                    count: scenario.unsupported_events,
                });
            }
            let minimum_paths = LIVE_MINIMUM_CLOSURE_PATHS.len() as u64;
            if scenario.closure_paths < minimum_paths {
                return Err(ReleaseMatrixError::VerifiedClosurePathCount {
                    id: scenario.id.clone(),
                    minimum: minimum_paths,
                    actual: scenario.closure_paths,
                });
            }
            let retained_report = ReleaseGateReport {
                schema: crate::release_gate::REPORT_SCHEMA.to_owned(),
                scenario: scenario.report_scenario.clone(),
                input_sha256: scenario.input_sha256.clone(),
                rom: scenario.rom.clone(),
                digest: scenario.fixed_cycle_digest.clone(),
                observations: scenario.observations.clone(),
                environment: scenario.environment.clone(),
                execution_destinations: scenario.execution_destinations.clone(),
                rsp_rdp: scenario.rsp_rdp.clone(),
                unsupported_instrumentation: scenario.unsupported_instrumentation.clone(),
                closure: scenario.closure.clone(),
                report_sha256: scenario.report_sha256.clone(),
            };
            retained_report.verify_integrity().map_err(|source| {
                ReleaseMatrixError::InvalidVerifiedReport {
                    id: scenario.id.clone(),
                    source,
                }
            })?;
            let mut derived = derive_scenario_coverage(&scenario.id, &retained_report)?;
            if let Some(authority) = &scenario.rom_class_authority {
                authority.verify_integrity(&scenario.id)?;
                if authority.run_event_sha256s != scenario.run_event_sha256s {
                    return Err(ReleaseMatrixError::RomClassAuthorityMismatch {
                        id: scenario.id.clone(),
                        field: "run_event_sha256s",
                    });
                }
                verify_rom_class_authority_binding(&scenario.id, &retained_report, authority)?;
                derived.rom_classes = vec![authority.rom_class];
            }
            if scenario.coverage != derived {
                return Err(ReleaseMatrixError::VerifiedDerivedCoverageMismatch {
                    id: scenario.id.clone(),
                });
            }
        }
        validate_platform_series_authority_usage_for_retained(
            &self.scenarios,
            &self.platform_case_authorities,
        )?;
        let (assignments, missing) =
            derive_profile_assignments(profile, &self.scenarios, &self.platform_case_authorities);
        if !missing.is_empty() {
            return Err(ReleaseMatrixError::VerifiedProfileIncomplete {
                missing: missing
                    .iter()
                    .map(|requirement| {
                        format!("{}:{}", requirement.class().as_str(), requirement.id())
                    })
                    .collect(),
            });
        }
        validate_assignment_partition(profile, &self.assignments, &[], true)?;
        if self.assignments != assignments {
            return Err(ReleaseMatrixError::VerifiedAssignmentsMismatch);
        }
        let retained_manifest = ReleaseMatrixManifest {
            schema: RELEASE_MATRIX_SCHEMA.to_owned(),
            profile: self.profile.clone(),
            scenarios: self
                .scenarios
                .iter()
                .map(retained_scenario_declaration)
                .collect(),
        };
        let recomputed_manifest = retained_manifest.recompute_manifest_sha256();
        if self.manifest_sha256 != recomputed_manifest {
            return Err(ReleaseMatrixError::VerifiedManifestIdentityMismatch {
                stored: self.manifest_sha256.clone(),
                recomputed: recomputed_manifest,
            });
        }
        let expected_total = self.scenarios.iter().map(|scenario| scenario.count).sum();
        if self.total_reports != expected_total {
            return Err(ReleaseMatrixError::VerifiedReportCountMismatch {
                stored: self.total_reports,
                recomputed: expected_total,
            });
        }
        let recomputed = verified_matrix_sha256(self);
        if self.verification_sha256 != recomputed {
            return Err(ReleaseMatrixError::VerifiedIntegrityMismatch {
                stored: self.verification_sha256.clone(),
                recomputed,
            });
        }
        Ok(())
    }
}

/// Verify private, run-ordered reports against the fixed full-parity profile.
///
/// Every report is routed by its integrity-checked `scenario` field. Coverage
/// is then derived from committed-boundary evidence; the manifest cannot label
/// a report as a different platform, feature, renderer, or executable lane.
/// Honest absence returns [`ReleaseMatrixVerification::Incomplete`].
pub fn verify_release_matrix(
    manifest: &ReleaseMatrixManifest,
    evidence: &[(ReleaseGateReport, ParsedUnsupportedJournal)],
) -> Result<ReleaseMatrixVerification, ReleaseMatrixError> {
    verify_release_matrix_with_authorities(manifest, evidence, &BTreeMap::new(), &BTreeMap::new())
}

/// Verify a release matrix with opaque locally verified RT64 platform-case
/// series. Only the repository-owned platform runner can construct the
/// capability; retained or self-hashed JSON cannot enter this API.
pub fn verify_release_matrix_with_platform_series(
    manifest: &ReleaseMatrixManifest,
    evidence: &[(ReleaseGateReport, ParsedUnsupportedJournal)],
    series: &[&VerifiedRt64PlatformCaseSeries],
) -> Result<ReleaseMatrixVerification, ReleaseMatrixError> {
    let authorities = collect_platform_series_authorities(series)?;
    verify_release_matrix_with_authorities(manifest, evidence, &BTreeMap::new(), &authorities)
}

/// Verify a release matrix while granting ROM-class credit only from opaque,
/// fully revalidated private release series.
///
/// Every supplied capability freshly re-reads its contract, program-build
/// receipt, exact retained receipt, runner, reports, journals, and admitted
/// ROM. Its semantic report digest and ordered run-event identities must equal
/// the evidence supplied to this matrix. Duplicate or unused capabilities fail
/// rather than being ignored.
pub fn verify_release_matrix_with_private_series(
    manifest: &ReleaseMatrixManifest,
    evidence: &[(ReleaseGateReport, ParsedUnsupportedJournal)],
    series: &[&VerifiedPrivateReleaseSeries],
) -> Result<ReleaseMatrixVerification, ReleaseMatrixError> {
    let authorities = collect_private_series_authorities(manifest, series)?;
    verify_release_matrix_with_authorities(manifest, evidence, &authorities, &BTreeMap::new())
}

fn collect_private_series_authorities(
    manifest: &ReleaseMatrixManifest,
    series: &[&VerifiedPrivateReleaseSeries],
) -> Result<BTreeMap<String, VerifiedRomClassAuthority>, ReleaseMatrixError> {
    let mut authorities = BTreeMap::new();
    for verified in series {
        let revalidated = verified
            .revalidate_for_release_matrix()
            .map_err(|source| ReleaseMatrixError::InvalidPrivateSeriesAuthority { source })?;
        let contract = &revalidated.contract;
        let receipt = &revalidated.receipt;
        let mut authority = VerifiedRomClassAuthority {
            schema: VERIFIED_ROM_CLASS_AUTHORITY_SCHEMA.to_owned(),
            contract_schema: contract.schema.clone(),
            contract_sha256: contract.contract_sha256.clone(),
            receipt_schema: receipt.schema.clone(),
            receipt_sha256: receipt.receipt_sha256.clone(),
            runner_executable_sha256: receipt.runner_executable_sha256.clone(),
            purpose: contract.purpose.clone(),
            report_scenario: contract.report_scenario.clone(),
            input_sha256: contract.input.sha256.clone(),
            input_bytes: contract.input.bytes,
            rom_class: contract.rom_class,
            guest_cycle: contract.guest_cycle,
            expected_execution_source: contract.expected_execution_source.clone(),
            child_executable_sha256: contract.child.executable.sha256.clone(),
            semantic_report_sha256: receipt.semantic_report_sha256.clone(),
            run_event_sha256s: receipt
                .runs
                .iter()
                .map(|run| run.run_event_sha256.clone())
                .collect(),
            authority_sha256: String::new(),
        };
        authority.authority_sha256 = authority.recompute_authority_sha256();
        authority.verify_integrity(&authority.report_scenario)?;
        let report_scenario = authority.report_scenario.clone();
        insert_private_series_authority(&mut authorities, report_scenario, authority)?;
    }
    validate_private_series_authority_usage(manifest, &authorities)?;
    Ok(authorities)
}

/// Combine private ROM-class capability evidence with opaque RT64 platform
/// case capabilities without allowing either authority to relabel the other.
pub fn verify_release_matrix_with_private_and_platform_series(
    manifest: &ReleaseMatrixManifest,
    evidence: &[(ReleaseGateReport, ParsedUnsupportedJournal)],
    private_series: &[&VerifiedPrivateReleaseSeries],
    platform_series: &[&VerifiedRt64PlatformCaseSeries],
) -> Result<ReleaseMatrixVerification, ReleaseMatrixError> {
    let private_authorities = collect_private_series_authorities(manifest, private_series)?;
    let platform_authorities = collect_platform_series_authorities(platform_series)?;
    verify_release_matrix_with_authorities(
        manifest,
        evidence,
        &private_authorities,
        &platform_authorities,
    )
}

fn insert_private_series_authority(
    authorities: &mut BTreeMap<String, VerifiedRomClassAuthority>,
    report_scenario: String,
    authority: VerifiedRomClassAuthority,
) -> Result<(), ReleaseMatrixError> {
    if authorities
        .insert(report_scenario.clone(), authority)
        .is_some()
    {
        return Err(ReleaseMatrixError::DuplicatePrivateSeriesAuthority { report_scenario });
    }
    Ok(())
}

fn validate_private_series_authority_usage(
    manifest: &ReleaseMatrixManifest,
    authorities: &BTreeMap<String, VerifiedRomClassAuthority>,
) -> Result<(), ReleaseMatrixError> {
    for report_scenario in authorities.keys() {
        if !manifest
            .scenarios
            .iter()
            .any(|scenario| scenario.report_scenario == *report_scenario)
        {
            return Err(ReleaseMatrixError::UnusedPrivateSeriesAuthority {
                report_scenario: report_scenario.clone(),
            });
        }
    }
    Ok(())
}

fn collect_platform_series_authorities(
    series: &[&VerifiedRt64PlatformCaseSeries],
) -> Result<
    BTreeMap<(Rt64PlatformTarget, Rt64PlatformCase), VerifiedRt64PlatformCaseAuthority>,
    ReleaseMatrixError,
> {
    let mut authorities = BTreeMap::new();
    for verified in series {
        let authority = verified
            .revalidate_for_release_matrix()
            .map_err(|source| ReleaseMatrixError::InvalidPlatformSeriesAuthority { source })?;
        let key = (authority.target, authority.case);
        if authorities.insert(key, authority).is_some() {
            return Err(ReleaseMatrixError::DuplicatePlatformSeriesAuthority {
                target: key.0.id().to_owned(),
                case: key.1.id().to_owned(),
            });
        }
    }
    Ok(authorities)
}

fn verify_release_matrix_with_authorities(
    manifest: &ReleaseMatrixManifest,
    evidence: &[(ReleaseGateReport, ParsedUnsupportedJournal)],
    authorities: &BTreeMap<String, VerifiedRomClassAuthority>,
    platform_authorities: &BTreeMap<
        (Rt64PlatformTarget, Rt64PlatformCase),
        VerifiedRt64PlatformCaseAuthority,
    >,
) -> Result<ReleaseMatrixVerification, ReleaseMatrixError> {
    let profile = validate_manifest(manifest)?;

    let mut evidence_by_report_scenario =
        BTreeMap::<String, Vec<(ReleaseGateReport, ParsedUnsupportedJournal)>>::new();
    for (report, journal) in evidence {
        report.verify_integrity().map_err(|source| {
            ReleaseMatrixError::InvalidUnassignedReport {
                scenario: report.scenario.clone(),
                source,
            }
        })?;
        if !manifest
            .scenarios
            .iter()
            .any(|scenario| scenario.report_scenario == report.scenario)
        {
            return Err(ReleaseMatrixError::UnexpectedReportScenario {
                scenario: report.scenario.clone(),
            });
        }
        evidence_by_report_scenario
            .entry(report.scenario.clone())
            .or_default()
            .push((report.clone(), journal.clone()));
    }

    let mut verified = Vec::with_capacity(manifest.scenarios.len());
    let mut matrix_run_events = BTreeSet::new();
    for scenario in &manifest.scenarios {
        let evidence = evidence_by_report_scenario
            .get(&scenario.report_scenario)
            .ok_or_else(|| ReleaseMatrixError::MissingEvidence {
                id: scenario.id.clone(),
            })?;
        if evidence.len() != RELEASE_MATRIX_REPORT_COUNT {
            return Err(ReleaseMatrixError::WrongReportCount {
                id: scenario.id.clone(),
                expected: RELEASE_MATRIX_REPORT_COUNT,
                actual: evidence.len(),
            });
        }

        let series = verify_release_evidence_series(evidence, RELEASE_MATRIX_REPORT_COUNT)
            .map_err(|source| ReleaseMatrixError::InvalidSeries {
                id: scenario.id.clone(),
                source: Box::new(source),
            })?;
        for run_event_sha256 in &series.run_event_sha256s {
            if !matrix_run_events.insert(run_event_sha256.clone()) {
                return Err(ReleaseMatrixError::DuplicateRunEventIdentity {
                    id: scenario.id.clone(),
                    run_event_sha256: run_event_sha256.clone(),
                });
            }
        }
        let reports: Vec<&ReleaseGateReport> = evidence.iter().map(|(report, _)| report).collect();
        for path_name in LIVE_MINIMUM_CLOSURE_PATHS {
            require_positive_path(&reports[0].closure, &scenario.id, path_name, false)?;
        }
        let authority = authorities.get(&scenario.report_scenario).cloned();
        let mut coverage = derive_scenario_coverage(&scenario.id, reports[0])?;
        if let Some(authority) = &authority {
            if authority.semantic_report_sha256 != series.report_sha256 {
                return Err(ReleaseMatrixError::RomClassAuthorityMismatch {
                    id: scenario.id.clone(),
                    field: "semantic_report_sha256",
                });
            }
            if authority.run_event_sha256s != series.run_event_sha256s {
                return Err(ReleaseMatrixError::RomClassAuthorityMismatch {
                    id: scenario.id.clone(),
                    field: "run_event_sha256s",
                });
            }
            verify_rom_class_authority_binding(&scenario.id, reports[0], authority)?;
            coverage.rom_classes = vec![authority.rom_class];
        }
        validate_feature_operation_paths(&scenario.id, &coverage, &reports[0].closure)?;
        if series.scenario != scenario.report_scenario {
            return Err(ReleaseMatrixError::ReportScenarioMismatch {
                id: scenario.id.clone(),
                expected: scenario.report_scenario.clone(),
                observed: series.scenario,
            });
        }
        let observed_input = &reports[0].input_sha256;
        if observed_input != &scenario.input_sha256 {
            return Err(ReleaseMatrixError::InputDigestMismatch {
                id: scenario.id.clone(),
                expected: scenario.input_sha256.clone(),
                observed: observed_input.clone(),
            });
        }
        if series.report_sha256 != scenario.report_sha256 {
            return Err(ReleaseMatrixError::ReportDigestMismatch {
                id: scenario.id.clone(),
                expected: scenario.report_sha256.clone(),
                observed: series.report_sha256,
            });
        }
        verified.push(VerifiedMatrixScenario {
            id: scenario.id.clone(),
            count: series.count,
            report_sha256: scenario.report_sha256.clone(),
            report_scenario: scenario.report_scenario.clone(),
            input_sha256: scenario.input_sha256.clone(),
            rom: reports[0].rom.clone(),
            rom_class_authority: authority,
            coverage,
            declaration_sha256: scenario.declaration_sha256.clone(),
            guest_cycle: series.guest_cycle,
            fixed_cycle_digest: reports[0].digest.clone(),
            observations: reports[0].observations.clone(),
            environment: reports[0].environment.clone(),
            execution_destinations: reports[0].execution_destinations.clone(),
            rsp_rdp: reports[0].rsp_rdp.clone(),
            unsupported_instrumentation: reports[0].unsupported_instrumentation.clone(),
            closure_paths: reports[0].closure.len() as u64,
            closure: reports[0].closure.clone(),
            unsupported_events: reports[0]
                .closure
                .iter()
                .map(|path| path.unsupported.len() as u64)
                .sum(),
            unsupported_journal_schema: "fn64.unsupported-journal.v3".to_owned(),
            bound_journals: evidence.len(),
            run_event_sha256s: series.run_event_sha256s,
            presentation_boundary: presentation_boundary(&reports[0].observations),
        });
    }

    verified.sort_by(|left, right| left.id.cmp(&right.id));
    validate_platform_series_authority_usage(&verified, platform_authorities)?;
    let retained_platform_authorities = platform_authorities.values().cloned().collect::<Vec<_>>();
    let manifest_sha256 = manifest.recompute_manifest_sha256();
    let total_reports = verified.iter().map(|scenario| scenario.count).sum();
    let (assignments, missing) =
        derive_profile_assignments(profile, &verified, &retained_platform_authorities);
    if !missing.is_empty() {
        let mut incomplete = IncompleteReleaseMatrix {
            schema: INCOMPLETE_RELEASE_MATRIX_SCHEMA.to_owned(),
            manifest_sha256,
            profile: manifest.profile.clone(),
            verified_scenarios: verified.len(),
            verified_reports: total_reports,
            unsupported_instrumentation: crate::UnsupportedInstrumentationEvidence {
                schema: fn64_runtime::UNSUPPORTED_INSTRUMENTATION_SCHEMA.to_owned(),
                sha256: hex(&fn64_runtime::UNSUPPORTED_INSTRUMENTATION_SHA256),
            },
            platform_case_authorities: retained_platform_authorities,
            satisfied: assignments,
            missing,
            assessment_sha256: String::new(),
        };
        incomplete.assessment_sha256 = incomplete_matrix_sha256(&incomplete);
        incomplete.verify_integrity()?;
        return Ok(ReleaseMatrixVerification::Incomplete(incomplete));
    }

    let mut result = VerifiedReleaseMatrix {
        schema: VERIFIED_RELEASE_MATRIX_SCHEMA.to_owned(),
        manifest_sha256,
        profile: manifest.profile.clone(),
        total_reports,
        scenarios: verified,
        platform_case_authorities: retained_platform_authorities,
        assignments,
        verification_sha256: String::new(),
    };
    result.verification_sha256 = verified_matrix_sha256(&result);
    result.verify_integrity()?;
    Ok(ReleaseMatrixVerification::Complete(result))
}

fn retained_scenario_declaration(scenario: &VerifiedMatrixScenario) -> ReleaseMatrixScenario {
    ReleaseMatrixScenario {
        id: scenario.id.clone(),
        report_scenario: scenario.report_scenario.clone(),
        input_sha256: scenario.input_sha256.clone(),
        report_sha256: scenario.report_sha256.clone(),
        declaration_sha256: scenario.declaration_sha256.clone(),
    }
}

fn derive_profile_assignments(
    profile: FullParityV1,
    scenarios: &[VerifiedMatrixScenario],
    platform_authorities: &[VerifiedRt64PlatformCaseAuthority],
) -> (
    Vec<CertificationRequirementAssignment>,
    Vec<CertificationRequirementRef>,
) {
    let mut evidence = BTreeMap::<(CertificationRequirementClass, String), BTreeSet<String>>::new();
    for scenario in scenarios {
        if let Some(authority) = &scenario.rom_class_authority {
            for rom_class in &scenario.coverage.rom_classes {
                insert_requirement_evidence(
                    &mut evidence,
                    CertificationRequirementClass::RomClass,
                    rom_class_id(*rom_class).to_owned(),
                    &authority.authority_sha256,
                );
            }
        }
        for tv_region in &scenario.coverage.tv_regions {
            insert_requirement_evidence(
                &mut evidence,
                CertificationRequirementClass::TvRegion,
                tv_region_id(*tv_region).to_owned(),
                &scenario.declaration_sha256,
            );
        }
        let program = program_feature_id(scenario.coverage.programs[0]);
        let renderer = if scenario
            .coverage
            .renderers
            .contains(&RendererFeature::ReferenceLleAccuracy)
        {
            "reference_lle_accuracy"
        } else {
            "rt64_lle_accuracy"
        };
        insert_requirement_evidence(
            &mut evidence,
            CertificationRequirementClass::ProgramRendererLane,
            format!("{program}/{renderer}"),
            &scenario.declaration_sha256,
        );
        if let Some(target) = platform_api_target(&scenario.environment) {
            insert_requirement_evidence(
                &mut evidence,
                CertificationRequirementClass::PlatformApiTarget,
                target.to_owned(),
                &scenario.declaration_sha256,
            );
        }
        insert_requirement_evidence(
            &mut evidence,
            CertificationRequirementClass::Save,
            save_feature_id(scenario.coverage.saves[0]).to_owned(),
            &scenario.declaration_sha256,
        );
        for controller in &scenario.coverage.controllers {
            insert_requirement_evidence(
                &mut evidence,
                CertificationRequirementClass::Controller,
                controller_feature_id(*controller).to_owned(),
                &scenario.declaration_sha256,
            );
        }
        for microcode in &scenario.coverage.microcodes {
            insert_requirement_evidence(
                &mut evidence,
                CertificationRequirementClass::PublicMicrocode,
                microcode_feature_id(*microcode).to_owned(),
                &scenario.declaration_sha256,
            );
        }
        for mechanism in &scenario.coverage.rsp_rdp_mechanisms {
            insert_requirement_evidence(
                &mut evidence,
                CertificationRequirementClass::RspRdpMechanism,
                rsp_rdp_mechanism_feature_id(*mechanism).to_owned(),
                &scenario.declaration_sha256,
            );
        }
    }
    for authority in platform_authorities {
        insert_requirement_evidence(
            &mut evidence,
            CertificationRequirementClass::Rt64TargetCase,
            format!("{}/{}", authority.target.id(), authority.case.id()),
            &authority.authority_sha256,
        );
    }

    let mut assignments = Vec::new();
    let mut missing = Vec::new();
    for requirement in profile.requirements() {
        let key = (requirement.class(), requirement.id().to_owned());
        if let Some(evidence_sha256s) = evidence.remove(&key) {
            assignments.push(CertificationRequirementAssignment {
                requirement: CertificationRequirementRef::from_requirement(&requirement),
                evidence_sha256s: evidence_sha256s.into_iter().collect(),
            });
        } else {
            missing.push(CertificationRequirementRef::from_requirement(&requirement));
        }
    }
    (assignments, missing)
}

const fn rom_class_id(value: ReleaseRomClass) -> &'static str {
    match value {
        ReleaseRomClass::Unclassified => "unclassified",
        ReleaseRomClass::RetailCartridge => "retail_cartridge",
        ReleaseRomClass::PublicHomebrew => "public_homebrew",
    }
}

fn verify_rom_class_authority_binding(
    id: &str,
    report: &ReleaseGateReport,
    authority: &VerifiedRomClassAuthority,
) -> Result<(), ReleaseMatrixError> {
    authority.verify_integrity(id)?;
    let rom = report
        .rom
        .as_ref()
        .ok_or_else(|| ReleaseMatrixError::RomClassAuthorityMismatch {
            id: id.to_owned(),
            field: "report.rom",
        })?;
    if authority.report_scenario != report.scenario {
        return Err(ReleaseMatrixError::RomClassAuthorityMismatch {
            id: id.to_owned(),
            field: "report_scenario",
        });
    }
    if authority.input_sha256 != report.input_sha256 {
        return Err(ReleaseMatrixError::RomClassAuthorityMismatch {
            id: id.to_owned(),
            field: "input_sha256",
        });
    }
    if authority.rom_class != rom.class {
        return Err(ReleaseMatrixError::RomClassAuthorityMismatch {
            id: id.to_owned(),
            field: "rom.class",
        });
    }
    if authority.input_bytes != rom.byte_len {
        return Err(ReleaseMatrixError::RomClassAuthorityMismatch {
            id: id.to_owned(),
            field: "rom.byte_len",
        });
    }
    if authority.guest_cycle != report.digest.guest_cycle {
        return Err(ReleaseMatrixError::RomClassAuthorityMismatch {
            id: id.to_owned(),
            field: "guest_cycle",
        });
    }
    if authority.expected_execution_source != report.execution_destinations.source {
        return Err(ReleaseMatrixError::RomClassAuthorityMismatch {
            id: id.to_owned(),
            field: "execution_destinations.source",
        });
    }
    if authority.semantic_report_sha256 != report.report_sha256 {
        return Err(ReleaseMatrixError::RomClassAuthorityMismatch {
            id: id.to_owned(),
            field: "report_sha256",
        });
    }
    Ok(())
}

const fn tv_region_id(value: ReleaseTvRegion) -> &'static str {
    match value {
        ReleaseTvRegion::Ntsc => "ntsc",
        ReleaseTvRegion::Pal => "pal",
        ReleaseTvRegion::Mpal => "mpal",
        ReleaseTvRegion::RegionFree => "region_free",
    }
}

fn platform_api_target(environment: &ReleaseEnvironmentEvidence) -> Option<&'static str> {
    match (environment.platform, &environment.renderer) {
        (
            ReleaseHostPlatform::MacosArm64,
            ReleaseRendererEvidence::Rt64 {
                graphics_api: ReleaseGraphicsApi::Metal,
                ..
            },
        ) => Some("macos-metal"),
        (
            ReleaseHostPlatform::LinuxX86_64,
            ReleaseRendererEvidence::Rt64 {
                graphics_api: ReleaseGraphicsApi::Vulkan,
                ..
            },
        ) => Some("linux-vulkan"),
        (
            ReleaseHostPlatform::WindowsX86_64,
            ReleaseRendererEvidence::Rt64 {
                graphics_api: ReleaseGraphicsApi::D3d12,
                ..
            },
        ) => match environment
            .windows_version
            .filter(|version| version.verify().is_ok())?
            .family
        {
            ReleaseWindowsFamily::Windows10 => Some("windows10-d3d12"),
            ReleaseWindowsFamily::Windows11 => Some("windows11-d3d12"),
        },
        (
            ReleaseHostPlatform::WindowsX86_64,
            ReleaseRendererEvidence::Rt64 {
                graphics_api: ReleaseGraphicsApi::Vulkan,
                ..
            },
        ) => match environment
            .windows_version
            .filter(|version| version.verify().is_ok())?
            .family
        {
            ReleaseWindowsFamily::Windows10 => Some("windows10-vulkan"),
            ReleaseWindowsFamily::Windows11 => Some("windows11-vulkan"),
        },
        _ => None,
    }
}

fn validate_platform_series_authority_usage(
    scenarios: &[VerifiedMatrixScenario],
    authorities: &BTreeMap<
        (Rt64PlatformTarget, Rt64PlatformCase),
        VerifiedRt64PlatformCaseAuthority,
    >,
) -> Result<(), ReleaseMatrixError> {
    for authority in authorities.values() {
        authority
            .verify_integrity()
            .map_err(|source| ReleaseMatrixError::InvalidPlatformSeriesAuthority { source })?;
        if !scenarios
            .iter()
            .any(|scenario| platform_authority_matches_scenario(authority, scenario))
        {
            return Err(ReleaseMatrixError::UnusedPlatformSeriesAuthority {
                target: authority.target.id().to_owned(),
                case: authority.case.id().to_owned(),
            });
        }
    }
    Ok(())
}

fn validate_platform_series_authority_usage_for_retained(
    scenarios: &[VerifiedMatrixScenario],
    authorities: &[VerifiedRt64PlatformCaseAuthority],
) -> Result<(), ReleaseMatrixError> {
    let mut indexed = BTreeMap::new();
    for authority in authorities {
        let key = (authority.target, authority.case);
        if indexed.insert(key, authority.clone()).is_some() {
            return Err(ReleaseMatrixError::DuplicatePlatformSeriesAuthority {
                target: key.0.id().to_owned(),
                case: key.1.id().to_owned(),
            });
        }
    }
    validate_platform_series_authority_usage(scenarios, &indexed)
}

fn platform_authority_matches_scenario(
    authority: &VerifiedRt64PlatformCaseAuthority,
    scenario: &VerifiedMatrixScenario,
) -> bool {
    if scenario.environment.platform != authority.platform
        || scenario.environment.windows_version != authority.windows_version
        || scenario.report_scenario != authority.bound_report_scenario
        || scenario.report_sha256 != authority.bound_report_sha256
        || scenario.run_event_sha256s != authority.bound_matrix_run_event_sha256s
    {
        return false;
    }
    let ReleaseRendererEvidence::Rt64 {
        graphics_api,
        backend_identity,
        source_authoritative: true,
        ..
    } = &scenario.environment.renderer
    else {
        return false;
    };
    *graphics_api == authority.graphics_api
        && authority
            .target
            .matches_host(authority.platform, authority.windows_version)
        && backend_identity.contains(&format!(
            ";adapter_sha256={};",
            authority.adapter_source_sha256
        ))
        && backend_identity.contains(&format!(";source={};", authority.rt64_source_id))
        && backend_identity.ends_with(&format!("post_vi_api={}", authority.capture_api))
}

fn insert_requirement_evidence(
    evidence: &mut BTreeMap<(CertificationRequirementClass, String), BTreeSet<String>>,
    class: CertificationRequirementClass,
    id: String,
    declaration_sha256: &str,
) {
    evidence
        .entry((class, id))
        .or_default()
        .insert(declaration_sha256.to_owned());
}

const fn program_feature_id(value: ProgramFeature) -> &'static str {
    match value {
        ProgramFeature::NativeArchive => "native_archive",
        ProgramFeature::TypedObservedFunction => "typed_observed_function",
        ProgramFeature::TypedBlock => "typed_block",
    }
}

const fn save_feature_id(value: SaveFeature) -> &'static str {
    match value {
        SaveFeature::NoCartridgeSave => "no_cartridge_save",
        SaveFeature::Eeprom4Kbit => "eeprom_4_kbit",
        SaveFeature::Eeprom16Kbit => "eeprom_16_kbit",
        SaveFeature::Sram32Kib => "sram_32_kib",
        SaveFeature::FlashRam128Kib => "flash_ram_128_kib",
    }
}

const fn controller_feature_id(value: ControllerFeature) -> &'static str {
    match value {
        ControllerFeature::StandardController => "standard_controller",
        ControllerFeature::ControllerPak => "controller_pak",
        ControllerFeature::RumblePak => "rumble_pak",
        ControllerFeature::TransferPak => "transfer_pak",
        ControllerFeature::VoiceRecognitionUnit => "voice_recognition_unit",
    }
}

const fn microcode_feature_id(value: MicrocodeFeature) -> &'static str {
    match value {
        MicrocodeFeature::Fast3d => "fast3d",
        MicrocodeFeature::F3dex => "f3dex",
        MicrocodeFeature::F3dlx => "f3dlx",
        MicrocodeFeature::F3dlxRej => "f3dlx-rej",
        MicrocodeFeature::F3dex2 => "f3dex2",
        MicrocodeFeature::F3dex2NoN => "f3dex2-non",
        MicrocodeFeature::F3dex2Rej => "f3dex2-rej",
        MicrocodeFeature::F3dlx2Rej => "f3dlx2-rej",
        MicrocodeFeature::S2dex => "s2dex",
        MicrocodeFeature::S2dex2 => "s2dex2",
        MicrocodeFeature::L3dex => "l3dex",
        MicrocodeFeature::L3dex2 => "l3dex2",
    }
}

const fn rsp_rdp_mechanism_feature_id(value: RspRdpMechanismFeature) -> &'static str {
    match value {
        RspRdpMechanismFeature::DramDpc => "dram-dpc",
        RspRdpMechanismFeature::XbusDpc => "xbus-dpc",
        RspRdpMechanismFeature::ImemReplacement => "imem-replacement",
    }
}

fn presentation_boundary(
    observations: &ReleaseObservationGeometry,
) -> PresentationBoundaryEvidence {
    match &observations.framebuffer.source {
        FramebufferObservationSource::PhysicalRdram { .. } => {
            PresentationBoundaryEvidence::CommittedViBoundary
        }
        FramebufferObservationSource::PostViSwapchain { .. } => {
            PresentationBoundaryEvidence::ExactPostViCapture
        }
    }
}

fn derive_scenario_coverage(
    id: &str,
    report: &ReleaseGateReport,
) -> Result<ReleaseMatrixCoverage, ReleaseMatrixError> {
    derive_scenario_coverage_with_catalog(id, report, CERTIFIED_PUBLIC_MICROCODE_CATALOG_V1)
}

fn derive_scenario_coverage_with_catalog(
    id: &str,
    report: &ReleaseGateReport,
    certified_microcodes: &[CertifiedMicrocodeIdentity],
) -> Result<ReleaseMatrixCoverage, ReleaseMatrixError> {
    let program = match &report.execution_destinations.source {
        crate::ExecutionDestinationSource::NoProgram => {
            return Err(ReleaseMatrixError::NoProgramEvidence { id: id.to_owned() });
        }
        crate::ExecutionDestinationSource::NativeArchive { .. } => ProgramFeature::NativeArchive,
        crate::ExecutionDestinationSource::TypedObservedFunctionProgram { .. } => {
            ProgramFeature::TypedObservedFunction
        }
        crate::ExecutionDestinationSource::TypedBlockProgram { .. } => ProgramFeature::TypedBlock,
    };

    let environment = &report.environment;
    let observations = &report.observations;
    let renderers = match (&environment.renderer, &observations.framebuffer.source) {
        (
            ReleaseRendererEvidence::Reference {
                execution_policy: ReleaseGraphicsExecutionPolicy::LleAccuracy,
                ..
            },
            FramebufferObservationSource::PhysicalRdram { .. },
        ) => vec![RendererFeature::ReferenceLleAccuracy],
        (
            ReleaseRendererEvidence::Rt64 {
                execution_policy: ReleaseGraphicsExecutionPolicy::LleAccuracy,
                graphics_api,
                backend_identity,
                source_authoritative: true,
                settings_sha256,
                replacement_packs_active,
                ..
            },
            FramebufferObservationSource::PostViSwapchain {
                backend_identity: observed_identity,
                settings_sha256: observed_settings,
                ..
            },
        ) if backend_identity == observed_identity && settings_sha256 == observed_settings => {
            if crate::render_evidence::validate_authoritative_rt64_backend_identity(
                backend_identity,
                environment.platform,
                *graphics_api,
            )
            .is_err()
            {
                return Err(ReleaseMatrixError::NonAuthoritativeRt64Identity {
                    id: id.to_owned(),
                    backend_identity: backend_identity.clone(),
                });
            }
            let mut values = vec![
                RendererFeature::Rt64LleAccuracy,
                RendererFeature::Rt64PostViCapture,
            ];
            if *replacement_packs_active {
                values.push(RendererFeature::Rt64ReplacementPacks);
            }
            values
        }
        _ => {
            return Err(ReleaseMatrixError::RendererEnvironmentMismatch { id: id.to_owned() });
        }
    };
    let platforms = vec![match environment.platform {
        ReleaseHostPlatform::MacosArm64 => ReleasePlatform::MacosArm64,
        ReleaseHostPlatform::LinuxX86_64 => ReleasePlatform::LinuxX86_64,
        ReleaseHostPlatform::WindowsX86_64 => ReleasePlatform::WindowsX86_64,
    }];
    let tv_regions = match report.rom.as_ref().map(|rom| rom.decoded_tv_region) {
        Some(ReleaseTvRegion::Ntsc) => vec![ReleaseTvRegion::Ntsc],
        Some(ReleaseTvRegion::Pal) => vec![ReleaseTvRegion::Pal],
        Some(ReleaseTvRegion::Mpal) => vec![ReleaseTvRegion::Mpal],
        Some(ReleaseTvRegion::RegionFree) | None => Vec::new(),
    };
    let mut observed_controllers = BTreeSet::new();
    for port in environment.controller_ports {
        match port {
            ReleaseControllerPort::StandardControllerNoPak => {
                observed_controllers.insert(ControllerFeature::StandardController);
            }
            ReleaseControllerPort::StandardControllerControllerPak => {
                observed_controllers.insert(ControllerFeature::StandardController);
                observed_controllers.insert(ControllerFeature::ControllerPak);
            }
            ReleaseControllerPort::StandardControllerRumblePak => {
                observed_controllers.insert(ControllerFeature::StandardController);
                observed_controllers.insert(ControllerFeature::RumblePak);
            }
            ReleaseControllerPort::StandardControllerTransferPak => {
                observed_controllers.insert(ControllerFeature::StandardController);
                observed_controllers.insert(ControllerFeature::TransferPak);
            }
            ReleaseControllerPort::VoiceRecognitionUnit => {
                observed_controllers.insert(ControllerFeature::VoiceRecognitionUnit);
            }
            ReleaseControllerPort::Absent => {}
        }
    }
    let saves = vec![match environment.cartridge_save {
        ReleaseCartridgeSave::NoCartridgeSave => SaveFeature::NoCartridgeSave,
        ReleaseCartridgeSave::Eeprom4k => SaveFeature::Eeprom4Kbit,
        ReleaseCartridgeSave::Eeprom16k => SaveFeature::Eeprom16Kbit,
        ReleaseCartridgeSave::Sram32Kib => SaveFeature::Sram32Kib,
        ReleaseCartridgeSave::FlashRam128Kib => SaveFeature::FlashRam128Kib,
    }];
    let mut microcodes = BTreeSet::new();
    let mut rsp_rdp_mechanisms = BTreeSet::new();
    for event in &report.rsp_rdp.ordered {
        match &event.observation {
            RspRdpObservationKindEvidence::MicrocodeRecognition {
                text_sha256,
                family,
                ..
            } => {
                if let Some(feature) =
                    certified_microcode_feature(id, text_sha256, *family, certified_microcodes)?
                {
                    microcodes.insert(feature);
                }
            }
            RspRdpObservationKindEvidence::DramDpcCommitted { .. } => {
                rsp_rdp_mechanisms.insert(RspRdpMechanismFeature::DramDpc);
            }
            RspRdpObservationKindEvidence::XbusDpcCommitted { .. } => {
                rsp_rdp_mechanisms.insert(RspRdpMechanismFeature::XbusDpc);
            }
            RspRdpObservationKindEvidence::ImemReplacementCommitted { .. } => {
                rsp_rdp_mechanisms.insert(RspRdpMechanismFeature::ImemReplacement);
            }
        }
    }
    let coverage = ReleaseMatrixCoverage {
        rom_classes: Vec::new(),
        tv_regions,
        platforms,
        controllers: observed_controllers.into_iter().collect(),
        saves,
        renderers,
        programs: vec![program],
        microcodes: microcodes.into_iter().collect(),
        rsp_rdp_mechanisms: rsp_rdp_mechanisms.into_iter().collect(),
    };
    validate_coverage(id, &coverage, None)?;
    validate_coverage_cardinality(id, &coverage)?;
    Ok(coverage)
}

fn certified_microcode_feature(
    id: &str,
    text_sha256: &str,
    observed_family: Option<ReleaseMicrocodeFamily>,
    catalog: &[CertifiedMicrocodeIdentity],
) -> Result<Option<MicrocodeFeature>, ReleaseMatrixError> {
    let digest =
        crate::release_gate::decode_sha256(text_sha256).expect("validated release report SHA-256");
    let mut matches = catalog
        .iter()
        .filter(|identity| identity.text_sha256 == digest);
    let Some(certified) = matches.next() else {
        return Ok(None);
    };
    if matches.next().is_some() {
        return Err(ReleaseMatrixError::DuplicateCertifiedMicrocodeIdentity {
            text_sha256: text_sha256.to_owned(),
        });
    }
    if let Some(observed) = observed_family {
        if observed != certified.family {
            return Err(ReleaseMatrixError::CertifiedMicrocodeFamilyMismatch {
                id: id.to_owned(),
                text_sha256: text_sha256.to_owned(),
                certified: certified.family,
                observed,
            });
        }
    }
    Ok(microcode_feature(certified.family))
}

const fn microcode_feature(family: ReleaseMicrocodeFamily) -> Option<MicrocodeFeature> {
    match family {
        ReleaseMicrocodeFamily::Fast3d => Some(MicrocodeFeature::Fast3d),
        ReleaseMicrocodeFamily::F3dex => Some(MicrocodeFeature::F3dex),
        ReleaseMicrocodeFamily::F3dlx => Some(MicrocodeFeature::F3dlx),
        ReleaseMicrocodeFamily::F3dlxRej => Some(MicrocodeFeature::F3dlxRej),
        ReleaseMicrocodeFamily::F3dex2 => Some(MicrocodeFeature::F3dex2),
        ReleaseMicrocodeFamily::F3dex2NoN => Some(MicrocodeFeature::F3dex2NoN),
        ReleaseMicrocodeFamily::F3dex2Rej => Some(MicrocodeFeature::F3dex2Rej),
        ReleaseMicrocodeFamily::F3dlx2Rej => Some(MicrocodeFeature::F3dlx2Rej),
        ReleaseMicrocodeFamily::S2dex => Some(MicrocodeFeature::S2dex),
        ReleaseMicrocodeFamily::S2dex2 => Some(MicrocodeFeature::S2dex2),
        ReleaseMicrocodeFamily::L3dex => Some(MicrocodeFeature::L3dex),
        ReleaseMicrocodeFamily::L3dex2 => Some(MicrocodeFeature::L3dex2),
        ReleaseMicrocodeFamily::F3dzex2 | ReleaseMicrocodeFamily::Other { .. } => None,
    }
}

fn validate_retained_closure(id: &str, closure: &[ClosurePath]) -> Result<(), ReleaseMatrixError> {
    crate::release_gate::validate_closure_paths(closure)
        .and_then(|()| crate::release_gate::validate_canonical_closure_order(closure))
        .map_err(|source| ReleaseMatrixError::InvalidVerifiedClosure {
            id: id.to_owned(),
            source,
        })?;
    for path in closure {
        require_positive_path(closure, id, &path.name, false)?;
    }
    for path in LIVE_MINIMUM_CLOSURE_PATHS {
        require_positive_path(closure, id, path, false)?;
    }
    Ok(())
}

fn validate_feature_operation_paths(
    id: &str,
    coverage: &ReleaseMatrixCoverage,
    closure: &[ClosurePath],
) -> Result<(), ReleaseMatrixError> {
    for save in &coverage.saves {
        let expected_path = match save {
            SaveFeature::NoCartridgeSave => None,
            SaveFeature::Eeprom4Kbit => Some("save.eeprom-4k-operation"),
            SaveFeature::Eeprom16Kbit => Some("save.eeprom-16k-operation"),
            SaveFeature::Sram32Kib => Some("save.sram-operation"),
            SaveFeature::FlashRam128Kib => Some("save.flashram-operation"),
        };
        for path in [
            "save.eeprom-4k-operation",
            "save.eeprom-16k-operation",
            "save.sram-operation",
            "save.flashram-operation",
        ] {
            if Some(path) != expected_path && closure.iter().any(|observed| observed.name == path) {
                return Err(ReleaseMatrixError::UnexpectedFeatureObservation {
                    id: id.to_owned(),
                    path: path.to_owned(),
                });
            }
        }
        if let Some(path) = expected_path {
            require_positive_path(closure, id, path, true)?;
        }
    }

    let controller_pak_declared = coverage
        .controllers
        .contains(&ControllerFeature::ControllerPak);
    let pfs_observed = closure
        .iter()
        .any(|candidate| candidate.name == "save.pfs-operation");
    if controller_pak_declared {
        require_positive_path(closure, id, "save.pfs-operation", true)?;
    } else if pfs_observed {
        return Err(ReleaseMatrixError::UnexpectedFeatureObservation {
            id: id.to_owned(),
            path: "save.pfs-operation".to_owned(),
        });
    }

    for (feature, path) in [
        (
            ControllerFeature::StandardController,
            "controller.standard-input-read",
        ),
        (ControllerFeature::RumblePak, "controller.rumble-operation"),
        (
            ControllerFeature::TransferPak,
            "controller.transfer-pak-operation",
        ),
        (
            ControllerFeature::VoiceRecognitionUnit,
            "controller.voice-operation",
        ),
    ] {
        let declared = coverage.controllers.contains(&feature);
        let observed = closure.iter().any(|candidate| candidate.name == path);
        if declared {
            require_positive_path(closure, id, path, true)?;
        } else if observed {
            return Err(ReleaseMatrixError::UnexpectedFeatureObservation {
                id: id.to_owned(),
                path: path.to_owned(),
            });
        }
    }
    Ok(())
}

fn require_positive_path(
    closure: &[ClosurePath],
    id: &str,
    path_name: &str,
    feature_specific: bool,
) -> Result<(), ReleaseMatrixError> {
    let Some(path) = closure.iter().find(|path| path.name == path_name) else {
        if feature_specific {
            return Err(ReleaseMatrixError::MissingFeatureObservation {
                id: id.to_owned(),
                path: path_name.to_owned(),
            });
        }
        return Err(ReleaseMatrixError::MissingLiveMinimumObservation {
            id: id.to_owned(),
            path: path_name.to_owned(),
        });
    };
    if path.observations == 0
        || !matches!(&path.status, ClosurePathStatus::ExercisedZeroUnsupported)
        || !path.unsupported.is_empty()
    {
        return Err(ReleaseMatrixError::InvalidLivePathEvidence {
            id: id.to_owned(),
            path: path_name.to_owned(),
            observations: path.observations,
            status: path.status.clone(),
            unsupported: path.unsupported.len(),
        });
    }
    Ok(())
}

fn validate_manifest(manifest: &ReleaseMatrixManifest) -> Result<FullParityV1, ReleaseMatrixError> {
    if manifest.schema != RELEASE_MATRIX_SCHEMA {
        return Err(ReleaseMatrixError::UnsupportedSchema(
            manifest.schema.clone(),
        ));
    }
    let profile = manifest
        .profile
        .verify()
        .map_err(ReleaseMatrixError::InvalidCertificationProfile)?;
    if manifest.scenarios.is_empty() || manifest.scenarios.len() > RELEASE_MATRIX_MAX_SCENARIOS {
        return Err(ReleaseMatrixError::ScenarioCount {
            minimum: 1,
            maximum: RELEASE_MATRIX_MAX_SCENARIOS,
            actual: manifest.scenarios.len(),
        });
    }
    let mut ids = BTreeSet::new();
    let mut report_scenarios = BTreeSet::new();

    for scenario in &manifest.scenarios {
        validate_id(&scenario.id)?;
        if !ids.insert(scenario.id.clone()) {
            return Err(ReleaseMatrixError::DuplicateScenarioId(scenario.id.clone()));
        }
        if scenario.report_scenario.is_empty()
            || scenario.report_scenario.len() > 256
            || scenario.report_scenario.chars().any(char::is_control)
        {
            return Err(ReleaseMatrixError::InvalidReportScenario {
                id: scenario.id.clone(),
            });
        }
        if !report_scenarios.insert(scenario.report_scenario.clone()) {
            return Err(ReleaseMatrixError::DuplicateReportScenario(
                scenario.report_scenario.clone(),
            ));
        }
        validate_sha256(&scenario.id, "input_sha256", &scenario.input_sha256)?;
        validate_sha256(&scenario.id, "report_sha256", &scenario.report_sha256)?;
        validate_sha256(
            &scenario.id,
            "declaration_sha256",
            &scenario.declaration_sha256,
        )?;
        let recomputed = scenario.recompute_declaration_sha256();
        if scenario.declaration_sha256 != recomputed {
            return Err(ReleaseMatrixError::DeclarationDigestMismatch {
                id: scenario.id.clone(),
                stored: scenario.declaration_sha256.clone(),
                recomputed,
            });
        }
    }
    Ok(profile)
}

fn validate_coverage_cardinality(
    id: &str,
    coverage: &ReleaseMatrixCoverage,
) -> Result<(), ReleaseMatrixError> {
    if coverage.platforms.len() != 1 {
        return Err(ReleaseMatrixError::ExactOneCoverage {
            id: id.to_owned(),
            dimension: "platforms",
            actual: coverage.platforms.len(),
        });
    }
    if coverage.saves.len() != 1 {
        return Err(ReleaseMatrixError::ExactOneCoverage {
            id: id.to_owned(),
            dimension: "saves",
            actual: coverage.saves.len(),
        });
    }
    if coverage.programs.len() != 1 {
        return Err(ReleaseMatrixError::ExactOneCoverage {
            id: id.to_owned(),
            dimension: "programs",
            actual: coverage.programs.len(),
        });
    }

    let has_reference = coverage
        .renderers
        .contains(&RendererFeature::ReferenceLleAccuracy);
    let has_rt64 = coverage
        .renderers
        .contains(&RendererFeature::Rt64LleAccuracy);
    let valid_renderer = match (has_reference, has_rt64) {
        (true, false) => coverage.renderers.len() == 1,
        (false, true) => true,
        _ => false,
    };
    if !valid_renderer {
        return Err(ReleaseMatrixError::InvalidRendererCombination { id: id.to_owned() });
    }
    Ok(())
}

fn validate_id(id: &str) -> Result<(), ReleaseMatrixError> {
    let valid = !id.is_empty()
        && id.len() <= 64
        && id
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
        && id.as_bytes()[0].is_ascii_alphanumeric()
        && id.as_bytes()[id.len() - 1].is_ascii_alphanumeric();
    if valid {
        Ok(())
    } else {
        Err(ReleaseMatrixError::InvalidScenarioId(id.to_owned()))
    }
}

fn validate_sha256(id: &str, field: &'static str, value: &str) -> Result<(), ReleaseMatrixError> {
    if value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        Ok(())
    } else {
        Err(ReleaseMatrixError::InvalidSha256 {
            id: id.to_owned(),
            field,
        })
    }
}

fn validate_coverage(
    scope: &str,
    coverage: &ReleaseMatrixCoverage,
    required: Option<&ReleaseMatrixCoverage>,
) -> Result<(), ReleaseMatrixError> {
    validate_optional_dimension(scope, "rom_classes", &coverage.rom_classes)?;
    if coverage.rom_classes.len() > 1 {
        return Err(ReleaseMatrixError::ExactOneCoverage {
            id: scope.to_owned(),
            dimension: "rom_classes",
            actual: coverage.rom_classes.len(),
        });
    }
    if coverage.rom_classes == [ReleaseRomClass::Unclassified] {
        return Err(ReleaseMatrixError::InvalidRomClassAuthority {
            id: scope.to_owned(),
            detail: "unclassified input cannot satisfy ROM-class coverage".to_owned(),
        });
    }
    validate_optional_dimension(scope, "tv_regions", &coverage.tv_regions)?;
    if coverage.tv_regions.len() > 1 {
        return Err(ReleaseMatrixError::ExactOneCoverage {
            id: scope.to_owned(),
            dimension: "tv_regions",
            actual: coverage.tv_regions.len(),
        });
    }
    validate_dimension(
        scope,
        "platforms",
        &coverage.platforms,
        required.map(|r| r.platforms.as_slice()),
    )?;
    validate_dimension(
        scope,
        "controllers",
        &coverage.controllers,
        required.map(|r| r.controllers.as_slice()),
    )?;
    validate_dimension(
        scope,
        "saves",
        &coverage.saves,
        required.map(|r| r.saves.as_slice()),
    )?;
    validate_dimension(
        scope,
        "renderers",
        &coverage.renderers,
        required.map(|r| r.renderers.as_slice()),
    )?;
    validate_dimension(
        scope,
        "programs",
        &coverage.programs,
        required.map(|r| r.programs.as_slice()),
    )?;
    validate_optional_dimension(scope, "microcodes", &coverage.microcodes)?;
    validate_optional_dimension(scope, "rsp_rdp_mechanisms", &coverage.rsp_rdp_mechanisms)?;
    Ok(())
}

fn validate_optional_dimension<T: Copy + fmt::Debug + Ord>(
    scope: &str,
    dimension: &'static str,
    values: &[T],
) -> Result<(), ReleaseMatrixError> {
    let mut unique = BTreeSet::new();
    for value in values {
        if !unique.insert(*value) {
            return Err(ReleaseMatrixError::DuplicateCoverage {
                scope: scope.to_owned(),
                dimension,
                value: format!("{value:?}"),
            });
        }
    }
    Ok(())
}

fn validate_dimension<T: Copy + fmt::Debug + Ord>(
    scope: &str,
    dimension: &'static str,
    values: &[T],
    required: Option<&[T]>,
) -> Result<(), ReleaseMatrixError> {
    if values.is_empty() {
        return Err(ReleaseMatrixError::EmptyCoverage {
            scope: scope.to_owned(),
            dimension,
        });
    }
    let mut unique = BTreeSet::new();
    for value in values {
        if !unique.insert(*value) {
            return Err(ReleaseMatrixError::DuplicateCoverage {
                scope: scope.to_owned(),
                dimension,
                value: format!("{value:?}"),
            });
        }
        if let Some(required) = required {
            if !required.contains(value) {
                return Err(ReleaseMatrixError::UndeclaredCoverage {
                    scope: scope.to_owned(),
                    dimension,
                    value: format!("{value:?}"),
                });
            }
        }
    }
    Ok(())
}

impl ReleasePlatform {
    const fn tag(self) -> u8 {
        match self {
            Self::MacosArm64 => 0,
            Self::LinuxX86_64 => 1,
            Self::WindowsX86_64 => 2,
        }
    }
}

impl ControllerFeature {
    const fn tag(self) -> u8 {
        match self {
            Self::StandardController => 0,
            Self::ControllerPak => 1,
            Self::RumblePak => 2,
            Self::TransferPak => 3,
            Self::VoiceRecognitionUnit => 4,
        }
    }
}

impl SaveFeature {
    const fn tag(self) -> u8 {
        match self {
            Self::NoCartridgeSave => 0,
            Self::Eeprom4Kbit => 1,
            Self::Eeprom16Kbit => 2,
            Self::Sram32Kib => 3,
            Self::FlashRam128Kib => 4,
        }
    }
}

impl RendererFeature {
    const fn tag(self) -> u8 {
        match self {
            Self::ReferenceLleAccuracy => 0,
            Self::Rt64LleAccuracy => 1,
            Self::Rt64PostViCapture => 2,
            Self::Rt64ReplacementPacks => 3,
        }
    }
}

impl ProgramFeature {
    const fn tag(self) -> u8 {
        match self {
            Self::NativeArchive => 0,
            Self::TypedObservedFunction => 1,
            Self::TypedBlock => 2,
        }
    }
}

impl MicrocodeFeature {
    const fn tag(self) -> u8 {
        match self {
            Self::Fast3d => 0,
            Self::F3dex => 1,
            Self::F3dlx => 2,
            Self::F3dlxRej => 3,
            Self::F3dex2 => 4,
            Self::F3dex2NoN => 5,
            Self::F3dex2Rej => 6,
            Self::F3dlx2Rej => 7,
            Self::S2dex => 8,
            Self::S2dex2 => 9,
            Self::L3dex => 10,
            Self::L3dex2 => 11,
        }
    }
}

impl RspRdpMechanismFeature {
    const fn tag(self) -> u8 {
        match self {
            Self::DramDpc => 0,
            Self::XbusDpc => 1,
            Self::ImemReplacement => 2,
        }
    }
}

fn push_bytes(wire: &mut Vec<u8>, bytes: &[u8]) {
    wire.extend_from_slice(&(bytes.len() as u32).to_be_bytes());
    wire.extend_from_slice(bytes);
}

fn push_tags<T: Copy>(wire: &mut Vec<u8>, values: &[T], tag: impl Fn(T) -> u8) {
    let mut tags: Vec<u8> = values.iter().copied().map(tag).collect();
    tags.sort_unstable();
    wire.extend_from_slice(&(tags.len() as u32).to_be_bytes());
    wire.extend_from_slice(&tags);
}

fn validate_assignment_partition(
    profile: FullParityV1,
    assignments: &[CertificationRequirementAssignment],
    missing: &[CertificationRequirementRef],
    require_complete: bool,
) -> Result<(), ReleaseMatrixError> {
    let mut observed = BTreeMap::new();
    for assignment in assignments {
        assignment
            .requirement
            .verify_member(profile)
            .map_err(ReleaseMatrixError::InvalidCertificationProfile)?;
        if assignment.evidence_sha256s.is_empty() {
            return Err(ReleaseMatrixError::EmptyRequirementEvidence {
                class: assignment.requirement.class(),
                id: assignment.requirement.id().to_owned(),
            });
        }
        let mut evidence = BTreeSet::new();
        for digest in &assignment.evidence_sha256s {
            validate_sha256("certification-assignment", "evidence_sha256s", digest)?;
            if !evidence.insert(digest) {
                return Err(ReleaseMatrixError::DuplicateRequirementEvidence {
                    class: assignment.requirement.class(),
                    id: assignment.requirement.id().to_owned(),
                    sha256: digest.clone(),
                });
            }
        }
        if assignment.evidence_sha256s
            != evidence
                .iter()
                .map(|digest| (*digest).clone())
                .collect::<Vec<_>>()
        {
            return Err(ReleaseMatrixError::InvalidRequirementPartition);
        }
        let key = (
            assignment.requirement.class(),
            assignment.requirement.id().to_owned(),
        );
        if observed.insert(key.clone(), true).is_some() {
            return Err(ReleaseMatrixError::DuplicateRequirementAssignment {
                class: key.0,
                id: key.1,
            });
        }
    }
    for requirement in missing {
        requirement
            .verify_member(profile)
            .map_err(ReleaseMatrixError::InvalidCertificationProfile)?;
        let key = (requirement.class(), requirement.id().to_owned());
        if observed.insert(key.clone(), false).is_some() {
            return Err(ReleaseMatrixError::DuplicateRequirementAssignment {
                class: key.0,
                id: key.1,
            });
        }
    }

    let requirements = profile.requirements();
    let expected_keys: Vec<_> = requirements
        .iter()
        .map(|requirement| (requirement.class(), requirement.id().to_owned()))
        .collect();
    let actual_satisfied: Vec<_> = assignments
        .iter()
        .map(|assignment| {
            (
                assignment.requirement.class(),
                assignment.requirement.id().to_owned(),
            )
        })
        .collect();
    let expected_satisfied: Vec<_> = expected_keys
        .iter()
        .filter(|key| observed.get(*key) == Some(&true))
        .cloned()
        .collect();
    let actual_missing: Vec<_> = missing
        .iter()
        .map(|requirement| (requirement.class(), requirement.id().to_owned()))
        .collect();
    let expected_missing: Vec<_> = expected_keys
        .iter()
        .filter(|key| observed.get(*key) == Some(&false))
        .cloned()
        .collect();
    if observed.len() != expected_keys.len()
        || actual_satisfied != expected_satisfied
        || actual_missing != expected_missing
        || (require_complete && !missing.is_empty())
    {
        return Err(ReleaseMatrixError::InvalidRequirementPartition);
    }
    Ok(())
}

fn push_assignment(wire: &mut Vec<u8>, assignment: &CertificationRequirementAssignment) {
    push_bytes(wire, assignment.requirement.class().as_str().as_bytes());
    push_bytes(wire, assignment.requirement.id().as_bytes());
    wire.extend_from_slice(&(assignment.evidence_sha256s.len() as u32).to_be_bytes());
    for digest in &assignment.evidence_sha256s {
        push_bytes(wire, digest.as_bytes());
    }
}

fn push_platform_authority_identities(
    wire: &mut Vec<u8>,
    authorities: &[VerifiedRt64PlatformCaseAuthority],
) {
    let mut ordered = authorities.iter().collect::<Vec<_>>();
    ordered.sort_by_key(|authority| (authority.target, authority.case));
    wire.extend_from_slice(&(ordered.len() as u32).to_be_bytes());
    for authority in ordered {
        push_bytes(wire, authority.target.id().as_bytes());
        push_bytes(wire, authority.case.id().as_bytes());
        push_bytes(wire, authority.authority_sha256.as_bytes());
    }
}

fn incomplete_matrix_sha256(report: &IncompleteReleaseMatrix) -> String {
    let mut wire = Vec::new();
    wire.extend_from_slice(b"fn64.release-matrix-incomplete.v7\0");
    push_bytes(&mut wire, report.schema.as_bytes());
    push_bytes(&mut wire, report.manifest_sha256.as_bytes());
    push_bytes(&mut wire, report.profile.schema.as_bytes());
    push_bytes(&mut wire, report.profile.definition_sha256.as_bytes());
    wire.extend_from_slice(&(report.verified_scenarios as u64).to_be_bytes());
    wire.extend_from_slice(&(report.verified_reports as u64).to_be_bytes());
    push_bytes(
        &mut wire,
        report.unsupported_instrumentation.schema.as_bytes(),
    );
    push_bytes(
        &mut wire,
        report.unsupported_instrumentation.sha256.as_bytes(),
    );
    push_platform_authority_identities(&mut wire, &report.platform_case_authorities);
    wire.extend_from_slice(&(report.satisfied.len() as u32).to_be_bytes());
    for assignment in &report.satisfied {
        push_assignment(&mut wire, assignment);
    }
    wire.extend_from_slice(&(report.missing.len() as u32).to_be_bytes());
    for requirement in &report.missing {
        push_bytes(&mut wire, requirement.class().as_str().as_bytes());
        push_bytes(&mut wire, requirement.id().as_bytes());
    }
    hex(&Sha256::digest(wire))
}

fn verified_matrix_sha256(report: &VerifiedReleaseMatrix) -> String {
    let mut wire = Vec::new();
    wire.extend_from_slice(b"fn64.verified-release-matrix.v18\0");
    push_bytes(&mut wire, report.schema.as_bytes());
    push_bytes(&mut wire, report.manifest_sha256.as_bytes());
    push_bytes(&mut wire, report.profile.schema.as_bytes());
    push_bytes(&mut wire, report.profile.definition_sha256.as_bytes());
    wire.extend_from_slice(&(report.total_reports as u64).to_be_bytes());
    push_platform_authority_identities(&mut wire, &report.platform_case_authorities);

    let mut scenarios: Vec<_> = report.scenarios.iter().collect();
    scenarios.sort_by(|left, right| left.id.cmp(&right.id));
    wire.extend_from_slice(&(scenarios.len() as u32).to_be_bytes());
    for scenario in scenarios {
        push_bytes(&mut wire, scenario.id.as_bytes());
        wire.extend_from_slice(&(scenario.count as u64).to_be_bytes());
        push_bytes(&mut wire, scenario.report_sha256.as_bytes());
        push_bytes(&mut wire, scenario.report_scenario.as_bytes());
        push_bytes(&mut wire, scenario.input_sha256.as_bytes());
        push_bytes(
            &mut wire,
            scenario.unsupported_instrumentation.schema.as_bytes(),
        );
        push_bytes(
            &mut wire,
            scenario.unsupported_instrumentation.sha256.as_bytes(),
        );
        push_rom_evidence(&mut wire, &scenario.rom);
        push_rom_class_authority(&mut wire, &scenario.rom_class_authority);
        push_tags(&mut wire, &scenario.coverage.rom_classes, rom_class_tag);
        push_tags(&mut wire, &scenario.coverage.tv_regions, tv_region_tag);
        push_tags(
            &mut wire,
            &scenario.coverage.platforms,
            ReleasePlatform::tag,
        );
        push_tags(
            &mut wire,
            &scenario.coverage.controllers,
            ControllerFeature::tag,
        );
        push_tags(&mut wire, &scenario.coverage.saves, SaveFeature::tag);
        push_tags(
            &mut wire,
            &scenario.coverage.renderers,
            RendererFeature::tag,
        );
        push_tags(&mut wire, &scenario.coverage.programs, ProgramFeature::tag);
        push_tags(
            &mut wire,
            &scenario.coverage.microcodes,
            MicrocodeFeature::tag,
        );
        push_tags(
            &mut wire,
            &scenario.coverage.rsp_rdp_mechanisms,
            RspRdpMechanismFeature::tag,
        );
        push_bytes(&mut wire, scenario.declaration_sha256.as_bytes());
        wire.extend_from_slice(&scenario.guest_cycle.to_be_bytes());
        wire.extend_from_slice(&scenario.fixed_cycle_digest.guest_cycle.to_be_bytes());
        push_bytes(
            &mut wire,
            scenario.fixed_cycle_digest.root_sha256.as_bytes(),
        );
        let mut artifacts: Vec<&ArtifactDigest> =
            scenario.fixed_cycle_digest.artifacts.iter().collect();
        artifacts.sort_by_key(|artifact| artifact.kind);
        wire.extend_from_slice(&(artifacts.len() as u32).to_be_bytes());
        for artifact in artifacts {
            wire.push(match artifact.kind {
                ArtifactKind::Framebuffer => 0,
                ArtifactKind::Audio => 1,
                ArtifactKind::Memory => 2,
                ArtifactKind::DeviceState => 3,
                ArtifactKind::TimingTrace => 4,
            });
            wire.extend_from_slice(&artifact.bytes.to_be_bytes());
            push_bytes(&mut wire, artifact.sha256.as_bytes());
        }
        push_observations(&mut wire, &scenario.observations);
        push_environment(&mut wire, &scenario.environment);
        push_bytes(
            &mut wire,
            &crate::release_gate::encode_execution_destination_evidence(
                &scenario.execution_destinations,
            )
            .expect("verified destination evidence was validated before hashing"),
        );
        push_bytes(
            &mut wire,
            &crate::release_gate::encode_rsp_rdp_observations(&scenario.rsp_rdp.ordered)
                .expect("verified RSP/RDP evidence was validated before hashing"),
        );
        wire.extend_from_slice(&scenario.rsp_rdp.total_observations.to_be_bytes());
        push_bytes(&mut wire, scenario.rsp_rdp.ordered_sha256.as_bytes());
        wire.extend_from_slice(&scenario.closure_paths.to_be_bytes());
        wire.extend_from_slice(&(scenario.closure.len() as u64).to_be_bytes());
        for path in &scenario.closure {
            push_bytes(&mut wire, path.name.as_bytes());
            wire.extend_from_slice(&path.observations.to_be_bytes());
            wire.push(match path.status {
                ClosurePathStatus::Unexercised => 0,
                ClosurePathStatus::ExercisedZeroUnsupported => 1,
                ClosurePathStatus::ExercisedUnsupported => 2,
            });
            wire.extend_from_slice(&(path.unsupported.len() as u64).to_be_bytes());
            for unsupported in &path.unsupported {
                push_bytes(&mut wire, unsupported.subsystem.as_bytes());
                push_bytes(&mut wire, unsupported.operation.as_bytes());
                push_bytes(&mut wire, unsupported.context.as_bytes());
                match unsupported.guest_cycle {
                    Some(cycle) => {
                        wire.push(1);
                        wire.extend_from_slice(&cycle.to_be_bytes());
                    }
                    None => wire.push(0),
                }
                push_bytes(&mut wire, unsupported.disposition.as_bytes());
            }
        }
        wire.extend_from_slice(&scenario.unsupported_events.to_be_bytes());
        push_bytes(&mut wire, scenario.unsupported_journal_schema.as_bytes());
        wire.extend_from_slice(&(scenario.bound_journals as u64).to_be_bytes());
        wire.extend_from_slice(&(scenario.run_event_sha256s.len() as u64).to_be_bytes());
        for run_event_sha256 in &scenario.run_event_sha256s {
            push_bytes(&mut wire, run_event_sha256.as_bytes());
        }
        wire.push(match scenario.presentation_boundary {
            PresentationBoundaryEvidence::CommittedViBoundary => 0,
            PresentationBoundaryEvidence::ExactPostViCapture => 1,
        });
    }
    wire.extend_from_slice(&(report.assignments.len() as u32).to_be_bytes());
    for assignment in &report.assignments {
        push_assignment(&mut wire, assignment);
    }
    hex(&Sha256::digest(wire))
}

fn push_rom_class_authority(wire: &mut Vec<u8>, authority: &Option<VerifiedRomClassAuthority>) {
    let Some(authority) = authority else {
        wire.push(0);
        return;
    };
    wire.push(1);
    push_bytes(wire, authority.schema.as_bytes());
    push_bytes(wire, authority.contract_schema.as_bytes());
    push_bytes(wire, authority.contract_sha256.as_bytes());
    push_bytes(wire, authority.receipt_schema.as_bytes());
    push_bytes(wire, authority.receipt_sha256.as_bytes());
    push_bytes(wire, authority.runner_executable_sha256.as_bytes());
    push_bytes(wire, authority.purpose.as_bytes());
    push_bytes(wire, authority.report_scenario.as_bytes());
    push_bytes(wire, authority.input_sha256.as_bytes());
    wire.extend_from_slice(&authority.input_bytes.to_be_bytes());
    wire.push(rom_class_tag(authority.rom_class));
    wire.extend_from_slice(&authority.guest_cycle.to_be_bytes());
    push_execution_source(wire, &authority.expected_execution_source);
    push_bytes(wire, authority.child_executable_sha256.as_bytes());
    push_bytes(wire, authority.semantic_report_sha256.as_bytes());
    wire.extend_from_slice(&(authority.run_event_sha256s.len() as u64).to_be_bytes());
    for run_event_sha256 in &authority.run_event_sha256s {
        push_bytes(wire, run_event_sha256.as_bytes());
    }
    push_bytes(wire, authority.authority_sha256.as_bytes());
}

const fn rom_class_tag(value: ReleaseRomClass) -> u8 {
    match value {
        ReleaseRomClass::Unclassified => 0,
        ReleaseRomClass::RetailCartridge => 1,
        ReleaseRomClass::PublicHomebrew => 2,
    }
}

fn push_execution_source(wire: &mut Vec<u8>, source: &ExecutionDestinationSource) {
    match source {
        ExecutionDestinationSource::NoProgram => wire.push(0),
        ExecutionDestinationSource::NativeArchive { artifact_sha256 } => {
            wire.push(1);
            push_bytes(wire, artifact_sha256.as_bytes());
        }
        ExecutionDestinationSource::TypedObservedFunctionProgram { artifact_sha256 } => {
            wire.push(2);
            push_bytes(wire, artifact_sha256.as_bytes());
        }
        ExecutionDestinationSource::TypedBlockProgram {
            program_sha256,
            dispatch_artifact_sha256,
        } => {
            wire.push(3);
            push_bytes(wire, program_sha256.as_bytes());
            push_bytes(wire, dispatch_artifact_sha256.as_bytes());
        }
    }
}

fn validate_execution_source_identity(
    id: &str,
    source: &ExecutionDestinationSource,
) -> Result<(), ReleaseMatrixError> {
    match source {
        ExecutionDestinationSource::NoProgram => {
            Err(ReleaseMatrixError::InvalidRomClassAuthority {
                id: id.to_owned(),
                detail: "production ROM-class authority cannot name NoProgram".to_owned(),
            })
        }
        ExecutionDestinationSource::NativeArchive { artifact_sha256 }
        | ExecutionDestinationSource::TypedObservedFunctionProgram { artifact_sha256 } => {
            validate_sha256(
                id,
                "authority.execution_source.artifact_sha256",
                artifact_sha256,
            )
        }
        ExecutionDestinationSource::TypedBlockProgram {
            program_sha256,
            dispatch_artifact_sha256,
        } => {
            validate_sha256(
                id,
                "authority.execution_source.program_sha256",
                program_sha256,
            )?;
            validate_sha256(
                id,
                "authority.execution_source.dispatch_artifact_sha256",
                dispatch_artifact_sha256,
            )
        }
    }
}

fn push_rom_evidence(wire: &mut Vec<u8>, rom: &Option<ReleaseRomEvidence>) {
    let Some(rom) = rom else {
        wire.push(0);
        return;
    };
    wire.push(1);
    wire.push(match rom.class {
        crate::ReleaseRomClass::Unclassified => 0,
        crate::ReleaseRomClass::RetailCartridge => 1,
        crate::ReleaseRomClass::PublicHomebrew => 2,
    });
    wire.push(match rom.source_byte_order {
        crate::ReleaseRomByteOrder::Z64 => 0,
        crate::ReleaseRomByteOrder::N64 => 1,
        crate::ReleaseRomByteOrder::V64 => 2,
    });
    wire.extend_from_slice(&rom.byte_len.to_be_bytes());
    push_bytes(wire, rom.canonical_sha256.as_bytes());
    wire.push(rom.destination_code);
    wire.push(tv_region_tag(rom.decoded_tv_region));
    wire.push(match rom.configured_tv_type {
        crate::ReleaseTvStandard::Ntsc => 0,
        crate::ReleaseTvStandard::Pal => 1,
        crate::ReleaseTvStandard::Mpal => 2,
    });
}

const fn tv_region_tag(value: ReleaseTvRegion) -> u8 {
    match value {
        ReleaseTvRegion::Ntsc => 0,
        ReleaseTvRegion::Pal => 1,
        ReleaseTvRegion::Mpal => 2,
        ReleaseTvRegion::RegionFree => 3,
    }
}

fn push_observations(wire: &mut Vec<u8>, observations: &ReleaseObservationGeometry) {
    match &observations.framebuffer.source {
        FramebufferObservationSource::PhysicalRdram { address } => {
            wire.push(0);
            wire.extend_from_slice(&address.to_be_bytes());
        }
        FramebufferObservationSource::PostViSwapchain {
            backend_identity,
            settings_sha256,
            workload_id,
            present_id,
        } => {
            wire.push(1);
            push_bytes(wire, backend_identity.as_bytes());
            push_bytes(wire, settings_sha256.as_bytes());
            wire.extend_from_slice(&workload_id.get().to_be_bytes());
            wire.extend_from_slice(&present_id.to_be_bytes());
        }
    }
    wire.extend_from_slice(&observations.framebuffer.width.to_be_bytes());
    wire.extend_from_slice(&observations.framebuffer.height.to_be_bytes());
    wire.extend_from_slice(&observations.framebuffer.row_bytes.to_be_bytes());
    wire.push(observations.framebuffer.format.tag());
    wire.extend_from_slice(&observations.framebuffer.payload_bytes.to_be_bytes());
    wire.extend_from_slice(&observations.memory.physical_address.to_be_bytes());
    wire.extend_from_slice(&observations.memory.payload_bytes.to_be_bytes());
}

fn push_environment(wire: &mut Vec<u8>, environment: &ReleaseEnvironmentEvidence) {
    wire.push(match environment.platform {
        ReleaseHostPlatform::MacosArm64 => 0,
        ReleaseHostPlatform::LinuxX86_64 => 1,
        ReleaseHostPlatform::WindowsX86_64 => 2,
    });
    match environment.windows_version {
        None => wire.push(0),
        Some(version) => {
            wire.push(1);
            wire.push(match version.family {
                ReleaseWindowsFamily::Windows10 => 0,
                ReleaseWindowsFamily::Windows11 => 1,
            });
            wire.extend_from_slice(&version.major.to_be_bytes());
            wire.extend_from_slice(&version.minor.to_be_bytes());
            wire.extend_from_slice(&version.build.to_be_bytes());
            wire.extend_from_slice(&version.update_build_revision.to_be_bytes());
            wire.push(0);
        }
    }
    for port in environment.controller_ports {
        wire.push(match port {
            ReleaseControllerPort::StandardControllerNoPak => 0,
            ReleaseControllerPort::StandardControllerControllerPak => 1,
            ReleaseControllerPort::StandardControllerRumblePak => 2,
            ReleaseControllerPort::StandardControllerTransferPak => 3,
            ReleaseControllerPort::VoiceRecognitionUnit => 4,
            ReleaseControllerPort::Absent => 5,
        });
    }
    wire.push(match environment.cartridge_save {
        ReleaseCartridgeSave::NoCartridgeSave => 0,
        ReleaseCartridgeSave::Eeprom4k => 1,
        ReleaseCartridgeSave::Eeprom16k => 2,
        ReleaseCartridgeSave::Sram32Kib => 3,
        ReleaseCartridgeSave::FlashRam128Kib => 4,
    });
    match &environment.renderer {
        ReleaseRendererEvidence::Reference {
            execution_policy,
            tv_type,
        } => {
            wire.push(0);
            wire.push(release_execution_policy_tag(*execution_policy));
            wire.push(match tv_type {
                crate::ReleaseTvStandard::Ntsc => 0,
                crate::ReleaseTvStandard::Pal => 1,
                crate::ReleaseTvStandard::Mpal => 2,
            });
        }
        ReleaseRendererEvidence::Rt64 {
            execution_policy,
            tv_type,
            graphics_api,
            backend_identity,
            source_authoritative,
            settings_sha256,
            replacement_packs_active,
        } => {
            wire.push(1);
            wire.push(release_execution_policy_tag(*execution_policy));
            wire.push(match tv_type {
                crate::ReleaseTvStandard::Ntsc => 0,
                crate::ReleaseTvStandard::Pal => 1,
                crate::ReleaseTvStandard::Mpal => 2,
            });
            wire.push(release_graphics_api_tag(*graphics_api));
            push_bytes(wire, backend_identity.as_bytes());
            wire.push(*source_authoritative as u8);
            push_bytes(wire, settings_sha256.as_bytes());
            wire.push(*replacement_packs_active as u8);
        }
    }
}

const fn release_execution_policy_tag(policy: ReleaseGraphicsExecutionPolicy) -> u8 {
    match policy {
        ReleaseGraphicsExecutionPolicy::HleOptimized => 0,
        ReleaseGraphicsExecutionPolicy::LleAccuracy => 1,
    }
}

const fn release_graphics_api_tag(api: ReleaseGraphicsApi) -> u8 {
    match api {
        ReleaseGraphicsApi::D3d12 => 0,
        ReleaseGraphicsApi::Vulkan => 1,
        ReleaseGraphicsApi::Metal => 2,
    }
}

fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(DIGITS[(byte >> 4) as usize] as char);
        out.push(DIGITS[(byte & 0xf) as usize] as char);
    }
    out
}

#[derive(Debug)]
pub enum ReleaseMatrixError {
    UnsupportedSchema(String),
    UnsupportedVerifiedSchema(String),
    UnsupportedIncompleteSchema(String),
    InvalidCertificationProfile(crate::CertificationProfileError),
    InvalidUnsupportedInstrumentation(crate::GateError),
    InvalidUnassignedReport {
        scenario: String,
        source: crate::GateError,
    },
    InvalidPrivateSeriesAuthority {
        source: crate::PrivateReleaseSeriesError,
    },
    InvalidPlatformSeriesAuthority {
        source: PlatformCertificationError,
    },
    DuplicatePlatformSeriesAuthority {
        target: String,
        case: String,
    },
    UnusedPlatformSeriesAuthority {
        target: String,
        case: String,
    },
    PlatformAuthorityAssignmentMismatch {
        target: String,
        case: String,
    },
    DuplicatePrivateSeriesAuthority {
        report_scenario: String,
    },
    UnusedPrivateSeriesAuthority {
        report_scenario: String,
    },
    InvalidRomClassAuthority {
        id: String,
        detail: String,
    },
    RomClassAuthorityMismatch {
        id: String,
        field: &'static str,
    },
    UnexpectedReportScenario {
        scenario: String,
    },
    VerifiedDerivedCoverageMismatch {
        id: String,
    },
    VerifiedProfileIncomplete {
        missing: Vec<String>,
    },
    VerifiedAssignmentsMismatch,
    InvalidIncompleteCounts {
        scenarios: usize,
        reports: usize,
    },
    IncompleteWithoutMissing,
    IncompleteIntegrityMismatch {
        stored: String,
        recomputed: String,
    },
    EmptyRequirementEvidence {
        class: CertificationRequirementClass,
        id: String,
    },
    DuplicateRequirementEvidence {
        class: CertificationRequirementClass,
        id: String,
        sha256: String,
    },
    DuplicateRequirementAssignment {
        class: CertificationRequirementClass,
        id: String,
    },
    InvalidRequirementPartition,
    VerifiedReportCountMismatch {
        stored: usize,
        recomputed: usize,
    },
    VerifiedScenarioReportCount {
        id: String,
        expected: usize,
        actual: usize,
    },
    VerifiedCycleMismatch {
        id: String,
        scenario_cycle: u64,
        digest_cycle: u64,
    },
    VerifiedArtifactSet {
        id: String,
    },
    InvalidVerifiedDigest {
        id: String,
        source: crate::GateError,
    },
    InvalidVerifiedReport {
        id: String,
        source: crate::GateError,
    },
    InvalidVerifiedDestinations {
        id: String,
        source: crate::GateError,
    },
    VerifiedUnsupportedEvents {
        id: String,
        count: u64,
    },
    VerifiedClosurePathCount {
        id: String,
        minimum: u64,
        actual: u64,
    },
    VerifiedClosurePathCountMismatch {
        id: String,
        stored: u64,
        observed: u64,
    },
    VerifiedUnsupportedEventCountMismatch {
        id: String,
        stored: u64,
        observed: u64,
    },
    InvalidVerifiedClosure {
        id: String,
        source: crate::GateError,
    },
    InvalidVerifiedObservations {
        id: String,
        source: crate::ObservationEvidenceError,
    },
    VerifiedPresentationMismatch {
        id: String,
        stored: PresentationBoundaryEvidence,
        expected: PresentationBoundaryEvidence,
    },
    VerifiedJournalBinding {
        id: String,
        schema: String,
        reports: usize,
        journals: usize,
    },
    VerifiedRunEventIdentities {
        id: String,
        reports: usize,
        identities: usize,
        unique: usize,
    },
    DuplicateRunEventIdentity {
        id: String,
        run_event_sha256: String,
    },
    VerifiedManifestIdentityMismatch {
        stored: String,
        recomputed: String,
    },
    VerifiedIntegrityMismatch {
        stored: String,
        recomputed: String,
    },
    ScenarioCount {
        minimum: usize,
        maximum: usize,
        actual: usize,
    },
    InvalidScenarioId(String),
    DuplicateScenarioId(String),
    InvalidReportScenario {
        id: String,
    },
    DuplicateReportScenario(String),
    InvalidSha256 {
        id: String,
        field: &'static str,
    },
    DeclarationDigestMismatch {
        id: String,
        stored: String,
        recomputed: String,
    },
    EmptyCoverage {
        scope: String,
        dimension: &'static str,
    },
    DuplicateCoverage {
        scope: String,
        dimension: &'static str,
        value: String,
    },
    UndeclaredCoverage {
        scope: String,
        dimension: &'static str,
        value: String,
    },
    MissingRequiredCoverage {
        dimension: &'static str,
        missing: Vec<String>,
    },
    ExactOneCoverage {
        id: String,
        dimension: &'static str,
        actual: usize,
    },
    InvalidRendererCombination {
        id: String,
    },
    RendererObservationMismatch {
        id: String,
        expected: &'static str,
        observed: &'static str,
    },
    RendererEnvironmentMismatch {
        id: String,
    },
    NoProgramEvidence {
        id: String,
    },
    ProgramCoverageMismatch {
        id: String,
        expected: ProgramFeature,
        observed: ProgramFeature,
    },
    EnvironmentCoverageMismatch {
        id: String,
        dimension: &'static str,
        declared: Vec<String>,
        observed: Vec<String>,
    },
    NonAuthoritativeRt64Identity {
        id: String,
        backend_identity: String,
    },
    DuplicateCertifiedMicrocodeIdentity {
        text_sha256: String,
    },
    CertifiedMicrocodeFamilyMismatch {
        id: String,
        text_sha256: String,
        certified: ReleaseMicrocodeFamily,
        observed: ReleaseMicrocodeFamily,
    },
    MissingEvidence {
        id: String,
    },
    UnexpectedEvidence {
        id: String,
    },
    WrongReportCount {
        id: String,
        expected: usize,
        actual: usize,
    },
    InvalidSeries {
        id: String,
        source: Box<ReportSeriesError>,
    },
    InvalidLivePathEvidence {
        id: String,
        path: String,
        observations: u64,
        status: ClosurePathStatus,
        unsupported: usize,
    },
    MissingFeatureObservation {
        id: String,
        path: String,
    },
    MissingLiveMinimumObservation {
        id: String,
        path: String,
    },
    UnexpectedFeatureObservation {
        id: String,
        path: String,
    },
    ReportScenarioMismatch {
        id: String,
        expected: String,
        observed: String,
    },
    InputDigestMismatch {
        id: String,
        expected: String,
        observed: String,
    },
    ReportDigestMismatch {
        id: String,
        expected: String,
        observed: String,
    },
}

impl fmt::Display for ReleaseMatrixError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnsupportedSchema(schema) => write!(f, "unsupported release-matrix schema {schema:?}"),
            Self::UnsupportedVerifiedSchema(schema) => write!(f, "unsupported verified release-matrix schema {schema:?}"),
            Self::UnsupportedIncompleteSchema(schema) => write!(f, "unsupported incomplete release-matrix schema {schema:?}"),
            Self::InvalidCertificationProfile(source) => write!(f, "invalid certification profile: {source}"),
            Self::InvalidUnsupportedInstrumentation(source) => write!(f, "invalid unsupported-instrumentation identity: {source}"),
            Self::InvalidUnassignedReport { scenario, source } => write!(f, "release report scenario {scenario:?} is invalid before matrix assignment: {source}"),
            Self::InvalidPrivateSeriesAuthority { source } => write!(f, "private release series authority failed fresh revalidation: {source}"),
            Self::InvalidPlatformSeriesAuthority { source } => write!(f, "RT64 platform-case series authority failed fresh revalidation: {source}"),
            Self::DuplicatePlatformSeriesAuthority { target, case } => write!(f, "RT64 platform-case authority for {target}/{case} was supplied more than once"),
            Self::UnusedPlatformSeriesAuthority { target, case } => write!(f, "RT64 platform-case authority for {target}/{case} does not bind any exact retained matrix report series"),
            Self::PlatformAuthorityAssignmentMismatch { target, case } => write!(f, "RT64 platform-case authority for {target}/{case} does not match its retained requirement assignment"),
            Self::DuplicatePrivateSeriesAuthority { report_scenario } => write!(f, "private release series authority for report scenario {report_scenario:?} was supplied more than once"),
            Self::UnusedPrivateSeriesAuthority { report_scenario } => write!(f, "private release series authority for report scenario {report_scenario:?} has no declared matrix scenario"),
            Self::InvalidRomClassAuthority { id, detail } => write!(f, "release-matrix scenario {id:?} has invalid ROM-class authority: {detail}"),
            Self::RomClassAuthorityMismatch { id, field } => write!(f, "release-matrix scenario {id:?} ROM-class authority does not match retained report field {field}"),
            Self::UnexpectedReportScenario { scenario } => write!(f, "release report scenario {scenario:?} is not declared by the matrix manifest"),
            Self::VerifiedDerivedCoverageMismatch { id } => write!(f, "verified release-matrix scenario {id:?} stores coverage that does not match its retained report evidence"),
            Self::VerifiedProfileIncomplete { missing } => write!(f, "verified release matrix is missing project-owned profile requirements {missing:?}"),
            Self::VerifiedAssignmentsMismatch => write!(f, "verified release-matrix requirement assignments do not match retained scenario evidence"),
            Self::InvalidIncompleteCounts { scenarios, reports } => write!(f, "incomplete release matrix has invalid verified counts: scenarios={scenarios}, reports={reports}"),
            Self::IncompleteWithoutMissing => write!(f, "incomplete release matrix has no missing requirements"),
            Self::IncompleteIntegrityMismatch { stored, recomputed } => write!(f, "incomplete release-matrix assessment SHA mismatch: stored={stored}, recomputed={recomputed}"),
            Self::EmptyRequirementEvidence { class, id } => write!(f, "certification requirement ({}:{id}) has no validating evidence identity", class.as_str()),
            Self::DuplicateRequirementEvidence { class, id, sha256 } => write!(f, "certification requirement ({}:{id}) repeats evidence identity {sha256}", class.as_str()),
            Self::DuplicateRequirementAssignment { class, id } => write!(f, "certification requirement ({}:{id}) appears more than once in the outcome partition", class.as_str()),
            Self::InvalidRequirementPartition => write!(f, "certification outcome is not the canonical complete partition of the project-owned profile"),
            Self::VerifiedReportCountMismatch { stored, recomputed } => write!(f, "verified release matrix stores total_reports={stored}, recomputed={recomputed}"),
            Self::VerifiedScenarioReportCount { id, expected, actual } => write!(f, "verified release-matrix scenario {id:?} has {actual} reports; exactly {expected} are required"),
            Self::VerifiedCycleMismatch { id, scenario_cycle, digest_cycle } => write!(f, "verified release-matrix scenario {id:?} names cycle {scenario_cycle}, but its fixed-cycle digest names {digest_cycle}"),
            Self::VerifiedArtifactSet { id } => write!(f, "verified release-matrix scenario {id:?} does not contain each fixed-cycle artifact exactly once"),
            Self::InvalidVerifiedDigest { id, source } => write!(f, "verified release-matrix scenario {id:?} has invalid fixed-cycle evidence: {source}"),
            Self::InvalidVerifiedReport { id, source } => write!(f, "verified release-matrix scenario {id:?} does not reconstruct its retained release report: {source}"),
            Self::InvalidVerifiedDestinations { id, source } => write!(f, "verified release-matrix scenario {id:?} has invalid execution-destination evidence: {source}"),
            Self::VerifiedUnsupportedEvents { id, count } => write!(f, "verified release-matrix scenario {id:?} contains {count} unsupported events"),
            Self::VerifiedClosurePathCount { id, minimum, actual } => write!(f, "verified release-matrix scenario {id:?} retains {actual} closure paths; at least {minimum} live minimum paths are required"),
            Self::VerifiedClosurePathCountMismatch { id, stored, observed } => write!(f, "verified release-matrix scenario {id:?} stores closure_paths={stored}, but retains {observed} exact closure entries"),
            Self::VerifiedUnsupportedEventCountMismatch { id, stored, observed } => write!(f, "verified release-matrix scenario {id:?} stores unsupported_events={stored}, but its exact closure retains {observed}"),
            Self::InvalidVerifiedClosure { id, source } => write!(f, "verified release-matrix scenario {id:?} has invalid exact closure evidence: {source}"),
            Self::InvalidVerifiedObservations { id, source } => write!(f, "verified release-matrix scenario {id:?} has invalid observation geometry: {source}"),
            Self::VerifiedPresentationMismatch { id, stored, expected } => write!(f, "verified release-matrix scenario {id:?} stores presentation boundary {stored:?}, expected {expected:?} from its observation source"),
            Self::VerifiedJournalBinding { id, schema, reports, journals } => write!(f, "verified release-matrix scenario {id:?} has journal schema {schema:?} and {journals} bindings for {reports} reports; exact v3 pairing is required"),
            Self::VerifiedRunEventIdentities { id, reports, identities, unique } => write!(f, "verified release-matrix scenario {id:?} retains {identities} run-event identities ({unique} unique) for {reports} reports; exactly one unique identity per report is required"),
            Self::DuplicateRunEventIdentity { id, run_event_sha256 } => write!(f, "release-matrix scenario {id:?} repeats run-event identity {run_event_sha256} already retained by the matrix"),
            Self::VerifiedManifestIdentityMismatch { stored, recomputed } => write!(f, "verified release-matrix manifest SHA mismatch: stored={stored}, recomputed={recomputed}"),
            Self::VerifiedIntegrityMismatch { stored, recomputed } => write!(f, "verified release-matrix SHA mismatch: stored={stored}, recomputed={recomputed}"),
            Self::ScenarioCount { minimum, maximum, actual } => write!(f, "release matrix has {actual} scenarios; required range is {minimum}..={maximum}"),
            Self::InvalidScenarioId(id) => write!(f, "release-matrix scenario id {id:?} is not a 1..=64 byte lowercase slug"),
            Self::DuplicateScenarioId(id) => write!(f, "release-matrix scenario id {id:?} is declared twice"),
            Self::InvalidReportScenario { id } => write!(f, "release-matrix scenario {id:?} has an empty, overlong, or control-bearing report_scenario"),
            Self::DuplicateReportScenario(scenario) => write!(f, "release report scenario {scenario:?} is assigned more than once"),
            Self::InvalidSha256 { id, field } => write!(f, "release-matrix scenario {id:?} field {field} is not lowercase SHA-256"),
            Self::DeclarationDigestMismatch { id, stored, recomputed } => write!(f, "release-matrix scenario {id:?} declaration SHA mismatch: stored={stored}, recomputed={recomputed}"),
            Self::EmptyCoverage { scope, dimension } => write!(f, "release-matrix {scope:?} has no {dimension} coverage"),
            Self::DuplicateCoverage { scope, dimension, value } => write!(f, "release-matrix {scope:?} repeats {dimension} value {value}"),
            Self::UndeclaredCoverage { scope, dimension, value } => write!(f, "release-matrix {scope:?} uses {dimension} value {value} outside required coverage"),
            Self::MissingRequiredCoverage { dimension, missing } => write!(f, "release matrix does not assign required {dimension} coverage {missing:?}"),
            Self::ExactOneCoverage { id, dimension, actual } => write!(f, "release-matrix scenario {id:?} declares {actual} {dimension}; exactly one is required"),
            Self::InvalidRendererCombination { id } => write!(f, "release-matrix scenario {id:?} must select reference LLE alone or RT64 LLE with optional RT64 capabilities"),
            Self::RendererObservationMismatch { id, expected, observed } => write!(f, "release-matrix scenario {id:?} requires {expected} framebuffer evidence, observed {observed}"),
            Self::RendererEnvironmentMismatch { id } => write!(f, "release-matrix scenario {id:?} renderer environment does not match its framebuffer observation"),
            Self::NoProgramEvidence { id } => write!(f, "release-matrix scenario {id:?} has no executable-entry evidence; representative full-ROM certification requires an observed program lane"),
            Self::ProgramCoverageMismatch { id, expected, observed } => write!(f, "release-matrix scenario {id:?} declares program lane {expected:?}, but its execution destinations prove {observed:?}"),
            Self::EnvironmentCoverageMismatch { id, dimension, declared, observed } => write!(f, "release-matrix scenario {id:?} declares {dimension} {declared:?}, but its committed-boundary environment observed {observed:?}"),
            Self::NonAuthoritativeRt64Identity { id, backend_identity } => write!(f, "release-matrix scenario {id:?} has non-authoritative RT64 backend identity {backend_identity:?}"),
            Self::DuplicateCertifiedMicrocodeIdentity { text_sha256 } => write!(f, "project-owned certified-microcode catalog repeats digest {text_sha256}"),
            Self::CertifiedMicrocodeFamilyMismatch { id, text_sha256, certified, observed } => write!(f, "release-matrix scenario {id:?} reports microcode digest {text_sha256} as {observed:?}, but the project-owned catalog adjudicates it as {certified:?}"),
            Self::MissingEvidence { id } => write!(f, "release-matrix scenario {id:?} has no report evidence"),
            Self::UnexpectedEvidence { id } => write!(f, "report evidence names undeclared release-matrix scenario {id:?}"),
            Self::WrongReportCount { id, expected, actual } => write!(f, "release-matrix scenario {id:?} has {actual} reports; exactly {expected} are required"),
            Self::InvalidSeries { id, source } => write!(f, "release-matrix scenario {id:?} has an invalid report series: {source}"),
            Self::InvalidLivePathEvidence { id, path, observations, status, unsupported } => write!(f, "release-matrix scenario {id:?} live path {path:?} is not positive zero-unsupported evidence: observations={observations}, status={status:?}, unsupported={unsupported}"),
            Self::MissingFeatureObservation { id, path } => write!(f, "release-matrix scenario {id:?} declares a feature without required positive operation path {path:?}"),
            Self::MissingLiveMinimumObservation { id, path } => write!(f, "release-matrix scenario {id:?} is missing required live minimum path {path:?}"),
            Self::UnexpectedFeatureObservation { id, path } => write!(f, "release-matrix scenario {id:?} observed feature path {path:?} outside its exact declaration"),
            Self::ReportScenarioMismatch { id, expected, observed } => write!(f, "release-matrix scenario {id:?} expected report scenario {expected:?}, observed {observed:?}"),
            Self::InputDigestMismatch { id, expected, observed } => write!(f, "release-matrix scenario {id:?} expected input SHA {expected}, observed {observed}"),
            Self::ReportDigestMismatch { id, expected, observed } => write!(f, "release-matrix scenario {id:?} expected report SHA {expected}, observed {observed}"),
        }
    }
}

impl std::error::Error for ReleaseMatrixError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::InvalidCertificationProfile(source) => Some(source),
            Self::InvalidUnsupportedInstrumentation(source) => Some(source),
            Self::InvalidUnassignedReport { source, .. } => Some(source),
            Self::InvalidPrivateSeriesAuthority { source } => Some(source),
            Self::InvalidPlatformSeriesAuthority { source } => Some(source),
            Self::InvalidSeries { source, .. } => Some(source.as_ref()),
            Self::InvalidVerifiedObservations { source, .. } => Some(source),
            Self::InvalidVerifiedDigest { source, .. } => Some(source),
            Self::InvalidVerifiedReport { source, .. } => Some(source),
            Self::InvalidVerifiedDestinations { source, .. } => Some(source),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        load_private_release_run_contract, materialize_release_program_build_receipt,
        parse_unsupported_journal, run_private_release_series, verify_private_release_series,
        ReleaseProgramBuildReceiptInput,
    };
    use crate::{
        ArtifactKind, ClosurePath, ClosurePathStatus, FixedCycleDigestGate, LiveRenderEvidence,
        RenderPixelFormat, RspRdpObservationEventEvidence, LIVE_MINIMUM_CLOSURE_PATHS,
    };
    use std::{
        fs,
        io::Write as _,
        path::{Path, PathBuf},
        sync::atomic::{AtomicU64, Ordering},
    };

    static PRODUCTION_MATRIX_FIXTURE_COUNTER: AtomicU64 = AtomicU64::new(0);

    struct ProductionMatrixFixtureDirectory(PathBuf);

    impl ProductionMatrixFixtureDirectory {
        fn new() -> Self {
            let counter = PRODUCTION_MATRIX_FIXTURE_COUNTER.fetch_add(1, Ordering::Relaxed);
            let base = if Path::new("/private/tmp").is_dir() {
                PathBuf::from("/private/tmp")
            } else {
                std::env::temp_dir()
            };
            let path = base.join(format!(
                "fn64-production-matrix-fixture-{}-{counter}",
                std::process::id()
            ));
            fs::create_dir(&path).unwrap();
            Self(path)
        }
    }

    impl Drop for ProductionMatrixFixtureDirectory {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    const CLEAN_RT64_IDENTITY: &str = concat!(
        "adapter=fn64-render-rt64/rt64;adapter_sha256=",
        "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        ";source=git:",
        "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
        ";provenance=git-clean;overlay=fn64-test;post_vi_api=vulkan-bgra8-rgba8-unorm"
    );

    fn clean_rt64_identity_for(api: ReleaseGraphicsApi) -> String {
        let post_vi_api = match api {
            ReleaseGraphicsApi::D3d12 => "d3d12-bgra8-rgba8-unorm",
            ReleaseGraphicsApi::Vulkan => "vulkan-bgra8-rgba8-unorm",
            ReleaseGraphicsApi::Metal => "metal-bgra8-unorm",
        };
        format!(
            "adapter=fn64-render-rt64/rt64;adapter_sha256={};source=git:{};provenance=git-clean;overlay=fn64-test;post_vi_api={post_vi_api}",
            "aa".repeat(32),
            "bb".repeat(20),
        )
    }

    fn closed_report(
        scenario: &str,
        input: &[u8],
        framebuffer_byte: u8,
        feature_path: &str,
        rt64_identity: &str,
        program: Option<ProgramFeature>,
    ) -> ReleaseGateReport {
        closed_report_with_rt64_environment(
            scenario,
            input,
            framebuffer_byte,
            feature_path,
            rt64_identity,
            program,
            ReleaseHostPlatform::LinuxX86_64,
            ReleaseGraphicsApi::Vulkan,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn closed_report_with_rt64_environment(
        scenario: &str,
        input: &[u8],
        framebuffer_byte: u8,
        feature_path: &str,
        rt64_identity: &str,
        program: Option<ProgramFeature>,
        rt64_platform: ReleaseHostPlatform,
        rt64_graphics_api: ReleaseGraphicsApi,
    ) -> ReleaseGateReport {
        let (observations, framebuffer_artifact) = if scenario.contains("rt64") {
            let render = LiveRenderEvidence::post_vi_swapchain(
                100,
                rt64_identity,
                [0x11; 32],
                1,
                1,
                4,
                RenderPixelFormat::Bgra8Unorm,
                1,
                1,
                vec![framebuffer_byte; 4],
            )
            .unwrap();
            (
                ReleaseObservationGeometry::post_vi_swapchain(
                    rt64_identity,
                    "11".repeat(32),
                    1,
                    1,
                    1,
                    1,
                    4,
                    4,
                )
                .unwrap(),
                render.canonical_bytes(),
            )
        } else {
            (
                ReleaseObservationGeometry::reference_rdram(0, 1, 1).unwrap(),
                vec![framebuffer_byte; 2],
            )
        };
        let mut digest = FixedCycleDigestGate::new(100);
        digest
            .capture(100, ArtifactKind::Framebuffer, &framebuffer_artifact)
            .unwrap();
        for kind in [
            ArtifactKind::Audio,
            ArtifactKind::DeviceState,
            ArtifactKind::TimingTrace,
        ] {
            digest.capture(100, kind, &[kind as u8]).unwrap();
        }
        digest
            .capture(
                100,
                ArtifactKind::Memory,
                &vec![0; crate::DEFAULT_RDRAM_SIZE],
            )
            .unwrap();
        let mut closure: Vec<_> = LIVE_MINIMUM_CLOSURE_PATHS
            .iter()
            .map(|name| ClosurePath {
                name: (*name).to_owned(),
                observations: 1,
                status: ClosurePathStatus::ExercisedZeroUnsupported,
                unsupported: Vec::new(),
            })
            .collect();
        closure.push(ClosurePath {
            name: feature_path.to_owned(),
            observations: 1,
            status: ClosurePathStatus::ExercisedZeroUnsupported,
            unsupported: Vec::new(),
        });
        let is_rt64 = scenario.contains("rt64");
        for path in if is_rt64 {
            [
                Some("controller.standard-input-read"),
                Some("controller.rumble-operation"),
            ]
        } else {
            [Some("controller.standard-input-read"), None]
        }
        .into_iter()
        .flatten()
        {
            closure.push(ClosurePath {
                name: path.to_owned(),
                observations: 1,
                status: ClosurePathStatus::ExercisedZeroUnsupported,
                unsupported: Vec::new(),
            });
        }
        let environment = ReleaseEnvironmentEvidence {
            platform: if is_rt64 {
                rt64_platform
            } else {
                ReleaseHostPlatform::MacosArm64
            },
            windows_version: (is_rt64 && rt64_platform == ReleaseHostPlatform::WindowsX86_64).then(
                || {
                    crate::ReleaseWindowsVersionEvidence::from_native_workstation(10, 0, 19_045, 1)
                        .unwrap()
                },
            ),
            controller_ports: [
                if is_rt64 {
                    ReleaseControllerPort::StandardControllerRumblePak
                } else {
                    ReleaseControllerPort::StandardControllerNoPak
                },
                ReleaseControllerPort::Absent,
                ReleaseControllerPort::Absent,
                ReleaseControllerPort::Absent,
            ],
            cartridge_save: if feature_path == "save.sram-operation" {
                ReleaseCartridgeSave::Sram32Kib
            } else {
                ReleaseCartridgeSave::Eeprom4k
            },
            audio_task_execution: crate::ReleaseAudioTaskExecutionPolicy::LleAccuracy,
            renderer: if is_rt64 {
                ReleaseRendererEvidence::Rt64 {
                    execution_policy: ReleaseGraphicsExecutionPolicy::LleAccuracy,
                    tv_type: crate::ReleaseTvStandard::Ntsc,
                    graphics_api: rt64_graphics_api,
                    backend_identity: rt64_identity.to_owned(),
                    source_authoritative: true,
                    settings_sha256: "11".repeat(32),
                    replacement_packs_active: false,
                }
            } else {
                ReleaseRendererEvidence::Reference {
                    execution_policy: ReleaseGraphicsExecutionPolicy::LleAccuracy,
                    tv_type: crate::ReleaseTvStandard::Ntsc,
                }
            },
        };
        let execution_destinations = match program {
            Some(ProgramFeature::NativeArchive) => ExecutionDestinationEvidence::from_ordered(
                crate::ExecutionDestinationSource::NativeArchive {
                    artifact_sha256: "aa".repeat(32),
                },
                vec![crate::ExecutionDestinationEventEvidence {
                    guest_cycle: Some(1),
                    destination: crate::ReleaseExecutionDestination::Native {
                        section_index: 0,
                        function_offset: 0x1000,
                        link_vram: 0x8000_1000,
                    },
                }],
            )
            .unwrap(),
            Some(ProgramFeature::TypedObservedFunction) => {
                ExecutionDestinationEvidence::from_ordered(
                    crate::ExecutionDestinationSource::TypedObservedFunctionProgram {
                        artifact_sha256: "cc".repeat(32),
                    },
                    vec![crate::ExecutionDestinationEventEvidence {
                        guest_cycle: Some(1),
                        destination: crate::ReleaseExecutionDestination::TypedFunction {
                            vram: 0x8000_1000,
                            symbol: "entry".to_owned(),
                        },
                    }],
                )
                .unwrap()
            }
            Some(ProgramFeature::TypedBlock) => ExecutionDestinationEvidence::from_ordered(
                crate::ExecutionDestinationSource::TypedBlockProgram {
                    program_sha256: "dd".repeat(32),
                    dispatch_artifact_sha256: "ee".repeat(32),
                },
                vec![crate::ExecutionDestinationEventEvidence {
                    guest_cycle: None,
                    destination: crate::ReleaseExecutionDestination::TypedBlock {
                        bank: 1,
                        pc: 0x8000_1000,
                        runner_artifact_sha256: "ff".repeat(32),
                    },
                }],
            )
            .unwrap(),
            None => ExecutionDestinationEvidence::no_program(),
        };
        ReleaseGateReport::new_with_test_environment_and_destinations(
            scenario,
            input,
            digest.finish().unwrap(),
            observations,
            environment,
            execution_destinations,
            closure,
        )
        .unwrap()
    }

    fn with_rsp_rdp_observations(
        mut report: ReleaseGateReport,
        ordered: Vec<RspRdpObservationEventEvidence>,
    ) -> ReleaseGateReport {
        report.rsp_rdp = RspRdpEvidence::from_ordered(ordered).unwrap();
        report.report_sha256 = hex(&Sha256::digest(
            crate::release_gate::encode_report_evidence(&report).unwrap(),
        ));
        report.verify_integrity().unwrap();
        report
    }

    fn scenario(id: &str, report: &ReleaseGateReport) -> ReleaseMatrixScenario {
        let mut scenario = ReleaseMatrixScenario {
            id: id.to_owned(),
            report_scenario: report.scenario.clone(),
            input_sha256: report.input_sha256.clone(),
            report_sha256: report.report_sha256.clone(),
            declaration_sha256: String::new(),
        };
        scenario.declaration_sha256 = scenario.recompute_declaration_sha256();
        scenario
    }

    fn evidence_series(
        report: ReleaseGateReport,
    ) -> Vec<(ReleaseGateReport, ParsedUnsupportedJournal)> {
        (0..RELEASE_MATRIX_REPORT_COUNT)
            .map(|index| {
                let journal = ParsedUnsupportedJournal {
                    events: Vec::new(),
                    completion: crate::UnsupportedJournalCompletion::V3RunBound {
                        guest_cycle: report.digest.guest_cycle,
                        report_sha256: report.report_sha256.clone(),
                        run_event_sha256: hex(&Sha256::digest(format!(
                            "{}:{index}",
                            report.report_sha256
                        ))),
                    },
                };
                (report.clone(), journal)
            })
            .collect()
    }

    fn fixture() -> (
        ReleaseMatrixManifest,
        Vec<(ReleaseGateReport, ParsedUnsupportedJournal)>,
    ) {
        let reference = closed_report(
            "game-a-reference",
            b"private-a",
            0xa1,
            "save.eeprom-4k-operation",
            CLEAN_RT64_IDENTITY,
            Some(ProgramFeature::TypedObservedFunction),
        );
        let rt64 = closed_report(
            "game-b-rt64",
            b"private-b",
            0xb2,
            "save.sram-operation",
            CLEAN_RT64_IDENTITY,
            Some(ProgramFeature::TypedObservedFunction),
        );
        let manifest = ReleaseMatrixManifest {
            schema: RELEASE_MATRIX_SCHEMA.to_owned(),
            profile: CertificationProfileIdentity::full_parity_v1(),
            scenarios: vec![
                scenario("reference-evidence", &reference),
                scenario("rt64-evidence", &rt64),
            ],
        };
        let reference = evidence_series(reference);
        let rt64 = evidence_series(rt64);
        let mut evidence = Vec::with_capacity(RELEASE_MATRIX_REPORT_COUNT * 2);
        for index in 0..RELEASE_MATRIX_REPORT_COUNT {
            // Deliberately interleave the flat input in the opposite order
            // from the manifest. Routing authority is report.scenario.
            evidence.push(rt64[index].clone());
            evidence.push(reference[index].clone());
        }
        (manifest, evidence)
    }

    fn incomplete_fixture() -> (
        ReleaseMatrixManifest,
        Vec<(ReleaseGateReport, ParsedUnsupportedJournal)>,
        IncompleteReleaseMatrix,
    ) {
        let (manifest, evidence) = fixture();
        let incomplete = match verify_release_matrix(&manifest, &evidence).unwrap() {
            ReleaseMatrixVerification::Incomplete(incomplete) => incomplete,
            ReleaseMatrixVerification::Complete(_) => {
                panic!("two scenarios cannot cover the fixed full-parity denominator")
            }
        };
        incomplete.verify_integrity().unwrap();
        (manifest, evidence, incomplete)
    }

    fn rt64_report_for_platform_api(
        scenario_name: &str,
        input: &[u8],
        platform: ReleaseHostPlatform,
        graphics_api: ReleaseGraphicsApi,
    ) -> ReleaseGateReport {
        let identity = clean_rt64_identity_for(graphics_api);
        closed_report_with_rt64_environment(
            scenario_name,
            input,
            0xc3,
            "save.sram-operation",
            &identity,
            Some(ProgramFeature::TypedObservedFunction),
            platform,
            graphics_api,
        )
    }

    fn incomplete_for_report(id: &str, report: ReleaseGateReport) -> IncompleteReleaseMatrix {
        let manifest = ReleaseMatrixManifest {
            schema: RELEASE_MATRIX_SCHEMA.to_owned(),
            profile: CertificationProfileIdentity::full_parity_v1(),
            scenarios: vec![scenario(id, &report)],
        };
        match verify_release_matrix(&manifest, &evidence_series(report)).unwrap() {
            ReleaseMatrixVerification::Incomplete(incomplete) => incomplete,
            ReleaseMatrixVerification::Complete(_) => {
                panic!("one report series cannot cover the fixed full-parity denominator")
            }
        }
    }

    fn with_rom(
        mut report: ReleaseGateReport,
        destination_code: u8,
        class: crate::ReleaseRomClass,
    ) -> ReleaseGateReport {
        let mut bytes = vec![0; 0x1000];
        bytes[..4].copy_from_slice(&0x8037_1240u32.to_be_bytes());
        bytes[0x3b..0x3f].copy_from_slice(&[b'N', b'F', b'6', destination_code]);
        let tv_type = crate::ReleaseRomEvidence::decode_tv_type(&bytes)
            .unwrap()
            .unwrap_or(fn64_runtime::TvType::Ntsc);
        let tv_standard = crate::ReleaseTvStandard::from(tv_type);
        match &mut report.environment.renderer {
            ReleaseRendererEvidence::Reference { tv_type, .. }
            | ReleaseRendererEvidence::Rt64 { tv_type, .. } => *tv_type = tv_standard,
        }
        report.input_sha256 = hex(&Sha256::digest(&bytes));
        report.rom = Some(
            crate::ReleaseRomEvidence::from_bytes(&bytes, class, tv_type)
                .expect("test ROM header is valid"),
        );
        report.report_sha256 = hex(&Sha256::digest(
            crate::release_gate::encode_report_evidence(&report)
                .expect("test report evidence encodes"),
        ));
        report.verify_integrity().unwrap();
        report
    }

    fn assigned_requirement_ids(
        incomplete: &IncompleteReleaseMatrix,
        class: CertificationRequirementClass,
    ) -> BTreeSet<String> {
        incomplete
            .satisfied
            .iter()
            .filter(|assignment| assignment.requirement.class() == class)
            .map(|assignment| assignment.requirement.id().to_owned())
            .collect()
    }

    fn rom_class_authority(report: &ReleaseGateReport) -> VerifiedRomClassAuthority {
        let rom = report
            .rom
            .as_ref()
            .expect("authority fixture has ROM evidence");
        let mut authority = VerifiedRomClassAuthority {
            schema: VERIFIED_ROM_CLASS_AUTHORITY_SCHEMA.to_owned(),
            contract_schema: crate::PRIVATE_RELEASE_RUN_CONTRACT_SCHEMA.to_owned(),
            contract_sha256: "91".repeat(32),
            receipt_schema: crate::PRIVATE_RELEASE_SERIES_RECEIPT_SCHEMA.to_owned(),
            receipt_sha256: "93".repeat(32),
            runner_executable_sha256: "94".repeat(32),
            purpose: "full_rom".to_owned(),
            report_scenario: report.scenario.clone(),
            input_sha256: report.input_sha256.clone(),
            input_bytes: rom.byte_len,
            rom_class: rom.class,
            guest_cycle: report.digest.guest_cycle,
            expected_execution_source: report.execution_destinations.source.clone(),
            child_executable_sha256: "92".repeat(32),
            semantic_report_sha256: report.report_sha256.clone(),
            run_event_sha256s: evidence_series(report.clone())
                .into_iter()
                .map(|(_, journal)| {
                    journal
                        .release_run_event_sha256()
                        .expect("test journal has a run event")
                        .to_owned()
                })
                .collect(),
            authority_sha256: String::new(),
        };
        authority.authority_sha256 = authority.recompute_authority_sha256();
        authority
    }

    #[test]
    fn exported_private_series_matrix_path_admits_public_fixture_and_rejects_tamper() {
        const REPORT_SCENARIO: &str = "public-homebrew-production-matrix-mechanism-v1";
        const CHILD_FIXTURE: &str = "private_release_series::tests::fresh_child_fixture";
        const CHILD_ENABLE_ENV: &str = "FN64_TEST_RELEASE_CHILD";
        const CHILD_TEMPLATE_ENV: &str = "FN64_TEST_RELEASE_TEMPLATE";

        let directory = ProductionMatrixFixtureDirectory::new();
        let manifest_path = directory.0.join("admission-manifest.json");
        let readiness_path = directory.0.join("readiness.json");
        let contract_path = directory.0.join("contract.json");
        let receipt_path = directory.0.join("program-build-receipt.json");
        let report_template_path = directory.0.join("report-template.json");
        let rom_path = directory.0.join("public-homebrew-fixture.z64");
        let text_path = directory.0.join("microcode-text.bin");
        let data_path = directory.0.join("microcode-data.bin");
        let recompiled_path = directory.0.join("typed-block-pack.bin");
        let series_path = directory.0.join("series");

        // This generated file is a public, non-game homebrew-shaped fixture.
        // It tests the production authority path; it is not representative-ROM
        // or runtime/microcode behavioral evidence.
        let mut rom_bytes = vec![0u8; 0x1000];
        rom_bytes[..4].copy_from_slice(&0x8037_1240u32.to_be_bytes());
        rom_bytes[0x20..0x34].copy_from_slice(b"FN64 MATRIX FIXTURE ");
        rom_bytes[0x3b..0x3f].copy_from_slice(b"NF6E");
        fs::write(&rom_path, &rom_bytes).unwrap();
        fs::write(&text_path, vec![0x5a; fn64_runtime::RSP_MEMORY_BANK_SIZE]).unwrap();
        fs::write(&data_path, b"fn64 public matrix fixture task data").unwrap();
        fs::write(
            &recompiled_path,
            b"fn64 public matrix fixture typed-block pack",
        )
        .unwrap();

        let executable = std::env::current_exe().unwrap().canonicalize().unwrap();
        let program_sha256 = hex(&Sha256::digest(
            b"fn64 public matrix fixture typed-block program v1",
        ));
        let materialized = materialize_release_program_build_receipt(
            &receipt_path,
            &executable,
            ReleaseProgramBuildReceiptInput::TypedBlock {
                pack: recompiled_path.clone(),
                expected_program_sha256: program_sha256,
            },
        )
        .unwrap();
        let source = materialized.execution_source;
        let recompiled_sha256 = hex(&Sha256::digest(fs::read(&recompiled_path).unwrap()));
        let text_sha256 = hex(&Sha256::digest(fs::read(&text_path).unwrap()));
        let data_bytes = fs::metadata(&data_path).unwrap().len() as u32;
        let data_sha256 = hex(&Sha256::digest(fs::read(&data_path).unwrap()));

        let mut report = closed_report(
            REPORT_SCENARIO,
            &rom_bytes,
            0xd6,
            "save.sram-operation",
            CLEAN_RT64_IDENTITY,
            Some(ProgramFeature::TypedBlock),
        );
        #[cfg(target_os = "macos")]
        let (platform, platform_wire) = (ReleaseHostPlatform::MacosArm64, "macos_arm64");
        #[cfg(target_os = "linux")]
        let (platform, platform_wire) = (ReleaseHostPlatform::LinuxX86_64, "linux_x86_64");
        #[cfg(target_os = "windows")]
        let (platform, platform_wire) = (ReleaseHostPlatform::WindowsX86_64, "windows_x86_64");
        report.environment.platform = platform;
        report.environment.windows_version = crate::test_release_windows_version();
        report.execution_destinations = ExecutionDestinationEvidence::from_ordered(
            source.clone(),
            vec![crate::ExecutionDestinationEventEvidence {
                guest_cycle: None,
                destination: crate::ReleaseExecutionDestination::TypedBlock {
                    bank: 1,
                    pc: 0x8000_1000,
                    runner_artifact_sha256: recompiled_sha256.clone(),
                },
            }],
        )
        .unwrap();
        report.rom = Some(
            ReleaseRomEvidence::from_bytes(
                &rom_bytes,
                ReleaseRomClass::PublicHomebrew,
                fn64_runtime::TvType::Ntsc,
            )
            .unwrap(),
        );
        report.rsp_rdp = RspRdpEvidence::from_ordered(vec![RspRdpObservationEventEvidence {
            guest_cycle: 42,
            observation: RspRdpObservationKindEvidence::MicrocodeRecognition {
                task_address: 0x1000,
                imem_generation: 1,
                text_sha256,
                data_address: 0x2000,
                data_bytes,
                data_sha256,
                family: Some(ReleaseMicrocodeFamily::Other { id: 0x464e_3634 }),
            },
        }])
        .unwrap();
        report.report_sha256 = hex(&Sha256::digest(
            crate::release_gate::encode_report_evidence(&report).unwrap(),
        ));
        report.verify_integrity().unwrap();
        report.write_json(&report_template_path).unwrap();

        let descriptor = |path: &Path, provenance: &str| {
            let bytes = fs::read(path).unwrap();
            serde_json::json!({
                "path": path.to_str().unwrap(),
                "length": bytes.len(),
                "sha256": hex(&Sha256::digest(&bytes)),
                "provenance": provenance,
                "git_identity": "excluded",
            })
        };
        let file_descriptor = |path: &Path| {
            let bytes = fs::read(path).unwrap();
            serde_json::json!({
                "path": path.to_str().unwrap(),
                "length": bytes.len(),
                "sha256": hex(&Sha256::digest(&bytes)),
                "git_identity": "excluded",
            })
        };
        let execution_source = serde_json::to_value(&source).unwrap();
        let manifest = serde_json::json!({
            "schema": "fn64.private-input-admission.v7",
            "purpose": "full_rom",
            "intent": {
                "wire_family": "full_rom_mixed",
                "report_scenario": REPORT_SCENARIO,
                "recognition": "runtime_must_confirm_backend_known_pair",
                "extended_gbi_cases": [],
                "characterization_suite": null,
                "program_evidence_lane": "typed_block_program",
                "rom_class": "public_homebrew",
            },
            "release_matrix": {
                "platform": platform_wire,
                "controllers": ["standard_controller"],
                "save": "sram_32_kib",
                "renderers": ["reference_lle_accuracy"],
                "repeat_bar": 10,
            },
            "artifacts": {
                "microcode_text": descriptor(&text_path, "user_owned_rom_derived"),
                "microcode_data": descriptor(&data_path, "user_owned_rom_derived"),
                "microcode_text_raw_window": null,
                "microcode_data_raw_window": null,
                "rom": descriptor(&rom_path, "publicly_distributed_homebrew_rom"),
                "recompiled": descriptor(&recompiled_path, "user_generated_from_owned_rom"),
            },
            "runner": {
                "executable": file_descriptor(&executable),
                "working_directory": directory.0.to_str().unwrap(),
                "argv": ["--exact", CHILD_FIXTURE, "--nocapture"],
                "env": {
                    CHILD_ENABLE_ENV: "1",
                    CHILD_TEMPLATE_ENV: report_template_path.to_str().unwrap(),
                },
                "release_gate_cycle": report.digest.guest_cycle,
                "execution_source": execution_source,
                "program_build_receipt": file_descriptor(&receipt_path),
            },
        });
        fs::write(
            &manifest_path,
            serde_json::to_vec_pretty(&manifest).unwrap(),
        )
        .unwrap();
        let admitted = crate::private_input_admission::admit_current_v7_manifest(
            &manifest_path,
            &readiness_path,
        )
        .unwrap();
        assert!(admitted.contract.is_some());
        let write_new = |path: &Path, payload: &[u8]| {
            let mut file = fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(path)
                .unwrap();
            file.write_all(payload).unwrap();
            file.flush().unwrap();
            file.sync_all().unwrap();
        };
        write_new(&readiness_path, &admitted.readiness_bytes);
        write_new(
            &contract_path,
            admitted
                .contract_bytes
                .as_deref()
                .expect("full-ROM admission emits a contract"),
        );

        let contract = load_private_release_run_contract(&contract_path).unwrap();
        let receipt = run_private_release_series(&contract, &series_path).unwrap();
        let verified_series =
            verify_private_release_series(&contract, &series_path, &receipt).unwrap();
        let evidence = (1..=RELEASE_MATRIX_REPORT_COUNT)
            .map(|ordinal| {
                let report_path = series_path.join(format!("report-{ordinal:02}.json"));
                let retained_report =
                    serde_json::from_slice(&fs::read(&report_path).unwrap()).unwrap();
                let journal_path = report_path.with_extension("unsupported.jsonl");
                let journal = parse_unsupported_journal(&fs::read(journal_path).unwrap()).unwrap();
                (retained_report, journal)
            })
            .collect::<Vec<_>>();
        let matrix_manifest = ReleaseMatrixManifest {
            schema: RELEASE_MATRIX_SCHEMA.to_owned(),
            profile: CertificationProfileIdentity::full_parity_v1(),
            scenarios: vec![scenario("public-homebrew-production-fixture", &report)],
        };

        let ReleaseMatrixVerification::Incomplete(incomplete) =
            verify_release_matrix_with_private_series(
                &matrix_manifest,
                &evidence,
                &[&verified_series],
            )
            .unwrap()
        else {
            panic!("one public fixture must remain incomplete against the full profile")
        };
        let homebrew_assignment = incomplete
            .satisfied
            .iter()
            .find(|assignment| {
                assignment.requirement.class() == CertificationRequirementClass::RomClass
                    && assignment.requirement.id() == "public_homebrew"
            })
            .expect("production opaque series earns its exact public-homebrew fixture row");
        assert_eq!(homebrew_assignment.evidence_sha256s.len(), 1);

        let mut reordered = evidence.clone();
        let mut journals = reordered
            .iter()
            .map(|(_, journal)| journal.clone())
            .collect::<Vec<_>>();
        journals.rotate_left(1);
        for ((_, journal), replacement) in reordered.iter_mut().zip(journals) {
            *journal = replacement;
        }
        assert!(matches!(
            verify_release_matrix_with_private_series(
                &matrix_manifest,
                &reordered,
                &[&verified_series],
            ),
            Err(ReleaseMatrixError::RomClassAuthorityMismatch {
                field: "run_event_sha256s",
                ..
            })
        ));

        fs::OpenOptions::new()
            .append(true)
            .open(series_path.join("report-01.json"))
            .unwrap()
            .write_all(b"\n")
            .unwrap();
        assert!(matches!(
            verify_release_matrix_with_private_series(
                &matrix_manifest,
                &evidence,
                &[&verified_series],
            ),
            Err(ReleaseMatrixError::InvalidPrivateSeriesAuthority { .. })
        ));
    }

    fn replace_report(
        manifest: &mut ReleaseMatrixManifest,
        evidence: &mut Vec<(ReleaseGateReport, ParsedUnsupportedJournal)>,
        report: ReleaseGateReport,
    ) {
        let declaration = manifest
            .scenarios
            .iter_mut()
            .find(|scenario| scenario.report_scenario == report.scenario)
            .expect("replacement report scenario is declared");
        declaration.input_sha256 = report.input_sha256.clone();
        declaration.report_sha256 = report.report_sha256.clone();
        declaration.declaration_sha256 = declaration.recompute_declaration_sha256();
        evidence.retain(|(existing, _)| existing.scenario != report.scenario);
        evidence.extend(evidence_series(report));
    }

    fn forged_ref(class: CertificationRequirementClass, id: &str) -> CertificationRequirementRef {
        serde_json::from_value(serde_json::json!({
            "class": class,
            "id": id,
        }))
        .unwrap()
    }

    fn requirement_keys(
        requirements: impl IntoIterator<Item = CertificationRequirementRef>,
    ) -> Vec<(CertificationRequirementClass, String)> {
        requirements
            .into_iter()
            .map(|requirement| (requirement.class(), requirement.id().to_owned()))
            .collect()
    }

    fn profile_keys() -> Vec<(CertificationRequirementClass, String)> {
        FullParityV1::new()
            .requirements()
            .into_iter()
            .map(|requirement| (requirement.class(), requirement.id().to_owned()))
            .collect()
    }

    #[test]
    fn valid_v5_evidence_returns_canonical_incomplete_profile() {
        let (manifest, evidence, incomplete) = incomplete_fixture();
        assert_eq!(FullParityV1::REQUIREMENT_COUNT, 162);
        assert_eq!(incomplete.verified_scenarios, 2);
        assert_eq!(incomplete.verified_reports, 20);
        assert_eq!(incomplete.satisfied.len(), 7);
        assert_eq!(incomplete.missing.len(), 155);
        assert_eq!(
            incomplete.manifest_sha256,
            manifest.recompute_manifest_sha256()
        );

        let satisfied_keys: BTreeSet<_> = incomplete
            .satisfied
            .iter()
            .map(|assignment| {
                (
                    assignment.requirement.class(),
                    assignment.requirement.id().to_owned(),
                )
            })
            .collect();
        let expected_satisfied: Vec<_> = profile_keys()
            .into_iter()
            .filter(|key| satisfied_keys.contains(key))
            .collect();
        let actual_satisfied: Vec<_> = incomplete
            .satisfied
            .iter()
            .map(|assignment| {
                (
                    assignment.requirement.class(),
                    assignment.requirement.id().to_owned(),
                )
            })
            .collect();
        assert_eq!(actual_satisfied, expected_satisfied);

        let missing_keys = requirement_keys(incomplete.missing.clone());
        let expected_missing: Vec<_> = profile_keys()
            .into_iter()
            .filter(|key| !satisfied_keys.contains(key))
            .collect();
        assert_eq!(missing_keys, expected_missing);

        let standard = incomplete
            .satisfied
            .iter()
            .find(|assignment| {
                assignment.requirement.class() == CertificationRequirementClass::Controller
                    && assignment.requirement.id() == "standard_controller"
            })
            .unwrap();
        assert_eq!(standard.evidence_sha256s.len(), 2);
        assert!(satisfied_keys.contains(&(
            CertificationRequirementClass::PlatformApiTarget,
            "linux-vulkan".to_owned(),
        )));

        // Re-running the interleaved flat stream is deterministic.
        let rerun = match verify_release_matrix(&manifest, &evidence).unwrap() {
            ReleaseMatrixVerification::Incomplete(value) => value,
            ReleaseMatrixVerification::Complete(_) => unreachable!(),
        };
        assert_eq!(rerun.assessment_sha256, incomplete.assessment_sha256);
    }

    #[test]
    fn schema_v20_tv_region_credit_requires_fixed_header_evidence_not_labels_or_region_free() {
        let fixed = with_rom(
            closed_report(
                "fixed-ntsc",
                b"placeholder",
                0xd1,
                "save.sram-operation",
                CLEAN_RT64_IDENTITY,
                Some(ProgramFeature::TypedObservedFunction),
            ),
            b'E',
            crate::ReleaseRomClass::RetailCartridge,
        );
        assert_eq!(
            derive_scenario_coverage("fixed-ntsc", &fixed)
                .unwrap()
                .tv_regions,
            vec![ReleaseTvRegion::Ntsc]
        );
        let fixed_incomplete = incomplete_for_report("fixed-ntsc", fixed);
        assert_eq!(
            assigned_requirement_ids(&fixed_incomplete, CertificationRequirementClass::TvRegion,),
            BTreeSet::from(["ntsc".to_owned()])
        );
        assert!(assigned_requirement_ids(
            &fixed_incomplete,
            CertificationRequirementClass::RomClass,
        )
        .is_empty());

        let region_free = with_rom(
            closed_report(
                "retail-pal-label",
                b"placeholder",
                0xd2,
                "save.sram-operation",
                CLEAN_RT64_IDENTITY,
                Some(ProgramFeature::TypedObservedFunction),
            ),
            0,
            crate::ReleaseRomClass::PublicHomebrew,
        );
        assert!(derive_scenario_coverage("retail-pal-label", &region_free)
            .unwrap()
            .tv_regions
            .is_empty());
        let region_free_incomplete = incomplete_for_report("retail-pal-label", region_free);
        assert!(assigned_requirement_ids(
            &region_free_incomplete,
            CertificationRequirementClass::TvRegion,
        )
        .is_empty());
        assert!(assigned_requirement_ids(
            &region_free_incomplete,
            CertificationRequirementClass::RomClass,
        )
        .is_empty());

        let label_only = incomplete_for_report(
            "retail-pal-label",
            closed_report(
                "retail-pal-label",
                b"placeholder",
                0xd3,
                "save.sram-operation",
                CLEAN_RT64_IDENTITY,
                Some(ProgramFeature::TypedObservedFunction),
            ),
        );
        assert!(
            assigned_requirement_ids(&label_only, CertificationRequirementClass::TvRegion,)
                .is_empty()
        );
        assert!(
            assigned_requirement_ids(&label_only, CertificationRequirementClass::RomClass,)
                .is_empty()
        );
    }

    #[test]
    fn rom_class_credit_requires_exact_contract_authority_and_binds_its_digest() {
        let report = with_rom(
            closed_report(
                "authority-retail",
                b"placeholder",
                0xd4,
                "save.sram-operation",
                CLEAN_RT64_IDENTITY,
                Some(ProgramFeature::TypedObservedFunction),
            ),
            b'E',
            ReleaseRomClass::RetailCartridge,
        );
        let manifest = ReleaseMatrixManifest {
            schema: RELEASE_MATRIX_SCHEMA.to_owned(),
            profile: CertificationProfileIdentity::full_parity_v1(),
            scenarios: vec![scenario("authority-retail", &report)],
        };
        let authority = rom_class_authority(&report);
        let authority_sha256 = authority.authority_sha256.clone();
        let authorities = BTreeMap::from([(report.scenario.clone(), authority)]);
        let ReleaseMatrixVerification::Incomplete(incomplete) =
            verify_release_matrix_with_authorities(
                &manifest,
                &evidence_series(report.clone()),
                &authorities,
                &BTreeMap::new(),
            )
            .unwrap()
        else {
            panic!("one authority-backed series remains intentionally incomplete")
        };
        let assignment = incomplete
            .satisfied
            .iter()
            .find(|assignment| {
                assignment.requirement.class() == CertificationRequirementClass::RomClass
                    && assignment.requirement.id() == "retail_cartridge"
            })
            .expect("retail class receives authority-backed credit");
        assert_eq!(assignment.evidence_sha256s, [authority_sha256]);

        let mut relabelled = rom_class_authority(&report);
        relabelled.rom_class = ReleaseRomClass::PublicHomebrew;
        relabelled.authority_sha256 = relabelled.recompute_authority_sha256();
        let relabelled = BTreeMap::from([(report.scenario.clone(), relabelled)]);
        assert!(matches!(
            verify_release_matrix_with_authorities(
                &manifest,
                &evidence_series(report.clone()),
                &relabelled,
                &BTreeMap::new(),
            ),
            Err(ReleaseMatrixError::RomClassAuthorityMismatch {
                field: "rom.class",
                ..
            })
        ));

        let mut tampered = rom_class_authority(&report);
        tampered.input_bytes += 4;
        let tampered = BTreeMap::from([(report.scenario.clone(), tampered)]);
        assert!(matches!(
            verify_release_matrix_with_authorities(
                &manifest,
                &evidence_series(report.clone()),
                &tampered,
                &BTreeMap::new(),
            ),
            Err(ReleaseMatrixError::InvalidRomClassAuthority { .. })
        ));

        let mut wrong_semantic_report = rom_class_authority(&report);
        wrong_semantic_report.semantic_report_sha256 = "95".repeat(32);
        wrong_semantic_report.authority_sha256 = wrong_semantic_report.recompute_authority_sha256();
        let wrong_semantic_report =
            BTreeMap::from([(report.scenario.clone(), wrong_semantic_report)]);
        assert!(matches!(
            verify_release_matrix_with_authorities(
                &manifest,
                &evidence_series(report.clone()),
                &wrong_semantic_report,
                &BTreeMap::new(),
            ),
            Err(ReleaseMatrixError::RomClassAuthorityMismatch {
                field: "semantic_report_sha256",
                ..
            })
        ));

        let mut reordered_runs = rom_class_authority(&report);
        reordered_runs.run_event_sha256s.swap(0, 1);
        reordered_runs.authority_sha256 = reordered_runs.recompute_authority_sha256();
        let reordered_runs = BTreeMap::from([(report.scenario.clone(), reordered_runs)]);
        assert!(matches!(
            verify_release_matrix_with_authorities(
                &manifest,
                &evidence_series(report),
                &reordered_runs,
                &BTreeMap::new(),
            ),
            Err(ReleaseMatrixError::RomClassAuthorityMismatch {
                field: "run_event_sha256s",
                ..
            })
        ));
    }

    #[test]
    fn private_series_authorities_reject_unused_and_duplicate_records() {
        let report = with_rom(
            closed_report(
                "authority-homebrew",
                b"placeholder",
                0xd5,
                "save.sram-operation",
                CLEAN_RT64_IDENTITY,
                Some(ProgramFeature::TypedObservedFunction),
            ),
            b'E',
            ReleaseRomClass::PublicHomebrew,
        );
        let manifest = ReleaseMatrixManifest {
            schema: RELEASE_MATRIX_SCHEMA.to_owned(),
            profile: CertificationProfileIdentity::full_parity_v1(),
            scenarios: vec![scenario("authority-homebrew", &report)],
        };
        let authority = rom_class_authority(&report);
        let mut duplicate = BTreeMap::new();
        insert_private_series_authority(&mut duplicate, report.scenario.clone(), authority.clone())
            .unwrap();
        assert!(matches!(
            insert_private_series_authority(
                &mut duplicate,
                report.scenario.clone(),
                authority.clone(),
            ),
            Err(ReleaseMatrixError::DuplicatePrivateSeriesAuthority { .. })
        ));

        let unused = BTreeMap::from([("unrelated-scenario".to_owned(), authority)]);
        assert!(matches!(
            validate_private_series_authority_usage(&manifest, &unused),
            Err(ReleaseMatrixError::UnusedPrivateSeriesAuthority { .. })
        ));
    }

    #[test]
    fn flat_evidence_auto_routes_by_report_scenario_not_manifest_id_or_input_order() {
        let (manifest, mut evidence) = fixture();
        assert_ne!(
            manifest.scenarios[0].id,
            manifest.scenarios[0].report_scenario
        );
        evidence.rotate_left(7);
        let ReleaseMatrixVerification::Incomplete(incomplete) =
            verify_release_matrix(&manifest, &evidence).unwrap()
        else {
            panic!("fixture remains intentionally incomplete")
        };
        assert_eq!(incomplete.verified_scenarios, 2);
        assert_eq!(incomplete.verified_reports, 20);
    }

    #[test]
    fn profile_identity_and_v4_relabels_are_rejected() {
        for (schema, digest, expected_schema_error) in [
            (
                "fn64.certification-profile.full-parity.v0",
                crate::FULL_PARITY_V1_DEFINITION_SHA256,
                true,
            ),
            (crate::FULL_PARITY_V1_SCHEMA, "00", false),
        ] {
            let (mut manifest, evidence) = fixture();
            manifest.profile.schema = schema.to_owned();
            manifest.profile.definition_sha256 = digest.to_owned();
            let error = verify_release_matrix(&manifest, &evidence).unwrap_err();
            match (error, expected_schema_error) {
                (
                    ReleaseMatrixError::InvalidCertificationProfile(
                        crate::CertificationProfileError::UnsupportedSchema(_),
                    ),
                    true,
                )
                | (
                    ReleaseMatrixError::InvalidCertificationProfile(
                        crate::CertificationProfileError::DefinitionDigestMismatch { .. },
                    ),
                    false,
                ) => {}
                (other, _) => panic!("unexpected profile error: {other:?}"),
            }
        }

        let (mut manifest, evidence) = fixture();
        manifest.schema = "fn64.release-matrix.v4".to_owned();
        assert!(matches!(
            verify_release_matrix(&manifest, &evidence),
            Err(ReleaseMatrixError::UnsupportedSchema(schema))
                if schema == "fn64.release-matrix.v4"
        ));

        let legacy = serde_json::json!({
            "schema": RELEASE_MATRIX_SCHEMA,
            "required": {
                "platforms": ["macos_arm64"],
                "controllers": ["standard_controller"],
                "saves": ["eeprom_4_kbit"],
                "renderers": ["reference_lle_accuracy"],
                "programs": ["typed_observed_function"]
            },
            "scenarios": []
        });
        assert!(serde_json::from_value::<ReleaseMatrixManifest>(legacy).is_err());
    }

    #[test]
    fn scenario_and_manifest_digests_use_the_v5_evidence_only_wire() {
        let (manifest, evidence) = fixture();
        let declaration = &manifest.scenarios[0];
        let baseline = declaration.recompute_declaration_sha256();

        let mut legacy_wire = Vec::new();
        legacy_wire.extend_from_slice(b"fn64.release-matrix.scenario.v4\0");
        push_bytes(&mut legacy_wire, declaration.id.as_bytes());
        push_bytes(&mut legacy_wire, declaration.report_scenario.as_bytes());
        push_bytes(&mut legacy_wire, declaration.input_sha256.as_bytes());
        push_bytes(&mut legacy_wire, declaration.report_sha256.as_bytes());
        assert_ne!(baseline, hex(&Sha256::digest(legacy_wire)));

        for field in ["id", "scenario", "input", "report"] {
            let mut changed = declaration.clone();
            match field {
                "id" => changed.id.push('x'),
                "scenario" => changed.report_scenario.push('x'),
                "input" => changed.input_sha256 = "11".repeat(32),
                "report" => changed.report_sha256 = "22".repeat(32),
                _ => unreachable!(),
            }
            assert_ne!(changed.recompute_declaration_sha256(), baseline, "{field}");
        }

        let mut relabeled = manifest.clone();
        relabeled.scenarios[0].report_scenario.push_str("-changed");
        assert!(matches!(
            verify_release_matrix(&relabeled, &evidence),
            Err(ReleaseMatrixError::DeclarationDigestMismatch { .. })
        ));

        let baseline_manifest = manifest.recompute_manifest_sha256();
        let mut changed_profile = manifest;
        changed_profile.profile.definition_sha256 = "33".repeat(32);
        assert_ne!(
            changed_profile.recompute_manifest_sha256(),
            baseline_manifest
        );
    }

    #[test]
    fn exact_ten_run_series_is_enforced_table_driven() {
        for requested in [9usize, 11] {
            let (manifest, evidence) = fixture();
            let mut first: Vec<_> = evidence
                .iter()
                .filter(|(report, _)| report.scenario == "game-a-reference")
                .cloned()
                .collect();
            if requested == 9 {
                first.pop();
            } else {
                let extra = first[0].clone();
                let mut extra = extra;
                let crate::UnsupportedJournalCompletion::V3RunBound {
                    run_event_sha256, ..
                } = &mut extra.1.completion
                else {
                    unreachable!()
                };
                *run_event_sha256 = "44".repeat(32);
                first.push(extra);
            }
            let mut changed: Vec<_> = evidence
                .into_iter()
                .filter(|(report, _)| report.scenario != "game-a-reference")
                .collect();
            changed.extend(first);
            assert!(matches!(
                verify_release_matrix(&manifest, &changed),
                Err(ReleaseMatrixError::WrongReportCount {
                    expected: 10,
                    actual,
                    ..
                }) if actual == requested
            ));
        }
    }

    #[test]
    fn missing_and_unexpected_scenario_evidence_are_distinct() {
        let (manifest, evidence) = fixture();
        let missing: Vec<_> = evidence
            .iter()
            .filter(|(report, _)| report.scenario != "game-a-reference")
            .cloned()
            .collect();
        assert!(matches!(
            verify_release_matrix(&manifest, &missing),
            Err(ReleaseMatrixError::MissingEvidence { id })
                if id == "reference-evidence"
        ));

        let unexpected = closed_report(
            "undeclared-report",
            b"private-c",
            0xc3,
            "save.eeprom-4k-operation",
            CLEAN_RT64_IDENTITY,
            Some(ProgramFeature::TypedObservedFunction),
        );
        let mut extra = evidence;
        extra.extend(evidence_series(unexpected));
        assert!(matches!(
            verify_release_matrix(&manifest, &extra),
            Err(ReleaseMatrixError::UnexpectedReportScenario { scenario })
                if scenario == "undeclared-report"
        ));
    }

    #[test]
    fn replayed_run_event_identity_across_scenarios_is_rejected() {
        let (manifest, mut evidence) = fixture();
        let replay = evidence
            .iter()
            .find_map(|(report, journal)| {
                (report.scenario == "game-a-reference").then(|| match &journal.completion {
                    crate::UnsupportedJournalCompletion::V3RunBound {
                        run_event_sha256, ..
                    } => run_event_sha256.clone(),
                    _ => unreachable!(),
                })
            })
            .unwrap();
        let (_, journal) = evidence
            .iter_mut()
            .find(|(report, _)| report.scenario == "game-b-rt64")
            .unwrap();
        let crate::UnsupportedJournalCompletion::V3RunBound {
            run_event_sha256, ..
        } = &mut journal.completion
        else {
            unreachable!()
        };
        *run_event_sha256 = replay;
        assert!(matches!(
            verify_release_matrix(&manifest, &evidence),
            Err(ReleaseMatrixError::DuplicateRunEventIdentity { .. })
        ));
    }

    #[test]
    fn report_and_input_digests_are_bound_by_each_v5_declaration() {
        for field in ["input", "report"] {
            let (mut manifest, evidence) = fixture();
            if field == "input" {
                manifest.scenarios[0].input_sha256 = "55".repeat(32);
            } else {
                manifest.scenarios[0].report_sha256 = "66".repeat(32);
            }
            manifest.scenarios[0].declaration_sha256 =
                manifest.scenarios[0].recompute_declaration_sha256();
            let error = verify_release_matrix(&manifest, &evidence).unwrap_err();
            assert!(
                matches!(
                    (&error, field),
                    (ReleaseMatrixError::InputDigestMismatch { .. }, "input")
                        | (ReleaseMatrixError::ReportDigestMismatch { .. }, "report")
                ),
                "unexpected {field} result: {error:?}"
            );
        }
    }

    #[test]
    fn report_without_entered_program_evidence_is_rejected() {
        let (mut manifest, mut evidence) = fixture();
        let original = evidence
            .iter()
            .find(|(report, _)| report.scenario == "game-a-reference")
            .unwrap()
            .0
            .clone();
        let replacement = ReleaseGateReport::new_with_test_environment(
            original.scenario.clone(),
            b"private-a",
            original.digest,
            original.observations,
            original.environment,
            original.closure,
        )
        .unwrap();
        replace_report(&mut manifest, &mut evidence, replacement);
        assert!(matches!(
            verify_release_matrix(&manifest, &evidence),
            Err(ReleaseMatrixError::NoProgramEvidence { .. })
        ));
    }

    #[test]
    fn rt64_identity_and_observation_source_are_authoritative() {
        const DIRTY_RT64_IDENTITY: &str = concat!(
            "adapter=fn64-render-rt64/rt64;adapter_sha256=",
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            ";source=git:",
            "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
            ";provenance=git-dirty;overlay=fn64-test;post_vi_api=vulkan-bgra8-rgba8-unorm"
        );
        let (_, evidence) = fixture();
        let original = evidence
            .iter()
            .find(|(report, _)| report.scenario == "game-b-rt64")
            .unwrap()
            .0
            .clone();
        let mut environment = original.environment.clone();
        let ReleaseRendererEvidence::Rt64 {
            backend_identity, ..
        } = &mut environment.renderer
        else {
            unreachable!()
        };
        *backend_identity = DIRTY_RT64_IDENTITY.to_owned();
        let mut observations = original.observations.clone();
        let FramebufferObservationSource::PostViSwapchain {
            backend_identity, ..
        } = &mut observations.framebuffer.source
        else {
            unreachable!()
        };
        *backend_identity = DIRTY_RT64_IDENTITY.to_owned();
        assert!(matches!(
            ReleaseGateReport::new_with_test_environment_and_destinations(
                original.scenario,
                b"private-b",
                original.digest,
                observations,
                environment,
                original.execution_destinations,
                original.closure,
            ),
            Err(crate::GateError::RendererObservationMismatch(_))
        ));

        let (manifest, mut evidence) = fixture();
        let report = evidence
            .iter_mut()
            .find(|(report, _)| report.scenario == "game-b-rt64")
            .unwrap();
        report.0.environment.renderer = ReleaseRendererEvidence::Reference {
            execution_policy: ReleaseGraphicsExecutionPolicy::LleAccuracy,
            tv_type: crate::ReleaseTvStandard::Ntsc,
        };
        assert!(matches!(
            verify_release_matrix(&manifest, &evidence),
            Err(ReleaseMatrixError::InvalidUnassignedReport {
                source: crate::GateError::RendererObservationMismatch(_),
                ..
            })
        ));
    }

    #[test]
    fn save_and_controller_assignments_require_positive_operation_paths() {
        for (scenario_name, input, path) in [
            (
                "game-a-reference",
                b"private-a".as_slice(),
                "save.eeprom-4k-operation",
            ),
            (
                "game-b-rt64",
                b"private-b".as_slice(),
                "controller.rumble-operation",
            ),
        ] {
            let (mut manifest, mut evidence) = fixture();
            let original = evidence
                .iter()
                .find(|(report, _)| report.scenario == scenario_name)
                .unwrap()
                .0
                .clone();
            let closure: Vec<_> = original
                .closure
                .iter()
                .filter(|entry| entry.name != path)
                .cloned()
                .collect();
            let replacement = ReleaseGateReport::new_with_test_environment_and_destinations(
                original.scenario,
                input,
                original.digest,
                original.observations,
                original.environment,
                original.execution_destinations,
                closure,
            )
            .unwrap();
            replace_report(&mut manifest, &mut evidence, replacement);
            assert!(matches!(
                verify_release_matrix(&manifest, &evidence),
                Err(ReleaseMatrixError::MissingFeatureObservation {
                    path: missing,
                    ..
                }) if missing == path
            ));
        }
    }

    #[test]
    fn program_renderer_save_and_controller_coverage_is_derived_from_reports() {
        let (_, evidence, incomplete) = incomplete_fixture();
        let reference = evidence
            .iter()
            .find(|(report, _)| report.scenario == "game-a-reference")
            .unwrap();
        let rt64 = evidence
            .iter()
            .find(|(report, _)| report.scenario == "game-b-rt64")
            .unwrap();

        assert_eq!(
            derive_scenario_coverage("reference-evidence", &reference.0).unwrap(),
            ReleaseMatrixCoverage {
                rom_classes: Vec::new(),
                tv_regions: Vec::new(),
                platforms: vec![ReleasePlatform::MacosArm64],
                controllers: vec![ControllerFeature::StandardController],
                saves: vec![SaveFeature::Eeprom4Kbit],
                renderers: vec![RendererFeature::ReferenceLleAccuracy],
                programs: vec![ProgramFeature::TypedObservedFunction],
                microcodes: Vec::new(),
                rsp_rdp_mechanisms: Vec::new(),
            }
        );
        assert_eq!(
            derive_scenario_coverage("rt64-evidence", &rt64.0).unwrap(),
            ReleaseMatrixCoverage {
                rom_classes: Vec::new(),
                tv_regions: Vec::new(),
                platforms: vec![ReleasePlatform::LinuxX86_64],
                controllers: vec![
                    ControllerFeature::StandardController,
                    ControllerFeature::RumblePak,
                ],
                saves: vec![SaveFeature::Sram32Kib],
                renderers: vec![
                    RendererFeature::Rt64LleAccuracy,
                    RendererFeature::Rt64PostViCapture,
                ],
                programs: vec![ProgramFeature::TypedObservedFunction],
                microcodes: Vec::new(),
                rsp_rdp_mechanisms: Vec::new(),
            }
        );

        let assigned: BTreeSet<_> = incomplete
            .satisfied
            .iter()
            .map(|assignment| {
                (
                    assignment.requirement.class(),
                    assignment.requirement.id().to_owned(),
                )
            })
            .collect();
        for key in [
            (
                CertificationRequirementClass::ProgramRendererLane,
                "typed_observed_function/reference_lle_accuracy",
            ),
            (
                CertificationRequirementClass::ProgramRendererLane,
                "typed_observed_function/rt64_lle_accuracy",
            ),
            (CertificationRequirementClass::Save, "eeprom_4_kbit"),
            (CertificationRequirementClass::Save, "sram_32_kib"),
            (
                CertificationRequirementClass::Controller,
                "standard_controller",
            ),
            (CertificationRequirementClass::Controller, "rumble_pak"),
            (
                CertificationRequirementClass::PlatformApiTarget,
                "linux-vulkan",
            ),
        ] {
            assert!(assigned.contains(&(key.0, key.1.to_owned())), "{key:?}");
        }
    }

    #[test]
    fn exact_macos_metal_and_linux_vulkan_evidence_receive_platform_credit() {
        assert_eq!(
            clean_rt64_identity_for(ReleaseGraphicsApi::Vulkan),
            CLEAN_RT64_IDENTITY,
        );
        for (scenario_name, platform, graphics_api, expected) in [
            (
                "exact-macos-rt64",
                ReleaseHostPlatform::MacosArm64,
                ReleaseGraphicsApi::Metal,
                "macos-metal",
            ),
            (
                "exact-linux-rt64",
                ReleaseHostPlatform::LinuxX86_64,
                ReleaseGraphicsApi::Vulkan,
                "linux-vulkan",
            ),
        ] {
            let report = rt64_report_for_platform_api(
                scenario_name,
                scenario_name.as_bytes(),
                platform,
                graphics_api,
            );
            let incomplete = incomplete_for_report(scenario_name, report);
            assert_eq!(
                assigned_requirement_ids(
                    &incomplete,
                    CertificationRequirementClass::PlatformApiTarget,
                ),
                BTreeSet::from([expected.to_owned()]),
            );
        }
    }

    fn platform_case_fixture(
        report: &ReleaseGateReport,
        evidence: &[(ReleaseGateReport, ParsedUnsupportedJournal)],
        target: Rt64PlatformTarget,
        case: Rt64PlatformCase,
        seed: u8,
    ) -> VerifiedRt64PlatformCaseSeries {
        let verified = verify_release_evidence_series(evidence, RELEASE_MATRIX_REPORT_COUNT)
            .expect("matrix fixture series is valid");
        VerifiedRt64PlatformCaseSeries::fixture_for_test(
            target,
            case,
            (
                report.environment.platform,
                report.environment.windows_version,
            ),
            (
                &report.scenario,
                report.report_sha256.clone(),
                verified.run_event_sha256s,
            ),
            seed,
        )
        .unwrap()
    }

    fn pinned_platform_identity(api: ReleaseGraphicsApi, adapter_sha256: &str) -> String {
        format!(
            "adapter=fn64-render-rt64/rt64;adapter_sha256={adapter_sha256};source=git:f0728a2520d5aa735886240de3fee75cc805f6d6;provenance=git-clean;overlay=fn64-test;post_vi_api={}",
            match api {
                ReleaseGraphicsApi::D3d12 => "d3d12-bgra8-rgba8-unorm",
                ReleaseGraphicsApi::Vulkan => "vulkan-bgra8-rgba8-unorm",
                ReleaseGraphicsApi::Metal => "metal-bgra8-unorm",
            }
        )
    }

    #[test]
    fn opaque_platform_case_authority_binds_exact_matrix_series() {
        let seed = 0x51;
        let adapter_sha256 = hex(&Sha256::digest([seed, 0]));
        let identity = pinned_platform_identity(ReleaseGraphicsApi::Metal, &adapter_sha256);
        let report = closed_report_with_rt64_environment(
            "bound-macos-rt64-platform-case",
            b"bound-macos-rt64-platform-case",
            0xc7,
            "save.sram-operation",
            &identity,
            Some(ProgramFeature::TypedObservedFunction),
            ReleaseHostPlatform::MacosArm64,
            ReleaseGraphicsApi::Metal,
        );
        let evidence = evidence_series(report.clone());
        let manifest = ReleaseMatrixManifest {
            schema: RELEASE_MATRIX_SCHEMA.to_owned(),
            profile: CertificationProfileIdentity::full_parity_v1(),
            scenarios: vec![scenario("bound-macos-rt64-platform-case", &report)],
        };
        let series = platform_case_fixture(
            &report,
            &evidence,
            Rt64PlatformTarget::MacosMetal,
            Rt64PlatformCase::ResolutionDownsample,
            seed,
        );
        let ReleaseMatrixVerification::Incomplete(incomplete) =
            verify_release_matrix_with_platform_series(&manifest, &evidence, &[&series]).unwrap()
        else {
            panic!("one synthetic report remains incomplete")
        };
        let case_id = "macos-metal/resolution-downsample";
        let assignment = incomplete
            .satisfied
            .iter()
            .find(|assignment| {
                assignment.requirement.class() == CertificationRequirementClass::Rt64TargetCase
                    && assignment.requirement.id() == case_id
            })
            .expect("opaque authority earns only its exact case row");
        assert_eq!(assignment.evidence_sha256s.len(), 1);
        let platform_assignment = incomplete
            .satisfied
            .iter()
            .find(|assignment| {
                assignment.requirement.class() == CertificationRequirementClass::PlatformApiTarget
                    && assignment.requirement.id() == "macos-metal"
            })
            .expect("the validated v28 report earns its exact platform/API row");
        assert_eq!(
            platform_assignment.evidence_sha256s,
            [manifest.scenarios[0].declaration_sha256.clone()]
        );
        assert_eq!(incomplete.platform_case_authorities.len(), 1);

        let mut detached_retained = incomplete.clone();
        detached_retained.platform_case_authorities.clear();
        detached_retained.assessment_sha256 = incomplete_matrix_sha256(&detached_retained);
        assert!(matches!(
            detached_retained.verify_integrity(),
            Err(ReleaseMatrixError::PlatformAuthorityAssignmentMismatch { .. })
        ));

        assert!(matches!(
            verify_release_matrix_with_platform_series(&manifest, &evidence, &[&series, &series],),
            Err(ReleaseMatrixError::DuplicatePlatformSeriesAuthority { .. })
        ));
    }

    #[test]
    fn platform_case_authority_rejects_detached_report_and_run_events() {
        let seed = 0x52;
        let adapter_sha256 = hex(&Sha256::digest([seed, 0]));
        let identity = pinned_platform_identity(ReleaseGraphicsApi::Metal, &adapter_sha256);
        let report = closed_report_with_rt64_environment(
            "rt64-platform-binding-original",
            b"rt64-platform-binding-original",
            0xc8,
            "save.sram-operation",
            &identity,
            Some(ProgramFeature::TypedObservedFunction),
            ReleaseHostPlatform::MacosArm64,
            ReleaseGraphicsApi::Metal,
        );
        let evidence = evidence_series(report.clone());
        let manifest = ReleaseMatrixManifest {
            schema: RELEASE_MATRIX_SCHEMA.to_owned(),
            profile: CertificationProfileIdentity::full_parity_v1(),
            scenarios: vec![scenario("rt64-platform-binding-original", &report)],
        };
        let verified =
            verify_release_evidence_series(&evidence, RELEASE_MATRIX_REPORT_COUNT).unwrap();
        let detached = VerifiedRt64PlatformCaseSeries::fixture_for_test(
            Rt64PlatformTarget::MacosMetal,
            Rt64PlatformCase::FramebufferEnhancement,
            (ReleaseHostPlatform::MacosArm64, None),
            (
                "different-report-scenario",
                report.report_sha256.clone(),
                verified.run_event_sha256s.clone(),
            ),
            seed,
        )
        .unwrap();
        assert!(matches!(
            verify_release_matrix_with_platform_series(&manifest, &evidence, &[&detached]),
            Err(ReleaseMatrixError::UnusedPlatformSeriesAuthority { .. })
        ));

        let wrong_report_sha = VerifiedRt64PlatformCaseSeries::fixture_for_test(
            Rt64PlatformTarget::MacosMetal,
            Rt64PlatformCase::FramebufferEnhancement,
            (ReleaseHostPlatform::MacosArm64, None),
            (
                &report.scenario,
                hex(&Sha256::digest(b"different-semantic-report")),
                verified.run_event_sha256s.clone(),
            ),
            seed,
        )
        .unwrap();
        assert!(matches!(
            verify_release_matrix_with_platform_series(&manifest, &evidence, &[&wrong_report_sha],),
            Err(ReleaseMatrixError::UnusedPlatformSeriesAuthority { .. })
        ));

        let mut reordered_events = verified.run_event_sha256s;
        reordered_events.rotate_left(1);
        let reordered = VerifiedRt64PlatformCaseSeries::fixture_for_test(
            Rt64PlatformTarget::MacosMetal,
            Rt64PlatformCase::FramebufferEnhancement,
            (ReleaseHostPlatform::MacosArm64, None),
            (
                &report.scenario,
                report.report_sha256.clone(),
                reordered_events,
            ),
            seed,
        )
        .unwrap();
        assert!(matches!(
            verify_release_matrix_with_platform_series(&manifest, &evidence, &[&reordered]),
            Err(ReleaseMatrixError::UnusedPlatformSeriesAuthority { .. })
        ));
    }

    #[test]
    fn exact_windows_build_and_observed_api_receive_only_their_target_credit() {
        for (scenario_name, build, graphics_api, expected) in [
            (
                "exact-windows10-d3d12-rt64",
                21_999,
                ReleaseGraphicsApi::D3d12,
                "windows10-d3d12",
            ),
            (
                "exact-windows10-vulkan-rt64",
                21_999,
                ReleaseGraphicsApi::Vulkan,
                "windows10-vulkan",
            ),
            (
                "exact-windows11-d3d12-rt64",
                22_000,
                ReleaseGraphicsApi::D3d12,
                "windows11-d3d12",
            ),
            (
                "exact-windows11-vulkan-rt64",
                22_000,
                ReleaseGraphicsApi::Vulkan,
                "windows11-vulkan",
            ),
        ] {
            let mut report = rt64_report_for_platform_api(
                scenario_name,
                scenario_name.as_bytes(),
                ReleaseHostPlatform::WindowsX86_64,
                graphics_api,
            );
            report.environment.windows_version = Some(
                crate::ReleaseWindowsVersionEvidence::from_native_workstation(10, 0, build, 123)
                    .unwrap(),
            );
            report.report_sha256 = hex(&Sha256::digest(
                crate::release_gate::encode_report_evidence(&report).unwrap(),
            ));
            let incomplete = incomplete_for_report(scenario_name, report);
            assert_eq!(
                assigned_requirement_ids(
                    &incomplete,
                    CertificationRequirementClass::PlatformApiTarget,
                ),
                BTreeSet::from([expected.to_owned()]),
            );
        }
    }

    #[test]
    fn windows_family_relabel_cannot_manufacture_platform_credit() {
        let mut report = rt64_report_for_platform_api(
            "relabeled-windows11-d3d12-rt64",
            b"relabeled-windows11-d3d12-rt64",
            ReleaseHostPlatform::WindowsX86_64,
            ReleaseGraphicsApi::D3d12,
        );
        let mut version =
            crate::ReleaseWindowsVersionEvidence::from_native_workstation(10, 0, 21_999, 123)
                .unwrap();
        version.family = ReleaseWindowsFamily::Windows11;
        report.environment.windows_version = Some(version);
        report.report_sha256 = hex(&Sha256::digest(
            crate::release_gate::encode_report_evidence(&report).unwrap(),
        ));
        let manifest = ReleaseMatrixManifest {
            schema: RELEASE_MATRIX_SCHEMA.to_owned(),
            profile: CertificationProfileIdentity::full_parity_v1(),
            scenarios: vec![scenario("relabeled-windows", &report)],
        };
        assert!(matches!(
            verify_release_matrix(&manifest, &evidence_series(report)),
            Err(ReleaseMatrixError::InvalidUnassignedReport {
                source: crate::GateError::InvalidWindowsVersionEvidence(_),
                ..
            })
        ));
    }

    #[test]
    fn reference_platform_and_scenario_label_do_not_substitute_for_api_evidence() {
        let report = closed_report(
            "macos-metal",
            b"reference-platform-only",
            0xd4,
            "save.eeprom-4k-operation",
            CLEAN_RT64_IDENTITY,
            Some(ProgramFeature::TypedObservedFunction),
        );
        assert_eq!(report.environment.platform, ReleaseHostPlatform::MacosArm64);
        assert!(matches!(
            report.environment.renderer,
            ReleaseRendererEvidence::Reference { .. }
        ));
        let incomplete = incomplete_for_report("macos-metal", report);
        assert!(assigned_requirement_ids(
            &incomplete,
            CertificationRequirementClass::PlatformApiTarget,
        )
        .is_empty());
    }

    #[test]
    fn graphics_api_changes_bind_environment_report_and_matrix_digests() {
        let d3d12 = rt64_report_for_platform_api(
            "windows-api-rt64",
            b"same-private-input",
            ReleaseHostPlatform::WindowsX86_64,
            ReleaseGraphicsApi::D3d12,
        );
        let vulkan = rt64_report_for_platform_api(
            "windows-api-rt64",
            b"same-private-input",
            ReleaseHostPlatform::WindowsX86_64,
            ReleaseGraphicsApi::Vulkan,
        );

        let environment_sha256 = |environment: &ReleaseEnvironmentEvidence| {
            let mut wire = Vec::new();
            push_environment(&mut wire, environment);
            hex(&Sha256::digest(wire))
        };
        let mut api_tag_only = d3d12.environment.clone();
        let ReleaseRendererEvidence::Rt64 { graphics_api, .. } = &mut api_tag_only.renderer else {
            unreachable!()
        };
        *graphics_api = ReleaseGraphicsApi::Vulkan;
        assert_ne!(
            environment_sha256(&d3d12.environment),
            environment_sha256(&api_tag_only),
            "the typed API tag must bind the environment wire independently of the identity string",
        );
        assert_ne!(
            environment_sha256(&d3d12.environment),
            environment_sha256(&vulkan.environment),
        );
        assert_ne!(d3d12.report_sha256, vulkan.report_sha256);

        let d3d12_declaration = scenario("windows-api", &d3d12);
        let vulkan_declaration = scenario("windows-api", &vulkan);
        assert_ne!(
            d3d12_declaration.declaration_sha256,
            vulkan_declaration.declaration_sha256,
        );
        assert_ne!(
            incomplete_for_report("windows-api", d3d12).assessment_sha256,
            incomplete_for_report("windows-api", vulkan).assessment_sha256,
        );
    }

    #[test]
    fn stale_verified_v16_and_incomplete_v5_schemas_are_rejected() {
        let (_, _, incomplete) = incomplete_fixture();
        let mut stale_incomplete = incomplete;
        stale_incomplete.schema = "fn64.release-matrix-incomplete.v5".to_owned();
        assert!(matches!(
            stale_incomplete.verify_integrity(),
            Err(ReleaseMatrixError::UnsupportedIncompleteSchema(schema))
                if schema == "fn64.release-matrix-incomplete.v5"
        ));

        let stale_verified = VerifiedReleaseMatrix {
            schema: "fn64.verified-release-matrix.v16".to_owned(),
            manifest_sha256: "00".repeat(32),
            profile: CertificationProfileIdentity::full_parity_v1(),
            total_reports: 0,
            scenarios: Vec::new(),
            platform_case_authorities: Vec::new(),
            assignments: Vec::new(),
            verification_sha256: "11".repeat(32),
        };
        assert!(matches!(
            stale_verified.verify_integrity(),
            Err(ReleaseMatrixError::UnsupportedVerifiedSchema(schema))
                if schema == "fn64.verified-release-matrix.v16"
        ));
    }

    #[test]
    fn microcode_credit_requires_project_catalog_and_rsp_rdp_uses_report_events() {
        let public_families = [
            ReleaseMicrocodeFamily::Fast3d,
            ReleaseMicrocodeFamily::F3dex,
            ReleaseMicrocodeFamily::F3dlx,
            ReleaseMicrocodeFamily::F3dlxRej,
            ReleaseMicrocodeFamily::F3dex2,
            ReleaseMicrocodeFamily::F3dex2NoN,
            ReleaseMicrocodeFamily::F3dex2Rej,
            ReleaseMicrocodeFamily::F3dlx2Rej,
            ReleaseMicrocodeFamily::S2dex,
            ReleaseMicrocodeFamily::S2dex2,
            ReleaseMicrocodeFamily::L3dex,
            ReleaseMicrocodeFamily::L3dex2,
        ];
        let certified_digest_families = public_families
            .iter()
            .copied()
            .enumerate()
            .map(|(index, family)| ([u8::try_from(index + 1).unwrap(); 32], family))
            .collect::<Vec<_>>();
        let certified_catalog = certified_digest_families
            .iter()
            .map(|(text_sha256, family)| CertifiedMicrocodeIdentity {
                text_sha256: *text_sha256,
                family: *family,
            })
            .collect::<Vec<_>>();
        let mut ordered = certified_digest_families
            .iter()
            .enumerate()
            .map(
                |(index, (text_sha256, family))| RspRdpObservationEventEvidence {
                    guest_cycle: 50,
                    observation: RspRdpObservationKindEvidence::MicrocodeRecognition {
                        task_address: 0x1000 + index as u32 * 0x40,
                        imem_generation: index as u64 + 1,
                        text_sha256: hex(text_sha256),
                        data_address: 0x4000 + index as u32 * 0x80,
                        data_bytes: 0x80,
                        data_sha256: hex(text_sha256),
                        family: Some(*family),
                    },
                },
            )
            .collect::<Vec<_>>();
        for family in [
            ReleaseMicrocodeFamily::F3dzex2,
            ReleaseMicrocodeFamily::Other { id: 7 },
        ] {
            ordered.push(RspRdpObservationEventEvidence {
                guest_cycle: 51,
                observation: RspRdpObservationKindEvidence::MicrocodeRecognition {
                    task_address: 0x2000,
                    imem_generation: 20,
                    text_sha256: "ee".repeat(32),
                    data_address: 0x6000,
                    data_bytes: 0x80,
                    data_sha256: "ed".repeat(32),
                    family: Some(family),
                },
            });
        }
        ordered.push(RspRdpObservationEventEvidence {
            guest_cycle: 52,
            observation: RspRdpObservationKindEvidence::MicrocodeRecognition {
                task_address: 0x2040,
                imem_generation: 21,
                text_sha256: "ef".repeat(32),
                data_address: 0x6080,
                data_bytes: 0x80,
                data_sha256: "ec".repeat(32),
                family: None,
            },
        });
        ordered.extend([
            RspRdpObservationEventEvidence {
                guest_cycle: 53,
                observation: RspRdpObservationKindEvidence::DramDpcCommitted {
                    start: 0x100,
                    end: 0x108,
                    command_sha256: "f1".repeat(32),
                },
            },
            RspRdpObservationEventEvidence {
                guest_cycle: 54,
                observation: RspRdpObservationKindEvidence::XbusDpcCommitted {
                    start: 0,
                    end: 8,
                    command_sha256: "f2".repeat(32),
                },
            },
            RspRdpObservationEventEvidence {
                guest_cycle: 55,
                observation: RspRdpObservationKindEvidence::ImemReplacementCommitted {
                    task_address: 0x3000,
                    imem_generation: 22,
                    text_sha256: "f3".repeat(32),
                },
            },
        ]);
        let report = with_rsp_rdp_observations(
            closed_report(
                "microcode-reference",
                b"private-microcode",
                0xc3,
                "save.eeprom-4k-operation",
                CLEAN_RT64_IDENTITY,
                Some(ProgramFeature::TypedObservedFunction),
            ),
            ordered,
        );
        let certified_coverage = derive_scenario_coverage_with_catalog(
            "microcode-evidence",
            &report,
            &certified_catalog,
        )
        .unwrap();
        assert_eq!(certified_coverage.microcodes.len(), 12);
        assert_eq!(
            certified_coverage.rsp_rdp_mechanisms,
            vec![
                RspRdpMechanismFeature::DramDpc,
                RspRdpMechanismFeature::XbusDpc,
                RspRdpMechanismFeature::ImemReplacement,
            ]
        );

        let production_coverage = derive_scenario_coverage("microcode-evidence", &report).unwrap();
        assert!(production_coverage.microcodes.is_empty());
        assert_eq!(
            production_coverage.rsp_rdp_mechanisms,
            certified_coverage.rsp_rdp_mechanisms
        );

        let mut mislabeled = report.clone();
        let RspRdpObservationKindEvidence::MicrocodeRecognition { family, .. } =
            &mut mislabeled.rsp_rdp.ordered[0].observation
        else {
            panic!("first fixture event must be microcode recognition");
        };
        *family = Some(ReleaseMicrocodeFamily::F3dex);
        assert!(matches!(
            derive_scenario_coverage_with_catalog(
                "microcode-evidence",
                &mislabeled,
                &certified_catalog
            ),
            Err(ReleaseMatrixError::CertifiedMicrocodeFamilyMismatch {
                certified: ReleaseMicrocodeFamily::Fast3d,
                observed: ReleaseMicrocodeFamily::F3dex,
                ..
            })
        ));

        let manifest = ReleaseMatrixManifest {
            schema: RELEASE_MATRIX_SCHEMA.to_owned(),
            profile: CertificationProfileIdentity::full_parity_v1(),
            scenarios: vec![scenario("microcode-evidence", &report)],
        };
        let ReleaseMatrixVerification::Incomplete(incomplete) =
            verify_release_matrix(&manifest, &evidence_series(report)).unwrap()
        else {
            panic!("platform and full-ROM requirements remain intentionally absent");
        };
        for (class, id) in [
            (CertificationRequirementClass::RspRdpMechanism, "dram-dpc"),
            (
                CertificationRequirementClass::RspRdpMechanism,
                "imem-replacement",
            ),
        ] {
            assert!(incomplete.satisfied.iter().any(|assignment| {
                assignment.requirement.class() == class && assignment.requirement.id() == id
            }));
        }
        assert!(!incomplete.satisfied.iter().any(|assignment| {
            assignment.requirement.class() == CertificationRequirementClass::PublicMicrocode
        }));
    }

    #[test]
    fn incomplete_integrity_rejects_cross_class_duplicates_partition_and_hash_tampering() {
        let (_, _, baseline) = incomplete_fixture();

        let mut instrumentation_drift = baseline.clone();
        instrumentation_drift.unsupported_instrumentation.schema =
            "fn64.unsupported-instrumentation.future".to_owned();
        instrumentation_drift.assessment_sha256 = incomplete_matrix_sha256(&instrumentation_drift);
        assert!(matches!(
            instrumentation_drift.verify_integrity(),
            Err(ReleaseMatrixError::InvalidUnsupportedInstrumentation(_))
        ));

        let mut cross_class = baseline.clone();
        cross_class.missing[0] =
            forged_ref(CertificationRequirementClass::Save, "standard_controller");
        assert!(matches!(
            cross_class.verify_integrity(),
            Err(ReleaseMatrixError::InvalidCertificationProfile(
                crate::CertificationProfileError::UnknownRequirement { .. }
            ))
        ));

        let mut duplicate = baseline.clone();
        duplicate.satisfied.push(duplicate.satisfied[0].clone());
        assert!(matches!(
            duplicate.verify_integrity(),
            Err(ReleaseMatrixError::DuplicateRequirementAssignment { .. })
        ));

        let mut overlap = baseline.clone();
        overlap
            .missing
            .push(overlap.satisfied[0].requirement.clone());
        assert!(matches!(
            overlap.verify_integrity(),
            Err(ReleaseMatrixError::DuplicateRequirementAssignment { .. })
        ));

        let mut missing_partition = baseline.clone();
        missing_partition.missing.pop();
        assert!(matches!(
            missing_partition.verify_integrity(),
            Err(ReleaseMatrixError::InvalidRequirementPartition)
        ));

        let mut duplicate_evidence = baseline.clone();
        let digest = duplicate_evidence.satisfied[0].evidence_sha256s[0].clone();
        duplicate_evidence.satisfied[0]
            .evidence_sha256s
            .push(digest);
        assert!(matches!(
            duplicate_evidence.verify_integrity(),
            Err(ReleaseMatrixError::DuplicateRequirementEvidence { .. })
        ));

        let mut malformed_evidence = baseline.clone();
        malformed_evidence.satisfied[0].evidence_sha256s[0] = "not-a-sha".to_owned();
        assert!(matches!(
            malformed_evidence.verify_integrity(),
            Err(ReleaseMatrixError::InvalidSha256 {
                field: "evidence_sha256s",
                ..
            })
        ));

        let mut empty_evidence = baseline.clone();
        empty_evidence.satisfied[0].evidence_sha256s.clear();
        assert!(matches!(
            empty_evidence.verify_integrity(),
            Err(ReleaseMatrixError::EmptyRequirementEvidence { .. })
        ));

        let mut semantic_hash_change = baseline.clone();
        semantic_hash_change.satisfied[0].evidence_sha256s[0] = "77".repeat(32);
        assert!(matches!(
            semantic_hash_change.verify_integrity(),
            Err(ReleaseMatrixError::IncompleteIntegrityMismatch { .. })
        ));

        let mut assessment_hash = baseline;
        assessment_hash.assessment_sha256 = "88".repeat(32);
        assert!(matches!(
            assessment_hash.verify_integrity(),
            Err(ReleaseMatrixError::IncompleteIntegrityMismatch { .. })
        ));
    }
}
