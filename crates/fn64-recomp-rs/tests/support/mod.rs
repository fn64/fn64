use std::path::{Path, PathBuf};

const DEV_INTERPRETER_ARTIFACT_MARKER: &[u8] = b"fn64-recomp-rs:dev-interpreter:artifact";

pub fn dev_interpreter_rlib(deps: &Path) -> PathBuf {
    std::fs::read_dir(deps)
        .expect("read target deps directory")
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| {
                    name.starts_with("libfn64_recomp_rs-") && name.ends_with(".rlib")
                })
        })
        .filter(|path| {
            std::fs::read(path).is_ok_and(|bytes| {
                bytes
                    .windows(DEV_INTERPRETER_ARTIFACT_MARKER.len())
                    .any(|window| window == DEV_INTERPRETER_ARTIFACT_MARKER)
            })
        })
        .max_by_key(|path| path.metadata().and_then(|meta| meta.modified()).ok())
        .expect("dev-interpreter fn64_recomp_rs rlib beside integration test")
}
