//! Race-resistant local filesystem admission for private release evidence.
//!
//! The retained path is policy data, while the digest and metadata come from
//! one no-follow descriptor or handle. A writer able to rename an external
//! directory ancestor and restore its identity remains outside this local,
//! single-owner boundary, matching `docs/PRIVATE-INPUT-ADMISSION.md`.

use sha2::{Digest, Sha256};
use std::{
    ffi::{OsStr, OsString},
    fs,
    io::Read,
    path::{Component, Path, PathBuf},
    process::{Command, Stdio},
};

const READ_BLOCK_BYTES: usize = 1024 * 1024;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum StableObjectId {
    #[cfg(unix)]
    Unix { device: u64, inode: u64 },
    #[cfg(windows)]
    Windows {
        volume_serial_number: u64,
        file_id: [u8; 16],
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct StableFileMeasurement {
    pub(crate) path: PathBuf,
    pub(crate) bytes: u64,
    pub(crate) sha256: String,
    pub(crate) object_id: StableObjectId,
    /// Captured from the verified open descriptor, never from a later path
    /// lookup. Windows native-image admission does not use Unix mode bits.
    pub(crate) unix_mode: Option<u32>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct StableFileRead {
    pub(crate) measurement: StableFileMeasurement,
    pub(crate) contents: Vec<u8>,
}

pub(crate) enum StableFileStream<'a> {
    Length(u64),
    Chunk(&'a [u8]),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct CheckedDirectory {
    pub(crate) path: PathBuf,
    pub(crate) object_id: StableObjectId,
}

#[derive(Clone, Debug)]
pub(crate) struct PrivateRepository {
    root: CheckedDirectory,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum EntryKind {
    Regular,
    Directory,
    SymlinkOrReparse,
    Other,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct PathEntry {
    object_id: StableObjectId,
    kind: EntryKind,
}

pub(crate) fn validate_absolute_no_parent(path: &Path, field: &str) -> Result<(), String> {
    if path.as_os_str().is_empty()
        || !path.is_absolute()
        || path
            .components()
            .any(|component| matches!(component, Component::ParentDir))
    {
        return Err(format!(
            "{field} {} must be absolute, nonempty, and contain no '..' component",
            path.display()
        ));
    }
    #[cfg(windows)]
    validate_windows_prefix(path, field)?;
    Ok(())
}

pub(crate) fn same_lexical_path(left: &Path, right: &Path) -> bool {
    left.as_os_str() == right.as_os_str()
}

pub(crate) fn measure_regular_stable(
    path: &Path,
    field: &str,
) -> Result<StableFileMeasurement, String> {
    measure_regular_stable_impl(path, field, false, || {}, |_| Ok(()))
        .map(|result| result.measurement)
}

pub(crate) fn measure_regular_stable_with<F>(
    path: &Path,
    field: &str,
    observer: F,
) -> Result<StableFileMeasurement, String>
where
    F: for<'a> FnMut(StableFileStream<'a>) -> Result<(), String>,
{
    measure_regular_stable_impl(path, field, false, || {}, observer)
        .map(|result| result.measurement)
}

pub(crate) fn read_regular_stable(path: &Path, field: &str) -> Result<StableFileRead, String> {
    measure_regular_stable_impl(path, field, true, || {}, |_| Ok(()))
}

fn measure_regular_stable_impl<F, O>(
    path: &Path,
    field: &str,
    retain_contents: bool,
    after_open: F,
    mut observer: O,
) -> Result<StableFileRead, String>
where
    F: FnOnce(),
    O: for<'a> FnMut(StableFileStream<'a>) -> Result<(), String>,
{
    validate_absolute_no_parent(path, field)?;
    let mut opened = platform::OpenedRegular::open(path, field)?;
    observer(StableFileStream::Length(opened.expected_bytes()))?;

    // Exact closed interleaving: validation opens object A, another process
    // replaces the path with B, and hashing continues through A's descriptor.
    // `finish` reopens the complete path chain and rejects B after the read.
    after_open();

    let mut digest = Sha256::new();
    let mut contents = Vec::new();
    let mut observed_bytes = 0u64;
    let mut buffer = [0u8; READ_BLOCK_BYTES];
    loop {
        let read = opened.file_mut().read(&mut buffer).map_err(|source| {
            format!(
                "read {field} {} from retained handle: {source}",
                path.display()
            )
        })?;
        if read == 0 {
            break;
        }
        observed_bytes = observed_bytes
            .checked_add(u64::try_from(read).expect("bounded read length fits u64"))
            .ok_or_else(|| format!("{field} {} length overflow", path.display()))?;
        digest.update(&buffer[..read]);
        observer(StableFileStream::Chunk(&buffer[..read]))?;
        if retain_contents {
            contents.extend_from_slice(&buffer[..read]);
        }
    }
    let snapshot = opened.finish(observed_bytes, field)?;
    let measurement = StableFileMeasurement {
        path: path.to_path_buf(),
        bytes: observed_bytes,
        sha256: hex(&digest.finalize()),
        object_id: snapshot.object_id,
        unix_mode: snapshot.unix_mode,
    };
    Ok(StableFileRead {
        measurement,
        contents,
    })
}

pub(crate) fn check_directory_nofollow(
    path: &Path,
    field: &str,
) -> Result<CheckedDirectory, String> {
    validate_absolute_no_parent(path, field)?;
    let object_id = platform::check_directory(path, field)?;
    Ok(CheckedDirectory {
        path: path.to_path_buf(),
        object_id,
    })
}

impl PrivateRepository {
    pub(crate) fn discover() -> Result<Self, String> {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .canonicalize()
            .map_err(|source| format!("resolve fn64 repository root: {source}"))?;
        Self::from_root(&root)
    }

    pub(crate) fn from_root(root: &Path) -> Result<Self, String> {
        Ok(Self {
            root: check_directory_nofollow(root, "fn64 repository root")?,
        })
    }

    pub(crate) fn root(&self) -> &Path {
        &self.root.path
    }

    /// Returns the identity-derived on-disk spelling below the repository.
    /// `None` means the path is outside the repository.
    pub(crate) fn filesystem_relative_to(
        &self,
        path: &Path,
        field: &str,
    ) -> Result<Option<PathBuf>, String> {
        validate_absolute_no_parent(path, field)?;
        let retained_root = check_directory_nofollow(&self.root.path, "fn64 repository root")?;
        if retained_root.object_id != self.root.object_id {
            return Err("fn64 repository root changed during private-path admission".to_owned());
        }

        let mut current = path.to_path_buf();
        let mut missing = Vec::<OsString>::new();
        let mut current_entry = loop {
            match fs::symlink_metadata(&current) {
                Ok(_) => break platform::inspect_path_entry(&current, field)?,
                Err(source) if source.kind() == std::io::ErrorKind::NotFound => {
                    let parent = current.parent().ok_or_else(|| {
                        format!(
                            "cannot locate an existing ancestor of {field} {}",
                            path.display()
                        )
                    })?;
                    if parent == current {
                        return Err(format!(
                            "cannot locate an existing ancestor of {field} {}",
                            path.display()
                        ));
                    }
                    missing.push(
                        current
                            .file_name()
                            .ok_or_else(|| {
                                format!("{field} {} has no file name", current.display())
                            })?
                            .to_os_string(),
                    );
                    current = parent.to_path_buf();
                }
                Err(source) => {
                    return Err(format!(
                        "inspect {field} ancestor {}: {source}",
                        current.display()
                    ));
                }
            }
        };
        if current_entry.kind == EntryKind::SymlinkOrReparse {
            return Err(format!(
                "{field} has forbidden symlink or reparse component {}",
                current.display()
            ));
        }

        let mut stored_components = Vec::<OsString>::new();
        while current_entry.object_id != self.root.object_id {
            let Some(parent) = current.parent() else {
                return Ok(None);
            };
            if parent == current {
                return Ok(None);
            }
            let parent_entry = platform::inspect_path_entry(parent, field)?;
            if parent_entry.kind == EntryKind::SymlinkOrReparse {
                return Err(format!(
                    "{field} has forbidden symlink or reparse component {}",
                    parent.display()
                ));
            }
            stored_components.push(stored_entry_name(
                parent,
                current
                    .file_name()
                    .ok_or_else(|| format!("{field} {} has no file name", current.display()))?,
                &current_entry.object_id,
                field,
            )?);
            current = parent.to_path_buf();
            current_entry = parent_entry;
        }

        stored_components.reverse();
        missing.reverse();
        let mut relative = PathBuf::new();
        relative.extend(stored_components);
        relative.extend(missing);
        Ok(Some(relative))
    }

    pub(crate) fn require_outside_or_gitignored(
        &self,
        path: &Path,
        field: &str,
    ) -> Result<(), String> {
        let Some(relative) = self.filesystem_relative_to(path, field)? else {
            return Ok(());
        };
        if run_git_path_check(
            self.root(),
            [
                OsStr::new("ls-files"),
                OsStr::new("--error-unmatch"),
                OsStr::new("--"),
            ],
            &relative,
            "ls-files",
        )? == 0
        {
            return Err(format!(
                "{field} {} is tracked by git; private evidence cannot enter the repository",
                path.display()
            ));
        }
        if run_git_path_check(
            self.root(),
            [
                OsStr::new("check-ignore"),
                OsStr::new("-q"),
                OsStr::new("--no-index"),
                OsStr::new("--"),
            ],
            &relative,
            "check-ignore",
        )? != 0
        {
            return Err(format!(
                "{field} {} is inside the repository and not gitignored",
                path.display()
            ));
        }
        Ok(())
    }
}

fn stored_entry_name(
    parent: &Path,
    requested_name: &OsStr,
    child_id: &StableObjectId,
    field: &str,
) -> Result<OsString, String> {
    let entries = fs::read_dir(parent).map_err(|source| {
        format!(
            "inspect repository path component {} for {field}: {source}",
            parent.display()
        )
    })?;
    let mut matches = Vec::<OsString>::new();
    for entry in entries {
        let Ok(entry) = entry else { continue };
        let Ok(candidate) = platform::inspect_path_entry(&entry.path(), field) else {
            continue;
        };
        if candidate.object_id == *child_id {
            matches.push(entry.file_name());
        }
    }
    if matches.is_empty() {
        return Err(format!(
            "repository path changed while inspecting {field} component {}",
            parent.join(requested_name).display()
        ));
    }
    if matches.iter().any(|name| name == requested_name) {
        return Ok(requested_name.to_os_string());
    }
    let case_matches: Vec<_> = matches
        .iter()
        .filter(|name| os_casefold_eq(name, requested_name))
        .collect();
    if case_matches.len() == 1 {
        return Ok(case_matches[0].to_os_string());
    }
    if matches.len() == 1 {
        return Ok(matches.remove(0));
    }
    Err(format!(
        "ambiguous hard-linked repository path component {}",
        parent.join(requested_name).display()
    ))
}

fn os_casefold_eq(left: &OsStr, right: &OsStr) -> bool {
    match (left.to_str(), right.to_str()) {
        (Some(left), Some(right)) => left.to_lowercase() == right.to_lowercase(),
        _ => false,
    }
}

fn run_git_path_check<const N: usize>(
    root: &Path,
    arguments: [&OsStr; N],
    relative: &Path,
    operation: &str,
) -> Result<i32, String> {
    let status = Command::new("git")
        .args(arguments)
        .arg(relative)
        .current_dir(root)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map_err(|source| format!("run git {operation}: {source}"))?;
    let code = status
        .code()
        .ok_or_else(|| format!("git {operation} terminated without an exit status"))?;
    if !matches!(code, 0 | 1) {
        return Err(format!("git {operation} failed with status {code}"));
    }
    Ok(code)
}

fn hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        output.push(char::from(HEX[usize::from(byte >> 4)]));
        output.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    output
}

#[cfg(unix)]
mod platform {
    use super::{EntryKind, PathEntry, StableObjectId};
    use rustix::{
        fd::OwnedFd,
        fs::{self as rfs, AtFlags, FileType, Mode, OFlags},
    };
    use std::{
        ffi::OsString,
        fs::File,
        path::{Path, PathBuf},
    };

    #[derive(Clone, Debug, PartialEq, Eq)]
    struct Snapshot {
        object_id: StableObjectId,
        mode: u32,
        bytes: u64,
        modified_seconds: i64,
        modified_nanoseconds: i64,
        changed_seconds: i64,
        changed_nanoseconds: i64,
    }

    pub(super) struct FinishedSnapshot {
        pub(super) object_id: StableObjectId,
        pub(super) unix_mode: Option<u32>,
    }

    pub(super) struct OpenedRegular {
        path: PathBuf,
        file_name: OsString,
        parent: OwnedFd,
        parent_chain: Vec<StableObjectId>,
        file: File,
        before: Snapshot,
    }

    impl OpenedRegular {
        pub(super) fn open(path: &Path, field: &str) -> Result<Self, String> {
            let (parent, parent_chain) = open_parent(path, field)?;
            let file_name = path
                .file_name()
                .ok_or_else(|| format!("{field} {} has no file name", path.display()))?
                .to_os_string();
            let descriptor = rfs::openat(
                &parent,
                &file_name,
                OFlags::RDONLY | OFlags::CLOEXEC | OFlags::NOFOLLOW | OFlags::NONBLOCK,
                Mode::empty(),
            )
            .map_err(|source| {
                if entry_is_symlink(&parent, &file_name) {
                    format!("{field} has forbidden symlink component {}", path.display())
                } else {
                    format!(
                        "open {field} {} without following links: {source}",
                        path.display()
                    )
                }
            })?;
            let before =
                snapshot(&rfs::fstat(&descriptor).map_err(|source| {
                    format!("inspect open {field} {}: {source}", path.display())
                })?)?;
            if FileType::from_raw_mode(before.mode as rfs::RawMode) != FileType::RegularFile {
                return Err(format!("{field} {} is not a regular file", path.display()));
            }
            let file = File::from(descriptor);
            Ok(Self {
                path: path.to_path_buf(),
                file_name,
                parent,
                parent_chain,
                file,
                before,
            })
        }

        pub(super) fn file_mut(&mut self) -> &mut File {
            &mut self.file
        }

        pub(super) fn expected_bytes(&self) -> u64 {
            self.before.bytes
        }

        pub(super) fn finish(
            &self,
            observed_bytes: u64,
            field: &str,
        ) -> Result<FinishedSnapshot, String> {
            let after = snapshot(&rfs::fstat(&self.file).map_err(|source| {
                format!("reinspect open {field} {}: {source}", self.path.display())
            })?)?;
            let path_after = snapshot(
                &rfs::statat(&self.parent, &self.file_name, AtFlags::SYMLINK_NOFOLLOW).map_err(
                    |source| {
                        format!(
                            "reinspect {field} {} by parent handle: {source}",
                            self.path.display()
                        )
                    },
                )?,
            )?;
            let (_, reopened_chain) = open_parent(&self.path, field)?;
            if self.before != after
                || after != path_after
                || self.parent_chain != reopened_chain
                || observed_bytes != after.bytes
            {
                return Err(format!(
                    "{field} {} changed while it was being measured",
                    self.path.display()
                ));
            }
            Ok(FinishedSnapshot {
                object_id: after.object_id,
                unix_mode: Some(after.mode),
            })
        }
    }

    pub(super) fn check_directory(path: &Path, field: &str) -> Result<StableObjectId, String> {
        let (_, before_chain) = open_directory_path(path, field)?;
        let (_, after_chain) = open_directory_path(path, field)?;
        if before_chain != after_chain {
            return Err(format!(
                "{field} {} changed while it was inspected",
                path.display()
            ));
        }
        before_chain
            .last()
            .cloned()
            .ok_or_else(|| format!("{field} {} has no directory identity", path.display()))
    }

    pub(super) fn inspect_path_entry(path: &Path, field: &str) -> Result<PathEntry, String> {
        use std::os::unix::fs::MetadataExt as _;

        let metadata = std::fs::symlink_metadata(path)
            .map_err(|source| format!("inspect {field} component {}: {source}", path.display()))?;
        let file_type = metadata.file_type();
        let kind = if file_type.is_file() {
            EntryKind::Regular
        } else if file_type.is_dir() {
            EntryKind::Directory
        } else if file_type.is_symlink() {
            EntryKind::SymlinkOrReparse
        } else {
            EntryKind::Other
        };
        Ok(PathEntry {
            object_id: StableObjectId::Unix {
                device: metadata.dev(),
                inode: metadata.ino(),
            },
            kind,
        })
    }

    fn open_parent(path: &Path, field: &str) -> Result<(OwnedFd, Vec<StableObjectId>), String> {
        let parent = path
            .parent()
            .ok_or_else(|| format!("{field} {} has no parent", path.display()))?;
        open_directory_path(parent, field)
    }

    fn open_directory_path(
        path: &Path,
        field: &str,
    ) -> Result<(OwnedFd, Vec<StableObjectId>), String> {
        let flags = OFlags::RDONLY | OFlags::CLOEXEC | OFlags::NOFOLLOW | OFlags::DIRECTORY;
        let mut descriptor = rfs::openat(rfs::ABS, Path::new("/"), flags, Mode::empty())
            .map_err(|source| format!("open filesystem root for {field}: {source}"))?;
        let mut chain = vec![
            snapshot(
                &rfs::fstat(&descriptor)
                    .map_err(|source| format!("inspect filesystem root for {field}: {source}"))?,
            )?
            .object_id,
        ];
        for component in path.components() {
            let std::path::Component::Normal(name) = component else {
                continue;
            };
            let next = rfs::openat(&descriptor, name, flags, Mode::empty()).map_err(|source| {
                if entry_is_symlink(&descriptor, name) {
                    format!(
                        "{field} has forbidden symlink component {}",
                        name.to_string_lossy()
                    )
                } else {
                    format!(
                        "open {field} directory component {} without following links: {source}",
                        name.to_string_lossy()
                    )
                }
            })?;
            let next_snapshot = snapshot(&rfs::fstat(&next).map_err(|source| {
                format!(
                    "inspect {field} directory component {}: {source}",
                    name.to_string_lossy()
                )
            })?)?;
            if FileType::from_raw_mode(next_snapshot.mode as rfs::RawMode) != FileType::Directory {
                return Err(format!(
                    "{field} directory component {} is not a directory",
                    name.to_string_lossy()
                ));
            }
            chain.push(next_snapshot.object_id);
            descriptor = next;
        }
        Ok((descriptor, chain))
    }

    fn entry_is_symlink(directory: &OwnedFd, name: &std::ffi::OsStr) -> bool {
        rfs::statat(directory, name, AtFlags::SYMLINK_NOFOLLOW)
            .is_ok_and(|stat| FileType::from_raw_mode(stat.st_mode) == FileType::Symlink)
    }

    // rustix deliberately normalizes field names, not field widths; these
    // conversions are fallible on some Unix targets and identities on others.
    #[allow(clippy::unnecessary_fallible_conversions, clippy::useless_conversion)]
    fn snapshot(stat: &rfs::Stat) -> Result<Snapshot, String> {
        let bytes = u64::try_from(stat.st_size)
            .map_err(|_| "filesystem object reports a negative or overflowing length".to_owned())?;
        Ok(Snapshot {
            object_id: StableObjectId::Unix {
                device: u64::try_from(stat.st_dev)
                    .map_err(|_| "filesystem object reports an invalid device identity")?,
                inode: u64::try_from(stat.st_ino)
                    .map_err(|_| "filesystem object reports an invalid inode identity")?,
            },
            mode: u32::try_from(stat.st_mode)
                .map_err(|_| "filesystem object reports an invalid mode")?,
            bytes,
            modified_seconds: i64::try_from(stat.st_mtime)
                .map_err(|_| "filesystem object reports an invalid modification time")?,
            modified_nanoseconds: i64::try_from(stat.st_mtime_nsec)
                .map_err(|_| "filesystem object reports invalid modification nanoseconds")?,
            changed_seconds: i64::try_from(stat.st_ctime)
                .map_err(|_| "filesystem object reports an invalid change time")?,
            changed_nanoseconds: i64::try_from(stat.st_ctime_nsec)
                .map_err(|_| "filesystem object reports invalid change nanoseconds")?,
        })
    }
}

#[cfg(windows)]
fn validate_windows_prefix(path: &Path, field: &str) -> Result<(), String> {
    use std::path::Prefix;

    match path.components().next() {
        Some(Component::Prefix(prefix))
            if matches!(
                prefix.kind(),
                Prefix::Disk(_)
                    | Prefix::UNC(_, _)
                    | Prefix::VerbatimDisk(_)
                    | Prefix::VerbatimUNC(_, _)
            ) =>
        {
            Ok(())
        }
        _ => Err(format!(
            "{field} {} must name an absolute disk or UNC filesystem path",
            path.display()
        )),
    }
}

#[cfg(windows)]
mod platform {
    use super::{EntryKind, PathEntry, StableObjectId};
    use std::{
        fs::File,
        mem::{size_of, MaybeUninit},
        os::windows::{
            ffi::OsStrExt as _,
            io::{AsRawHandle as _, FromRawHandle as _},
        },
        path::{Component, Path, PathBuf},
        ptr,
    };
    use windows_sys::Win32::{
        Foundation::{GENERIC_READ, HANDLE, INVALID_HANDLE_VALUE},
        Storage::FileSystem::{
            CreateFileW, FileBasicInfo, FileIdInfo, FileStandardInfo, GetFileInformationByHandleEx,
            FILE_ATTRIBUTE_DEVICE, FILE_ATTRIBUTE_REPARSE_POINT, FILE_BASIC_INFO,
            FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, FILE_ID_INFO,
            FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE, FILE_STANDARD_INFO,
            OPEN_EXISTING,
        },
    };

    #[derive(Clone, Debug, PartialEq, Eq)]
    struct Snapshot {
        object_id: StableObjectId,
        creation_time: i64,
        last_access_time: i64,
        last_write_time: i64,
        change_time: i64,
        attributes: u32,
        allocation_size: i64,
        end_of_file: i64,
        number_of_links: u32,
        delete_pending: bool,
        directory: bool,
    }

    pub(super) struct FinishedSnapshot {
        pub(super) object_id: StableObjectId,
        pub(super) unix_mode: Option<u32>,
    }

    pub(super) struct OpenedRegular {
        path: PathBuf,
        file: File,
        before: Snapshot,
        before_chain: Vec<StableObjectId>,
    }

    impl OpenedRegular {
        pub(super) fn open(path: &Path, field: &str) -> Result<Self, String> {
            let before_chain = inspect_component_chain(path, false, field)?;
            let file = open_file(path, GENERIC_READ, false, field)?;
            let before = query_snapshot(&file, field, path)?;
            require_regular(&before, field, path)?;
            let confirmed_chain = inspect_component_chain(path, false, field)?;
            if before_chain != confirmed_chain {
                return Err(format!(
                    "{field} {} changed between component inspection and open",
                    path.display()
                ));
            }
            Ok(Self {
                path: path.to_path_buf(),
                file,
                before,
                before_chain,
            })
        }

        pub(super) fn file_mut(&mut self) -> &mut File {
            &mut self.file
        }

        pub(super) fn expected_bytes(&self) -> u64 {
            u64::try_from(self.before.end_of_file)
                .expect("regular Windows file length was validated as nonnegative")
        }

        pub(super) fn finish(
            &self,
            observed_bytes: u64,
            field: &str,
        ) -> Result<FinishedSnapshot, String> {
            let after = query_snapshot(&self.file, field, &self.path)?;
            let path_file = open_file(&self.path, GENERIC_READ, false, field)?;
            let path_after = query_snapshot(&path_file, field, &self.path)?;
            let after_chain = inspect_component_chain(&self.path, false, field)?;
            if self.before != after
                || after != path_after
                || self.before_chain != after_chain
                || observed_bytes != u64::try_from(after.end_of_file).unwrap_or(u64::MAX)
            {
                return Err(format!(
                    "{field} {} changed while it was being measured",
                    self.path.display()
                ));
            }
            Ok(FinishedSnapshot {
                object_id: after.object_id,
                unix_mode: None,
            })
        }
    }

    pub(super) fn check_directory(path: &Path, field: &str) -> Result<StableObjectId, String> {
        let before = inspect_component_chain(path, true, field)?;
        let after = inspect_component_chain(path, true, field)?;
        if before != after {
            return Err(format!(
                "{field} {} changed while it was inspected",
                path.display()
            ));
        }
        before
            .last()
            .cloned()
            .ok_or_else(|| format!("{field} {} has no directory identity", path.display()))
    }

    pub(super) fn inspect_path_entry(path: &Path, field: &str) -> Result<PathEntry, String> {
        let file = open_file(path, 0, true, field)?;
        let snapshot = query_snapshot(&file, field, path)?;
        let kind = if snapshot.attributes & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            EntryKind::SymlinkOrReparse
        } else if snapshot.directory {
            EntryKind::Directory
        } else if snapshot.attributes & FILE_ATTRIBUTE_DEVICE != 0 {
            EntryKind::Other
        } else {
            EntryKind::Regular
        };
        Ok(PathEntry {
            object_id: snapshot.object_id,
            kind,
        })
    }

    fn inspect_component_chain(
        path: &Path,
        include_leaf: bool,
        field: &str,
    ) -> Result<Vec<StableObjectId>, String> {
        let components: Vec<_> = path.components().collect();
        let last_normal = components
            .iter()
            .rposition(|component| matches!(component, Component::Normal(_)));
        let mut current = PathBuf::new();
        let mut chain = Vec::new();
        for (index, component) in components.into_iter().enumerate() {
            current.push(component.as_os_str());
            if !matches!(component, Component::RootDir | Component::Normal(_)) {
                continue;
            }
            let is_leaf = last_normal == Some(index);
            if is_leaf && !include_leaf {
                break;
            }
            let file = open_file(&current, 0, true, field)?;
            let snapshot = query_snapshot(&file, field, &current)?;
            if snapshot.attributes & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
                return Err(format!(
                    "{field} has forbidden reparse component {}",
                    current.display()
                ));
            }
            if snapshot.attributes & FILE_ATTRIBUTE_DEVICE != 0 {
                return Err(format!(
                    "{field} has device component {}",
                    current.display()
                ));
            }
            if !is_leaf || include_leaf {
                if !snapshot.directory {
                    return Err(format!(
                        "{field} component {} is not a directory",
                        current.display()
                    ));
                }
            }
            chain.push(snapshot.object_id);
        }
        Ok(chain)
    }

    fn require_regular(snapshot: &Snapshot, field: &str, path: &Path) -> Result<(), String> {
        if snapshot.attributes & (FILE_ATTRIBUTE_REPARSE_POINT | FILE_ATTRIBUTE_DEVICE) != 0
            || snapshot.directory
        {
            return Err(format!("{field} {} is not a regular file", path.display()));
        }
        if snapshot.end_of_file < 0 {
            return Err(format!(
                "{field} {} reports a negative length",
                path.display()
            ));
        }
        Ok(())
    }

    fn open_file(path: &Path, access: u32, directory: bool, field: &str) -> Result<File, String> {
        let wide = wide_path(path)?;
        let mut flags = FILE_FLAG_OPEN_REPARSE_POINT;
        if directory {
            flags |= FILE_FLAG_BACKUP_SEMANTICS;
        }
        // SAFETY: `wide` is NUL-terminated, the optional pointers are null,
        // and successful ownership transfers exactly once to `File`.
        let handle = unsafe {
            CreateFileW(
                wide.as_ptr(),
                access,
                FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                ptr::null(),
                OPEN_EXISTING,
                flags,
                ptr::null_mut(),
            )
        };
        if handle == INVALID_HANDLE_VALUE {
            return Err(format!(
                "open {field} {} without following reparse points: {}",
                path.display(),
                std::io::Error::last_os_error()
            ));
        }
        // SAFETY: the handle is valid and newly owned by this call.
        Ok(unsafe { File::from_raw_handle(handle.cast()) })
    }

    fn wide_path(path: &Path) -> Result<Vec<u16>, String> {
        let mut wide: Vec<u16> = path.as_os_str().encode_wide().collect();
        if wide.contains(&0) {
            return Err(format!("path {} contains a NUL code unit", path.display()));
        }
        wide.push(0);
        Ok(wide)
    }

    fn query_snapshot(file: &File, field: &str, path: &Path) -> Result<Snapshot, String> {
        let handle = file.as_raw_handle().cast::<core::ffi::c_void>();
        let id: FILE_ID_INFO = query_info(handle, FileIdInfo, field, path)?;
        let basic: FILE_BASIC_INFO = query_info(handle, FileBasicInfo, field, path)?;
        let standard: FILE_STANDARD_INFO = query_info(handle, FileStandardInfo, field, path)?;
        Ok(Snapshot {
            object_id: StableObjectId::Windows {
                volume_serial_number: id.VolumeSerialNumber,
                file_id: id.FileId.Identifier,
            },
            creation_time: basic.CreationTime,
            last_access_time: basic.LastAccessTime,
            last_write_time: basic.LastWriteTime,
            change_time: basic.ChangeTime,
            attributes: basic.FileAttributes,
            allocation_size: standard.AllocationSize,
            end_of_file: standard.EndOfFile,
            number_of_links: standard.NumberOfLinks,
            delete_pending: standard.DeletePending,
            directory: standard.Directory,
        })
    }

    fn query_info<T: Copy>(
        handle: HANDLE,
        class: i32,
        field: &str,
        path: &Path,
    ) -> Result<T, String> {
        let mut output = MaybeUninit::<T>::uninit();
        // SAFETY: the selected information classes have exactly the caller's
        // `T` layout and the output buffer is writable for its full size.
        let ok = unsafe {
            GetFileInformationByHandleEx(
                handle,
                class,
                output.as_mut_ptr().cast(),
                u32::try_from(size_of::<T>()).expect("Windows info structure size fits u32"),
            )
        };
        if ok == 0 {
            return Err(format!(
                "inspect open {field} {}: {}",
                path.display(),
                std::io::Error::last_os_error()
            ));
        }
        // SAFETY: a successful API call initialized the complete structure.
        Ok(unsafe { output.assume_init() })
    }
}

#[cfg(not(any(unix, windows)))]
mod platform {
    use super::{PathEntry, StableObjectId};
    use std::{fs::File, path::Path};

    pub(super) struct FinishedSnapshot {
        pub(super) object_id: StableObjectId,
        pub(super) unix_mode: Option<u32>,
    }

    pub(super) struct OpenedRegular;

    impl OpenedRegular {
        pub(super) fn open(_path: &Path, _field: &str) -> Result<Self, String> {
            Err("private filesystem admission is unsupported on this target".to_owned())
        }

        pub(super) fn file_mut(&mut self) -> &mut File {
            unreachable!("unsupported targets fail during open")
        }

        pub(super) fn expected_bytes(&self) -> u64 {
            unreachable!("unsupported targets fail during open")
        }

        pub(super) fn finish(
            &self,
            _observed_bytes: u64,
            _field: &str,
        ) -> Result<FinishedSnapshot, String> {
            unreachable!("unsupported targets fail during open")
        }
    }

    pub(super) fn check_directory(_path: &Path, _field: &str) -> Result<StableObjectId, String> {
        Err("private filesystem admission is unsupported on this target".to_owned())
    }

    pub(super) fn inspect_path_entry(_path: &Path, _field: &str) -> Result<PathEntry, String> {
        Err("private filesystem admission is unsupported on this target".to_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};
    #[cfg(unix)]
    use std::{
        sync::{Arc, Barrier},
        thread,
    };

    static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);

    struct TestDirectory(PathBuf);

    impl TestDirectory {
        fn new(label: &str) -> Self {
            let base = std::env::temp_dir()
                .canonicalize()
                .expect("resolve system temporary directory without symlinks");
            loop {
                let sequence = TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
                let candidate = base.join(format!(
                    "fn64-private-fs-{label}-{}-{sequence}",
                    std::process::id()
                ));
                match fs::create_dir(&candidate) {
                    Ok(()) => return Self(candidate),
                    Err(source) if source.kind() == std::io::ErrorKind::AlreadyExists => {}
                    Err(source) => {
                        panic!("create test directory {}: {source}", candidate.display())
                    }
                }
            }
        }
    }

    impl Drop for TestDirectory {
        fn drop(&mut self) {
            fs::remove_dir_all(&self.0).expect("remove exact private-fs test directory");
        }
    }

    #[test]
    fn stable_read_binds_handle_bytes_hash_and_mode() {
        let directory = TestDirectory::new("stable-read");
        let path = directory.0.join("input.bin");
        fs::write(&path, b"abc").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
        }

        let retained = read_regular_stable(&path, "test input").unwrap();
        assert_eq!(retained.contents, b"abc");
        assert_eq!(retained.measurement.path, path);
        assert_eq!(retained.measurement.bytes, 3);
        assert_eq!(
            retained.measurement.sha256,
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        #[cfg(unix)]
        assert_ne!(retained.measurement.unix_mode.unwrap() & 0o111, 0);
        #[cfg(windows)]
        assert_eq!(retained.measurement.unix_mode, None);

        let mut streamed_length = None;
        let mut observed = Vec::new();
        let streamed = measure_regular_stable_with(&path, "streamed test input", |event| {
            match event {
                StableFileStream::Length(bytes) => streamed_length = Some(bytes),
                StableFileStream::Chunk(chunk) => observed.extend_from_slice(chunk),
            }
            Ok(())
        })
        .unwrap();
        assert_eq!(streamed, retained.measurement);
        assert_eq!(streamed_length, Some(3));
        assert_eq!(observed, b"abc");
    }

    #[test]
    fn invalid_paths_and_nonregular_leaf_fail_closed() {
        assert!(validate_absolute_no_parent(Path::new("relative"), "test path").is_err());
        let directory = TestDirectory::new("invalid");
        assert!(validate_absolute_no_parent(&directory.0.join("a/../b"), "test path").is_err());
        assert!(measure_regular_stable(&directory.0, "directory leaf")
            .unwrap_err()
            .contains("not a regular file"));
    }

    #[cfg(unix)]
    #[test]
    fn symlink_components_and_socket_leaf_fail_closed() {
        use std::os::unix::{fs::symlink, net::UnixListener};

        let directory = TestDirectory::new("special-files");
        let real = directory.0.join("real");
        fs::create_dir(&real).unwrap();
        fs::write(real.join("input.bin"), b"payload").unwrap();
        let linked = directory.0.join("linked");
        symlink(&real, &linked).unwrap();
        assert!(measure_regular_stable(&linked.join("input.bin"), "symlinked input").is_err());

        let socket_path = directory.0.join("socket");
        let _socket = UnixListener::bind(&socket_path).unwrap();
        assert!(measure_regular_stable(&socket_path, "socket input").is_err());
    }

    #[cfg(unix)]
    #[test]
    fn replacement_between_open_and_hash_is_rejected_twenty_times() {
        for iteration in 0..20 {
            let directory = TestDirectory::new(&format!("replace-{iteration}"));
            let path = directory.0.join("input.bin");
            let replacement = directory.0.join("replacement.bin");
            let displaced = directory.0.join("displaced.bin");
            fs::write(&path, vec![b'a'; READ_BLOCK_BYTES + 17]).unwrap();
            fs::write(&replacement, vec![b'b'; READ_BLOCK_BYTES + 17]).unwrap();
            let opened = Arc::new(Barrier::new(2));
            let resume = Arc::new(Barrier::new(2));

            thread::scope(|scope| {
                let worker_opened = Arc::clone(&opened);
                let worker_resume = Arc::clone(&resume);
                let worker_path = path.clone();
                let measurement = scope.spawn(move || {
                    measure_regular_stable_impl(
                        &worker_path,
                        "replace-race input",
                        false,
                        || {
                            worker_opened.wait();
                            worker_resume.wait();
                        },
                        |_| Ok(()),
                    )
                });
                opened.wait();
                fs::rename(&path, &displaced).unwrap();
                fs::rename(&replacement, &path).unwrap();
                resume.wait();
                let error = measurement.join().unwrap().unwrap_err();
                assert!(
                    error.contains("changed while it was being measured"),
                    "iteration {iteration}: {error}"
                );
            });
        }
    }

    #[test]
    fn repository_containment_uses_stored_identity_and_git_exit_contract() {
        let repository = PrivateRepository::discover().unwrap();
        let tracked = repository
            .root()
            .join("crates/fn64-boot-harness/Cargo.toml");
        assert_eq!(
            repository
                .filesystem_relative_to(&tracked, "tracked fixture")
                .unwrap(),
            Some(PathBuf::from("crates/fn64-boot-harness/Cargo.toml"))
        );
        assert!(repository
            .require_outside_or_gitignored(&tracked, "tracked fixture")
            .unwrap_err()
            .contains("tracked by git"));

        let external = TestDirectory::new("external");
        let external_file = external.0.join("input.bin");
        fs::write(&external_file, b"private").unwrap();
        assert_eq!(
            repository
                .filesystem_relative_to(&external_file, "external fixture")
                .unwrap(),
            None
        );
        repository
            .require_outside_or_gitignored(&external_file, "external fixture")
            .unwrap();

        let non_repository = TestDirectory::new("non-repository");
        let local_file = non_repository.0.join("input.bin");
        fs::write(&local_file, b"local").unwrap();
        let non_repository = PrivateRepository::from_root(&non_repository.0).unwrap();
        assert!(non_repository
            .require_outside_or_gitignored(&local_file, "non-repository fixture")
            .unwrap_err()
            .contains("git ls-files failed with status"));
    }

    #[cfg(unix)]
    #[test]
    fn containment_retains_exact_hardlink_and_case_spelling() {
        let root = TestDirectory::new("stored-spelling");
        let repository = PrivateRepository::from_root(&root.0).unwrap();
        let original = root.0.join("MixedCase");
        let alias = root.0.join("alias");
        fs::write(&original, b"identity").unwrap();
        fs::hard_link(&original, &alias).unwrap();
        assert_eq!(
            repository
                .filesystem_relative_to(&alias, "hardlink fixture")
                .unwrap(),
            Some(PathBuf::from("alias"))
        );

        let case_variant = root.0.join("mixedcase");
        if fs::symlink_metadata(&case_variant).is_ok() {
            assert_eq!(
                repository
                    .filesystem_relative_to(&case_variant, "case fixture")
                    .unwrap(),
                Some(PathBuf::from("MixedCase"))
            );
        }
    }

    #[test]
    fn lexical_path_comparison_preserves_spelling() {
        assert!(same_lexical_path(
            Path::new("/private/a"),
            Path::new("/private/a")
        ));
        assert!(!same_lexical_path(
            Path::new("/private/A"),
            Path::new("/private/a")
        ));
    }
}
