#![allow(clippy::module_inception)]
use super::*;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PrivateReleaseSeriesRun {
    pub ordinal: u64,
    pub run_event_sha256: String,
    pub report_file_sha256: String,
    pub journal_file_sha256: String,
    pub report_sha256: String,
    pub artifact_root_sha256: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
/// Canonical integrity record for a completed series. This self-hashed value
/// is not a signature or an operating-system process attestation.
pub struct PrivateReleaseSeriesReceipt {
    pub schema: String,
    pub contract_sha256: String,
    pub runner_executable_sha256: String,
    pub child_executable_sha256: String,
    pub series_nonce: String,
    pub report_scenario: String,
    pub input_sha256: String,
    pub guest_cycle: u64,
    pub expected_execution_source: ExecutionDestinationSource,
    pub count: usize,
    pub semantic_report_sha256: String,
    pub runs: Vec<PrivateReleaseSeriesRun>,
    pub receipt_sha256: String,
}

impl PrivateReleaseSeriesReceipt {
    pub fn recompute_receipt_sha256(&self) -> Result<String, PrivateReleaseSeriesError> {
        let mut wire = Vec::new();
        wire.extend_from_slice(RECEIPT_DIGEST_DOMAIN);
        push_bytes(&mut wire, self.schema.as_bytes());
        push_hash(&mut wire, &self.contract_sha256, "contract_sha256")?;
        push_hash(
            &mut wire,
            &self.runner_executable_sha256,
            "runner_executable_sha256",
        )?;
        push_hash(
            &mut wire,
            &self.child_executable_sha256,
            "child_executable_sha256",
        )?;
        push_hash(&mut wire, &self.series_nonce, "series_nonce")?;
        push_bytes(&mut wire, self.report_scenario.as_bytes());
        push_hash(&mut wire, &self.input_sha256, "input_sha256")?;
        push_u64(&mut wire, self.guest_cycle);
        encode_execution_source(&mut wire, &self.expected_execution_source)?;
        push_u64(
            &mut wire,
            u64::try_from(self.count)
                .map_err(|_| error("receipt count exceeds the canonical wire"))?,
        );
        push_hash(
            &mut wire,
            &self.semantic_report_sha256,
            "semantic_report_sha256",
        )?;
        push_u64(
            &mut wire,
            u64::try_from(self.runs.len())
                .map_err(|_| error("receipt run count exceeds the canonical wire"))?,
        );
        for run in &self.runs {
            push_u64(&mut wire, run.ordinal);
            push_hash(&mut wire, &run.run_event_sha256, "runs[].run_event_sha256")?;
            push_hash(
                &mut wire,
                &run.report_file_sha256,
                "runs[].report_file_sha256",
            )?;
            push_hash(
                &mut wire,
                &run.journal_file_sha256,
                "runs[].journal_file_sha256",
            )?;
            push_hash(&mut wire, &run.report_sha256, "runs[].report_sha256")?;
            push_hash(
                &mut wire,
                &run.artifact_root_sha256,
                "runs[].artifact_root_sha256",
            )?;
        }
        Ok(sha256_hex(&wire))
    }

    pub fn verify_integrity(&self) -> Result<(), PrivateReleaseSeriesError> {
        if self.schema != PRIVATE_RELEASE_SERIES_RECEIPT_SCHEMA {
            return Err(error(format!(
                "unsupported private release series receipt schema {:?}",
                self.schema
            )));
        }
        if self.count != PRIVATE_RELEASE_SERIES_COUNT
            || self.runs.len() != PRIVATE_RELEASE_SERIES_COUNT
        {
            return Err(error(format!(
                "private release receipt retains count={} and {} runs; exactly {} are required",
                self.count,
                self.runs.len(),
                PRIVATE_RELEASE_SERIES_COUNT
            )));
        }
        validate_scenario(&self.report_scenario)?;
        validate_execution_source(&self.expected_execution_source)?;
        for (field, value) in [
            ("contract_sha256", &self.contract_sha256),
            ("runner_executable_sha256", &self.runner_executable_sha256),
            ("child_executable_sha256", &self.child_executable_sha256),
            ("series_nonce", &self.series_nonce),
            ("input_sha256", &self.input_sha256),
            ("semantic_report_sha256", &self.semantic_report_sha256),
            ("receipt_sha256", &self.receipt_sha256),
        ] {
            require_sha256(value, field)?;
        }
        let mut events = BTreeSet::new();
        for (index, run) in self.runs.iter().enumerate() {
            let expected = u64::try_from(index + 1).expect("ten runs fit u64");
            if run.ordinal != expected {
                return Err(error(format!(
                    "private release receipt run at index {index} has ordinal {}; expected {expected}",
                    run.ordinal
                )));
            }
            for (field, value) in [
                ("runs[].run_event_sha256", &run.run_event_sha256),
                ("runs[].report_file_sha256", &run.report_file_sha256),
                ("runs[].journal_file_sha256", &run.journal_file_sha256),
                ("runs[].report_sha256", &run.report_sha256),
                ("runs[].artifact_root_sha256", &run.artifact_root_sha256),
            ] {
                require_sha256(value, field)?;
            }
            if !events.insert(&run.run_event_sha256) {
                return Err(error(format!(
                    "private release receipt repeats run-event identity {}",
                    run.run_event_sha256
                )));
            }
            if run.report_sha256 != self.semantic_report_sha256 {
                return Err(error(format!(
                    "receipt run {} report SHA {} differs from series SHA {}",
                    run.ordinal, run.report_sha256, self.semantic_report_sha256
                )));
            }
        }
        let recomputed = self.recompute_receipt_sha256()?;
        if recomputed != self.receipt_sha256 {
            return Err(error(format!(
                "private release receipt digest mismatch: stored {}, recomputed {recomputed}",
                self.receipt_sha256
            )));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PrivateReleaseSeriesError(pub(super) String);

impl fmt::Display for PrivateReleaseSeriesError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for PrivateReleaseSeriesError {}

pub(super) struct StagedChildExecutable(pub(super) PathBuf);

impl Drop for StagedChildExecutable {
    fn drop(&mut self) {
        make_removable(&self.0);
        let _ = fs::remove_file(&self.0);
    }
}

pub(super) struct StagedPrivateArtifact(pub(super) PathBuf);

impl Drop for StagedPrivateArtifact {
    fn drop(&mut self) {
        make_removable(&self.0);
        let _ = fs::remove_file(&self.0);
    }
}

pub(super) struct StagedMicrocodePair {
    pub(super) text: StagedPrivateArtifact,
    pub(super) data: StagedPrivateArtifact,
    pub(super) text_identity: PrivateFileIdentity,
    pub(super) data_identity: PrivateFileIdentity,
}

impl StagedMicrocodePair {
    pub(super) fn verify(&self) -> Result<(), PrivateReleaseSeriesError> {
        verify_file_identity(&self.text_identity, "staged microcode text", false)?;
        verify_file_identity(&self.data_identity, "staged microcode data", false)
    }
}

#[cfg(windows)]
pub(super) fn make_removable(path: &Path) {
    if let Ok(metadata) = fs::metadata(path) {
        let mut permissions = metadata.permissions();
        if permissions.readonly() {
            permissions.set_readonly(false);
            let _ = fs::set_permissions(path, permissions);
        }
    }
}

#[cfg(not(windows))]
pub(super) fn make_removable(_path: &Path) {}

pub(super) fn open_private_stage(path: &Path) -> std::io::Result<File> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.mode(0o600);
    }
    options.open(path)
}

pub(super) fn seal_private_stage(path: &Path, executable: bool) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let mode = if executable { 0o500 } else { 0o400 };
        fs::set_permissions(path, fs::Permissions::from_mode(mode))
    }
    #[cfg(not(unix))]
    {
        let _ = executable;
        let mut permissions = fs::metadata(path)?.permissions();
        permissions.set_readonly(true);
        fs::set_permissions(path, permissions)
    }
}

pub(super) fn stage_child_executable(
    identity: &PrivateFileIdentity,
) -> Result<StagedChildExecutable, PrivateReleaseSeriesError> {
    let source = Path::new(&identity.path);
    let parent = source
        .parent()
        .ok_or_else(|| error("child executable has no parent directory"))?;
    for _ in 0..32 {
        let mut random = [0u8; 16];
        getrandom::fill(&mut random)
            .map_err(|source| error(format!("obtain child-stage nonce: {source}")))?;
        let extension = source
            .extension()
            .and_then(|value| value.to_str())
            .map(|value| format!(".{value}"))
            .unwrap_or_default();
        let path = parent.join(format!(".fn64-release-child-{}{}", hex(&random), extension));
        match open_private_stage(&path) {
            Ok(mut destination) => {
                let staged = StagedChildExecutable(path.clone());
                let mut source_file = File::open(source).map_err(|source| {
                    error(format!(
                        "open child executable {} for exact staging: {source}",
                        identity.path
                    ))
                })?;
                std::io::copy(&mut source_file, &mut destination)
                    .and_then(|_| destination.flush())
                    .and_then(|()| destination.sync_all())
                    .map_err(|source| {
                        error(format!(
                            "persist exact staged child executable {}: {source}",
                            path.display()
                        ))
                    })?;
                seal_private_stage(&path, true).map_err(|source| {
                    error(format!(
                        "seal staged child executable owner-only {}: {source}",
                        path.display()
                    ))
                })?;
                let staged_identity = PrivateFileIdentity {
                    path: path
                        .to_str()
                        .ok_or_else(|| error("staged child executable path is not UTF-8"))?
                        .to_owned(),
                    bytes: identity.bytes,
                    sha256: identity.sha256.clone(),
                };
                verify_file_identity(&staged_identity, "staged child executable", false)?;
                validate_native_executable(&path)?;
                return Ok(staged);
            }
            Err(source) if source.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(source) => {
                return Err(error(format!(
                    "create exact child-executable stage beside {}: {source}",
                    identity.path
                )))
            }
        }
    }
    Err(error(format!(
        "could not allocate an exact staged child executable beside {}",
        identity.path
    )))
}

pub(super) fn stage_private_artifact(
    identity: &PrivateArtifactIdentity,
) -> Result<(StagedPrivateArtifact, PrivateFileIdentity), PrivateReleaseSeriesError> {
    let source = Path::new(&identity.path);
    let parent = source.parent().ok_or_else(|| {
        error(format!(
            "private artifact {:?} has no parent directory",
            identity.role
        ))
    })?;
    for _ in 0..32 {
        let mut random = [0u8; 16];
        getrandom::fill(&mut random)
            .map_err(|source| error(format!("obtain artifact-stage nonce: {source}")))?;
        let path = parent.join(format!(".fn64-release-{}-{}", identity.role, hex(&random)));
        match open_private_stage(&path) {
            Ok(mut destination) => {
                let staged = StagedPrivateArtifact(path.clone());
                let mut source_file = File::open(source).map_err(|source| {
                    error(format!(
                        "open private artifact {:?} for exact staging: {source}",
                        identity.role
                    ))
                })?;
                std::io::copy(&mut source_file, &mut destination)
                    .and_then(|_| destination.flush())
                    .and_then(|()| destination.sync_all())
                    .map_err(|source| {
                        error(format!(
                            "persist exact staged private artifact {:?}: {source}",
                            identity.role
                        ))
                    })?;
                seal_private_stage(&path, false).map_err(|source| {
                    error(format!(
                        "seal staged private artifact {:?} owner-only: {source}",
                        identity.role
                    ))
                })?;
                let staged_identity = PrivateFileIdentity {
                    path: path
                        .to_str()
                        .ok_or_else(|| error("staged private artifact path is not UTF-8"))?
                        .to_owned(),
                    bytes: identity.bytes,
                    sha256: identity.sha256.clone(),
                };
                verify_file_identity(&staged_identity, "staged private artifact", false)?;
                return Ok((staged, staged_identity));
            }
            Err(source) if source.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(source) => {
                return Err(error(format!(
                    "create exact private-artifact stage beside {:?}: {source}",
                    identity.role
                )))
            }
        }
    }
    Err(error(format!(
        "could not allocate an exact staged private artifact for {:?}",
        identity.role
    )))
}

pub(super) fn stage_microcode_pair(
    contract: &PrivateReleaseRunContract,
) -> Result<Option<StagedMicrocodePair>, PrivateReleaseSeriesError> {
    if contract.purpose == "synthetic_mechanism" {
        return Ok(None);
    }
    let (text, text_identity) =
        stage_private_artifact(required_artifact(contract, "microcode_text")?)?;
    let (data, data_identity) =
        stage_private_artifact(required_artifact(contract, "microcode_data")?)?;
    let pair = StagedMicrocodePair {
        text,
        data,
        text_identity,
        data_identity,
    };
    pair.verify()?;
    Ok(Some(pair))
}

pub(super) fn same_program_file_identity(
    build: &ReleaseProgramFileIdentity,
    contract_path: &str,
    contract_bytes: u64,
    contract_sha256: &str,
) -> bool {
    build.path == contract_path && build.bytes == contract_bytes && build.sha256 == contract_sha256
}

pub(super) fn required_artifact<'a>(
    contract: &'a PrivateReleaseRunContract,
    role: &str,
) -> Result<&'a PrivateArtifactIdentity, PrivateReleaseSeriesError> {
    contract
        .admitted_artifacts
        .iter()
        .find(|artifact| artifact.role == role)
        .ok_or_else(|| {
            error(format!(
                "private release contract omits required artifact {role:?}"
            ))
        })
}

pub(super) fn verify_release_program_build_receipt_binding(
    contract: &PrivateReleaseRunContract,
) -> Result<Option<VerifiedReleaseProgramBuildReceipt>, PrivateReleaseSeriesError> {
    match contract.purpose.as_str() {
        "full_rom" | "combined" => {
            let identity = contract.program_build_receipt.as_ref().ok_or_else(|| {
                error(format!(
                    "private {} execution requires a {RELEASE_PROGRAM_BUILD_RECEIPT_SCHEMA}",
                    contract.purpose
                ))
            })?;
            verify_private_file_identity(identity, "program_build_receipt")?;
            let verified =
                load_release_program_build_receipt(&identity.path).map_err(|source| {
                    error(format!(
                        "verify bound {RELEASE_PROGRAM_BUILD_RECEIPT_SCHEMA}: {source}"
                    ))
                })?;
            verify_private_file_identity(identity, "program_build_receipt")?;
            if !same_program_file_identity(
                &verified.receipt.child_executable,
                &contract.child.executable.path,
                contract.child.executable.bytes,
                &contract.child.executable.sha256,
            ) {
                return Err(error(
                    "program-build receipt child executable does not match the private run contract",
                ));
            }
            if verified.receipt.expected_execution_source != contract.expected_execution_source
                || verified.recomputed_execution_source != contract.expected_execution_source
            {
                return Err(error(
                    "program-build receipt execution source does not match the private run contract",
                ));
            }
            let recompiled = required_artifact(contract, "recompiled")?;
            let matching_inputs = match &verified.receipt.lane {
                ReleaseProgramBuildLane::NativeArchives { archives } => archives
                    .iter()
                    .filter(|archive| {
                        same_program_file_identity(
                            &archive.file,
                            &recompiled.path,
                            recompiled.bytes,
                            &recompiled.sha256,
                        )
                    })
                    .count(),
                ReleaseProgramBuildLane::TypedObservedFunction { identity_wire } => {
                    usize::from(same_program_file_identity(
                        identity_wire,
                        &recompiled.path,
                        recompiled.bytes,
                        &recompiled.sha256,
                    ))
                }
                ReleaseProgramBuildLane::TypedBlock { pack, .. } => {
                    usize::from(same_program_file_identity(
                        pack,
                        &recompiled.path,
                        recompiled.bytes,
                        &recompiled.sha256,
                    ))
                }
            };
            if matching_inputs != 1 {
                return Err(error(format!(
                    "program-build receipt binds {matching_inputs} exact lane inputs matching the admitted recompiled artifact; exactly one is required"
                )));
            }
            Ok(Some(verified))
        }
        "synthetic_mechanism" => {
            if contract.program_build_receipt.is_some() {
                return Err(error(
                    "synthetic mechanism contract cannot bind a production program-build receipt",
                ));
            }
            Ok(None)
        }
        _ => Err(error("private release contract purpose was not validated")),
    }
}

pub fn load_private_release_run_contract(
    path: impl AsRef<Path>,
) -> Result<VerifiedPrivateReleaseRunContract, PrivateReleaseSeriesError> {
    let admitted =
        crate::private_input_admission::verify_retained_private_run_contract(path.as_ref())
            .map_err(|source| {
                error(format!(
                    "in-process private input admission rejected {}: {source}",
                    path.as_ref().display()
                ))
            })?;
    let contract = admitted.into_contract();
    verify_release_program_build_receipt_binding(&contract)?;
    Ok(VerifiedPrivateReleaseRunContract { contract })
}

pub(super) fn identity_matches_bytes(identity: &PrivateFileIdentity, bytes: &[u8]) -> bool {
    identity.bytes == u64::try_from(bytes.len()).expect("fixture length fits u64")
        && identity.sha256 == sha256_hex(bytes)
}

/// Admit only fn64's fixed, non-game synthetic mechanism fixture without the
/// content-bearing production path. Arbitrary caller-labelled synthetic inputs do
/// not create runner authority.
pub fn verify_repository_synthetic_private_release_run_contract(
    contract: PrivateReleaseRunContract,
) -> Result<VerifiedPrivateReleaseRunContract, PrivateReleaseSeriesError> {
    verify_repository_synthetic_contract_common(&contract)?;
    if contract.report_scenario != REPOSITORY_SYNTHETIC_RELEASE_SCENARIO
        || !contract.admitted_artifacts.is_empty()
        || contract.expected_execution_source != ExecutionDestinationSource::NoProgram
    {
        return Err(error(
            "no-program synthetic runner authority requires the exact repository no-program scenario",
        ));
    }
    contract.verify_bound_files()?;
    Ok(VerifiedPrivateReleaseRunContract { contract })
}

/// Validate, execute, and reverify one caller-bound identified-native
/// synthetic series.
///
/// The archive and child values are caller-supplied consistency bounds, not
/// repository acceptance anchors. The verified contract remains inside this
/// operation so this path cannot mint the general production-runner
/// capability. A platform-specific gate separately compares the resulting
/// reports with an exact checked-in semantic fingerprint.
pub fn run_synthetic_native_private_release_series(
    contract: PrivateReleaseRunContract,
    caller_bound_archives: [PrivateArtifactIdentity; 2],
    caller_bound_child: PrivateChildCommand,
    output_directory: impl AsRef<Path>,
) -> Result<PrivateReleaseSeriesReceipt, PrivateReleaseSeriesError> {
    let verified_contract = verify_synthetic_native_private_release_run_contract(
        contract,
        caller_bound_archives,
        caller_bound_child,
    )?;
    let receipt = run_private_release_series(&verified_contract, &output_directory)?;
    verify_private_release_series(&verified_contract, output_directory, &receipt)?;
    Ok(receipt)
}

pub(super) fn verify_synthetic_native_private_release_run_contract(
    contract: PrivateReleaseRunContract,
    caller_bound_archives: [PrivateArtifactIdentity; 2],
    caller_bound_child: PrivateChildCommand,
) -> Result<VerifiedPrivateReleaseRunContract, PrivateReleaseSeriesError> {
    verify_repository_synthetic_contract_common(&contract)?;
    if contract.report_scenario != REPOSITORY_SYNTHETIC_NATIVE_RELEASE_SCENARIO
        || contract.admitted_artifacts != caller_bound_archives
        || contract.child != caller_bound_child
    {
        return Err(error(
            "identified-native synthetic series requires the exact caller-bound archives and child invocation",
        ));
    }
    let roles = contract
        .admitted_artifacts
        .iter()
        .map(|artifact| artifact.role.as_str())
        .collect::<Vec<_>>();
    if roles
        != [
            "synthetic_generated_archive",
            "synthetic_section_bridge_archive",
        ]
        || contract
            .admitted_artifacts
            .iter()
            .any(|artifact| artifact.provenance != "repository_defined_synthetic")
    {
        return Err(error(
            "identified-native synthetic series requires the exact generated-code and section-bridge archive roles",
        ));
    }
    contract.verify_bound_files()?;
    let ExecutionDestinationSource::NativeArchive { artifact_sha256 } =
        &contract.expected_execution_source
    else {
        return Err(error(
            "identified-native synthetic series requires the native-archive execution source",
        ));
    };
    let archives = contract
        .admitted_artifacts
        .iter()
        .enumerate()
        .map(|(index, artifact)| {
            fs::read(&artifact.path)
                .map(|bytes| {
                    let label = match index {
                        0 => "synthetic-generated-code",
                        1 => "synthetic-section-bridge",
                        _ => unreachable!("archive roles were checked above"),
                    };
                    (label.to_owned(), bytes)
                })
                .map_err(|source| {
                    error(format!(
                        "read identified-native synthetic archive {}: {source}",
                        artifact.path
                    ))
                })
        })
        .collect::<Result<Vec<_>, _>>()?;
    let observed = hex(&crate::native_program_archives_sha256(archives));
    if &observed != artifact_sha256 {
        return Err(error(format!(
            "identified-native synthetic archive identity mismatch: contract={artifact_sha256}, observed={observed}"
        )));
    }
    contract.verify_bound_files()?;
    Ok(VerifiedPrivateReleaseRunContract { contract })
}

pub(super) fn verify_repository_synthetic_contract_common(
    contract: &PrivateReleaseRunContract,
) -> Result<(), PrivateReleaseSeriesError> {
    contract.verify_integrity()?;
    let input_file = PrivateFileIdentity {
        path: contract.input.path.clone(),
        bytes: contract.input.bytes,
        sha256: contract.input.sha256.clone(),
    };
    let current_executable = std::env::current_exe().map_err(|source| {
        error(format!(
            "resolve repository synthetic test executable: {source}"
        ))
    })?;
    if contract.purpose != "synthetic_mechanism"
        || contract.rom_class != ReleaseRomClass::Unclassified
        || contract.guest_cycle != REPOSITORY_SYNTHETIC_RELEASE_CYCLE
        || contract.input.role != "synthetic_input"
        || contract.input.provenance != "repository_defined_synthetic"
        || !identity_matches_bytes(
            &contract.admission_manifest,
            REPOSITORY_SYNTHETIC_RELEASE_MANIFEST_BYTES,
        )
        || !identity_matches_bytes(
            &contract.readiness_report,
            REPOSITORY_SYNTHETIC_RELEASE_READINESS_BYTES,
        )
        || !identity_matches_bytes(&input_file, REPOSITORY_SYNTHETIC_RELEASE_INPUT_BYTES)
        || Path::new(&contract.child.executable.path) != current_executable
    {
        return Err(error(
            "synthetic runner authority is confined to fn64's exact repository-defined fixture, typed synthetic source, and current test executable",
        ));
    }
    Ok(())
}

pub fn run_private_release_series(
    verified_contract: &VerifiedPrivateReleaseRunContract,
    output_directory: impl AsRef<Path>,
) -> Result<PrivateReleaseSeriesReceipt, PrivateReleaseSeriesError> {
    let contract = verified_contract.contract();
    contract.verify_integrity()?;
    contract.verify_bound_files()?;
    verify_release_program_build_receipt_binding(contract)?;
    let output_directory = output_directory.as_ref();
    validate_new_private_directory(output_directory)?;
    fs::create_dir(output_directory).map_err(|source| {
        error(format!(
            "create private release output directory {}: {source}",
            output_directory.display()
        ))
    })?;
    let staged_child = stage_child_executable(&contract.child.executable)?;
    let staged_microcode_pair = stage_microcode_pair(contract)?;

    let runner_executable = std::env::current_exe()
        .map_err(|source| error(format!("resolve runner executable: {source}")))?;
    let runner_executable_sha256 = sha256_file(&runner_executable, "runner executable")?.1;
    let mut nonce = [0u8; 32];
    getrandom::fill(&mut nonce)
        .map_err(|source| error(format!("obtain OS-random private series nonce: {source}")))?;
    let nonce_hex = hex(&nonce);
    let child_executable_sha256 = contract.child.executable.sha256.clone();

    let mut evidence = Vec::with_capacity(PRIVATE_RELEASE_SERIES_COUNT);
    let mut runs = Vec::with_capacity(PRIVATE_RELEASE_SERIES_COUNT);
    for index in 0..PRIVATE_RELEASE_SERIES_COUNT {
        contract.verify_bound_files().map_err(|source| {
            error(format!(
                "private release preflight before child {} failed: {source}",
                index + 1
            ))
        })?;
        verify_release_program_build_receipt_binding(contract).map_err(|source| {
            error(format!(
                "private release program-build preflight before child {} failed: {source}",
                index + 1
            ))
        })?;
        if let Some(pair) = &staged_microcode_pair {
            pair.verify().map_err(|source| {
                error(format!(
                    "private release staged microcode preflight before child {} failed: {source}",
                    index + 1
                ))
            })?;
        }
        let staged_identity = PrivateFileIdentity {
            path: staged_child
                .0
                .to_str()
                .ok_or_else(|| error("staged child executable path is not UTF-8"))?
                .to_owned(),
            bytes: contract.child.executable.bytes,
            sha256: contract.child.executable.sha256.clone(),
        };
        verify_file_identity(&staged_identity, "staged child executable", false)?;
        let ordinal = u64::try_from(index + 1).expect("ten runs fit u64");
        let report_name = report_name(ordinal);
        let report_path = output_directory.join(&report_name);
        let journal_path = report_path.with_extension("unsupported.jsonl");
        let stdout_path = output_directory.join(format!("run-{ordinal:02}.stdout.log"));
        let stderr_path = output_directory.join(format!("run-{ordinal:02}.stderr.log"));
        for path in [&report_path, &journal_path, &stdout_path, &stderr_path] {
            if path.exists() || path.symlink_metadata().is_ok() {
                return Err(error(format!(
                    "private release child {ordinal} output {} already exists",
                    path.display()
                )));
            }
        }
        let run_event_sha256 = derive_run_event_sha256(
            &nonce,
            &contract.contract_sha256,
            &child_executable_sha256,
            ordinal,
            &report_name,
        )?;
        let stdout = create_new_file(&stdout_path, "child stdout log")?;
        let stderr = create_new_file(&stderr_path, "child stderr log")?;
        let mut command = Command::new(&staged_child.0);
        command
            .args(&contract.child.argv)
            .current_dir(&contract.child.working_directory)
            .env_clear()
            .stdin(Stdio::null())
            .stdout(Stdio::from(stdout))
            .stderr(Stdio::from(stderr));
        for entry in &contract.child.environment {
            command.env(&entry.name, &entry.value);
        }
        command
            .env("ROM", &contract.input.path)
            .env(RELEASE_ROM_CLASS_ENV, contract.rom_class.wire_name())
            .env(RELEASE_GATE_CYCLE_ENV, contract.guest_cycle.to_string())
            .env(RELEASE_REPORT_ENV, &report_path)
            .env(RELEASE_RUN_EVENT_SHA256_ENV, &run_event_sha256);
        if let Some(pair) = &staged_microcode_pair {
            command
                .env(RELEASE_MICROCODE_TEXT_PATH_ENV, &pair.text.0)
                .env(RELEASE_MICROCODE_DATA_PATH_ENV, &pair.data.0);
        }

        let status = command.status().map_err(|source| {
            error(format!(
                "spawn private release child {ordinal} directly from {}: {source}",
                staged_child.0.display()
            ))
        })?;
        if !status.success() {
            return Err(error(format!(
                "private release child {ordinal} exited unsuccessfully with {status}; outputs and journal were preserved in {}",
                output_directory.display()
            )));
        }

        let verified = read_and_verify_pair(
            contract,
            ordinal,
            &run_event_sha256,
            &report_path,
            &journal_path,
        )?;
        runs.push(PrivateReleaseSeriesRun {
            ordinal,
            run_event_sha256,
            report_file_sha256: verified.report_file_sha256,
            journal_file_sha256: verified.journal_file_sha256,
            report_sha256: verified.report.report_sha256.clone(),
            artifact_root_sha256: verified.report.digest.root_sha256.clone(),
        });
        evidence.push((verified.report, verified.journal));
    }
    contract.verify_bound_files()?;
    verify_release_program_build_receipt_binding(contract)?;
    if let Some(pair) = &staged_microcode_pair {
        pair.verify()?;
    }
    let staged_identity = PrivateFileIdentity {
        path: staged_child
            .0
            .to_str()
            .ok_or_else(|| error("staged child executable path is not UTF-8"))?
            .to_owned(),
        bytes: contract.child.executable.bytes,
        sha256: contract.child.executable.sha256.clone(),
    };
    verify_file_identity(&staged_identity, "staged child executable", false)?;
    let verified = verify_release_evidence_series(&evidence, PRIVATE_RELEASE_SERIES_COUNT)
        .map_err(|source| error(format!("verify exact private release series: {source}")))?;
    if verified.count != PRIVATE_RELEASE_SERIES_COUNT
        || evidence.len() != PRIVATE_RELEASE_SERIES_COUNT
    {
        return Err(error(format!(
            "private release verifier returned {} reports; exactly {} are required",
            verified.count, PRIVATE_RELEASE_SERIES_COUNT
        )));
    }
    let mut receipt = PrivateReleaseSeriesReceipt {
        schema: PRIVATE_RELEASE_SERIES_RECEIPT_SCHEMA.to_owned(),
        contract_sha256: contract.contract_sha256.clone(),
        runner_executable_sha256,
        child_executable_sha256,
        series_nonce: nonce_hex,
        report_scenario: contract.report_scenario.clone(),
        input_sha256: contract.input.sha256.clone(),
        guest_cycle: contract.guest_cycle,
        expected_execution_source: contract.expected_execution_source.clone(),
        count: verified.count,
        semantic_report_sha256: verified.report_sha256,
        runs,
        receipt_sha256: String::new(),
    };
    receipt.receipt_sha256 = receipt.recompute_receipt_sha256()?;
    receipt.verify_integrity()?;
    write_receipt_new(&output_directory.join(RECEIPT_FILE), &receipt)?;
    Ok(receipt)
}

pub fn verify_private_release_series(
    verified_contract: &VerifiedPrivateReleaseRunContract,
    output_directory: impl AsRef<Path>,
    receipt: &PrivateReleaseSeriesReceipt,
) -> Result<VerifiedPrivateReleaseSeries, PrivateReleaseSeriesError> {
    let runner_executable = std::env::current_exe()
        .map_err(|source| error(format!("resolve verifier executable: {source}")))?;
    verify_private_release_series_with_runner(
        verified_contract,
        output_directory,
        receipt,
        runner_executable,
    )
}

/// Verify a retained series against the exact executable image that ran it.
///
/// This explicit runner path is required when verification occurs in a
/// different binary from `run-private-release-series`. The receipt binds only
/// the runner digest, so accepting no path here would turn that identity into
/// an unchecked caller assertion.
pub fn verify_private_release_series_with_runner(
    verified_contract: &VerifiedPrivateReleaseRunContract,
    output_directory: impl AsRef<Path>,
    receipt: &PrivateReleaseSeriesReceipt,
    runner_executable: impl AsRef<Path>,
) -> Result<VerifiedPrivateReleaseSeries, PrivateReleaseSeriesError> {
    let output_directory = output_directory.as_ref();
    let runner_executable = runner_executable.as_ref();
    verify_private_release_series_inner(
        verified_contract.contract(),
        output_directory,
        receipt,
        runner_executable,
    )?;
    Ok(VerifiedPrivateReleaseSeries {
        contract: verified_contract.contract().clone(),
        output_directory: output_directory.canonicalize().map_err(|source| {
            error(format!(
                "resolve private release output directory {}: {source}",
                output_directory.display()
            ))
        })?,
        receipt: receipt.clone(),
        runner_executable: runner_executable.canonicalize().map_err(|source| {
            error(format!(
                "resolve private release runner executable {}: {source}",
                runner_executable.display()
            ))
        })?,
    })
}

pub(super) fn verify_private_release_series_inner(
    contract: &PrivateReleaseRunContract,
    output_directory: &Path,
    receipt: &PrivateReleaseSeriesReceipt,
    runner_executable: &Path,
) -> Result<(), PrivateReleaseSeriesError> {
    contract.verify_integrity()?;
    contract.verify_bound_files()?;
    verify_release_program_build_receipt_binding(contract)?;
    receipt.verify_integrity()?;
    validate_private_existing_directory(output_directory, "private release output directory")?;
    let retained_receipt_path = output_directory.join(RECEIPT_FILE);
    validate_regular_no_symlink_file(&retained_receipt_path, "private release receipt")?;
    let retained_receipt: PrivateReleaseSeriesReceipt =
        serde_json::from_slice(&fs::read(&retained_receipt_path).map_err(|source| {
            error(format!(
                "read private release receipt {}: {source}",
                retained_receipt_path.display()
            ))
        })?)
        .map_err(|source| {
            error(format!(
                "parse private release receipt {}: {source}",
                retained_receipt_path.display()
            ))
        })?;
    if &retained_receipt != receipt {
        return Err(error(
            "supplied private release receipt differs from the exact receipt retained in its output directory",
        ));
    }
    if receipt.contract_sha256 != contract.contract_sha256
        || receipt.report_scenario != contract.report_scenario
        || receipt.input_sha256 != contract.input.sha256
        || receipt.guest_cycle != contract.guest_cycle
        || receipt.expected_execution_source != contract.expected_execution_source
        || receipt.child_executable_sha256 != contract.child.executable.sha256
    {
        return Err(error(
            "private release receipt does not match its exact run contract",
        ));
    }
    validate_regular_no_symlink_file(runner_executable, "private release runner executable")?;
    validate_native_executable(runner_executable)?;
    let runner_sha256 = sha256_file(runner_executable, "private release runner executable")?.1;
    if receipt.runner_executable_sha256 != runner_sha256 {
        return Err(error(format!(
            "private release receipt runner executable SHA {} differs from supplied runner {}",
            receipt.runner_executable_sha256, runner_sha256
        )));
    }
    let nonce = decode_hash(&receipt.series_nonce, "series_nonce")?;
    let mut evidence = Vec::with_capacity(PRIVATE_RELEASE_SERIES_COUNT);
    for run in &receipt.runs {
        let report_name = report_name(run.ordinal);
        let expected_event = derive_run_event_sha256(
            &nonce,
            &contract.contract_sha256,
            &contract.child.executable.sha256,
            run.ordinal,
            &report_name,
        )?;
        if run.run_event_sha256 != expected_event {
            return Err(error(format!(
                "receipt run {} event identity does not derive from its series nonce and contract",
                run.ordinal
            )));
        }
        let report_path = output_directory.join(&report_name);
        let journal_path = report_path.with_extension("unsupported.jsonl");
        let verified = read_and_verify_pair(
            contract,
            run.ordinal,
            &run.run_event_sha256,
            &report_path,
            &journal_path,
        )?;
        if verified.report_file_sha256 != run.report_file_sha256
            || verified.journal_file_sha256 != run.journal_file_sha256
            || verified.report.report_sha256 != run.report_sha256
            || verified.report.digest.root_sha256 != run.artifact_root_sha256
        {
            return Err(error(format!(
                "receipt run {} file or semantic identity differs from retained evidence",
                run.ordinal
            )));
        }
        evidence.push((verified.report, verified.journal));
    }
    let verified = verify_release_evidence_series(&evidence, PRIVATE_RELEASE_SERIES_COUNT)
        .map_err(|source| error(format!("reverify exact private release series: {source}")))?;
    if verified.count != PRIVATE_RELEASE_SERIES_COUNT
        || verified.report_sha256 != receipt.semantic_report_sha256
    {
        return Err(error(
            "retained private release evidence does not match its exact ten-run receipt",
        ));
    }
    contract.verify_bound_files()?;
    verify_release_program_build_receipt_binding(contract)?;
    Ok(())
}
