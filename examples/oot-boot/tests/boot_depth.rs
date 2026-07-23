//! Boot-depth regression gate: runs the real oot-boot binary and asserts OoT
//! still reaches its established frame-loop depth. Guards the hard-won boot
//! progress (region → DMA → scheduler → overlays → SRAM → continuous frame
//! loop) against a silent regression in a shared fn64 crate that the unit
//! tests wouldn't catch — the 231-swap depth was only ever verified by hand.
//!
//! Requires game content, which is NOT in the repo (per the no-ROM rule).
//! When RECOMPILED_DIR/RECOMP_H_DIR/ROM are set at RUN time this asserts the
//! swap floor; when unset at run time it skips (passes). Reads the harness's
//! own `VI swaps observed:` line rather than reaching into internals — the
//! binary is the contract.
//!
//! Caveat: this crate's `build.rs` hard-requires RECOMPILED_DIR/RECOMP_H_DIR
//! at BUILD time (it compiles the generated C), so `cargo test -p oot-boot`
//! only reaches this test at all when those two are set. The run-time skip
//! guards the case where the binary built (env was present) but a runner
//! invokes the test suite without re-exporting ROM. A fully content-free CI
//! lane must therefore exclude this crate from the build, not rely on the
//! skip — the skip is a convenience, not a content-free-build guarantee.

use std::process::Command;

/// The floor, not the exact count: OoT reaches 230-231 double-buffered swaps
/// within the 2M-step budget. Assert a conservative minimum so normal
/// run-to-run variance (a frame more or less) doesn't flake, but a real
/// regression (boot dies early → single-digit or zero swaps) trips it.
const MIN_EXPECTED_SWAPS: u64 = 200;

#[test]
fn oot_reaches_continuous_frame_loop() {
    // Skip (pass) without game content — keeps CI green without shipping a ROM.
    let (Ok(recompiled), Ok(recomp_h), Ok(rom)) = (
        std::env::var("RECOMPILED_DIR"),
        std::env::var("RECOMP_H_DIR"),
        std::env::var("ROM"),
    ) else {
        eprintln!(
            "boot_depth: SKIP — set RECOMPILED_DIR/RECOMP_H_DIR/ROM to run the real OoT boot gate"
        );
        return;
    };

    // Run the already-built binary directly (Cargo exports its path to
    // integration tests as CARGO_BIN_EXE_<name>). Invoking `cargo run` from
    // inside `cargo test` nests cargo and mangles output capture — run the
    // binary itself.
    let output = Command::new(env!("CARGO_BIN_EXE_oot-boot"))
        .env("RECOMPILED_DIR", recompiled)
        .env("RECOMP_H_DIR", recomp_h)
        .env("ROM", rom)
        .env("RUST_BACKTRACE", "0")
        .output()
        .expect("failed to launch oot-boot");

    assert!(
        output.status.success(),
        "oot-boot did not complete normal process teardown: {}",
        output.status
    );

    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let swaps = combined
        .lines()
        .find_map(|l| l.strip_prefix("[oot-boot] VI swaps observed: "))
        .and_then(|n| n.trim().parse::<u64>().ok())
        .expect("normally exited oot-boot omitted its VI summary");

    if swaps == 0 {
        panic!(
            "boot_depth: 0 VI swaps — OoT boot regressed before reaching the frame loop.\n\
             --- last output ---\n{}",
            combined
                .lines()
                .rev()
                .take(20)
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .collect::<Vec<_>>()
                .join("\n")
        );
    }

    assert!(
        swaps >= MIN_EXPECTED_SWAPS,
        "boot_depth: OoT reached only {swaps} VI swaps (floor {MIN_EXPECTED_SWAPS}) — the frame \
         loop regressed. A shared-crate change likely broke boot without failing a unit test."
    );
    eprintln!("boot_depth: OK — {swaps} VI swaps (>= {MIN_EXPECTED_SWAPS})");
}
