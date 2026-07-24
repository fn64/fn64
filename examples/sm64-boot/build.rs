//! Build script: compiles Super Mario 64's out-of-tree, game-derived
//! `RecompiledFuncs/*.c` (N64Recomp's own MIT-licensed generated output)
//! plus `fn64-boot-harness`'s shared `bridge/section_bridge.c` glue into a
//! static lib the harness links against.
//!
//! Identical shape to `examples/oot-boot/build.rs` (that file's doc comment
//! explains the clean-room rationale in full) -- copied and only the game
//! name and archive stems changed. Like OoT's bring-up, SM64's recompile
//! (aki-recomp/games/SM64U) built directly against upstream N64Recomp's own
//! `refs/N64RecompSource/include`.
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
//!   `funcs.h` for SM64 (SM64U). In the reference dev environment, this is
//!   `aki-recomp/games/SM64U/RecompiledFuncs`.
//! - `RECOMP_H_DIR` -- N64Recomp's MIT-licensed `recomp.h`. The vendored copy
//!   at `crates/fn64-boot-harness/bridge/include/vendor/` is used unless this
//!   is set; SM64's bring-up sets it to
//!   `aki-recomp/refs/N64RecompSource/include` (recomp.h lives there, NOT in
//!   the RecompiledFuncs output dir).
//! - `ROM` -- path to the sm64-decomp's OWN BUILD-OUTPUT ROM
//!   (`build/us/sm64.us.z64`, byte-identical to retail SM64 US). Never read by
//!   this build script (only by the runtime binary, at startup) -- declared
//!   here only so a missing ROM is reported early with a clear message rather
//!   than a confusing runtime panic much later.
//!
//! No default paths are baked in -- missing/wrong env vars fail the build
//! loudly with instructions, per `AGENTS.md`'s "loud traps, no silent
//! shrugs."

use std::env;
use std::fs;
use std::path::PathBuf;

#[path = "../../crates/fn64-boot-harness/build_support.rs"]
mod build_support;

#[path = "../../crates/fn64-boot-harness/native_program_identity.rs"]
mod native_program_identity;

/// Locate `crates/fn64-boot-harness/bridge` by walking up from this manifest.
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

/// This crate's own `bridge/` directory (holds sm64_missing_stubs.c).
fn manifest_dir_bridge() -> PathBuf {
    PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap()).join("bridge")
}

/// The vendored MIT `recomp.h`, used unless RECOMP_H_DIR overrides it.
fn vendored_recomp_h_dir() -> PathBuf {
    let d = bridge_dir().join("include/vendor");
    assert!(
        d.join("recomp.h").is_file(),
        "sm64-boot build.rs: vendored recomp.h missing at {} -- it ships in this repo (MIT, \
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
                "sm64-boot build.rs: required environment variable {name} is not set.\n\
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
        "sm64-boot build.rs: expected exactly one produced archive for {stem} in {}, found {existing:?}",
        out_dir.display()
    );
    fs::read(existing[0]).unwrap_or_else(|error| {
        panic!(
            "sm64-boot build.rs: read produced native archive {}: {error}",
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

/// Parse `recomp_overlays.inl`'s section_table into (index, rom, ram, size).
fn parse_section_geometry(recompiled_dir: &std::path::Path) -> Vec<(u32, u32, u32, u32)> {
    let text = fs::read_to_string(recompiled_dir.join("recomp_overlays.inl"))
        .expect("sm64-boot build.rs: read recomp_overlays.inl for section geometry");
    let mut out = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if !line.starts_with(".rom_addr") && !line.contains(".rom_addr =") {
            continue;
        }
        let get = |key: &str| -> Option<u32> {
            let start = line.find(key)? + key.len();
            let rest = &line[start..];
            let hex = rest
                .trim_start()
                .trim_start_matches("0x")
                .trim_start_matches("0X");
            let end = hex
                .find(|c: char| !c.is_ascii_hexdigit())
                .unwrap_or(hex.len());
            u32::from_str_radix(&hex[..end], 16).ok()
        };
        // .index = is decimal, handle separately.
        let idx_dec = line.find(".index =").map(|p| {
            let rest = &line[p + ".index =".len()..];
            let rest = rest.trim_start();
            let end = rest.find(|c: char| !c.is_ascii_digit()).unwrap_or(rest.len());
            rest[..end].parse::<u32>().unwrap()
        });
        if let (Some(rom), Some(ram), Some(size), Some(idx)) = (
            get(".rom_addr ="),
            get(".ram_addr ="),
            get(".size ="),
            idx_dec,
        ) {
            out.push((idx, rom, ram, size));
        }
    }
    out
}

/// Scan the generated `.c` files for `RECOMP_FUNC void static_<sec>_<vram>(`
/// definitions -- functions the corpus calls directly but leaves out of the
/// section_table -- and emit a C registrar that hands each to a Rust callback
/// with its owning section's geometry. Returns the path of the emitted C file.
fn emit_static_registration(recompiled_dir: &std::path::Path, out_dir: &std::path::Path) -> PathBuf {
    let sections = parse_section_geometry(recompiled_dir);
    let section_by_index: std::collections::HashMap<u32, (u32, u32, u32)> = sections
        .iter()
        .map(|&(idx, rom, ram, size)| (idx, (rom, ram, size)))
        .collect();

    // Collect distinct static_<sec>_<vram> names across every funcs_*.c.
    let mut statics: std::collections::BTreeSet<(u32, u32, String)> =
        std::collections::BTreeSet::new();
    for entry in fs::read_dir(recompiled_dir).expect("read RECOMPILED_DIR") {
        let path = entry.expect("dir entry").path();
        if path.extension().and_then(|e| e.to_str()) != Some("c") {
            continue;
        }
        let text = fs::read_to_string(&path).expect("read funcs .c");
        for line in text.lines() {
            let needle = "RECOMP_FUNC void static_";
            let Some(pos) = line.find(needle) else {
                continue;
            };
            let after = &line[pos + "RECOMP_FUNC void ".len()..];
            let end = after.find('(').unwrap_or(after.len());
            let name = &after[..end];
            // name = static_<sec>_<vram>
            let body = name.strip_prefix("static_").unwrap_or(name);
            let mut parts = body.splitn(2, '_');
            let (Some(sec), Some(vram)) = (parts.next(), parts.next()) else {
                continue;
            };
            if let (Ok(sec), Ok(vram)) = (
                sec.parse::<u32>(),
                u32::from_str_radix(vram, 16),
            ) {
                statics.insert((sec, vram, name.to_owned()));
            }
        }
    }

    let mut c = String::new();
    c.push_str(
        "// GENERATED by sm64-boot/build.rs from RECOMPILED_DIR at build time.\n\
         // Registers SM64's `static_<sec>_<vram>` functions -- called directly\n\
         // by recompiled code but absent from recomp_overlays.inl's section_table.\n\
         // Contains only linkable symbol names + geometry derived from the\n\
         // out-of-tree corpus; ZERO game content ships in the fn64 repo.\n\
         #include \"recomp.h\"\n#include \"funcs.h\"\n\n\
         #ifdef __cplusplus\nextern \"C\" {\n#endif\n\n\
         typedef void (*fn64_static_cb)(uint32_t section_index, uint32_t rom_addr,\n\
         \tuint32_t ram_addr, uint32_t size, uint32_t offset, recomp_func_t* func);\n\n\
         void fn64_sm64_register_statics(fn64_static_cb cb) {\n",
    );
    let mut emitted = 0usize;
    for (sec, vram, name) in &statics {
        let Some(&(rom, ram, size)) = section_by_index.get(sec) else {
            continue;
        };
        if *vram < ram || *vram >= ram + size {
            continue;
        }
        let offset = vram - ram;
        c.push_str(&format!(
            "\tcb({sec}u, {rom}u, {ram}u, {size}u, {offset}u, {name});\n"
        ));
        emitted += 1;
    }
    c.push_str("}\n\n#ifdef __cplusplus\n}\n#endif\n");

    let path = out_dir.join("fn64_sm64_static_registration.cpp");
    fs::write(&path, c).expect("write static registration C");
    eprintln!("sm64-boot: emitted registrar for {emitted} corpus-static functions");
    path
}

fn main() {
    println!("cargo:rerun-if-env-changed=RECOMPILED_DIR");
    println!("cargo:rerun-if-env-changed=RECOMP_H_DIR");
    println!("cargo:rerun-if-env-changed=ROM");

    // ROM is validated but not read here.
    let _ = required_env(
        "ROM",
        "Point it at the sm64-decomp build-output ROM (build/us/sm64.us.z64).",
    );

    let recompiled_dir = required_env(
        "RECOMPILED_DIR",
        "Point it at a directory containing SM64 (SM64U)'s N64Recomp-generated \
         RecompiledFuncs/*.c + recomp_overlays.inl (e.g. aki-recomp/games/SM64U/RecompiledFuncs).",
    );
    // recomp.h is MIT (N64Recomp, (c) 2024 Wiseguy) and contains no game
    // content, so it is vendored at bridge/include/vendor/. RECOMP_H_DIR
    // overrides, and SM64's bring-up points it at N64Recomp's own include/.
    let recomp_h_dir = env::var("RECOMP_H_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| vendored_recomp_h_dir());
    if !recompiled_dir.join("recomp_overlays.inl").exists() {
        panic!(
            "sm64-boot build.rs: RECOMPILED_DIR={} does not contain recomp_overlays.inl -- \
             expected the directory N64Recomp emitted SM64's RecompiledFuncs/*.c into.",
            recompiled_dir.display()
        );
    }
    if !recomp_h_dir.join("recomp.h").exists() {
        panic!(
            "sm64-boot build.rs: RECOMP_H_DIR={} does not contain recomp.h -- expected \
             N64Recomp's include/ directory.",
            recomp_h_dir.display()
        );
    }

    let bridge_dir = bridge_dir();

    let mut build = cc::Build::new();
    build
        .cpp(true)
        // The shared bridge include comes FIRST: it holds fn64's clean-room
        // stand-in librecomp/sections.h, which must shadow any real
        // (GPL-3.0-licensed) librecomp/include on the search path.
        .include(bridge_dir.join("include"))
        .include(&recompiled_dir)
        .include(&recomp_h_dir)
        .flag("-include")
        .flag("fn64_mmio_proxy.h")
        .flag_if_supported("-std=c++17")
        .flag_if_supported("-Wno-everything")
        // Generated code carries aki-recomp's NAN_CHECK debug asserts; real
        // hardware propagates NaN silently (0.0/0.0 in genuinely
        // uninitialized-BSS math is normal mid-boot). NDEBUG disables the
        // asserts, matching a release build of the same sources.
        .define("NDEBUG", None)
        // RecompiledFuncs/*.c is generated code with no warning hygiene of
        // its own (matches aki-recomp's own CMakeLists.txt build recipe).
        .warnings(false);

    let out_dir = PathBuf::from(env::var("OUT_DIR").expect("Cargo must provide OUT_DIR"));
    let (cxx_sources, jump_snapshot_count, prototype_count) =
        build_support::prepare_recompiled_cxx_sources(&recompiled_dir, &out_dir);
    let c_file_count = cxx_sources.len();

    build.files(cxx_sources);
    eprintln!(
        "sm64-boot: compiling {c_file_count} RecompiledFuncs/*.c files from {} \
         ({jump_snapshot_count} C jump snapshots normalized and {prototype_count} missing C \
         prototypes supplied for C++)",
        recompiled_dir.display()
    );

    build.compile("sm64_recompiled");

    // The bridge glue (section_bridge.c) is built SEPARATELY, as C++ --
    // recomp_overlays.inl's SectionTableEntry initializers use `nullptr`
    // (the file was generated for a C++ port build), which is not valid in C.
    // Linkable stubs for the 9 libultra internals SM64's corpus references but
    // does not define (and fn64-abi has no host adapter for) -- see
    // bridge/sm64_missing_stubs.c's header. Compiled alongside the section
    // bridge so its symbols are in the same archive Cargo links.
    let missing_stubs = manifest_dir_bridge().join("sm64_missing_stubs.c");
    assert!(
        missing_stubs.is_file(),
        "sm64-boot build.rs: missing stub source {} not found",
        missing_stubs.display()
    );

    // SM64's corpus (unlike OoT's) emits 500 `static_<section>_<vram>`
    // functions that are CALLED directly (C-to-C) from other recompiled
    // functions but are OMITTED from `recomp_overlays.inl`'s section_table --
    // so `register_linked_sections` never registers them, and the first one
    // the guest calls trips fn64's `fn64_c_recompiled_function_enter`
    // "entered native callable ... was not registered" trap. These are the 73
    // "Unnamed (func_*)" gap the syms header notes, expanded across all files.
    // Generate a companion C registrar (out-of-tree, into OUT_DIR -- ZERO game
    // content in the repo) that walks each static and hands it to a Rust
    // callback with its owning section's geometry, so the harness can register
    // the missing pointer -> destination mappings before boot.
    let static_registrar = emit_static_registration(&recompiled_dir, &out_dir);

    let mut bridge_build = cc::Build::new();
    bridge_build
        .cpp(true)
        .include(bridge_dir.join("include"))
        .include(&recompiled_dir)
        .include(&recomp_h_dir)
        .flag_if_supported("-Wno-everything")
        .warnings(false)
        .file(bridge_dir.join("section_bridge.c"))
        .file(&missing_stubs)
        .file(&static_registrar);
    bridge_build.compile("sm64_bridge");
    println!("cargo:rerun-if-changed={}", missing_stubs.display());

    // Bind the exact machine-code archives Cargo will link. The digest wire
    // contains only stable logical labels and bytes, never filesystem paths or
    // host pointers.
    let program_identity =
        lowercase_hex(native_program_identity::native_program_archives_sha256([
            (
                "generated-code".to_owned(),
                produced_archive_bytes(&out_dir, "sm64_recompiled"),
            ),
            (
                "section-bridge".to_owned(),
                produced_archive_bytes(&out_dir, "sm64_bridge"),
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
