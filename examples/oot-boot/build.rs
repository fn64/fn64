//! Build script: compiles OoT's out-of-tree, game-derived
//! `RecompiledFuncs/*.c` (N64Recomp's own MIT-licensed generated output)
//! plus `fn64-boot-harness`'s shared `bridge/section_bridge.c` glue into a
//! static lib the harness links against.
//!
//! Identical shape to `examples/wm2000-boot/build.rs` (that file's doc
//! comment explains the clean-room rationale in full) -- copied verbatim
//! and only the RECOMP_H_DIR hint's example path changed, since OoT's
//! bring-up (aki-recomp/games/OOTU) built directly against upstream
//! N64Recomp's own `refs/N64RecompSource/include`, not through the
//! WCWnWoRevengeRecomp vendor path WM2000 used.
//!
//! Per `fn64/README.md`'s "no game content ships in this repo" rule and
//! `fn64/AGENTS.md`'s clean-room protocol: this crate contains ZERO game
//! bytes, ZERO recompiled function bodies, and ZERO copies of
//! `recomp_overlays.inl`. Every game-derived input comes from environment
//! variables pointing OUTSIDE this repo, at build time only, never
//! vendored/copied in.
//!
//! ## Required environment variables
//!
//! - `RECOMPILED_DIR` -- path to a directory containing the N64Recomp-
//!   generated `RecompiledFuncs/*.c`, `recomp_overlays.inl`, `recomp.h`,
//!   `funcs.h` for OoT (OOTU). In the reference dev environment, this is
//!   `aki-recomp/games/OOTU/RecompiledFuncs`.
//! - `RECOMP_H_DIR` -- OPTIONAL. N64Recomp's MIT-licensed `recomp.h` (the ABI
//!   this crate serves, per `docs/DESIGN.md` section 1's provenance table) is
//!   vendored at `crates/fn64-boot-harness/bridge/include/vendor/`; set this
//!   only to build against a different fork. (`librecomp/sections.h` is
//!   fn64's OWN clean-room header and never came from N64Recomp.)
//! - `ROM` -- path to the decomp's OWN BUILD-OUTPUT ROM (NOT the retail
//!   compressed cartridge image -- OoT's linker `.map`-derived symbol rom
//!   offsets only line up against the decomp's decompressed build output;
//!   see aki-recomp/games/OOTU/docs/decomp-recomp-notes.md's "ROM basis"
//!   section for the byte-cited reason this harness hit and fixed during
//!   bring-up). In the reference dev environment, this is
//!   `aki-recomp/refs/oot-decomp/build/ntsc-1.0/oot-ntsc-1.0.z64`. Never
//!   read by this build script (only by the runtime binary, at startup) --
//!   declared here only so a missing ROM is reported early with a clear
//!   message rather than a confusing runtime panic much later.
//!
//! No default paths are baked in (they would either point outside this
//! machine or, worse, tempt a future edit to hardcode a path into a
//! colleague's home directory) -- missing/wrong env vars fail the build
//! loudly with instructions, per `AGENTS.md`'s "loud traps, no silent
//! shrugs."
//!
//! `FN64_RECOMP=rs` selects the typed-Rust lane; `c` selects the C lane and is
//! the default. In rs mode,
//! `RECOMP_RS_DIR` must contain the out-of-tree standalone crate
//! emitted by `fn64-recomp-rs`; no C file or section bridge is compiled.

use std::env;
use std::path::PathBuf;

/// Locate `crates/fn64-boot-harness/bridge` by walking up from this manifest.
/// Shared by BOTH the main manifest (examples/oot-boot/) and the rs-only one
/// (examples/oot-boot/rs/, a level deeper), so a fixed `../../` breaks.
fn bridge_dir() -> PathBuf {
    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let rel = "crates/fn64-boot-harness/bridge";
    let mut d = manifest_dir.as_path();
    loop {
        let candidate = d.join(rel);
        if candidate.join("section_bridge.c").exists() {
            return candidate;
        }
        match d.parent() {
            Some(p) => d = p,
            None => panic!(
                "could not locate {rel} walking up from {}",
                manifest_dir.display()
            ),
        }
    }
}

/// The vendored MIT `recomp.h`, used unless RECOMP_H_DIR overrides it.
fn vendored_recomp_h_dir() -> PathBuf {
    let d = bridge_dir().join("include/vendor");
    assert!(
        d.join("recomp.h").is_file(),
        "oot-boot build.rs: vendored recomp.h missing at {} -- it ships in this repo (MIT, \
         see LICENSE-N64Recomp beside it); set RECOMP_H_DIR to override.",
        d.display()
    );
    d
}

fn required_env(name: &str, hint: &str) -> PathBuf {
    match env::var(name) {
        Ok(v) => PathBuf::from(v),
        Err(_) => {
            panic!(
                "oot-boot build.rs: required environment variable {name} is not set.\n\
                 {hint}\n\
                 This example harness contains zero game content (fn64/README.md's \"no game \
                 content ships in this repo\" rule) -- every game-derived input must be supplied \
                 out-of-tree via environment variables, never vendored into this repository."
            );
        }
    }
}

fn main() {
    println!("cargo:rerun-if-env-changed=RECOMPILED_DIR");
    println!("cargo:rerun-if-env-changed=RECOMP_H_DIR");
    println!("cargo:rerun-if-env-changed=ROM");
    println!("cargo:rerun-if-env-changed=FN64_RECOMP");
    println!("cargo:rerun-if-env-changed=RECOMP_RS_DIR");
    println!("cargo:rustc-check-cfg=cfg(fn64_recomp_rs)");

    let recompiled_rs = match env::var("FN64_RECOMP") {
        Ok(value) if value == "rs" => true,
        Ok(value) if value == "c" => false,
        Ok(value) => panic!(
            "oot-boot build.rs: FN64_RECOMP must be `rs` or `c`, got {value:?}"
        ),
        Err(env::VarError::NotPresent) => false,
        Err(env::VarError::NotUnicode(_)) => {
            panic!("oot-boot build.rs: FN64_RECOMP must be valid Unicode (`rs` or `c`)")
        }
    };

    // ROM is shared by both lanes and validated but not read here.
    let _ = required_env(
        "ROM",
        "Point it at the zeldaret/oot decomp's OWN build-output ROM (decompressed).",
    );

    if recompiled_rs {
        let recompiled_dir = required_env(
            "RECOMP_RS_DIR",
            "Point it at fn64-recomp-rs's out-of-tree crate output directory.",
        );
        let manifest = recompiled_dir.join("Cargo.toml");
        let lib = recompiled_dir.join("src/lib.rs");
        assert!(
            manifest.is_file() && lib.is_file(),
            "oot-boot build.rs: RECOMP_RS_DIR={} is not an emitted recompiled function crate (expected Cargo.toml + src/lib.rs)",
            recompiled_dir.display()
        );
        let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
        let dependency_link = manifest_dir.join("recompiled");
        let selected = recompiled_dir.canonicalize().unwrap_or_else(|e| {
            panic!(
                "oot-boot build.rs: failed to canonicalize RECOMP_RS_DIR={}: {e}",
                recompiled_dir.display()
            )
        });
        let linked = dependency_link.canonicalize().unwrap_or_else(|e| {
            panic!(
                "oot-boot build.rs: rs-lane dependency link {} is unavailable: {e}; invoke examples/oot-boot/oot so it can point the path dependency at RECOMP_RS_DIR",
                dependency_link.display()
            )
        });
        assert_eq!(
            linked,
            selected,
            "oot-boot build.rs: rs-lane path dependency resolves to {}, but RECOMP_RS_DIR resolves to {}; rerun through examples/oot-boot/oot to refresh it",
            linked.display(),
            selected.display()
        );
        println!("cargo:rustc-cfg=fn64_recomp_rs");
        println!("cargo:rerun-if-changed={}", manifest.display());
        println!("cargo:rerun-if-changed={}", lib.display());
        println!(
            "cargo:rerun-if-changed={}",
            recompiled_dir.join("src").display()
        );
        return;
    }

    let recompiled_dir = required_env(
        "RECOMPILED_DIR",
        "Point it at a directory containing OoT (OOTU)'s N64Recomp-generated \
         RecompiledFuncs/*.c + recomp_overlays.inl (e.g. aki-recomp/games/OOTU/RecompiledFuncs).",
    );
    // recomp.h is MIT (N64Recomp, (c) 2024 Wiseguy) and contains no game
    // content, so it is vendored at bridge/include/vendor/ -- see ROADMAP H1.
    // RECOMP_H_DIR still overrides, for testing against a different fork.
    let recomp_h_dir = env::var("RECOMP_H_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| vendored_recomp_h_dir());
    if !recompiled_dir.join("recomp_overlays.inl").exists() {
        panic!(
            "oot-boot build.rs: RECOMPILED_DIR={} does not contain recomp_overlays.inl -- \
             expected the directory N64Recomp emitted OoT's RecompiledFuncs/*.c into.",
            recompiled_dir.display()
        );
    }
    if !recomp_h_dir.join("recomp.h").exists() {
        panic!(
            "oot-boot build.rs: RECOMP_H_DIR={} does not contain recomp.h -- expected \
             N64Recomp's include/ directory.",
            recomp_h_dir.display()
        );
    }

    let bridge_dir = bridge_dir();

    let mut build = cc::Build::new();
    build
        // The shared bridge include comes FIRST: it holds fn64's clean-room
        // stand-in librecomp/sections.h, which must shadow any real
        // (GPL-3.0-licensed) librecomp/include on the search path -- see
        // that header's doc comment and section_bridge.c's.
        .include(bridge_dir.join("include"))
        .include(&recompiled_dir)
        .include(&recomp_h_dir)
        .flag_if_supported("-Wno-everything")
        // RecompiledFuncs/*.c is generated code with no warning hygiene of
        // its own (matches aki-recomp's own CMakeLists.txt build recipe for
        // this same source, per M1-WORKLIST.md's method section).
        .warnings(false);

    let mut c_file_count = 0usize;
    for entry in std::fs::read_dir(&recompiled_dir).unwrap_or_else(|e| {
        panic!(
            "oot-boot build.rs: failed to read RECOMPILED_DIR={}: {e}",
            recompiled_dir.display()
        )
    }) {
        let entry = entry.unwrap();
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) == Some("c") {
            build.file(&path);
            c_file_count += 1;
        }
    }
    assert!(
        c_file_count > 0,
        "oot-boot build.rs: found zero .c files in RECOMPILED_DIR={} -- expected N64Recomp's \
         generated RecompiledFuncs/*.c output.",
        recompiled_dir.display()
    );
    eprintln!(
        "oot-boot: compiling {c_file_count} RecompiledFuncs/*.c files from {}",
        recompiled_dir.display()
    );

    build.compile("oot_recompiled");

    // The bridge glue (section_bridge.c) is built SEPARATELY, as C++ --
    // recomp_overlays.inl's SectionTableEntry initializers use `nullptr`
    // (the file was generated for a C++ port build, per N64Recomp's own
    // codegen target), which is not valid in C. RecompiledFuncs/*.c itself
    // compiles fine as plain C, so only this one glue file needs the C++
    // compiler.
    let mut bridge_build = cc::Build::new();
    bridge_build
        .cpp(true)
        .include(bridge_dir.join("include"))
        .include(&recompiled_dir)
        .include(&recomp_h_dir)
        .flag_if_supported("-Wno-everything")
        .warnings(false)
        .file(bridge_dir.join("section_bridge.c"));
    bridge_build.compile("oot_bridge");

    println!(
        "cargo:rerun-if-changed={}",
        bridge_dir.join("section_bridge.c").display()
    );
    println!("cargo:rerun-if-changed={}", recompiled_dir.display());
}
