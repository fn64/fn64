use std::path::{Path, PathBuf};
use std::process::Command;

const DEV_INTERPRETER_ARTIFACT_MARKER: &[u8] = b"fn64-cpu-runtime:dev-interpreter:artifact";

/// A symbol only `#[cfg(feature = "dev-interpreter")]` code defines. Several
/// workspace members (`fn64-abi`, `fn64-recomp-rs-codegen`, the `aot-runtime`
/// example crates) depend on `fn64-cpu-runtime` with `default-features = false,
/// features = ["aot-runtime"]`, so `target/debug/deps` routinely holds *both*
/// a dev-interpreter rlib and one or more aot-runtime-only rlibs side by
/// side, all rebuilt together whenever shared source (e.g. `runtime/host.rs`)
/// changes.
///
/// [`DEV_INTERPRETER_ARTIFACT_MARKER`] alone cannot tell those apart: its
/// module is `#[cfg]`'d out of the aot-runtime build, but rustc's crate
/// metadata can still retain the *source text* of a disabled item (spans,
/// doc comments, macro hygiene bookkeeping), so the marker bytes leak into
/// non-dev-interpreter rlibs too. Confirmed by inspection: an aot-runtime-only
/// rlib built from this tree contains the marker string via `strings` yet
/// exports zero `run_bank` symbols via `nm`. The marker is kept as a cheap
/// first-pass filter; `run_bank` (public only under `dev-interpreter`, see
/// `src/lib.rs`) is the decisive check because it is the exact capability the
/// isolated bank-runner compile unit links against.
const DEV_INTERPRETER_ONLY_SYMBOL: &str = "run_bank";

fn exports_dev_interpreter_symbol(path: &Path) -> bool {
    let nm = std::env::var("NM").unwrap_or_else(|_| "nm".into());
    Command::new(nm)
        .arg(path)
        .output()
        .is_ok_and(|output| {
            output.status.success()
                && String::from_utf8_lossy(&output.stdout).contains(DEV_INTERPRETER_ONLY_SYMBOL)
        })
}

pub fn dev_interpreter_rlib(deps: &Path) -> PathBuf {
    std::fs::read_dir(deps)
        .expect("read target deps directory")
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| {
                    name.starts_with("libfn64_cpu_runtime-") && name.ends_with(".rlib")
                })
        })
        .filter(|path| {
            std::fs::read(path).is_ok_and(|bytes| {
                bytes
                    .windows(DEV_INTERPRETER_ARTIFACT_MARKER.len())
                    .any(|window| window == DEV_INTERPRETER_ARTIFACT_MARKER)
            })
        })
        .filter(|path| exports_dev_interpreter_symbol(path))
        .max_by_key(|path| path.metadata().and_then(|meta| meta.modified()).ok())
        .expect("dev-interpreter fn64_cpu_runtime rlib beside integration test")
}
