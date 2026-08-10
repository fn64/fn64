#![allow(clippy::module_inception)]
use super::*;

pub(super) fn validate_inputs(
    inputs: &Wm2000GeneratedRunnerBuildInputsV1,
) -> Result<(), GeneratedRunnerBuildError> {
    validate_input_path(&inputs.rom, "ROM")?;
    validate_input_path(&inputs.boot_context, "BootContext")?;
    if inputs.executable_image_groups.is_empty() {
        return Err(error(
            "generated-runner build requires at least one executable-image group",
        ));
    }
    if !(MIN_BUILD_TIMEOUT_SECONDS..=MAX_BUILD_TIMEOUT_SECONDS).contains(&inputs.max_build_seconds)
    {
        return Err(error(format!(
            "generated-runner max_build_seconds must be {MIN_BUILD_TIMEOUT_SECONDS}..={MAX_BUILD_TIMEOUT_SECONDS}"
        )));
    }
    let mut names = BTreeSet::new();
    for group in &inputs.executable_image_groups {
        let valid_name = group.environment_name.starts_with("FN64_EXECUTABLE_IMAGE_")
            && group
                .environment_name
                .bytes()
                .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit() || byte == b'_');
        if !valid_name || !names.insert(&group.environment_name) {
            return Err(error(
                "generated-runner capture group has an invalid or duplicate environment name",
            ));
        }
        if group.captures.len() < 3 {
            return Err(error(
                "generated-runner capture group requires at least three captures",
            ));
        }
        for capture in &group.captures {
            validate_input_path(capture, "executable-image capture")?;
        }
    }
    Ok(())
}

pub(super) fn validate_input_path(path: &Path, label: &str) -> Result<(), GeneratedRunnerBuildError> {
    crate::private_fs::validate_absolute_no_parent(path, label).map_err(error)
}

pub(super) fn stage_private_inputs(
    inputs: &Wm2000GeneratedRunnerBuildInputsV1,
    scratch: &Path,
) -> Result<Wm2000GeneratedRunnerBuildInputsV1, GeneratedRunnerBuildError> {
    let directory = scratch.join("private-inputs");
    fs::create_dir(&directory).map_err(|source| {
        error(format!(
            "create generated-runner private-input staging directory: {source}"
        ))
    })?;
    let rom = stage_private_input_file(&inputs.rom, &directory.join("rom"), "ROM")?;
    let boot_context = stage_private_input_file(
        &inputs.boot_context,
        &directory.join("boot-context"),
        "BootContext",
    )?;
    let executable_image_groups = inputs
        .executable_image_groups
        .iter()
        .enumerate()
        .map(|(group_index, group)| {
            let captures = group
                .captures
                .iter()
                .enumerate()
                .map(|(capture_index, capture)| {
                    stage_private_input_file(
                        capture,
                        &directory.join(format!("group-{group_index}-capture-{capture_index}")),
                        "executable-image capture",
                    )
                })
                .collect::<Result<Vec<_>, _>>()?;
            Ok(Wm2000ExecutableImageGroupV1 {
                environment_name: group.environment_name.clone(),
                captures,
            })
        })
        .collect::<Result<Vec<_>, GeneratedRunnerBuildError>>()?;
    Ok(Wm2000GeneratedRunnerBuildInputsV1 {
        rom,
        boot_context,
        executable_image_groups,
        max_build_seconds: inputs.max_build_seconds,
    })
}

pub(super) fn stage_private_input_file(
    source: &Path,
    destination: &Path,
    label: &str,
) -> Result<PathBuf, GeneratedRunnerBuildError> {
    let mut output = create_new(destination)?;
    let source_measurement =
        crate::private_fs::measure_regular_stable_with(source, label, |event| match event {
            crate::private_fs::StableFileStream::Length(_) => Ok(()),
            crate::private_fs::StableFileStream::Chunk(bytes) => output
                .write_all(bytes)
                .map_err(|source| format!("stage {label} bytes: {source}")),
        })
        .map_err(error)?;
    output
        .flush()
        .map_err(|source| error(format!("flush staged {label}: {source}")))?;
    output
        .sync_all()
        .map_err(|source| error(format!("sync staged {label}: {source}")))?;
    drop(output);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        fs::set_permissions(destination, fs::Permissions::from_mode(0o400))
            .map_err(|source| error(format!("make staged {label} read-only: {source}")))?;
    }
    let staged = crate::private_fs::measure_regular_stable(destination, &format!("staged {label}"))
        .map_err(error)?;
    if staged.bytes != source_measurement.bytes || staged.sha256 != source_measurement.sha256 {
        return Err(error(format!(
            "staged {label} does not match the descriptor-stable source measurement"
        )));
    }
    Ok(destination.to_path_buf())
}

pub(super) fn private_inputs_sha256(
    inputs: &Wm2000GeneratedRunnerBuildInputsV1,
) -> Result<String, GeneratedRunnerBuildError> {
    let mut digest = Sha256::new();
    digest.update(b"fn64.wm2000-generated-runner-private-inputs.v2\0");
    hash_input_file(&mut digest, b"ROM", &inputs.rom)?;
    hash_input_file(&mut digest, b"BootContext", &inputs.boot_context)?;
    for group in &inputs.executable_image_groups {
        push_bytes(&mut digest, group.environment_name.as_bytes());
        digest.update((group.captures.len() as u64).to_be_bytes());
        for capture in &group.captures {
            hash_input_file(&mut digest, b"capture", capture)?;
        }
    }
    Ok(hex(&digest.finalize()))
}

pub(super) fn hash_input_file(
    digest: &mut Sha256,
    label: &[u8],
    path: &Path,
) -> Result<(), GeneratedRunnerBuildError> {
    validate_input_path(path, "private build input")?;
    push_bytes(digest, label);
    push_bytes(digest, path.as_os_str().as_encoded_bytes());
    crate::private_fs::measure_regular_stable_with(path, "private build input", |event| {
        match event {
            crate::private_fs::StableFileStream::Length(bytes) => {
                digest.update(bytes.to_be_bytes());
            }
            crate::private_fs::StableFileStream::Chunk(bytes) => digest.update(bytes),
        }
        Ok(())
    })
    .map_err(error)?;
    Ok(())
}

pub(super) fn stage_selected_binary(
    source: &Path,
    scratch: &Path,
    expected: &str,
) -> Result<PathBuf, GeneratedRunnerBuildError> {
    stage_executable(
        source,
        &scratch.join("selected-generated-runner"),
        expected,
        "generated runner",
    )
}

pub(super) fn stage_executable(
    source: &Path,
    destination: &Path,
    expected: &str,
    label: &str,
) -> Result<PathBuf, GeneratedRunnerBuildError> {
    let mut source_file = File::open(source)
        .map_err(|source_error| error(format!("open built {label}: {source_error}")))?;
    let mut destination_file = create_new(destination)?;
    std::io::copy(&mut source_file, &mut destination_file)
        .map_err(|source_error| error(format!("stage {label}: {source_error}")))?;
    destination_file
        .sync_all()
        .map_err(|source_error| error(format!("sync staged {label}: {source_error}")))?;
    drop(destination_file);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        fs::set_permissions(destination, fs::Permissions::from_mode(0o500)).map_err(
            |source_error| error(format!("make staged {label} executable: {source_error}")),
        )?;
    }
    if sha256_file(destination, &format!("staged {label}"))? != expected {
        return Err(error(format!(
            "staged {label} does not match selected Cargo artifact"
        )));
    }
    Ok(destination.to_path_buf())
}

pub(super) fn repository_workspace() -> Result<PathBuf, GeneratedRunnerBuildError> {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .map_err(|source| error(format!("resolve fn64 workspace: {source}")))
}

pub(super) fn wait_with_watchdog(
    child: &mut std::process::Child,
    timeout: Duration,
    label: &str,
) -> Result<(), GeneratedRunnerBuildError> {
    let started = Instant::now();
    loop {
        if child
            .try_wait()
            .map_err(|source| error(format!("poll {label}: {source}")))?
            .is_some()
        {
            return Ok(());
        }
        if started.elapsed() >= timeout {
            let _ = child.kill();
            let _ = child.wait();
            return Err(error(format!(
                "{label} exceeded {} seconds",
                timeout.as_secs()
            )));
        }
        thread::sleep(Duration::from_millis(25));
    }
}

pub(super) fn create_new(path: &Path) -> Result<File, GeneratedRunnerBuildError> {
    OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|source| error(format!("create {}: {source}", path.display())))
}

pub(super) fn sha256_file(path: &Path, label: &str) -> Result<String, GeneratedRunnerBuildError> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|source| error(format!("inspect {label} {}: {source}", path.display())))?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(error(format!("{label} is not a regular non-symlink file")));
    }
    let mut file = File::open(path)
        .map_err(|source| error(format!("open {label} {}: {source}", path.display())))?;
    let mut digest = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let count = file
            .read(&mut buffer)
            .map_err(|source| error(format!("read {label} {}: {source}", path.display())))?;
        if count == 0 {
            break;
        }
        digest.update(&buffer[..count]);
    }
    Ok(hex(&digest.finalize()))
}

pub(super) fn require_sha256(value: &str, field: &str) -> Result<(), GeneratedRunnerBuildError> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    {
        return Err(error(format!("{field} is not canonical lowercase SHA-256")));
    }
    Ok(())
}

pub(super) fn decode_sha256(value: &str) -> Result<[u8; 32], GeneratedRunnerBuildError> {
    require_sha256(value, "digest")?;
    let mut output = [0u8; 32];
    for (index, byte) in output.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&value[index * 2..index * 2 + 2], 16)
            .map_err(|source| error(format!("decode SHA-256: {source}")))?;
    }
    Ok(output)
}

pub(super) fn push_bytes(digest: &mut Sha256, bytes: &[u8]) {
    digest.update((bytes.len() as u64).to_be_bytes());
    digest.update(bytes);
}

pub(super) fn hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        output.push(HEX[(byte >> 4) as usize] as char);
        output.push(HEX[(byte & 0xf) as usize] as char);
    }
    output
}

pub(super) fn error(message: impl Into<String>) -> GeneratedRunnerBuildError {
    GeneratedRunnerBuildError(message.into())
}

#[derive(Debug)]
pub(super) struct ScratchDirectory(PathBuf);

impl ScratchDirectory {
    pub(super) fn create(nonce: &[u8; 32]) -> Result<Self, GeneratedRunnerBuildError> {
        let path = std::env::temp_dir().join(format!("fn64-generated-runner-{}", hex(nonce)));
        fs::create_dir(&path).map_err(|source| {
            error(format!(
                "create generated-runner scratch {}: {source}",
                path.display()
            ))
        })?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).map_err(|source| {
                error(format!(
                    "restrict generated-runner scratch {}: {source}",
                    path.display()
                ))
            })?;
        }
        let canonical = path.canonicalize().map_err(|source| {
            error(format!(
                "resolve generated-runner scratch {}: {source}",
                path.display()
            ))
        })?;
        Ok(Self(canonical))
    }

    pub(super) fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for ScratchDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
