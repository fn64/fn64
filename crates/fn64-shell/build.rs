//! fn64-shell build script: OPTIONALLY links a game.
//!
//! fn64-shell is a member of the main fn64 workspace, so `cargo build
//! --workspace` (with ZERO game content, per `fn64/README.md`'s "no game
//! content ships in this repo" rule) MUST succeed. Therefore this script is
//! a **no-op unless `RECOMPILED_DIR` is set**: with no game env, the shell
//! compiles to a runnable binary that prints intake instructions and exits.
//!
//! When `RECOMPILED_DIR`/`RECOMP_H_DIR`/`ROM` ARE set (the exact same
//! contract as `examples/oot-boot/build.rs` -- see that file's module doc
//! for the byte-cited rationale on each), this compiles the out-of-tree,
//! game-derived `RecompiledFuncs/*.c` + `fn64-boot-harness`'s shared
//! `bridge/section_bridge.c` glue into a static lib, exactly as oot-boot does, and
//! emits `cargo:rustc-cfg=fn64_game_linked` so the shared harness FFI and its
//! live boot/window loop are compiled in. Without that cfg, the binary has no
//! game symbols to link against and falls back to the intake-instructions
//! path -- no unresolved-symbol link error, no game content required.
//!
//! ## Required environment variables (only when linking a game)
//!
//! - `RECOMPILED_DIR` -- dir with the N64Recomp-generated `RecompiledFuncs/
//!   *.c`, `recomp_overlays.inl`, `recomp.h`, `funcs.h` (e.g.
//!   `aki-recomp/games/OOTU/RecompiledFuncs`).
//! - `RECOMP_H_DIR` -- dir with N64Recomp's MIT-licensed `recomp.h` +
//!   `librecomp/sections.h` (e.g. `aki-recomp/refs/N64RecompSource/include`).
//! - `ROM` -- the decomp's OWN decompressed BUILD-OUTPUT z64 (NOT the retail
//!   compressed cartridge -- see oot-boot/build.rs). Read only by the binary
//!   at startup, validated here so a missing ROM fails the build early.

use std::env;
use std::path::PathBuf;

fn main() {
    println!("cargo:rerun-if-env-changed=RECOMPILED_DIR");
    println!("cargo:rerun-if-env-changed=RECOMP_H_DIR");
    println!("cargo:rerun-if-env-changed=ROM");
    println!("cargo:rerun-if-env-changed=FN64_RECOMP");
    // Declared so a clean `#[cfg(fn64_game_linked)]` doesn't warn as an
    // unexpected cfg under `-Wunexpected_cfgs` (Rust 1.80+ lint).
    println!("cargo:rustc-check-cfg=cfg(fn64_game_linked)");
    println!("cargo:rustc-check-cfg=cfg(fn64_recomp_rs)");

    // FN64_RECOMP=rs (same contract as oot-boot/build.rs): the game comes
    // from the linked `oot-recompiled` typed-Rust crate (the rs manifest at
    // crates/fn64-shell/rs/), so there is no C to compile -- ROM is still
    // required at runtime. Only the rs manifest carries the rs deps, so
    // this branch is unreachable from the plain workspace build.
    if env::var("FN64_RECOMP").as_deref() == Ok("rs") {
        let _ = required_env(
            "ROM",
            "Point it at the decomp's OWN decompressed build-output z64 -- NOT the retail \
             compressed cartridge image.",
        );
        println!("cargo:rustc-cfg=fn64_recomp_rs");
        println!("cargo:rustc-cfg=fn64_game_linked");
        return;
    }

    let Some(recompiled_dir) = env::var_os("RECOMPILED_DIR").map(PathBuf::from) else {
        // No game env: the content-free path. The shell still builds and
        // runs (prints intake instructions). This is the state every
        // `cargo build --workspace` in a content-free checkout sees.
        println!(
            "cargo:warning=fn64-shell: RECOMPILED_DIR unset -- building the shell WITHOUT a linked \
             game (it will print intake instructions and exit). Set RECOMPILED_DIR/RECOMP_H_DIR/ROM \
             to link a game and get a live window."
        );
        return;
    };

    let recomp_h_dir = required_env(
        "RECOMP_H_DIR",
        "Point it at the directory containing N64Recomp's MIT-licensed recomp.h + \
         librecomp/sections.h (e.g. aki-recomp/refs/N64RecompSource/include).",
    );
    // ROM validated (not read) here so a missing ROM fails fast, matching
    // oot-boot/build.rs.
    let _ = required_env(
        "ROM",
        "Point it at the decomp's OWN decompressed build-output z64 (e.g. \
         aki-recomp/refs/oot-decomp/build/ntsc-1.0/oot-ntsc-1.0.z64) -- NOT the retail compressed \
         cartridge image.",
    );

    if !recompiled_dir.join("recomp_overlays.inl").exists() {
        panic!(
            "fn64-shell build.rs: RECOMPILED_DIR={} does not contain recomp_overlays.inl -- \
             expected the directory N64Recomp emitted the game's RecompiledFuncs/*.c into.",
            recompiled_dir.display()
        );
    }
    if !recomp_h_dir.join("recomp.h").exists() {
        panic!(
            "fn64-shell build.rs: RECOMP_H_DIR={} does not contain recomp.h -- expected \
             N64Recomp's include/ directory.",
            recomp_h_dir.display()
        );
    }

    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let bridge_dir = manifest_dir.join("../fn64-boot-harness/bridge");

    // RecompiledFuncs/*.c: plain C, generated, no warning hygiene.
    let mut build = cc::Build::new();
    build
        // The shared bridge include comes FIRST: its clean-room sections.h
        // must shadow any real (GPL-3.0) librecomp header on the path.
        .include(bridge_dir.join("include"))
        .include(&recompiled_dir)
        .include(&recomp_h_dir)
        .flag_if_supported("-Wno-everything")
        .warnings(false);

    let mut c_file_count = 0usize;
    for entry in std::fs::read_dir(&recompiled_dir).unwrap_or_else(|e| {
        panic!(
            "fn64-shell build.rs: failed to read RECOMPILED_DIR={}: {e}",
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
        "fn64-shell build.rs: found zero .c files in RECOMPILED_DIR={} -- expected N64Recomp's \
         generated RecompiledFuncs/*.c output.",
        recompiled_dir.display()
    );
    println!(
        "cargo:warning=fn64-shell: compiling {c_file_count} RecompiledFuncs/*.c files from {}",
        recompiled_dir.display()
    );
    build.compile("fn64_shell_recompiled");

    // The bridge glue is C++ (recomp_overlays.inl's initializers use
    // `nullptr`), built separately -- same split as oot-boot/build.rs.
    let mut bridge_build = cc::Build::new();
    bridge_build
        .cpp(true)
        .include(bridge_dir.join("include"))
        .include(&recompiled_dir)
        .include(&recomp_h_dir)
        .flag_if_supported("-Wno-everything")
        .warnings(false)
        .file(bridge_dir.join("section_bridge.c"));
    bridge_build.compile("fn64_shell_bridge");

    // The game symbols are now linkable: turn on the boot/window path.
    println!("cargo:rustc-cfg=fn64_game_linked");
    println!(
        "cargo:rerun-if-changed={}",
        bridge_dir.join("section_bridge.c").display()
    );
    println!("cargo:rerun-if-changed={}", recompiled_dir.display());
}

fn required_env(name: &str, hint: &str) -> PathBuf {
    match env::var(name) {
        Ok(v) => PathBuf::from(v),
        Err(_) => panic!(
            "fn64-shell build.rs: RECOMPILED_DIR was set (linking a game) but required \
             environment variable {name} is not.\n{hint}"
        ),
    }
}
