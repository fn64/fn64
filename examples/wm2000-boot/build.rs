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

use std::collections::HashMap;
use std::env;
use std::path::PathBuf;

/// Parse `recomp_overlays.inl`'s per-section `FuncEntry` arrays into a
/// fall-through successor map: function A -> function B iff A and B are
/// consecutive entries of the SAME section and A's `offset + rom_size`
/// equals B's `offset` (i.e. B starts at the very next ROM byte).
///
/// Why this exists (2026-07-21 DL-cursor rung, README "frontier"): the
/// answer-key partition behind this corpus split several real functions at
/// internal labels, so N64Recomp emitted each piece as a SEPARATE C
/// function with no fall-through tail call. On hardware, execution simply
/// continues at the next address; in the generated C, control falls off
/// the end of the fragment and silently skips the real function's
/// continuation. Watchpoint-proven instance: `func_8011F67C` (announcer
/// sprite DL emitter) ends at 0x8011FE74 and falls through into
/// `func_8011FE78` -- the register-restore + `sp += 0x88` epilogue --
/// which the generated C never ran, shredding the caller chain's saved
/// registers and stack pointer (the demo-frame "DL cursor into the
/// stack" corruption).
fn parse_fallthrough_successors(inl_source: &str) -> HashMap<String, String> {
    let mut successors = HashMap::new();
    let mut prev: Option<(String, u64, u64)> = None;
    for line in inl_source.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("static FuncEntry ") {
            // New section array: fragments never fall through across
            // sections (different banks live at different ROM regions).
            prev = None;
            continue;
        }
        // Entry shape (N64Recomp's own emission):
        //   { .func = func_8011FE78, .offset = 0x00003578, .rom_size = 0x0000003C },
        let Some(rest) = trimmed.strip_prefix("{ .func = ") else {
            continue;
        };
        let parse = |rest: &str, key: &str| -> Option<u64> {
            let v = rest.split(key).nth(1)?;
            let v = v.trim_start().strip_prefix("0x")?;
            let end = v.find(|c: char| !c.is_ascii_hexdigit()).unwrap_or(v.len());
            u64::from_str_radix(&v[..end], 16).ok()
        };
        let Some(name) = rest.split(',').next() else {
            continue;
        };
        let name = name.trim().to_string();
        let (Some(offset), Some(rom_size)) =
            (parse(rest, ".offset = "), parse(rest, ".rom_size = "))
        else {
            continue;
        };
        if let Some((prev_name, prev_offset, prev_size)) = prev.take() {
            if prev_offset + prev_size == offset {
                successors.insert(prev_name, name.clone());
            }
        }
        prev = Some((name, offset, rom_size));
    }
    successors
}

/// Append a fall-through tail call to every generated function body that
/// (a) has an address-contiguous successor per the section tables and
/// (b) can syntactically fall off its closing brace (its last reachable
/// statement is not `return;`, an unconditional `goto`, or the generated
/// `switch_error` abort). Returns the patched source and how many bodies
/// were mended. See `parse_fallthrough_successors` for the full story.
fn mend_fallthrough(
    source: &str,
    successors: &HashMap<String, String>,
    file_name: &str,
) -> (String, usize) {
    let mut out = String::with_capacity(source.len());
    let mut mended = 0usize;
    let mut rest = source;
    const FN_PREFIX: &str = "RECOMP_FUNC void ";
    while let Some(start) = rest.find(FN_PREFIX) {
        let after_prefix = &rest[start + FN_PREFIX.len()..];
        let name_end = after_prefix
            .find('(')
            .unwrap_or_else(|| panic!("{file_name}: malformed RECOMP_FUNC header"));
        let name = &after_prefix[..name_end];
        // Generated bodies close with a line-leading `;}`.
        let body_close_rel = rest[start..]
            .find("\n;}")
            .unwrap_or_else(|| panic!("{file_name}: {name}: no `;}}` body close found"));
        let body_end = start + body_close_rel; // index of the '\n' before ";}"
        let body = &rest[start..body_end];
        let can_fall_off = {
            let last_meaningful = body
                .lines()
                .rev()
                .map(str::trim)
                .find(|l| !l.is_empty() && !l.starts_with("//"));
            match last_meaningful {
                // Header-only body (empty fragment): trivially falls off.
                Some(l) => {
                    l != "return;"
                        && !l.starts_with("goto ")
                        && !l.starts_with("switch_error(")
                }
                None => true,
            }
        };
        out.push_str(&rest[..body_end]);
        if can_fall_off {
            if let Some(successor) = successors.get(name) {
                out.push_str(&format!(
                    "\n    // fn64 fall-through mend (wm2000-boot build.rs): this generated \
                     function\n    // ends flush against `{successor}` in the same section; on \
                     hardware\n    // execution continues at that next address. The answer-key \
                     partition split\n    // them, so tail-call the successor exactly as \
                     N64Recomp does for its own\n    // fall-through functions.\n    \
                     {successor}(rdram, ctx);"
                ));
                mended += 1;
            }
        }
        out.push_str("\n;}");
        rest = &rest[body_end + 3..];
    }
    out.push_str(rest);
    (out, mended)
}

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
fn patch_osgettime(source: &str) -> (String, usize) {
    const HEADER: &str =
        "RECOMP_FUNC void func_80032570(uint8_t* rdram, recomp_context* ctx) {";
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

    // N64Recomp's jump-table codegen declares `gpr jr_addend_XXXX = <expr>;`
    // mid-function, and other case arms `goto` past that declaration. Valid
    // C11 (the variable is simply uninitialized on bypassing paths, and the
    // generated code only reads it after the assignment), but a hard error in
    // C++ ("jump bypasses variable initialization"), which this build needs
    // for fn64_mmio_proxy.h. Splitting into declaration + assignment is
    // semantically identical and legal in both languages, so rewrite each .c
    // into OUT_DIR before compiling. ponytail: line-based string match, not a
    // C parser -- generated code is mechanically regular, so this holds until
    // N64Recomp changes its emission shape (then the C++ error returns loudly).
    let patched_dir = PathBuf::from(env::var("OUT_DIR").unwrap()).join("cxx-safe-funcs");
    std::fs::create_dir_all(&patched_dir).unwrap();
    let inl_source = std::fs::read_to_string(recompiled_dir.join("recomp_overlays.inl"))
        .unwrap_or_else(|e| panic!("wm2000-boot build.rs: reading recomp_overlays.inl: {e}"));
    let successors = parse_fallthrough_successors(&inl_source);
    assert!(
        !successors.is_empty(),
        "wm2000-boot build.rs: recomp_overlays.inl parsed to an empty fall-through successor \
         map -- the FuncEntry emission shape must have changed; fix parse_fallthrough_successors \
         rather than silently skipping the fall-through mend."
    );
    let mut mended_total = 0usize;
    let mut osgettime_total = 0usize;
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
            let source = std::fs::read_to_string(&path).unwrap_or_else(|e| {
                panic!("wm2000-boot build.rs: reading {}: {e}", path.display())
            });
            let patched: String = source
                .lines()
                .map(|line| {
                    let trimmed = line.trim_start();
                    if let Some(rest) = trimmed.strip_prefix("gpr jr_addend_") {
                        if let Some((name_tail, expr)) = rest.split_once(" = ") {
                            let indent = &line[..line.len() - trimmed.len()];
                            return format!(
                                "{indent}gpr jr_addend_{name_tail}; jr_addend_{name_tail} = {expr}\n"
                            );
                        }
                    }
                    format!("{line}\n")
                })
                .collect();
            let file_name = path.file_name().unwrap().to_string_lossy().into_owned();
            let (patched, mended) = mend_fallthrough(&patched, &successors, &file_name);
            mended_total += mended;
            let (patched, osgettime_hits) = patch_osgettime(&patched);
            osgettime_total += osgettime_hits;
            let out_path = patched_dir.join(path.file_name().unwrap());
            std::fs::write(&out_path, patched).unwrap_or_else(|e| {
                panic!("wm2000-boot build.rs: writing {}: {e}", out_path.display())
            });
            build.file(&out_path);
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
        "cargo:warning=wm2000-boot: compiling {c_file_count} RecompiledFuncs/*.c files from {} \
         ({mended_total} fall-through fragments mended with successor tail calls)",
        recompiled_dir.display()
    );
    assert_eq!(
        osgettime_total, 1,
        "wm2000-boot build.rs: the osGetTime identification patch must match exactly one \
         function body (func_80032570 in funcs_14.c) -- {osgettime_total} matched. Either the \
         corpus was regenerated with osGetTime properly named (delete patch_osgettime) or the \
         generated shape changed (fix it). A silent miss re-opens the attract-clock poison \
         (see patch_osgettime's doc comment)."
    );
    assert!(
        mended_total > 0,
        "wm2000-boot build.rs: the fall-through mend matched zero function bodies -- either the \
         corpus was regenerated with correct function boundaries (delete this assert and the \
         mend) or the generated-code shape changed (fix mend_fallthrough). Silent no-op is not \
         an option: unmended fall-through fragments corrupt the guest stack (see README \
         frontier, func_8011F67C/func_8011FE78)."
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
