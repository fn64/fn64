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

/// The system libraries a bare `cc`/`c++` link against `libfn64_abi.a` must
/// name, straight from the compiler.
///
/// A staticlib archive records no dependency libs of its own: the
/// `cargo:rustc-link-lib` directives its crates emit apply only when *rustc*
/// drives the link, so a plain `cc a.c lib.a` has to repeat every transitive
/// system dep. That set is platform-specific (CoreAudio frameworks on macOS,
/// ALSA + libm on Linux) and it grows whenever a dependency does -- hardcoding
/// it per-OS means discovering each addition as a link failure.
///
/// `rustc --print native-static-libs` reports exactly that set for the current
/// target, so ask rather than guess.
fn native_static_libs() -> Vec<String> {
    let mut cargo =
        Command::new(std::env::var("CARGO").unwrap_or_else(|_| "cargo".into()));
    cargo.args(["rustc", "-p", "fn64-abi", "--lib"]);
    if cfg!(feature = "recomp-rs") {
        cargo.args(["--features", "recomp-rs"]);
    }
    cargo.args(["--", "--print", "native-static-libs"]);
    let out = cargo
        .output()
        .expect("spawn cargo rustc --print native-static-libs");
    let stderr = String::from_utf8_lossy(&out.stderr);
    let line = stderr
        .lines()
        .find_map(|l| l.trim().strip_prefix("note: native-static-libs:"))
        .unwrap_or_else(|| {
            panic!("rustc did not report native-static-libs:\n{stderr}");
        });
    // Deduplicate while preserving order: rustc repeats entries (e.g. -lSystem,
    // -framework Foundation), and a repeated `-framework` would otherwise pair
    // with the wrong following token.
    let mut seen = Vec::new();
    let tokens: Vec<&str> = line.split_whitespace().collect();
    let mut index = 0;
    while index < tokens.len() {
        let entry = if tokens[index] == "-framework" && index + 1 < tokens.len() {
            index += 2;
            vec![tokens[index - 2].to_string(), tokens[index - 1].to_string()]
        } else {
            index += 1;
            vec![tokens[index - 1].to_string()]
        };
        if !seen.contains(&entry) {
            seen.push(entry);
        }
    }
    seen.into_iter().flatten().collect()
}

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

    // fn64-abi statically links fn64-audio's cpal dependency (the real
    // CpalBackend, see fn64-audio's crate doc), so the link pulls in the host
    // audio stack and libm on top of plain libc. System libs must follow the
    // archive on the command line: GNU ld resolves left to right.
    let native_libs = native_static_libs();

    let mut cc = Command::new("cc");
    cc.arg(&smoke_c).arg(&staticlib).arg("-o").arg(&out_bin);
    cc.args(&native_libs);
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
    cxx.args(&native_libs);
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
