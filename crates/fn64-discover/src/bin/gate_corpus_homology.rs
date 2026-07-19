//! Grading-only corpus-scale N-ROM homology gate.
//!
//! The pairwise engines ([`homology`](fn64_discover::homology) body hashes,
//! [`callgraph_match`](fn64_discover::callgraph_match) call-graph propagation)
//! decide each cross-ROM edge. [`corpus_homology`](fn64_discover::corpus_homology)
//! runs them over every ROM pair and closes the edges transitively into
//! function identities, under a per-ROM-uniqueness + body-corroboration guard
//! that collapses a conflicting component to ambiguous rather than guessing.
//!
//! This gate assembles the corpus, then grades HELD-OUT: for every admitted
//! identity that spans two-or-more DUMP-BEARING ROMs, do the dump's real
//! (hand-named, non-`func_ADDR`) symbol names agree across the identity's
//! members? The closure keys only on relocation-masked bodies and call-graph
//! structure, never on those names, so the names are a fair independent judge.
//! A single identity whose members carry two different real names is the
//! cross-ROM cascade this whole line of work exists to prevent — it fails the
//! gate loudly, so a regression gets reverted rather than a false "done"
//! recorded.
//!
//! The corpus is 6 ROMs across 3+ engine families. Three carry dumps and are
//! graded (NW4E, NWXE, OoT); three are ungraded contributors (GoldenEye,
//! Perfect Dark, SM64) — they still consume and contribute identities, but no
//! answer key grades them.
//!
//! Inputs are named and declared, never defaulted (DESIGN.md section 1.0). A
//! ROM that is unset is a loud, deterministic skip line — the corpus shrinks to
//! whatever is present, so the recorded digest is fixed only with the full
//! six-ROM set:
//!   FN64_DISCOVER_NW4E_ROM / _DUMP   WWF No Mercy (U) v1.1 + NW4E dump.toml
//!   FN64_DISCOVER_NWXE_ROM / _DUMP   WWF WrestleMania 2000 (U) + NWXE dump.toml
//!   FN64_DISCOVER_OOT_ROM  / _DUMP   OoT NTSC 1.0 + OOTU dump.toml
//!   FN64_DISCOVER_GE_ROM             GoldenEye 007 (U)     [ungraded]
//!   FN64_DISCOVER_PD_ROM             Perfect Dark (U)      [ungraded]
//!   FN64_DISCOVER_SM64_ROM           Super Mario 64 (U)    [ungraded]

use fn64_discover::callgraph_match::FunctionBody;
use fn64_discover::corpus_homology::{build_corpus, CorpusConfig, CorpusIdentity, CorpusRom};
use fn64_discover::{normalize, NormalizedRom};
use serde::Deserialize;
use std::collections::BTreeMap;
use std::path::Path;

/// Fixed-point cap per pair. Part of the gate's reproducible parameters.
const MAX_ROUNDS: u32 = 64;

#[derive(Debug, Deserialize)]
struct SymbolsDoc {
    #[serde(default)]
    section: Vec<SectionDoc>,
}

#[derive(Debug, Deserialize)]
struct SectionDoc {
    name: String,
    rom: u32,
    vram: u32,
    size: u32,
    #[serde(default)]
    functions: Vec<FunctionDoc>,
}

#[derive(Debug, Deserialize)]
struct FunctionDoc {
    name: String,
    vram: u32,
    size: u32,
}

/// One corpus ROM as loaded: the matcher input plus the held-out oracle (real
/// symbol name per entry VA, when the ROM carries a dump).
struct LoadedRom {
    label: String,
    functions: Vec<FunctionBody>,
    /// entry VA -> real (non-`func_ADDR`) symbol name, if the dump names it.
    /// Empty for an ungraded (dump-less) ROM.
    real_name_by_va: BTreeMap<u32, String>,
    graded: bool,
}

/// One corpus ROM's declaration: env vars and the human label.
struct RomSpec {
    label: &'static str,
    rom_var: &'static str,
    dump_var: &'static str,
}

const SPECS: &[RomSpec] = &[
    RomSpec {
        label: "NW4E",
        rom_var: "FN64_DISCOVER_NW4E_ROM",
        dump_var: "FN64_DISCOVER_NW4E_DUMP",
    },
    RomSpec {
        label: "NWXE",
        rom_var: "FN64_DISCOVER_NWXE_ROM",
        dump_var: "FN64_DISCOVER_NWXE_DUMP",
    },
    RomSpec {
        label: "OoT",
        rom_var: "FN64_DISCOVER_OOT_ROM",
        dump_var: "FN64_DISCOVER_OOT_DUMP",
    },
    RomSpec {
        label: "GE",
        rom_var: "FN64_DISCOVER_GE_ROM",
        dump_var: "FN64_DISCOVER_GE_DUMP",
    },
    RomSpec {
        label: "PD",
        rom_var: "FN64_DISCOVER_PD_ROM",
        dump_var: "FN64_DISCOVER_PD_DUMP",
    },
    RomSpec {
        label: "SM64",
        rom_var: "FN64_DISCOVER_SM64_ROM",
        dump_var: "FN64_DISCOVER_SM64_DUMP",
    },
];

fn main() {
    if let Err(error) = run() {
        eprintln!("gate_corpus_homology: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    println!("gate_corpus_homology: N-ROM corpus identity graph");

    let mut loaded: Vec<LoadedRom> = Vec::new();
    for spec in SPECS {
        let Ok(rom_path) = std::env::var(spec.rom_var) else {
            println!(
                "  rom {label} SKIPPED ({var} unset)",
                label = spec.label,
                var = spec.rom_var
            );
            continue;
        };
        // A corpus member needs prior, independently-derived function
        // boundaries. Where a dump provides them, the ROM joins and is graded
        // held-out against its real symbol names. A ROM with no dump has no
        // boundary source this gate is allowed to invent (running the discovery
        // pipeline for boundaries is out of this gate's scope), so it is a loud,
        // deterministic skip — a documented frontier, never a silent guess. The
        // ungraded families (GE/PD/SM64) join the moment a boundary source is
        // supplied via their _DUMP var.
        let Ok(dump_path) = std::env::var(spec.dump_var) else {
            println!(
                "  rom {label} SKIPPED (no {var}; contributes once a boundary source exists)",
                label = spec.label,
                var = spec.dump_var
            );
            continue;
        };
        let rom = load_rom(spec.label, &rom_path, &dump_path)?;
        println!(
            "  rom {label} functions={fns} graded={graded} named={named}",
            label = rom.label,
            fns = rom.functions.len(),
            graded = rom.graded,
            named = rom.real_name_by_va.len(),
        );
        loaded.push(rom);
    }

    if loaded.len() < 2 {
        return Err(format!(
            "corpus needs at least two ROMs; {} present (set the FN64_DISCOVER_*_ROM vars)",
            loaded.len()
        ));
    }

    let corpus: Vec<CorpusRom> = loaded
        .iter()
        .map(|r| CorpusRom {
            label: r.label.clone(),
            functions: r.functions.clone(),
        })
        .collect();

    let report = build_corpus(
        &corpus,
        CorpusConfig {
            max_rounds: MAX_ROUNDS,
        },
    )
    .map_err(|error| format!("corpus build rejected: {error}"))?;

    // Held-out grade: an identity spanning >=2 graded ROMs is checked against
    // the dumps' real symbol names. It is CORRECT iff every graded member that
    // carries a real name carries the SAME real name (an autoname carries no
    // signal, so a member the dump left as func_ADDR neither confirms nor
    // refutes). It is WRONG iff two graded members carry DIFFERENT real names.
    let name_by_rom: Vec<&BTreeMap<u32, String>> =
        loaded.iter().map(|r| &r.real_name_by_va).collect();
    let graded_flags: Vec<bool> = loaded.iter().map(|r| r.graded).collect();

    let mut graded_identities = 0usize;
    let mut correct = 0usize;
    let mut wrong = 0usize;
    let mut wrong_examples: Vec<String> = Vec::new();
    for identity in &report.identities {
        match grade_identity(identity, &name_by_rom, &graded_flags) {
            IdentityGrade::Ungraded => {}
            IdentityGrade::Correct => {
                graded_identities += 1;
                correct += 1;
            }
            IdentityGrade::Wrong(example) => {
                graded_identities += 1;
                wrong += 1;
                if wrong_examples.len() < 8 {
                    wrong_examples.push(example);
                }
            }
        }
    }

    let precision = if graded_identities == 0 {
        0.0
    } else {
        correct as f64 / graded_identities as f64
    };

    // Span histogram: how many identities span exactly k ROMs. A big tail
    // (k near the ROM count) is the superlinear libultra/SDK payoff.
    let mut span_hist: BTreeMap<usize, usize> = BTreeMap::new();
    for identity in &report.identities {
        *span_hist.entry(identity.span()).or_default() += 1;
    }

    // The single widest identity, resolved to its members, so the report shows
    // WHAT spans the corpus (a libultra routine, ideally), not just a number.
    let widest = report.identities.iter().max_by_key(|identity| {
        (
            identity.span(),
            std::cmp::Reverse(first_member_label(identity)),
        )
    });

    println!(
        "  corpus roms={} total_functions={} pairwise_edges={}",
        loaded.len(),
        report.total_functions,
        report.pairwise_edges
    );
    println!(
        "  identities count={} max_span={} singletons={} ambiguous={}",
        report.identity_count,
        report.max_span,
        report.singletons,
        report.ambiguous.len()
    );
    for (span, count) in &span_hist {
        println!("  span_histogram span={span} identities={count}");
    }
    if let Some(identity) = widest {
        let members: Vec<String> = identity
            .members
            .iter()
            .map(|m| format!("{}:{}", m.rom_label, m.identity))
            .collect();
        println!(
            "  widest_identity span={} members={}",
            identity.span(),
            members.join(",")
        );
    }
    println!(
        "  held_out graded_identities={graded_identities} correct={correct} wrong={wrong} precision={:.4}%",
        precision * 100.0
    );
    for example in &wrong_examples {
        println!("  wrong_example {example}");
    }

    // A wrong cross-ROM identity is THE failure. The corpus must hold the
    // pairwise ~98% floor; anything less means the transitive-uniqueness guard
    // regressed and let a false identity cascade.
    if graded_identities > 0 && precision < 0.98 {
        return Err(format!(
            "corpus precision {:.4}% is below the 98% floor ({} wrong of {} graded identities) — \
             the transitive-uniqueness guard regressed and must be reverted",
            precision * 100.0,
            wrong,
            graded_identities
        ));
    }

    Ok(())
}

enum IdentityGrade {
    /// Fewer than two graded members carry a real name — no held-out signal.
    Ungraded,
    Correct,
    Wrong(String),
}

/// Grade one identity against the dumps' real names. Only graded ROMs with a
/// real (non-autoname) symbol contribute; two distinct real names among them is
/// a wrong cross-ROM identity.
fn grade_identity(
    identity: &CorpusIdentity,
    name_by_rom: &[&BTreeMap<u32, String>],
    graded: &[bool],
) -> IdentityGrade {
    let mut names: Vec<(&str, &str)> = Vec::new();
    for member in &identity.members {
        if !graded[member.rom] {
            continue;
        }
        if let Some(name) = name_by_rom[member.rom].get(&member.va_start) {
            names.push((member.rom_label.as_str(), name.as_str()));
        }
    }
    if names.len() < 2 {
        return IdentityGrade::Ungraded;
    }
    let first = names[0].1;
    if names.iter().all(|(_, name)| *name == first) {
        IdentityGrade::Correct
    } else {
        let rendered: Vec<String> = names
            .iter()
            .map(|(label, name)| format!("{label}:{name}"))
            .collect();
        IdentityGrade::Wrong(rendered.join(" != "))
    }
}

fn first_member_label(identity: &CorpusIdentity) -> String {
    identity
        .members
        .first()
        .map(|m| m.rom_label.clone())
        .unwrap_or_default()
}

fn load_rom(label: &str, rom_path: &str, dump_path: &str) -> Result<LoadedRom, String> {
    if !Path::new(rom_path).exists() {
        return Err(format!("{label} ROM {rom_path} does not exist"));
    }
    if !Path::new(dump_path).exists() {
        return Err(format!("{label} dump {dump_path} does not exist"));
    }
    let bytes = std::fs::read(rom_path).map_err(|error| format!("reading {rom_path}: {error}"))?;
    let rom = normalize(&bytes).map_err(|error| format!("{label} ROM: {error}"))?;
    let doc = parse_dump(dump_path)?;
    let (functions, real_name_by_va) = functions_from_dump(label, &rom, &doc)?;
    Ok(LoadedRom {
        label: label.to_string(),
        functions,
        real_name_by_va,
        graded: true,
    })
}

fn parse_dump(path: &str) -> Result<SymbolsDoc, String> {
    let text = std::fs::read_to_string(path).map_err(|error| format!("reading {path}: {error}"))?;
    toml::from_str(&text).map_err(|error| format!("parsing {path}: {error}"))
}

/// Turn one dump into function bodies plus the real-name oracle. Only
/// resident-image functions (section ROM range inside the physical ROM) are
/// used, matching gate_callgraph_match: the resident image is the shared engine
/// surface. A malformed dump is a loud error, not a silent skip.
fn functions_from_dump(
    label: &str,
    rom: &NormalizedRom,
    doc: &SymbolsDoc,
) -> Result<(Vec<FunctionBody>, BTreeMap<u32, String>), String> {
    let mut functions = Vec::new();
    let mut real_name_by_va = BTreeMap::new();
    let mut seen = std::collections::BTreeSet::new();
    for section in &doc.section {
        if !section.size.is_multiple_of(4) || !section.rom.is_multiple_of(4) {
            return Err(format!(
                "{label} section {:?} is not word-aligned",
                section.name
            ));
        }
        let section_end = section
            .rom
            .checked_add(section.size)
            .ok_or_else(|| format!("{label} section {:?} extent overflows", section.name))?;
        // Overlay sections whose ROM range is outside the physical image are
        // resident-relative VROM; skip them rather than read garbage.
        if section_end as usize > rom.len() {
            continue;
        }
        for function in &section.functions {
            if !function.size.is_multiple_of(4) {
                return Err(format!(
                    "{label} function {:?} has non-word size {:#x}",
                    function.name, function.size
                ));
            }
            if function.size == 0 {
                continue;
            }
            let section_offset = function.vram.checked_sub(section.vram).ok_or_else(|| {
                format!(
                    "{label} function {:?} precedes section {:?}",
                    function.name, section.name
                )
            })?;
            let function_end = section_offset
                .checked_add(function.size)
                .ok_or_else(|| format!("{label} function {:?} extent overflows", function.name))?;
            if function_end > section.size {
                return Err(format!(
                    "{label} function {:?} exceeds section {:?}",
                    function.name, section.name
                ));
            }
            let rom_start = section.rom.checked_add(section_offset).ok_or_else(|| {
                format!("{label} function {:?} ROM offset overflows", function.name)
            })?;
            let bytes = rom
                .bytes
                .get(rom_start as usize..(rom_start + function.size) as usize)
                .ok_or_else(|| format!("{label} function {:?} exceeds ROM", function.name))?;
            // A dump can list the same vram twice across aliased sections; the
            // call graph requires unique entries, so keep the first.
            if !seen.insert(function.vram) {
                continue;
            }
            if let Some(name) = real_name(&function.name) {
                real_name_by_va.insert(function.vram, name);
            }
            functions.push(FunctionBody {
                identity: format!("{}:{}", section.name, function.name),
                va_start: function.vram,
                words: words_from_be(bytes),
            });
        }
    }
    Ok((functions, real_name_by_va))
}

/// A `func_ADDR` autoname is address-derived and differs between games even for
/// the same function, so it carries no correspondence signal. Only a hand-named
/// symbol (e.g. `osSetIntMask`) is a held-out oracle.
fn real_name(name: &str) -> Option<String> {
    if name.starts_with("func_") {
        None
    } else {
        Some(name.to_string())
    }
}

fn words_from_be(bytes: &[u8]) -> Vec<u32> {
    bytes
        .chunks_exact(4)
        .map(|word| u32::from_be_bytes(word.try_into().expect("four-byte chunk")))
        .collect()
}
