//! Links `tests/c_smoke/smoke.c` against the `fn64-abi` staticlib and runs
//! it, proving the extern "C" symbols this crate exports actually link and
//! callable exactly the way N64Recomp-generated `RecompiledFuncs/*.c` would
//! call them -- see `docs/DESIGN.md` section 1's "fn64-abi is deliberately
//! dumb" framing: this test is the mechanical check that the ABI-SURFACE.md
//! shape is honored, independent of any behavior.
//!
//! Deliberately shells out to the system `cc` rather than adding a build-time
//! dependency (the `cc` crate) -- there is exactly one C file, compiled
//! once, at test time; a build.rs would run this on every `cargo build` of
//! this crate for no benefit, since only this test needs it.

use std::path::PathBuf;
use std::process::Command;

/// Locates the staticlib cargo just built for this crate (`cargo test`
/// runs this integration test only after the lib target is built, so the
/// `.a` is guaranteed present in the same profile's `deps` output dir by
/// the time this test executes).
fn find_staticlib() -> PathBuf {
    // CARGO_TARGET_TMPDIR isn't quite what we want here; derive the
    // profile dir from the test binary's own path, which cargo places at
    // <target>/<profile>/deps/<test-binary>.
    let exe = std::env::current_exe().expect("current_exe");
    let deps_dir = exe.parent().expect("deps dir");
    let profile_dir = deps_dir.parent().expect("profile dir");

    for dir in [deps_dir, profile_dir] {
        let candidate = dir.join("libfn64_abi.a");
        if candidate.exists() {
            return candidate;
        }
    }
    // `cargo test` builds the staticlib crate-type as a side effect, but
    // `cargo nextest` builds only the test binaries -- so under nextest the
    // staticlib may not exist yet. Build it explicitly (idempotent, cached)
    // so the test is runner-agnostic instead of assuming cargo test's layout.
    let status =
        std::process::Command::new(std::env::var("CARGO").unwrap_or_else(|_| "cargo".into()))
            .args(["build", "-p", "fn64-abi"])
            .status()
            .expect("spawn cargo build -p fn64-abi for the staticlib");
    assert!(status.success(), "cargo build -p fn64-abi failed");
    for dir in [deps_dir, profile_dir] {
        let candidate = dir.join("libfn64_abi.a");
        if candidate.exists() {
            return candidate;
        }
    }
    panic!(
        "libfn64_abi.a not found under {:?} or {:?} even after `cargo build -p fn64-abi`",
        deps_dir, profile_dir
    );
}

#[test]
fn c_caller_links_and_runs_against_fn64_abi_staticlib() {
    let staticlib = find_staticlib();
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let smoke_c = manifest_dir.join("tests/c_smoke/smoke.c");
    let out_dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"));
    let out_bin = out_dir.join("fn64_abi_c_smoke");

    let mut cc = Command::new("cc");
    cc.arg(&smoke_c).arg(&staticlib).arg("-o").arg(&out_bin);
    // fn64-abi now statically links fn64-audio's cpal dependency (the real
    // CpalBackend, see fn64-audio's crate doc), which on macOS pulls in
    // CoreAudio/AudioToolbox/objc2 -- a plain `cc a.c lib.a` link needs
    // those frameworks named explicitly, the same way any other consumer
    // of a Rust staticlib with platform-audio deps would. Not needed on
    // other platforms (cpal's non-Darwin backends link via other means
    // already covered by the .a's own build).
    if cfg!(target_os = "macos") {
        cc.args([
            "-framework",
            "CoreAudio",
            "-framework",
            "AudioToolbox",
            "-framework",
            "CoreFoundation",
            "-framework",
            "Foundation",
            "-lobjc",
        ]);
    }
    let compile = cc
        .output()
        .expect("failed to invoke cc -- is a C toolchain installed?");

    assert!(
        compile.status.success(),
        "cc failed to compile+link smoke.c against {:?}:\nstdout: {}\nstderr: {}",
        staticlib,
        String::from_utf8_lossy(&compile.stdout),
        String::from_utf8_lossy(&compile.stderr)
    );

    let run = Command::new(&out_bin)
        .output()
        .unwrap_or_else(|e| panic!("failed to execute {:?}: {e}", out_bin));

    assert!(
        run.status.success(),
        "smoke test binary exited non-zero:\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&run.stdout),
        String::from_utf8_lossy(&run.stderr)
    );

    let stdout = String::from_utf8_lossy(&run.stdout);
    assert!(
        stdout.contains("fn64-abi C smoke test: linked and returned OK"),
        "unexpected smoke test output: {stdout}"
    );
}
