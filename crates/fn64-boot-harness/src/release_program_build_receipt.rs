//! Private program-input identity co-binding for a release child image.
//!
//! Receipts contain local paths and are therefore private, content-bearing
//! inputs. They are never release-report payloads and must not be committed.

#[cfg(test)]
use crate::native_program_archives_sha256;
use crate::ExecutionDestinationSource;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fmt,
    fs::{self, File},
    io::Read,
    path::{Component, Path, PathBuf},
};

pub(crate) const RELEASE_PROGRAM_BUILD_RECEIPT_SCHEMA: &str =
    "fn64.release-program-build-receipt.v1";

const RECEIPT_DIGEST_DOMAIN: &[u8] = b"fn64.release-program-build-receipt-digest.v1\0";
const MAX_RELEASE_PROGRAM_FILE_BYTES: u64 = 8 * 1024 * 1024 * 1024;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ReleaseProgramFileIdentity {
    pub(crate) path: String,
    pub(crate) bytes: u64,
    pub(crate) sha256: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct NativeArchiveBuildInput {
    pub(crate) label: String,
    pub(crate) file: ReleaseProgramFileIdentity,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum ReleaseProgramBuildLane {
    NativeArchives {
        archives: Vec<NativeArchiveBuildInput>,
    },
    TypedObservedFunction {
        identity_wire: ReleaseProgramFileIdentity,
    },
    TypedBlock {
        pack: ReleaseProgramFileIdentity,
        expected_program_sha256: String,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ReleaseProgramBuildReceipt {
    pub(crate) schema: String,
    pub(crate) child_executable: ReleaseProgramFileIdentity,
    pub(crate) lane: ReleaseProgramBuildLane,
    pub(crate) expected_execution_source: ExecutionDestinationSource,
    pub(crate) receipt_sha256: String,
}

#[derive(Debug)]
pub(crate) struct VerifiedReleaseProgramBuildReceipt {
    pub(crate) receipt: ReleaseProgramBuildReceipt,
    pub(crate) recomputed_execution_source: ExecutionDestinationSource,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ReleaseProgramBuildReceiptError(String);

impl fmt::Display for ReleaseProgramBuildReceiptError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for ReleaseProgramBuildReceiptError {}

impl ReleaseProgramBuildReceipt {
    pub(crate) fn recompute_receipt_sha256(
        &self,
    ) -> Result<String, ReleaseProgramBuildReceiptError> {
        self.verify_shape(false)?;
        let mut wire = Vec::new();
        wire.extend_from_slice(RECEIPT_DIGEST_DOMAIN);
        push_bytes(&mut wire, self.schema.as_bytes());
        encode_file_identity(&mut wire, &self.child_executable)?;
        encode_lane(&mut wire, &self.lane)?;
        encode_execution_source(&mut wire, &self.expected_execution_source)?;
        Ok(sha256_hex(&wire))
    }

    fn verify_shape(
        &self,
        require_receipt_hash: bool,
    ) -> Result<(), ReleaseProgramBuildReceiptError> {
        if self.schema != RELEASE_PROGRAM_BUILD_RECEIPT_SCHEMA {
            return Err(error(format!(
                "unsupported release program build receipt schema {:?}",
                self.schema
            )));
        }
        validate_file_identity(&self.child_executable, "child_executable")?;
        match &self.lane {
            ReleaseProgramBuildLane::NativeArchives { archives } => {
                if archives.is_empty() {
                    return Err(error(
                        "native archive receipt must bind at least one archive",
                    ));
                }
                let mut previous: Option<&str> = None;
                for (index, archive) in archives.iter().enumerate() {
                    validate_label(&archive.label, &format!("lane.archives[{index}].label"))?;
                    if let Some(previous) = previous {
                        if previous == archive.label {
                            return Err(error(format!(
                                "lane.archives repeats logical label {:?}",
                                archive.label
                            )));
                        }
                        if previous > archive.label.as_str() {
                            return Err(error(
                                "lane.archives must be in strictly increasing canonical-label order",
                            ));
                        }
                    }
                    validate_file_identity(&archive.file, &format!("lane.archives[{index}].file"))?;
                    previous = Some(&archive.label);
                }
                if !matches!(
                    self.expected_execution_source,
                    ExecutionDestinationSource::NativeArchive { .. }
                ) {
                    return Err(error(
                        "native archive receipt requires a native_archive execution source",
                    ));
                }
            }
            ReleaseProgramBuildLane::TypedObservedFunction { identity_wire } => {
                validate_file_identity(identity_wire, "lane.identity_wire")?;
                if !matches!(
                    self.expected_execution_source,
                    ExecutionDestinationSource::TypedObservedFunctionProgram { .. }
                ) {
                    return Err(error(
                        "typed observed-function receipt requires a typed_observed_function_program execution source",
                    ));
                }
            }
            ReleaseProgramBuildLane::TypedBlock {
                pack,
                expected_program_sha256,
            } => {
                validate_file_identity(pack, "lane.pack")?;
                require_sha256(expected_program_sha256, "lane.expected_program_sha256")?;
                if !matches!(
                    self.expected_execution_source,
                    ExecutionDestinationSource::TypedBlockProgram { .. }
                ) {
                    return Err(error(
                        "typed-block receipt requires a typed_block_program execution source",
                    ));
                }
            }
        }
        validate_execution_source(&self.expected_execution_source)?;
        if require_receipt_hash || !self.receipt_sha256.is_empty() {
            require_sha256(&self.receipt_sha256, "receipt_sha256")?;
        }
        Ok(())
    }
}

pub(crate) fn load_release_program_build_receipt(
    path: impl AsRef<Path>,
) -> Result<VerifiedReleaseProgramBuildReceipt, ReleaseProgramBuildReceiptError> {
    let path = path.as_ref();
    validate_regular_no_symlink_file(path, "release program build receipt")?;
    let bytes = fs::read(path).map_err(|source| {
        error(format!(
            "read release program build receipt {}: {source}",
            path.display()
        ))
    })?;
    let receipt: ReleaseProgramBuildReceipt = serde_json::from_slice(&bytes).map_err(|source| {
        error(format!(
            "parse release program build receipt {}: {source}",
            path.display()
        ))
    })?;
    verify_release_program_build_receipt(receipt)
}

pub(crate) fn verify_release_program_build_receipt(
    receipt: ReleaseProgramBuildReceipt,
) -> Result<VerifiedReleaseProgramBuildReceipt, ReleaseProgramBuildReceiptError> {
    receipt.verify_shape(true)?;
    let recomputed_receipt_sha256 = receipt.recompute_receipt_sha256()?;
    if receipt.receipt_sha256 != recomputed_receipt_sha256 {
        return Err(error(format!(
            "release program build receipt digest mismatch: stored {}, recomputed {recomputed_receipt_sha256}",
            receipt.receipt_sha256
        )));
    }

    verify_bound_file(&receipt.child_executable, "child_executable")?;
    let recomputed_execution_source = match &receipt.lane {
        ReleaseProgramBuildLane::NativeArchives { archives } => {
            let mut digest = Sha256::new();
            digest.update(b"fn64.native-program-archives.v1\0");
            digest.update(
                u64::try_from(archives.len())
                    .expect("archive count fits canonical u64")
                    .to_be_bytes(),
            );
            for (index, archive) in archives.iter().enumerate() {
                digest.update(
                    u64::try_from(archive.label.len())
                        .expect("archive label length fits canonical u64")
                        .to_be_bytes(),
                );
                digest.update(archive.label.as_bytes());
                digest.update(archive.file.bytes.to_be_bytes());
                verify_bound_file_and_update(
                    &archive.file,
                    &format!("lane.archives[{index}].file"),
                    Some(&mut digest),
                )?;
            }
            ExecutionDestinationSource::NativeArchive {
                artifact_sha256: hex(&digest.finalize()),
            }
        }
        ReleaseProgramBuildLane::TypedObservedFunction { identity_wire } => {
            let (_, sha256) = verify_bound_file(identity_wire, "lane.identity_wire")?;
            ExecutionDestinationSource::TypedObservedFunctionProgram {
                artifact_sha256: sha256,
            }
        }
        ReleaseProgramBuildLane::TypedBlock {
            pack,
            expected_program_sha256,
        } => {
            let (_, dispatch_artifact_sha256) = verify_bound_file(pack, "lane.pack")?;
            ExecutionDestinationSource::TypedBlockProgram {
                program_sha256: expected_program_sha256.clone(),
                dispatch_artifact_sha256,
            }
        }
    };
    if recomputed_execution_source != receipt.expected_execution_source {
        return Err(error(format!(
            "release program build receipt execution source mismatch: declared {:?}, recomputed {:?}",
            receipt.expected_execution_source, recomputed_execution_source
        )));
    }

    Ok(VerifiedReleaseProgramBuildReceipt {
        receipt,
        recomputed_execution_source,
    })
}

fn validate_file_identity(
    identity: &ReleaseProgramFileIdentity,
    field: &str,
) -> Result<(), ReleaseProgramBuildReceiptError> {
    if identity.bytes == 0 {
        return Err(error(format!("{field}.bytes must be positive")));
    }
    if identity.bytes > MAX_RELEASE_PROGRAM_FILE_BYTES {
        return Err(error(format!(
            "{field}.bytes exceeds the {}-byte release-program limit",
            MAX_RELEASE_PROGRAM_FILE_BYTES
        )));
    }
    require_sha256(&identity.sha256, &format!("{field}.sha256"))?;
    validate_absolute_no_parent(Path::new(&identity.path), &format!("{field}.path"))
}

fn validate_label(value: &str, field: &str) -> Result<(), ReleaseProgramBuildReceiptError> {
    let bytes = value.as_bytes();
    let canonical = (1..=128).contains(&bytes.len())
        && (bytes[0].is_ascii_lowercase() || bytes[0].is_ascii_digit())
        && bytes.iter().all(|byte| {
            byte.is_ascii_lowercase()
                || byte.is_ascii_digit()
                || matches!(*byte, b'.' | b'_' | b'-')
        });
    if !canonical {
        return Err(error(format!(
            "{field} {value:?} is not a canonical logical label"
        )));
    }
    Ok(())
}

fn validate_execution_source(
    source: &ExecutionDestinationSource,
) -> Result<(), ReleaseProgramBuildReceiptError> {
    match source {
        ExecutionDestinationSource::NoProgram => Err(error(
            "release program build receipt cannot declare a no_program execution source",
        )),
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

fn verify_bound_file(
    identity: &ReleaseProgramFileIdentity,
    field: &str,
) -> Result<(u64, String), ReleaseProgramBuildReceiptError> {
    verify_bound_file_and_update(identity, field, None)
}

fn verify_bound_file_and_update(
    identity: &ReleaseProgramFileIdentity,
    field: &str,
    mut aggregate: Option<&mut Sha256>,
) -> Result<(u64, String), ReleaseProgramBuildReceiptError> {
    validate_file_identity(identity, field)?;
    let path = Path::new(&identity.path);
    validate_regular_no_symlink_file(path, field)?;
    let mut file = File::open(path)
        .map_err(|source| error(format!("open {field} {}: {source}", path.display())))?;
    let before = file
        .metadata()
        .map_err(|source| error(format!("inspect {field} {}: {source}", path.display())))?;
    let mut digest = Sha256::new();
    let mut observed_bytes = 0u64;
    let mut buffer = [0u8; 1024 * 1024];
    loop {
        let read = file
            .read(&mut buffer)
            .map_err(|source| error(format!("read {field} {}: {source}", path.display())))?;
        if read == 0 {
            break;
        }
        observed_bytes = observed_bytes
            .checked_add(u64::try_from(read).expect("buffer length fits u64"))
            .ok_or_else(|| error(format!("{field} {} length overflow", path.display())))?;
        digest.update(&buffer[..read]);
        if let Some(aggregate) = aggregate.as_deref_mut() {
            aggregate.update(&buffer[..read]);
        }
    }
    let after = file
        .metadata()
        .map_err(|source| error(format!("reinspect {field} {}: {source}", path.display())))?;
    if before.len() != after.len() || before.modified().ok() != after.modified().ok() {
        return Err(error(format!(
            "{field} {} changed while it was being measured",
            path.display()
        )));
    }
    let observed_sha256 = hex(&digest.finalize());
    if observed_bytes != identity.bytes || observed_sha256 != identity.sha256 {
        return Err(error(format!(
            "{field} identity drift at {}: expected bytes={} sha256={}, observed bytes={observed_bytes} sha256={observed_sha256}",
            path.display(), identity.bytes, identity.sha256
        )));
    }
    Ok((observed_bytes, observed_sha256))
}

fn validate_regular_no_symlink_file(
    path: &Path,
    field: &str,
) -> Result<(), ReleaseProgramBuildReceiptError> {
    validate_absolute_no_parent(path, field)?;
    reject_symlink_components(path, field)?;
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

fn validate_absolute_no_parent(
    path: &Path,
    field: &str,
) -> Result<(), ReleaseProgramBuildReceiptError> {
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

fn reject_symlink_components(
    path: &Path,
    field: &str,
) -> Result<(), ReleaseProgramBuildReceiptError> {
    let mut current = PathBuf::new();
    for component in path.components() {
        current.push(component.as_os_str());
        let metadata = fs::symlink_metadata(&current).map_err(|source| {
            error(format!(
                "inspect {field} component {}: {source}",
                current.display()
            ))
        })?;
        if metadata.file_type().is_symlink() {
            return Err(error(format!(
                "{field} has forbidden symlink component {}",
                current.display()
            )));
        }
    }
    Ok(())
}

fn encode_file_identity(
    wire: &mut Vec<u8>,
    identity: &ReleaseProgramFileIdentity,
) -> Result<(), ReleaseProgramBuildReceiptError> {
    push_bytes(wire, identity.path.as_bytes());
    push_u64(wire, identity.bytes);
    push_hash(wire, &identity.sha256, "file identity sha256")
}

fn encode_lane(
    wire: &mut Vec<u8>,
    lane: &ReleaseProgramBuildLane,
) -> Result<(), ReleaseProgramBuildReceiptError> {
    match lane {
        ReleaseProgramBuildLane::NativeArchives { archives } => {
            wire.push(1);
            push_u64(
                wire,
                u64::try_from(archives.len()).expect("archive count fits canonical u64"),
            );
            for archive in archives {
                push_bytes(wire, archive.label.as_bytes());
                encode_file_identity(wire, &archive.file)?;
            }
        }
        ReleaseProgramBuildLane::TypedObservedFunction { identity_wire } => {
            wire.push(2);
            encode_file_identity(wire, identity_wire)?;
        }
        ReleaseProgramBuildLane::TypedBlock {
            pack,
            expected_program_sha256,
        } => {
            wire.push(3);
            encode_file_identity(wire, pack)?;
            push_hash(
                wire,
                expected_program_sha256,
                "lane.expected_program_sha256",
            )?;
        }
    }
    Ok(())
}

fn encode_execution_source(
    wire: &mut Vec<u8>,
    source: &ExecutionDestinationSource,
) -> Result<(), ReleaseProgramBuildReceiptError> {
    match source {
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

fn require_sha256(value: &str, field: &str) -> Result<(), ReleaseProgramBuildReceiptError> {
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

fn push_u64(wire: &mut Vec<u8>, value: u64) {
    wire.extend_from_slice(&value.to_be_bytes());
}

fn push_bytes(wire: &mut Vec<u8>, value: &[u8]) {
    push_u64(
        wire,
        u64::try_from(value.len()).expect("host byte slice fits canonical u64"),
    );
    wire.extend_from_slice(value);
}

fn push_hash(
    wire: &mut Vec<u8>,
    value: &str,
    field: &str,
) -> Result<(), ReleaseProgramBuildReceiptError> {
    require_sha256(value, field)?;
    let mut bytes = [0u8; 32];
    for (index, pair) in value.as_bytes().chunks_exact(2).enumerate() {
        bytes[index] = (hex_nibble(pair[0]) << 4) | hex_nibble(pair[1]);
    }
    wire.extend_from_slice(&bytes);
    Ok(())
}

fn hex_nibble(value: u8) -> u8 {
    match value {
        b'0'..=b'9' => value - b'0',
        b'a'..=b'f' => value - b'a' + 10,
        _ => unreachable!("SHA-256 syntax was validated before decoding"),
    }
}

fn sha256_hex(bytes: &[u8]) -> String {
    hex(&Sha256::digest(bytes))
}

fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut value = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        value.push(char::from(DIGITS[usize::from(byte >> 4)]));
        value.push(char::from(DIGITS[usize::from(byte & 0x0f)]));
    }
    value
}

fn error(message: impl Into<String>) -> ReleaseProgramBuildReceiptError {
    ReleaseProgramBuildReceiptError(message.into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

    struct TestDirectory(PathBuf);

    impl TestDirectory {
        fn new() -> Self {
            let base = if Path::new("/private/tmp").is_dir() {
                PathBuf::from("/private/tmp")
            } else {
                std::env::temp_dir()
            };
            loop {
                let path = base.join(format!(
                    "fn64-program-receipt-{}-{}",
                    std::process::id(),
                    TEMP_COUNTER.fetch_add(1, Ordering::Relaxed)
                ));
                match fs::create_dir(&path) {
                    Ok(()) => return Self(path),
                    Err(source) if source.kind() == std::io::ErrorKind::AlreadyExists => {}
                    Err(source) => panic!("create test directory {}: {source}", path.display()),
                }
            }
        }
    }

    impl Drop for TestDirectory {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn write_file(directory: &Path, name: &str, bytes: &[u8]) -> ReleaseProgramFileIdentity {
        let path = directory.join(name);
        fs::write(&path, bytes).unwrap();
        ReleaseProgramFileIdentity {
            path: path.to_str().unwrap().to_owned(),
            bytes: u64::try_from(bytes.len()).unwrap(),
            sha256: sha256_hex(bytes),
        }
    }

    fn finish_receipt(
        child_executable: ReleaseProgramFileIdentity,
        lane: ReleaseProgramBuildLane,
        expected_execution_source: ExecutionDestinationSource,
    ) -> ReleaseProgramBuildReceipt {
        let mut receipt = ReleaseProgramBuildReceipt {
            schema: RELEASE_PROGRAM_BUILD_RECEIPT_SCHEMA.to_owned(),
            child_executable,
            lane,
            expected_execution_source,
            receipt_sha256: String::new(),
        };
        receipt.receipt_sha256 = receipt.recompute_receipt_sha256().unwrap();
        receipt
    }

    #[test]
    fn native_archives_recompute_existing_canonical_identity() {
        let directory = TestDirectory::new();
        let child = write_file(&directory.0, "child", b"native child image");
        let generated = write_file(&directory.0, "generated.a", b"generated archive bytes");
        let bridge = write_file(&directory.0, "bridge.a", b"bridge archive bytes");
        let expected = hex(&native_program_archives_sha256([
            (
                "generated-code".to_owned(),
                b"generated archive bytes".to_vec(),
            ),
            (
                "section-bridge".to_owned(),
                b"bridge archive bytes".to_vec(),
            ),
        ]));
        let receipt = finish_receipt(
            child,
            ReleaseProgramBuildLane::NativeArchives {
                archives: vec![
                    NativeArchiveBuildInput {
                        label: "generated-code".to_owned(),
                        file: generated.clone(),
                    },
                    NativeArchiveBuildInput {
                        label: "section-bridge".to_owned(),
                        file: bridge,
                    },
                ],
            },
            ExecutionDestinationSource::NativeArchive {
                artifact_sha256: expected,
            },
        );

        let verified = verify_release_program_build_receipt(receipt.clone()).unwrap();
        assert_eq!(verified.receipt, receipt);
        assert_eq!(
            verified.recomputed_execution_source,
            receipt.expected_execution_source
        );

        fs::write(&generated.path, b"changed archive bytes").unwrap();
        assert!(verify_release_program_build_receipt(receipt)
            .unwrap_err()
            .to_string()
            .contains("lane.archives[0].file identity drift"));
    }

    #[test]
    fn typed_observed_function_uses_raw_identity_wire_sha256() {
        let directory = TestDirectory::new();
        let child = write_file(&directory.0, "child", b"typed function child");
        let identity_wire = write_file(
            &directory.0,
            "function-identity.wire",
            b"canonical typed observed function identity wire",
        );
        let expected = identity_wire.sha256.clone();
        let receipt = finish_receipt(
            child,
            ReleaseProgramBuildLane::TypedObservedFunction { identity_wire },
            ExecutionDestinationSource::TypedObservedFunctionProgram {
                artifact_sha256: expected,
            },
        );

        verify_release_program_build_receipt(receipt.clone()).unwrap();
        if let ReleaseProgramBuildLane::TypedObservedFunction { identity_wire } = &receipt.lane {
            fs::write(&identity_wire.path, b"changed identity wire").unwrap();
        }
        assert!(verify_release_program_build_receipt(receipt)
            .unwrap_err()
            .to_string()
            .contains("lane.identity_wire identity drift"));
    }

    #[test]
    fn typed_block_binds_pack_dispatch_and_expected_program_identity() {
        let directory = TestDirectory::new();
        let child = write_file(&directory.0, "child", b"typed block child");
        let pack = write_file(
            &directory.0,
            "program.pack",
            b"exact typed block pack bytes",
        );
        let dispatch = pack.sha256.clone();
        let program = sha256_hex(b"expected installed block program");
        let receipt = finish_receipt(
            child,
            ReleaseProgramBuildLane::TypedBlock {
                pack,
                expected_program_sha256: program.clone(),
            },
            ExecutionDestinationSource::TypedBlockProgram {
                program_sha256: program,
                dispatch_artifact_sha256: dispatch,
            },
        );

        verify_release_program_build_receipt(receipt.clone()).unwrap();

        let mut wrong_program = receipt;
        if let ExecutionDestinationSource::TypedBlockProgram { program_sha256, .. } =
            &mut wrong_program.expected_execution_source
        {
            *program_sha256 = "66".repeat(32);
        }
        wrong_program.receipt_sha256 = wrong_program.recompute_receipt_sha256().unwrap();
        assert!(verify_release_program_build_receipt(wrong_program)
            .unwrap_err()
            .to_string()
            .contains("execution source mismatch"));
    }

    #[test]
    fn labels_must_be_unique_canonical_and_preordered() {
        let directory = TestDirectory::new();
        let child = write_file(&directory.0, "child", b"child");
        let first = write_file(&directory.0, "first.a", b"first");
        let second = write_file(&directory.0, "second.a", b"second");
        let source = ExecutionDestinationSource::NativeArchive {
            artifact_sha256: "11".repeat(32),
        };

        for labels in [
            ["same", "same"],
            ["z-last", "a-first"],
            ["Uppercase", "valid"],
        ] {
            let receipt = ReleaseProgramBuildReceipt {
                schema: RELEASE_PROGRAM_BUILD_RECEIPT_SCHEMA.to_owned(),
                child_executable: child.clone(),
                lane: ReleaseProgramBuildLane::NativeArchives {
                    archives: vec![
                        NativeArchiveBuildInput {
                            label: labels[0].to_owned(),
                            file: first.clone(),
                        },
                        NativeArchiveBuildInput {
                            label: labels[1].to_owned(),
                            file: second.clone(),
                        },
                    ],
                },
                expected_execution_source: source.clone(),
                receipt_sha256: String::new(),
            };
            assert!(receipt.recompute_receipt_sha256().is_err());
        }
    }

    #[test]
    fn source_lane_and_recomputed_identity_must_match() {
        let directory = TestDirectory::new();
        let child = write_file(&directory.0, "child", b"child");
        let wire = write_file(&directory.0, "wire", b"wire");
        let wrong_lane = ReleaseProgramBuildReceipt {
            schema: RELEASE_PROGRAM_BUILD_RECEIPT_SCHEMA.to_owned(),
            child_executable: child.clone(),
            lane: ReleaseProgramBuildLane::TypedObservedFunction {
                identity_wire: wire.clone(),
            },
            expected_execution_source: ExecutionDestinationSource::NativeArchive {
                artifact_sha256: wire.sha256.clone(),
            },
            receipt_sha256: String::new(),
        };
        assert!(wrong_lane.recompute_receipt_sha256().is_err());

        let mismatched = finish_receipt(
            child,
            ReleaseProgramBuildLane::TypedObservedFunction {
                identity_wire: wire,
            },
            ExecutionDestinationSource::TypedObservedFunctionProgram {
                artifact_sha256: "22".repeat(32),
            },
        );
        assert!(verify_release_program_build_receipt(mismatched)
            .unwrap_err()
            .to_string()
            .contains("execution source mismatch"));
    }

    #[test]
    fn receipt_and_bound_file_tamper_fail_closed() {
        let directory = TestDirectory::new();
        let child = write_file(&directory.0, "child", b"child");
        let pack = write_file(&directory.0, "pack", b"pack");
        let program = sha256_hex(b"program");
        let receipt = finish_receipt(
            child.clone(),
            ReleaseProgramBuildLane::TypedBlock {
                pack: pack.clone(),
                expected_program_sha256: program.clone(),
            },
            ExecutionDestinationSource::TypedBlockProgram {
                program_sha256: program,
                dispatch_artifact_sha256: pack.sha256.clone(),
            },
        );

        let mut digest_tamper = receipt.clone();
        digest_tamper.receipt_sha256 = "00".repeat(32);
        assert!(verify_release_program_build_receipt(digest_tamper)
            .unwrap_err()
            .to_string()
            .contains("receipt digest mismatch"));

        fs::write(&child.path, b"changed child length and bytes").unwrap();
        assert!(verify_release_program_build_receipt(receipt.clone())
            .unwrap_err()
            .to_string()
            .contains("child_executable identity drift"));
        fs::write(&child.path, b"child").unwrap();
        fs::write(&pack.path, b"PACK").unwrap();
        assert!(verify_release_program_build_receipt(receipt)
            .unwrap_err()
            .to_string()
            .contains("lane.pack identity drift"));
    }

    #[test]
    fn unknown_fields_nonregular_and_descriptor_drift_are_rejected() {
        let directory = TestDirectory::new();
        let child = write_file(&directory.0, "child", b"child");
        let wire = write_file(&directory.0, "wire", b"wire");
        let receipt = finish_receipt(
            child,
            ReleaseProgramBuildLane::TypedObservedFunction {
                identity_wire: wire,
            },
            ExecutionDestinationSource::TypedObservedFunctionProgram {
                artifact_sha256: sha256_hex(b"wire"),
            },
        );
        let receipt_path = directory.0.join("receipt.json");
        let mut json = serde_json::to_value(&receipt).unwrap();
        json.as_object_mut()
            .unwrap()
            .insert("unknown".to_owned(), serde_json::json!(true));
        fs::write(&receipt_path, serde_json::to_vec(&json).unwrap()).unwrap();
        assert!(load_release_program_build_receipt(&receipt_path)
            .unwrap_err()
            .to_string()
            .contains("unknown field"));

        let directory_identity = ReleaseProgramFileIdentity {
            path: directory.0.to_str().unwrap().to_owned(),
            bytes: 1,
            sha256: "11".repeat(32),
        };
        assert!(verify_bound_file(&directory_identity, "directory")
            .unwrap_err()
            .to_string()
            .contains("not a regular file"));

        let file = write_file(&directory.0, "drift", b"bytes");
        let mut wrong_length = file.clone();
        wrong_length.bytes += 1;
        assert!(verify_bound_file(&wrong_length, "wrong_length").is_err());
        let mut oversized = file.clone();
        oversized.bytes = MAX_RELEASE_PROGRAM_FILE_BYTES + 1;
        assert!(verify_bound_file(&oversized, "oversized")
            .unwrap_err()
            .to_string()
            .contains("release-program limit"));
        let mut wrong_hash = file;
        wrong_hash.sha256 = "33".repeat(32);
        assert!(verify_bound_file(&wrong_hash, "wrong_hash").is_err());
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_receipt_and_bound_paths_are_rejected() {
        use std::os::unix::fs::symlink;

        let directory = TestDirectory::new();
        let child = write_file(&directory.0, "child", b"child");
        let wire = write_file(&directory.0, "wire", b"wire");
        let receipt = finish_receipt(
            child,
            ReleaseProgramBuildLane::TypedObservedFunction {
                identity_wire: wire.clone(),
            },
            ExecutionDestinationSource::TypedObservedFunctionProgram {
                artifact_sha256: wire.sha256.clone(),
            },
        );
        let receipt_path = directory.0.join("receipt.json");
        fs::write(&receipt_path, serde_json::to_vec(&receipt).unwrap()).unwrap();
        let receipt_link = directory.0.join("receipt-link.json");
        symlink(&receipt_path, &receipt_link).unwrap();
        assert!(load_release_program_build_receipt(&receipt_link)
            .unwrap_err()
            .to_string()
            .contains("forbidden symlink"));

        let wire_link = directory.0.join("wire-link");
        symlink(&wire.path, &wire_link).unwrap();
        let linked = ReleaseProgramFileIdentity {
            path: wire_link.to_str().unwrap().to_owned(),
            bytes: wire.bytes,
            sha256: wire.sha256,
        };
        assert!(verify_bound_file(&linked, "linked")
            .unwrap_err()
            .to_string()
            .contains("forbidden symlink"));
    }

    #[test]
    fn canonical_receipt_digest_binds_paths_child_lane_and_source() {
        let directory = TestDirectory::new();
        let child = write_file(&directory.0, "child", b"child");
        let pack = write_file(&directory.0, "pack", b"pack");
        let program = sha256_hex(b"program");
        let receipt = finish_receipt(
            child,
            ReleaseProgramBuildLane::TypedBlock {
                pack: pack.clone(),
                expected_program_sha256: program.clone(),
            },
            ExecutionDestinationSource::TypedBlockProgram {
                program_sha256: program,
                dispatch_artifact_sha256: pack.sha256,
            },
        );
        let original = receipt.receipt_sha256.clone();

        let mut mutations = Vec::new();
        let mut changed_path = receipt.clone();
        changed_path.child_executable.path.push_str("-other");
        mutations.push(changed_path);
        let mut changed_lane = receipt.clone();
        if let ReleaseProgramBuildLane::TypedBlock {
            expected_program_sha256,
            ..
        } = &mut changed_lane.lane
        {
            *expected_program_sha256 = "44".repeat(32);
        }
        mutations.push(changed_lane);
        let mut changed_source = receipt;
        if let ExecutionDestinationSource::TypedBlockProgram { program_sha256, .. } =
            &mut changed_source.expected_execution_source
        {
            *program_sha256 = "55".repeat(32);
        }
        mutations.push(changed_source);

        for mutation in mutations {
            assert_ne!(mutation.recompute_receipt_sha256().unwrap(), original);
        }
    }

    #[test]
    fn canonical_receipt_digest_has_a_fixed_cross_language_golden() {
        let receipt = ReleaseProgramBuildReceipt {
            schema: RELEASE_PROGRAM_BUILD_RECEIPT_SCHEMA.to_owned(),
            child_executable: ReleaseProgramFileIdentity {
                path: "/private/fn64/child".to_owned(),
                bytes: 999,
                sha256: "11".repeat(32),
            },
            lane: ReleaseProgramBuildLane::TypedBlock {
                pack: ReleaseProgramFileIdentity {
                    path: "/private/fn64/program.pack".to_owned(),
                    bytes: 789,
                    sha256: "22".repeat(32),
                },
                expected_program_sha256: "33".repeat(32),
            },
            expected_execution_source: ExecutionDestinationSource::TypedBlockProgram {
                program_sha256: "33".repeat(32),
                dispatch_artifact_sha256: "22".repeat(32),
            },
            receipt_sha256: String::new(),
        };
        assert_eq!(
            receipt.recompute_receipt_sha256().unwrap(),
            "3ce6e14e0a67c1837ca506e85815d20b6b9fe45f70b8a425ef2eeaf0ab6cd650"
        );
    }
}
