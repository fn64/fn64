//! Publish the exact generated-source identity wire embedded by `build.rs`.
//!
//! The output is private and content-bearing. It is the typed-function lane
//! input consumed by `materialize-release-program-build-receipt`, not a
//! transferable build/link attestation.

#[cfg(fn64_recomp_rs)]
use sha2::{Digest as _, Sha256};
#[cfg(fn64_recomp_rs)]
use std::{
    env,
    fs::{self, OpenOptions},
    io::Write as _,
    path::{Component, Path, PathBuf},
    process,
};

#[cfg(fn64_recomp_rs)]
const IDENTITY_WIRE: &[u8] = include_bytes!(env!("FN64_RECOMP_RS_FUNCTION_IDENTITY_WIRE_PATH"));

#[cfg(fn64_recomp_rs)]
fn main() {
    if let Err(error) = run() {
        eprintln!("write-function-identity-wire: {error}");
        process::exit(1);
    }
}

#[cfg(not(fn64_recomp_rs))]
fn main() {
    eprintln!(
        "write-function-identity-wire: this binary must be built with FN64_RECOMP=rs and FN64_RS_EXECUTION=function"
    );
    std::process::exit(1);
}

#[cfg(fn64_recomp_rs)]
fn run() -> Result<(), String> {
    let mut arguments = env::args_os().skip(1);
    let output = arguments
        .next()
        .ok_or_else(|| "usage: write-function-identity-wire ABSOLUTE_OUTPUT".to_owned())?;
    if arguments.next().is_some() {
        return Err("usage: write-function-identity-wire ABSOLUTE_OUTPUT".to_owned());
    }
    let output = Path::new(&output);
    if !output.is_absolute()
        || output
            .components()
            .any(|component| matches!(component, Component::ParentDir))
    {
        return Err("output must be absolute and contain no '..' component".to_owned());
    }
    let parent = output
        .parent()
        .ok_or_else(|| "output must have an existing parent directory".to_owned())?;
    reject_symlink_components(parent)?;
    if !fs::symlink_metadata(parent)
        .map_err(|error| format!("inspect output parent {}: {error}", parent.display()))?
        .is_dir()
    {
        return Err(format!(
            "output parent {} is not a directory",
            parent.display()
        ));
    }
    let expected = env!("FN64_RECOMP_RS_FUNCTION_ARTIFACT_SHA256");
    let observed = format!("{:x}", Sha256::digest(IDENTITY_WIRE));
    if observed != expected {
        return Err(format!(
            "embedded identity wire SHA-256 {observed} differs from child artifact identity {expected}"
        ));
    }

    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.mode(0o600);
    }
    let mut file = options
        .open(output)
        .map_err(|error| format!("create-new output {}: {error}", output.display()))?;
    file.write_all(IDENTITY_WIRE)
        .and_then(|()| file.flush())
        .and_then(|()| file.sync_all())
        .map_err(|error| format!("persist output {}: {error}", output.display()))?;
    println!("artifact_sha256={observed}");
    Ok(())
}

#[cfg(fn64_recomp_rs)]
fn reject_symlink_components(path: &Path) -> Result<(), String> {
    let mut current = PathBuf::new();
    for component in path.components() {
        current.push(component.as_os_str());
        let metadata = fs::symlink_metadata(&current).map_err(|error| {
            format!(
                "inspect output parent component {}: {error}",
                current.display()
            )
        })?;
        if metadata.file_type().is_symlink() {
            return Err(format!(
                "output parent has forbidden symlink component {}",
                current.display()
            ));
        }
    }
    Ok(())
}
