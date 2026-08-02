#![allow(clippy::module_inception)]
use super::*;

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

pub(super) fn collect_private_series_authorities(
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

pub(super) fn insert_private_series_authority(
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

pub(super) fn validate_private_series_authority_usage(
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

pub(super) fn collect_platform_series_authorities(
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

pub(super) fn verify_release_matrix_with_authorities(
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

pub(super) fn retained_scenario_declaration(scenario: &VerifiedMatrixScenario) -> ReleaseMatrixScenario {
    ReleaseMatrixScenario {
        id: scenario.id.clone(),
        report_scenario: scenario.report_scenario.clone(),
        input_sha256: scenario.input_sha256.clone(),
        report_sha256: scenario.report_sha256.clone(),
        declaration_sha256: scenario.declaration_sha256.clone(),
    }
}

pub(super) fn derive_profile_assignments(
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

pub(super) const fn rom_class_id(value: ReleaseRomClass) -> &'static str {
    match value {
        ReleaseRomClass::Unclassified => "unclassified",
        ReleaseRomClass::RetailCartridge => "retail_cartridge",
        ReleaseRomClass::PublicHomebrew => "public_homebrew",
    }
}

pub(super) fn verify_rom_class_authority_binding(
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

pub(super) const fn tv_region_id(value: ReleaseTvRegion) -> &'static str {
    match value {
        ReleaseTvRegion::Ntsc => "ntsc",
        ReleaseTvRegion::Pal => "pal",
        ReleaseTvRegion::Mpal => "mpal",
        ReleaseTvRegion::RegionFree => "region_free",
    }
}

pub(super) fn platform_api_target(environment: &ReleaseEnvironmentEvidence) -> Option<&'static str> {
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

pub(super) fn validate_platform_series_authority_usage(
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

pub(super) fn validate_platform_series_authority_usage_for_retained(
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

pub(super) fn platform_authority_matches_scenario(
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

pub(super) fn insert_requirement_evidence(
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

pub(super) const fn program_feature_id(value: ProgramFeature) -> &'static str {
    match value {
        ProgramFeature::NativeArchive => "native_archive",
        ProgramFeature::TypedObservedFunction => "typed_observed_function",
        ProgramFeature::TypedBlock => "typed_block",
    }
}

pub(super) const fn save_feature_id(value: SaveFeature) -> &'static str {
    match value {
        SaveFeature::NoCartridgeSave => "no_cartridge_save",
        SaveFeature::Eeprom4Kbit => "eeprom_4_kbit",
        SaveFeature::Eeprom16Kbit => "eeprom_16_kbit",
        SaveFeature::Sram32Kib => "sram_32_kib",
        SaveFeature::FlashRam128Kib => "flash_ram_128_kib",
    }
}

pub(super) const fn controller_feature_id(value: ControllerFeature) -> &'static str {
    match value {
        ControllerFeature::StandardController => "standard_controller",
        ControllerFeature::ControllerPak => "controller_pak",
        ControllerFeature::RumblePak => "rumble_pak",
        ControllerFeature::TransferPak => "transfer_pak",
        ControllerFeature::VoiceRecognitionUnit => "voice_recognition_unit",
    }
}

pub(super) const fn microcode_feature_id(value: MicrocodeFeature) -> &'static str {
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

pub(super) const fn rsp_rdp_mechanism_feature_id(value: RspRdpMechanismFeature) -> &'static str {
    match value {
        RspRdpMechanismFeature::DramDpc => "dram-dpc",
        RspRdpMechanismFeature::XbusDpc => "xbus-dpc",
        RspRdpMechanismFeature::ImemReplacement => "imem-replacement",
    }
}

pub(super) fn presentation_boundary(
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

pub(super) fn derive_scenario_coverage(
    id: &str,
    report: &ReleaseGateReport,
) -> Result<ReleaseMatrixCoverage, ReleaseMatrixError> {
    derive_scenario_coverage_with_catalog(id, report, CERTIFIED_PUBLIC_MICROCODE_CATALOG_V1)
}

pub(super) fn derive_scenario_coverage_with_catalog(
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

pub(super) fn certified_microcode_feature(
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

pub(super) const fn microcode_feature(family: ReleaseMicrocodeFamily) -> Option<MicrocodeFeature> {
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

pub(super) fn validate_retained_closure(id: &str, closure: &[ClosurePath]) -> Result<(), ReleaseMatrixError> {
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

pub(super) fn validate_feature_operation_paths(
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

pub(super) fn require_positive_path(
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

pub(super) fn validate_manifest(manifest: &ReleaseMatrixManifest) -> Result<FullParityV1, ReleaseMatrixError> {
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

pub(super) fn validate_coverage_cardinality(
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

pub(super) fn validate_id(id: &str) -> Result<(), ReleaseMatrixError> {
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

pub(super) fn validate_sha256(id: &str, field: &'static str, value: &str) -> Result<(), ReleaseMatrixError> {
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

pub(super) fn validate_coverage(
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

pub(super) fn validate_optional_dimension<T: Copy + fmt::Debug + Ord>(
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

pub(super) fn validate_dimension<T: Copy + fmt::Debug + Ord>(
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
    pub(super) const fn tag(self) -> u8 {
        match self {
            Self::MacosArm64 => 0,
            Self::LinuxX86_64 => 1,
            Self::WindowsX86_64 => 2,
        }
    }
}

impl ControllerFeature {
    pub(super) const fn tag(self) -> u8 {
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
    pub(super) const fn tag(self) -> u8 {
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
    pub(super) const fn tag(self) -> u8 {
        match self {
            Self::ReferenceLleAccuracy => 0,
            Self::Rt64LleAccuracy => 1,
            Self::Rt64PostViCapture => 2,
            Self::Rt64ReplacementPacks => 3,
        }
    }
}

impl ProgramFeature {
    pub(super) const fn tag(self) -> u8 {
        match self {
            Self::NativeArchive => 0,
            Self::TypedObservedFunction => 1,
            Self::TypedBlock => 2,
        }
    }
}

impl MicrocodeFeature {
    pub(super) const fn tag(self) -> u8 {
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
    pub(super) const fn tag(self) -> u8 {
        match self {
            Self::DramDpc => 0,
            Self::XbusDpc => 1,
            Self::ImemReplacement => 2,
        }
    }
}
