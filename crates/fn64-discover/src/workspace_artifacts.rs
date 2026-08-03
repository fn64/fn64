//! Contained, create-new publication for out-of-tree tool workspaces.
//!
//! External-tool artifacts may contain game-derived bytes, so every producer
//! shares one path boundary: a canonical absolute workspace outside a Git
//! worktree, canonical output parents contained by that workspace, and an
//! atomic publication primitive that cannot replace an existing destination.

use std::fs::{self, OpenOptions};
use std::io::Write;
#[cfg(unix)]
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};

pub fn validate_workspace(path: &Path) -> Result<PathBuf, String> {
    if !path.is_absolute() {
        return Err("workspace must be absolute".into());
    }
    let canonical = fs::canonicalize(path)
        .map_err(|error| format!("resolving workspace {}: {error}", path.display()))?;
    if canonical != path {
        return Err("workspace must be canonical and contain no symlink traversal".into());
    }
    if !canonical.is_dir() {
        return Err("workspace must be a directory".into());
    }
    #[cfg(unix)]
    {
        let mode = fs::metadata(&canonical)
            .map_err(|error| format!("inspecting workspace {}: {error}", canonical.display()))?
            .mode()
            & 0o777;
        if mode != 0o700 {
            return Err(format!(
                "workspace must have mode 0700, got {mode:04o}: {}",
                canonical.display()
            ));
        }
    }
    for ancestor in canonical.ancestors() {
        if ancestor.join(".git").exists() {
            return Err(format!(
                "workspace must not be inside a Git worktree: {}",
                canonical.display()
            ));
        }
    }
    Ok(canonical)
}

pub fn validate_output_path(workspace: &Path, output: &Path) -> Result<(), String> {
    if !output.is_absolute() {
        return Err("output path must be absolute".into());
    }
    let parent = output
        .parent()
        .ok_or_else(|| "output path has no parent".to_string())?;
    let canonical_parent = fs::canonicalize(parent)
        .map_err(|error| format!("resolving output directory {}: {error}", parent.display()))?;
    if canonical_parent != parent || !canonical_parent.starts_with(workspace) {
        return Err(format!(
            "output directory must be canonical and inside workspace {}",
            workspace.display()
        ));
    }
    Ok(())
}

pub fn publish_new(path: &Path, bytes: &[u8]) -> Result<(), String> {
    match fs::symlink_metadata(path) {
        Ok(_) => return Err(format!("refusing to overwrite {}", path.display())),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(format!("inspecting output {}: {error}", path.display())),
    }
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    if !parent.is_dir() {
        return Err(format!(
            "output directory does not exist: {}",
            parent.display()
        ));
    }
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| "output filename must be valid UTF-8".to_string())?;
    for attempt in 0..128u32 {
        let temporary = parent.join(format!(
            ".{file_name}.fn64-tmp-{}-{attempt}",
            std::process::id()
        ));
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        options.mode(0o600);
        let mut file = match options.open(&temporary) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => {
                return Err(format!(
                    "creating staging file {}: {error}",
                    temporary.display()
                ));
            }
        };
        if let Err(error) = file.write_all(bytes).and_then(|_| file.sync_all()) {
            let _ = fs::remove_file(&temporary);
            return Err(format!(
                "writing staging file {}: {error}",
                temporary.display()
            ));
        }
        drop(file);
        // A hard link is the no-replace commit point. Concurrent publishers
        // may finish separate staging files, but only one can claim `path`.
        if let Err(error) = fs::hard_link(&temporary, path) {
            let _ = fs::remove_file(&temporary);
            return Err(format!("publishing {}: {error}", path.display()));
        }
        if let Err(error) = fs::remove_file(&temporary) {
            return Err(format!(
                "published {}, but could not remove staging link {}: {error}",
                path.display(),
                temporary.display()
            ));
        }
        // File content was synced before the hard-link commit. Sync the
        // containing directory after both the destination link and staging
        // unlink so create-new publication order survives a power loss. This
        // is the durability boundary that makes a manifest published last a
        // durable completion marker, not merely a process-order marker.
        #[cfg(unix)]
        fs::File::open(parent)
            .and_then(|directory| directory.sync_all())
            .map_err(|error| {
                format!(
                    "published {}, but could not sync output directory {}: {error}",
                    path.display(),
                    parent.display()
                )
            })?;
        return Ok(());
    }
    Err(format!(
        "could not reserve a staging filename beside {}",
        path.display()
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Barrier};
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temporary_directory(label: &str) -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "fn64-workspace-artifacts-{label}-{}-{nonce}",
            std::process::id()
        ));
        fs::create_dir(&path).unwrap();
        fs::canonicalize(path).unwrap()
    }

    #[test]
    fn workspace_and_output_must_stay_outside_git() {
        let directory = temporary_directory("containment");
        let clean = directory.join("clean");
        fs::create_dir(&clean).unwrap();
        #[cfg(unix)]
        fs::set_permissions(&clean, std::os::unix::fs::PermissionsExt::from_mode(0o700)).unwrap();
        let clean = fs::canonicalize(clean).unwrap();
        assert_eq!(validate_workspace(&clean).unwrap(), clean);
        validate_output_path(&clean, &clean.join("claims.json")).unwrap();
        assert!(validate_output_path(&clean, &directory.join("outside.json")).is_err());

        #[cfg(unix)]
        {
            fs::set_permissions(&clean, std::os::unix::fs::PermissionsExt::from_mode(0o755))
                .unwrap();
            assert!(validate_workspace(&clean)
                .unwrap_err()
                .contains("mode 0700"));
            fs::set_permissions(&clean, std::os::unix::fs::PermissionsExt::from_mode(0o700))
                .unwrap();
        }

        let worktree = directory.join("worktree");
        let nested = worktree.join("out");
        fs::create_dir_all(worktree.join(".git")).unwrap();
        fs::create_dir(&nested).unwrap();
        let nested = fs::canonicalize(nested).unwrap();
        assert!(validate_workspace(&nested).is_err());
        fs::remove_dir_all(directory).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_workspace_and_output_parent_are_rejected() {
        use std::os::unix::fs::symlink;

        let directory = temporary_directory("symlink");
        let real = directory.join("real");
        fs::create_dir(&real).unwrap();
        fs::set_permissions(&real, std::os::unix::fs::PermissionsExt::from_mode(0o700)).unwrap();
        let alias = directory.join("alias");
        symlink(&real, &alias).unwrap();
        assert!(validate_workspace(&alias).is_err());

        let workspace = fs::canonicalize(&real).unwrap();
        let child = real.join("child");
        fs::create_dir(&child).unwrap();
        let child_alias = real.join("child-alias");
        symlink(&child, &child_alias).unwrap();
        assert!(validate_output_path(&workspace, &child_alias.join("out")).is_err());
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn publish_new_never_overwrites_a_concurrent_winner() {
        let directory = temporary_directory("publish-race");
        let output = directory.join("claims.json");
        let barrier = Arc::new(Barrier::new(3));
        let mut writers = Vec::new();
        for bytes in [b"first\n".as_slice(), b"second\n".as_slice()] {
            let barrier = Arc::clone(&barrier);
            let output = output.clone();
            writers.push(std::thread::spawn(move || {
                barrier.wait();
                (bytes, publish_new(&output, bytes))
            }));
        }
        barrier.wait();
        let results: Vec<_> = writers
            .into_iter()
            .map(|writer| writer.join().unwrap())
            .collect();
        assert_eq!(
            results.iter().filter(|(_, result)| result.is_ok()).count(),
            1
        );
        let winner = results
            .iter()
            .find_map(|(bytes, result)| result.is_ok().then_some(*bytes))
            .unwrap();
        assert_eq!(fs::read(&output).unwrap(), winner);
        fs::remove_dir_all(directory).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn publish_new_refuses_a_dangling_symlink_destination() {
        use std::os::unix::fs::symlink;

        let directory = temporary_directory("dangling-output");
        let output = directory.join("output");
        symlink(directory.join("missing"), &output).unwrap();
        assert!(publish_new(&output, b"bytes")
            .unwrap_err()
            .contains("refusing"));
        fs::remove_dir_all(directory).unwrap();
    }
}
