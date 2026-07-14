//! Build script: compiles WM2000's out-of-tree, game-derived
//! `RecompiledFuncs/*.c` (N64Recomp's own MIT-licensed generated output)
//! plus this example's hand-written `bridge/section_bridge.c` glue into a
//! static lib the harness links against.
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
//!   `funcs.h` for WM2000 (NWXE). In the reference dev environment, this is
//!   `aki-recomp/games/NWXE/RecompiledFuncs`.
//! - `RECOMP_H_DIR` -- path to the directory containing N64Recomp's own
//!   MIT-licensed `recomp.h`/`librecomp/sections.h` headers (the ABI this
//!   crate serves, per `docs/DESIGN.md` section 1's provenance table). In
//!   the reference dev environment, this is
//!   `aki-recomp/refs/WCWnWoRevengeRecomp/lib/N64ModernRuntime/N64Recomp/include`.
//! - `ROM` -- path to the user's own legally-obtained WM2000 ROM file. Never
//!   read by this build script (only by the runtime binary, at startup) --
//!   declared here only so a missing ROM is reported early with a clear
//!   message rather than a confusing runtime panic much later.
//!
//! No default paths are baked in (they would either point outside this
//! machine or, worse, tempt a future edit to hardcode a path into a
//! colleague's home directory) -- missing/wrong env vars fail the build
//! loudly with instructions, per `AGENTS.md`'s "loud traps, no silent
//! shrugs."

use std::env;
use std::path::PathBuf;

fn required_env(name: &str, hint: &str) -> PathBuf {
    match env::var(name) {
        Ok(v) => PathBuf::from(v),
        Err(_) => {
            panic!(
                "wm2000-boot build.rs: required environment variable {name} is not set.\n\
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

    let recompiled_dir = required_env(
        "RECOMPILED_DIR",
        "Point it at a directory containing WM2000 (NWXE)'s N64Recomp-generated \
         RecompiledFuncs/*.c + recomp_overlays.inl (e.g. aki-recomp/games/NWXE/RecompiledFuncs).",
    );
    let recomp_h_dir = required_env(
        "RECOMP_H_DIR",
        "Point it at the directory containing N64Recomp's MIT-licensed recomp.h + \
         librecomp/sections.h (e.g. \
         aki-recomp/refs/WCWnWoRevengeRecomp/lib/N64ModernRuntime/N64Recomp/include).",
    );
    // ROM is validated but not read here -- see module doc.
    let _ = required_env(
        "ROM",
        "Point it at your own legally-obtained WM2000 (NWXE) ROM file. Not read by this build \
         script (only by the compiled binary at startup) -- checked here so a missing ROM fails \
         fast with a clear message.",
    );

    if !recompiled_dir.join("recomp_overlays.inl").exists() {
        panic!(
            "wm2000-boot build.rs: RECOMPILED_DIR={} does not contain recomp_overlays.inl -- \
             expected the directory N64Recomp emitted WM2000's RecompiledFuncs/*.c into.",
            recompiled_dir.display()
        );
    }
    if !recomp_h_dir.join("recomp.h").exists() {
        panic!(
            "wm2000-boot build.rs: RECOMP_H_DIR={} does not contain recomp.h -- expected \
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
            "wm2000-boot build.rs: failed to read RECOMPILED_DIR={}: {e}",
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
        "wm2000-boot build.rs: found zero .c files in RECOMPILED_DIR={} -- expected N64Recomp's \
         generated RecompiledFuncs/*.c output.",
        recompiled_dir.display()
    );
    println!(
        "cargo:warning=wm2000-boot: compiling {c_file_count} RecompiledFuncs/*.c files from {}",
        recompiled_dir.display()
    );

    build.compile("wm2000_recompiled");

    // The bridge glue (section_bridge.c) is built SEPARATELY, as C++ --
    // recomp_overlays.inl's SectionTableEntry initializers use `nullptr`
    // (the file was generated for a C++ port build, per N64Recomp's own
    // codegen target), which is not valid in C. RecompiledFuncs/*.c itself
    // compiles fine as plain C (per M1-WORKLIST.md's own method), so only
    // this one glue file needs the C++ compiler.
    let mut bridge_build = cc::Build::new();
    bridge_build
        .cpp(true)
        .include(bridge_dir.join("include"))
        .include(&recompiled_dir)
        .include(&recomp_h_dir)
        .flag_if_supported("-Wno-everything")
        .warnings(false)
        .file(bridge_dir.join("section_bridge.c"));
    bridge_build.compile("wm2000_bridge");

    println!("cargo:rerun-if-changed=bridge/section_bridge.c");
    println!("cargo:rerun-if-changed={}", recompiled_dir.display());
}
