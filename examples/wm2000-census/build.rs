//! Build script: compiles WM2000's out-of-tree, game-derived
//! `RecompiledFuncs/*.c` (N64Recomp's own MIT-licensed generated output)
//! plus `fn64-boot-harness`'s shared `bridge/section_bridge.c` glue into a
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

// The shared generated-C preparer, used verbatim by `fn64-shell/build.rs`.
// Sharing it is the point: this harness previously carried a hand-rolled
// subset that normalized only N64Recomp's `jr_addend_XXXX` declarations, and
// so silently omitted the address-proven fall-through mend. See this file's
// "Why the shared preparer" note in `main`.
#[path = "../../crates/fn64-boot-harness/build_support.rs"]
mod build_support;

fn required_env(name: &str, hint: &str) -> PathBuf {
    match env::var(name) {
        Ok(v) => PathBuf::from(v),
        Err(_) => {
            panic!(
                "wm2000-census build.rs: required environment variable {name} is not set.\n\
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
            "wm2000-census build.rs: RECOMPILED_DIR={} does not contain recomp_overlays.inl -- \
             expected the directory N64Recomp emitted WM2000's RecompiledFuncs/*.c into.",
            recompiled_dir.display()
        );
    }
    if !recomp_h_dir.join("recomp.h").exists() {
        panic!(
            "wm2000-census build.rs: RECOMP_H_DIR={} does not contain recomp.h -- expected \
             N64Recomp's include/ directory.",
            recomp_h_dir.display()
        );
    }

    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let bridge_dir = manifest_dir.join("../../crates/fn64-boot-harness/bridge");

    let mut build = cc::Build::new();
    build
        .cpp(true)
        // The shared bridge include comes FIRST: it holds fn64's clean-room
        // stand-in librecomp/sections.h, which must shadow any real
        // (GPL-3.0-licensed) librecomp/include on the search path -- see
        // that header's doc comment and section_bridge.c's.
        .include(bridge_dir.join("include"))
        .include(&recompiled_dir)
        .include(&recomp_h_dir)
        .flag("-include")
        .flag("fn64_mmio_proxy.h")
        .flag_if_supported("-std=c++17")
        .flag_if_supported("-Wno-everything")
        // Generated code carries aki-recomp's NAN_CHECK debug asserts;
        // real hardware propagates NaN silently (0.0/0.0 in genuinely
        // uninitialized-BSS math is normal mid-boot). NDEBUG disables the
        // asserts, matching a release build of the same sources.
        .define("NDEBUG", None)
        // RecompiledFuncs/*.c is generated code with no warning hygiene of
        // its own (matches aki-recomp's own CMakeLists.txt build recipe for
        // this same source, per M1-WORKLIST.md's method section).
        .warnings(false);

    // ## Why the shared preparer
    //
    // This used to be a hand-rolled loop that rewrote only N64Recomp's
    // `gpr jr_addend_XXXX = <expr>;` mid-function declarations (valid C11, a
    // hard error in C++, which this build needs for fn64_mmio_proxy.h). That
    // subset compiled, but it omitted the *address-proven fall-through mend*
    // that `fn64-shell/build.rs` already applies to the same generated shape,
    // and the omission was load-bearing rather than cosmetic.
    //
    // MEASURED on this corpus (NWXE `RecompiledFuncs/*.c`, 2,387 generated
    // functions): 13 of them allocate a stack frame with
    // `addiu $sp, $sp, -0xN` and never emit the matching `addiu $sp, $sp, 0xN`
    // epilogue, because IDO emitted a *shared* epilogue that N64Recomp split
    // into the address-contiguous next function. `func_8011F67C` (bank2_text,
    // `size:0x7FC` per the corpus's own `disasm/symbol_addrs.txt`) ends at
    // 0x8011FE74 in a `jal` delay slot and falls through into
    // `func_8011FE78`, which restores `$ra`/`$fp`/`$s7..$s0` from
    // 0x84..0x60($sp) and does `addiu $sp, $sp, 0x88` -- exactly matching
    // `func_8011F67C`'s own `-0x88` prologue and save slots.
    //
    // Without the mend, `func_8011F67C` returns with `$sp` 0x88 low and every
    // callee-saved register clobbered. Its caller `func_801200DC` then reloads
    // `$s1` from `0x2C($sp)` -- now pointing into the wrong frame -- and hands
    // the garbage up to `func_80121764`, whose `lw $v0, 0xDC($s1)` at guest PC
    // 0x80121A3C computes 0xF0 + 0xDC = 0x1CC and traps. That is the abort the
    // census docs recorded as an "unmodelled 0x1CC MMIO read": 0x1CC is not a
    // device offset at all, it is a KUSEG near-null address reached through a
    // corrupted frame pointer.
    //
    // The mend is section-local and structurally gated
    // (`prepare_recompiled_cxx_sources_with_proven_fallthrough_repair`): it
    // fires only where the generated section table proves an address-contiguous
    // successor AND that successor has the split-epilogue instruction shape.
    // Ordinary adjacent functions are left untouched, so this cannot invent a
    // call between two unrelated bodies.
    let out_dir = PathBuf::from(env::var("OUT_DIR").expect("Cargo must provide OUT_DIR"));
    let (cxx_sources, jump_snapshot_count, prototype_count, fallthrough_count) =
        build_support::prepare_recompiled_cxx_sources_with_proven_fallthrough_repair(
            &recompiled_dir,
            &out_dir,
        );
    let c_file_count = cxx_sources.len();
    assert!(
        c_file_count > 0,
        "wm2000-census build.rs: found zero .c files in RECOMPILED_DIR={} -- expected N64Recomp's \
         generated RecompiledFuncs/*.c output.",
        recompiled_dir.display()
    );
    build.files(cxx_sources);
    println!(
        "cargo:warning=wm2000-census: compiling {c_file_count} RecompiledFuncs/*.c files from {} \
         ({jump_snapshot_count} C jump snapshots normalized, {prototype_count} missing C \
         prototypes supplied for C++, and {fallthrough_count} structurally admitted \
         fall-through fragments mended)",
        recompiled_dir.display()
    );

    build.compile("wm2000_recompiled");

    // The bridge glue (section_bridge.c) is built SEPARATELY, as C++ --
    // recomp_overlays.inl's SectionTableEntry initializers use `nullptr`
    // (the file was generated for a C++ port build, per N64Recomp's own
    // codegen target), which is not valid in C. RecompiledFuncs/*.c itself
    // also uses C++ now so fn64_mmio_proxy.h can preserve MEM_W lvalue syntax
    // while intercepting raw RCP register words.
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
