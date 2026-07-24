//! Links `tests/c_smoke/smoke.c` against the `fn64-abi` staticlib and runs
//! it, proving the extern "C" symbols this crate exports actually link and
//! callable exactly the way N64Recomp-generated `RecompiledFuncs/*.c` would
//! call them -- see `docs/DESIGN.md` section 1's "fn64-abi is deliberately
//! dumb" framing: this test is the mechanical check that the ABI-SURFACE.md
//! shape is honored, independent of any behavior.
//!
//! Deliberately shells out to the system C/C++ compilers rather than adding a
//! build-time dependency: one plain-C ABI caller and one generated-C-shaped
//! C++ MMIO-proxy caller compile only when this integration test runs.

use std::path::PathBuf;
use std::process::Command;

/// Locates the staticlib cargo just built for this crate (`cargo test`
/// runs this integration test only after the lib target is built, so the
/// `.a` is guaranteed present in the same profile's `deps` output dir by
/// the time this test executes).
fn find_staticlib() -> PathBuf {
    // Derive the exact target/profile before spawning the nested build.
    // `cargo --target-dir ... test` does not export CARGO_TARGET_DIR to the
    // test process, so an unqualified nested build would silently place the
    // archive in the workspace default while this test searches the fresh
    // target that owns its executable.
    let exe = std::env::current_exe().expect("current_exe");
    let deps_dir = exe.parent().expect("deps dir");
    let profile_dir = deps_dir.parent().expect("profile dir");
    let target_dir = profile_dir.parent().expect("target dir");
    let profile = profile_dir
        .file_name()
        .and_then(|name| name.to_str())
        .expect("UTF-8 cargo profile directory");

    // A prior staticlib can exist while `cargo test` has rebuilt only the
    // test/rlib targets. Refresh it unconditionally so new exported ABI hooks
    // cannot be hidden by a stale archive that merely has the expected name.
    let mut cargo =
        std::process::Command::new(std::env::var("CARGO").unwrap_or_else(|_| "cargo".into()));
    cargo
        .args(["build", "-p", "fn64-abi", "--target-dir"])
        .arg(target_dir);
    match profile {
        "debug" => {}
        "release" => {
            cargo.arg("--release");
        }
        custom => {
            cargo.args(["--profile", custom]);
        }
    }
    if cfg!(feature = "recomp-rs") {
        cargo.args(["--features", "recomp-rs"]);
    }
    let status = cargo
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
    // CpalBackend, see fn64-audio's crate doc), which pulls in the platform
    // audio stack -- a plain `cc a.c lib.a` link needs those named
    // explicitly, the same way any other consumer of a Rust staticlib with
    // platform-audio deps would. A staticlib records no dependency libs, so
    // each host's cpal backend has to be named here.
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
    } else if cfg!(target_os = "linux") {
        // cpal's ALSA backend: libasound provides snd_pcm_*/snd_hctl_*.
        cc.arg("-lasound");
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

    let proxy_cpp = manifest_dir.join("tests/c_smoke/mmio_proxy.cpp");
    let bridge_include = manifest_dir.join("../fn64-boot-harness/bridge/include");
    let vendor_include = bridge_include.join("vendor");
    let proxy_bin = out_dir.join("fn64_abi_mmio_proxy_smoke");
    let mut cxx = Command::new(std::env::var("CXX").unwrap_or_else(|_| "c++".into()));
    cxx.arg("-std=c++17")
        .arg(&proxy_cpp)
        .arg(&staticlib)
        .arg("-I")
        .arg(&bridge_include)
        .arg("-I")
        .arg(&vendor_include)
        .arg("-o")
        .arg(&proxy_bin);
    if cfg!(target_os = "macos") {
        cxx.args([
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
    } else if cfg!(target_os = "linux") {
        cxx.arg("-lasound");
    }
    let proxy_compile = cxx
        .output()
        .expect("failed to invoke C++ compiler for generated-C MMIO proxy smoke");
    assert!(
        proxy_compile.status.success(),
        "C++ compiler failed for MMIO proxy smoke:\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&proxy_compile.stdout),
        String::from_utf8_lossy(&proxy_compile.stderr)
    );
    let proxy_run = Command::new(&proxy_bin)
        .output()
        .unwrap_or_else(|error| panic!("failed to execute {:?}: {error}", proxy_bin));
    assert!(
        proxy_run.status.success(),
        "MMIO proxy smoke exited non-zero:\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&proxy_run.stdout),
        String::from_utf8_lossy(&proxy_run.stderr)
    );
    assert!(
        String::from_utf8_lossy(&proxy_run.stdout)
            .contains("fn64 generated-C MMIO proxy: live DeviceFabric round-trip OK"),
        "unexpected MMIO proxy output: {}",
        String::from_utf8_lossy(&proxy_run.stdout)
    );
    let bad_width = Command::new(&proxy_bin)
        .arg("--bad-width")
        .output()
        .unwrap_or_else(|error| panic!("failed to execute {:?}: {error}", proxy_bin));
    assert!(
        !bad_width.status.success(),
        "generated-C subword MMIO must trap loudly rather than bypass DeviceFabric"
    );
    for argument in [
        "--bad-kuseg",
        "--bad-kseg2",
        "--bad-noncanonical-sparse",
        "--bad-pif-kuseg",
        "--bad-pif-kseg2",
    ] {
        let bad_address = Command::new(&proxy_bin)
            .arg(argument)
            .output()
            .unwrap_or_else(|error| {
                panic!("failed to execute {:?} {argument}: {error}", proxy_bin)
            });
        assert!(
            !bad_address.status.success(),
            "generated-C {argument} access must trap before dereferencing host storage"
        );
        assert!(
            String::from_utf8_lossy(&bad_address.stderr)
                .contains("only zero- or sign-extended KSEG0/KSEG1 are modeled"),
            "generated-C {argument} trap lost its named address diagnostic: {}",
            String::from_utf8_lossy(&bad_address.stderr)
        );
    }
    for argument in [
        "--bad-dword-read",
        "--bad-dword-write",
        "--bad-swl",
        "--bad-swr",
    ] {
        let bad_width = Command::new(&proxy_bin)
            .arg(argument)
            .output()
            .unwrap_or_else(|error| {
                panic!("failed to execute {:?} {argument}: {error}", proxy_bin)
            });
        assert!(
            !bad_width.status.success(),
            "generated-C {argument} must trap before decomposing into word MMIO"
        );
        assert!(
            String::from_utf8_lossy(&bad_width.stderr)
                .contains("RCP registers require modeled word semantics"),
            "generated-C {argument} trap lost its named width diagnostic: {}",
            String::from_utf8_lossy(&bad_width.stderr)
        );
    }
    for argument in ["--bad-unaligned-word-read", "--bad-unaligned-half-write"] {
        let unaligned = Command::new(&proxy_bin)
            .arg(argument)
            .output()
            .unwrap_or_else(|error| {
                panic!("failed to execute {:?} {argument}: {error}", proxy_bin)
            });
        assert!(
            !unaligned.status.success(),
            "generated-C {argument} must trap before dereferencing host storage"
        );
        assert!(
            String::from_utf8_lossy(&unaligned.stderr).contains("unaligned guest address"),
            "generated-C {argument} trap lost its alignment diagnostic: {}",
            String::from_utf8_lossy(&unaligned.stderr)
        );
    }
}
