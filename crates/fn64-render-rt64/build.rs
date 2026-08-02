use std::env;
use std::path::{Path, PathBuf};
use std::process::Command;

#[path = "adapter_source_identity.rs"]
mod adapter_source_identity;

#[path = "../fn64-boot-harness/native_program_identity.rs"]
mod native_program_identity;

const RT64_SOURCE_OVERLAY_ID: &str = "fn64:raster-shader-start-stop:v1+vi-region-rate:v1+ucode-generation-admission:v1+vi-gamma-dither:v1+vi-dither-filter:v1+vi-divot:v1+vi-silhouette-aa:v1+vi-retrace-cadence:v1+rdp-alpha-dither:v1+rdp-shared-fragment-noise:v1+s2dex-object-rect:v3";
const RT64_HFR_SOURCE_OVERLAY_ID: &str = "fn64:raster-shader-start-stop:v1+vi-region-rate:v1+ucode-generation-admission:v1+vi-gamma-dither:v1+vi-dither-filter:v1+vi-divot:v1+vi-silhouette-aa:v1+vi-retrace-cadence:v1+rdp-alpha-dither:v1+rdp-shared-fragment-noise:v1+s2dex-object-rect:v3+hfr-post-present-call:v1";

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

fn rt64_source_identity(rt64_dir: &Path) -> (String, &'static str) {
    if let Some(declared) = env::var_os("FN64_RT64_SOURCE_ID") {
        let declared = declared.to_string_lossy();
        assert!(
            !declared.is_empty()
                && declared
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || b"-._:+/".contains(&byte)),
            "FN64_RT64_SOURCE_ID must be a nonempty stable identifier using ASCII letters, digits, or -._:+/"
        );
        return (format!("declared:{declared}"), "declared");
    }

    let revision = Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(rt64_dir)
        .output()
        .ok()
        .filter(|output| output.status.success())
        .and_then(|output| String::from_utf8(output.stdout).ok())
        .map(|revision| revision.trim().to_owned())
        .filter(|revision| {
            revision.len() == 40 && revision.bytes().all(|byte| byte.is_ascii_hexdigit())
        })
        .unwrap_or_else(|| {
            panic!(
                "RT64 source identity is unavailable; use a Git checkout or set FN64_RT64_SOURCE_ID"
            )
        });
    let status = Command::new("git")
        .args(["status", "--porcelain", "--", "."])
        .current_dir(rt64_dir)
        .output()
        .unwrap_or_else(|error| panic!("failed to inspect RT64 source status: {error}"));
    assert!(status.status.success(), "git status failed for RT64 source");
    let provenance = if status.stdout.is_empty() {
        "git-clean"
    } else {
        "git-dirty"
    };
    (format!("git:{revision}"), provenance)
}

fn produced_archive(out_dir: &Path, name: &str) -> PathBuf {
    let candidates = [
        out_dir.join(format!("lib{name}.a")),
        out_dir.join(format!("{name}.lib")),
    ];
    candidates
        .into_iter()
        .find(|path| path.is_file())
        .unwrap_or_else(|| {
            panic!(
                "synthetic native archive {name} was not produced in {}",
                out_dir.display()
            )
        })
}

fn lowercase_hex(bytes: [u8; 32]) -> String {
    let mut output = String::with_capacity(64);
    for byte in bytes {
        use std::fmt::Write as _;
        write!(&mut output, "{byte:02x}").expect("writing to String cannot fail");
    }
    output
}

fn build_synthetic_native_archives(manifest_dir: &Path) {
    let fixture_dir = manifest_dir.join("fixtures/synthetic_native_archive");
    let generated_source = fixture_dir.join("generated_code.c");
    let bridge_source = fixture_dir.join("section_bridge.c");
    println!("cargo:rerun-if-changed={}", generated_source.display());
    println!("cargo:rerun-if-changed={}", bridge_source.display());

    cc::Build::new()
        .file(&generated_source)
        .warnings(true)
        .compile("fn64_synthetic_generated_code");
    cc::Build::new()
        .file(&bridge_source)
        .warnings(true)
        .compile("fn64_synthetic_section_bridge");

    let out_dir = PathBuf::from(env::var_os("OUT_DIR").expect("Cargo must set OUT_DIR"));
    let generated_archive = produced_archive(&out_dir, "fn64_synthetic_generated_code");
    let bridge_archive = produced_archive(&out_dir, "fn64_synthetic_section_bridge");
    let generated_bytes = std::fs::read(&generated_archive).unwrap_or_else(|error| {
        panic!(
            "failed to read produced archive {}: {error}",
            generated_archive.display()
        )
    });
    let bridge_bytes = std::fs::read(&bridge_archive).unwrap_or_else(|error| {
        panic!(
            "failed to read produced archive {}: {error}",
            bridge_archive.display()
        )
    });
    let identity = lowercase_hex(native_program_identity::native_program_archives_sha256([
        ("synthetic-generated-code".to_owned(), generated_bytes),
        ("synthetic-section-bridge".to_owned(), bridge_bytes),
    ]));
    println!("cargo:rustc-env=FN64_SYNTHETIC_NATIVE_PROGRAM_SHA256={identity}");
    println!(
        "cargo:rustc-env=FN64_SYNTHETIC_GENERATED_ARCHIVE={}",
        generated_archive.display()
    );
    println!(
        "cargo:rustc-env=FN64_SYNTHETIC_BRIDGE_ARCHIVE={}",
        bridge_archive.display()
    );
}

fn main() {
    println!("cargo:rerun-if-env-changed=FN64_RT64_DIR");
    println!("cargo:rerun-if-env-changed=FN64_RT64_SOURCE_ID");
    println!("cargo:rerun-if-changed=ffi/CMakeLists.txt");
    println!("cargo:rerun-if-changed=ffi/fn64_rt64_shim.cpp");
    println!("cargo:rerun-if-changed=ffi/fn64_rt64_shim.h");

    let manifest_dir = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").unwrap());
    for path in adapter_source_identity::adapter_source_paths(&manifest_dir)
        .expect("enumerate fn64 RT64 adapter source inputs")
    {
        println!("cargo:rerun-if-changed={path}");
    }
    let target = env::var("TARGET").expect("Cargo must set TARGET");
    println!("cargo:rustc-env=FN64_SYNTHETIC_NATIVE_TARGET={target}");
    let enabled_features = env::vars_os()
        .filter_map(|(name, _)| {
            name.to_str()
                .and_then(|name| name.strip_prefix("CARGO_FEATURE_"))
                .map(str::to_owned)
        })
        .collect::<Vec<_>>();
    let adapter_source_sha256 = lowercase_hex(
        adapter_source_identity::adapter_source_sha256(&manifest_dir, &target, &enabled_features)
            .expect("hash fn64 RT64 adapter source inputs"),
    );
    println!("cargo:rustc-env=FN64_RT64_ADAPTER_SOURCE_SHA256={adapter_source_sha256}");
    if env::var_os("CARGO_FEATURE_SYNTHETIC_NATIVE_ARCHIVE_EVIDENCE").is_some() {
        build_synthetic_native_archives(&manifest_dir);
    }

    if env::var_os("CARGO_FEATURE_RT64").is_none() {
        return;
    }

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
    let (source_id, source_provenance) = rt64_source_identity(&rt64_dir);
    println!("cargo:rustc-env=FN64_RT64_SOURCE_ID={source_id}");
    println!("cargo:rustc-env=FN64_RT64_SOURCE_PROVENANCE={source_provenance}");
    let source_overlay_id = if env::var_os("CARGO_FEATURE_HFR_EVIDENCE").is_some() {
        RT64_HFR_SOURCE_OVERLAY_ID
    } else {
        RT64_SOURCE_OVERLAY_ID
    };
    println!("cargo:rustc-env=FN64_RT64_SOURCE_OVERLAY_ID={source_overlay_id}");

    let out_dir = PathBuf::from(env::var_os("OUT_DIR").unwrap());
    // RT64's spirv-cross helper redirects its generated files beneath
    // CMAKE_SOURCE_DIR. Configure from an OUT_DIR copy of this tiny wrapper
    // so enabling the feature never writes build products into the checkout.
    let cmake_source = out_dir.join("rt64-cmake-source");
    std::fs::create_dir_all(&cmake_source).expect("create RT64 CMake source wrapper");
    for file in [
        "CMakeLists.txt",
        "fn64_rt64_shim.cpp",
        "fn64_rt64_shim.h",
        "fn64_rt64_raster_ps_overlay.hlsli",
        "fn64_rt64_video_interface.h",
        "fn64_rt64_video_interface_ps.hlsl",
    ] {
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
        .arg(if env::var_os("CARGO_FEATURE_HFR_EVIDENCE").is_some() {
            "-DFN64_RT64_HFR_EVIDENCE=ON"
        } else {
            "-DFN64_RT64_HFR_EVIDENCE=OFF"
        })
        .arg(
            if env::var_os("CARGO_FEATURE_SYNTHETIC_F3DEX2_EVIDENCE").is_some() {
                "-DFN64_RT64_SYNTHETIC_F3DEX2_EVIDENCE=ON"
            } else {
                "-DFN64_RT64_SYNTHETIC_F3DEX2_EVIDENCE=OFF"
            },
        )
        .arg(
            if env::var_os("CARGO_FEATURE_SYNTHETIC_S2DEX_EVIDENCE").is_some() {
                "-DFN64_RT64_SYNTHETIC_S2DEX_EVIDENCE=ON"
            } else {
                "-DFN64_RT64_SYNTHETIC_S2DEX_EVIDENCE=OFF"
            },
        )
        .arg("-DCMAKE_BUILD_TYPE=Release");
    run(&mut configure, "RT64 CMake configure");

    let cargo_jobs =
        env::var("NUM_JOBS").expect("Cargo must publish NUM_JOBS to bound the RT64 native build");
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
        .arg("fn64_rt64_shim")
        .arg("--parallel")
        .arg(cargo_jobs);
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
