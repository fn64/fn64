//! Stable source identity for an out-of-tree `fn64-recomp-rs` crate.
//!
//! The wire deliberately contains no absolute path. It binds a canonicalized
//! exact generated-manifest contract plus every regular file under `src/`, in
//! normalized relative-path order. Only the manifest's machine-local
//! `fn64-recomp-rs` path is normalized, after it resolves to the expected
//! runtime crate. Unexpected targets, features, dependencies, build scripts,
//! symlinks, and non-file entries fail closed.

use std::fs;
use std::path::{Path, PathBuf};

const DOMAIN: &[u8] = b"fn64.typed-rust-generated-source.v2\0";
const CANONICAL_MANIFEST: &[u8] = b"fn64.recompile-rom-manifest.v1\0\
package.name=oot-recompiled\0\
package.version=0.0.0\0\
package.edition=2021\0\
package.license=MIT OR Apache-2.0\0\
package.publish=false\0\
dependencies.fn64-recomp-rs.path=<current-runtime>\0";

pub fn canonical_source_wire(
    root: &Path,
    expected_runtime: &Path,
) -> Result<(Vec<u8>, Vec<PathBuf>), String> {
    let manifest = root.join("Cargo.toml");
    let source_root = root.join("src");
    let manifest_metadata = fs::symlink_metadata(&manifest)
        .map_err(|error| format!("inspect generated manifest {}: {error}", manifest.display()))?;
    let source_metadata = fs::symlink_metadata(&source_root).map_err(|error| {
        format!(
            "inspect generated source root {}: {error}",
            source_root.display()
        )
    })?;
    if manifest_metadata.file_type().is_symlink()
        || source_metadata.file_type().is_symlink()
        || !manifest_metadata.is_file()
        || !source_metadata.is_dir()
    {
        return Err(format!(
            "{} must contain a non-symlink regular Cargo.toml and src directory",
            root.display()
        ));
    }
    validate_root_entries(root)?;
    validate_manifest(&manifest, expected_runtime)?;
    let mut files = Vec::new();
    collect_regular_files(&source_root, &mut files)?;
    let mut entries = files
        .into_iter()
        .map(|path| {
            let relative = path.strip_prefix(root).map_err(|_| {
                format!(
                    "generated source {} escaped artifact root {}",
                    path.display(),
                    root.display()
                )
            })?;
            let relative = normalized_relative_path(relative)?;
            let bytes = fs::read(&path)
                .map_err(|error| format!("read generated source {}: {error}", path.display()))?;
            Ok((relative, path, bytes))
        })
        .collect::<Result<Vec<_>, String>>()?;
    entries.sort_unstable_by(|left, right| left.0.cmp(&right.0));
    if !entries.iter().any(|(path, _, _)| path == "src/lib.rs") {
        return Err(format!(
            "{} does not contain the generated src/lib.rs entry",
            root.display()
        ));
    }

    let mut wire = Vec::new();
    wire.extend_from_slice(DOMAIN);
    push_bytes(&mut wire, CANONICAL_MANIFEST);
    push_u64(&mut wire, entries.len() as u64);
    let mut watched = Vec::with_capacity(entries.len());
    for (relative, path, bytes) in entries {
        push_bytes(&mut wire, relative.as_bytes());
        push_bytes(&mut wire, &bytes);
        watched.push(path);
    }
    Ok((wire, watched))
}

fn validate_root_entries(root: &Path) -> Result<(), String> {
    let entries = fs::read_dir(root)
        .map_err(|error| format!("read generated artifact root {}: {error}", root.display()))?;
    for entry in entries {
        let entry = entry.map_err(|error| {
            format!(
                "read generated artifact root entry in {}: {error}",
                root.display()
            )
        })?;
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            return Err(format!(
                "generated artifact root entry {} is not UTF-8",
                entry.path().display()
            ));
        };
        if !matches!(name, "Cargo.toml" | "gap-report.md" | "src") {
            return Err(format!(
                "generated artifact root contains unexpected compilation input {name:?}; expected only Cargo.toml, gap-report.md, and src"
            ));
        }
        let metadata = fs::symlink_metadata(entry.path()).map_err(|error| {
            format!(
                "inspect generated artifact root entry {}: {error}",
                entry.path().display()
            )
        })?;
        if metadata.file_type().is_symlink() {
            return Err(format!(
                "generated artifact root entry {} is a symlink",
                entry.path().display()
            ));
        }
        if name == "src" && !metadata.is_dir() || name != "src" && !metadata.is_file() {
            return Err(format!(
                "generated artifact root entry {} has the wrong file type",
                entry.path().display()
            ));
        }
    }
    Ok(())
}

fn validate_manifest(manifest: &Path, expected_runtime: &Path) -> Result<(), String> {
    let text = fs::read_to_string(manifest)
        .map_err(|error| format!("read generated manifest {}: {error}", manifest.display()))?;
    let value = text
        .parse::<toml::Value>()
        .map_err(|error| format!("parse generated manifest {}: {error}", manifest.display()))?;
    let root = value
        .as_table()
        .ok_or_else(|| "generated manifest root must be a TOML table".to_owned())?;
    exact_keys(
        root,
        &["dependencies", "package"],
        "generated manifest root",
    )?;
    let package = root
        .get("package")
        .and_then(toml::Value::as_table)
        .ok_or_else(|| "generated manifest package must be a table".to_owned())?;
    exact_keys(
        package,
        &["edition", "license", "name", "publish", "version"],
        "generated manifest package",
    )?;
    exact_string(package, "name", "oot-recompiled")?;
    exact_string(package, "version", "0.0.0")?;
    exact_string(package, "edition", "2021")?;
    exact_string(package, "license", "MIT OR Apache-2.0")?;
    if package.get("publish").and_then(toml::Value::as_bool) != Some(false) {
        return Err("generated manifest package.publish must be false".to_owned());
    }

    let dependencies = root
        .get("dependencies")
        .and_then(toml::Value::as_table)
        .ok_or_else(|| "generated manifest dependencies must be a table".to_owned())?;
    exact_keys(
        dependencies,
        &["fn64-recomp-rs"],
        "generated manifest dependencies",
    )?;
    let runtime = dependencies
        .get("fn64-recomp-rs")
        .and_then(toml::Value::as_table)
        .ok_or_else(|| "generated manifest fn64-recomp-rs dependency must be a table".to_owned())?;
    exact_keys(
        runtime,
        &["path"],
        "generated manifest fn64-recomp-rs dependency",
    )?;
    let runtime_path = runtime
        .get("path")
        .and_then(toml::Value::as_str)
        .ok_or_else(|| "generated manifest fn64-recomp-rs.path must be a string".to_owned())?;
    let observed = Path::new(runtime_path).canonicalize().map_err(|error| {
        format!("canonicalize generated manifest fn64-recomp-rs.path {runtime_path:?}: {error}")
    })?;
    let expected = expected_runtime.canonicalize().map_err(|error| {
        format!(
            "canonicalize expected fn64-recomp-rs path {}: {error}",
            expected_runtime.display()
        )
    })?;
    if observed != expected {
        return Err(format!(
            "generated manifest fn64-recomp-rs.path resolves to {}, expected {}",
            observed.display(),
            expected.display()
        ));
    }
    Ok(())
}

fn exact_keys(
    table: &toml::map::Map<String, toml::Value>,
    expected: &[&str],
    label: &str,
) -> Result<(), String> {
    let observed = table.keys().map(String::as_str).collect::<Vec<_>>();
    if observed == expected {
        Ok(())
    } else {
        Err(format!(
            "{label} keys differ from emitter contract: observed {observed:?}, expected {expected:?}"
        ))
    }
}

fn exact_string(
    table: &toml::map::Map<String, toml::Value>,
    key: &str,
    expected: &str,
) -> Result<(), String> {
    if table.get(key).and_then(toml::Value::as_str) == Some(expected) {
        Ok(())
    } else {
        Err(format!(
            "generated manifest package.{key} must equal {expected:?}"
        ))
    }
}

fn collect_regular_files(directory: &Path, output: &mut Vec<PathBuf>) -> Result<(), String> {
    let entries = fs::read_dir(directory).map_err(|error| {
        format!(
            "read generated source directory {}: {error}",
            directory.display()
        )
    })?;
    for entry in entries {
        let entry = entry.map_err(|error| {
            format!(
                "read generated source directory entry in {}: {error}",
                directory.display()
            )
        })?;
        let path = entry.path();
        let metadata = fs::symlink_metadata(&path)
            .map_err(|error| format!("inspect generated source {}: {error}", path.display()))?;
        if metadata.file_type().is_symlink() {
            return Err(format!(
                "generated source {} is a symlink; artifact inputs must be self-contained regular files",
                path.display()
            ));
        }
        if metadata.is_dir() {
            collect_regular_files(&path, output)?;
        } else if metadata.is_file() {
            output.push(path);
        } else {
            return Err(format!(
                "generated source {} is neither a regular file nor directory",
                path.display()
            ));
        }
    }
    Ok(())
}

fn normalized_relative_path(path: &Path) -> Result<String, String> {
    let components = path
        .components()
        .map(|component| {
            component.as_os_str().to_str().ok_or_else(|| {
                format!(
                    "generated artifact relative path {} is not UTF-8",
                    path.display()
                )
            })
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(components.join("/"))
}

fn push_u64(output: &mut Vec<u8>, value: u64) {
    output.extend_from_slice(&value.to_le_bytes());
}

fn push_bytes(output: &mut Vec<u8>, bytes: &[u8]) {
    push_u64(output, bytes.len() as u64);
    output.extend_from_slice(bytes);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_manifest(root: &Path, runtime: &Path, extra: &str) {
        fs::write(
            root.join("Cargo.toml"),
            format!(
                "# Generated by fn64-recomp-rs whole-ROM driver (recompile_rom).\n\
                 [package]\n\
                 name = \"oot-recompiled\"\n\
                 version = \"0.0.0\"\n\
                 edition = \"2021\"\n\
                 license = \"MIT OR Apache-2.0\"\n\
                 publish = false\n\n\
                 [dependencies]\n\
                 fn64-recomp-rs = {{ path = {:?} }}\n\
                 {extra}",
                runtime.to_str().unwrap()
            ),
        )
        .unwrap();
    }

    fn fixture(label: &str) -> (PathBuf, PathBuf) {
        let root = std::env::temp_dir().join(format!(
            "fn64-oot-rs-program-identity-{}-{label}",
            std::process::id()
        ));
        let runtime = std::env::temp_dir().join(format!(
            "fn64-oot-rs-program-runtime-{}-{label}",
            std::process::id()
        ));
        if root.exists() {
            fs::remove_dir_all(&root).unwrap();
        }
        if runtime.exists() {
            fs::remove_dir_all(&runtime).unwrap();
        }
        fs::create_dir_all(root.join("src/parts")).unwrap();
        fs::create_dir_all(&runtime).unwrap();
        write_manifest(&root, &runtime, "");
        fs::write(root.join("src/lib.rs"), b"mod parts;\n").unwrap();
        fs::write(root.join("src/parts/one.rs"), b"pub fn one() {}\n").unwrap();
        (root, runtime)
    }

    #[test]
    fn identity_is_path_independent_and_content_sensitive() {
        let (first, first_runtime) = fixture("first");
        let (second, second_runtime) = fixture("second");
        let (first_wire, first_files) = canonical_source_wire(&first, &first_runtime).unwrap();
        let (second_wire, second_files) = canonical_source_wire(&second, &second_runtime).unwrap();
        assert_eq!(first_wire, second_wire);
        assert_eq!(first_files.len(), 2);
        assert_eq!(second_files.len(), 2);

        fs::write(second.join("src/parts/one.rs"), b"pub fn two() {}\n").unwrap();
        assert_ne!(
            first_wire,
            canonical_source_wire(&second, &second_runtime).unwrap().0
        );
        fs::remove_dir_all(first).unwrap();
        fs::remove_dir_all(second).unwrap();
        fs::remove_dir_all(first_runtime).unwrap();
        fs::remove_dir_all(second_runtime).unwrap();
    }

    #[test]
    fn identity_rejects_manifest_and_root_compilation_extensions() {
        let (root, runtime) = fixture("manifest-drift");
        write_manifest(&root, &runtime, "[features]\ndefault = []\n");
        assert!(canonical_source_wire(&root, &runtime)
            .unwrap_err()
            .contains("root keys differ"));

        write_manifest(&root, &runtime, "");
        fs::write(root.join("build.rs"), b"fn main() {}\n").unwrap();
        assert!(canonical_source_wire(&root, &runtime)
            .unwrap_err()
            .contains("unexpected compilation input"));
        fs::remove_dir_all(root).unwrap();
        fs::remove_dir_all(runtime).unwrap();
    }

    #[test]
    fn identity_rejects_dependency_and_target_mutation() {
        let (root, runtime) = fixture("dependency-drift");
        let text = fs::read_to_string(root.join("Cargo.toml")).unwrap();
        fs::write(
            root.join("Cargo.toml"),
            text.replace("fn64-recomp-rs = {", "serde = \"1\"\nfn64-recomp-rs = {"),
        )
        .unwrap();
        assert!(canonical_source_wire(&root, &runtime)
            .unwrap_err()
            .contains("dependencies keys differ"));

        write_manifest(
            &root,
            &runtime,
            "[[bin]]\nname = \"alternate\"\npath = \"src/lib.rs\"\n",
        );
        assert!(canonical_source_wire(&root, &runtime)
            .unwrap_err()
            .contains("root keys differ"));
        fs::remove_dir_all(root).unwrap();
        fs::remove_dir_all(runtime).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn identity_rejects_source_symlinks() {
        use std::os::unix::fs::symlink;

        let (root, runtime) = fixture("symlink");
        symlink(root.join("src/lib.rs"), root.join("src/alias.rs")).unwrap();
        assert!(canonical_source_wire(&root, &runtime)
            .unwrap_err()
            .contains("is a symlink"));
        fs::remove_dir_all(root).unwrap();
        fs::remove_dir_all(runtime).unwrap();
    }
}
