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

fn main() {
    println!("cargo:rerun-if-env-changed=FN64_RT64_DIR");
    println!("cargo:rerun-if-changed=ffi/CMakeLists.txt");
    println!("cargo:rerun-if-changed=ffi/fn64_rt64_shim.cpp");
    println!("cargo:rerun-if-changed=ffi/fn64_rt64_shim.h");

    if env::var_os("CARGO_FEATURE_RT64").is_none() {
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
    let license = rt64_dir.join("LICENSE");
    let license_text = std::fs::read_to_string(&license)
        .unwrap_or_else(|e| panic!("failed to read RT64 license {}: {e}", license.display()));
    assert!(
        license_text.contains("MIT License"),
        "RT64 source at {} does not carry the expected MIT license",
        rt64_dir.display()
    );

    let out_dir = PathBuf::from(env::var_os("OUT_DIR").unwrap());
    // RT64's spirv-cross helper redirects its generated files beneath
    // CMAKE_SOURCE_DIR. Configure from an OUT_DIR copy of this tiny wrapper
    // so enabling the feature never writes build products into the checkout.
    let cmake_source = out_dir.join("rt64-cmake-source");
    std::fs::create_dir_all(&cmake_source).expect("create RT64 CMake source wrapper");
    for file in ["CMakeLists.txt", "fn64_rt64_shim.cpp", "fn64_rt64_shim.h"] {
        let source = manifest_dir.join("ffi").join(file);
        let destination = cmake_source.join(file);
        std::fs::copy(&source, &destination).unwrap_or_else(|e| {
            panic!(
                "failed to stage {} as {}: {e}",
                source.display(),
                destination.display()
            )
        });
    }
    let build_dir = out_dir.join("rt64-cmake-build");
    std::fs::create_dir_all(&build_dir).expect("create RT64 CMake build directory");

    let mut configure = Command::new("cmake");
    configure
        .arg("-S")
        .arg(&cmake_source)
        .arg("-B")
        .arg(&build_dir)
        .arg(format!("-DFN64_RT64_SOURCE_DIR={}", rt64_dir.display()))
        .arg("-DRT64_STATIC=ON")
        .arg("-DBUILD_SHARED_LIBS=OFF")
        .arg("-DNFD_BUILD_TESTS=OFF")
        .arg("-DNFD_INSTALL=OFF")
        .arg("-DZSTD_BUILD_PROGRAMS=OFF")
        .arg("-DZSTD_BUILD_TESTS=OFF")
        .arg("-DPLUME_BUILD_EXAMPLES=OFF")
        .arg("-DCMAKE_BUILD_TYPE=Release");
    run(&mut configure, "RT64 CMake configure");

    let mut build = Command::new("cmake");
    build
        .arg("--build")
        .arg(&build_dir)
        .arg("--config")
        .arg("Release")
        .arg("--target")
        .arg("fn64_rt64_shim")
        .arg("--parallel");
    run(&mut build, "RT64 static core/HLE build");

    // CMake owns the transitive build graph, while Cargo owns the final link.
    // Publish every directory containing the exact static targets linked by
    // `rt64` plus this crate's shim. No mupen target exists or is built.
    let expected = [
        "libfn64_rt64_shim.a",
        "rt64.a",
        "librt64.a",
        "libre-spirv.a",
        "libnfd.a",
        "libzstd.a",
        "libzstd_static.a",
        "libplume.a",
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
    // RT64 deliberately removes the conventional `lib` prefix. Copying the
    // archive inside OUT_DIR gives rustc a normal `librt64.a` without
    // modifying or vendoring the sibling source checkout.
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
        panic!(
            "failed to stage {} as {}: {e}",
            rt64_archive.display(),
            cargo_rt64.display()
        )
    });
    println!("cargo:rustc-link-search=native={}", out_dir.display());

    // Static archive order is significant: the shim references RT64, and
    // RT64 references the four libraries that follow it.
    for (names, link_name) in [
        (&["libfn64_rt64_shim.a"][..], "fn64_rt64_shim"),
        (&["librt64.a", "rt64.a"][..], "rt64"),
        (&["libre-spirv.a"][..], "re-spirv"),
        (&["libnfd.a"][..], "nfd"),
        (&["libzstd.a", "libzstd_static.a"][..], "zstd"),
        (&["libplume.a"][..], "plume"),
    ] {
        assert!(
            link_name == "rt64" || has(names),
            "CMake did not produce expected static library {names:?}"
        );
        println!("cargo:rustc-link-lib=static={link_name}");
    }

    let sdl_libdir = Command::new("pkg-config")
        .args(["--variable=libdir", "sdl2"])
        .output()
        .ok()
        .filter(|output| output.status.success())
        .and_then(|output| String::from_utf8(output.stdout).ok())
        .map(|path| path.trim().to_string())
        .filter(|path| !path.is_empty())
        .expect("pkg-config could not locate SDL2's library directory");
    println!("cargo:rustc-link-search=native={sdl_libdir}");

    let target = env::var("TARGET").unwrap();
    if target.contains("apple-darwin") {
        println!("cargo:rustc-link-lib=dylib=c++");
        println!("cargo:rustc-link-lib=dylib=SDL2");
        for framework in [
            "AppKit",
            "Cocoa",
            "CoreFoundation",
            "CoreGraphics",
            "Foundation",
            "IOKit",
            "Metal",
            "QuartzCore",
            "UniformTypeIdentifiers",
        ] {
            println!("cargo:rustc-link-lib=framework={framework}");
        }
    } else if target.contains("linux") {
        println!("cargo:rustc-link-lib=dylib=stdc++");
        println!("cargo:rustc-link-lib=dylib=SDL2");
        for lib in ["X11", "Xrandr", "dl", "pthread"] {
            println!("cargo:rustc-link-lib=dylib={lib}");
        }
    }
}
