//! Whole-ROM driver for `fn64-cpu-runtime`.
//!
//! Loads an N64Recomp config (`oot.toml`) + its symbol dump via
//! [`fn64_recomp::load_config`], then runs the Rust recompiler over EVERY
//! function in the ROM — **without bailing on the first bad function**. Each
//! function is classified (clean / stubbed / ROM-range error / unknown
//! opcode / runtime-trap), the clean bodies are emitted as a standalone Rust
//! library crate, and a full gap report is written and printed.
//!
//! This is the link gate for the "prove `fn64-cpu-runtime` can recompile the
//! whole OoT ROM" milestone: it emits the clean bodies together with their
//! safe indirect dispatcher, while routing host-owned trap bodies through the
//! runtime resolver. The report remains an honest inventory of every body the
//! generated module does not contain.
//!
//! # Usage
//!
//! ```text
//! recompile_rom --config <oot.toml> --rom <oot.z64> [--out <dir>] [--profile <profile.toml>]
//! # or via env:
//! FN64_CONFIG=<oot.toml> FN64_ROM=<oot.z64> recompile_rom
//! ```
//!
//! When `--profile`/`RECOMP_RS_PROFILE` is absent, the driver loads a
//! sibling `profile.toml` if one exists. This keeps the mechanical one-command
//! lane profile-aware while preserving bare N64Recomp configs that have no
//! companion profile.
//!
//! The config and ROM live OUT-OF-TREE (game-derived content); this binary
//! only ever reads their paths from args/env, never vendoring them.

#![forbid(unsafe_code)]

use std::collections::BTreeMap;
use std::collections::HashSet;
use std::path::PathBuf;

use fn64_cpu_runtime::{decode, Instruction};
use fn64_cpu_runtime_codegen::swallowed_entries::{
    apply_repairs, cross_check_region, CodeRegion, CrossCheck, DumpFunction,
};
use fn64_cpu_runtime_codegen::{
    emit_function_resolved, emit_lookup_dispatcher, module::SymbolTable, FuncInput,
};
use fn64_recomp::{load_config, Function, InstructionPatch, RecompConfig, Section};

fn main() -> std::process::ExitCode {
    let args = match Args::parse() {
        Ok(a) => a,
        Err(msg) => {
            eprintln!("error: {msg}\n\n{USAGE}");
            return std::process::ExitCode::from(2);
        }
    };

    let cfg = match load_config(&args.config, Some(args.rom.clone())) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("failed to load config {}: {e}", args.config.display());
            return std::process::ExitCode::from(2);
        }
    };

    let rom = match std::fs::read(&cfg.rom_file_path) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("failed to read ROM {}: {e}", cfg.rom_file_path.display());
            return std::process::ExitCode::from(2);
        }
    };
    eprintln!(
        "loaded config: {} sections, {} functions; ROM {} ({} bytes)",
        cfg.sections.len(),
        cfg.sections
            .iter()
            .map(|s| s.functions.len())
            .sum::<usize>(),
        cfg.rom_file_path.display(),
        rom.len()
    );

    let profile = resolve_profile_path(&args.config, args.profile, |path| path.is_file());
    let force_recompile = match profile.as_deref() {
        Some(path) => match load_force_recompile(path) {
            Ok(names) => {
                eprintln!(
                    "loaded {} force-recompile overrides from {}",
                    names.len(),
                    path.display()
                );
                names
            }
            Err(e) => {
                eprintln!("failed to load profile {}: {e}", path.display());
                return std::process::ExitCode::from(2);
            }
        },
        None => HashSet::new(),
    };

    // Cross-check the pre-baked symbol dump against the ROM's own `jal`
    // evidence BEFORE recompiling. A dump that swallowed a real function
    // entry inside a preceding function's declared size produces a runtime
    // `lookup: no recompiled function ... at vram` trap with no build-time
    // signal at all; this turns that into a named build-time diagnostic, and
    // repairs the entries whose containing function provably returned first.
    let mut cfg = cfg;
    let diagnostic = check_and_repair_symbol_dump(&mut cfg, &rom);
    eprint!("{diagnostic}");

    let report = run(&cfg, &rom, &force_recompile);

    if let Err(e) = std::fs::create_dir_all(args.out.join("src")) {
        eprintln!(
            "failed to create emitted crate directory {}: {e}",
            args.out.display()
        );
        return std::process::ExitCode::from(2);
    }
    for (relative_path, contents) in &report.crate_files {
        let path = args.out.join(relative_path);
        if let Err(e) = std::fs::write(&path, contents) {
            eprintln!("failed to write emitted crate file {}: {e}", path.display());
            return std::process::ExitCode::from(2);
        }
    }
    let report_path = args.out.join("gap-report.md");
    if let Err(e) = std::fs::write(&report_path, report.render_markdown(&cfg)) {
        eprintln!("failed to write report {}: {e}", report_path.display());
        return std::process::ExitCode::from(2);
    }

    // Console summary.
    eprintln!("\n{}", report.render_summary());
    eprintln!(
        "aggregate generated source: {} bytes across {RECOMPILED_PART_COUNT} balanced parts",
        report.module.len()
    );
    eprintln!(
        "recompiled function crate written to {}",
        args.out.display()
    );
    eprintln!("gap report written to {}", report_path.display());

    std::process::ExitCode::SUCCESS
}

const USAGE: &str = "usage: recompile_rom --config <oot.toml> --rom <oot.z64> [--out <dir>] \
                     [--profile <profile.toml>]\n\
                     env fallbacks: FN64_CONFIG, FN64_ROM, FN64_OUT, RECOMP_RS_PROFILE; \
                     otherwise loads sibling profile.toml when present";

struct Args {
    config: PathBuf,
    rom: PathBuf,
    out: PathBuf,
    profile: Option<PathBuf>,
}

/// The `OOT_*` spellings of these knobs are gone; an unset var means "off", so
/// a silent rename would turn a stale `OOT_CONFIG=…` invocation into a no-op.
fn reject_legacy_env() {
    for (old, new) in [
        ("OOT_CONFIG", "FN64_CONFIG"),
        ("OOT_ROM", "FN64_ROM"),
        ("OOT_OUT", "FN64_OUT"),
    ] {
        if std::env::var_os(old).is_some() {
            panic!("{old} was renamed to {new}; unset {old} and set {new} instead");
        }
    }
}

impl Args {
    fn parse() -> Result<Self, String> {
        reject_legacy_env();

        let mut config = std::env::var("FN64_CONFIG").ok().map(PathBuf::from);
        let mut rom = std::env::var("FN64_ROM").ok().map(PathBuf::from);
        let mut out = std::env::var("FN64_OUT").ok().map(PathBuf::from);
        let mut profile = std::env::var("RECOMP_RS_PROFILE").ok().map(PathBuf::from);

        let mut it = std::env::args().skip(1);
        while let Some(arg) = it.next() {
            match arg.as_str() {
                "--config" => config = Some(PathBuf::from(next(&mut it, "--config")?)),
                "--rom" => rom = Some(PathBuf::from(next(&mut it, "--rom")?)),
                "--out" => out = Some(PathBuf::from(next(&mut it, "--out")?)),
                "--profile" => profile = Some(PathBuf::from(next(&mut it, "--profile")?)),
                "-h" | "--help" => return Err("help".to_string()),
                other => return Err(format!("unknown argument {other:?}")),
            }
        }
        Ok(Args {
            config: config.ok_or("--config (or FN64_CONFIG) is required")?,
            rom: rom.ok_or("--rom (or FN64_ROM) is required")?,
            out: out.unwrap_or_else(|| PathBuf::from("recomp-out")),
            profile,
        })
    }
}

fn resolve_profile_path(
    config: &std::path::Path,
    explicit: Option<PathBuf>,
    exists: impl FnOnce(&std::path::Path) -> bool,
) -> Option<PathBuf> {
    explicit.or_else(|| {
        let sibling = config.with_file_name("profile.toml");
        exists(&sibling).then_some(sibling)
    })
}

#[derive(serde::Deserialize)]
struct RecompileProfile {
    syms: RecompileProfileSymbols,
}

#[derive(serde::Deserialize)]
struct RecompileProfileSymbols {
    #[serde(default)]
    force_recompile: Vec<String>,
}

fn load_force_recompile(path: &std::path::Path) -> Result<HashSet<String>, String> {
    let text = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
    let profile: RecompileProfile = toml::from_str(&text).map_err(|e| e.to_string())?;
    Ok(profile.syms.force_recompile.into_iter().collect())
}

fn next(it: &mut impl Iterator<Item = String>, flag: &str) -> Result<String, String> {
    it.next().ok_or_else(|| format!("{flag} needs a value"))
}

// ---- classification ----

/// How one function came out of the Rust recompiler.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Outcome {
    /// Emitted a real body with no `Unknown` opcode and no runtime-trap
    /// instruction — fully recompiled.
    Clean,
    /// Excluded by `[patches].stubs` — intentionally not recompiled (the
    /// cop0/cache/cop2/break class N64Recomp itself stubs).
    Stubbed,
    /// Excluded by `[patches].ignored`.
    Ignored,
    /// Function range fell outside its section's ROM extent — could not read
    /// its words at all.
    RomRange,
    /// Emitted, but contains at least one `Instruction::Unknown` — a GENUINE
    /// gap in our decoder/emitter (produces `compile_error!` in the output).
    UnknownOpcode,
    /// Contains a `panic!(...)` runtime trap (COP0/COP2/TLB/ERET/SYSCALL/
    /// BREAK). Its body is omitted and its vram is host-bound through lookup.
    RuntimeTrap,
}

struct FuncResult {
    name: String,
    vram: u32,
    section: String,
    outcome: Outcome,
    /// For UnknownOpcode: the distinct (opcode-group, word, mnemonic) samples.
    unknown_words: Vec<u32>,
    /// For RuntimeTrap: the trap mnemonics present.
    trap_kinds: Vec<&'static str>,
}

struct Report {
    /// The historical single-module rendering retained for the gap-report and
    /// unit tests that assert directly on emitted function text. The on-disk
    /// product is `crate_files`; oot-boot never parses this string.
    module: String,
    /// Standalone Cargo package files, relative to the requested output root.
    /// Generated game content stays out-of-tree and is compiled once as an
    /// rlib dependency instead of being textually included by every consumer.
    crate_files: Vec<(PathBuf, String)>,
    results: Vec<FuncResult>,
    lookup_sites: usize,
    recompiled_symbols: usize,
    /// Vrams claimed by more than one overlay bank, with every claimant's
    /// `(section index, name)`. These are absent from the flat dispatch table
    /// by construction; the report names them so a run that reads
    /// "99%+ linkable" cannot hide bodies that no direct call can reach.
    banked_claimants: Vec<(u32, Vec<(usize, String)>)>,
}

/// Enough buckets to keep generated codegen units balanced without producing
/// hundreds of tiny files for OoT's 472 sections. Function bodies are assigned
/// deterministically by greedy byte-size bin packing below.
const RECOMPILED_PART_COUNT: usize = 64;

const GENERATED_USE: &str = "use fn64_cpu_runtime::{call_host_or_recompiled, pause_self, resolve_host_function, RecompContext, RecompFunc, Rdram, round_ties_even_f32, round_ties_even_f64};\n\n";

fn run(cfg: &RecompConfig, rom: &[u8], force_recompile: &HashSet<String>) -> Report {
    assert_every_instruction_patch_is_reachable(cfg);
    // The bootstrap config's stub scan treats every BREAK as privileged. MIPS
    // compilers also emit two rigid BREAK shapes after integer DIV/DIVU for
    // divide-by-zero and INT_MIN/-1. Recognize the complete guard sequences,
    // not just the opcode, so the whole recurring class is admitted while an
    // arbitrary BREAK remains host-bound/stubbed.
    let auto_div_guards: HashSet<&str> = cfg
        .sections
        .iter()
        .flat_map(|section| {
            section.functions.iter().filter_map(move |func| {
                if !cfg.patches.stubs.iter().any(|name| name == &func.name) {
                    return None;
                }
                let words = read_func_words(rom, section, func, &cfg.patches.instructions)?;
                let instrs: Vec<_> = words.into_iter().map(decode).collect();
                compiler_div_guards_only(&instrs).then_some(func.name.as_str())
            })
        })
        .collect();
    let stubbed: HashSet<&str> = cfg
        .patches
        .stubs
        .iter()
        .map(String::as_str)
        .filter(|name| !force_recompile.contains(*name) && !auto_div_guards.contains(*name))
        .collect();
    let ignored: HashSet<&str> = cfg.patches.ignored.iter().map(String::as_str).collect();

    // Runtime-trap functions are absent from the recompiled table: direct JAL and
    // computed JALR calls to one must both use the host resolver, never bind
    // to a recompiled panic body merely because its name is statically known.
    // Section index is carried through: overlay banks share a VRAM window, so
    // a collided vram is only dispatchable if the emitted table knows which
    // section owns each claimant. The index is the config's section order,
    // which is also `RECOMPILED_SECTION_GEOMETRY`'s emission order and the
    // registration order the host's `SectionRegistry` assigns.
    let symbols = SymbolTable::from_section_entries(cfg.sections.iter().enumerate().flat_map(
        |(section_index, s)| {
            let (stubbed, ignored, auto_div_guards, force_recompile) =
                (&stubbed, &ignored, &auto_div_guards, &force_recompile);
            s.functions.iter().filter_map(move |f| {
                if stubbed.contains(f.name.as_str()) || ignored.contains(f.name.as_str()) {
                    return None;
                }
                let forced = force_recompile.contains(f.name.as_str())
                    || auto_div_guards.contains(f.name.as_str());
                let words = read_func_words(rom, s, f, &cfg.patches.instructions)?;
                let mut decoded = words.iter().map(|&word| decode(word));
                decoded
                    .all(|instr| {
                        !matches!(instr, Instruction::Unknown { .. })
                            && (forced && matches!(instr, Instruction::Break { .. })
                                || trap_kind(&instr).is_none())
                    })
                    .then(|| (section_index, f.name.clone(), f.vram))
            })
        },
    ));

    let mut module = String::new();
    module.push_str("// Generated by fn64-cpu-runtime whole-ROM driver (recompile_rom).\n");
    module.push_str(
        "// Only CLEAN recompiled functions are included; host-owned trap functions route through lookup.\n",
    );
    // An outer module attribute works both when funcs.rs is compiled as a
    // crate root and when a boot harness `include!`s it inside its own module.
    // An inner `#![allow]` is rejected in the latter context.
    module.push_str("#[allow(clippy::all, unused, non_snake_case)]\nmod generated {\n");
    module.push_str("pub const FN64_FUNCTION_ENTRY_OBSERVATION_SCHEMA: fn64_cpu_runtime::FunctionEntryObservationSchema = fn64_cpu_runtime::FUNCTION_ENTRY_OBSERVATION_SCHEMA;\n");
    module.push_str("#[allow(unused_imports)]\n");
    module.push_str(
        "use fn64_cpu_runtime::{call_host_or_recompiled, pause_self, resolve_host_function, RecompContext, RecompFunc, Rdram, round_ties_even_f32, round_ties_even_f64};\n\n",
    );

    let mut results = Vec::new();
    let mut lookup_sites = 0usize;
    let mut part_bodies = vec![String::new(); RECOMPILED_PART_COUNT];

    for section in &cfg.sections {
        for func in &section.functions {
            if stubbed.contains(func.name.as_str()) {
                results.push(FuncResult::simple(func, section, Outcome::Stubbed));
                continue;
            }
            if ignored.contains(func.name.as_str()) {
                results.push(FuncResult::simple(func, section, Outcome::Ignored));
                continue;
            }
            let words = match read_func_words(rom, section, func, &cfg.patches.instructions) {
                Some(w) => w,
                None => {
                    results.push(FuncResult::simple(func, section, Outcome::RomRange));
                    continue;
                }
            };

            // Classify from decoded instructions.
            let instrs: Vec<Instruction> = words.iter().map(|&w| decode(w)).collect();
            let unknown_words: Vec<u32> = instrs
                .iter()
                .filter_map(|instr| match instr {
                    Instruction::Unknown { word } => Some(*word),
                    _ => None,
                })
                .collect();
            // Profile-vetted force-recompile entries are compiler-generated
            // divide guards: retain their BREAK as a loud path-local panic,
            // but do not discard the entire otherwise ordinary function.
            // Any other privileged/trapping instruction still host-binds the
            // body even if the profile names it.
            let forced = force_recompile.contains(func.name.as_str())
                || auto_div_guards.contains(func.name.as_str());
            let trap_kinds: Vec<&'static str> = instrs
                .iter()
                .filter_map(|instr| {
                    if forced && matches!(instr, Instruction::Break { .. }) {
                        None
                    } else {
                        trap_kind(instr)
                    }
                })
                .collect();

            let outcome = if !unknown_words.is_empty() {
                Outcome::UnknownOpcode
            } else if !trap_kinds.is_empty() {
                Outcome::RuntimeTrap
            } else {
                Outcome::Clean
            };

            // RuntimeTrap functions are host-owned and absent from `symbols`,
            // so every direct caller also takes lookup(vram). Unknown bodies
            // are not emitted because they cannot compile.
            if outcome == Outcome::Clean {
                let input = FuncInput {
                    name: &func.name,
                    vram: func.vram,
                    words: &words,
                };
                let body = emit_function_resolved(&input, &symbols);
                lookup_sites += body.matches("lookup(").count();
                module.push_str(&body);
                module.push('\n');

                // Close the recurring giant-section failure mode by putting
                // each body in the currently smallest bucket, rather than
                // assuming config section sizes are roughly uniform.
                let bucket = part_bodies
                    .iter()
                    .enumerate()
                    .min_by_key(|(_, bodies)| bodies.len())
                    .map(|(index, _)| index)
                    .expect("RECOMPILED_PART_COUNT is non-zero");
                part_bodies[bucket].push_str(&body);
                part_bodies[bucket].push('\n');
            }

            let mut trap_kinds = trap_kinds;
            trap_kinds.sort_unstable();
            trap_kinds.dedup();
            results.push(FuncResult {
                name: func.name.clone(),
                vram: func.vram,
                section: section.name.clone(),
                outcome,
                unknown_words,
                trap_kinds,
            });
        }
    }

    module.push_str(&emit_lookup_dispatcher(&symbols));
    module.push_str(
        "\n/// (ROM address, static vram, byte size) in config section order.\n\
         pub static RECOMPILED_SECTION_GEOMETRY: &[(u32, u32, u32)] = &[\n",
    );
    for section in &cfg.sections {
        let _ = std::fmt::Write::write_fmt(
            &mut module,
            format_args!(
                "    ({:#010X}, {:#010X}, {:#010X}),\n",
                section.rom, section.vram, section.size
            ),
        );
    }
    module.push_str("];\n");
    module.push_str("}\npub use generated::*;\n");

    let mut crate_files = Vec::with_capacity(RECOMPILED_PART_COUNT + 2);
    crate_files.push((PathBuf::from("Cargo.toml"), render_generated_manifest()));
    for (index, bodies) in part_bodies.into_iter().enumerate() {
        let mut part = String::new();
        part.push_str("// Generated by fn64-cpu-runtime whole-ROM driver (recompile_rom).\n");
        part.push_str("// One balanced bucket of typed recompiled function bodies.\n");
        part.push_str("#[allow(unused_imports)]\nuse crate::*;\n");
        part.push_str("#[allow(unused_imports)]\n");
        part.push_str(GENERATED_USE);
        part.push_str(&bodies);
        crate_files.push((PathBuf::from(format!("src/part_{index:03}.rs")), part));
    }
    crate_files.push((
        PathBuf::from("src/lib.rs"),
        render_generated_lib(&symbols, &cfg.sections),
    ));

    Report {
        module,
        crate_files,
        results,
        lookup_sites,
        recompiled_symbols: symbols.len(),
        banked_claimants: symbols
            .ambiguous_claimants()
            .into_iter()
            .map(|(vram, claimants)| {
                (
                    vram,
                    claimants
                        .into_iter()
                        .map(|(section, name)| (section, name.to_string()))
                        .collect(),
                )
            })
            .collect(),
    }
}

fn render_generated_manifest() -> String {
    let runtime_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("codegen crate has a workspace parent")
        .join("fn64-cpu-runtime");
    let quoted_path = toml::Value::String(runtime_path.to_string_lossy().into_owned()).to_string();
    format!(
        "# Generated by fn64-cpu-runtime whole-ROM driver (recompile_rom).\n\
         [package]\n\
         name = \"oot-recompiled\"\n\
         version = \"0.0.0\"\n\
         edition = \"2021\"\n\
         license = \"MIT OR Apache-2.0\"\n\
         publish = false\n\n\
         [dependencies]\n\
         fn64-cpu-runtime = {{ path = {quoted_path} }}\n"
    )
}

fn render_generated_lib(symbols: &SymbolTable, sections: &[Section]) -> String {
    let mut root = String::new();
    root.push_str("// Generated by fn64-cpu-runtime whole-ROM driver (recompile_rom).\n");
    root.push_str(
        "// Standalone generated crate: consumers link this rlib, never include! its source.\n",
    );
    root.push_str("#![forbid(unsafe_code)]\n");
    root.push_str("#![allow(clippy::all, unused, non_snake_case)]\n\n");
    root.push_str("pub const FN64_FUNCTION_ENTRY_OBSERVATION_SCHEMA: fn64_cpu_runtime::FunctionEntryObservationSchema = fn64_cpu_runtime::FUNCTION_ENTRY_OBSERVATION_SCHEMA;\n\n");
    for index in 0..RECOMPILED_PART_COUNT {
        root.push_str(&format!(
            "mod part_{index:03};\npub use part_{index:03}::*;\n"
        ));
    }
    root.push_str("\n#[allow(unused_imports)]\n");
    root.push_str(GENERATED_USE);
    root.push_str(&emit_lookup_dispatcher(symbols));
    root.push_str(
        "\n/// (ROM address, static vram, byte size) in config section order.\n\
         pub static RECOMPILED_SECTION_GEOMETRY: &[(u32, u32, u32)] = &[\n",
    );
    for section in sections {
        let _ = std::fmt::Write::write_fmt(
            &mut root,
            format_args!(
                "    ({:#010X}, {:#010X}, {:#010X}),\n",
                section.rom, section.vram, section.size
            ),
        );
    }
    root.push_str("];\n");
    root
}

fn compiler_div_guards_only(instrs: &[Instruction]) -> bool {
    let mut saw_break = false;
    for (i, instr) in instrs.iter().enumerate() {
        if matches!(instr, Instruction::Unknown { .. }) {
            return false;
        }
        let Some(kind) = trap_kind(instr) else {
            continue;
        };
        if kind != "break" {
            return false;
        }
        saw_break = true;
        let divide_by_zero = i >= 2
            && matches!(instrs[i - 2], Instruction::Bne { rt: 0, off: 2, .. })
            && matches!(instrs[i - 1], Instruction::Nop);
        let signed_overflow = i >= 5
            && matches!(
                instrs[i - 5],
                Instruction::Addiu {
                    rt: 1,
                    rs: 0,
                    imm: -1
                }
            )
            && matches!(instrs[i - 4], Instruction::Bne { rt: 1, off: 4, .. })
            && matches!(instrs[i - 3], Instruction::Lui { rt: 1, imm: 0x8000 })
            && matches!(instrs[i - 2], Instruction::Bne { rt: 1, off: 2, .. })
            && matches!(instrs[i - 1], Instruction::Nop);
        if !divide_by_zero && !signed_overflow {
            return false;
        }
    }
    saw_break
}

impl FuncResult {
    fn simple(func: &Function, section: &Section, outcome: Outcome) -> Self {
        FuncResult {
            name: func.name.clone(),
            vram: func.vram,
            section: section.name.clone(),
            outcome,
            unknown_words: Vec::new(),
            trap_kinds: Vec::new(),
        }
    }
}

/// Which runtime-trap class an instruction emits, if any (mirrors emit.rs's
/// `panic!(...)` arms). `None` for ordinary instructions.
fn trap_kind(instr: &Instruction) -> Option<&'static str> {
    use Instruction::*;
    Some(match instr {
        Mfc0 {
            cop0d: 0 | 2 | 3 | 4 | 5 | 6 | 8 | 9 | 10 | 11 | 12 | 13 | 14 | 18 | 19 | 30,
            ..
        }
        | Mtc0 {
            cop0d: 0 | 2 | 3 | 4 | 5 | 6 | 9 | 10 | 11 | 12 | 13 | 14 | 18 | 19 | 30,
            ..
        }
        | Tlbwi
        | Tlbp
        | Tlbr => return None,
        Mfc0 { .. } | Mtc0 { .. } => "cop0-move",
        Dmfc0 { .. } | Dmtc0 { .. } => "cop0-dmove",
        Eret => "eret",
        Tlbwr => "tlb",
        Mfc2 { .. } | Mtc2 { .. } | Cfc2 { .. } | Ctc2 { .. } => "cop2",
        Syscall { .. } => "syscall",
        Break { .. } => "break",
        _ => return None,
    })
}

/// The MIPS primary opcode field (bits 31..26).
fn primary_op(word: u32) -> u32 {
    (word >> 26) & 0x3F
}

/// A short human label for an unknown word, grouped by primary opcode (and
/// funct for SPECIAL/REGIMM/COPz) so the report can bucket them.
fn unknown_group(word: u32) -> String {
    let op = primary_op(word);
    match op {
        0x00 => format!("SPECIAL funct=0x{:02X}", word & 0x3F),
        0x01 => format!("REGIMM rt=0x{:02X}", (word >> 16) & 0x1F),
        0x10 => format!("COP0 rs=0x{:02X}", (word >> 21) & 0x1F),
        0x11 => format!("COP1 rs=0x{:02X}", (word >> 21) & 0x1F),
        0x12 => format!("COP2 rs=0x{:02X}", (word >> 21) & 0x1F),
        0x13 => format!("COP1X funct=0x{:02X}", word & 0x3F),
        _ => format!("op=0x{:02X}", op),
    }
}

// ---- report rendering ----

impl Report {
    fn counts(&self) -> BTreeMap<&'static str, usize> {
        let mut m: BTreeMap<&'static str, usize> = BTreeMap::new();
        for r in &self.results {
            *m.entry(outcome_label(r.outcome)).or_default() += 1;
        }
        m
    }

    fn render_summary(&self) -> String {
        let total = self.results.len();
        let c = self.counts();
        let clean = c.get("clean").copied().unwrap_or(0);
        let trap = c.get("runtime-trap").copied().unwrap_or(0);
        let mut s = String::new();
        s.push_str("=== whole-ROM recompile summary ===\n");
        s.push_str(&format!("total functions: {total}\n"));
        for (label, n) in &c {
            let pct = 100.0 * *n as f64 / total as f64;
            s.push_str(&format!("  {label:16} {n:6}  ({pct:5.2}%)\n"));
        }
        s.push_str(&format!(
            "linkable (recompiled + host-bound): {} ({:.2}%)\n",
            clean + trap,
            100.0 * (clean + trap) as f64 / total as f64
        ));
        let banked_bodies: usize = self.banked_claimants.iter().map(|(_, c)| c.len()).sum();
        s.push_str(&format!(
            "bank-ambiguous vrams: {} ({banked_bodies} bodies) -- dispatchable only via \
             residency, absent from the flat table\n",
            self.banked_claimants.len(),
        ));
        s
    }

    /// Name every vram two or more overlay banks claim.
    ///
    /// The "linkable" percentage above counts a body as linkable when it
    /// recompiled cleanly, which a bank-collided body does. What it cannot
    /// say is whether a *call* can reach that body: two differently-named
    /// functions at one vram are dropped from the flat `LOOKUP_TABLE`, so a
    /// report that stopped at the percentage read clean while hiding the gap.
    /// These rows are that gap, stated.
    fn render_bank_ambiguity(&self) -> String {
        let mut s = String::new();
        s.push_str("## Bank-ambiguous vrams (dispatchable only via overlay residency)\n\n");
        if self.banked_claimants.is_empty() {
            s.push_str(
                "None. Every recompiled vram in this config is claimed by exactly one body, so \
                 the flat `LOOKUP_TABLE` expresses the whole dispatch surface.\n\n",
            );
            return s;
        }
        let bodies: usize = self.banked_claimants.iter().map(|(_, c)| c.len()).sum();
        s.push_str(&format!(
            "**{} vrams, {bodies} bodies.** Overlay banks share a VRAM window, so these \
             addresses are claimed by more than one differently-named function and are \
             deliberately absent from the flat `LOOKUP_TABLE` -- a flat `vram -> fn` array \
             cannot say which bank is resident. They are emitted instead into \
             `BANKED_LOOKUP_TABLE` and resolved at call time by \
             `fn64_cpu_runtime::resolve_banked_function` against the section residency the \
             host's `SectionRegistry` tracks from the guest's own DMA. A call arriving while \
             zero or more than one claimant is resident is a named trap, not a guess.\n\n\
             These bodies recompiled cleanly and are counted in the linkable percentage above; \
             what this table adds is that reaching them requires residency, so a host that \
             never marks their sections loaded cannot call them at all.\n\n",
            self.banked_claimants.len(),
        ));
        s.push_str("| vram | claimants (section: name) |\n|---|---|\n");
        for (vram, claimants) in &self.banked_claimants {
            let names = claimants
                .iter()
                .map(|(section, name)| format!("{section}: `{name}`"))
                .collect::<Vec<_>>()
                .join(", ");
            s.push_str(&format!("| `{vram:#010X}` | {names} |\n"));
        }
        s.push('\n');
        s
    }

    fn render_markdown(&self, cfg: &RecompConfig) -> String {
        let total = self.results.len();
        let c = self.counts();
        let mut s = String::new();
        s.push_str("# fn64-cpu-runtime whole-ROM gap report (OoT NTSC 1.0)\n\n");
        s.push_str(&format!(
            "Driver: `recompile_rom`. Config: `{}`.\n\n",
            cfg.rom_file_path.display()
        ));
        s.push_str(&format!(
            "Total functions in symbol dump: **{total}** across {} sections.\n\n",
            cfg.sections.len()
        ));

        s.push_str("## Outcome counts\n\n");
        s.push_str("| outcome | count | % |\n|---|---:|---:|\n");
        for (label, n) in &c {
            s.push_str(&format!(
                "| {label} | {n} | {:.2}% |\n",
                100.0 * *n as f64 / total as f64
            ));
        }
        let clean = c.get("clean").copied().unwrap_or(0);
        let trap = c.get("runtime-trap").copied().unwrap_or(0);
        s.push_str(&format!(
            "\n**Linkable (recompiled + host-bound): {} / {} = {:.2}%.**\n\n",
            clean + trap,
            total,
            100.0 * (clean + trap) as f64 / total as f64
        ));
        s.push_str(&self.render_bank_ambiguity());

        // Runtime-trap breakdown by kind.
        s.push_str("## Runtime-trap functions (host-bound; panic bodies are not emitted)\n\n");
        s.push_str(
            "These emit a `panic!(...)` for a privileged/unmodeled op (COP0/COP2/TLB/ERET/\
             SYSCALL/BREAK). They are NOT decoder gaps, but their recompiled bodies are deliberately \
             omitted from the recompiled symbol table. Direct JAL and computed JALR calls both route \
             through `lookup(vram)`, which consults the host resolver before the recompiled table. \
             Libultra/OS entries bind to fn64 shims; guest functions containing an assertion BREAK \
             require a host break adapter rather than being mislabeled as libultra.\n\n",
        );
        let mut trap_by_kind: BTreeMap<&'static str, Vec<&FuncResult>> = BTreeMap::new();
        for r in &self.results {
            if r.outcome == Outcome::RuntimeTrap {
                for k in &r.trap_kinds {
                    trap_by_kind.entry(k).or_default().push(r);
                }
            }
        }
        s.push_str("| trap kind | functions | examples |\n|---|---:|---|\n");
        for (kind, funcs) in &trap_by_kind {
            let examples = funcs
                .iter()
                .take(3)
                .map(|r| format!("`{}`@{:#010X}", r.name, r.vram))
                .collect::<Vec<_>>()
                .join(", ");
            s.push_str(&format!("| {kind} | {} | {examples} |\n", funcs.len()));
        }
        s.push('\n');

        s.push_str("### Complete host-bound inventory\n\n");
        s.push_str("| function | vram | section | trap kinds |\n|---|---:|---|---|\n");
        for r in self
            .results
            .iter()
            .filter(|r| r.outcome == Outcome::RuntimeTrap)
        {
            s.push_str(&format!(
                "| `{}` | `{:#010X}` | `{}` | {} |\n",
                r.name,
                r.vram,
                r.section,
                r.trap_kinds.join(", ")
            ));
        }
        s.push('\n');

        // Unknown-opcode breakdown (the genuine decoder gaps).
        s.push_str("## Unknown-opcode functions (GENUINE decoder/emitter gaps)\n\n");
        s.push_str(
            "Each contains at least one word the decoder returns as `Instruction::Unknown`, \
             which the emitter renders as a `compile_error!` — so the function does not compile. \
             Bucketed by opcode group; closing a group unblocks every function that only trips \
             on it.\n\n",
        );
        // group -> (count of occurrences, example (word, func, vram) up to 3)
        let mut group_occurrences: BTreeMap<String, usize> = BTreeMap::new();
        let mut group_funcs: BTreeMap<String, Vec<(u32, String, u32)>> = BTreeMap::new();
        for r in &self.results {
            if r.outcome != Outcome::UnknownOpcode {
                continue;
            }
            // Distinct groups within this function.
            let mut seen: BTreeMap<String, u32> = BTreeMap::new();
            for &w in &r.unknown_words {
                seen.entry(unknown_group(w)).or_insert(w);
            }
            for (g, w) in seen {
                *group_occurrences.entry(g.clone()).or_default() += 1;
                let ex = group_funcs.entry(g).or_default();
                if ex.len() < 3 {
                    ex.push((w, r.name.clone(), r.vram));
                }
            }
        }
        if group_occurrences.is_empty() {
            s.push_str("_None — every readable function decoded fully._\n\n");
        } else {
            // Sort by count desc for a prioritized list.
            let mut groups: Vec<(&String, &usize)> = group_occurrences.iter().collect();
            groups.sort_by(|a, b| b.1.cmp(a.1).then(a.0.cmp(b.0)));
            s.push_str(
                "| opcode group | funcs blocked | example words / functions |\n|---|---:|---|\n",
            );
            for (g, n) in &groups {
                let examples = group_funcs
                    .get(*g)
                    .map(|v| {
                        v.iter()
                            .map(|(w, name, vram)| {
                                format!("`{:#010X}` in `{}`@{:#010X}", w, name, vram)
                            })
                            .collect::<Vec<_>>()
                            .join("; ")
                    })
                    .unwrap_or_default();
                s.push_str(&format!("| {g} | {n} | {examples} |\n"));
            }
            s.push('\n');
            s.push_str("### Prioritized close list (most functions unblocked first)\n\n");
            for (i, (g, n)) in groups.iter().enumerate() {
                s.push_str(&format!("{}. **{g}** — unblocks {n} functions.\n", i + 1));
            }
            s.push('\n');
            // Note the known false-positive: RSP microcode blobs symbol-dumped
            // as "functions" decode as garbage COP2 words. They are NOT CPU
            // code and should be stubbed/ignored, not decoded.
            if group_funcs
                .values()
                .flatten()
                .any(|(_, name, _)| name.contains("rspboot") || name.contains("TextStart"))
            {
                s.push_str(
                    "> Note: entries like `rspbootTextStart` are RSP microcode/data blobs the \
                     symbol dump lists as \"functions\"; their bytes are RSP (or raw data), not \
                     VR4300 CPU code, so the COP2 `Unknown` words are expected garbage. These \
                     belong in `[patches].stubs`/`ignored`, not in the decoder — closing them is \
                     a config fix, not a recompiler gap.\n\n",
                );
            }
        }

        // ROM-range failures.
        let rom_range: Vec<&FuncResult> = self
            .results
            .iter()
            .filter(|r| r.outcome == Outcome::RomRange)
            .collect();
        s.push_str("## ROM-range failures (function outside its section's ROM extent)\n\n");
        if rom_range.is_empty() {
            s.push_str("_None._\n\n");
        } else {
            s.push_str(&format!("{} functions:\n\n", rom_range.len()));
            for r in rom_range.iter().take(20) {
                s.push_str(&format!(
                    "- `{}`@{:#010X} (section `{}`)\n",
                    r.name, r.vram, r.section
                ));
            }
            if rom_range.len() > 20 {
                s.push_str(&format!("- … and {} more\n", rom_range.len() - 20));
            }
            s.push('\n');
        }

        // Stubs vs real gaps.
        s.push_str("## Stubs (expected) vs genuine gaps\n\n");
        let stubbed_n = c.get("stubbed").copied().unwrap_or(0);
        let ignored_n = c.get("ignored").copied().unwrap_or(0);
        let unknown_n = c.get("unknown-opcode").copied().unwrap_or(0);
        let romrange_n = c.get("rom-range").copied().unwrap_or(0);
        s.push_str(&format!(
            "- **Expected/intentional:** {} stubbed + {} ignored = {} functions the config \
             deliberately excludes (the cop0/cache/cop2/break class N64Recomp also can't do).\n",
            stubbed_n,
            ignored_n,
            stubbed_n + ignored_n
        ));
        s.push_str(&format!(
            "- **Genuine gaps in OUR recompiler:** {} unknown-opcode + {} ROM-range = {} \
             functions that fail for reasons we own.\n",
            unknown_n,
            romrange_n,
            unknown_n + romrange_n
        ));
        s.push_str(&format!(
            "- **Runtime-trap (host-bound):** {} functions — omitted from recompiled dispatch so \
             neither a direct nor indirect call can select their panic bodies.\n\n",
            trap,
        ));

        s.push_str("## Whole-module indirect dispatcher\n\n");
        s.push_str(&format!(
            "The emitted module contains **{}** `lookup(addr)(ctx, mem)` call sites \
             (indirect calls: register-indirect `JALR`, or `JAL`/`J` to a vram not uniquely in \
             the recompiled symbol table). `lookup` is defined in this module and binary-searches a \
             sorted table of **{}** typed Rust function pointers. It first consults \
             `fn64_cpu_runtime::resolve_host_function`, so host-owned vrams override recompiled \
             entries and omitted trap bodies fail loudly unless the boot host binds them. The \
             table and callback use ordinary safe `fn` pointers: no `unsafe`, pointer cast, or \
             `transmute`.\n\n",
            self.lookup_sites, self.recompiled_symbols,
        ));

        s
    }
}

fn outcome_label(o: Outcome) -> &'static str {
    match o {
        Outcome::Clean => "clean",
        Outcome::Stubbed => "stubbed",
        Outcome::Ignored => "ignored",
        Outcome::RomRange => "rom-range",
        Outcome::UnknownOpcode => "unknown-opcode",
        Outcome::RuntimeTrap => "runtime-trap",
    }
}

/// Slice a function's instruction words out of the ROM (big-endian → host
/// `u32`), then apply every `[[patches.instruction]]` that targets this
/// function. Mirrors `RsRecompiler::read_func_words`, but here it is a
/// classification input, not a hard error.
///
/// Patch application belongs here, at the single point all three callers
/// (the div-guard prescan, the symbol table, and the emitter) read words
/// through, so a patched word cannot classify one way and emit another.
///
/// `[patches].instructions` was parsed into the typed config and then read by
/// nothing: WM2000's one entry rewrites `func_800004D0`'s idle-loop `j` at
/// `0x800005AC` into the self-branch `0x1000FFFF`, which the emitter turns
/// into `pause_self()`. Unapplied, the emitted body is a tight non-yielding
/// `loop` and cooperative scheduling never resumes another thread — measured
/// on WM2000: 100% of samples inside `func_800004D0`, no VI swap ever.
/// The whole pre-pass `main` runs: cross-check the dump, report every
/// swallowed entry, and repair the ones whose containing function provably
/// returned. Returns the text to print (empty when the dump is clean), so the
/// orchestration itself is testable rather than living inline in `main`.
fn check_and_repair_symbol_dump(cfg: &mut RecompConfig, rom: &[u8]) -> String {
    let check = cross_check_symbol_dump(cfg, rom);
    if check.is_clean() {
        return String::new();
    }
    let mut out = check.render_diagnostic();
    let applied = repair_symbol_dump(cfg, &check);
    out.push_str(&format!(
        "swallowed-entry cross-check: {} proven root(s) examined, {} missing entry/entries, \
         {} repaired by splitting, {} reported only\n",
        check.proven_roots,
        check.swallowed.len(),
        applied,
        check.refused().count(),
    ));
    out
}

/// Cross-check every configured section's function list against the `jal`
/// evidence in the section's own ROM bytes.
///
/// Reuses the exact rule `fn64-discover`'s CFG builder applies to promote a
/// `jal` target to a `proven_root`. `fn64-discover` depends on this crate, so
/// the rule is applied through `fn64_cpu_runtime_codegen::swallowed_entries`
/// rather than by importing the analysis back the other way.
fn cross_check_symbol_dump(cfg: &RecompConfig, rom: &[u8]) -> CrossCheck {
    let mut combined = CrossCheck::default();
    for section in &cfg.sections {
        let Some(words) = read_section_words(rom, section) else {
            continue;
        };
        let region = CodeRegion {
            name: section.name.clone(),
            vram: section.vram,
            words: &words,
        };
        let functions: Vec<DumpFunction> = section
            .functions
            .iter()
            .map(|f| DumpFunction {
                name: f.name.clone(),
                vram: f.vram,
                size: f.size,
            })
            .collect();
        let mut section_check = cross_check_region(&region, &functions);
        combined.proven_roots += section_check.proven_roots;
        combined.swallowed.append(&mut section_check.swallowed);
    }
    combined
}

/// Apply the repairable splits from `check` to `cfg`'s function lists.
///
/// A split shrinks the containing entry's declared `size` to end exactly at
/// the proven root and inserts a new entry covering the remainder. Every
/// downstream stage — `read_func_words`, `SymbolTable`, `LOOKUP_TABLE`, and
/// body emission — derives from this list, so the proven entry becomes
/// dispatchable without any other change. Refused entries are left alone.
fn repair_symbol_dump(cfg: &mut RecompConfig, check: &CrossCheck) -> usize {
    let mut applied = 0usize;
    for section in &mut cfg.sections {
        let mut functions: Vec<DumpFunction> = section
            .functions
            .iter()
            .map(|f| DumpFunction {
                name: f.name.clone(),
                vram: f.vram,
                size: f.size,
            })
            .collect();
        let scoped = CrossCheck {
            swallowed: check
                .swallowed
                .iter()
                .filter(|e| e.region == section.name)
                .cloned()
                .collect(),
            proven_roots: check.proven_roots,
        };
        let count = apply_repairs(&mut functions, &scoped);
        if count == 0 {
            continue;
        }
        applied += count;
        section.functions = functions
            .into_iter()
            .map(|f| Function {
                name: f.name,
                vram: f.vram,
                size: f.size,
            })
            .collect();
    }
    applied
}

/// Read one whole section's big-endian words, or `None` when the section's
/// declared ROM range does not fit the image.
fn read_section_words(rom: &[u8], section: &Section) -> Option<Vec<u32>> {
    let start = section.rom as usize;
    let len = (section.size as usize) & !0x3;
    let end = start.checked_add(len)?;
    if end > rom.len() {
        return None;
    }
    Some(
        rom[start..end]
            .chunks_exact(4)
            .map(|c| u32::from_be_bytes([c[0], c[1], c[2], c[3]]))
            .collect(),
    )
}

fn read_func_words(
    rom: &[u8],
    section: &Section,
    func: &Function,
    patches: &[InstructionPatch],
) -> Option<Vec<u32>> {
    let vram_delta = func.vram.checked_sub(section.vram)?;
    let start = (section.rom as usize).checked_add(vram_delta as usize)?;
    let len = (func.size as usize) & !0x3;
    let end = start.checked_add(len)?;
    if end > rom.len() {
        return None;
    }
    let mut words: Vec<u32> = rom[start..end]
        .chunks_exact(4)
        .map(|c| u32::from_be_bytes([c[0], c[1], c[2], c[3]]))
        .collect();
    for patch in patches.iter().filter(|p| p.func == func.name) {
        let offset = patch
            .vram
            .checked_sub(func.vram)
            .filter(|delta| delta % 4 == 0)
            .map(|delta| delta as usize / 4)
            .filter(|index| *index < words.len())
            .unwrap_or_else(|| {
                panic!(
                    "instruction patch for {} targets vram {:#010x}, which is not a word inside \
                     that function ({:#010x}..{:#010x})",
                    patch.func,
                    patch.vram,
                    func.vram,
                    func.vram + func.size,
                )
            });
        words[offset] = patch.value;
    }
    Some(words)
}

/// Refuse a config whose `[[patches.instruction]]` names a function this run
/// never reads words for — a patch that silently applies to nothing is the
/// same class of defect as the unapplied-patch bug itself.
fn assert_every_instruction_patch_is_reachable(cfg: &RecompConfig) {
    for patch in &cfg.patches.instructions {
        let found = cfg
            .sections
            .iter()
            .flat_map(|section| section.functions.iter())
            .any(|func| func.name == patch.func);
        assert!(
            found,
            "instruction patch names function {:?}, which is in no configured section",
            patch.func
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fn64_recomp::Patches;

    #[test]
    fn absent_profile_flag_discovers_sibling_profile() {
        let config = std::path::Path::new("/games/OOTU/oot.toml");
        let expected = PathBuf::from("/games/OOTU/profile.toml");

        let resolved = resolve_profile_path(config, None, |path| path == expected);

        assert_eq!(resolved, Some(expected));
    }

    #[test]
    fn explicit_profile_overrides_sibling_discovery() {
        let config = std::path::Path::new("/games/OOTU/oot.toml");
        let explicit = PathBuf::from("/profiles/alternate.toml");

        let resolved = resolve_profile_path(config, Some(explicit.clone()), |_| {
            panic!("explicit profile must bypass sibling discovery")
        });

        assert_eq!(resolved, Some(explicit));
    }

    #[test]
    fn modeled_whole_function_cop0_operations_are_not_runtime_traps() {
        for instruction in [
            Instruction::Mfc0 { rt: 2, cop0d: 9 },
            Instruction::Mfc0 { rt: 2, cop0d: 11 },
            Instruction::Mtc0 { rt: 2, cop0d: 9 },
            Instruction::Mtc0 { rt: 2, cop0d: 11 },
            Instruction::Mfc0 { rt: 2, cop0d: 12 },
            Instruction::Mtc0 { rt: 2, cop0d: 12 },
            Instruction::Tlbwi,
            Instruction::Tlbp,
            Instruction::Tlbr,
        ] {
            assert_eq!(trap_kind(&instruction), None);
        }
        assert_eq!(
            trap_kind(&Instruction::Mfc0 { rt: 2, cop0d: 1 }),
            Some("cop0-move")
        );
        assert_eq!(trap_kind(&Instruction::Tlbwr), Some("tlb"));
    }

    #[test]
    fn only_complete_compiler_divide_guard_breaks_are_auto_vetted() {
        let div_zero = [
            Instruction::Bne {
                rs: 11,
                rt: 0,
                off: 2,
            },
            Instruction::Nop,
            Instruction::Break { code: 0x1c00 },
        ];
        assert!(compiler_div_guards_only(&div_zero));

        let overflow = [
            Instruction::Addiu {
                rt: 1,
                rs: 0,
                imm: -1,
            },
            Instruction::Bne {
                rs: 11,
                rt: 1,
                off: 4,
            },
            Instruction::Lui { rt: 1, imm: 0x8000 },
            Instruction::Bne {
                rs: 25,
                rt: 1,
                off: 2,
            },
            Instruction::Nop,
            Instruction::Break { code: 0x1800 },
        ];
        assert!(compiler_div_guards_only(&overflow));
        assert!(!compiler_div_guards_only(&[Instruction::Break { code: 7 }]));
    }

    #[test]
    fn runtime_trap_is_host_bound_for_direct_and_indirect_dispatch() {
        // caller: jal osSendMesg; nop; jr ra; nop
        // osSendMesg: break 0x1c00; jr ra; nop
        let words = [
            0x0C00_0004u32,
            0x0000_0000,
            0x03E0_0008,
            0x0000_0000,
            0x0007_000D,
            0x03E0_0008,
            0x0000_0000,
        ];
        let rom = words
            .into_iter()
            .flat_map(u32::to_be_bytes)
            .collect::<Vec<_>>();
        let cfg = RecompConfig {
            entrypoint: 0x8000_0000,
            rom_file_path: PathBuf::from("synthetic.z64"),
            bss_section_suffix: "_bss".to_string(),
            output_func_path: PathBuf::from("out"),
            trace_mode: false,
            sections: vec![Section {
                name: "boot".to_string(),
                rom: 0,
                vram: 0x8000_0000,
                size: rom.len() as u32,
                functions: vec![
                    Function {
                        name: "caller".to_string(),
                        vram: 0x8000_0000,
                        size: 0x10,
                    },
                    Function {
                        name: "osSendMesg".to_string(),
                        vram: 0x8000_0010,
                        size: 0x0C,
                    },
                ],
            }],
            patches: Patches::default(),
        };

        let report = run(&cfg, &rom, &HashSet::new());
        assert!(report.module.contains("lookup(0x80000010)(ctx, mem);"));
        assert!(!report.module.contains("pub fn osSendMesg"));
        assert!(report.module.contains("(0x80000000, caller as RecompFunc)"));
        assert!(!report
            .module
            .contains("(0x80000010, osSendMesg as RecompFunc)"));
        assert_eq!(report.lookup_sites, 1);
        assert_eq!(report.recompiled_symbols, 1);
        let manifest = report
            .crate_files
            .iter()
            .find(|(path, _)| path == std::path::Path::new("Cargo.toml"))
            .map(|(_, contents)| contents)
            .expect("generated crate manifest");
        assert!(manifest.contains("name = \"oot-recompiled\""));
        assert!(manifest.contains("fn64-cpu-runtime = { path = "));
        assert!(manifest.contains("/crates/fn64-cpu-runtime\" }"));
        assert!(!manifest.contains("fn64-cpu-runtime-codegen\" }"));
        let lib = report
            .crate_files
            .iter()
            .find(|(path, _)| path == std::path::Path::new("src/lib.rs"))
            .map(|(_, contents)| contents)
            .expect("generated crate root");
        assert!(lib.contains("#![forbid(unsafe_code)]"));
        assert!(lib.contains("pub const FN64_FUNCTION_ENTRY_OBSERVATION_SCHEMA"));
        assert!(lib.contains("pub fn lookup(vram: u32) -> RecompFunc"));
        assert!(lib.contains("pub static RECOMPILED_SECTION_GEOMETRY"));
        assert_eq!(
            report
                .crate_files
                .iter()
                .filter(|(path, _)| {
                    path.parent() == Some(std::path::Path::new("src"))
                        && path
                            .file_name()
                            .is_some_and(|name| name.to_string_lossy().starts_with("part_"))
                })
                .count(),
            RECOMPILED_PART_COUNT
        );
        assert!(report.crate_files.iter().any(|(path, contents)| {
            path.parent() == Some(std::path::Path::new("src"))
                && path
                    .file_name()
                    .is_some_and(|name| name.to_string_lossy().starts_with("part_"))
                && contents.contains("pub fn caller")
        }));
        assert_eq!(
            report
                .results
                .iter()
                .filter(|result| result.outcome == Outcome::RuntimeTrap)
                .count(),
            1
        );
    }

    /// Two overlay banks sharing one VRAM window — WM2000's real shape,
    /// reduced. The collided vram must leave the flat table, enter the banked
    /// table with BOTH claimants and their section indices, and be NAMED in
    /// the gap report. The last part is the silent-exclusion fix: before it,
    /// the report said "100% linkable" while two bodies were unreachable.
    #[test]
    fn colliding_banks_are_banked_dispatched_and_named_in_the_report() {
        // Each bank: addiu v0,zero,N; jr ra; nop.
        let bank_a = [0x2402_000Au32, 0x03E0_0008, 0x0000_0000];
        let bank_b = [0x2402_000Bu32, 0x03E0_0008, 0x0000_0000];
        let rom = bank_a
            .into_iter()
            .chain(bank_b)
            .flat_map(u32::to_be_bytes)
            .collect::<Vec<_>>();
        let shared_vram = 0x800E_1B90u32;
        let cfg = RecompConfig {
            entrypoint: shared_vram,
            rom_file_path: PathBuf::from("synthetic.z64"),
            bss_section_suffix: "_bss".to_string(),
            output_func_path: PathBuf::from("out"),
            trace_mode: false,
            sections: vec![
                Section {
                    name: "bank1_text".to_string(),
                    rom: 0,
                    vram: shared_vram,
                    size: 0x0C,
                    functions: vec![Function {
                        name: "func_800E1B90".to_string(),
                        vram: shared_vram,
                        size: 0x0C,
                    }],
                },
                Section {
                    name: "bank4_text".to_string(),
                    rom: 0x0C,
                    vram: shared_vram,
                    size: 0x0C,
                    functions: vec![Function {
                        name: "func_800E1B90_bank4_text".to_string(),
                        vram: shared_vram,
                        size: 0x0C,
                    }],
                },
            ],
            patches: Patches::default(),
        };

        let report = run(&cfg, &rom, &HashSet::new());

        // Both bodies are emitted — the collision is a dispatch problem, not
        // a recompilation one.
        assert!(report.module.contains("pub fn func_800E1B90("));
        assert!(report.module.contains("pub fn func_800E1B90_bank4_text("));
        // Neither is in the flat table: it cannot say which bank is resident.
        assert!(!report
            .module
            .contains("(0x800E1B90, func_800E1B90 as RecompFunc)"));
        assert_eq!(report.recompiled_symbols, 0);
        // Both ARE in the banked table, each tagged with its section index.
        assert!(
            report.module.contains(
                "(0x800E1B90, &[(0, \"func_800E1B90\", func_800E1B90 as RecompFunc), \
                 (1, \"func_800E1B90_bank4_text\", func_800E1B90_bank4_text as RecompFunc), ]),"
            ),
            "banked row missing or malformed:\n{}",
            report.module
        );
        assert_eq!(
            report.banked_claimants,
            vec![(
                shared_vram,
                vec![
                    (0usize, "func_800E1B90".to_string()),
                    (1usize, "func_800E1B90_bank4_text".to_string()),
                ]
            )]
        );

        // The report must NAME the gap, not merely be arithmetically correct.
        let markdown = report.render_markdown(&cfg);
        assert!(markdown.contains("## Bank-ambiguous vrams"));
        assert!(
            markdown.contains("`0x800E1B90`"),
            "gap report must name the ambiguous vram:\n{markdown}"
        );
        assert!(
            markdown.contains("0: `func_800E1B90`")
                && markdown.contains("1: `func_800E1B90_bank4_text`"),
            "gap report must name every claimant and its section:\n{markdown}"
        );
        assert!(markdown.contains("**1 vrams, 2 bodies.**"));
        let summary = report.render_summary();
        assert!(
            summary.contains("bank-ambiguous vrams: 1 (2 bodies)"),
            "summary must surface the gap too:\n{summary}"
        );
    }

    /// The negative half: a config with no shared VRAM window says so
    /// explicitly rather than omitting the section, so "no bank ambiguity"
    /// is a stated measurement and not an absence a reader must infer.
    #[test]
    fn a_config_without_collisions_states_that_explicitly() {
        let words = [0x2402_002Au32, 0x03E0_0008, 0x0000_0000];
        let rom = words
            .into_iter()
            .flat_map(u32::to_be_bytes)
            .collect::<Vec<_>>();
        let cfg = RecompConfig {
            entrypoint: 0x8000_0000,
            rom_file_path: PathBuf::from("synthetic.z64"),
            bss_section_suffix: "_bss".to_string(),
            output_func_path: PathBuf::from("out"),
            trace_mode: false,
            sections: vec![Section {
                name: "code".to_string(),
                rom: 0,
                vram: 0x8000_0000,
                size: rom.len() as u32,
                functions: vec![Function {
                    name: "solo".to_string(),
                    vram: 0x8000_0000,
                    size: rom.len() as u32,
                }],
            }],
            patches: Patches::default(),
        };

        let report = run(&cfg, &rom, &HashSet::new());
        assert!(report.banked_claimants.is_empty());
        assert!(report.module.contains("static BANKED_LOOKUP_TABLE"));
        let markdown = report.render_markdown(&cfg);
        assert!(markdown.contains("## Bank-ambiguous vrams"));
        assert!(markdown.contains("None. Every recompiled vram in this config"));
        assert!(report
            .render_summary()
            .contains("bank-ambiguous vrams: 0 (0 bodies)"));
    }

    #[test]
    fn profile_override_recompiles_a_stubbed_divide_guard() {
        // addiu v0,zero,1; break 7; jr ra; nop. The profile is the evidence
        // that BREAK is a compiler guard in this named function; if reached,
        // the generated panic remains loud.
        let words = [0x2402_0001u32, 0x0000_01CD, 0x03E0_0008, 0x0000_0000];
        let rom = words
            .into_iter()
            .flat_map(u32::to_be_bytes)
            .collect::<Vec<_>>();
        let cfg = RecompConfig {
            entrypoint: 0x8000_0000,
            rom_file_path: PathBuf::from("synthetic.z64"),
            bss_section_suffix: "_bss".to_string(),
            output_func_path: PathBuf::from("out"),
            trace_mode: false,
            sections: vec![Section {
                name: "code".to_string(),
                rom: 0,
                vram: 0x8000_0000,
                size: rom.len() as u32,
                functions: vec![Function {
                    name: "guarded_div".to_string(),
                    vram: 0x8000_0000,
                    size: rom.len() as u32,
                }],
            }],
            patches: Patches {
                stubs: vec!["guarded_div".to_string()],
                ..Patches::default()
            },
        };
        let forced = HashSet::from(["guarded_div".to_string()]);

        let report = run(&cfg, &rom, &forced);

        assert!(report.module.contains("pub fn guarded_div"));
        assert!(report.module.contains("panic!(\"break (code"));
        assert!(report.results[0].outcome == Outcome::Clean);
    }

    /// The idle-spin config shape, reduced to its two words: a backward `j`
    /// that is not a self-branch, plus its delay-slot nop. Unpatched, the
    /// emitter renders an ordinary `pc = ...; continue` — a tight loop that
    /// never yields to the cooperative scheduler. `[[patches.instruction]]`
    /// rewrites the `j` into the self-branch `0x1000FFFF`, which the emitter
    /// special-cases into `pause_self()`.
    ///
    /// WM2000 is the live case: `func_800004D0`'s tail drops to priority 0 and
    /// spins here, and with the patch unapplied 100% of samples sat inside
    /// that function with no VI swap ever committed.
    fn idle_spin_config(patches: Vec<InstructionPatch>) -> (RecompConfig, Vec<u8>) {
        // 0x80000000: nop
        // 0x80000004: nop
        // 0x80000008: j 0x80000000  -- backward, NOT a self-branch
        // 0x8000000C: nop
        // This mirrors WM2000's `L_800005A4: jal ...; j L_800005A4` shape: the
        // jump target is an EARLIER word, so the self-branch rule does not fire
        // and the patch is the only thing that can make this yield.
        let words = [0x0000_0000u32, 0x0000_0000, 0x0800_0000, 0x0000_0000];
        let rom = words
            .into_iter()
            .flat_map(u32::to_be_bytes)
            .collect::<Vec<_>>();
        let cfg = RecompConfig {
            entrypoint: 0x8000_0000,
            rom_file_path: PathBuf::from("synthetic.z64"),
            bss_section_suffix: "_bss".to_string(),
            output_func_path: PathBuf::from("out"),
            trace_mode: false,
            sections: vec![Section {
                name: "code".to_string(),
                rom: 0,
                vram: 0x8000_0000,
                size: rom.len() as u32,
                functions: vec![Function {
                    name: "idle_spin".to_string(),
                    vram: 0x8000_0000,
                    size: rom.len() as u32,
                }],
            }],
            patches: Patches {
                instructions: patches,
                ..Patches::default()
            },
        };
        (cfg, rom)
    }

    #[test]
    fn instruction_patch_rewrites_the_word_the_emitter_reads() {
        let (cfg, rom) = idle_spin_config(vec![InstructionPatch {
            func: "idle_spin".to_string(),
            vram: 0x8000_0008,
            value: 0x1000_FFFF,
        }]);

        let report = run(&cfg, &rom, &HashSet::new());

        assert!(
            report.module.contains("pause_self()"),
            "a self-branch patch must reach the emitter as a cooperative yield; module was:\n{}",
            report.module
        );
    }

    /// The mutation that matters: drop the patch and the same config must
    /// emit the non-yielding loop instead. Without this pairing, a test that
    /// only asserts the patched form would still pass if `read_func_words`
    /// ignored patches and the emitter happened to yield for another reason.
    #[test]
    fn without_the_patch_the_same_config_emits_no_yield() {
        let (cfg, rom) = idle_spin_config(Vec::new());

        let report = run(&cfg, &rom, &HashSet::new());

        assert!(
            !report.module.contains("pause_self()"),
            "unpatched, this body is a plain backward jump and must NOT yield"
        );
    }

    #[test]
    #[should_panic(expected = "is in no configured section")]
    fn instruction_patch_naming_an_absent_function_is_refused() {
        let (cfg, rom) = idle_spin_config(vec![InstructionPatch {
            func: "not_in_any_section".to_string(),
            vram: 0x8000_0000,
            value: 0x1000_FFFF,
        }]);

        let _ = run(&cfg, &rom, &HashSet::new());
    }

    #[test]
    #[should_panic(expected = "not a word inside that function")]
    fn instruction_patch_outside_its_function_is_refused() {
        let (cfg, rom) = idle_spin_config(vec![InstructionPatch {
            func: "idle_spin".to_string(),
            vram: 0x8000_0100,
            value: 0x1000_FFFF,
        }]);

        let _ = run(&cfg, &rom, &HashSet::new());
    }

    /// Hand-built fixture reproducing the WM2000 defect shape:
    ///
    ///   0x80000000 jal 0x80000010   <- proves 0x80000010 is a real entry
    ///   0x80000004 nop              (delay slot)
    ///   0x80000008 jr $ra           <- head RETURNS here
    ///   0x8000000C nop              (delay slot)
    ///   0x80000010 nop              <- swallowed entry
    ///   0x80000014 jr $ra
    ///   0x80000018 nop
    ///
    /// The dump declares ONE function spanning 0x80000000..0x8000001C, so
    /// 0x80000010 is absent from `LOOKUP_TABLE` and `jal 0x80000010` traps at
    /// runtime. This is the exact shape of `func_8012079C_bank3_text`
    /// swallowing `0x80120854` in WWF No Mercy.
    fn swallowed_entry_config() -> (RecompConfig, Vec<u8>) {
        let words = [
            0x0C00_0004u32, // jal 0x80000010
            0x0000_0000,    // nop (delay slot)
            0x03E0_0008,    // jr $ra
            0x0000_0000,    // nop (delay slot)
            0x0000_0000,    // 0x80000010: the swallowed entry
            0x03E0_0008,    // jr $ra
            0x0000_0000,    // nop (delay slot)
        ];
        let rom = words
            .into_iter()
            .flat_map(u32::to_be_bytes)
            .collect::<Vec<_>>();
        let cfg = RecompConfig {
            entrypoint: 0x8000_0000,
            rom_file_path: PathBuf::from("synthetic.z64"),
            bss_section_suffix: "_bss".to_string(),
            output_func_path: PathBuf::from("out"),
            trace_mode: false,
            sections: vec![Section {
                name: "boot".to_string(),
                rom: 0,
                vram: 0x8000_0000,
                size: rom.len() as u32,
                functions: vec![Function {
                    name: "head".to_string(),
                    vram: 0x8000_0000,
                    // Declared size swallows the 0x80000010 entry.
                    size: 0x1C,
                }],
            }],
            patches: Patches::default(),
        };
        (cfg, rom)
    }

    /// Before the cross-check existed, the swallowed entry was simply absent
    /// from the emitted dispatch table and every call to it trapped at
    /// runtime. This pins that the un-repaired config really does omit it,
    /// so the repair test below is not asserting a coincidence.
    #[test]
    fn an_unrepaired_swallowed_entry_is_absent_from_the_lookup_table() {
        let (cfg, rom) = swallowed_entry_config();

        let report = run(&cfg, &rom, &HashSet::new());

        // No LOOKUP_TABLE row exists for it...
        assert!(
            !report.module.contains("(0x80000010, "),
            "the swallowed entry must have no dispatch row before repair"
        );
        // ...yet the `jal` to it is emitted as a `lookup(0x80000010)` call,
        // which is precisely the call that traps at runtime.
        assert!(
            report.module.contains("lookup(0x80000010)"),
            "the call site must route through lookup, proving the trap path"
        );
    }

    #[test]
    fn the_cross_check_names_the_swallowed_entry_and_its_jal_evidence() {
        let (cfg, rom) = swallowed_entry_config();

        let check = cross_check_symbol_dump(&cfg, &rom);

        assert_eq!(check.swallowed.len(), 1, "{:?}", check.swallowed);
        let entry = &check.swallowed[0];
        assert_eq!(entry.vram, 0x8000_0010);
        assert_eq!(entry.containing_name, "head");
        // Derived by hand: the only jal in the fixture is at 0x80000000.
        assert_eq!(entry.jal_sites, vec![0x8000_0000]);
        assert!(entry.is_repairable(), "{:?}", entry.refusal);
        let text = check.render_diagnostic();
        assert!(text.contains("SWALLOWED-FUNCTION-ENTRY"), "{text}");
        assert!(text.contains("0x80000010"), "{text}");
    }

    #[test]
    fn repairing_the_config_puts_the_swallowed_entry_in_the_lookup_table() {
        let (mut cfg, rom) = swallowed_entry_config();
        let check = cross_check_symbol_dump(&cfg, &rom);

        let applied = repair_symbol_dump(&mut cfg, &check);

        assert_eq!(applied, 1);
        // By hand: head shrinks to 0x80000000..0x80000010, tail covers
        // 0x80000010..0x8000001C. The two must tile the original range exactly.
        let shape: Vec<(u32, u32)> = cfg.sections[0]
            .functions
            .iter()
            .map(|f| (f.vram, f.size))
            .collect();
        assert_eq!(shape, vec![(0x8000_0000, 0x10), (0x8000_0010, 0x0C)]);

        let report = run(&cfg, &rom, &HashSet::new());
        assert!(
            report
                .module
                .contains("(0x80000010, func_80000010_split as RecompFunc)"),
            "the repaired entry must reach LOOKUP_TABLE"
        );
        // And the repaired config is itself clean: nothing is swallowed twice.
        assert!(cross_check_symbol_dump(&cfg, &rom).is_clean());
    }

    /// MUTATION GUARD: the same fixture, but the head does NOT return before
    /// the proven root. The entry is still REPORTED, and the config is left
    /// exactly as-is -- a split here would redirect the `jal` into the middle
    /// of a live body.
    ///
    /// This is the case that actually occurs in WM2000's `main_1050` section,
    /// where words inside an embedded data table decode as `jal` and
    /// "prove" roots that are really mid-instruction (e.g. the low half of a
    /// lui/addiu address pair). Dropping the `jr $ra` precondition would
    /// corrupt those functions.
    #[test]
    fn a_swallowed_entry_whose_head_never_returns_is_reported_but_not_split() {
        let (mut cfg, rom) = swallowed_entry_config();
        // Replace the head's `jr $ra` at 0x80000008 with an ordinary
        // `addiu $sp, $sp, -0x20`, so the head is still live at 0x80000010.
        let rom: Vec<u8> = {
            let mut bytes = rom;
            bytes[8..12].copy_from_slice(&0x27BD_FFE0u32.to_be_bytes());
            bytes
        };

        let check = cross_check_symbol_dump(&cfg, &rom);

        assert_eq!(check.swallowed.len(), 1, "still reported");
        assert!(!check.swallowed[0].is_repairable());
        assert_eq!(check.refused().count(), 1);
        assert!(check.render_diagnostic().contains("REFUSED"));

        let before = cfg.sections[0].functions.clone();
        assert_eq!(repair_symbol_dump(&mut cfg, &check), 0);
        assert_eq!(
            cfg.sections[0].functions, before,
            "config must be untouched"
        );
    }

    /// MUTATION GUARD for the orchestration `main` actually runs.
    ///
    /// The individual pieces are tested above, but nothing exercised the
    /// wiring: a build that detected and reported the defect yet silently
    /// skipped the repair would still pass every other test here. This pins
    /// that one call to the pre-pass both reports AND repairs.
    #[test]
    fn the_pre_pass_reports_and_repairs_in_one_call() {
        let (mut cfg, rom) = swallowed_entry_config();

        let diagnostic = check_and_repair_symbol_dump(&mut cfg, &rom);

        assert!(
            diagnostic.contains("SWALLOWED-FUNCTION-ENTRY"),
            "{diagnostic}"
        );
        assert!(diagnostic.contains("0x80000010"), "{diagnostic}");
        assert!(
            diagnostic.contains("1 repaired by splitting"),
            "the pre-pass must actually apply the repair: {diagnostic}"
        );
        assert!(diagnostic.contains("0 reported only"), "{diagnostic}");
        // ...and the config really was mutated, not just described.
        let shape: Vec<(u32, u32)> = cfg.sections[0]
            .functions
            .iter()
            .map(|f| (f.vram, f.size))
            .collect();
        assert_eq!(shape, vec![(0x8000_0000, 0x10), (0x8000_0010, 0x0C)]);
    }

    #[test]
    fn a_clean_dump_makes_the_pre_pass_print_nothing() {
        let (mut cfg, rom) = swallowed_entry_config();
        cfg.sections[0].functions[0].size = 0x10;
        cfg.sections[0].functions.push(Function {
            name: "tail".to_string(),
            vram: 0x8000_0010,
            size: 0x0C,
        });

        assert_eq!(check_and_repair_symbol_dump(&mut cfg, &rom), "");
    }

    /// MUTATION GUARD for per-section repair scoping.
    ///
    /// Two sections each swallow an entry at the SAME vram offset within
    /// their own address window. If repairs were applied without filtering by
    /// section, each section would be handed the other's entry as well, and
    /// the second (non-matching) repair would silently do nothing or split
    /// the wrong function. Both sections must come out correctly split.
    #[test]
    fn repairs_are_scoped_to_the_section_that_owns_them() {
        // Each section holds the identical 7-word shape from
        // `swallowed_entry_config`, but at different vrams. The `jal`
        // immediate is region-relative, so the same word works in both.
        let words = [
            0x0C00_0004u32, // jal <region>+0x10
            0x0000_0000,
            0x03E0_0008, // jr $ra
            0x0000_0000,
            0x0000_0000, // <region>+0x10: swallowed entry
            0x03E0_0008,
            0x0000_0000,
        ];
        let one: Vec<u8> = words.into_iter().flat_map(u32::to_be_bytes).collect();
        let mut rom = one.clone();
        rom.extend_from_slice(&one);
        let section = |name: &str, rom_off: u32, vram: u32| Section {
            name: name.to_string(),
            rom: rom_off,
            vram,
            size: one.len() as u32,
            functions: vec![Function {
                name: format!("head_{name}"),
                vram,
                size: 0x1C,
            }],
        };
        let mut cfg = RecompConfig {
            entrypoint: 0x8000_0000,
            rom_file_path: PathBuf::from("synthetic.z64"),
            bss_section_suffix: "_bss".to_string(),
            output_func_path: PathBuf::from("out"),
            trace_mode: false,
            sections: vec![
                section("alpha", 0, 0x8000_0000),
                // 0x0C000004 in this window resolves to 0x80000010 as well,
                // because `jal`'s target is region-relative -- so give the
                // second section a vram whose 0x0FFFFFFF bits differ, forcing
                // a genuinely different proven root.
                section("beta", one.len() as u32, 0x8000_0100),
            ],
            patches: Patches::default(),
        };
        // Rewrite beta's jal so it targets 0x80000110, its own entry.
        let beta_jal = (3u32 << 26) | (0x8000_0110 >> 2) & 0x03FF_FFFF;
        let base = one.len();
        rom[base..base + 4].copy_from_slice(&beta_jal.to_be_bytes());

        let diagnostic = check_and_repair_symbol_dump(&mut cfg, &rom);

        assert!(
            diagnostic.contains("2 repaired by splitting"),
            "both sections must be repaired: {diagnostic}"
        );
        // By hand: each head shrinks to 0x10 and each tail covers 0xC.
        for (index, base_vram) in [(0usize, 0x8000_0000u32), (1, 0x8000_0100)] {
            let shape: Vec<(u32, u32)> = cfg.sections[index]
                .functions
                .iter()
                .map(|f| (f.vram, f.size))
                .collect();
            assert_eq!(
                shape,
                vec![(base_vram, 0x10), (base_vram + 0x10, 0x0C)],
                "section {index}"
            );
        }
    }

    /// MUTATION GUARD for the per-section repair filter.
    ///
    /// Overlay banks share a VRAM window by design (WM2000's `bank2_text` and
    /// `bank3_text` are both based at 0x8011C900), so two sections can hold
    /// functions with the SAME name at the SAME vram whose bytes differ. Here
    /// only `alpha` swallows an entry; `beta` declares the identical
    /// name/vram but its bytes make the split unsafe, and it is correctly
    /// reported as refused.
    ///
    /// Without the `e.region == section.name` filter, alpha's repairable
    /// entry is offered to beta as well, and beta's function -- which does
    /// NOT return before that point -- gets split anyway, cutting a live body
    /// in half. That is the exact corruption the safety check exists to
    /// prevent, so the filter must not be removed.
    #[test]
    fn a_repair_proven_in_one_bank_is_not_applied_to_its_vram_twin() {
        // alpha: head returns before the entry -> repairable.
        let alpha = [
            0x0C00_0004u32, // jal 0x80000010
            0x0000_0000,
            0x03E0_0008, // jr $ra
            0x0000_0000,
            0x0000_0000, // 0x80000010: swallowed entry
            0x03E0_0008,
            0x0000_0000,
        ];
        // beta: identical layout EXCEPT the head never returns, so a split at
        // 0x80000010 would cut it in half.
        let beta = [
            0x0C00_0004u32, // jal 0x80000010
            0x0000_0000,
            0x27BD_FFE0, // addiu $sp, $sp, -0x20 -- NOT a return
            0x0000_0000,
            0x0000_0000,
            0x03E0_0008,
            0x0000_0000,
        ];
        let mut rom: Vec<u8> = alpha.into_iter().flat_map(u32::to_be_bytes).collect();
        let beta_bytes: Vec<u8> = beta.into_iter().flat_map(u32::to_be_bytes).collect();
        let beta_rom = rom.len() as u32;
        rom.extend_from_slice(&beta_bytes);

        // Both sections base at the SAME vram with the SAME function name,
        // exactly as two overlay banks sharing a window do.
        let bank = |name: &str, rom_off: u32| Section {
            name: name.to_string(),
            rom: rom_off,
            vram: 0x8000_0000,
            size: 0x1C,
            functions: vec![Function {
                name: "shared_name".to_string(),
                vram: 0x8000_0000,
                size: 0x1C,
            }],
        };
        let mut cfg = RecompConfig {
            entrypoint: 0x8000_0000,
            rom_file_path: PathBuf::from("synthetic.z64"),
            bss_section_suffix: "_bss".to_string(),
            output_func_path: PathBuf::from("out"),
            trace_mode: false,
            sections: vec![bank("alpha", 0), bank("beta", beta_rom)],
            patches: Patches::default(),
        };

        let diagnostic = check_and_repair_symbol_dump(&mut cfg, &rom);

        // Exactly one repair: alpha's. Beta's identical-looking entry is
        // reported but refused.
        assert!(
            diagnostic.contains("1 repaired by splitting"),
            "only alpha may be repaired: {diagnostic}"
        );
        assert!(
            diagnostic.contains("1 reported only"),
            "beta must be reported and refused: {diagnostic}"
        );
        // alpha split; beta untouched.
        let alpha_shape: Vec<(u32, u32)> = cfg.sections[0]
            .functions
            .iter()
            .map(|f| (f.vram, f.size))
            .collect();
        assert_eq!(alpha_shape, vec![(0x8000_0000, 0x10), (0x8000_0010, 0x0C)]);
        let beta_shape: Vec<(u32, u32)> = cfg.sections[1]
            .functions
            .iter()
            .map(|f| (f.vram, f.size))
            .collect();
        assert_eq!(
            beta_shape,
            vec![(0x8000_0000, 0x1C)],
            "beta's live body must not be split"
        );
    }

    /// A dump with no swallowed entries must produce an empty diagnostic and
    /// leave the config byte-identical -- the check must not be a source of
    /// churn on healthy configs.
    #[test]
    fn a_healthy_dump_is_clean_and_unchanged() {
        let (cfg, rom) = swallowed_entry_config();
        let mut cfg = cfg;
        // Declare the entry the evidence proves, as a correct dump would.
        cfg.sections[0].functions[0].size = 0x10;
        cfg.sections[0].functions.push(Function {
            name: "tail".to_string(),
            vram: 0x8000_0010,
            size: 0x0C,
        });

        let check = cross_check_symbol_dump(&cfg, &rom);

        assert!(check.is_clean(), "{:?}", check.swallowed);
        assert_eq!(check.render_diagnostic(), "");
        let before = cfg.sections[0].functions.clone();
        assert_eq!(repair_symbol_dump(&mut cfg, &check), 0);
        assert_eq!(cfg.sections[0].functions, before);
    }
}
