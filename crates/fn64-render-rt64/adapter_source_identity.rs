//! Canonical identity of the fn64-owned RT64 adapter source and build shape.

use sha2::{Digest, Sha256};
use std::{fs, io, path::Path};

const ROOT_FILES: &[&str] = &["Cargo.toml", "adapter_source_identity.rs", "build.rs"];
const FFI_FILES: &[&str] = &[
    "ffi/CMakeLists.txt",
    "ffi/fn64_rt64_shim.cpp",
    "ffi/fn64_rt64_shim.h",
];

pub fn adapter_source_sha256(
    manifest_dir: &Path,
    target: &str,
    enabled_features: &[String],
) -> io::Result<[u8; 32]> {
    let paths = adapter_source_paths(manifest_dir)?;

    let mut features = enabled_features.to_vec();
    features.sort();
    features.dedup();

    let inputs = paths
        .into_iter()
        .map(|relative| {
            let bytes = fs::read(manifest_dir.join(&relative))?;
            Ok((relative, bytes))
        })
        .collect::<io::Result<Vec<_>>>()?;
    Ok(hash_inputs(target, &features, &inputs))
}

fn hash_inputs(target: &str, features: &[String], inputs: &[(String, Vec<u8>)]) -> [u8; 32] {
    let mut digest = Sha256::new();
    digest.update(b"fn64.rt64-adapter-source.v1\0");
    push_bytes(&mut digest, target.as_bytes());
    digest.update((features.len() as u64).to_be_bytes());
    for feature in features {
        push_bytes(&mut digest, feature.as_bytes());
    }
    digest.update((inputs.len() as u64).to_be_bytes());
    for (relative, bytes) in inputs {
        push_bytes(&mut digest, relative.as_bytes());
        push_bytes(&mut digest, bytes);
    }
    digest.finalize().into()
}

pub fn adapter_source_paths(manifest_dir: &Path) -> io::Result<Vec<String>> {
    let mut paths = ROOT_FILES
        .iter()
        .chain(FFI_FILES)
        .map(|path| (*path).to_owned())
        .collect::<Vec<_>>();
    collect_rs_files(manifest_dir, manifest_dir.join("src"), &mut paths)?;
    paths.sort();
    Ok(paths)
}

fn collect_rs_files(
    root: &Path,
    directory: impl AsRef<Path>,
    paths: &mut Vec<String>,
) -> io::Result<()> {
    for entry in fs::read_dir(directory)? {
        let path = entry?.path();
        if path.is_dir() {
            collect_rs_files(root, &path, paths)?;
        } else if path.extension().and_then(|extension| extension.to_str()) == Some("rs") {
            paths.push(
                path.strip_prefix(root)
                    .expect("adapter source traversal stays beneath its manifest")
                    .to_string_lossy()
                    .replace('\\', "/"),
            );
        }
    }
    Ok(())
}

fn push_bytes(digest: &mut Sha256, bytes: &[u8]) {
    digest.update((bytes.len() as u64).to_be_bytes());
    digest.update(bytes);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_binds_source_paths_bytes_target_and_features() {
        let inputs = vec![
            ("ffi/shim.cpp".to_owned(), b"shim-v1".to_vec()),
            ("src/lib.rs".to_owned(), b"adapter-v1".to_vec()),
        ];
        let features = vec!["RT64".to_owned()];
        let baseline = hash_inputs("aarch64-apple-darwin", &features, &inputs);

        for changed in [
            vec![
                ("ffi/shim.cpp".to_owned(), b"shim-v2".to_vec()),
                ("src/lib.rs".to_owned(), b"adapter-v1".to_vec()),
            ],
            vec![
                ("ffi/renamed.cpp".to_owned(), b"shim-v1".to_vec()),
                ("src/lib.rs".to_owned(), b"adapter-v1".to_vec()),
            ],
        ] {
            assert_ne!(
                hash_inputs("aarch64-apple-darwin", &features, &changed),
                baseline
            );
        }
        assert_ne!(
            hash_inputs("x86_64-unknown-linux-gnu", &features, &inputs),
            baseline
        );
        assert_ne!(
            hash_inputs(
                "aarch64-apple-darwin",
                &["HFR_EVIDENCE".to_owned(), "RT64".to_owned()],
                &inputs,
            ),
            baseline
        );
    }

    #[test]
    fn crate_identity_covers_rust_cpp_manifest_and_build_inputs() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"));
        let paths = adapter_source_paths(root).unwrap();
        for required in [
            "Cargo.toml",
            "adapter_source_identity.rs",
            "build.rs",
            "src/lib.rs",
            "src/ffi.rs",
            "ffi/CMakeLists.txt",
            "ffi/fn64_rt64_shim.cpp",
            "ffi/fn64_rt64_shim.h",
        ] {
            assert!(
                paths.iter().any(|path| path == required),
                "missing {required}"
            );
        }
        assert_ne!(
            adapter_source_sha256(root, "test-target", &["RT64".to_owned()]).unwrap(),
            [0; 32]
        );
    }
}
