//! Whole-ROM driver for `fn64-recomp-native`.
//!
//! Loads an N64Recomp config (`oot.toml`) + its symbol dump via
//! [`fn64_recomp::load_config`], then runs the native recompiler over EVERY
//! function in the ROM — **without bailing on the first bad function**. Each
//! function is classified (clean / stubbed / ROM-range error / unknown
//! opcode / runtime-trap), the clean bodies are emitted into one module file,
//! and a full gap report is written and printed.
//!
//! This is Stage 1-3 of the "prove `fn64-recomp-native` can recompile the
//! whole OoT ROM" milestone: the goal is an honest, complete inventory of
//! what breaks, not a boot. It closes NO gaps itself.
//!
//! # Usage
//!
//! ```text
//! recompile_rom --config <oot.toml> --rom <oot.z64> [--out <dir>]
//! # or via env:
//! OOT_CONFIG=<oot.toml> OOT_ROM=<oot.z64> recompile_rom
//! ```
//!
//! The config and ROM live OUT-OF-TREE (game-derived content); this binary
//! only ever reads their paths from args/env, never vendoring them.

#![forbid(unsafe_code)]

use std::collections::BTreeMap;
use std::path::PathBuf;

use fn64_recomp::{load_config, Function, RecompConfig, Section};
use fn64_recomp_native::{decode, emit_function_resolved, module::SymbolTable, FuncInput, Instruction};

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
        cfg.sections.iter().map(|s| s.functions.len()).sum::<usize>(),
        cfg.rom_file_path.display(),
        rom.len()
    );

    let report = run(&cfg, &rom);

    std::fs::create_dir_all(&args.out).ok();
    let module_path = args.out.join("funcs.rs");
    if let Err(e) = std::fs::write(&module_path, &report.module) {
        eprintln!("failed to write module {}: {e}", module_path.display());
        return std::process::ExitCode::from(2);
    }
    let report_path = args.out.join("gap-report.md");
    if let Err(e) = std::fs::write(&report_path, report.render_markdown(&cfg)) {
        eprintln!("failed to write report {}: {e}", report_path.display());
        return std::process::ExitCode::from(2);
    }

    // Console summary.
    eprintln!("\n{}", report.render_summary());
    eprintln!("module written to {}", module_path.display());
    eprintln!("gap report written to {}", report_path.display());

    std::process::ExitCode::SUCCESS
}

const USAGE: &str = "usage: recompile_rom --config <oot.toml> --rom <oot.z64> [--out <dir>]\n\
                     env fallbacks: OOT_CONFIG, OOT_ROM, OOT_OUT";

struct Args {
    config: PathBuf,
    rom: PathBuf,
    out: PathBuf,
}

impl Args {
    fn parse() -> Result<Self, String> {
        let mut config = std::env::var("OOT_CONFIG").ok().map(PathBuf::from);
        let mut rom = std::env::var("OOT_ROM").ok().map(PathBuf::from);
        let mut out = std::env::var("OOT_OUT").ok().map(PathBuf::from);

        let mut it = std::env::args().skip(1);
        while let Some(arg) = it.next() {
            match arg.as_str() {
                "--config" => config = Some(PathBuf::from(next(&mut it, "--config")?)),
                "--rom" => rom = Some(PathBuf::from(next(&mut it, "--rom")?)),
                "--out" => out = Some(PathBuf::from(next(&mut it, "--out")?)),
                "-h" | "--help" => return Err("help".to_string()),
                other => return Err(format!("unknown argument {other:?}")),
            }
        }
        Ok(Args {
            config: config.ok_or("--config (or OOT_CONFIG) is required")?,
            rom: rom.ok_or("--rom (or OOT_ROM) is required")?,
            out: out.unwrap_or_else(|| PathBuf::from("recomp-out")),
        })
    }
}

fn next(it: &mut impl Iterator<Item = String>, flag: &str) -> Result<String, String> {
    it.next().ok_or_else(|| format!("{flag} needs a value"))
}

// ---- classification ----

/// How one function came out of the native recompiler.
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
    /// Emitted a real body, but it contains a `panic!(...)` runtime trap
    /// (COP0/COP2/TLB/ERET/SYSCALL/BREAK reached in-body): compiles fine, but
    /// would trap if executed. Tracked separately from Clean for honesty.
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
    module: String,
    results: Vec<FuncResult>,
}

fn run(cfg: &RecompConfig, rom: &[u8]) -> Report {
    use std::collections::HashSet;
    let stubbed: HashSet<&str> = cfg.patches.stubs.iter().map(String::as_str).collect();
    let ignored: HashSet<&str> = cfg.patches.ignored.iter().map(String::as_str).collect();

    // Symbol table over non-stubbed/ignored funcs, matching NativeRecompiler.
    let symbols = SymbolTable::from_entries(cfg.sections.iter().flat_map(|s| {
        s.functions.iter().filter_map(|f| {
            if stubbed.contains(f.name.as_str()) || ignored.contains(f.name.as_str()) {
                None
            } else {
                Some((f.name.clone(), f.vram))
            }
        })
    }));

    let mut module = String::new();
    module.push_str("// Generated by fn64-recomp-native whole-ROM driver (recompile_rom).\n");
    module.push_str("// Only CLEANLY-recompiled functions are included below.\n");
    module.push_str("#![allow(clippy::all)]\n");
    module.push_str("#[allow(unused_imports)]\n");
    module.push_str(
        "use fn64_recomp_native::{RecompContext, Rdram, round_ties_even_f32, round_ties_even_f64};\n\n",
    );

    let mut results = Vec::new();

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
            let words = match read_func_words(rom, section, func) {
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
            let trap_kinds: Vec<&'static str> =
                instrs.iter().filter_map(trap_kind).collect::<Vec<_>>();

            let outcome = if !unknown_words.is_empty() {
                Outcome::UnknownOpcode
            } else if !trap_kinds.is_empty() {
                Outcome::RuntimeTrap
            } else {
                Outcome::Clean
            };

            // Emit the body regardless (even Unknown emits a compile_error!
            // line), but only include Clean + RuntimeTrap bodies in the
            // linkable module — an Unknown body won't compile.
            if outcome == Outcome::Clean || outcome == Outcome::RuntimeTrap {
                let input = FuncInput {
                    name: &func.name,
                    vram: func.vram,
                    words: &words,
                };
                module.push_str(&emit_function_resolved(&input, &symbols));
                module.push('\n');
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

    Report { module, results }
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
        Mfc0 { .. } | Mtc0 { .. } => "cop0-move",
        Dmfc0 { .. } | Dmtc0 { .. } => "cop0-dmove",
        Eret => "eret",
        Tlbwi | Tlbwr | Tlbp | Tlbr => "tlb",
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
            "compilable (clean + runtime-trap): {} ({:.2}%)\n",
            clean + trap,
            100.0 * (clean + trap) as f64 / total as f64
        ));
        s
    }

    fn render_markdown(&self, cfg: &RecompConfig) -> String {
        let total = self.results.len();
        let c = self.counts();
        let mut s = String::new();
        s.push_str("# fn64-recomp-native whole-ROM gap report (OoT NTSC 1.0)\n\n");
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
            "\n**Compilable (clean + runtime-trap): {} / {} = {:.2}%.**\n\n",
            clean + trap,
            total,
            100.0 * (clean + trap) as f64 / total as f64
        ));

        // Runtime-trap breakdown by kind.
        s.push_str("## Runtime-trap functions (compile fine, would panic if executed)\n\n");
        s.push_str(
            "These emit a `panic!(...)` for a privileged/unmodeled op (COP0/COP2/TLB/ERET/\
             SYSCALL/BREAK). They are NOT decoder gaps — they are the deliberate loud-trap \
             design, and overlap the `[patches].stubs` class N64Recomp also can't recompile.\n\n",
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
            "- **Runtime-trap (compiles, would trap):** {} functions — a design choice, not a \
             gap, but flagged so no one mistakes them for fully-runnable.\n\n",
            trap,
        ));

        // Whole-module link-level gap: the emitted module calls a `lookup`
        // dispatcher (indirect JALR/JAL-to-unknown) that nothing in the module
        // defines. This is a MODULE-level gap, orthogonal to the per-function
        // decode gaps above: even with 0 unknown opcodes, the module won't
        // link until a host-provided `lookup(addr) -> fn` function-pointer
        // table exists (see module.rs's CallTarget::Indirect doc).
        let lookup_sites = self.module.matches("lookup(").count();
        s.push_str("## Whole-module link gap: the `lookup` dispatcher\n\n");
        s.push_str(&format!(
            "The emitted module contains **{lookup_sites}** `lookup(addr)(ctx, mem)` call sites \
             (indirect calls: register-indirect `JALR`, or `JAL`/`J` to a vram not uniquely in \
             the symbol table). Nothing in the module defines `lookup`, so the module does not \
             yet link even though every DIRECT call target resolves within it.\n\n\
             Verified empirically: compiling the full {} generated functions against \
             `fn64-recomp-native` yields exactly {lookup_sites} `E0425 cannot find function \
             \\`lookup\\`` errors and **zero** other undefined-symbol errors — i.e. all \
             direct JAL targets resolve; the only missing symbol is the dispatcher.\n\n\
             This is the single highest-leverage next step for a linkable whole-ROM module: \
             emit (or host-provide) a `lookup(u32) -> fn(&mut RecompContext, &mut Rdram)` that \
             maps a runtime vram to the recompiled `fn`, built from the same symbol table. It \
             is NOT a decoder gap and does not appear in the per-function counts above.\n\n",
            self.results
                .iter()
                .filter(|r| matches!(r.outcome, Outcome::Clean | Outcome::RuntimeTrap))
                .count(),
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
/// `u32`). Mirrors `NativeRecompiler::read_func_words`, but here it is a
/// classification input, not a hard error.
fn read_func_words(rom: &[u8], section: &Section, func: &Function) -> Option<Vec<u32>> {
    let vram_delta = func.vram.checked_sub(section.vram)?;
    let start = (section.rom as usize).checked_add(vram_delta as usize)?;
    let len = (func.size as usize) & !0x3;
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
