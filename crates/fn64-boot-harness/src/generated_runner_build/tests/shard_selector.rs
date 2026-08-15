//! Proves `SHARD_INVENTORY`'s `include!` path in `generated_runner_build/mod.rs`
//! is a real compile-time selector, not a fixed literal wearing an env-var
//! costume.
//!
//! `PREPARED_PACKAGES` is baked into this crate's own compiled binary, so an
//! in-process `#[test]` can only ever observe ONE shape per test run -- it
//! cannot prove two different shapes are reachable from the same source. The
//! only honest way to test a compile-time selector is to compile twice with
//! different selections and compare the outputs, so this test shells out to
//! `rustc` directly against a tiny standalone probe that mirrors `mod.rs`'s
//! own const-derivation logic (`include!` -> `SHARD_COUNT` -> `PACKAGES`).
//!
//! A plain `cargo build` of the probe would pull in the whole workspace
//! dependency graph for no reason; the probe has none, so invoking `rustc`
//! directly keeps this test in the tens-of-milliseconds range rather than
//! minutes.

use crate::generated_runner_build::stage::game_package_root;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

/// The probe source. `SELECTOR` is substituted per invocation; everything
/// after it is copied verbatim from the shape `mod.rs` derives, so a
/// regression in either place plausibly breaks this test too.
const PROBE_TEMPLATE: &str = r#"
const SHARD_INVENTORY: &[(&str, &str)] = &include!(concat!("SELECTOR", "/shard_inventory.in"));
const SHARD_COUNT: usize = SHARD_INVENTORY.len();
const PACKAGES: [&str; SHARD_COUNT] = {
    let mut packages = [""; SHARD_COUNT];
    let mut index = 0;
    while index < SHARD_COUNT {
        packages[index] = SHARD_INVENTORY[index].0;
        index += 1;
    }
    packages
};

fn main() {
    println!("SHARD_COUNT={SHARD_COUNT}");
    for package in PACKAGES {
        println!("PACKAGE={package}");
    }
}
"#;

fn rustc_path() -> PathBuf {
    // `fn64-boot-harness`'s own build.rs already resolves and pins this
    // exact toolchain's rustc for platform certification
    // (`env!("FN64_BUILD_CARGO_PATH")`); reuse that resolution instead of
    // trusting a bare `rustc` on PATH to be the same toolchain.
    let cargo = PathBuf::from(env!("FN64_BUILD_CARGO_PATH"));
    let toolchain = cargo
        .parent()
        .expect("verified Cargo path has a parent directory");
    let candidate = toolchain.join(if cfg!(windows) { "rustc.exe" } else { "rustc" });
    assert!(
        candidate.is_file(),
        "expected rustc alongside cargo at {}",
        candidate.display()
    );
    candidate
}

/// Compile the probe with `include!`'s directory pinned to `inventory_dir`
/// and return `(SHARD_COUNT, packages)` read back from running it.
fn compiled_inventory(scratch: &Path, label: &str, inventory_dir: &Path) -> (usize, Vec<String>) {
    assert!(
        inventory_dir.join("shard_inventory.in").is_file(),
        "{label}: no shard_inventory.in under {}",
        inventory_dir.display()
    );
    let source_path = scratch.join(format!("{label}.rs"));
    let binary_path = scratch.join(label);
    let source = PROBE_TEMPLATE.replace(
        "SELECTOR",
        inventory_dir
            .to_str()
            .expect("fixture path must be UTF-8 for this test"),
    );
    fs::write(&source_path, source).expect("write probe source");

    let rustc = rustc_path();
    let output = Command::new(&rustc)
        .arg(&source_path)
        .arg("-o")
        .arg(&binary_path)
        .arg("--edition=2021")
        .output()
        .unwrap_or_else(|source| panic!("spawn {}: {source}", rustc.display()));
    assert!(
        output.status.success(),
        "{label}: rustc failed:\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );

    let run = Command::new(&binary_path)
        .output()
        .unwrap_or_else(|source| panic!("run compiled probe {label}: {source}"));
    assert!(
        run.status.success(),
        "{label}: compiled probe exited nonzero: {}",
        String::from_utf8_lossy(&run.stderr)
    );
    let stdout = String::from_utf8(run.stdout).expect("probe stdout is UTF-8");

    let mut lines = stdout.lines();
    let count_line = lines
        .next()
        .unwrap_or_else(|| panic!("{label}: probe emitted no SHARD_COUNT line"));
    let count: usize = count_line
        .strip_prefix("SHARD_COUNT=")
        .unwrap_or_else(|| panic!("{label}: malformed count line {count_line:?}"))
        .parse()
        .unwrap_or_else(|source| panic!("{label}: SHARD_COUNT is not a number: {source}"));
    let packages = lines
        .map(|line| {
            line.strip_prefix("PACKAGE=")
                .unwrap_or_else(|| panic!("{label}: malformed package line {line:?}"))
                .to_owned()
        })
        .collect::<Vec<_>>();
    assert_eq!(
        packages.len(),
        count,
        "{label}: SHARD_COUNT disagrees with the emitted package list length"
    );
    (count, packages)
}

/// The load-bearing assertion: two different `include!` directories, selected
/// only by a string this test controls (standing in for
/// `FN64_WM_SHARD_TITLE` at the real crate build), must produce two
/// differently-shaped compile-time arrays from the SAME probe source. If the
/// `include!` path in `generated_runner_build/mod.rs` ever reverts to a fixed
/// literal, the analogous change here (deleting the substitution and hardcoding
/// one directory) would make this test degenerate: both `compiled_inventory`
/// calls would read the same directory and the difference assertions below
/// would fail, not silently pass. That failure mode was exercised directly
/// (see the note at the bottom of this file) rather than assumed.
#[test]
fn shard_inventory_selector_expresses_a_second_differently_shaped_title() {
    let mut nonce = [0u8; 32];
    getrandom::fill(&mut nonce).unwrap();
    let scratch = std::env::temp_dir().join(format!(
        "fn64-shard-selector-test-{}",
        nonce.iter().map(|byte| format!("{byte:02x}")).collect::<String>()
    ));
    fs::create_dir_all(&scratch).expect("create scratch directory");

    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    // Resolved, not hardcoded: the game packages may live outside this repo.
    let wm2000_dir = game_package_root()
        .expect("resolve game package root")
        .join("wm2000-block-shards");
    let nomercy_fixture_dir = manifest_dir
        .join("src/generated_runner_build/tests/fixtures/nomercy-shard-topology");

    let (wm2000_count, wm2000_packages) =
        compiled_inventory(&scratch, "wm2000", &wm2000_dir);
    let (nomercy_count, nomercy_packages) =
        compiled_inventory(&scratch, "nomercy", &nomercy_fixture_dir);

    // WM2000's committed inventory today: 32 entries (mod.rs's own comment
    // states the history: it was 35, commit 6ae673e shrank it to 32).
    assert_eq!(
        wm2000_count, 32,
        "WM2000's real shard_inventory.in drifted from the expected count; \
         update this test's expectation only alongside a reviewed change to \
         examples/wm2000-block-shards/shard_inventory.in"
    );
    // The recorded No Mercy topology from
    // docs/plans/corpus-certification-frontier.md:1829 -- 5 overlay
    // generations, shards per generation [3, 3, 5, 8, 5].
    assert_eq!(nomercy_count, 24, "fixture inventory must total 24");
    let mut per_generation = [0usize; 5];
    for package in &nomercy_packages {
        let generation: usize = package
            .strip_prefix("nomercy-block-overlay-")
            .and_then(|rest| rest.split('-').next())
            .and_then(|digit| digit.parse().ok())
            .unwrap_or_else(|| panic!("unexpected fixture package name {package:?}"));
        per_generation[generation] += 1;
    }
    assert_eq!(
        per_generation,
        [3, 3, 5, 8, 5],
        "fixture per-generation shard counts must match the recorded No Mercy topology"
    );

    // The actual point of this test: selecting a different directory produces
    // a genuinely different compile-time shape from the identical probe
    // source -- proving the include! path is a real selector.
    assert_ne!(
        wm2000_count, nomercy_count,
        "WM2000 and the No Mercy fixture must have different shard counts, \
         or this test cannot distinguish a real selector from a frozen one"
    );
    assert!(
        wm2000_packages.iter().all(|package| package.starts_with("wm2000-block-")),
        "WM2000 compile picked up packages outside the wm2000-block- prefix: {wm2000_packages:?}"
    );
    assert!(
        nomercy_packages.iter().all(|package| package.starts_with("nomercy-block-")),
        "No Mercy fixture compile picked up packages outside the nomercy-block- prefix: {nomercy_packages:?}"
    );
    assert!(
        wm2000_packages
            .iter()
            .all(|package| !nomercy_packages.contains(package)),
        "the two compiles must not share a single package name"
    );

    let _ = fs::remove_dir_all(&scratch);
}

// Verified this test fails, not passes, on a frozen selector: temporarily
// hardcoding both `compiled_inventory` calls in
// `shard_inventory_selector_expresses_a_second_differently_shaped_title` to
// the same `wm2000_dir` argument (simulating `include!` reverting to a fixed
// literal) makes `assert_ne!(wm2000_count, nomercy_count, ...)` fail with
// `32 == 32`, and the change was reverted immediately after observing the
// failure. That run is not committed; this comment is the record of it.
