//! Build script: compiles OoT's out-of-tree, game-derived
//! `RecompiledFuncs/*.c` (N64Recomp's own MIT-licensed generated output)
//! plus this example's hand-written `bridge/section_bridge.c` glue into a
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
//! - `RECOMP_H_DIR` -- path to the directory containing N64Recomp's own
//!   MIT-licensed `recomp.h`/`librecomp/sections.h` headers (the ABI this
//!   crate serves, per `docs/DESIGN.md` section 1's provenance table). In
//!   the reference dev environment, this is
//!   `aki-recomp/refs/N64RecompSource/include`.
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
//! `FN64_NATIVE_RECOMP=1` selects the typed-Rust lane. In that mode
//! `NATIVE_RECOMPILED_DIR` must contain the out-of-tree `funcs.rs` emitted by
//! `fn64-recomp-native`; no C file or section bridge is compiled.

use std::env;
use std::path::PathBuf;

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
    println!("cargo:rerun-if-env-changed=FN64_NATIVE_RECOMP");
    println!("cargo:rerun-if-env-changed=NATIVE_RECOMPILED_DIR");
    println!("cargo:rustc-check-cfg=cfg(fn64_native_recomp)");

    let native = env::var_os("FN64_NATIVE_RECOMP").is_some_and(|v| v != "0");

    // ROM is shared by both lanes and validated but not read here.
    let _ = required_env(
        "ROM",
        "Point it at the zeldaret/oot decomp's OWN build-output ROM (decompressed).",
    );

    if native {
        let native_dir = required_env(
            "NATIVE_RECOMPILED_DIR",
            "Point it at fn64-recomp-native's out-of-tree output directory containing funcs.rs.",
        );
        let funcs = native_dir.join("funcs.rs");
        assert!(
            funcs.is_file(),
            "oot-boot build.rs: NATIVE_RECOMPILED_DIR={} has no funcs.rs",
            native_dir.display()
        );
        println!("cargo:rustc-cfg=fn64_native_recomp");
        println!("cargo:rustc-env=FN64_NATIVE_FUNCS_RS={}", funcs.display());
        println!("cargo:rerun-if-changed={}", funcs.display());
        return;
    }

    let recompiled_dir = required_env(
        "RECOMPILED_DIR",
        "Point it at a directory containing OoT (OOTU)'s N64Recomp-generated \
         RecompiledFuncs/*.c + recomp_overlays.inl (e.g. aki-recomp/games/OOTU/RecompiledFuncs).",
    );
    let recomp_h_dir = required_env(
        "RECOMP_H_DIR",
        "Point it at the directory containing N64Recomp's MIT-licensed recomp.h + \
         librecomp/sections.h (e.g. aki-recomp/refs/N64RecompSource/include).",
    );
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

    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let bridge_dir = manifest_dir.join("bridge");

    let mut build = cc::Build::new();
    build
        // bridge/include comes FIRST: it holds this harness's clean-room
        // stand-in librecomp/sections.h, which must shadow any real
        // (GPL-3.0-licensed) librecomp/include on the search path -- see
        // that header's doc comment and bridge/section_bridge.c's.
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
    println!(
        "cargo:warning=oot-boot: compiling {c_file_count} RecompiledFuncs/*.c files from {}",
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

    println!("cargo:rerun-if-changed=bridge/section_bridge.c");
    println!("cargo:rerun-if-changed={}", recompiled_dir.display());
}
