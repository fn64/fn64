use std::env;
use std::path::{Path, PathBuf};
use std::process::Command;

fn run(command: &mut Command, description: &str) {
    let status = command
        .status()
        .unwrap_or_else(|e| panic!("failed to run {description}: {e}"));
    assert!(status.success(), "{description} failed with {status}");
}

fn find_archives(root: &Path, names: &[&str]) -> Vec<PathBuf> {
    fn visit(dir: &Path, names: &[&str], found: &mut Vec<PathBuf>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                visit(&path, names, found);
            } else if path
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| names.contains(&name))
            {
                found.push(path);
            }
        }
    }
    let mut found = Vec::new();
    visit(root, names, &mut found);
    found
}

fn check_mit_license(dir: &Path, project_name: &str) {
    let license = dir.join("LICENSE.txt");
    let license = if license.is_file() {
        license
    } else {
        dir.join("LICENSE")
    };
    let license_text = std::fs::read_to_string(&license)
        .unwrap_or_else(|e| panic!("failed to read {project_name} license {}: {e}", license.display()));
    assert!(
        license_text.contains("MIT License"),
        "{project_name} source at {} does not carry the expected MIT license",
        dir.display()
    );
}

fn main() {
    println!("cargo:rerun-if-env-changed=FN64_RT64_DIR");
    println!("cargo:rerun-if-env-changed=FN64_RMLUI_DIR");
    println!("cargo:rerun-if-changed=ffi/CMakeLists.txt");
    println!("cargo:rerun-if-changed=ffi/fn64_rmlui_shim.cpp");
    println!("cargo:rerun-if-changed=ffi/fn64_rmlui_shim.h");

    if env::var_os("CARGO_FEATURE_RMLUI").is_none() {
        // Pure-Rust default for CI/no-GPU hosts, matching
        // fn64-render-rt64's own default-off `rt64` feature convention.
        return;
    }

    let manifest_dir = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").unwrap());

    let default_rt64 = manifest_dir.join("../../../no-mercy-recompiled/third_party/rt64");
    let rt64_dir = env::var_os("FN64_RT64_DIR")
        .map(PathBuf::from)
        .unwrap_or(default_rt64)
        .canonicalize()
        .unwrap_or_else(|e| {
            panic!("RT64 source checkout not found ({e}); set FN64_RT64_DIR to its MIT source tree")
        });
    check_mit_license(&rt64_dir, "RT64");

    let default_rmlui = manifest_dir
        .join("../../../no-mercy-recompiled/third_party/RecompFrontend/recompui/lib/RmlUi");
    let rmlui_dir = env::var_os("FN64_RMLUI_DIR")
        .map(PathBuf::from)
        .unwrap_or(default_rmlui)
        .canonicalize()
        .unwrap_or_else(|e| {
            panic!("RmlUi source checkout not found ({e}); set FN64_RMLUI_DIR to its MIT source tree")
        });
    check_mit_license(&rmlui_dir, "RmlUi");

    // fn64-render-rt64's own C shim header, for Fn64Rt64Context and the
    // overlay-draw registration calls this crate's shim calls into. Not a
    // Cargo dependency (this crate only needs the header at C++ compile
    // time, not fn64-render-rt64's Rust surface), so locate it by relative
    // path within the workspace rather than through Cargo metadata.
    let render_rt64_ffi_dir = manifest_dir
        .join("../fn64-render-rt64/ffi")
        .canonicalize()
        .unwrap_or_else(|e| {
            panic!("fn64-render-rt64/ffi not found relative to fn64-rmlui ({e})")
        });

    let out_dir = PathBuf::from(env::var_os("OUT_DIR").unwrap());
    let cmake_source = out_dir.join("rmlui-cmake-source");
    std::fs::create_dir_all(&cmake_source).expect("create fn64-rmlui CMake source wrapper");
    for file in ["CMakeLists.txt", "fn64_rmlui_shim.cpp", "fn64_rmlui_shim.h"] {
        let source = manifest_dir.join("ffi").join(file);
        let destination = cmake_source.join(file);
        std::fs::copy(&source, &destination).unwrap_or_else(|e| {
            panic!("failed to stage {} as {}: {e}", source.display(), destination.display())
        });
    }
    let build_dir = out_dir.join("rmlui-cmake-build");
    std::fs::create_dir_all(&build_dir).expect("create fn64-rmlui CMake build directory");

    let mut configure = Command::new("cmake");
    configure
        .arg("-S")
        .arg(&cmake_source)
        .arg("-B")
        .arg(&build_dir)
        .arg(format!("-DFN64_RT64_SOURCE_DIR={}", rt64_dir.display()))
        .arg(format!("-DFN64_RMLUI_SOURCE_DIR={}", rmlui_dir.display()))
        .arg(format!("-DFN64_RENDER_RT64_FFI_DIR={}", render_rt64_ffi_dir.display()))
        .arg("-DRT64_STATIC=ON")
        .arg("-DBUILD_SHARED_LIBS=OFF")
        .arg("-DNFD_BUILD_TESTS=OFF")
        .arg("-DNFD_INSTALL=OFF")
        .arg("-DZSTD_BUILD_PROGRAMS=OFF")
        .arg("-DZSTD_BUILD_TESTS=OFF")
        .arg("-DPLUME_BUILD_EXAMPLES=OFF")
        .arg("-DCMAKE_BUILD_TYPE=Release");
    run(&mut configure, "fn64-rmlui CMake configure");

    let cargo_jobs =
        env::var("NUM_JOBS").expect("Cargo must publish NUM_JOBS to bound the RmlUi/RT64 native build");
    assert!(
        cargo_jobs.parse::<usize>().is_ok_and(|jobs| jobs > 0),
        "Cargo NUM_JOBS must be a positive integer, got {cargo_jobs:?}"
    );
    let mut build = Command::new("cmake");
    build
        .arg("--build")
        .arg(&build_dir)
        .arg("--config")
        .arg("Release")
        .arg("--target")
        .arg("fn64_rmlui_shim")
        .arg("--parallel")
        .arg(&cargo_jobs);
    run(&mut build, "fn64-rmlui native build");

    let expected = [
        "libfn64_rmlui_shim.a",
        "rt64.a",
        "librt64.a",
        "libre-spirv.a",
        "libnfd.a",
        "libzstd.a",
        "libzstd_static.a",
        "libplume.a",
        // RmlUi's core module sets OUTPUT_NAME "rmlui" explicitly
        // (Source/Core/CMakeLists.txt), so its archive is librmlui.a, not
        // librmlui_core.a/libRmlUiCore.a as the CMake TARGET name
        // (rmlui_core) or its RmlUi::Core ALIAS might suggest. The
        // Debugger module has no such OUTPUT_NAME override, so it keeps
        // its raw target name, librmlui_debugger.a.
        //
        // Freetype is NOT in this list: RmlUi's CMake resolves it via
        // `find_package(Freetype)` against the SYSTEM install
        // (Homebrew on this machine), not a vendored/built copy, so it
        // never appears anywhere under this crate's own CMake build_dir
        // -- searching for it here would always fail. It's linked
        // separately below via pkg-config, the same pattern
        // fn64-render-rt64/build.rs already uses for SDL2.
        "librmlui.a",
        "librmlui_debugger.a",
    ];
    let archives = find_archives(&build_dir, &expected);
    for archive in &archives {
        if let Some(parent) = archive.parent() {
            println!("cargo:rustc-link-search=native={}", parent.display());
        }
    }

    let has = |names: &[&str]| {
        archives.iter().any(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| names.contains(&name))
        })
    };
    // Matches fn64-render-rt64/build.rs: RT64 removes the conventional
    // `lib` prefix, so stage a normal `librt64.a` in this crate's own
    // OUT_DIR without touching the vendored checkout.
    let rt64_archive = archives
        .iter()
        .find(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name == "rt64.a" || name == "librt64.a")
        })
        .expect("CMake did not produce the static RT64 core library");
    let cargo_rt64 = out_dir.join("librt64.a");
    std::fs::copy(rt64_archive, &cargo_rt64).unwrap_or_else(|e| {
        panic!("failed to stage {} as {}: {e}", rt64_archive.display(), cargo_rt64.display())
    });
    println!("cargo:rustc-link-search=native={}", out_dir.display());

    // Static archive order is significant: the shim references RmlUi and
    // RT64; RT64 references the libraries that follow it. Same discipline
    // as fn64-render-rt64/build.rs's own link-order comment.
    for (names, link_name) in [
        (&["libfn64_rmlui_shim.a"][..], "fn64_rmlui_shim"),
        (&["librmlui_debugger.a"][..], "rmlui_debugger"),
        (&["librmlui.a"][..], "rmlui"),
        (&["librt64.a", "rt64.a"][..], "rt64"),
        (&["libre-spirv.a"][..], "re-spirv"),
        (&["libnfd.a"][..], "nfd"),
        (&["libzstd.a", "libzstd_static.a"][..], "zstd"),
        (&["libplume.a"][..], "plume"),
    ] {
        // rt64.a is skipped here deliberately: it's staged and linked
        // separately below as librt64.a (RT64 drops the conventional `lib`
        // prefix, same as fn64-render-rt64/build.rs handles it).
        // rmlui_debugger is genuinely optional -- RmlUi's Debugger module
        // is added unconditionally in its own CMakeLists.txt today, but
        // this stays lenient in case a future RmlUi version/config gates
        // it, since fn64-rmlui does not need the debugger module at all.
        let optional = link_name == "rt64" || link_name == "rmlui_debugger";
        let found = has(names);
        assert!(
            optional || found,
            "CMake did not produce expected static library {names:?}"
        );
        if found && (link_name != "rt64") {
            println!("cargo:rustc-link-lib=static={link_name}");
        }
    }

    // Freetype, RmlUi's one real dependency, is resolved by RmlUi's own
    // CMake against the system install (confirmed on this machine:
    // Homebrew's Freetype 2.14.3), never built by this crate's CMake run
    // -- so it needs its own search path and link directive, the same
    // pkg-config pattern fn64-render-rt64/build.rs already uses for SDL2.
    let freetype_libdir = Command::new("pkg-config")
        .args(["--variable=libdir", "freetype2"])
        .output()
        .ok()
        .filter(|output| output.status.success())
        .and_then(|output| String::from_utf8(output.stdout).ok())
        .map(|path| path.trim().to_string())
        .filter(|path| !path.is_empty())
        .expect("pkg-config could not locate Freetype's library directory");
    println!("cargo:rustc-link-search=native={freetype_libdir}");
    println!("cargo:rustc-link-lib=dylib=freetype");

    let target = env::var("TARGET").unwrap();
    if target.contains("apple-darwin") {
        println!("cargo:rustc-link-lib=dylib=c++");
        for framework in ["CoreText", "CoreGraphics", "CoreFoundation"] {
            println!("cargo:rustc-link-lib=framework={framework}");
        }
    } else if target.contains("linux") {
        println!("cargo:rustc-link-lib=dylib=stdc++");
    }
}
