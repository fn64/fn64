#![allow(clippy::module_inception)]
use super::*;

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
pub(super) struct CertifiedMicrocodeIdentity {
    pub(super) text_sha256: [u8; 32],
    pub(super) family: ReleaseMicrocodeFamily,
}

/// Immutable digest-to-family adjudication for matrix-v5/v15 certification.
///
/// Runtime/backend admission remains host-configurable because it selects an
/// optimization, not certification. Public-microcode denominator credit is
/// intentionally empty until allowed-source digest provenance is reviewed and
/// lands in a new, schema-versioned project catalog.
pub(super) const CERTIFIED_PUBLIC_MICROCODE_CATALOG_V1: &[CertifiedMicrocodeIdentity] = &[];

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
    pub(super) fn recompute_authority_sha256(&self) -> String {
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

    pub(super) fn verify_integrity(&self, id: &str) -> Result<(), ReleaseMatrixError> {
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
    /// Exact scenario string bound by every schema-v29 report in this series.
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
    /// from the verified v29 series.
    pub execution_destinations: ExecutionDestinationEvidence,
    /// Complete schema-v29 RSP/RDP observation stream retained for independent
    /// report reconstruction and coverage derivation.
    pub rsp_rdp: RspRdpEvidence,
    pub unsupported_instrumentation: crate::UnsupportedInstrumentationEvidence,
    /// Exact canonical closure ledger retained from the verified v29 series.
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

pub(super) fn validate_incomplete_platform_authority_assignments(
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
