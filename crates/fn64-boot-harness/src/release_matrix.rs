//! Typed representative-scenario matrix over deterministic release reports.
//!
//! The manifest contains identities and coverage declarations, never ROM bytes
//! or captured output. Dynamic evidence remains the schema-v15 report series;
//! this layer assigns one exact ten-run series to each declared scenario.

use crate::{
    verify_release_evidence_series, ArtifactDigest, ArtifactKind, ClosurePath, ClosurePathStatus,
    DeterministicDigest, ExecutionDestinationEvidence, FramebufferObservationSource,
    ParsedUnsupportedJournal, ReleaseCartridgeSave, ReleaseControllerPort,
    ReleaseEnvironmentEvidence, ReleaseGateReport, ReleaseGraphicsExecutionPolicy,
    ReleaseHostPlatform, ReleaseObservationGeometry, ReleaseRendererEvidence, ReportSeriesError,
    LIVE_MINIMUM_CLOSURE_PATHS,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    fmt,
};

pub const RELEASE_MATRIX_SCHEMA: &str = "fn64.release-matrix.v4";
pub const VERIFIED_RELEASE_MATRIX_SCHEMA: &str = "fn64.verified-release-matrix.v11";
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

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReleaseMatrixCoverage {
    pub platforms: Vec<ReleasePlatform>,
    pub controllers: Vec<ControllerFeature>,
    pub saves: Vec<SaveFeature>,
    pub renderers: Vec<RendererFeature>,
    pub programs: Vec<ProgramFeature>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReleaseMatrixScenario {
    /// Stable manifest key used to associate private report paths at verify time.
    pub id: String,
    /// Exact scenario string bound by every schema-v15 report in this series.
    pub report_scenario: String,
    /// Exact private-input identity bound by every report; no input bytes are stored.
    pub input_sha256: String,
    pub report_sha256: String,
    pub coverage: ReleaseMatrixCoverage,
    /// Canonical digest over this declaration and its exact v15 evidence IDs.
    pub declaration_sha256: String,
}

impl ReleaseMatrixScenario {
    /// Recompute the declaration digest without trusting vector ordering.
    pub fn recompute_declaration_sha256(&self) -> String {
        let mut wire = Vec::new();
        wire.extend_from_slice(b"fn64.release-matrix.scenario.v4\0");
        push_bytes(&mut wire, self.id.as_bytes());
        push_bytes(&mut wire, self.report_scenario.as_bytes());
        push_bytes(&mut wire, self.input_sha256.as_bytes());
        push_bytes(&mut wire, self.report_sha256.as_bytes());
        push_tags(&mut wire, &self.coverage.platforms, ReleasePlatform::tag);
        push_tags(
            &mut wire,
            &self.coverage.controllers,
            ControllerFeature::tag,
        );
        push_tags(&mut wire, &self.coverage.saves, SaveFeature::tag);
        push_tags(&mut wire, &self.coverage.renderers, RendererFeature::tag);
        push_tags(&mut wire, &self.coverage.programs, ProgramFeature::tag);
        hex(&Sha256::digest(wire))
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReleaseMatrixManifest {
    pub schema: String,
    pub required: ReleaseMatrixCoverage,
    pub scenarios: Vec<ReleaseMatrixScenario>,
}

impl ReleaseMatrixManifest {
    /// Canonical identity for the complete policy declaration and its bound
    /// per-scenario evidence identities. No ROM or captured bytes enter it.
    pub fn recompute_manifest_sha256(&self) -> String {
        let mut wire = Vec::new();
        wire.extend_from_slice(b"fn64.release-matrix.manifest.v4\0");
        push_bytes(&mut wire, self.schema.as_bytes());
        push_tags(&mut wire, &self.required.platforms, ReleasePlatform::tag);
        push_tags(
            &mut wire,
            &self.required.controllers,
            ControllerFeature::tag,
        );
        push_tags(&mut wire, &self.required.saves, SaveFeature::tag);
        push_tags(&mut wire, &self.required.renderers, RendererFeature::tag);
        push_tags(&mut wire, &self.required.programs, ProgramFeature::tag);
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
    /// from the verified v15 series.
    pub execution_destinations: ExecutionDestinationEvidence,
    /// Exact canonical closure ledger retained from the verified v15 series.
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
    pub required: ReleaseMatrixCoverage,
    pub total_reports: usize,
    pub scenarios: Vec<VerifiedMatrixScenario>,
    /// Canonical digest over this retained verification result.
    pub verification_sha256: String,
}

impl VerifiedReleaseMatrix {
    pub fn verify_integrity(&self) -> Result<(), ReleaseMatrixError> {
        if self.schema != VERIFIED_RELEASE_MATRIX_SCHEMA {
            return Err(ReleaseMatrixError::UnsupportedVerifiedSchema(
                self.schema.clone(),
            ));
        }
        if self.scenarios.is_empty() || self.scenarios.len() > RELEASE_MATRIX_MAX_SCENARIOS {
            return Err(ReleaseMatrixError::ScenarioCount {
                minimum: 1,
                maximum: RELEASE_MATRIX_MAX_SCENARIOS,
                actual: self.scenarios.len(),
            });
        }
        validate_coverage("verified required", &self.required, None)?;
        validate_sha256("verified-matrix", "manifest_sha256", &self.manifest_sha256)?;

        let mut ids = BTreeSet::new();
        let mut report_scenarios = BTreeSet::new();
        let mut covered_platforms = BTreeSet::new();
        let mut covered_controllers = BTreeSet::new();
        let mut covered_saves = BTreeSet::new();
        let mut covered_renderers = BTreeSet::new();
        let mut covered_programs = BTreeSet::new();
        let mut matrix_run_events = BTreeSet::new();
        let expected_artifacts = BTreeSet::from([
            ArtifactKind::Framebuffer,
            ArtifactKind::Audio,
            ArtifactKind::Memory,
            ArtifactKind::DeviceState,
            ArtifactKind::TimingTrace,
        ]);
        for scenario in &self.scenarios {
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
            validate_coverage(&scenario.id, &scenario.coverage, Some(&self.required))?;
            let declaration = retained_scenario_declaration(scenario);
            validate_scenario_cardinality(&declaration)?;
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
            covered_platforms.extend(scenario.coverage.platforms.iter().copied());
            covered_controllers.extend(scenario.coverage.controllers.iter().copied());
            covered_saves.extend(scenario.coverage.saves.iter().copied());
            covered_renderers.extend(scenario.coverage.renderers.iter().copied());
            covered_programs.extend(scenario.coverage.programs.iter().copied());
            scenario.observations.validate().map_err(|source| {
                ReleaseMatrixError::InvalidVerifiedObservations {
                    id: scenario.id.clone(),
                    source,
                }
            })?;
            validate_environment_coverage(&scenario.id, &scenario.coverage, &scenario.environment)?;
            validate_renderer_observation(
                &scenario.id,
                &scenario.coverage,
                &scenario.observations,
                &scenario.environment,
            )?;
            scenario
                .execution_destinations
                .verify_integrity()
                .map_err(|source| ReleaseMatrixError::InvalidVerifiedDestinations {
                    id: scenario.id.clone(),
                    source,
                })?;
            validate_program_coverage(
                &scenario.id,
                &scenario.coverage,
                &scenario.execution_destinations.source,
            )?;
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
                if crate::render_evidence::validate_authoritative_rt64_backend_identity(
                    backend_identity,
                    scenario.environment.platform,
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
            ReleaseGateReport {
                schema: crate::release_gate::REPORT_SCHEMA.to_owned(),
                scenario: scenario.report_scenario.clone(),
                input_sha256: scenario.input_sha256.clone(),
                digest: scenario.fixed_cycle_digest.clone(),
                observations: scenario.observations.clone(),
                environment: scenario.environment.clone(),
                execution_destinations: scenario.execution_destinations.clone(),
                closure: scenario.closure.clone(),
                report_sha256: scenario.report_sha256.clone(),
            }
            .verify_integrity()
            .map_err(|source| ReleaseMatrixError::InvalidVerifiedReport {
                id: scenario.id.clone(),
                source,
            })?;
        }
        require_all("platforms", &self.required.platforms, &covered_platforms)?;
        require_all(
            "controllers",
            &self.required.controllers,
            &covered_controllers,
        )?;
        require_all("saves", &self.required.saves, &covered_saves)?;
        require_all("renderers", &self.required.renderers, &covered_renderers)?;
        require_all("programs", &self.required.programs, &covered_programs)?;
        let retained_manifest = ReleaseMatrixManifest {
            schema: RELEASE_MATRIX_SCHEMA.to_owned(),
            required: self.required.clone(),
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

/// Verify a bounded representative matrix against private, run-ordered reports.
///
/// Coverage tags identify scenario selection. Save-device and every
/// controller/accessory tag require matching positive operation paths derived
/// by the live gate; platform and renderer tags bind the frozen environment,
/// while the program tag must equal the retained execution-destination source.
pub fn verify_release_matrix(
    manifest: &ReleaseMatrixManifest,
    evidence_by_scenario: &BTreeMap<String, Vec<(ReleaseGateReport, ParsedUnsupportedJournal)>>,
) -> Result<VerifiedReleaseMatrix, ReleaseMatrixError> {
    validate_manifest(manifest)?;

    for id in evidence_by_scenario.keys() {
        if !manifest.scenarios.iter().any(|scenario| &scenario.id == id) {
            return Err(ReleaseMatrixError::UnexpectedEvidence { id: id.clone() });
        }
    }

    let mut verified = Vec::with_capacity(manifest.scenarios.len());
    let mut matrix_run_events = BTreeSet::new();
    for scenario in &manifest.scenarios {
        let evidence = evidence_by_scenario.get(&scenario.id).ok_or_else(|| {
            ReleaseMatrixError::MissingEvidence {
                id: scenario.id.clone(),
            }
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
                source,
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
        validate_feature_operation_paths(&scenario.id, &scenario.coverage, &reports[0].closure)?;
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
        validate_environment_coverage(&scenario.id, &scenario.coverage, &reports[0].environment)?;
        validate_renderer_observation(
            &scenario.id,
            &scenario.coverage,
            &reports[0].observations,
            &reports[0].environment,
        )?;
        validate_program_coverage(
            &scenario.id,
            &scenario.coverage,
            &reports[0].execution_destinations.source,
        )?;
        verified.push(VerifiedMatrixScenario {
            id: scenario.id.clone(),
            count: series.count,
            report_sha256: scenario.report_sha256.clone(),
            report_scenario: scenario.report_scenario.clone(),
            input_sha256: scenario.input_sha256.clone(),
            coverage: scenario.coverage.clone(),
            declaration_sha256: scenario.declaration_sha256.clone(),
            guest_cycle: series.guest_cycle,
            fixed_cycle_digest: reports[0].digest.clone(),
            observations: reports[0].observations.clone(),
            environment: reports[0].environment.clone(),
            execution_destinations: reports[0].execution_destinations.clone(),
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
    let mut result = VerifiedReleaseMatrix {
        schema: VERIFIED_RELEASE_MATRIX_SCHEMA.to_owned(),
        manifest_sha256: manifest.recompute_manifest_sha256(),
        required: manifest.required.clone(),
        total_reports: verified.iter().map(|scenario| scenario.count).sum(),
        scenarios: verified,
        verification_sha256: String::new(),
    };
    result.verification_sha256 = verified_matrix_sha256(&result);
    Ok(result)
}

fn retained_scenario_declaration(scenario: &VerifiedMatrixScenario) -> ReleaseMatrixScenario {
    ReleaseMatrixScenario {
        id: scenario.id.clone(),
        report_scenario: scenario.report_scenario.clone(),
        input_sha256: scenario.input_sha256.clone(),
        report_sha256: scenario.report_sha256.clone(),
        coverage: scenario.coverage.clone(),
        declaration_sha256: scenario.declaration_sha256.clone(),
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

fn validate_program_coverage(
    id: &str,
    coverage: &ReleaseMatrixCoverage,
    source: &crate::ExecutionDestinationSource,
) -> Result<(), ReleaseMatrixError> {
    let observed = match source {
        crate::ExecutionDestinationSource::NoProgram => {
            return Err(ReleaseMatrixError::NoProgramEvidence { id: id.to_owned() });
        }
        crate::ExecutionDestinationSource::NativeArchive { .. } => ProgramFeature::NativeArchive,
        crate::ExecutionDestinationSource::TypedObservedFunctionProgram { .. } => {
            ProgramFeature::TypedObservedFunction
        }
        crate::ExecutionDestinationSource::TypedBlockProgram { .. } => ProgramFeature::TypedBlock,
    };
    let expected = coverage.programs[0];
    if expected == observed {
        Ok(())
    } else {
        Err(ReleaseMatrixError::ProgramCoverageMismatch {
            id: id.to_owned(),
            expected,
            observed,
        })
    }
}

fn validate_renderer_observation(
    id: &str,
    coverage: &ReleaseMatrixCoverage,
    observations: &ReleaseObservationGeometry,
    environment: &ReleaseEnvironmentEvidence,
) -> Result<(), ReleaseMatrixError> {
    let expected = if coverage
        .renderers
        .contains(&RendererFeature::ReferenceLleAccuracy)
    {
        "physical_rdram"
    } else {
        "post_vi_swapchain"
    };
    let observed = match &observations.framebuffer.source {
        FramebufferObservationSource::PhysicalRdram { .. } => "physical_rdram",
        FramebufferObservationSource::PostViSwapchain {
            backend_identity, ..
        } => {
            if expected == "post_vi_swapchain"
                && crate::render_evidence::validate_authoritative_rt64_backend_identity(
                    backend_identity,
                    environment.platform,
                )
                .is_err()
            {
                return Err(ReleaseMatrixError::NonAuthoritativeRt64Identity {
                    id: id.to_owned(),
                    backend_identity: backend_identity.clone(),
                });
            }
            "post_vi_swapchain"
        }
    };
    match (&environment.renderer, &observations.framebuffer.source) {
        (
            ReleaseRendererEvidence::Reference { .. },
            FramebufferObservationSource::PhysicalRdram { .. },
        ) => {}
        (
            ReleaseRendererEvidence::Rt64 {
                backend_identity,
                source_authoritative: true,
                settings_sha256,
                ..
            },
            FramebufferObservationSource::PostViSwapchain {
                backend_identity: observed_identity,
                settings_sha256: observed_settings,
                ..
            },
        ) if backend_identity == observed_identity && settings_sha256 == observed_settings => {}
        _ => {
            return Err(ReleaseMatrixError::RendererEnvironmentMismatch { id: id.to_owned() });
        }
    }
    if expected == observed {
        Ok(())
    } else {
        Err(ReleaseMatrixError::RendererObservationMismatch {
            id: id.to_owned(),
            expected,
            observed,
        })
    }
}

fn validate_environment_coverage(
    id: &str,
    declared: &ReleaseMatrixCoverage,
    environment: &ReleaseEnvironmentEvidence,
) -> Result<(), ReleaseMatrixError> {
    let observed_platforms = vec![match environment.platform {
        ReleaseHostPlatform::MacosArm64 => ReleasePlatform::MacosArm64,
        ReleaseHostPlatform::LinuxX86_64 => ReleasePlatform::LinuxX86_64,
        ReleaseHostPlatform::WindowsX86_64 => ReleasePlatform::WindowsX86_64,
    }];
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
    let observed_saves = vec![match environment.cartridge_save {
        ReleaseCartridgeSave::NoCartridgeSave => SaveFeature::NoCartridgeSave,
        ReleaseCartridgeSave::Eeprom4k => SaveFeature::Eeprom4Kbit,
        ReleaseCartridgeSave::Eeprom16k => SaveFeature::Eeprom16Kbit,
        ReleaseCartridgeSave::Sram32Kib => SaveFeature::Sram32Kib,
        ReleaseCartridgeSave::FlashRam128Kib => SaveFeature::FlashRam128Kib,
    }];
    let observed_renderers: Vec<_> = match &environment.renderer {
        ReleaseRendererEvidence::Reference {
            execution_policy: ReleaseGraphicsExecutionPolicy::LleAccuracy,
        } => vec![RendererFeature::ReferenceLleAccuracy],
        ReleaseRendererEvidence::Rt64 {
            execution_policy: ReleaseGraphicsExecutionPolicy::LleAccuracy,
            replacement_packs_active,
            ..
        } => {
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

    require_exact_environment_dimension(id, "platforms", &declared.platforms, observed_platforms)?;
    require_exact_environment_dimension(
        id,
        "controllers",
        &declared.controllers,
        observed_controllers.into_iter().collect(),
    )?;
    require_exact_environment_dimension(id, "saves", &declared.saves, observed_saves)?;
    require_exact_environment_dimension(id, "renderers", &declared.renderers, observed_renderers)
}

fn require_exact_environment_dimension<T: Copy + fmt::Debug + Ord>(
    id: &str,
    dimension: &'static str,
    declared: &[T],
    observed: Vec<T>,
) -> Result<(), ReleaseMatrixError> {
    let declared: BTreeSet<_> = declared.iter().copied().collect();
    let observed: BTreeSet<_> = observed.into_iter().collect();
    if declared == observed {
        Ok(())
    } else {
        Err(ReleaseMatrixError::EnvironmentCoverageMismatch {
            id: id.to_owned(),
            dimension,
            declared: declared
                .into_iter()
                .map(|value| format!("{value:?}"))
                .collect(),
            observed: observed
                .into_iter()
                .map(|value| format!("{value:?}"))
                .collect(),
        })
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

fn validate_manifest(manifest: &ReleaseMatrixManifest) -> Result<(), ReleaseMatrixError> {
    if manifest.schema != RELEASE_MATRIX_SCHEMA {
        return Err(ReleaseMatrixError::UnsupportedSchema(
            manifest.schema.clone(),
        ));
    }
    if manifest.scenarios.is_empty() || manifest.scenarios.len() > RELEASE_MATRIX_MAX_SCENARIOS {
        return Err(ReleaseMatrixError::ScenarioCount {
            minimum: 1,
            maximum: RELEASE_MATRIX_MAX_SCENARIOS,
            actual: manifest.scenarios.len(),
        });
    }
    validate_coverage("required", &manifest.required, None)?;

    let mut ids = BTreeSet::new();
    let mut report_scenarios = BTreeSet::new();
    let mut covered_platforms = BTreeSet::new();
    let mut covered_controllers = BTreeSet::new();
    let mut covered_saves = BTreeSet::new();
    let mut covered_renderers = BTreeSet::new();
    let mut covered_programs = BTreeSet::new();

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
        validate_coverage(&scenario.id, &scenario.coverage, Some(&manifest.required))?;
        validate_scenario_cardinality(scenario)?;
        let recomputed = scenario.recompute_declaration_sha256();
        if scenario.declaration_sha256 != recomputed {
            return Err(ReleaseMatrixError::DeclarationDigestMismatch {
                id: scenario.id.clone(),
                stored: scenario.declaration_sha256.clone(),
                recomputed,
            });
        }

        covered_platforms.extend(scenario.coverage.platforms.iter().copied());
        covered_controllers.extend(scenario.coverage.controllers.iter().copied());
        covered_saves.extend(scenario.coverage.saves.iter().copied());
        covered_renderers.extend(scenario.coverage.renderers.iter().copied());
        covered_programs.extend(scenario.coverage.programs.iter().copied());
    }

    require_all(
        "platforms",
        &manifest.required.platforms,
        &covered_platforms,
    )?;
    require_all(
        "controllers",
        &manifest.required.controllers,
        &covered_controllers,
    )?;
    require_all("saves", &manifest.required.saves, &covered_saves)?;
    require_all(
        "renderers",
        &manifest.required.renderers,
        &covered_renderers,
    )?;
    require_all("programs", &manifest.required.programs, &covered_programs)?;
    Ok(())
}

fn validate_scenario_cardinality(
    scenario: &ReleaseMatrixScenario,
) -> Result<(), ReleaseMatrixError> {
    if scenario.coverage.platforms.len() != 1 {
        return Err(ReleaseMatrixError::ExactOneCoverage {
            id: scenario.id.clone(),
            dimension: "platforms",
            actual: scenario.coverage.platforms.len(),
        });
    }
    if scenario.coverage.saves.len() != 1 {
        return Err(ReleaseMatrixError::ExactOneCoverage {
            id: scenario.id.clone(),
            dimension: "saves",
            actual: scenario.coverage.saves.len(),
        });
    }
    if scenario.coverage.programs.len() != 1 {
        return Err(ReleaseMatrixError::ExactOneCoverage {
            id: scenario.id.clone(),
            dimension: "programs",
            actual: scenario.coverage.programs.len(),
        });
    }

    let has_reference = scenario
        .coverage
        .renderers
        .contains(&RendererFeature::ReferenceLleAccuracy);
    let has_rt64 = scenario
        .coverage
        .renderers
        .contains(&RendererFeature::Rt64LleAccuracy);
    let valid_renderer = match (has_reference, has_rt64) {
        (true, false) => scenario.coverage.renderers.len() == 1,
        (false, true) => true,
        _ => false,
    };
    if !valid_renderer {
        return Err(ReleaseMatrixError::InvalidRendererCombination {
            id: scenario.id.clone(),
        });
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

fn require_all<T: Copy + fmt::Debug + Ord>(
    dimension: &'static str,
    required: &[T],
    covered: &BTreeSet<T>,
) -> Result<(), ReleaseMatrixError> {
    let missing: Vec<String> = required
        .iter()
        .filter(|value| !covered.contains(value))
        .map(|value| format!("{value:?}"))
        .collect();
    if missing.is_empty() {
        Ok(())
    } else {
        Err(ReleaseMatrixError::MissingRequiredCoverage { dimension, missing })
    }
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

fn verified_matrix_sha256(report: &VerifiedReleaseMatrix) -> String {
    let mut wire = Vec::new();
    wire.extend_from_slice(b"fn64.verified-release-matrix.v11\0");
    push_bytes(&mut wire, report.schema.as_bytes());
    push_bytes(&mut wire, report.manifest_sha256.as_bytes());
    push_tags(&mut wire, &report.required.platforms, ReleasePlatform::tag);
    push_tags(
        &mut wire,
        &report.required.controllers,
        ControllerFeature::tag,
    );
    push_tags(&mut wire, &report.required.saves, SaveFeature::tag);
    push_tags(&mut wire, &report.required.renderers, RendererFeature::tag);
    push_tags(&mut wire, &report.required.programs, ProgramFeature::tag);
    wire.extend_from_slice(&(report.total_reports as u64).to_be_bytes());

    let mut scenarios: Vec<_> = report.scenarios.iter().collect();
    scenarios.sort_by(|left, right| left.id.cmp(&right.id));
    wire.extend_from_slice(&(scenarios.len() as u32).to_be_bytes());
    for scenario in scenarios {
        push_bytes(&mut wire, scenario.id.as_bytes());
        wire.extend_from_slice(&(scenario.count as u64).to_be_bytes());
        push_bytes(&mut wire, scenario.report_sha256.as_bytes());
        push_bytes(&mut wire, scenario.report_scenario.as_bytes());
        push_bytes(&mut wire, scenario.input_sha256.as_bytes());
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
    hex(&Sha256::digest(wire))
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
        ReleaseRendererEvidence::Reference { execution_policy } => {
            wire.push(0);
            wire.push(release_execution_policy_tag(*execution_policy));
        }
        ReleaseRendererEvidence::Rt64 {
            execution_policy,
            backend_identity,
            source_authoritative,
            settings_sha256,
            replacement_packs_active,
        } => {
            wire.push(1);
            wire.push(release_execution_policy_tag(*execution_policy));
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
        source: ReportSeriesError,
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
            Self::InvalidSeries { source, .. } => Some(source),
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
        ArtifactKind, ClosurePath, ClosurePathStatus, FixedCycleDigestGate, LiveRenderEvidence,
        RenderPixelFormat, LIVE_MINIMUM_CLOSURE_PATHS,
    };

    const CLEAN_RT64_IDENTITY: &str = concat!(
        "adapter=fn64-render-rt64/rt64;adapter_sha256=",
        "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        ";source=git:",
        "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
        ";provenance=git-clean;overlay=fn64-test;post_vi_api=vulkan-bgra8-rgba8-unorm"
    );

    fn closed_report(
        scenario: &str,
        input: &[u8],
        framebuffer_byte: u8,
        feature_path: &str,
        rt64_identity: &str,
        program: Option<ProgramFeature>,
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
                ReleaseHostPlatform::LinuxX86_64
            } else {
                ReleaseHostPlatform::MacosArm64
            },
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
            renderer: if is_rt64 {
                ReleaseRendererEvidence::Rt64 {
                    execution_policy: ReleaseGraphicsExecutionPolicy::LleAccuracy,
                    backend_identity: rt64_identity.to_owned(),
                    source_authoritative: true,
                    settings_sha256: "11".repeat(32),
                    replacement_packs_active: false,
                }
            } else {
                ReleaseRendererEvidence::Reference {
                    execution_policy: ReleaseGraphicsExecutionPolicy::LleAccuracy,
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

    fn coverage(
        platform: ReleasePlatform,
        controller: ControllerFeature,
        save: SaveFeature,
        renderer: RendererFeature,
    ) -> ReleaseMatrixCoverage {
        ReleaseMatrixCoverage {
            platforms: vec![platform],
            controllers: vec![controller],
            saves: vec![save],
            renderers: vec![renderer],
            programs: vec![ProgramFeature::TypedObservedFunction],
        }
    }

    fn fixture() -> (
        ReleaseMatrixManifest,
        BTreeMap<String, Vec<(ReleaseGateReport, ParsedUnsupportedJournal)>>,
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
        let mut manifest = ReleaseMatrixManifest {
            schema: RELEASE_MATRIX_SCHEMA.to_owned(),
            required: ReleaseMatrixCoverage {
                platforms: vec![ReleasePlatform::MacosArm64, ReleasePlatform::LinuxX86_64],
                controllers: vec![
                    ControllerFeature::StandardController,
                    ControllerFeature::RumblePak,
                ],
                saves: vec![SaveFeature::Eeprom4Kbit, SaveFeature::Sram32Kib],
                renderers: vec![
                    RendererFeature::ReferenceLleAccuracy,
                    RendererFeature::Rt64LleAccuracy,
                    RendererFeature::Rt64PostViCapture,
                ],
                programs: vec![ProgramFeature::TypedObservedFunction],
            },
            scenarios: vec![
                ReleaseMatrixScenario {
                    id: "game-a-reference".to_owned(),
                    report_scenario: reference.scenario.clone(),
                    input_sha256: reference.input_sha256.clone(),
                    report_sha256: reference.report_sha256.clone(),
                    coverage: coverage(
                        ReleasePlatform::MacosArm64,
                        ControllerFeature::StandardController,
                        SaveFeature::Eeprom4Kbit,
                        RendererFeature::ReferenceLleAccuracy,
                    ),
                    declaration_sha256: String::new(),
                },
                ReleaseMatrixScenario {
                    id: "game-b-rt64".to_owned(),
                    report_scenario: rt64.scenario.clone(),
                    input_sha256: rt64.input_sha256.clone(),
                    report_sha256: rt64.report_sha256.clone(),
                    coverage: {
                        let mut coverage = coverage(
                            ReleasePlatform::LinuxX86_64,
                            ControllerFeature::RumblePak,
                            SaveFeature::Sram32Kib,
                            RendererFeature::Rt64LleAccuracy,
                        );
                        coverage
                            .controllers
                            .push(ControllerFeature::StandardController);
                        coverage.renderers.push(RendererFeature::Rt64PostViCapture);
                        coverage
                    },
                    declaration_sha256: String::new(),
                },
            ],
        };
        for scenario in &mut manifest.scenarios {
            scenario.declaration_sha256 = scenario.recompute_declaration_sha256();
        }
        let reports = BTreeMap::from([
            ("game-a-reference".to_owned(), evidence_series(reference)),
            ("game-b-rt64".to_owned(), evidence_series(rt64)),
        ]);
        (manifest, reports)
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

    #[test]
    fn accepts_complete_exact_ten_report_matrix() {
        let (manifest, reports) = fixture();
        let verified = verify_release_matrix(&manifest, &reports).unwrap();
        verified.verify_integrity().unwrap();
        assert_eq!(verified.schema, VERIFIED_RELEASE_MATRIX_SCHEMA);
        assert_eq!(verified.total_reports, 20);
        assert_eq!(
            verified.manifest_sha256,
            manifest.recompute_manifest_sha256()
        );
        assert_eq!(verified.scenarios.len(), 2);
        assert!(verified
            .scenarios
            .iter()
            .all(|scenario| scenario.count == 10));
        assert!(verified.scenarios.iter().all(|scenario| scenario
            .fixed_cycle_digest
            .artifacts
            .len()
            == 5
            && scenario.unsupported_events == 0
            && scenario.unsupported_journal_schema == "fn64.unsupported-journal.v3"
            && scenario.bound_journals == 10
            && scenario.run_event_sha256s.len() == 10));
        assert_eq!(
            verified.scenarios[0].coverage,
            manifest.scenarios[0].coverage
        );
        assert_eq!(
            verified.scenarios[1].coverage,
            manifest.scenarios[1].coverage
        );
    }

    #[test]
    fn program_lane_is_derived_from_destinations_and_cannot_be_cross_labeled() {
        let (mut manifest, reports) = fixture();
        manifest.required.programs = vec![
            ProgramFeature::TypedObservedFunction,
            ProgramFeature::NativeArchive,
        ];
        manifest.scenarios[0].coverage.programs = vec![ProgramFeature::NativeArchive];
        manifest.scenarios[0].declaration_sha256 =
            manifest.scenarios[0].recompute_declaration_sha256();
        assert!(matches!(
            verify_release_matrix(&manifest, &reports),
            Err(ReleaseMatrixError::ProgramCoverageMismatch {
                expected: ProgramFeature::NativeArchive,
                observed: ProgramFeature::TypedObservedFunction,
                ..
            })
        ));

        let (manifest, reports) = fixture();
        let mut verified = verify_release_matrix(&manifest, &reports).unwrap();
        let baseline_sha = verified.verification_sha256.clone();
        verified
            .required
            .programs
            .push(ProgramFeature::NativeArchive);
        verified.scenarios[0].coverage.programs = vec![ProgramFeature::NativeArchive];
        verified.scenarios[0].declaration_sha256 =
            retained_scenario_declaration(&verified.scenarios[0]).recompute_declaration_sha256();
        verified.verification_sha256 = verified_matrix_sha256(&verified);
        assert_ne!(verified.verification_sha256, baseline_sha);
        assert!(matches!(
            verified.verify_integrity(),
            Err(ReleaseMatrixError::ProgramCoverageMismatch { .. })
        ));
    }

    #[test]
    fn every_program_lane_accepts_only_its_matching_destination_source() {
        for (id, program) in [
            ("program-native-archive", ProgramFeature::NativeArchive),
            (
                "program-typed-function",
                ProgramFeature::TypedObservedFunction,
            ),
            ("program-typed-block", ProgramFeature::TypedBlock),
        ] {
            let report = closed_report(
                id,
                b"private-program",
                0x77,
                "save.eeprom-4k-operation",
                CLEAN_RT64_IDENTITY,
                Some(program),
            );
            let coverage = ReleaseMatrixCoverage {
                platforms: vec![ReleasePlatform::MacosArm64],
                controllers: vec![ControllerFeature::StandardController],
                saves: vec![SaveFeature::Eeprom4Kbit],
                renderers: vec![RendererFeature::ReferenceLleAccuracy],
                programs: vec![program],
            };
            let mut scenario = ReleaseMatrixScenario {
                id: id.to_owned(),
                report_scenario: report.scenario.clone(),
                input_sha256: report.input_sha256.clone(),
                report_sha256: report.report_sha256.clone(),
                coverage: coverage.clone(),
                declaration_sha256: String::new(),
            };
            scenario.declaration_sha256 = scenario.recompute_declaration_sha256();
            let manifest = ReleaseMatrixManifest {
                schema: RELEASE_MATRIX_SCHEMA.to_owned(),
                required: coverage,
                scenarios: vec![scenario],
            };
            let reports = BTreeMap::from([(id.to_owned(), evidence_series(report))]);

            let verified = verify_release_matrix(&manifest, &reports).unwrap();
            assert_eq!(verified.scenarios[0].coverage.programs, vec![program]);
            verified.verify_integrity().unwrap();
        }
    }

    #[test]
    fn representative_matrix_rejects_a_report_without_program_entry_evidence() {
        let (mut manifest, mut reports) = fixture();
        let original = &reports["game-a-reference"][0].0;
        let replacement = ReleaseGateReport::new_with_test_environment(
            original.scenario.clone(),
            b"private-a",
            original.digest.clone(),
            original.observations.clone(),
            original.environment.clone(),
            original.closure.clone(),
        )
        .unwrap();
        reports.insert(
            "game-a-reference".to_owned(),
            evidence_series(replacement.clone()),
        );
        manifest.scenarios[0].report_sha256 = replacement.report_sha256;
        manifest.scenarios[0].declaration_sha256 =
            manifest.scenarios[0].recompute_declaration_sha256();
        assert!(matches!(
            verify_release_matrix(&manifest, &reports),
            Err(ReleaseMatrixError::NoProgramEvidence { .. })
        ));
    }

    #[test]
    fn renderer_declaration_cannot_cross_label_observation_source() {
        let (mut manifest, mut reports) = fixture();
        let original = &reports["game-b-rt64"][0].0;
        let physical_digest = reports["game-a-reference"][0].0.digest.clone();
        let mut replacement_environment = original.environment.clone();
        replacement_environment.renderer = ReleaseRendererEvidence::Reference {
            execution_policy: ReleaseGraphicsExecutionPolicy::LleAccuracy,
        };
        let replacement = ReleaseGateReport::new_with_test_environment(
            original.scenario.clone(),
            b"private-b",
            physical_digest,
            ReleaseObservationGeometry::reference_rdram(0, 1, 1).unwrap(),
            replacement_environment,
            original.closure.clone(),
        )
        .unwrap();
        reports.insert(
            "game-b-rt64".to_owned(),
            evidence_series(replacement.clone()),
        );
        manifest.scenarios[1].report_sha256 = replacement.report_sha256;
        manifest.scenarios[1].declaration_sha256 =
            manifest.scenarios[1].recompute_declaration_sha256();

        assert!(matches!(
            verify_release_matrix(&manifest, &reports),
            Err(ReleaseMatrixError::EnvironmentCoverageMismatch {
                dimension: "renderers",
                ..
            })
        ));
    }

    #[test]
    fn rt64_certification_rejects_unbound_or_nonauthoritative_identities() {
        for identity in [
            concat!(
                "adapter=fn64-render-rt64/rt64;adapter_sha256=",
                "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                ";source=git:",
                "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
                ";provenance=git-dirty;overlay=fn64-test;post_vi_api=vulkan-bgra8-rgba8-unorm"
            ),
            concat!(
                "adapter=fn64-render-rt64/rt64;adapter_sha256=",
                "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                ";source=declared:test;provenance=declared;overlay=fn64-test;post_vi_api=vulkan-bgra8-rgba8-unorm"
            ),
            concat!(
                "adapter=fn64-render-rt64/rt64;source=git:",
                "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
                ";provenance=git-clean;overlay=fn64-test;post_vi_api=vulkan-bgra8-rgba8-unorm"
            ),
            concat!(
                "adapter=fn64-render-rt64/rt64;adapter_sha256=NOT-A-SHA;source=git:",
                "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
                ";provenance=git-clean;overlay=fn64-test;post_vi_api=vulkan-bgra8-rgba8-unorm"
            ),
            concat!(
                "adapter=fn64-render-rt64/rt64;adapter_sha256=",
                "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                ";source=abc123;provenance=git-clean;overlay=fn64-test;post_vi_api=vulkan-bgra8-rgba8-unorm"
            ),
            concat!(
                "adapter=fn64-render-rt64/rt64;adapter_sha256=",
                "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                ";source=git:",
                "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
                ";provenance=git-clean;overlay=fn64-test;post_vi_api=metal-bgra8-unorm"
            ),
            "synthetic-release-backend",
        ] {
            assert!(
                crate::render_evidence::validate_authoritative_rt64_backend_identity(
                identity,
                    ReleaseHostPlatform::LinuxX86_64,
                )
                .is_err(),
                "invalid identity passed: {identity}"
            );
        }
    }

    #[test]
    fn verified_matrix_json_binds_artifacts_zero_unsupported_and_presentation_boundary() {
        let (mut manifest, reports) = fixture();
        manifest.scenarios[1].declaration_sha256 =
            manifest.scenarios[1].recompute_declaration_sha256();

        let verified = verify_release_matrix(&manifest, &reports).unwrap();
        let json = serde_json::to_string(&verified).unwrap();
        let retained: VerifiedReleaseMatrix = serde_json::from_str(&json).unwrap();
        retained.verify_integrity().unwrap();
        assert!(json.contains("\"coverage\""));
        assert!(json.contains("\"framebuffer\""));
        assert!(json.contains("\"audio\""));
        assert!(json.contains("\"memory\""));
        assert!(json.contains("\"device_state\""));
        assert!(json.contains("\"timing_trace\""));
        assert_eq!(
            retained.scenarios[1].presentation_boundary,
            PresentationBoundaryEvidence::ExactPostViCapture
        );

        let mut mutated = retained.clone();
        mutated.scenarios[0].fixed_cycle_digest.artifacts[0].sha256 = "0".repeat(64);
        assert!(matches!(
            mutated.verify_integrity(),
            Err(ReleaseMatrixError::InvalidVerifiedDigest { .. })
        ));

        mutated.scenarios[0].fixed_cycle_digest.artifacts.pop();
        assert!(matches!(
            mutated.verify_integrity(),
            Err(ReleaseMatrixError::VerifiedArtifactSet { .. })
        ));

        let mut invalid_destinations = retained.clone();
        invalid_destinations.scenarios[0]
            .execution_destinations
            .total_observations = 2;
        invalid_destinations.verification_sha256 = verified_matrix_sha256(&invalid_destinations);
        assert!(matches!(
            invalid_destinations.verify_integrity(),
            Err(ReleaseMatrixError::InvalidVerifiedDestinations { .. })
        ));

        let mut wrong_memory_count = retained.clone();
        let digest = &mut wrong_memory_count.scenarios[0].fixed_cycle_digest;
        digest.artifacts[2].bytes -= 1;
        digest.root_sha256 =
            crate::release_gate::recompute_digest_root(digest.guest_cycle, &digest.artifacts)
                .unwrap();
        wrong_memory_count.verification_sha256 = verified_matrix_sha256(&wrong_memory_count);
        assert!(matches!(
            wrong_memory_count.verify_integrity(),
            Err(ReleaseMatrixError::InvalidVerifiedDigest {
                source: crate::GateError::ArtifactObservationByteMismatch {
                    kind: ArtifactKind::Memory,
                    ..
                },
                ..
            })
        ));

        let mut dirty_identity = retained.clone();
        let FramebufferObservationSource::PostViSwapchain {
            backend_identity, ..
        } = &mut dirty_identity.scenarios[1].observations.framebuffer.source
        else {
            unreachable!("fixture RT64 scenario uses post-VI evidence")
        };
        *backend_identity =
            backend_identity.replace("provenance=git-clean", "provenance=git-dirty");
        dirty_identity.verification_sha256 = verified_matrix_sha256(&dirty_identity);
        assert!(matches!(
            dirty_identity.verify_integrity(),
            Err(ReleaseMatrixError::NonAuthoritativeRt64Identity { .. })
        ));

        let mut unbound = verified;
        unbound.scenarios[0].bound_journals = 9;
        assert!(matches!(
            unbound.verify_integrity(),
            Err(ReleaseMatrixError::VerifiedJournalBinding { .. })
        ));

        let mut replayed = retained.clone();
        replayed.scenarios[1].run_event_sha256s[0] =
            replayed.scenarios[0].run_event_sha256s[0].clone();
        replayed.verification_sha256 = verified_matrix_sha256(&replayed);
        assert!(matches!(
            replayed.verify_integrity(),
            Err(ReleaseMatrixError::DuplicateRunEventIdentity { .. })
        ));

        let mut historical_schema = retained;
        historical_schema.schema = "fn64.verified-release-matrix.v9".to_owned();
        assert!(matches!(
            historical_schema.verify_integrity(),
            Err(ReleaseMatrixError::UnsupportedVerifiedSchema(schema))
                if schema == "fn64.verified-release-matrix.v9"
        ));
    }

    #[test]
    fn verified_matrix_v11_wire_binds_every_frozen_environment_field() {
        let (manifest, reports) = fixture();
        let verified = verify_release_matrix(&manifest, &reports).unwrap();
        let baseline = verified_matrix_sha256(&verified);

        macro_rules! changed {
            ($name:literal, $body:expr) => {{
                let mut value = verified.clone();
                $body(&mut value.scenarios[1].environment);
                assert_ne!(verified_matrix_sha256(&value), baseline, $name);
            }};
        }

        for platform in [
            ReleaseHostPlatform::MacosArm64,
            ReleaseHostPlatform::LinuxX86_64,
            ReleaseHostPlatform::WindowsX86_64,
        ] {
            if platform != verified.scenarios[1].environment.platform {
                changed!(
                    "platform collided",
                    |environment: &mut ReleaseEnvironmentEvidence| {
                        environment.platform = platform;
                    }
                );
            }
        }
        for index in 0..4 {
            for state in [
                ReleaseControllerPort::StandardControllerNoPak,
                ReleaseControllerPort::StandardControllerControllerPak,
                ReleaseControllerPort::StandardControllerRumblePak,
                ReleaseControllerPort::StandardControllerTransferPak,
                ReleaseControllerPort::VoiceRecognitionUnit,
                ReleaseControllerPort::Absent,
            ] {
                if state != verified.scenarios[1].environment.controller_ports[index] {
                    changed!(
                        "controller placement collided",
                        |environment: &mut ReleaseEnvironmentEvidence| {
                            environment.controller_ports[index] = state;
                        }
                    );
                }
            }
        }
        for save in [
            ReleaseCartridgeSave::NoCartridgeSave,
            ReleaseCartridgeSave::Eeprom4k,
            ReleaseCartridgeSave::Eeprom16k,
            ReleaseCartridgeSave::Sram32Kib,
            ReleaseCartridgeSave::FlashRam128Kib,
        ] {
            if save != verified.scenarios[1].environment.cartridge_save {
                changed!(
                    "cartridge save collided",
                    |environment: &mut ReleaseEnvironmentEvidence| {
                        environment.cartridge_save = save;
                    }
                );
            }
        }
        changed!(
            "renderer class collided",
            |environment: &mut ReleaseEnvironmentEvidence| {
                environment.renderer = ReleaseRendererEvidence::Reference {
                    execution_policy: ReleaseGraphicsExecutionPolicy::LleAccuracy,
                };
            }
        );
        changed!(
            "execution policy collided",
            |environment: &mut ReleaseEnvironmentEvidence| {
                let ReleaseRendererEvidence::Rt64 {
                    execution_policy, ..
                } = &mut environment.renderer
                else {
                    unreachable!()
                };
                *execution_policy = ReleaseGraphicsExecutionPolicy::HleOptimized;
            }
        );
        changed!(
            "backend identity collided",
            |environment: &mut ReleaseEnvironmentEvidence| {
                let ReleaseRendererEvidence::Rt64 {
                    backend_identity, ..
                } = &mut environment.renderer
                else {
                    unreachable!()
                };
                backend_identity.push_str("-changed");
            }
        );
        changed!(
            "source authority collided",
            |environment: &mut ReleaseEnvironmentEvidence| {
                let ReleaseRendererEvidence::Rt64 {
                    source_authoritative,
                    ..
                } = &mut environment.renderer
                else {
                    unreachable!()
                };
                *source_authoritative = false;
            }
        );
        changed!(
            "settings identity collided",
            |environment: &mut ReleaseEnvironmentEvidence| {
                let ReleaseRendererEvidence::Rt64 {
                    settings_sha256, ..
                } = &mut environment.renderer
                else {
                    unreachable!()
                };
                *settings_sha256 = "22".repeat(32);
            }
        );
        changed!(
            "replacement activity collided",
            |environment: &mut ReleaseEnvironmentEvidence| {
                let ReleaseRendererEvidence::Rt64 {
                    replacement_packs_active,
                    ..
                } = &mut environment.renderer
                else {
                    unreachable!()
                };
                *replacement_packs_active = true;
            }
        );
    }

    #[test]
    fn retained_rt64_workload_is_nonzero_and_bound_to_report_and_matrix() {
        let (manifest, reports) = fixture();
        let verified = verify_release_matrix(&manifest, &reports).unwrap();
        let baseline_sha = verified_matrix_sha256(&verified);

        let mut changed = verified.clone();
        let FramebufferObservationSource::PostViSwapchain { workload_id, .. } =
            &mut changed.scenarios[1].observations.framebuffer.source
        else {
            unreachable!("fixture RT64 scenario uses post-VI evidence")
        };
        *workload_id = std::num::NonZeroU64::new(workload_id.get() + 1).unwrap();
        assert_ne!(verified_matrix_sha256(&changed), baseline_sha);
        changed.verification_sha256 = verified_matrix_sha256(&changed);
        assert!(matches!(
            changed.verify_integrity(),
            Err(ReleaseMatrixError::InvalidVerifiedReport {
                source: crate::GateError::ReportIntegrityMismatch { .. },
                ..
            })
        ));

        let mut zero = serde_json::to_value(&verified).unwrap();
        zero["scenarios"][1]["observations"]["framebuffer"]["source"]["workload_id"] = 0.into();
        assert!(serde_json::from_value::<VerifiedReleaseMatrix>(zero).is_err());
    }

    #[test]
    fn retained_matrix_revalidates_semantic_envelope_after_digest_recomputation() {
        let (manifest, reports) = fixture();
        let verified = verify_release_matrix(&manifest, &reports).unwrap();

        let mut bad_self_digest = verified.clone();
        bad_self_digest.verification_sha256 = "00".repeat(32);
        assert!(matches!(
            bad_self_digest.verify_integrity(),
            Err(ReleaseMatrixError::VerifiedIntegrityMismatch { .. })
        ));

        let mut empty = verified.clone();
        empty.scenarios.clear();
        empty.total_reports = 0;
        empty.verification_sha256 = verified_matrix_sha256(&empty);
        assert!(matches!(
            empty.verify_integrity(),
            Err(ReleaseMatrixError::ScenarioCount { actual: 0, .. })
        ));

        let mut empty_required = verified.clone();
        empty_required.required.platforms.clear();
        empty_required.verification_sha256 = verified_matrix_sha256(&empty_required);
        assert!(matches!(
            empty_required.verify_integrity(),
            Err(ReleaseMatrixError::EmptyCoverage {
                ref scope,
                dimension: "platforms"
            }) if scope == "verified required"
        ));

        let mut duplicate_id = verified.clone();
        duplicate_id.scenarios[1].id = duplicate_id.scenarios[0].id.clone();
        duplicate_id.verification_sha256 = verified_matrix_sha256(&duplicate_id);
        assert!(matches!(
            duplicate_id.verify_integrity(),
            Err(ReleaseMatrixError::DuplicateScenarioId(_))
        ));

        let mut malformed_manifest_identity = verified.clone();
        malformed_manifest_identity.manifest_sha256 = "not-a-sha256".to_owned();
        malformed_manifest_identity.verification_sha256 =
            verified_matrix_sha256(&malformed_manifest_identity);
        assert!(matches!(
            malformed_manifest_identity.verify_integrity(),
            Err(ReleaseMatrixError::InvalidSha256 {
                field: "manifest_sha256",
                ..
            })
        ));

        let mut no_positive_closure = verified;
        no_positive_closure.scenarios[0].closure_paths = 0;
        no_positive_closure.verification_sha256 = verified_matrix_sha256(&no_positive_closure);
        assert!(matches!(
            no_positive_closure.verify_integrity(),
            Err(ReleaseMatrixError::VerifiedClosurePathCountMismatch { stored: 0, .. })
        ));
    }

    #[test]
    fn retained_matrix_revalidates_exact_feature_operation_paths() {
        let (manifest, reports) = fixture();
        let verified = verify_release_matrix(&manifest, &reports).unwrap();

        let mut missing = verified.clone();
        missing.scenarios[1]
            .closure
            .retain(|path| path.name != "controller.rumble-operation");
        missing.scenarios[1].closure_paths = missing.scenarios[1].closure.len() as u64;
        missing.verification_sha256 = verified_matrix_sha256(&missing);
        assert!(matches!(
            missing.verify_integrity(),
            Err(ReleaseMatrixError::MissingFeatureObservation { path, .. })
                if path == "controller.rumble-operation"
        ));

        let mut unexpected = verified;
        unexpected.scenarios[0].closure.push(ClosurePath {
            name: "controller.transfer-pak-operation".to_owned(),
            observations: 1,
            status: ClosurePathStatus::ExercisedZeroUnsupported,
            unsupported: Vec::new(),
        });
        unexpected.scenarios[0]
            .closure
            .sort_by(|left, right| left.name.cmp(&right.name));
        unexpected.scenarios[0].closure_paths = unexpected.scenarios[0].closure.len() as u64;
        unexpected.verification_sha256 = verified_matrix_sha256(&unexpected);
        assert!(matches!(
            unexpected.verify_integrity(),
            Err(ReleaseMatrixError::UnexpectedFeatureObservation { path, .. })
                if path == "controller.transfer-pak-operation"
        ));
    }

    #[test]
    fn verified_matrix_v11_wire_binds_exact_closure_evidence() {
        let (manifest, reports) = fixture();
        let verified = verify_release_matrix(&manifest, &reports).unwrap();
        let baseline = verified_matrix_sha256(&verified);

        let mut changed = verified.clone();
        changed.scenarios[0].closure[0].observations += 1;
        assert_ne!(verified_matrix_sha256(&changed), baseline);

        let mut changed = verified;
        changed.scenarios[0].closure[0].name.push_str("-changed");
        assert_ne!(verified_matrix_sha256(&changed), baseline);
    }

    #[test]
    fn verified_matrix_v11_wire_binds_run_event_order() {
        let (manifest, reports) = fixture();
        let verified = verify_release_matrix(&manifest, &reports).unwrap();
        let baseline = verified_matrix_sha256(&verified);

        let mut reordered = verified;
        reordered.scenarios[0].run_event_sha256s.swap(0, 1);
        assert_ne!(verified_matrix_sha256(&reordered), baseline);
    }

    #[test]
    fn retained_matrix_independently_revalidates_coverage_and_manifest_identity() {
        let (manifest, reports) = fixture();
        let verified = verify_release_matrix(&manifest, &reports).unwrap();

        let mut missing_required = verified.clone();
        missing_required.scenarios[1].coverage.platforms = vec![ReleasePlatform::MacosArm64];
        missing_required.scenarios[1].declaration_sha256 =
            retained_scenario_declaration(&missing_required.scenarios[1])
                .recompute_declaration_sha256();
        missing_required.verification_sha256 = verified_matrix_sha256(&missing_required);
        assert!(matches!(
            missing_required.verify_integrity(),
            Err(ReleaseMatrixError::EnvironmentCoverageMismatch {
                dimension: "platforms",
                ..
            })
        ));

        let mut undeclared = verified.clone();
        undeclared.scenarios[0]
            .coverage
            .controllers
            .push(ControllerFeature::TransferPak);
        undeclared.verification_sha256 = verified_matrix_sha256(&undeclared);
        assert!(matches!(
            undeclared.verify_integrity(),
            Err(ReleaseMatrixError::UndeclaredCoverage {
                dimension: "controllers",
                ..
            })
        ));

        let mut cross_labeled_renderer = verified.clone();
        cross_labeled_renderer.scenarios[0].coverage.renderers =
            vec![RendererFeature::Rt64LleAccuracy];
        cross_labeled_renderer.scenarios[0].declaration_sha256 =
            retained_scenario_declaration(&cross_labeled_renderer.scenarios[0])
                .recompute_declaration_sha256();
        cross_labeled_renderer.verification_sha256 =
            verified_matrix_sha256(&cross_labeled_renderer);
        assert!(matches!(
            cross_labeled_renderer.verify_integrity(),
            Err(ReleaseMatrixError::EnvironmentCoverageMismatch {
                dimension: "renderers",
                ..
            })
        ));

        let mut relabeled = verified.clone();
        relabeled.scenarios[0].coverage.controllers = vec![ControllerFeature::RumblePak];
        relabeled.verification_sha256 = verified_matrix_sha256(&relabeled);
        assert!(matches!(
            relabeled.verify_integrity(),
            Err(ReleaseMatrixError::DeclarationDigestMismatch { .. })
        ));

        let mut wrong_manifest = verified;
        wrong_manifest.manifest_sha256 = "00".repeat(32);
        wrong_manifest.verification_sha256 = verified_matrix_sha256(&wrong_manifest);
        assert!(matches!(
            wrong_manifest.verify_integrity(),
            Err(ReleaseMatrixError::VerifiedManifestIdentityMismatch { .. })
        ));
    }

    #[test]
    fn rejects_matrix_series_with_unbound_v1_journal() {
        let (manifest, mut evidence) = fixture();
        evidence.get_mut("game-a-reference").unwrap()[4]
            .1
            .completion = crate::UnsupportedJournalCompletion::V1Unbound { guest_cycle: 100 };
        assert!(matches!(
            verify_release_matrix(&manifest, &evidence),
            Err(ReleaseMatrixError::InvalidSeries { .. })
        ));
    }

    #[test]
    fn rejects_run_event_identity_replayed_across_scenarios() {
        let (manifest, mut evidence) = fixture();
        let replayed = evidence["game-a-reference"][0]
            .1
            .release_run_event_sha256()
            .unwrap()
            .to_owned();
        let second = &mut evidence.get_mut("game-b-rt64").unwrap()[0].1.completion;
        let crate::UnsupportedJournalCompletion::V3RunBound {
            run_event_sha256, ..
        } = second
        else {
            unreachable!("fixture uses v3 run-bound journals")
        };
        *run_event_sha256 = replayed;
        assert!(matches!(
            verify_release_matrix(&manifest, &evidence),
            Err(ReleaseMatrixError::DuplicateRunEventIdentity { .. })
        ));
    }

    #[test]
    fn manifest_identity_is_independent_of_declaration_order() {
        let (manifest, _) = fixture();
        let expected = manifest.recompute_manifest_sha256();
        let mut reversed = manifest;
        reversed.scenarios.reverse();
        reversed.required.platforms.reverse();
        reversed.required.controllers.reverse();
        reversed.required.saves.reverse();
        reversed.required.renderers.reverse();
        reversed.required.programs.reverse();
        assert_eq!(reversed.recompute_manifest_sha256(), expected);
    }

    #[test]
    fn wire_feature_names_are_explicit_and_stable() {
        assert_eq!(
            serde_json::to_string(&ReleasePlatform::LinuxX86_64).unwrap(),
            "\"linux_x86_64\""
        );
        assert_eq!(
            serde_json::to_string(&SaveFeature::Eeprom4Kbit).unwrap(),
            "\"eeprom_4_kbit\""
        );
        assert_eq!(
            serde_json::to_string(&SaveFeature::FlashRam128Kib).unwrap(),
            "\"flash_ram_128_kib\""
        );
        assert_eq!(
            serde_json::to_string(&RendererFeature::Rt64PostViCapture).unwrap(),
            "\"rt64_post_vi_capture\""
        );
        assert_eq!(
            serde_json::to_string(&ProgramFeature::TypedObservedFunction).unwrap(),
            "\"typed_observed_function\""
        );
    }

    #[test]
    fn rejects_nine_or_eleven_reports() {
        let (manifest, mut reports) = fixture();
        reports.get_mut("game-a-reference").unwrap().pop();
        assert!(matches!(
            verify_release_matrix(&manifest, &reports),
            Err(ReleaseMatrixError::WrongReportCount { actual: 9, .. })
        ));

        let (manifest, mut reports) = fixture();
        let extra = reports["game-a-reference"][0].clone();
        reports.get_mut("game-a-reference").unwrap().push(extra);
        assert!(matches!(
            verify_release_matrix(&manifest, &reports),
            Err(ReleaseMatrixError::WrongReportCount { actual: 11, .. })
        ));
    }

    #[test]
    fn rejects_unassigned_required_coverage() {
        let (mut manifest, reports) = fixture();
        manifest
            .required
            .platforms
            .push(ReleasePlatform::WindowsX86_64);
        assert!(matches!(
            verify_release_matrix(&manifest, &reports),
            Err(ReleaseMatrixError::MissingRequiredCoverage {
                dimension: "platforms",
                ..
            })
        ));
    }

    #[test]
    fn rejects_scenario_coverage_outside_requirements() {
        let (mut manifest, reports) = fixture();
        manifest.scenarios[0].coverage.controllers = vec![ControllerFeature::TransferPak];
        assert!(matches!(
            verify_release_matrix(&manifest, &reports),
            Err(ReleaseMatrixError::UndeclaredCoverage {
                dimension: "controllers",
                ..
            })
        ));
    }

    #[test]
    fn rejects_wrong_declared_input_or_report_digest() {
        let (mut manifest, reports) = fixture();
        manifest.scenarios[0].input_sha256 = "0".repeat(64);
        manifest.scenarios[0].declaration_sha256 =
            manifest.scenarios[0].recompute_declaration_sha256();
        assert!(matches!(
            verify_release_matrix(&manifest, &reports),
            Err(ReleaseMatrixError::InputDigestMismatch { .. })
        ));

        let (mut manifest, reports) = fixture();
        manifest.scenarios[0].report_sha256 = "0".repeat(64);
        manifest.scenarios[0].declaration_sha256 =
            manifest.scenarios[0].recompute_declaration_sha256();
        assert!(matches!(
            verify_release_matrix(&manifest, &reports),
            Err(ReleaseMatrixError::ReportDigestMismatch { .. })
        ));
    }

    #[test]
    fn rejects_missing_or_unexpected_evidence() {
        let (manifest, mut reports) = fixture();
        reports.remove("game-a-reference");
        assert!(matches!(
            verify_release_matrix(&manifest, &reports),
            Err(ReleaseMatrixError::MissingEvidence { .. })
        ));

        let (manifest, mut reports) = fixture();
        reports.insert("undeclared".to_owned(), Vec::new());
        assert!(matches!(
            verify_release_matrix(&manifest, &reports),
            Err(ReleaseMatrixError::UnexpectedEvidence { .. })
        ));
    }

    #[test]
    fn propagates_v15_series_integrity_and_closure_failure() {
        let (manifest, mut reports) = fixture();
        reports.get_mut("game-a-reference").unwrap()[3]
            .0
            .scenario
            .push_str("-mutated");
        assert!(matches!(
            verify_release_matrix(&manifest, &reports),
            Err(ReleaseMatrixError::InvalidSeries { .. })
        ));
    }

    #[test]
    fn rejects_zero_count_or_inconsistent_live_path_evidence() {
        let (_manifest, reports) = fixture();
        let original = &reports["game-a-reference"][0].0;
        let mut closure = original.closure.clone();
        closure[0].observations = 0;
        assert!(matches!(
            ReleaseGateReport::new(
                original.scenario.clone(),
                b"private-a",
                original.digest.clone(),
                original.observations.clone(),
                closure,
            ),
            Err(crate::GateError::InvalidClosurePath { .. })
        ));
    }

    #[test]
    fn rejects_declared_save_without_matching_operation_path() {
        let (mut manifest, mut reports) = fixture();
        let original = &reports["game-a-reference"][0].0;
        let closure: Vec<_> = original
            .closure
            .iter()
            .filter(|path| path.name != "save.eeprom-4k-operation")
            .cloned()
            .collect();
        let replacement = ReleaseGateReport::new(
            original.scenario.clone(),
            b"private-a",
            original.digest.clone(),
            original.observations.clone(),
            closure,
        )
        .unwrap();
        reports.insert(
            "game-a-reference".to_owned(),
            evidence_series(replacement.clone()),
        );
        manifest.scenarios[0].report_sha256 = replacement.report_sha256;
        manifest.scenarios[0].declaration_sha256 =
            manifest.scenarios[0].recompute_declaration_sha256();

        assert!(matches!(
            verify_release_matrix(&manifest, &reports),
            Err(ReleaseMatrixError::MissingFeatureObservation { path, .. })
                if path == "save.eeprom-4k-operation"
        ));
    }

    #[test]
    fn rejects_save_observation_outside_exact_device_declaration() {
        for declared in [SaveFeature::NoCartridgeSave, SaveFeature::Eeprom16Kbit] {
            let (mut manifest, reports) = fixture();
            manifest.required.saves[0] = declared;
            manifest.scenarios[0].coverage.saves[0] = declared;
            manifest.scenarios[0].declaration_sha256 =
                manifest.scenarios[0].recompute_declaration_sha256();

            assert!(matches!(
                verify_release_matrix(&manifest, &reports),
                Err(ReleaseMatrixError::UnexpectedFeatureObservation { path, .. })
                    if path == "save.eeprom-4k-operation"
            ));
        }
    }

    #[test]
    fn controller_pak_requires_positive_pfs_operation() {
        let (mut manifest, reports) = fixture();
        manifest
            .required
            .controllers
            .push(ControllerFeature::ControllerPak);
        manifest.scenarios[0]
            .coverage
            .controllers
            .push(ControllerFeature::ControllerPak);
        manifest.scenarios[0].declaration_sha256 =
            manifest.scenarios[0].recompute_declaration_sha256();

        assert!(matches!(
            verify_release_matrix(&manifest, &reports),
            Err(ReleaseMatrixError::MissingFeatureObservation { path, .. })
                if path == "save.pfs-operation"
        ));
    }

    #[test]
    fn pfs_operation_is_rejected_without_controller_pak_declaration() {
        let (mut manifest, mut reports) = fixture();
        let original = &reports["game-a-reference"][0].0;
        let mut closure = original.closure.clone();
        closure.push(ClosurePath {
            name: "save.pfs-operation".to_owned(),
            observations: 1,
            status: ClosurePathStatus::ExercisedZeroUnsupported,
            unsupported: Vec::new(),
        });
        let replacement = ReleaseGateReport::new_with_test_environment(
            original.scenario.clone(),
            b"private-a",
            original.digest.clone(),
            original.observations.clone(),
            original.environment.clone(),
            closure,
        )
        .unwrap();
        reports.insert(
            "game-a-reference".to_owned(),
            evidence_series(replacement.clone()),
        );
        manifest.scenarios[0].report_sha256 = replacement.report_sha256;
        manifest.scenarios[0].declaration_sha256 =
            manifest.scenarios[0].recompute_declaration_sha256();

        assert!(matches!(
            verify_release_matrix(&manifest, &reports),
            Err(ReleaseMatrixError::UnexpectedFeatureObservation { path, .. })
                if path == "save.pfs-operation"
        ));
    }

    #[test]
    fn declared_controller_accessory_requires_matching_operation_path() {
        let (mut manifest, mut reports) = fixture();
        let original = &reports["game-b-rt64"][0].0;
        let closure = original
            .closure
            .iter()
            .filter(|path| path.name != "controller.rumble-operation")
            .cloned()
            .collect();
        let replacement = ReleaseGateReport::new_with_test_environment(
            original.scenario.clone(),
            b"private-b",
            original.digest.clone(),
            original.observations.clone(),
            original.environment.clone(),
            closure,
        )
        .unwrap();
        reports.insert(
            "game-b-rt64".to_owned(),
            evidence_series(replacement.clone()),
        );
        manifest.scenarios[1].report_sha256 = replacement.report_sha256;
        manifest.scenarios[1].declaration_sha256 =
            manifest.scenarios[1].recompute_declaration_sha256();

        assert!(matches!(
            verify_release_matrix(&manifest, &reports),
            Err(ReleaseMatrixError::MissingFeatureObservation { path, .. })
                if path == "controller.rumble-operation"
        ));
    }

    #[test]
    fn controller_operation_cannot_be_cross_labeled_as_another_configuration() {
        let (mut manifest, mut reports) = fixture();
        let original = &reports["game-a-reference"][0].0;
        let mut closure = original.closure.clone();
        closure.push(ClosurePath {
            name: "controller.transfer-pak-operation".to_owned(),
            observations: 1,
            status: ClosurePathStatus::ExercisedZeroUnsupported,
            unsupported: Vec::new(),
        });
        let replacement = ReleaseGateReport::new_with_test_environment(
            original.scenario.clone(),
            b"private-a",
            original.digest.clone(),
            original.observations.clone(),
            original.environment.clone(),
            closure,
        )
        .unwrap();
        reports.insert(
            "game-a-reference".to_owned(),
            evidence_series(replacement.clone()),
        );
        manifest.scenarios[0].report_sha256 = replacement.report_sha256;
        manifest.scenarios[0].declaration_sha256 =
            manifest.scenarios[0].recompute_declaration_sha256();

        assert!(matches!(
            verify_release_matrix(&manifest, &reports),
            Err(ReleaseMatrixError::UnexpectedFeatureObservation { path, .. })
                if path == "controller.transfer-pak-operation"
        ));
    }

    #[test]
    fn rejects_unbounded_or_ambiguous_manifest_shapes() {
        let (mut manifest, reports) = fixture();
        manifest.scenarios[0].id = "INVALID_ID".to_owned();
        assert!(matches!(
            verify_release_matrix(&manifest, &reports),
            Err(ReleaseMatrixError::InvalidScenarioId(_))
        ));

        let (mut manifest, reports) = fixture();
        manifest.scenarios[1].report_scenario = manifest.scenarios[0].report_scenario.clone();
        assert!(matches!(
            verify_release_matrix(&manifest, &reports),
            Err(ReleaseMatrixError::DuplicateReportScenario(_))
        ));
    }

    #[test]
    fn rejects_relabeling_without_a_new_declaration_digest() {
        let (mut manifest, reports) = fixture();
        manifest.scenarios[0].coverage.controllers = vec![ControllerFeature::RumblePak];
        assert!(matches!(
            verify_release_matrix(&manifest, &reports),
            Err(ReleaseMatrixError::DeclarationDigestMismatch { .. })
        ));
    }

    #[test]
    fn rejects_ambiguous_platform_save_and_renderer_selection() {
        let (mut manifest, reports) = fixture();
        manifest.scenarios[0]
            .coverage
            .platforms
            .push(ReleasePlatform::LinuxX86_64);
        manifest.scenarios[0].declaration_sha256 =
            manifest.scenarios[0].recompute_declaration_sha256();
        assert!(matches!(
            verify_release_matrix(&manifest, &reports),
            Err(ReleaseMatrixError::ExactOneCoverage {
                dimension: "platforms",
                ..
            })
        ));

        let (mut manifest, reports) = fixture();
        manifest.scenarios[0]
            .coverage
            .renderers
            .push(RendererFeature::Rt64LleAccuracy);
        manifest.scenarios[0].declaration_sha256 =
            manifest.scenarios[0].recompute_declaration_sha256();
        assert!(matches!(
            verify_release_matrix(&manifest, &reports),
            Err(ReleaseMatrixError::InvalidRendererCombination { .. })
        ));
    }
}
