#![allow(clippy::module_inception)]
use super::*;

pub(super) struct VerifiedPair {
    pub(super) report: ReleaseGateReport,
    pub(super) journal: ParsedUnsupportedJournal,
    pub(super) report_file_sha256: String,
    pub(super) journal_file_sha256: String,
}

pub(super) fn read_and_verify_pair(
    contract: &PrivateReleaseRunContract,
    ordinal: u64,
    expected_run_event_sha256: &str,
    report_path: &Path,
    journal_path: &Path,
) -> Result<VerifiedPair, PrivateReleaseSeriesError> {
    validate_regular_no_symlink_file(report_path, "release report")?;
    validate_regular_no_symlink_file(journal_path, "unsupported journal")?;
    let report_bytes = fs::read(report_path)
        .map_err(|source| error(format!("read report {}: {source}", report_path.display())))?;
    let journal_bytes = fs::read(journal_path)
        .map_err(|source| error(format!("read journal {}: {source}", journal_path.display())))?;
    let report_file_sha256 = sha256_hex(&report_bytes);
    let journal_file_sha256 = sha256_hex(&journal_bytes);
    let report: ReleaseGateReport = serde_json::from_slice(&report_bytes)
        .map_err(|source| error(format!("parse report {}: {source}", report_path.display())))?;
    if report.schema != RELEASE_REPORT_SCHEMA {
        return Err(error(format!(
            "private release child {ordinal} emitted report schema {:?}; exact {RELEASE_REPORT_SCHEMA} is required",
            report.schema
        )));
    }
    if report.scenario != contract.report_scenario
        || report.digest.guest_cycle != contract.guest_cycle
        || report.execution_destinations.source != contract.expected_execution_source
    {
        return Err(error(format!(
            "private release child {ordinal} report does not match contract scenario/cycle/execution source"
        )));
    }
    verify_report_rom_binding(contract, ordinal, &report)?;
    require_exact_artifacts(&report)?;
    let journal = parse_unsupported_journal(&journal_bytes).map_err(|source| {
        error(format!(
            "parse unsupported journal {}: {source}",
            journal_path.display()
        ))
    })?;
    verify_release_report_journal(&report, &journal).map_err(|source| {
        error(format!(
            "verify private release child {ordinal} report/journal: {source}"
        ))
    })?;
    verify_consumed_microcode_pair(contract, ordinal, &report)?;
    if journal.release_run_event_sha256() != Some(expected_run_event_sha256) {
        return Err(error(format!(
            "private release child {ordinal} journal does not bind its runner-derived event identity"
        )));
    }
    verify_release_evidence_series(&[(report.clone(), journal.clone())], 1).map_err(|source| {
        error(format!(
            "private release child {ordinal} omits live-minimum or zero-unsupported evidence: {source}"
        ))
    })?;
    Ok(VerifiedPair {
        report,
        journal,
        report_file_sha256,
        journal_file_sha256,
    })
}

pub(super) fn verify_report_rom_binding(
    contract: &PrivateReleaseRunContract,
    ordinal: u64,
    report: &ReleaseGateReport,
) -> Result<(), PrivateReleaseSeriesError> {
    if report.input_sha256 != contract.input.sha256 {
        return Err(error(format!(
            "private release child {ordinal} report input SHA-256 does not match the contract ROM"
        )));
    }
    if contract.purpose == "synthetic_mechanism" {
        if report.rom.is_some() {
            return Err(error(format!(
                "private release child {ordinal} synthetic report fabricated ROM evidence"
            )));
        }
    } else {
        match &report.rom {
            Some(rom)
                if rom.class == contract.rom_class
                    && rom.byte_len == contract.input.bytes => {
                let input_bytes = fs::read(&contract.input.path).map_err(|source| {
                    error(format!(
                        "private release child {ordinal} reread admitted ROM for header verification: {source}"
                    ))
                })?;
                if u64::try_from(input_bytes.len()).ok() != Some(contract.input.bytes)
                    || sha256_hex(&input_bytes) != contract.input.sha256
                {
                    return Err(error(format!(
                        "private release child {ordinal} admitted ROM identity drifted before header verification"
                    )));
                }
                let configured_tv_type = match rom.configured_tv_type {
                    ReleaseTvStandard::Ntsc => fn64_runtime::TvType::Ntsc,
                    ReleaseTvStandard::Pal => fn64_runtime::TvType::Pal,
                    ReleaseTvStandard::Mpal => fn64_runtime::TvType::Mpal,
                };
                let expected = ReleaseRomEvidence::from_bytes(
                    &input_bytes,
                    contract.rom_class,
                    configured_tv_type,
                )
                .map_err(|source| {
                    error(format!(
                        "private release child {ordinal} admitted ROM header is invalid: {source}"
                    ))
                })?;
                if rom != &expected {
                    return Err(error(format!(
                        "private release child {ordinal} ROM evidence does not match the independently decoded admitted ROM"
                    )));
                }
            }
            Some(rom) => {
                return Err(error(format!(
                    "private release child {ordinal} ROM evidence class/length {:?}/{} does not match contract {}/{}",
                    rom.class,
                    rom.byte_len,
                    contract.rom_class.wire_name(),
                    contract.input.bytes
                )))
            }
            None => {
                return Err(error(format!(
                    "private release child {ordinal} omitted contract-bound ROM evidence"
                )))
            }
        }
    }
    Ok(())
}

pub(super) fn verify_consumed_microcode_pair(
    contract: &PrivateReleaseRunContract,
    ordinal: u64,
    report: &ReleaseGateReport,
) -> Result<(), PrivateReleaseSeriesError> {
    if contract.purpose == "synthetic_mechanism" {
        return Ok(());
    }
    let text = required_artifact(contract, "microcode_text")?;
    let data = required_artifact(contract, "microcode_data")?;
    let matched = report.rsp_rdp.ordered.iter().any(|event| {
        matches!(
            &event.observation,
            RspRdpObservationKindEvidence::MicrocodeRecognition {
                text_sha256,
                data_bytes,
                data_sha256,
                family: Some(_),
                ..
            } if text_sha256 == &text.sha256
                && u64::from(*data_bytes) == data.bytes
                && data_sha256 == &data.sha256
        )
    });
    if !matched {
        return Err(error(format!(
            "private release child {ordinal} has no single recognized microcode event binding the admitted text SHA-256 and exact data length/SHA-256"
        )));
    }
    Ok(())
}

pub(super) fn require_exact_artifacts(report: &ReleaseGateReport) -> Result<(), PrivateReleaseSeriesError> {
    let observed: BTreeSet<_> = report
        .digest
        .artifacts
        .iter()
        .map(|artifact| artifact.kind)
        .collect();
    let expected = BTreeSet::from([
        ArtifactKind::Framebuffer,
        ArtifactKind::Audio,
        ArtifactKind::Memory,
        ArtifactKind::DeviceState,
        ArtifactKind::TimingTrace,
    ]);
    if report.digest.artifacts.len() != expected.len() || observed != expected {
        return Err(error(
            "private release report does not contain the exact five fixed-cycle artifacts",
        ));
    }
    let declared: BTreeSet<_> = report
        .closure
        .iter()
        .map(|path| path.name.as_str())
        .collect();
    let missing: Vec<_> = LIVE_MINIMUM_CLOSURE_PATHS
        .iter()
        .copied()
        .filter(|path| !declared.contains(path))
        .collect();
    if !missing.is_empty() {
        return Err(error(format!(
            "private release report omits live-minimum paths {missing:?}"
        )));
    }
    Ok(())
}

pub(super) fn derive_run_event_sha256(
    nonce: &[u8; 32],
    contract_sha256: &str,
    executable_sha256: &str,
    ordinal: u64,
    report_name: &str,
) -> Result<String, PrivateReleaseSeriesError> {
    let mut wire = Vec::new();
    wire.extend_from_slice(RUN_EVENT_DOMAIN);
    wire.extend_from_slice(nonce);
    push_hash(&mut wire, contract_sha256, "contract_sha256")?;
    push_hash(&mut wire, executable_sha256, "child.executable.sha256")?;
    push_u64(&mut wire, ordinal);
    push_bytes(&mut wire, report_name.as_bytes());
    Ok(sha256_hex(&wire))
}

pub(super) fn report_name(ordinal: u64) -> String {
    format!("report-{ordinal:02}.json")
}

pub(super) fn validate_file_identity_shape(
    identity: &PrivateFileIdentity,
    field: &str,
) -> Result<(), PrivateReleaseSeriesError> {
    if identity.bytes == 0 {
        return Err(error(format!("{field}.bytes must be positive")));
    }
    require_sha256(&identity.sha256, &format!("{field}.sha256"))?;
    validate_absolute_no_parent(Path::new(&identity.path), &format!("{field}.path"))
}

pub(super) fn validate_artifact_identity_shape(
    identity: &PrivateArtifactIdentity,
    field: &str,
) -> Result<(), PrivateReleaseSeriesError> {
    if !canonical_role(&identity.role) {
        return Err(error(format!(
            "{field}.role {:?} is not canonical",
            identity.role
        )));
    }
    if !canonical_role(&identity.provenance) {
        return Err(error(format!(
            "{field}.provenance {:?} is not canonical",
            identity.provenance
        )));
    }
    validate_file_identity_shape(
        &PrivateFileIdentity {
            path: identity.path.clone(),
            bytes: identity.bytes,
            sha256: identity.sha256.clone(),
        },
        field,
    )
}

pub(super) fn validate_execution_source(
    source: &ExecutionDestinationSource,
) -> Result<(), PrivateReleaseSeriesError> {
    match source {
        ExecutionDestinationSource::NoProgram => Ok(()),
        ExecutionDestinationSource::NativeArchive { artifact_sha256 }
        | ExecutionDestinationSource::TypedObservedFunctionProgram { artifact_sha256 } => {
            require_sha256(artifact_sha256, "expected_execution_source.artifact_sha256")
        }
        ExecutionDestinationSource::TypedBlockProgram {
            program_sha256,
            dispatch_artifact_sha256,
        } => {
            require_sha256(program_sha256, "expected_execution_source.program_sha256")?;
            require_sha256(
                dispatch_artifact_sha256,
                "expected_execution_source.dispatch_artifact_sha256",
            )
        }
    }
}

pub(super) fn validate_environment_entry(
    entry: &PrivateEnvironmentEntry,
) -> Result<(), PrivateReleaseSeriesError> {
    let bytes = entry.name.as_bytes();
    if bytes.is_empty()
        || !(bytes[0] == b'_' || bytes[0].is_ascii_uppercase())
        || !bytes[1..]
            .iter()
            .all(|byte| *byte == b'_' || byte.is_ascii_uppercase() || byte.is_ascii_digit())
    {
        return Err(error(format!(
            "child environment name {:?} must match [A-Z_][A-Z0-9_]*",
            entry.name
        )));
    }
    if entry.name.contains('=') || entry.name.contains('\0') || entry.value.contains('\0') {
        return Err(error(format!(
            "child environment entry {:?} contains a forbidden character",
            entry.name
        )));
    }
    if reserved_environment_name(&entry.name) {
        return Err(error(format!(
            "child environment entry {:?} is runner-owned and cannot be declared",
            entry.name
        )));
    }
    if dangerous_code_loading_environment_name(&entry.name) {
        return Err(error(format!(
            "child environment entry {:?} can alter loader or interpreter code selection and is forbidden",
            entry.name
        )));
    }
    Ok(())
}

pub(super) fn reserved_environment_name(name: &str) -> bool {
    name == "ROM"
        || name.starts_with("FN64_RELEASE_")
        || name.starts_with("OOT_RELEASE_")
        || name.starts_with("FN64_PRIVATE_RUN_")
}

pub(super) fn dangerous_code_loading_environment_name(name: &str) -> bool {
    matches!(
        name,
        "PATH"
            | "PATHEXT"
            | "COMSPEC"
            | "BASH_ENV"
            | "ENV"
            | "SHELLOPTS"
            | "ZDOTDIR"
            | "GCONV_PATH"
            | "LOCPATH"
            | "NLSPATH"
            | "CLASSPATH"
            | "JAVA_TOOL_OPTIONS"
            | "JDK_JAVA_OPTIONS"
            | "_JAVA_OPTIONS"
            | "SSLKEYLOGFILE"
            | "NODE_OPTIONS"
            | "GBM_BACKEND"
            | "GALLIUM_DRIVER"
            | "EGL_PLATFORM"
    ) || [
        "LD_",
        "DYLD_",
        "PYTHON",
        "PERL",
        "RUBY",
        "NODE_",
        "LUA_",
        "TCL_",
        "DOTNET_",
        "MONO_",
        "POWERSHELL_",
        "GTK_",
        "QT_",
        "VK_",
        "LIBGL_",
        "MESA_",
        "__GL_",
        "D3D12SDK",
        "DXVK_",
        "VKD3D_",
    ]
    .iter()
    .any(|prefix| name.starts_with(prefix))
}

pub(super) fn validate_scenario(value: &str) -> Result<(), PrivateReleaseSeriesError> {
    let bytes = value.as_bytes();
    let canonical = (1..=128).contains(&bytes.len())
        && (bytes[0].is_ascii_lowercase() || bytes[0].is_ascii_digit())
        && bytes.iter().all(|byte| {
            byte.is_ascii_lowercase()
                || byte.is_ascii_digit()
                || matches!(*byte, b'.' | b'_' | b'-')
        });
    if !canonical
        || (bytes.len() == 64
            && bytes
                .iter()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(byte)))
    {
        return Err(error(format!(
            "private release scenario {value:?} is not a canonical content-free label"
        )));
    }
    Ok(())
}

pub(super) fn canonical_role(value: &str) -> bool {
    !value.is_empty()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
}

pub(super) fn verify_private_file_identity(
    identity: &PrivateFileIdentity,
    field: &str,
) -> Result<(), PrivateReleaseSeriesError> {
    verify_file_identity(identity, field, true)
}

pub(super) fn verify_private_artifact_identity(
    identity: &PrivateArtifactIdentity,
    field: &str,
) -> Result<(), PrivateReleaseSeriesError> {
    verify_file_identity(
        &PrivateFileIdentity {
            path: identity.path.clone(),
            bytes: identity.bytes,
            sha256: identity.sha256.clone(),
        },
        field,
        true,
    )
}

pub(super) fn verify_file_identity(
    identity: &PrivateFileIdentity,
    field: &str,
    private: bool,
) -> Result<(), PrivateReleaseSeriesError> {
    validate_file_identity_shape(identity, field)?;
    let path = Path::new(&identity.path);
    validate_regular_no_symlink_file(path, field)?;
    if private {
        require_outside_or_ignored(path, field)?;
    }
    let (bytes, sha256) = sha256_file(path, field)?;
    if bytes != identity.bytes || sha256 != identity.sha256 {
        return Err(error(format!(
            "{field} identity drift at {}: expected bytes={} sha256={}, observed bytes={bytes} sha256={sha256}",
            path.display(), identity.bytes, identity.sha256
        )));
    }
    Ok(())
}

pub(super) fn sha256_file(path: &Path, field: &str) -> Result<(u64, String), PrivateReleaseSeriesError> {
    let mut file = File::open(path)
        .map_err(|source| error(format!("open {field} {}: {source}", path.display())))?;
    let mut digest = Sha256::new();
    let mut total = 0u64;
    let mut buffer = [0u8; 1024 * 1024];
    loop {
        let read = file
            .read(&mut buffer)
            .map_err(|source| error(format!("read {field} {}: {source}", path.display())))?;
        if read == 0 {
            break;
        }
        total = total
            .checked_add(u64::try_from(read).expect("buffer read fits u64"))
            .ok_or_else(|| error(format!("{field} {} length overflow", path.display())))?;
        digest.update(&buffer[..read]);
    }
    Ok((total, hex(&digest.finalize())))
}

pub(super) fn validate_absolute_no_parent(path: &Path, field: &str) -> Result<(), PrivateReleaseSeriesError> {
    if !path.is_absolute()
        || path.as_os_str().is_empty()
        || path
            .components()
            .any(|component| matches!(component, Component::ParentDir))
    {
        return Err(error(format!(
            "{field} {} must be absolute and contain no '..' component",
            path.display()
        )));
    }
    Ok(())
}

pub(super) fn reject_symlink_components(
    path: &Path,
    include_leaf: bool,
    field: &str,
) -> Result<(), PrivateReleaseSeriesError> {
    let components: Vec<_> = path.components().collect();
    let limit = if include_leaf {
        components.len()
    } else {
        components.len().saturating_sub(1)
    };
    let mut current = PathBuf::new();
    for component in components.into_iter().take(limit) {
        current.push(component.as_os_str());
        match fs::symlink_metadata(&current) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                return Err(error(format!(
                    "{field} has forbidden symlink component {}",
                    current.display()
                )))
            }
            Ok(_) => {}
            Err(source) if source.kind() == std::io::ErrorKind::NotFound => {}
            Err(source) => {
                return Err(error(format!(
                    "inspect {field} component {}: {source}",
                    current.display()
                )))
            }
        }
    }
    Ok(())
}

pub(super) fn validate_regular_no_symlink_file(
    path: &Path,
    field: &str,
) -> Result<(), PrivateReleaseSeriesError> {
    validate_absolute_no_parent(path, field)?;
    reject_symlink_components(path, true, field)?;
    let metadata = fs::symlink_metadata(path)
        .map_err(|source| error(format!("inspect {field} {}: {source}", path.display())))?;
    if !metadata.file_type().is_file() {
        return Err(error(format!(
            "{field} {} is not a regular file",
            path.display()
        )));
    }
    Ok(())
}

pub(super) fn validate_native_executable(path: &Path) -> Result<(), PrivateReleaseSeriesError> {
    let mut file = File::open(path).map_err(|source| {
        error(format!(
            "open native executable candidate {}: {source}",
            path.display()
        ))
    })?;
    let mut magic = [0u8; 4];
    file.read_exact(&mut magic).map_err(|source| {
        error(format!(
            "read native executable magic {}: {source}",
            path.display()
        ))
    })?;
    let elf = magic == *b"\x7fELF";
    let mach_o = matches!(
        magic,
        [0xfe, 0xed, 0xfa, 0xce]
            | [0xce, 0xfa, 0xed, 0xfe]
            | [0xfe, 0xed, 0xfa, 0xcf]
            | [0xcf, 0xfa, 0xed, 0xfe]
            | [0xca, 0xfe, 0xba, 0xbe]
            | [0xbe, 0xba, 0xfe, 0xca]
            | [0xca, 0xfe, 0xba, 0xbf]
            | [0xbf, 0xba, 0xfe, 0xca]
    );
    let portable_executable = if magic[..2] == *b"MZ" {
        file.seek(SeekFrom::Start(0x3c))
            .and_then(|_| {
                let mut offset = [0u8; 4];
                file.read_exact(&mut offset)?;
                file.seek(SeekFrom::Start(u64::from(u32::from_le_bytes(offset))))
            })
            .and_then(|_| {
                let mut signature = [0u8; 4];
                file.read_exact(&mut signature)?;
                Ok(signature == *b"PE\0\0")
            })
            .unwrap_or(false)
    } else {
        false
    };
    if !elf && !mach_o && !portable_executable {
        return Err(error(format!(
            "child executable {} is not a native ELF, Mach-O, or PE image; scripts and interpreter-mediated launchers are forbidden",
            path.display()
        )));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = file
            .metadata()
            .map_err(|source| {
                error(format!(
                    "inspect native executable permissions {}: {source}",
                    path.display()
                ))
            })?
            .permissions()
            .mode();
        if mode & 0o111 == 0 {
            return Err(error(format!(
                "child executable {} has native image bytes but no Unix execute bit",
                path.display()
            )));
        }
    }
    Ok(())
}

pub(super) fn validate_absolute_no_symlink_directory(
    path: &Path,
    field: &str,
) -> Result<(), PrivateReleaseSeriesError> {
    validate_absolute_no_parent(path, field)?;
    reject_symlink_components(path, true, field)?;
    let metadata = fs::symlink_metadata(path)
        .map_err(|source| error(format!("inspect {field} {}: {source}", path.display())))?;
    if !metadata.file_type().is_dir() {
        return Err(error(format!(
            "{field} {} is not a directory",
            path.display()
        )));
    }
    Ok(())
}

pub(super) fn validate_private_existing_directory(
    path: &Path,
    field: &str,
) -> Result<(), PrivateReleaseSeriesError> {
    validate_absolute_no_symlink_directory(path, field)?;
    require_outside_or_ignored(path, field)
}

pub(super) fn validate_new_private_directory(path: &Path) -> Result<(), PrivateReleaseSeriesError> {
    validate_absolute_no_parent(path, "output directory")?;
    reject_symlink_components(path, false, "output directory")?;
    if path.exists() || fs::symlink_metadata(path).is_ok() {
        return Err(error(format!(
            "private release output directory {} already exists",
            path.display()
        )));
    }
    let parent = path
        .parent()
        .ok_or_else(|| error("private release output directory has no parent"))?;
    validate_absolute_no_symlink_directory(parent, "output directory parent")?;
    require_outside_or_ignored(path, "output directory")
}

pub(super) fn require_outside_or_ignored(path: &Path, field: &str) -> Result<(), PrivateReleaseSeriesError> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .map_err(|source| error(format!("resolve fn64 repository root: {source}")))?;
    let comparable = if path.exists() {
        path.canonicalize()
            .map_err(|source| error(format!("resolve {field} {}: {source}", path.display())))?
    } else {
        let parent = path
            .parent()
            .ok_or_else(|| error(format!("{field} {} has no parent", path.display())))?
            .canonicalize()
            .map_err(|source| {
                error(format!(
                    "resolve {field} parent {}: {source}",
                    path.display()
                ))
            })?;
        parent.join(
            path.file_name()
                .ok_or_else(|| error(format!("{field} {} has no file name", path.display())))?,
        )
    };
    let Ok(relative) = comparable.strip_prefix(&root) else {
        return Ok(());
    };
    if Command::new("git")
        .args(["ls-files", "--error-unmatch", "--"])
        .arg(relative)
        .current_dir(&root)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
    {
        return Err(error(format!(
            "{field} {} is tracked by git; private evidence cannot enter the repository",
            path.display()
        )));
    }
    if !Command::new("git")
        .args(["check-ignore", "-q", "--no-index", "--"])
        .arg(relative)
        .current_dir(&root)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
    {
        return Err(error(format!(
            "{field} {} is inside the repository and not gitignored",
            path.display()
        )));
    }
    Ok(())
}

pub(super) fn create_new_file(path: &Path, field: &str) -> Result<File, PrivateReleaseSeriesError> {
    OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|source| error(format!("create {field} {}: {source}", path.display())))
}

pub(super) fn write_receipt_new(
    path: &Path,
    receipt: &PrivateReleaseSeriesReceipt,
) -> Result<(), PrivateReleaseSeriesError> {
    let mut file = create_new_file(path, "private release receipt")?;
    serde_json::to_writer_pretty(&mut file, receipt)
        .map_err(|source| error(format!("serialize receipt {}: {source}", path.display())))?;
    file.write_all(b"\n")
        .and_then(|()| file.flush())
        .and_then(|()| file.sync_all())
        .map_err(|source| error(format!("persist receipt {}: {source}", path.display())))?;
    drop(file);
    #[cfg(unix)]
    {
        let parent = path
            .parent()
            .ok_or_else(|| error("private release receipt has no parent directory"))?;
        File::open(parent)
            .and_then(|directory| directory.sync_all())
            .map_err(|source| {
                error(format!(
                    "persist private release receipt directory {}: {source}",
                    parent.display()
                ))
            })?;
    }
    Ok(())
}

pub(super) fn encode_file_identity(
    wire: &mut Vec<u8>,
    value: &PrivateFileIdentity,
) -> Result<(), PrivateReleaseSeriesError> {
    push_bytes(wire, value.path.as_bytes());
    push_u64(wire, value.bytes);
    push_hash(wire, &value.sha256, "file identity sha256")
}

pub(super) fn encode_artifact_identity(
    wire: &mut Vec<u8>,
    value: &PrivateArtifactIdentity,
) -> Result<(), PrivateReleaseSeriesError> {
    push_bytes(wire, value.role.as_bytes());
    push_bytes(wire, value.path.as_bytes());
    push_u64(wire, value.bytes);
    push_hash(wire, &value.sha256, "artifact identity sha256")?;
    push_bytes(wire, value.provenance.as_bytes());
    Ok(())
}

pub(super) fn encode_execution_source(
    wire: &mut Vec<u8>,
    value: &ExecutionDestinationSource,
) -> Result<(), PrivateReleaseSeriesError> {
    match value {
        ExecutionDestinationSource::NoProgram => wire.push(0),
        ExecutionDestinationSource::NativeArchive { artifact_sha256 } => {
            wire.push(1);
            push_hash(wire, artifact_sha256, "execution source artifact_sha256")?;
        }
        ExecutionDestinationSource::TypedObservedFunctionProgram { artifact_sha256 } => {
            wire.push(2);
            push_hash(wire, artifact_sha256, "execution source artifact_sha256")?;
        }
        ExecutionDestinationSource::TypedBlockProgram {
            program_sha256,
            dispatch_artifact_sha256,
        } => {
            wire.push(3);
            push_hash(wire, program_sha256, "execution source program_sha256")?;
            push_hash(
                wire,
                dispatch_artifact_sha256,
                "execution source dispatch_artifact_sha256",
            )?;
        }
    }
    Ok(())
}

pub(super) fn push_u64(wire: &mut Vec<u8>, value: u64) {
    wire.extend_from_slice(&value.to_be_bytes());
}

pub(super) fn push_bytes(wire: &mut Vec<u8>, bytes: &[u8]) {
    push_u64(
        wire,
        u64::try_from(bytes.len()).expect("host byte slice length fits canonical u64"),
    );
    wire.extend_from_slice(bytes);
}

pub(super) fn push_hash(
    wire: &mut Vec<u8>,
    value: &str,
    field: &str,
) -> Result<(), PrivateReleaseSeriesError> {
    wire.extend_from_slice(&decode_hash(value, field)?);
    Ok(())
}

pub(super) fn require_sha256(value: &str, field: &str) -> Result<(), PrivateReleaseSeriesError> {
    decode_hash(value, field).map(|_| ())
}

pub(super) fn decode_hash(value: &str, field: &str) -> Result<[u8; 32], PrivateReleaseSeriesError> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(error(format!("{field} is not a lowercase SHA-256")));
    }
    let mut output = [0u8; 32];
    for (index, pair) in value.as_bytes().chunks_exact(2).enumerate() {
        let high = (pair[0] as char).to_digit(16).expect("validated hex") as u8;
        let low = (pair[1] as char).to_digit(16).expect("validated hex") as u8;
        output[index] = (high << 4) | low;
    }
    Ok(output)
}

pub(super) fn sha256_hex(bytes: &[u8]) -> String {
    hex(&Sha256::digest(bytes))
}

pub(super) fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        output.push(DIGITS[(byte >> 4) as usize] as char);
        output.push(DIGITS[(byte & 0x0f) as usize] as char);
    }
    output
}

pub(super) fn error(message: impl Into<String>) -> PrivateReleaseSeriesError {
    PrivateReleaseSeriesError(message.into())
}
