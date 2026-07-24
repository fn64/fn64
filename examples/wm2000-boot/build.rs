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
use std::fs;
use std::path::PathBuf;

#[path = "../../crates/fn64-boot-harness/build_support.rs"]
mod build_support;

#[path = "../../crates/fn64-boot-harness/native_program_identity.rs"]
mod native_program_identity;

/// Route the game's own linked libultra `osGetTime` (`func_80032570` --
/// verbatim: `__osDisableInt`; `osGetCount`; 64-bit `base pair (0x800974E8/
/// 0x800974EC) + (Count - lastCount @0x80088410)`; `__osRestoreInt`; return
/// v0:v1 = hi:lo) to fn64's `osGetTime_recomp` virtual-clock shim, exactly as
/// the corpus syms already route its sibling `osGetCount` (0x80037690 ->
/// `osGetCount_recomp`). The corpus simply failed to identify this one
/// libultra symbol.
///
/// Why this is load-bearing and faithful (2026-07 title-reveal rung): on
/// hardware the OS counter interrupt (`__osTimeServices`) refreshes the
/// osGetTime base pair every CP0 Count wrap; fn64 never runs a guest counter
/// interrupt, so the unpatched guest implementation SAWTOOTHS -- time jumps
/// back 91.6s (2^32 Count units) every 91.6s of virtual time. WM2000's
/// per-frame sound tick (funcs_16, asm 0x800E2110..0x800E2200) converts
/// osGetTime deltas to usec (`*64/0xBB8`) and then to a 60Hz frame count
/// (`(delta+0x208D)/0x411A`, 64-bit unsigned): at each wrap the negative
/// delta becomes a rate of ~0x5899xxxx (~1.49e9; verified numerically:
/// `(2^64 - 91,625,968usec)/0x411A mod 2^32 = 0x589985B2` + observed real
/// elapsed frames), the attract clock at 0x8009749C leaps ~25M units past
/// every attract-script end time (probed live: clock 1544 ->
/// 1,486,459,755 in one frame while script ends sit at 780..2440), and the
/// attract fade scheduler (funcs_22 `func_8011DFCC`, 0x8011E0D4..0x8011E188)
/// computes its master fade level as `(end - clock) << 8 / duration` --
/// astronomically negative forever, which is exactly the observed
/// never-lifting opaque-black + sawing-white full-screen covers that hid
/// every attract scene (title screen included) in the presented frames.
///
/// The shared build_support preparer (Codex's reconciled fall-through mend +
/// entry instrumentation) has already written each generated source into
/// OUT_DIR as C++; this transform runs per prepared file alongside that
/// pipeline, replacing the identified body wholesale (dropping only its
/// self-referential entry observer, which is moot once the body is a pure
/// tail-call into the Rust shim).
fn patch_osgettime(source: &str) -> (String, usize) {
    const HEADER: &str = "RECOMP_FUNC void func_80032570(uint8_t* rdram, recomp_context* ctx) {";
    let Some(start) = source.find(HEADER) else {
        return (source.to_string(), 0);
    };
    let close_rel = source[start..]
        .find("\n;}")
        .expect("wm2000-boot build.rs: func_80032570 body has no `;}` close");
    let end = start + close_rel + 3;
    let mut out = String::with_capacity(source.len());
    out.push_str(&source[..start]);
    out.push_str(
        "// fn64 libultra-identification patch (wm2000-boot build.rs): this function\n\
         // is the game's linked libultra osGetTime, which the corpus syms failed to\n\
         // name (its sibling osGetCount IS routed to osGetCount_recomp). The guest\n\
         // implementation depends on the OS counter-interrupt time service that fn64\n\
         // does not run, so it sawtooths at every CP0 Count wrap and poisons the\n\
         // attract-mode clock -- see patch_osgettime's doc comment in build.rs.\n\
         extern \"C\" void osGetTime_recomp(uint8_t* rdram, recomp_context* ctx);\n\
         RECOMP_FUNC void func_80032570(uint8_t* rdram, recomp_context* ctx) {\n    \
         osGetTime_recomp(rdram, ctx);\n    return;\n;}",
    );
    out.push_str(&source[end..]);
    (out, 1)
}

/// Apply `patch_osgettime` to the prepared C++ sources the shared preparer
/// wrote into OUT_DIR, rewriting the matched file in place. Asserts the
/// libultra osGetTime body was matched exactly once (its unique definition is
/// funcs_14.c) -- a silent miss re-opens the attract-clock poison documented
/// on `patch_osgettime`.
fn route_osgettime_in_prepared_sources(cxx_sources: &[PathBuf]) {
    let mut osgettime_total = 0usize;
    for path in cxx_sources {
        let source = std::fs::read_to_string(path)
            .unwrap_or_else(|e| panic!("wm2000-boot build.rs: reading {}: {e}", path.display()));
        let (patched, hits) = patch_osgettime(&source);
        if hits > 0 {
            std::fs::write(path, patched).unwrap_or_else(|e| {
                panic!("wm2000-boot build.rs: writing {}: {e}", path.display())
            });
        }
        osgettime_total += hits;
    }
    assert_eq!(
        osgettime_total, 1,
        "wm2000-boot build.rs: the osGetTime identification patch must match exactly one \
         function body (func_80032570 in funcs_14.c) -- {osgettime_total} matched. Either the \
         corpus was regenerated with osGetTime properly named (delete patch_osgettime) or the \
         generated shape changed (fix it). A silent miss re-opens the attract-clock poison \
         (see patch_osgettime's doc comment)."
    );
}

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

fn produced_archive_bytes(out_dir: &std::path::Path, stem: &str) -> Vec<u8> {
    let candidates = [
        out_dir.join(format!("lib{stem}.a")),
        out_dir.join(format!("{stem}.lib")),
        out_dir.join(format!("lib{stem}.lib")),
    ];
    let existing: Vec<_> = candidates.iter().filter(|path| path.is_file()).collect();
    assert_eq!(
        existing.len(),
        1,
        "wm2000-boot build.rs: expected exactly one produced archive for {stem} in {}, found {existing:?}",
        out_dir.display()
    );
    fs::read(existing[0]).unwrap_or_else(|error| {
        panic!(
            "wm2000-boot build.rs: read produced native archive {}: {error}",
            existing[0].display()
        )
    })
}

fn lowercase_hex(bytes: [u8; 32]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(64);
    for byte in bytes {
        output.push(DIGITS[(byte >> 4) as usize] as char);
        output.push(DIGITS[(byte & 0x0f) as usize] as char);
    }
    output
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

    let out_dir = PathBuf::from(env::var("OUT_DIR").expect("Cargo must provide OUT_DIR"));
    // WM2000's admitted corpus is known to split hardware fall-through at
    // internal labels. Keep the repair explicit here; every other generated-C
    // consumer uses the default preparation path without this transform.
    let (cxx_sources, jump_snapshot_count, prototype_count, fallthrough_count) =
        build_support::prepare_recompiled_cxx_sources_with_fallthrough_repair(
            &recompiled_dir,
            &out_dir,
        );
    let c_file_count = cxx_sources.len();
    // Graft the ladder-3 libultra osGetTime routing onto Codex's reconciled
    // preparer output: rewrite the one prepared source whose body is the
    // game's unidentified osGetTime so the attract clock reads fn64's virtual
    // clock instead of sawtoothing (see patch_osgettime).
    route_osgettime_in_prepared_sources(&cxx_sources);
    build.files(cxx_sources);
    println!(
        "cargo:warning=wm2000-boot: compiling {c_file_count} RecompiledFuncs/*.c files from {} \
         ({jump_snapshot_count} C jump snapshots normalized, {prototype_count} missing C \
         prototypes supplied for C++, and {fallthrough_count} address-proven fragments mended)",
        recompiled_dir.display()
    );
    assert!(
        fallthrough_count > 0,
        "wm2000-boot: the admitted corpus needed no fall-through repairs; either its partition is fixed (remove the opt-in) or the generated shape changed"
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

    // Release evidence binds the exact machine-code archives Cargo links.
    // Logical labels and archive bytes are the complete identity wire; host
    // paths and pointers never enter it.
    let program_identity =
        lowercase_hex(native_program_identity::native_program_archives_sha256([
            (
                "generated-code".to_owned(),
                produced_archive_bytes(&out_dir, "wm2000_recompiled"),
            ),
            (
                "section-bridge".to_owned(),
                produced_archive_bytes(&out_dir, "wm2000_bridge"),
            ),
        ]));
    println!("cargo:rustc-env=FN64_NATIVE_PROGRAM_ARTIFACT_SHA256={program_identity}");

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
