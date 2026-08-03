#![allow(clippy::module_inception)]
use super::*;

pub(super) fn push_bytes(wire: &mut Vec<u8>, bytes: &[u8]) {
    wire.extend_from_slice(&(bytes.len() as u32).to_be_bytes());
    wire.extend_from_slice(bytes);
}

pub(super) fn push_tags<T: Copy>(wire: &mut Vec<u8>, values: &[T], tag: impl Fn(T) -> u8) {
    let mut tags: Vec<u8> = values.iter().copied().map(tag).collect();
    tags.sort_unstable();
    wire.extend_from_slice(&(tags.len() as u32).to_be_bytes());
    wire.extend_from_slice(&tags);
}

pub(super) fn validate_assignment_partition(
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

pub(super) fn push_assignment(wire: &mut Vec<u8>, assignment: &CertificationRequirementAssignment) {
    push_bytes(wire, assignment.requirement.class().as_str().as_bytes());
    push_bytes(wire, assignment.requirement.id().as_bytes());
    wire.extend_from_slice(&(assignment.evidence_sha256s.len() as u32).to_be_bytes());
    for digest in &assignment.evidence_sha256s {
        push_bytes(wire, digest.as_bytes());
    }
}

pub(super) fn push_platform_authority_identities(
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

pub(super) fn incomplete_matrix_sha256(report: &IncompleteReleaseMatrix) -> String {
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

pub(super) fn verified_matrix_sha256(report: &VerifiedReleaseMatrix) -> String {
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

pub(super) fn push_rom_class_authority(wire: &mut Vec<u8>, authority: &Option<VerifiedRomClassAuthority>) {
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

pub(super) const fn rom_class_tag(value: ReleaseRomClass) -> u8 {
    match value {
        ReleaseRomClass::Unclassified => 0,
        ReleaseRomClass::RetailCartridge => 1,
        ReleaseRomClass::PublicHomebrew => 2,
    }
}

pub(super) fn push_execution_source(wire: &mut Vec<u8>, source: &ExecutionDestinationSource) {
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

pub(super) fn validate_execution_source_identity(
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

pub(super) fn push_rom_evidence(wire: &mut Vec<u8>, rom: &Option<ReleaseRomEvidence>) {
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

pub(super) const fn tv_region_tag(value: ReleaseTvRegion) -> u8 {
    match value {
        ReleaseTvRegion::Ntsc => 0,
        ReleaseTvRegion::Pal => 1,
        ReleaseTvRegion::Mpal => 2,
        ReleaseTvRegion::RegionFree => 3,
    }
}

pub(super) fn push_observations(wire: &mut Vec<u8>, observations: &ReleaseObservationGeometry) {
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

pub(super) fn push_environment(wire: &mut Vec<u8>, environment: &ReleaseEnvironmentEvidence) {
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

pub(super) const fn release_execution_policy_tag(policy: ReleaseGraphicsExecutionPolicy) -> u8 {
    match policy {
        ReleaseGraphicsExecutionPolicy::HleOptimized => 0,
        ReleaseGraphicsExecutionPolicy::LleAccuracy => 1,
    }
}

pub(super) const fn release_graphics_api_tag(api: ReleaseGraphicsApi) -> u8 {
    match api {
        ReleaseGraphicsApi::D3d12 => 0,
        ReleaseGraphicsApi::Vulkan => 1,
        ReleaseGraphicsApi::Metal => 2,
    }
}

pub(super) fn hex(bytes: &[u8]) -> String {
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
