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
//! - `RECOMP_H_DIR` -- OPTIONAL. N64Recomp's MIT `recomp.h` is vendored at
//!   `fn64-boot-harness/bridge/include/vendor/`; set this only to build
//!   against a different fork. (`librecomp/sections.h` is fn64's own
//!   clean-room header, not N64Recomp's -- it never came from there.)
//! - `ROM` -- the decomp's OWN decompressed BUILD-OUTPUT z64 (NOT the retail
//!   compressed cartridge -- see oot-boot/build.rs). Read only by the binary
//!   at startup, validated here so a missing ROM fails the build early.

use std::env;
use std::path::PathBuf;

#[path = "../fn64-boot-harness/build_support.rs"]
mod build_support;

fn main() {
    println!("cargo:rerun-if-env-changed=RECOMPILED_DIR");
    println!("cargo:rerun-if-env-changed=RECOMP_H_DIR");
    println!("cargo:rerun-if-env-changed=ROM");
    println!("cargo:rerun-if-env-changed=FN64_RECOMP");
    // Declared so a clean `#[cfg(fn64_game_linked)]` doesn't warn as an
    // unexpected cfg under `-Wunexpected_cfgs` (Rust 1.80+ lint).
    println!("cargo:rustc-check-cfg=cfg(fn64_game_linked)");
    println!("cargo:rustc-check-cfg=cfg(fn64_cpu_runtime)");

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
        println!("cargo:rustc-cfg=fn64_cpu_runtime");
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

    // recomp.h is MIT (N64Recomp, (c) 2024 Wiseguy) and contains no game
    // content, so it is vendored in-tree -- see ROADMAP H1. RECOMP_H_DIR still
    // overrides, for building against a different fork.
    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let bridge_dir = manifest_dir.join("../fn64-boot-harness/bridge");
    let recomp_h_dir = env::var("RECOMP_H_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| bridge_dir.join("include/vendor"));
    // ROM validated (not read) here so a missing ROM fails fast, matching
    // oot-boot/build.rs.
    let _ = required_env(
        "ROM",
        "Point it at the decomp's OWN decompressed build-output z64 -- NOT the retail \
         compressed cartridge image.",
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
            "fn64-shell build.rs: no recomp.h at {} -- the vendored copy ships in this repo \
             (MIT, see LICENSE-N64Recomp beside it); set RECOMP_H_DIR only to override it.",
            recomp_h_dir.display()
        );
    }

    // Generated sources use C++ only for fn64's MEM_W lvalue proxy; the
    // emitted program remains N64Recomp's C ABI and has no warning hygiene.
    let mut build = cc::Build::new();
    build
        .cpp(true)
        // The shared bridge include comes FIRST: its clean-room sections.h
        // must shadow any real (GPL-3.0) librecomp header on the path.
        .include(bridge_dir.join("include"))
        .include(&recompiled_dir)
        .include(&recomp_h_dir)
        .flag("-include")
        .flag("fn64_mmio_proxy.h")
        .flag_if_supported("-std=c++17")
        .flag_if_supported("-Wno-everything")
        .warnings(false);

    let out_dir = PathBuf::from(env::var("OUT_DIR").expect("Cargo must provide OUT_DIR"));
    // A generic shell cannot carry a title-name opt-in, but it also cannot
    // skip a corpus's proven answer-key-split epilogue. The shared preparer
    // enables its section-local mend only when the generated instruction
    // shapes prove that split; ordinary adjacent functions remain untouched.
    let (cxx_sources, jump_snapshot_count, prototype_count, fallthrough_count) =
        build_support::prepare_recompiled_cxx_sources_with_proven_fallthrough_repair(
            &recompiled_dir,
            &out_dir,
        );
    let c_file_count = cxx_sources.len();
    build.files(cxx_sources);
    println!(
        "cargo:warning=fn64-shell: compiling {c_file_count} RecompiledFuncs/*.c files from {} \
         ({jump_snapshot_count} C jump snapshots normalized, {prototype_count} missing C \
         prototypes supplied for C++, and {fallthrough_count} structurally admitted \
         fall-through fragments mended)",
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
    println!(
        "cargo:rerun-if-changed={}",
        bridge_dir.join("include/fn64_mmio_proxy.h").display()
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
