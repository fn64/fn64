//! Streaming, content-silent publication of a private prepared shard tree.

use std::collections::BTreeSet;
#[cfg(any(target_os = "macos", target_os = "linux"))]
use std::ffi::CString;
use std::fmt;
use std::fs::{self, OpenOptions};
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use sha2::{Digest, Sha256};

use crate::generator::{GeneratedShard, PACKAGES};

pub const ROOT_SCHEMA_V2: &str = "fn64.wm-prepared-shard-tree.v2";
pub const ARTIFACT_SCHEMA_V1: &str = "fn64.wm-prepared-shard-artifact.v1";
const MANIFEST_NAME: &str = "manifest.v2";
const IDENTITY_NAME: &str = "identity.v1";
const UPDATE_MARKER_NAME: &str = ".update.v2";
static STAGING_NONCE: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SourceIdentityClaims {
    pub generator_source_sha256: [u8; 32],
    pub discovery_source_sha256: [u8; 32],
    pub emitter_source_sha256: [u8; 32],
    pub runtime_source_sha256: [u8; 32],
}

#[derive(Debug)]
pub struct PublishError(String);

impl fmt::Display for PublishError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for PublishError {}

fn error(message: impl Into<String>) -> PublishError {
    PublishError(message.into())
}

/// A one-shard-at-a-time publication. At most one ROM-derived generated shard
/// needs to be resident outside the generator; only identity lines accumulate.
pub struct PreparedTreePublication {
    output: PathBuf,
    parent: PathBuf,
    staging: Option<PathBuf>,
    manifest: String,
    next_package: usize,
}

impl PreparedTreePublication {
    pub fn begin(
        output: &Path,
        forbidden_repository_root: &Path,
        normalized_rom_sha256: [u8; 32],
        claims: SourceIdentityClaims,
    ) -> Result<Self, PublishError> {
        validate_absolute(output, "prepared output")?;
        validate_absolute(forbidden_repository_root, "repository root")?;
        validate_digest("normalized ROM", normalized_rom_sha256)?;
        validate_digest("generator source", claims.generator_source_sha256)?;
        validate_digest("discovery source", claims.discovery_source_sha256)?;
        validate_digest("emitter source", claims.emitter_source_sha256)?;
        validate_digest("runtime source", claims.runtime_source_sha256)?;
        let parent = output
            .parent()
            .ok_or_else(|| error("prepared output has no parent"))?;
        reject_symlink_components(parent, "prepared output parent")?;
        reject_symlink_components(forbidden_repository_root, "repository root")?;
        let canonical_parent = canonical_nonsymlink_directory(parent, "prepared output parent")?;
        let canonical_repository =
            canonical_nonsymlink_directory(forbidden_repository_root, "repository root")?;
        if canonical_parent.starts_with(&canonical_repository) {
            return Err(error("prepared output must be outside the repository"));
        }
        let output_name = output
            .file_name()
            .ok_or_else(|| error("prepared output must name one destination directory"))?;
        let canonical_output = canonical_parent.join(output_name);
        match fs::symlink_metadata(&canonical_output) {
            Ok(metadata) => require_private_directory_metadata(&metadata, "prepared output")?,
            Err(source) if source.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => return Err(error("cannot inspect prepared output")),
        }
        let staging = create_staging_directory(&canonical_parent)?;
        let manifest = format!(
            "schema {ROOT_SCHEMA_V2}\nnormalized_rom_sha256 {}\ngenerator_source_sha256 {}\ndiscovery_source_sha256 {}\nemitter_source_sha256 {}\nruntime_source_sha256 {}\nartifact_count {}\n",
            hex(normalized_rom_sha256),
            hex(claims.generator_source_sha256),
            hex(claims.discovery_source_sha256),
            hex(claims.emitter_source_sha256),
            hex(claims.runtime_source_sha256),
            PACKAGES.len(),
        );
        Ok(Self {
            output: canonical_output,
            parent: canonical_parent,
            staging: Some(staging),
            manifest,
            next_package: 0,
        })
    }

    pub fn push(&mut self, artifact: GeneratedShard) -> Result<(), PublishError> {
        let expected = PACKAGES
            .get(self.next_package)
            .ok_or_else(|| error("prepared publication received more packages than the inventory"))?;
        if artifact.package != *expected {
            return Err(error("prepared artifact stream is not canonical"));
        }
        let runner_sha256: [u8; 32] = Sha256::digest(artifact.runner.as_bytes()).into();
        let metadata_sha256: [u8; 32] = Sha256::digest(artifact.metadata.as_bytes()).into();
        let identity = format!(
            "schema {ARTIFACT_SCHEMA_V1}\npackage {expected}\nrunner_sha256 {}\nmetadata_sha256 {}\n",
            hex(runner_sha256),
            hex(metadata_sha256),
        );
        let identity_sha256: [u8; 32] = Sha256::digest(identity.as_bytes()).into();
        let staging = self
            .staging
            .as_deref()
            .expect("unfinished publication owns staging");
        let package_dir = staging.join(expected);
        create_private_directory(&package_dir)
            .map_err(|_| error("cannot create private prepared package"))?;
        write_new_private(&package_dir.join("runner.rs"), artifact.runner.as_bytes())?;
        write_new_private(
            &package_dir.join("metadata.rs"),
            artifact.metadata.as_bytes(),
        )?;
        write_new_private(&package_dir.join(IDENTITY_NAME), identity.as_bytes())?;
        sync_directory(&package_dir)?;
        self.manifest.push_str(&format!(
            "artifact {} {} {} {}\n",
            expected,
            hex(identity_sha256),
            hex(runner_sha256),
            hex(metadata_sha256),
        ));
        self.next_package += 1;
        Ok(())
    }

    pub fn finish(mut self) -> Result<[u8; 32], PublishError> {
        if self.next_package != PACKAGES.len() {
            return Err(error(
                "prepared artifact stream ended before the whole shard inventory",
            ));
        }
        let staging = self
            .staging
            .as_deref()
            .expect("unfinished publication owns staging");
        write_new_private(&staging.join(MANIFEST_NAME), self.manifest.as_bytes())?;
        sync_directory(staging)?;
        let manifest_sha256: [u8; 32] = Sha256::digest(self.manifest.as_bytes()).into();
        match rename_noreplace(staging, &self.output) {
            Ok(()) => {
                self.staging = None;
                sync_directory(&self.parent)?;
                Ok(manifest_sha256)
            }
            Err(source) if source.kind() == std::io::ErrorKind::AlreadyExists => {
                reconcile_existing_projection(&self.output, staging)?;
                Ok(manifest_sha256)
            }
            Err(_) => Err(error("cannot atomically publish prepared output")),
        }
    }
}

impl Drop for PreparedTreePublication {
    fn drop(&mut self) {
        if let Some(path) = self.staging.take() {
            let _ = fs::remove_dir_all(path);
        }
    }
}

fn validate_digest(label: &str, digest: [u8; 32]) -> Result<(), PublishError> {
    if digest == [0; 32] {
        return Err(error(format!("{label} identity must be nonzero")));
    }
    Ok(())
}

fn validate_absolute(path: &Path, label: &str) -> Result<(), PublishError> {
    if !path.is_absolute() {
        return Err(error(format!("{label} must be an explicit absolute path")));
    }
    Ok(())
}

fn canonical_nonsymlink_directory(path: &Path, label: &str) -> Result<PathBuf, PublishError> {
    let metadata =
        fs::symlink_metadata(path).map_err(|_| error(format!("cannot inspect {label}")))?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(error(format!("{label} must be a non-symlink directory")));
    }
    fs::canonicalize(path).map_err(|_| error(format!("cannot canonicalize {label}")))
}

fn reject_symlink_components(path: &Path, label: &str) -> Result<(), PublishError> {
    let mut prefix = PathBuf::new();
    for component in path.components() {
        prefix.push(component.as_os_str());
        let metadata = fs::symlink_metadata(&prefix)
            .map_err(|_| error(format!("cannot inspect {label} component")))?;
        if metadata.file_type().is_symlink() {
            return Err(error(format!("{label} contains a symlink component")));
        }
    }
    Ok(())
}

fn create_staging_directory(parent: &Path) -> Result<PathBuf, PublishError> {
    for _ in 0..100 {
        let nonce = STAGING_NONCE.fetch_add(1, Ordering::Relaxed);
        let staging = parent.join(format!(
            ".fn64-wm-prepared-stage-{}-{nonce}",
            std::process::id()
        ));
        match create_private_directory(&staging) {
            Ok(()) => return Ok(staging),
            Err(source) if source.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(_) => return Err(error("cannot create private prepared staging directory")),
        }
    }
    Err(error("cannot allocate unique prepared staging directory"))
}

#[cfg(unix)]
fn create_private_directory(path: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
    let mut builder = fs::DirBuilder::new();
    builder.mode(0o700).create(path)?;
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))
}

#[cfg(not(unix))]
fn create_private_directory(path: &Path) -> std::io::Result<()> {
    fs::create_dir(path)
}

fn write_new_private(path: &Path, bytes: &[u8]) -> Result<(), PublishError> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options
        .open(path)
        .map_err(|_| error("cannot create private prepared file"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        file.set_permissions(fs::Permissions::from_mode(0o600))
            .map_err(|_| error("cannot set private prepared file permissions"))?;
    }
    file.write_all(bytes)
        .map_err(|_| error("cannot write private prepared file"))?;
    file.sync_all()
        .map_err(|_| error("cannot sync private prepared file"))
}

fn sync_directory(path: &Path) -> Result<(), PublishError> {
    fs::File::open(path)
        .and_then(|directory| directory.sync_all())
        .map_err(|_| error("cannot sync prepared directory"))
}

#[cfg(target_os = "macos")]
fn rename_noreplace(source: &Path, destination: &Path) -> std::io::Result<()> {
    use std::os::unix::ffi::OsStrExt;

    const AT_FDCWD: i32 = -2;
    const RENAME_EXCL: u32 = 0x0000_0004;
    unsafe extern "C" {
        fn renameatx_np(
            fromfd: i32,
            from: *const std::ffi::c_char,
            tofd: i32,
            to: *const std::ffi::c_char,
            flags: u32,
        ) -> i32;
    }
    let source = CString::new(source.as_os_str().as_bytes()).map_err(|_| {
        std::io::Error::new(std::io::ErrorKind::InvalidInput, "source contains NUL")
    })?;
    let destination = CString::new(destination.as_os_str().as_bytes()).map_err(|_| {
        std::io::Error::new(std::io::ErrorKind::InvalidInput, "destination contains NUL")
    })?;
    // SAFETY: both C strings are NUL-terminated and live for the call;
    // RENAME_EXCL performs one no-replace rename without retaining pointers.
    let result = unsafe {
        renameatx_np(
            AT_FDCWD,
            source.as_ptr(),
            AT_FDCWD,
            destination.as_ptr(),
            RENAME_EXCL,
        )
    };
    if result == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

#[cfg(target_os = "linux")]
fn rename_noreplace(source: &Path, destination: &Path) -> std::io::Result<()> {
    use std::os::unix::ffi::OsStrExt;

    const AT_FDCWD: i32 = -100;
    const RENAME_NOREPLACE: u32 = 1;
    unsafe extern "C" {
        fn renameat2(
            olddirfd: i32,
            oldpath: *const std::ffi::c_char,
            newdirfd: i32,
            newpath: *const std::ffi::c_char,
            flags: u32,
        ) -> i32;
    }
    let source = CString::new(source.as_os_str().as_bytes()).map_err(|_| {
        std::io::Error::new(std::io::ErrorKind::InvalidInput, "source contains NUL")
    })?;
    let destination = CString::new(destination.as_os_str().as_bytes()).map_err(|_| {
        std::io::Error::new(std::io::ErrorKind::InvalidInput, "destination contains NUL")
    })?;
    // SAFETY: both C strings live for this no-retention system call.
    let result = unsafe {
        renameat2(
            AT_FDCWD,
            source.as_ptr(),
            AT_FDCWD,
            destination.as_ptr(),
            RENAME_NOREPLACE,
        )
    };
    if result == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn rename_noreplace(_source: &Path, _destination: &Path) -> std::io::Result<()> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "no-replace prepared publication is not implemented on this platform",
    ))
}

fn reconcile_existing_projection(existing: &Path, staged: &Path) -> Result<(), PublishError> {
    require_private_directory(existing, "existing prepared output")?;
    let mut expected_entries = BTreeSet::from([MANIFEST_NAME.to_owned()]);
    expected_entries.extend(PACKAGES.iter().map(|package| (*package).to_owned()));
    let mut existing_entries = directory_entries(existing)?;
    let update_was_interrupted = existing_entries.remove(UPDATE_MARKER_NAME);
    if existing_entries != expected_entries || directory_entries(staged)? != expected_entries {
        return Err(error("existing prepared output has noncanonical topology"));
    }
    let staged_manifest = staged.join(MANIFEST_NAME);
    let target_manifest = read_private_regular(&staged_manifest)?;
    let marker = existing.join(UPDATE_MARKER_NAME);
    let marker_bytes = format!(
        "schema fn64.wm-prepared-shard-update.v1\ntarget_manifest_sha256 {}\n",
        hex(Sha256::digest(&target_manifest).into())
    )
    .into_bytes();
    if update_was_interrupted {
        if read_private_regular(&marker)? != marker_bytes {
            return Err(error(
                "interrupted prepared update targets a different manifest; recover it first",
            ));
        }
    } else {
        write_new_private(&marker, &marker_bytes)?;
        sync_directory(existing)?;
    }
    for package in PACKAGES {
        let existing_package = existing.join(package);
        let staged_package = staged.join(package);
        require_private_directory(&existing_package, "existing prepared package")?;
        require_private_directory(&staged_package, "staged prepared package")?;
        let expected_files = BTreeSet::from([
            IDENTITY_NAME.to_owned(),
            "metadata.rs".to_owned(),
            "runner.rs".to_owned(),
        ]);
        if directory_entries(&existing_package)? != expected_files
            || directory_entries(&staged_package)? != expected_files
        {
            return Err(error("existing prepared package has noncanonical topology"));
        }
        // Artifact bytes commit before the package sidecar. A materializer
        // racing any prefix sees the update marker and fails; without the
        // marker, an old sidecar against new content also fails its digest.
        for name in ["runner.rs", "metadata.rs", IDENTITY_NAME] {
            replace_if_changed(&existing_package.join(name), &staged_package.join(name))?;
        }
    }
    let existing_manifest = existing.join(MANIFEST_NAME);
    // Root authority commits after every package source/sidecar pair.
    replace_if_changed(&existing_manifest, &staged_manifest)?;
    fs::remove_file(&marker).map_err(|_| error("cannot clear prepared update marker"))?;
    sync_directory(existing)?;
    Ok(())
}

fn replace_if_changed(existing: &Path, staged: &Path) -> Result<bool, PublishError> {
    if read_private_regular(existing)? == read_private_regular(staged)? {
        return Ok(false);
    }
    fs::rename(staged, existing).map_err(|_| error("cannot atomically replace prepared file"))?;
    sync_directory(existing.parent().expect("prepared file has parent"))?;
    Ok(true)
}

fn require_private_directory(path: &Path, label: &str) -> Result<(), PublishError> {
    let metadata =
        fs::symlink_metadata(path).map_err(|_| error(format!("cannot inspect {label}")))?;
    require_private_directory_metadata(&metadata, label)
}

fn require_private_directory_metadata(
    metadata: &fs::Metadata,
    label: &str,
) -> Result<(), PublishError> {
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(error(format!("{label} must be a non-symlink directory")));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o777 != 0o700 {
            return Err(error(format!("{label} must have mode 0700")));
        }
    }
    Ok(())
}

fn read_private_regular(path: &Path) -> Result<Vec<u8>, PublishError> {
    let metadata =
        fs::symlink_metadata(path).map_err(|_| error("cannot inspect existing prepared file"))?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(error("prepared artifact is not a regular non-symlink file"));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o777 != 0o600 {
            return Err(error("prepared artifact must have mode 0600"));
        }
    }
    fs::read(path).map_err(|_| error("cannot read prepared file"))
}

fn directory_entries(path: &Path) -> Result<BTreeSet<String>, PublishError> {
    let mut entries = BTreeSet::new();
    for entry in fs::read_dir(path).map_err(|_| error("cannot enumerate prepared directory"))? {
        let entry = entry.map_err(|_| error("cannot enumerate prepared directory"))?;
        entries.insert(
            entry
                .file_name()
                .into_string()
                .map_err(|_| error("prepared entry name is not UTF-8"))?,
        );
    }
    Ok(entries)
}

fn hex(bytes: [u8; 32]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(64);
    for byte in bytes {
        output.push(HEX[(byte >> 4) as usize] as char);
        output.push(HEX[(byte & 0x0f) as usize] as char);
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NONCE: AtomicU64 = AtomicU64::new(0);

    struct Fixture {
        repository: PathBuf,
        parent: PathBuf,
        output: PathBuf,
    }

    impl Fixture {
        fn new() -> Self {
            let nonce = NONCE.fetch_add(1, Ordering::Relaxed);
            let parent = std::env::temp_dir().canonicalize().unwrap().join(format!(
                "fn64-prepared-publisher-test-{}-{nonce}",
                std::process::id()
            ));
            let repository = parent.join("repository");
            let outside = parent.join("outside");
            fs::create_dir(&parent).unwrap();
            fs::create_dir(&repository).unwrap();
            fs::create_dir(&outside).unwrap();
            Self {
                repository,
                output: outside.join("prepared"),
                parent,
            }
        }

        fn claims() -> SourceIdentityClaims {
            SourceIdentityClaims {
                generator_source_sha256: [0x11; 32],
                discovery_source_sha256: [0x22; 32],
                emitter_source_sha256: [0x33; 32],
                runtime_source_sha256: [0x44; 32],
            }
        }

        fn begin(&self, output: &Path) -> Result<PreparedTreePublication, PublishError> {
            self.begin_with_claims(output, Self::claims())
        }

        fn begin_with_claims(
            &self,
            output: &Path,
            claims: SourceIdentityClaims,
        ) -> Result<PreparedTreePublication, PublishError> {
            PreparedTreePublication::begin(output, &self.repository, [0x55; 32], claims)
        }

        fn artifact(package: &str) -> GeneratedShard {
            GeneratedShard {
                package: package.to_owned(),
                runner: format!("// synthetic runner {package}\n"),
                metadata: format!("// synthetic metadata {package}\n"),
                reuse_2k: fn64_recomp_rs_codegen::inventory_dense_body_reuse(&[], 512),
                reuse_64k: fn64_recomp_rs_codegen::inventory_dense_body_reuse(&[], 16_384),
                static_micro_op_bytes: 0,
                static_micro_op_instructions: 0,
                static_micro_op_body_sha256: [0; 32],
            }
        }

        fn publish(&self) -> Result<[u8; 32], PublishError> {
            self.publish_to(&self.output, Self::claims(), None)
        }

        fn publish_to(
            &self,
            output: &Path,
            claims: SourceIdentityClaims,
            changed_package: Option<&str>,
        ) -> Result<[u8; 32], PublishError> {
            let mut publication = self.begin_with_claims(output, claims)?;
            for package in PACKAGES {
                let mut artifact = Self::artifact(package);
                if changed_package == Some(package) {
                    artifact.runner.push_str("// changed artifact\n");
                }
                publication.push(artifact)?;
            }
            publication.finish()
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            fs::remove_dir_all(&self.parent).unwrap();
        }
    }

    #[test]
    fn streaming_publication_is_deterministic_and_idempotent() {
        let fixture = Fixture::new();
        let first = fixture.publish().unwrap();
        let manifest = fs::read(fixture.output.join(MANIFEST_NAME)).unwrap();
        assert_eq!(first, fixture.publish().unwrap());
        assert_eq!(
            manifest,
            fs::read(fixture.output.join(MANIFEST_NAME)).unwrap()
        );
        assert_eq!(directory_entries(&fixture.output).unwrap().len(), 36);
    }

    #[test]
    fn root_manifest_cross_binds_sorted_sidecar_and_artifact_digests() {
        let fixture = Fixture::new();
        fixture.publish().unwrap();
        let manifest = fs::read_to_string(fixture.output.join(MANIFEST_NAME)).unwrap();
        let lines = manifest.lines().collect::<Vec<_>>();
        assert_eq!(lines[0], format!("schema {ROOT_SCHEMA_V2}"));
        assert_eq!(lines.len(), 7 + PACKAGES.len());
        for (line, package) in lines[7..].iter().zip(PACKAGES) {
            let fields = line.split(' ').collect::<Vec<_>>();
            assert_eq!(fields.len(), 5);
            assert_eq!(fields[0], "artifact");
            assert_eq!(fields[1], package);
            for (observed, name) in
                fields[2..]
                    .iter()
                    .zip([IDENTITY_NAME, "runner.rs", "metadata.rs"])
            {
                let bytes = fs::read(fixture.output.join(package).join(name)).unwrap();
                assert_eq!(*observed, hex(Sha256::digest(bytes).into()));
            }
        }
    }

    #[test]
    fn root_claim_only_change_does_not_change_any_package_identity() {
        let fixture = Fixture::new();
        fixture
            .publish_to(&fixture.output, Fixture::claims(), None)
            .unwrap();
        let baseline_manifest = fs::read(fixture.output.join(MANIFEST_NAME)).unwrap();
        let baseline_identities = PACKAGES
            .iter()
            .map(|package| {
                (
                    *package,
                    fs::read(fixture.output.join(package).join(IDENTITY_NAME)).unwrap(),
                )
            })
            .collect::<Vec<_>>();
        let mut claims = Fixture::claims();
        claims.emitter_source_sha256 = [0x66; 32];
        fixture.publish_to(&fixture.output, claims, None).unwrap();
        assert_ne!(
            baseline_manifest,
            fs::read(fixture.output.join(MANIFEST_NAME)).unwrap()
        );
        for (package, baseline_identity) in baseline_identities {
            assert_eq!(
                baseline_identity,
                fs::read(fixture.output.join(package).join(IDENTITY_NAME)).unwrap(),
            );
        }
    }

    #[test]
    fn one_artifact_change_changes_only_its_package_identity() {
        let fixture = Fixture::new();
        let selected = PACKAGES[24];
        fixture
            .publish_to(&fixture.output, Fixture::claims(), None)
            .unwrap();
        let baseline_identities = PACKAGES
            .iter()
            .map(|package| {
                (
                    *package,
                    fs::read(fixture.output.join(package).join(IDENTITY_NAME)).unwrap(),
                )
            })
            .collect::<Vec<_>>();
        #[cfg(unix)]
        let untouched_stamps = [
            fixture.output.join(PACKAGES[0]).join(IDENTITY_NAME),
            fixture.output.join(PACKAGES[0]).join("runner.rs"),
            fixture.output.join(PACKAGES[0]).join("metadata.rs"),
            fixture.output.join(selected).join("metadata.rs"),
        ]
        .map(|path| (file_stamp(&path), path));
        fixture
            .publish_to(&fixture.output, Fixture::claims(), Some(selected))
            .unwrap();
        let changed_identities = baseline_identities
            .into_iter()
            .filter(|(package, identity)| {
                let observed = fs::read(fixture.output.join(*package).join(IDENTITY_NAME)).unwrap();
                identity != &observed
            })
            .map(|(package, _)| package)
            .collect::<Vec<_>>();
        assert_eq!(changed_identities, vec![selected]);
        #[cfg(unix)]
        for (stamp, path) in untouched_stamps {
            assert_eq!(stamp, file_stamp(&path));
        }
    }

    #[test]
    fn interrupted_update_prefixes_fail_materialization_and_rerun_recover() {
        for prefix in 0..4 {
            let fixture = Fixture::new();
            let candidate = fixture.output.with_file_name("prepared-crash-candidate");
            let selected = PACKAGES[24];
            fixture.publish().unwrap();
            fixture
                .publish_to(&candidate, Fixture::claims(), Some(selected))
                .unwrap();
            let target_manifest = fs::read(candidate.join(MANIFEST_NAME)).unwrap();
            let marker_bytes = format!(
                "schema fn64.wm-prepared-shard-update.v1\ntarget_manifest_sha256 {}\n",
                hex(Sha256::digest(target_manifest).into())
            );
            write_new_private(
                &fixture.output.join(UPDATE_MARKER_NAME),
                marker_bytes.as_bytes(),
            )
            .unwrap();

            let selected_files = match prefix {
                0 => &['r'; 1][..],
                1 => &['r', 'm'][..],
                _ => &['r', 'm', 'i'][..],
            };
            for file in selected_files {
                let name = match file {
                    'r' => "runner.rs",
                    'm' => "metadata.rs",
                    'i' => IDENTITY_NAME,
                    _ => unreachable!(),
                };
                fs::rename(
                    candidate.join(selected).join(name),
                    fixture.output.join(selected).join(name),
                )
                .unwrap();
            }
            if prefix == 3 {
                for package in PACKAGES {
                    if package == selected {
                        continue;
                    }
                    for name in ["runner.rs", "metadata.rs", IDENTITY_NAME] {
                        fs::rename(
                            candidate.join(package).join(name),
                            fixture.output.join(package).join(name),
                        )
                        .unwrap();
                    }
                }
                fs::rename(
                    candidate.join(MANIFEST_NAME),
                    fixture.output.join(MANIFEST_NAME),
                )
                .unwrap();
            }

            let materialized = fixture.parent.join("materialized-crash-probe");
            fs::create_dir(&materialized).unwrap();
            assert!(crate::materializer::materialize_package(
                &fixture.output,
                selected,
                &materialized,
            )
            .is_err());
            fixture
                .publish_to(&fixture.output, Fixture::claims(), Some(selected))
                .unwrap();
            assert!(!fixture.output.join(UPDATE_MARKER_NAME).exists());
            crate::materializer::materialize_package(&fixture.output, selected, &materialized)
                .unwrap();
            assert_eq!(
                fs::read(materialized.join("runner.rs")).unwrap(),
                Fixture::artifact(selected)
                    .runner
                    .into_bytes()
                    .into_iter()
                    .chain(b"// changed artifact\n".iter().copied())
                    .collect::<Vec<_>>()
            );
        }
    }

    #[cfg(unix)]
    fn file_stamp(path: &Path) -> (u64, std::time::SystemTime) {
        use std::os::unix::fs::MetadataExt;
        let metadata = fs::metadata(path).unwrap();
        (metadata.ino(), metadata.modified().unwrap())
    }

    #[test]
    fn canonical_stream_rejects_wrong_order_and_missing_tail() {
        let fixture = Fixture::new();
        let mut wrong = fixture.begin(&fixture.output).unwrap();
        assert!(wrong.push(Fixture::artifact(PACKAGES[1])).is_err());
        drop(wrong);
        let mut missing = fixture.begin(&fixture.output).unwrap();
        for package in &PACKAGES[..PACKAGES.len() - 1] {
            missing.push(Fixture::artifact(package)).unwrap();
        }
        assert!(missing.finish().is_err());
        assert!(!fixture.output.exists());
    }

    #[test]
    fn repository_relative_and_partial_destinations_fail_closed() {
        let fixture = Fixture::new();
        assert!(fixture.begin(&fixture.repository.join("prepared")).is_err());
        assert!(PreparedTreePublication::begin(
            Path::new("relative"),
            &fixture.repository,
            [0x55; 32],
            Fixture::claims(),
        )
        .is_err());
        create_private_directory(&fixture.output).unwrap();
        write_new_private(&fixture.output.join("partial"), b"incomplete").unwrap();
        assert!(fixture.publish().is_err());
        assert_eq!(
            fs::read(fixture.output.join("partial")).unwrap(),
            b"incomplete"
        );
    }

    #[test]
    fn existing_empty_destination_is_not_replaced() {
        let fixture = Fixture::new();
        create_private_directory(&fixture.output).unwrap();
        assert!(fixture.publish().is_err());
        assert!(directory_entries(&fixture.output).unwrap().is_empty());
    }

    #[test]
    fn extra_existing_topology_fails_closed_and_artifact_corruption_is_repaired() {
        let fixture = Fixture::new();
        fixture.publish().unwrap();
        write_new_private(&fixture.output.join("extra"), b"rejected").unwrap();
        assert!(fixture.publish().is_err());
        fs::remove_file(fixture.output.join("extra")).unwrap();
        let runner = fixture.output.join(PACKAGES[0]).join("runner.rs");
        fs::write(&runner, b"changed").unwrap();
        fixture.publish().unwrap();
        assert_eq!(
            fs::read(runner).unwrap(),
            Fixture::artifact(PACKAGES[0]).runner.as_bytes()
        );
    }

    #[cfg(unix)]
    #[test]
    fn published_tree_is_private() {
        use std::os::unix::fs::PermissionsExt;
        let fixture = Fixture::new();
        fixture.publish().unwrap();
        assert_eq!(
            fs::metadata(&fixture.output).unwrap().permissions().mode() & 0o777,
            0o700
        );
        assert_eq!(
            fs::metadata(fixture.output.join(MANIFEST_NAME))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
        assert_eq!(
            fs::metadata(fixture.output.join(PACKAGES[0]).join("runner.rs"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
        assert_eq!(
            fs::metadata(fixture.output.join(PACKAGES[0]).join(IDENTITY_NAME))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }
}
