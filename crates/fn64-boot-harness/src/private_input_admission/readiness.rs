#![allow(clippy::module_inception)]
use super::*;

pub(super) fn derive_readiness(
    manifest: &ValidatedManifest,
) -> Result<StoredReadiness, PrivateInputAdmissionError> {
    let mut roles = manifest.artifacts.keys().copied().collect::<Vec<_>>();
    roles.sort_unstable_by_key(|role| role.wire_name());
    let mut controllers = manifest.release.controllers.clone();
    controllers.sort_unstable_by_key(|controller| controller.wire_name());
    let mut renderers = manifest.release.renderers.clone();
    renderers.sort_unstable();
    let required_extended_cases = if manifest.purpose.requests_extended_gbi() {
        EXTENDED_CASES
            .iter()
            .map(|value| (*value).to_owned())
            .collect()
    } else {
        Vec::new()
    };
    let common = (
        "ready".to_owned(),
        manifest.purpose,
        manifest.wire_family,
        manifest.report_scenario.clone(),
        roles,
        if manifest.purpose.requests_extended_gbi() {
            "ready_for_runtime_recognition"
        } else {
            "not_requested"
        }
        .to_owned(),
        if manifest.artifacts.contains_key(&ArtifactRole::Rom)
            && manifest.artifacts.contains_key(&ArtifactRole::Recompiled)
        {
            "ready"
        } else {
            "not_supplied"
        }
        .to_owned(),
        "ready_for_ten_run_evidence".to_owned(),
        REPEAT_BAR,
        required_extended_cases,
        manifest.release.platform,
        controllers,
        manifest.release.save,
        renderers,
        manifest.program_lane,
        if manifest.program_lane.is_authoritative() {
            "verified"
        } else {
            "not_applicable"
        }
        .to_owned(),
        manifest.rom_class,
    );

    if manifest.schema == MANIFEST_SCHEMA {
        let characterization = manifest.purpose == Purpose::F3dzex2Characterization;
        Ok(StoredReadiness::V6(ReadinessV6 {
            status: common.0,
            purpose: common.1,
            wire_family: common.2,
            report_scenario: common.3,
            artifact_roles_admitted: common.4,
            extended_gbi_fixture: common.5,
            full_rom_inputs: common.6,
            release_matrix_policy: common.7,
            repeat_bar: common.8,
            required_extended_cases: common.9,
            platform: common.10,
            controllers: common.11,
            save: common.12,
            renderers: common.13,
            program_evidence_lane: common.14,
            program_build_receipt: common.15,
            rom_class: common.16,
            characterization_fixture: if characterization {
                "ready_for_controlled_native_evidence"
            } else {
                "not_requested"
            }
            .to_owned(),
            characterization_suite: if characterization {
                F3DZEX2_CHARACTERIZATION_SUITE
            } else {
                "not_requested"
            }
            .to_owned(),
            characterization_vector_source: if characterization {
                "repository_generated"
            } else {
                "not_requested"
            }
            .to_owned(),
            required_characterization_cases: if characterization {
                F3DZEX2_CHARACTERIZATION_CASES
                    .iter()
                    .map(|value| (*value).to_owned())
                    .collect()
            } else {
                Vec::new()
            },
        }))
    } else {
        Ok(StoredReadiness::V5(ReadinessV5 {
            status: common.0,
            purpose: common.1,
            wire_family: common.2,
            report_scenario: common.3,
            artifact_roles_admitted: common.4,
            extended_gbi_fixture: common.5,
            full_rom_inputs: common.6,
            release_matrix_policy: common.7,
            repeat_bar: common.8,
            required_extended_cases: common.9,
            platform: common.10,
            controllers: common.11,
            save: common.12,
            renderers: common.13,
            program_evidence_lane: common.14,
            program_build_receipt: common.15,
            rom_class: common.16,
        }))
    }
}

pub(super) fn validate_readiness(readiness: &StoredReadiness) -> Result<(), PrivateInputAdmissionError> {
    match readiness {
        StoredReadiness::V6(value) => validate_readiness_common(
            READINESS_SCHEMA,
            value.status.as_str(),
            value.purpose,
            value.wire_family,
            &value.report_scenario,
            &value.artifact_roles_admitted,
            &value.extended_gbi_fixture,
            &value.full_rom_inputs,
            &value.release_matrix_policy,
            value.repeat_bar,
            &value.required_extended_cases,
            value.platform,
            &value.controllers,
            value.save,
            &value.renderers,
            value.program_evidence_lane,
            &value.program_build_receipt,
            value.rom_class,
            Some((
                &value.characterization_fixture,
                &value.characterization_suite,
                &value.characterization_vector_source,
                &value.required_characterization_cases,
            )),
        ),
        StoredReadiness::V5(value) => validate_readiness_common(
            LEGACY_READINESS_SCHEMA,
            value.status.as_str(),
            value.purpose,
            value.wire_family,
            &value.report_scenario,
            &value.artifact_roles_admitted,
            &value.extended_gbi_fixture,
            &value.full_rom_inputs,
            &value.release_matrix_policy,
            value.repeat_bar,
            &value.required_extended_cases,
            value.platform,
            &value.controllers,
            value.save,
            &value.renderers,
            value.program_evidence_lane,
            &value.program_build_receipt,
            value.rom_class,
            None,
        ),
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) fn validate_readiness_common(
    schema: &str,
    status: &str,
    purpose: Purpose,
    wire_family: WireFamily,
    scenario: &str,
    roles: &[ArtifactRole],
    extended_fixture: &str,
    full_rom_inputs: &str,
    release_matrix_policy: &str,
    repeat_bar: u64,
    extended_cases: &[String],
    _platform: Platform,
    controllers: &[Controller],
    _save: SavePolicy,
    renderers: &[Renderer],
    program_lane: ProgramEvidenceLane,
    program_receipt: &str,
    rom_class: ManifestRomClass,
    characterization: Option<(&String, &String, &String, &Vec<String>)>,
) -> Result<(), PrivateInputAdmissionError> {
    if status != "ready"
        || release_matrix_policy != "ready_for_ten_run_evidence"
        || repeat_bar != REPEAT_BAR
    {
        return Err(error("readiness fixed policy fields are invalid"));
    }
    if schema == LEGACY_READINESS_SCHEMA
        && (purpose == Purpose::F3dzex2Characterization || wire_family == WireFamily::F3dzex2)
    {
        return Err(error(
            "retained v5 readiness cannot claim F3DZEX2 characterization",
        ));
    }
    validate_scenario(scenario, "readiness.report_scenario")?;
    validate_unique(roles, "readiness.artifact_roles_admitted")?;
    validate_unique(controllers, "readiness.controllers")?;
    validate_unique(renderers, "readiness.renderers")?;
    if controllers.is_empty() || renderers.is_empty() {
        return Err(error("readiness controllers/renderers must not be empty"));
    }
    let role_set = roles.iter().copied().collect::<BTreeSet<_>>();
    if purpose == Purpose::F3dzex2Characterization {
        if role_set
            != BTreeSet::from([
                ArtifactRole::MicrocodeDataRawWindow,
                ArtifactRole::MicrocodeTextRawWindow,
            ])
        {
            return Err(error(
                "readiness F3DZEX2 characterization roles are incomplete or ambiguous",
            ));
        }
    } else if !(role_set.contains(&ArtifactRole::MicrocodeData)
        && role_set.contains(&ArtifactRole::MicrocodeText))
        || role_set.contains(&ArtifactRole::MicrocodeDataRawWindow)
        || role_set.contains(&ArtifactRole::MicrocodeTextRawWindow)
    {
        return Err(error("readiness logical microcode roles are invalid"));
    }
    let renderer_set = renderers.iter().copied().collect::<BTreeSet<_>>();
    if renderer_set.contains(&Renderer::ReferenceLleAccuracy) {
        if renderer_set != BTreeSet::from([Renderer::ReferenceLleAccuracy]) {
            return Err(error("readiness reference LLE must stand alone"));
        }
    } else if !renderer_set.contains(&Renderer::Rt64LleAccuracy) {
        return Err(error("readiness RT64 policy lacks rt64_lle_accuracy"));
    }
    validate_unique_strings(extended_cases, "readiness.required_extended_cases")?;
    let extended_set = extended_cases
        .iter()
        .map(String::as_str)
        .collect::<BTreeSet<_>>();
    let expected_extended = EXTENDED_CASES.into_iter().collect::<BTreeSet<_>>();
    if purpose.requests_extended_gbi() {
        if extended_fixture != "ready_for_runtime_recognition"
            || extended_set != expected_extended
            || !(renderer_set.contains(&Renderer::Rt64LleAccuracy)
                && renderer_set.contains(&Renderer::Rt64PostViCapture))
        {
            return Err(error("readiness Extended GBI state is inconsistent"));
        }
    } else if extended_fixture != "not_requested" || !extended_set.is_empty() {
        return Err(error("readiness claims unrequested Extended GBI"));
    }
    if !matches!(full_rom_inputs, "ready" | "not_supplied") {
        return Err(error("readiness full_rom_inputs is invalid"));
    }
    if purpose.is_private_run() {
        if rom_class == ManifestRomClass::NotApplicable
            || full_rom_inputs != "ready"
            || !(role_set.contains(&ArtifactRole::Rom)
                && role_set.contains(&ArtifactRole::Recompiled))
            || program_receipt != "verified"
            || !program_lane.is_authoritative()
        {
            return Err(error("readiness full-ROM policy is incomplete"));
        }
    } else if rom_class != ManifestRomClass::NotApplicable
        || program_receipt != "not_applicable"
        || program_lane != ProgramEvidenceLane::NoProgramFixture
    {
        return Err(error("readiness fixture program policy is inconsistent"));
    }
    if purpose == Purpose::F3dzex2Characterization {
        let Some((fixture, suite, source, cases)) = characterization else {
            return Err(error(
                "F3DZEX2 characterization requires current readiness schema",
            ));
        };
        if fixture != "ready_for_controlled_native_evidence"
            || suite != F3DZEX2_CHARACTERIZATION_SUITE
            || source != "repository_generated"
            || cases.iter().map(String::as_str).collect::<Vec<_>>()
                != F3DZEX2_CHARACTERIZATION_CASES
        {
            return Err(error(
                "readiness F3DZEX2 characterization suite contract is incomplete",
            ));
        }
    } else if let Some((fixture, suite, source, cases)) = characterization {
        if fixture != "not_requested"
            || suite != "not_requested"
            || source != "not_requested"
            || !cases.is_empty()
        {
            return Err(error(
                "readiness claims unrequested F3DZEX2 characterization",
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
pub(super) fn build_private_run_contract(
    manifest: &ValidatedManifest,
    manifest_measurement: &StableFileMeasurement,
    readiness_path: &Path,
    readiness_bytes: &[u8],
) -> Result<PrivateReleaseRunContract, PrivateInputAdmissionError> {
    if !manifest.purpose.is_private_run() {
        return Err(error(
            "private run-contract emission requires purpose full_rom or combined",
        ));
    }
    let receipt = manifest
        .program_receipt
        .as_ref()
        .ok_or_else(|| error("private run contract requires a verified program-build receipt"))?;
    let rom = manifest
        .artifacts
        .get(&ArtifactRole::Rom)
        .ok_or_else(|| error("private run contract requires an admitted ROM input"))?;
    let mut artifacts = manifest
        .artifacts
        .iter()
        .filter(|(role, _)| **role != ArtifactRole::Rom)
        .map(|(role, artifact)| private_artifact_identity(*role, artifact))
        .collect::<Result<Vec<_>, _>>()?;
    artifacts.sort_unstable_by(|left, right| left.role.cmp(&right.role));
    let environment = manifest
        .runner
        .env
        .0
        .iter()
        .map(|(name, value)| PrivateEnvironmentEntry {
            name: name.clone(),
            value: value.clone(),
        })
        .collect();
    Ok(PrivateReleaseRunContract {
        schema: PRIVATE_RELEASE_RUN_CONTRACT_SCHEMA.to_owned(),
        admission_manifest: private_file_identity(manifest_measurement)?,
        readiness_report: PrivateFileIdentity {
            path: path_to_utf8(readiness_path, "readiness output")?,
            bytes: u64::try_from(readiness_bytes.len())
                .map_err(|_| error("readiness payload length exceeds u64"))?,
            sha256: sha256_hex(readiness_bytes),
        },
        program_build_receipt: Some(private_file_identity(receipt)?),
        purpose: manifest.purpose.wire_name().to_owned(),
        rom_class: manifest.rom_class.release_class()?,
        report_scenario: manifest.report_scenario.clone(),
        guest_cycle: manifest.runner.release_gate_cycle,
        repeat_count: usize::try_from(REPEAT_BAR).expect("repeat bar fits usize"),
        input: private_artifact_identity(ArtifactRole::Rom, rom)?,
        admitted_artifacts: artifacts,
        expected_execution_source: manifest.runner.execution_source.clone(),
        child: PrivateChildCommand {
            executable: private_file_identity(&manifest.executable)?,
            working_directory: manifest.runner.working_directory.clone(),
            argv: manifest.runner.argv.clone(),
            environment,
        },
        contract_sha256: String::new(),
    })
}

pub(super) fn cross_bind_contract(
    repository: &PrivateRepository,
    contract: &PrivateReleaseRunContract,
    manifest: &ValidatedManifest,
) -> Result<(), PrivateInputAdmissionError> {
    if contract.purpose != manifest.purpose.wire_name()
        || contract.rom_class != manifest.rom_class.release_class()?
        || contract.report_scenario != manifest.report_scenario
        || contract.guest_cycle != manifest.runner.release_gate_cycle
        || contract.repeat_count != usize::try_from(REPEAT_BAR).expect("repeat bar fits usize")
        || contract.expected_execution_source != manifest.runner.execution_source
    {
        return Err(error(
            "contract policy fields do not match the validated manifest",
        ));
    }
    if matches!(
        contract.expected_execution_source,
        ExecutionDestinationSource::NoProgram
    ) {
        return Err(error(
            "contract execution source does not match an authoritative manifest lane",
        ));
    }

    let receipt_identity = contract
        .program_build_receipt
        .as_ref()
        .ok_or_else(|| error("private run contract omits program_build_receipt"))?;
    let manifest_receipt = manifest
        .program_receipt
        .as_ref()
        .ok_or_else(|| error("validated manifest omits program_build_receipt"))?;
    require_private_identity_matches(
        receipt_identity,
        manifest_receipt,
        "contract.program_build_receipt",
    )?;

    let rom = manifest
        .artifacts
        .get(&ArtifactRole::Rom)
        .ok_or_else(|| error("validated manifest omits ROM"))?;
    require_contract_artifact_matches(&contract.input, ArtifactRole::Rom, rom, "contract.input")?;
    if contract.input.provenance
        != manifest
            .rom_class
            .expected_rom_provenance()
            .ok_or_else(|| error("private contract ROM class is invalid"))?
    {
        return Err(error(
            "private run contract ROM provenance does not match its class",
        ));
    }

    let expected_roles = manifest
        .artifacts
        .keys()
        .copied()
        .filter(|role| *role != ArtifactRole::Rom)
        .map(ArtifactRole::wire_name)
        .collect::<BTreeSet<_>>();
    let observed_roles = contract
        .admitted_artifacts
        .iter()
        .map(|artifact| artifact.role.as_str())
        .collect::<Vec<_>>();
    if !observed_roles.windows(2).all(|pair| pair[0] < pair[1])
        || observed_roles.iter().copied().collect::<BTreeSet<_>>() != expected_roles
    {
        return Err(error(
            "contract admitted artifact roles are not the exact sorted manifest roles",
        ));
    }
    for artifact in &contract.admitted_artifacts {
        let role = parse_artifact_role(&artifact.role)?;
        let expected = manifest.artifacts.get(&role).ok_or_else(|| {
            error(format!(
                "contract artifact {:?} is not admitted",
                artifact.role
            ))
        })?;
        require_contract_artifact_matches(
            artifact,
            role,
            expected,
            &format!("contract artifact {:?}", artifact.role),
        )?;
    }

    require_private_identity_matches(
        &contract.child.executable,
        &manifest.executable,
        "contract.child.executable",
    )?;
    let working = Path::new(&contract.child.working_directory);
    map_fs(
        check_directory_nofollow(working, "contract.child.working_directory"),
        "inspect contract.child.working_directory",
    )?;
    map_fs(
        repository.require_outside_or_gitignored(working, "contract.child.working_directory"),
        "exclude contract.child.working_directory from git",
    )?;
    let names = contract
        .child
        .environment
        .iter()
        .map(|entry| entry.name.as_str())
        .collect::<Vec<_>>();
    if !names.windows(2).all(|pair| pair[0] < pair[1]) {
        return Err(error(
            "contract child environment is not strictly sorted and unique",
        ));
    }
    let environment = contract
        .child
        .environment
        .iter()
        .map(|entry| (entry.name.clone(), entry.value.clone()))
        .collect::<BTreeMap<_, _>>();
    if contract.child.working_directory != manifest.runner.working_directory
        || contract.child.argv != manifest.runner.argv
        || environment != manifest.runner.env.0
    {
        return Err(error(
            "contract child policy does not match the manifest runner",
        ));
    }
    Ok(())
}

pub(super) fn read_bound_descriptor(
    repository: &PrivateRepository,
    identity: &PrivateFileIdentity,
    field: &str,
) -> Result<StableFileRead, PrivateInputAdmissionError> {
    validate_positive_length(identity.bytes, &format!("{field}.bytes"))?;
    require_sha256(&identity.sha256, &format!("{field}.sha256"))?;
    let read = read_private_file(repository, Path::new(&identity.path), field)?;
    require_measurement(&read.measurement, identity.bytes, &identity.sha256, field)?;
    Ok(read)
}

#[cfg(test)]
pub(super) fn validate_private_output_path(
    repository: &PrivateRepository,
    path: &Path,
    field: &str,
) -> Result<(), PrivateInputAdmissionError> {
    path_to_utf8(path, field)?;
    map_fs(
        validate_absolute_no_parent(path, field),
        &format!("validate {field}"),
    )?;
    map_fs(
        repository.require_outside_or_gitignored(path, field),
        &format!("exclude {field} from git"),
    )
}

pub(super) fn read_private_file(
    repository: &PrivateRepository,
    path: &Path,
    field: &str,
) -> Result<StableFileRead, PrivateInputAdmissionError> {
    path_to_utf8(path, field)?;
    map_fs(
        validate_absolute_no_parent(path, field),
        &format!("validate {field}"),
    )?;
    map_fs(
        repository.require_outside_or_gitignored(path, field),
        &format!("exclude {field} from git"),
    )?;
    map_fs(read_regular_stable(path, field), &format!("read {field}"))
}

pub(super) fn measure_private_file(
    repository: &PrivateRepository,
    path: &Path,
    field: &str,
) -> Result<StableFileMeasurement, PrivateInputAdmissionError> {
    path_to_utf8(path, field)?;
    map_fs(
        validate_absolute_no_parent(path, field),
        &format!("validate {field}"),
    )?;
    map_fs(
        repository.require_outside_or_gitignored(path, field),
        &format!("exclude {field} from git"),
    )?;
    map_fs(
        measure_regular_stable(path, field),
        &format!("measure {field}"),
    )
}

pub(super) fn require_measurement(
    measurement: &StableFileMeasurement,
    expected_bytes: u64,
    expected_sha256: &str,
    field: &str,
) -> Result<(), PrivateInputAdmissionError> {
    if measurement.bytes != expected_bytes || measurement.sha256 != expected_sha256 {
        return Err(error(format!(
            "{field} identity drift: expected bytes={expected_bytes} sha256={expected_sha256}, observed bytes={} sha256={}",
            measurement.bytes, measurement.sha256
        )));
    }
    Ok(())
}

pub(super) fn measure_private_executable(
    repository: &PrivateRepository,
    path: &Path,
    field: &str,
) -> Result<StableFileMeasurement, PrivateInputAdmissionError> {
    path_to_utf8(path, field)?;
    map_fs(
        validate_absolute_no_parent(path, field),
        &format!("validate {field}"),
    )?;
    map_fs(
        repository.require_outside_or_gitignored(path, field),
        &format!("exclude {field} from git"),
    )?;

    let mut prefix = [0u8; 64];
    let mut prefix_length = 0usize;
    let mut stream_offset = 0u64;
    let mut pe_offset = None;
    let mut pe_magic = [0u8; 4];
    let mut pe_magic_length = 0usize;
    let measurement = map_fs(
        measure_regular_stable_with(path, field, |event| {
            let StableFileStream::Chunk(chunk) = event else {
                return Ok(());
            };
            if prefix_length < prefix.len() {
                let count = (prefix.len() - prefix_length).min(chunk.len());
                prefix[prefix_length..prefix_length + count].copy_from_slice(&chunk[..count]);
                prefix_length += count;
                if prefix_length == prefix.len() && prefix.starts_with(b"MZ") {
                    pe_offset = Some(u64::from(u32::from_le_bytes(
                        prefix[0x3c..0x40]
                            .try_into()
                            .expect("fixed executable-prefix slice"),
                    )));
                    let target = usize::try_from(pe_offset.expect("PE offset was just set"))
                        .unwrap_or(usize::MAX);
                    if let Some(header) = prefix.get(target..target.saturating_add(4)) {
                        pe_magic.copy_from_slice(header);
                        pe_magic_length = 4;
                    }
                }
            }
            if let Some(target) = pe_offset {
                let chunk_end = stream_offset
                    .checked_add(u64::try_from(chunk.len()).expect("bounded chunk length fits u64"))
                    .ok_or_else(|| format!("{field} stream offset overflow"))?;
                let target_end = target.saturating_add(4);
                let overlap_start = target.max(stream_offset);
                let overlap_end = target_end.min(chunk_end);
                if overlap_start < overlap_end {
                    let source_start = usize::try_from(overlap_start - stream_offset)
                        .expect("overlap lies within bounded chunk");
                    let destination_start = usize::try_from(overlap_start - target)
                        .expect("four-byte destination offset fits usize");
                    let count = usize::try_from(overlap_end - overlap_start)
                        .expect("four-byte overlap fits usize");
                    pe_magic[destination_start..destination_start + count]
                        .copy_from_slice(&chunk[source_start..source_start + count]);
                    pe_magic_length = pe_magic_length.max(destination_start + count);
                }
            }
            stream_offset = stream_offset
                .checked_add(u64::try_from(chunk.len()).expect("bounded chunk length fits u64"))
                .ok_or_else(|| format!("{field} stream offset overflow"))?;
            Ok(())
        }),
        &format!("measure {field}"),
    )?;

    let magic = prefix.get(..prefix_length.min(4)).unwrap_or_default();
    let elf = magic == b"\x7fELF";
    let mach_o = magic.get(..4).is_some_and(|magic| {
        matches!(
            magic,
            [0xfe, 0xed, 0xfa, 0xce]
                | [0xce, 0xfa, 0xed, 0xfe]
                | [0xfe, 0xed, 0xfa, 0xcf]
                | [0xcf, 0xfa, 0xed, 0xfe]
                | [0xca, 0xfe, 0xba, 0xbe]
                | [0xbe, 0xba, 0xfe, 0xca]
                | [0xca, 0xfe, 0xba, 0xbf]
                | [0xbf, 0xba, 0xfe, 0xca]
        )
    });
    let pe = prefix.starts_with(b"MZ") && pe_magic_length == 4 && pe_magic == *b"PE\0\0";
    if !(elf || mach_o || pe) {
        return Err(error(format!(
            "{field} must be a native ELF, Mach-O, or PE image; scripts are forbidden"
        )));
    }
    #[cfg(unix)]
    if rustix::fs::access(path, rustix::fs::Access::EXEC_OK).is_err() {
        return Err(error(format!(
            "{field} has native image bytes but is not executable by the current process"
        )));
    }
    Ok(measurement)
}

pub(super) fn validate_executable_descriptor(
    descriptor: &ExecutableDescriptor,
    field: &str,
) -> Result<(), PrivateInputAdmissionError> {
    if descriptor.git_identity != "excluded" {
        return Err(error(format!("{field}.git_identity must be 'excluded'")));
    }
    validate_positive_length(descriptor.length, &format!("{field}.length"))?;
    require_sha256(&descriptor.sha256, &format!("{field}.sha256"))
}

pub(super) fn validate_positive_length(value: u64, field: &str) -> Result<(), PrivateInputAdmissionError> {
    if value == 0 || value > MAX_ARTIFACT_BYTES {
        return Err(error(format!(
            "{field} must be positive and at most {MAX_ARTIFACT_BYTES} bytes"
        )));
    }
    Ok(())
}

pub(super) fn validate_execution_source(
    source: &ExecutionDestinationSource,
    lane: ProgramEvidenceLane,
    field: &str,
) -> Result<(), PrivateInputAdmissionError> {
    let observed_kind = match source {
        ExecutionDestinationSource::NoProgram => "no_program",
        ExecutionDestinationSource::NativeArchive { artifact_sha256 }
        | ExecutionDestinationSource::TypedObservedFunctionProgram { artifact_sha256 } => {
            require_sha256(artifact_sha256, &format!("{field}.artifact_sha256"))?;
            match source {
                ExecutionDestinationSource::NativeArchive { .. } => "native_archive",
                _ => "typed_observed_function_program",
            }
        }
        ExecutionDestinationSource::TypedBlockProgram {
            program_sha256,
            dispatch_artifact_sha256,
        } => {
            require_sha256(program_sha256, &format!("{field}.program_sha256"))?;
            require_sha256(
                dispatch_artifact_sha256,
                &format!("{field}.dispatch_artifact_sha256"),
            )?;
            "typed_block_program"
        }
    };
    if observed_kind != lane.execution_kind() {
        return Err(error(format!(
            "{field}.kind {observed_kind:?} does not match program lane {lane:?}"
        )));
    }
    Ok(())
}

pub(super) fn validate_environment_name(name: &str) -> Result<(), PrivateInputAdmissionError> {
    let bytes = name.as_bytes();
    let valid = !bytes.is_empty()
        && (bytes[0].is_ascii_uppercase() || bytes[0] == b'_')
        && bytes[1..]
            .iter()
            .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit() || *byte == b'_');
    if !valid {
        return Err(error(format!("runner.env name {name:?} is invalid")));
    }
    if RESERVED_RUNNER_ENV.contains(&name)
        || name.starts_with("FN64_RELEASE_")
        || name.starts_with("FN64_PRIVATE_RUN_")
        || name.starts_with("OOT_RELEASE_")
    {
        return Err(error(format!(
            "runner.env name {name:?} is reserved for the trusted runner"
        )));
    }
    if FORBIDDEN_RUNNER_ENV.contains(&name)
        || FORBIDDEN_RUNNER_ENV_PREFIXES
            .iter()
            .any(|prefix| name.starts_with(prefix))
    {
        return Err(error(format!(
            "runner.env name {name:?} can inject or replace child process code"
        )));
    }
    Ok(())
}

pub(super) fn validate_scenario(value: &str, field: &str) -> Result<(), PrivateInputAdmissionError> {
    let bytes = value.as_bytes();
    let canonical = (1..=128).contains(&bytes.len())
        && (bytes[0].is_ascii_lowercase() || bytes[0].is_ascii_digit())
        && bytes.iter().all(|byte| {
            byte.is_ascii_lowercase()
                || byte.is_ascii_digit()
                || matches!(*byte, b'.' | b'_' | b'-')
        });
    let looks_like_sha256 = bytes.len() == 64
        && bytes
            .iter()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(byte));
    if !canonical || looks_like_sha256 {
        return Err(error(format!("{field} is invalid")));
    }
    Ok(())
}

pub(super) fn validate_unique<T: Ord + Copy>(
    values: &[T],
    field: &str,
) -> Result<(), PrivateInputAdmissionError> {
    if values.iter().copied().collect::<BTreeSet<_>>().len() != values.len() {
        return Err(error(format!("{field} contains duplicates")));
    }
    Ok(())
}

pub(super) fn validate_unique_strings(
    values: &[String],
    field: &str,
) -> Result<(), PrivateInputAdmissionError> {
    if values.iter().any(String::is_empty)
        || values
            .iter()
            .map(String::as_str)
            .collect::<BTreeSet<_>>()
            .len()
            != values.len()
    {
        return Err(error(format!(
            "{field} entries must be nonempty and unique"
        )));
    }
    Ok(())
}

pub(super) fn require_sha256(value: &str, field: &str) -> Result<(), PrivateInputAdmissionError> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(error(format!(
            "{field} must be a lowercase hexadecimal SHA-256"
        )));
    }
    Ok(())
}

#[cfg(test)]
pub(super) fn private_file_identity(
    measurement: &StableFileMeasurement,
) -> Result<PrivateFileIdentity, PrivateInputAdmissionError> {
    Ok(PrivateFileIdentity {
        path: path_to_utf8(&measurement.path, "private file identity")?,
        bytes: measurement.bytes,
        sha256: measurement.sha256.clone(),
    })
}

#[cfg(test)]
pub(super) fn private_artifact_identity(
    role: ArtifactRole,
    artifact: &AdmittedArtifact,
) -> Result<PrivateArtifactIdentity, PrivateInputAdmissionError> {
    Ok(PrivateArtifactIdentity {
        role: role.wire_name().to_owned(),
        path: path_to_utf8(&artifact.measurement.path, "private artifact identity")?,
        bytes: artifact.measurement.bytes,
        sha256: artifact.measurement.sha256.clone(),
        provenance: artifact.descriptor.provenance.clone(),
    })
}

pub(super) fn require_private_identity_matches(
    identity: &PrivateFileIdentity,
    measurement: &StableFileMeasurement,
    field: &str,
) -> Result<(), PrivateInputAdmissionError> {
    if !same_lexical_path(Path::new(&identity.path), &measurement.path)
        || identity.bytes != measurement.bytes
        || identity.sha256 != measurement.sha256
    {
        return Err(error(format!(
            "{field} does not match the validated manifest"
        )));
    }
    Ok(())
}

pub(super) fn require_contract_artifact_matches(
    identity: &PrivateArtifactIdentity,
    role: ArtifactRole,
    artifact: &AdmittedArtifact,
    field: &str,
) -> Result<(), PrivateInputAdmissionError> {
    if identity.role != role.wire_name()
        || !same_lexical_path(Path::new(&identity.path), &artifact.measurement.path)
        || identity.bytes != artifact.measurement.bytes
        || identity.sha256 != artifact.measurement.sha256
        || identity.provenance != artifact.descriptor.provenance
    {
        return Err(error(format!(
            "{field} does not match the admitted manifest descriptor"
        )));
    }
    Ok(())
}

pub(super) fn parse_artifact_role(value: &str) -> Result<ArtifactRole, PrivateInputAdmissionError> {
    match value {
        "microcode_data" => Ok(ArtifactRole::MicrocodeData),
        "microcode_data_raw_window" => Ok(ArtifactRole::MicrocodeDataRawWindow),
        "microcode_text" => Ok(ArtifactRole::MicrocodeText),
        "microcode_text_raw_window" => Ok(ArtifactRole::MicrocodeTextRawWindow),
        "recompiled" => Ok(ArtifactRole::Recompiled),
        "rom" => Ok(ArtifactRole::Rom),
        _ => Err(error(format!(
            "contract artifact role {value:?} is invalid"
        ))),
    }
}

pub(super) fn serialize_json_document<T: Serialize>(
    value: &T,
    field: &str,
) -> Result<Vec<u8>, PrivateInputAdmissionError> {
    let utf8 = serde_json::to_string_pretty(value)
        .map_err(|source| error(format!("serialize {field}: {source}")))?;
    // Python's retained canonical writer uses `json.dumps(..., indent=2)`
    // with its default `ensure_ascii=True`. Preserve that byte wire for paths
    // and environment values outside printable ASCII, including surrogate
    // pairs for non-BMP code points.
    let mut bytes = Vec::with_capacity(utf8.len());
    for character in utf8.chars() {
        if character <= '~' {
            let mut encoded = [0u8; 4];
            bytes.extend_from_slice(character.encode_utf8(&mut encoded).as_bytes());
        } else {
            let scalar = u32::from(character);
            if scalar <= 0xffff {
                bytes.extend_from_slice(format!("\\u{scalar:04x}").as_bytes());
            } else {
                let adjusted = scalar - 0x1_0000;
                let high = 0xd800 | (adjusted >> 10);
                let low = 0xdc00 | (adjusted & 0x3ff);
                bytes.extend_from_slice(format!("\\u{high:04x}\\u{low:04x}").as_bytes());
            }
        }
    }
    bytes.push(b'\n');
    Ok(bytes)
}

pub(super) fn path_to_utf8(path: &Path, field: &str) -> Result<String, PrivateInputAdmissionError> {
    path.to_str()
        .map(str::to_owned)
        .ok_or_else(|| error(format!("{field} path must be valid UTF-8")))
}

#[cfg(test)]
pub(super) fn sha256_hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let digest = Sha256::digest(bytes);
    let mut value = String::with_capacity(64);
    for byte in digest {
        value.push(char::from(DIGITS[usize::from(byte >> 4)]));
        value.push(char::from(DIGITS[usize::from(byte & 0x0f)]));
    }
    value
}
