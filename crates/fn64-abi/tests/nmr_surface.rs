//! Keeps the clean-room NMR compatibility inventory synchronized with the
//! live `#[no_mangle] extern "C"` surface and its generated completeness doc.

use std::path::PathBuf;
use std::process::Command;

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|path| path.parent())
        .expect("fn64-abi must live at <repo>/crates/fn64-abi")
        .to_path_buf()
}

fn run_checker(arg: &str) -> std::process::Output {
    let root = repo_root();
    Command::new("python3")
        .arg(root.join("scripts/check-nmr-surface.py"))
        .arg(arg)
        .current_dir(root)
        .output()
        .expect("run scripts/check-nmr-surface.py with python3")
}

#[test]
fn nmr_surface_inventory_matches_live_abi_and_completeness_doc() {
    let output = run_checker("--check-doc");
    assert!(
        output.status.success(),
        "NMR surface inventory drifted:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
}

#[test]
fn nmr_surface_checker_proves_its_classifier_can_fail() {
    let output = run_checker("--selftest");
    assert!(
        output.status.success(),
        "NMR surface checker selftest failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
}
