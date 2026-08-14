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
    println!("cargo:rerun-if-changed=ffi/fn64_rmlui_render_interface.cpp");
    println!("cargo:rerun-if-changed=ffi/fn64_rmlui_render_interface.h");
    println!("cargo:rerun-if-changed=ffi/fn64_rmlui_ui.h");
    println!("cargo:rerun-if-changed=ffi/fn64_rmlui_ui_vs.hlsl");
    println!("cargo:rerun-if-changed=ffi/fn64_rmlui_ui_ps.hlsl");

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
    for file in [
        "CMakeLists.txt",
        "fn64_rmlui_shim.cpp",
        "fn64_rmlui_shim.h",
        "fn64_rmlui_render_interface.cpp",
        "fn64_rmlui_render_interface.h",
        "fn64_rmlui_ui.h",
        "fn64_rmlui_ui_vs.hlsl",
        "fn64_rmlui_ui_ps.hlsl",
    ] {
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
    // This crate builds its OWN independent copy of RT64 (see this file's
    // and ffi/CMakeLists.txt's own comments on why: no supported way to
    // share one CMake build tree across two crates' build.rs invocations).
    // That copy is not just a duplicate: it additionally compiles this
    // crate's own fn64_rmlui_ui_vs.hlsl/fn64_rmlui_ui_ps.hlsl shader blobs
    // into it (ffi/CMakeLists.txt's own shader-compilation block attaches
    // them to `rt64` via `add_dependencies(rt64 fn64_rmlui_ui_shaders)`),
    // so it is NOT symbol-for-symbol interchangeable with
    // fn64-render-rt64's own independently-built `rt64.a`, even though both
    // archives share the same on-disk filename.
    //
    // A prior version of this file treated the two as interchangeable --
    // staging this crate's rt64.a under the generic name `librt64.a` but
    // then deliberately skipping `-lrt64` for it (reasoning that
    // fn64-render-rt64's own `-lrt64` would already satisfy the linker in
    // any binary that links both crates). That was silently wrong the
    // moment the two archives diverged: `wm2000-shell` (which links both
    // fn64-render-rt64 and fn64-rmlui) resolved the single ambiguous
    // `-lrt64` to fn64-render-rt64's copy, which has none of
    // Fn64RmluiRenderInterface's shader symbols, producing undefined-symbol
    // linker errors for Fn64RmluiUi{VS,PS}Blob{SPIRV,MSL} -- caught by
    // actually linking wm2000-shell end to end, which fn64-rmlui's own
    // standalone `cargo build -p fn64-rmlui` never exercises (it has only
    // one `rt64.a` in scope, so the collision is invisible there).
    //
    // Fixed by staging this crate's rt64.a under a name that cannot collide
    // with fn64-render-rt64's own `rt64` link name, and always linking it.
    // Duplicate RT64 symbols between the two archives (everything except
    // this crate's added shaders) are resolved by ordinary static-linking
    // "first definition wins, unreferenced symbols in later archives are
    // simply not pulled in" semantics -- this crate's `rt64` copy is listed
    // in the link line, so ITS shader symbols get pulled in even where
    // fn64-render-rt64's identically-named non-shader symbols are also
    // present and already satisfied by its own archive.
    let rt64_archive = archives
        .iter()
        .find(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name == "rt64.a" || name == "librt64.a")
        })
        .expect("CMake did not produce the static RT64 core library");
    let cargo_rt64 = out_dir.join("libfn64_rmlui_rt64.a");
    std::fs::copy(rt64_archive, &cargo_rt64).unwrap_or_else(|e| {
        panic!("failed to stage {} as {}: {e}", rt64_archive.display(), cargo_rt64.display())
    });
    println!("cargo:rustc-link-search=native={}", out_dir.display());
    println!("cargo:rustc-link-lib=static=fn64_rmlui_rt64");

    // Static archive order is significant: the shim references RmlUi and
    // RT64; RT64 references the libraries that follow it. Same discipline
    // as fn64-render-rt64/build.rs's own link-order comment. rt64.a is
    // excluded from this loop: it is staged and linked separately above,
    // under its own fn64_rmlui_rt64 name, per this function's own comment
    // on why it cannot share fn64-render-rt64's `-lrt64` directive.
    for (names, link_name) in [
        (&["libfn64_rmlui_shim.a"][..], "fn64_rmlui_shim"),
        (&["librmlui_debugger.a"][..], "rmlui_debugger"),
        (&["librmlui.a"][..], "rmlui"),
        (&["libre-spirv.a"][..], "re-spirv"),
        (&["libnfd.a"][..], "nfd"),
        (&["libzstd.a", "libzstd_static.a"][..], "zstd"),
        (&["libplume.a"][..], "plume"),
    ] {
        // rmlui_debugger is genuinely optional -- RmlUi's Debugger module
        // is added unconditionally in its own CMakeLists.txt today, but
        // this stays lenient in case a future RmlUi version/config gates
        // it, since fn64-rmlui does not need the debugger module at all.
        let optional = link_name == "rmlui_debugger";
        let found = has(names);
        assert!(
            optional || found,
            "CMake did not produce expected static library {names:?}"
        );
        if found {
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
