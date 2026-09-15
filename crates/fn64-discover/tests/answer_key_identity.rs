//! The answer-key identity guard on `gate_decomp_functions`.
//!
//! `wrong == 0` is the firewall's whole contract, and it is meaningless if the
//! answer key can drift underneath it. A locally edited key grades as a
//! perfectly plausible `wrong=1` that reads exactly like a discovery
//! regression -- which is what held the firewall red on main and cost a full
//! bisect to attribute. These tests drive the CLI so the guard is exercised
//! end to end, including that an UNSET declaration stays unchecked.

use std::process::Command;

fn discover_bin() -> std::path::PathBuf {
    // The integration-test binary lives in target/<profile>/deps/.
    let mut path = std::env::current_exe().expect("test binary path");
    path.pop();
    if path.ends_with("deps") {
        path.pop();
    }
    path.join("fn64-discover")
}

/// A two-section key with three functions, enough to be parsed and counted.
const KEY: &str = r#"
[[section]]
name = "boot"
rom = 0x00001000
vram = 0x80000400
size = 0x20

functions = [
    { name = "a", vram = 0x80000400, size = 0x10 },
    { name = "b", vram = 0x80000410, size = 0x10 },
]

[[section]]
name = "ovl"
rom = 0x00002000
vram = 0x80100000
size = 0x10

functions = [
    { name = "c", vram = 0x80100000, size = 0x10 },
]
"#;

fn run_with(dump: &std::path::Path, vars: &[(&str, &str)]) -> (bool, String) {
    let binary = discover_bin();
    if !binary.exists() {
        // Built per-crate; if the bin is absent this test cannot run the CLI.
        return (true, String::from("<binary absent>"));
    }
    let mut command = Command::new(binary);
    command.arg("gate-decomp-functions");
    // A ROM is required before the dump is even read; point at the dump
    // itself so the run fails AFTER the identity check, never before it.
    command.env("FN64_DISCOVER_ROM", dump);
    command.env("FN64_DISCOVER_DUMP", dump);
    for (key, value) in vars {
        command.env(key, value);
    }
    let output = command.output().expect("running fn64-discover");
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    (output.status.success(), text)
}

fn write_key() -> (tempdir_shim::TempDir, std::path::PathBuf) {
    let dir = tempdir_shim::TempDir::new();
    let path = dir.path().join("dump.toml");
    std::fs::write(&path, KEY).expect("writing test key");
    (dir, path)
}

#[test]
fn a_mismatched_function_count_fails_loudly() {
    let (_dir, path) = write_key();
    let (_ok, text) = run_with(&path, &[("FN64_DISCOVER_DUMP_FUNCTIONS", "999")]);
    if text == "<binary absent>" {
        return;
    }
    assert!(
        text.contains("answer-key identity") && text.contains("has 3 functions, expected 999"),
        "expected a loud identity failure naming both counts, got:\n{text}"
    );
}

#[test]
fn a_mismatched_section_count_fails_loudly() {
    let (_dir, path) = write_key();
    let (_ok, text) = run_with(&path, &[("FN64_DISCOVER_DUMP_SECTIONS", "9")]);
    if text == "<binary absent>" {
        return;
    }
    assert!(
        text.contains("answer-key identity") && text.contains("has 2 sections, expected 9"),
        "expected a loud identity failure naming both counts, got:\n{text}"
    );
}

/// The guard must not fire when the declaration matches, and must not fire at
/// all when nothing is declared -- ad-hoc runs against a new key still work.
#[test]
fn a_matching_or_absent_declaration_does_not_trip_the_guard() {
    let (_dir, path) = write_key();
    for vars in [
        vec![
            ("FN64_DISCOVER_DUMP_FUNCTIONS", "3"),
            ("FN64_DISCOVER_DUMP_SECTIONS", "2"),
        ],
        vec![],
    ] {
        let (_ok, text) = run_with(&path, &vars);
        if text == "<binary absent>" {
            return;
        }
        assert!(
            !text.contains("answer-key identity"),
            "the identity guard fired for {vars:?}, which matches the key:\n{text}"
        );
    }
}

/// Minimal scratch-directory helper: the crate has no dev-dependency on
/// `tempfile`, and adding one for three tests is not worth the tree churn.
mod tempdir_shim {
    pub struct TempDir(std::path::PathBuf);

    impl TempDir {
        pub fn new() -> Self {
            let unique = format!(
                "fn64-key-identity-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .expect("clock")
                    .as_nanos()
            );
            let path = std::env::temp_dir().join(unique);
            std::fs::create_dir_all(&path).expect("creating scratch dir");
            Self(path)
        }

        pub fn path(&self) -> &std::path::Path {
            &self.0
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
}
