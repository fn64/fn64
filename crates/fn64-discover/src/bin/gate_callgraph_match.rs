//! Grading-only cross-ROM call-graph propagation gate.
//!
//! Body-hash seeding matches functions whose relocation-masked whole body is
//! unique on both sides (the [`homology`](fn64_discover::homology) baseline).
//! Call-graph propagation then admits further correspondences MECHANICALLY —
//! a match extends along a proven direct-call edge only when the mapping is
//! unique at a shared structural call index (see
//! [`callgraph_match`](fn64_discover::callgraph_match)).
//!
//! The two AKI titles (WWF No Mercy = NW4E, WWF WrestleMania 2000 = NWXE)
//! share an engine, so many resident functions are byte-identical after
//! relocation masking. This gate grades, HELD-OUT, whether each *propagated*
//! pair is the same function. The held-out oracle is independent of the
//! propagation mechanism: a pair is correct iff the two bodies are
//! relocation-masked-identical, or the two functions carry the same real
//! (non-`func_ADDR`) symbol name in their dumps. Propagation keys only on
//! call-graph structure, never on that oracle, so the oracle is a fair judge.
//!
//! Inputs are named and declared, never defaulted (DESIGN.md section 1.0):
//!   FN64_DISCOVER_NW4E_ROM    path to a WWF No Mercy (U) v1.1 .z64
//!   FN64_DISCOVER_NW4E_DUMP   NW4E syms/dump.toml
//!   FN64_DISCOVER_NWXE_ROM    path to a WWF WrestleMania 2000 (U) .z64
//!   FN64_DISCOVER_NWXE_DUMP   NWXE syms/dump.toml

use fn64_discover::callgraph_match::{
    match_programs, FunctionBody, MatchSource, MatchedPair, Program,
};
use fn64_discover::homology::relocation_masked_word;
use fn64_discover::{normalize, required_env_path, NormalizedRom};
use serde::Deserialize;
use std::collections::BTreeMap;
use std::path::Path;

/// Fixed-point cap. Propagation is monotone; this only guards against a
/// pathological input, and is part of the gate's reproducible parameters.
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

fn main() {
    if let Err(error) = run() {
        eprintln!("gate_callgraph_match: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let nw4e_rom = required_env_path("FN64_DISCOVER_NW4E_ROM", "the NW4E .z64")?;
    let nw4e_dump = required_env_path("FN64_DISCOVER_NW4E_DUMP", "the NW4E syms/dump.toml")?;
    let nwxe_rom = required_env_path("FN64_DISCOVER_NWXE_ROM", "the NWXE .z64")?;
    let nwxe_dump = required_env_path("FN64_DISCOVER_NWXE_DUMP", "the NWXE syms/dump.toml")?;

    let left = load_side("NW4E", &nw4e_rom, &nw4e_dump)?;
    let right = load_side("NWXE", &nwxe_rom, &nwxe_dump)?;

    let left_program = Program::new(left.functions.clone())
        .map_err(|error| format!("NW4E call graph rejected: {error}"))?;
    let right_program = Program::new(right.functions.clone())
        .map_err(|error| format!("NWXE call graph rejected: {error}"))?;

    let report = match_programs(&left_program, &right_program, MAX_ROUNDS);

    // Held-out grade of the PROPAGATED pairs only. Seeds are the body-hash
    // baseline and are not what this gate is measuring.
    let left_meta = metadata(&left);
    let right_meta = metadata(&right);
    let mut propagated = 0usize;
    let mut correct = 0usize;
    let mut wrong = 0usize;
    let mut wrong_examples: Vec<String> = Vec::new();
    for pair in &report.pairs {
        if pair.source != MatchSource::CallGraph {
            continue;
        }
        propagated += 1;
        if pair_is_correct(pair, &left_meta, &right_meta) {
            correct += 1;
        } else {
            wrong += 1;
            if wrong_examples.len() < 8 {
                wrong_examples.push(format!(
                    "{}@{:#010x} -> {}@{:#010x} (round {})",
                    pair.left_identity,
                    pair.left_va,
                    pair.right_identity,
                    pair.right_va,
                    pair.round
                ));
            }
        }
    }

    let precision = if propagated == 0 {
        0.0
    } else {
        correct as f64 / propagated as f64
    };

    // Also grade the seeds (the body-hash baseline) so the report shows the
    // precision propagation must not fall below, and report the masked-body
    // overlap ceiling — the number of NW4E functions with any NWXE body twin,
    // the theoretical maximum a body-corroborated matcher can reach.
    let mut seed_correct = 0usize;
    for pair in &report.pairs {
        if pair.source == MatchSource::BodyHash && pair_is_correct(pair, &left_meta, &right_meta) {
            seed_correct += 1;
        }
    }
    let right_hashes: std::collections::BTreeSet<u64> =
        right_meta.hash_by_va.values().copied().collect();
    let overlap_ceiling = left_meta
        .hash_by_va
        .values()
        .filter(|h| right_hashes.contains(h))
        .count();

    // Stable, digest-friendly report. Every printed number is a deterministic
    // function of the two ROMs' bytes and dumps.
    println!("gate_callgraph_match: NW4E <-> NWXE call-graph propagation");
    println!(
        "  corpus nw4e_functions={} nwxe_functions={}",
        left_program.len(),
        right_program.len()
    );
    println!(
        "  seeds body_hash_unique={} seed_correct={} rounds={}",
        report.seed_count, seed_correct, report.rounds
    );
    println!(
        "  propagated call_graph={} total_matched={} body_overlap_ceiling={}",
        report.propagated_count,
        report.seed_count + report.propagated_count,
        overlap_ceiling
    );
    println!(
        "  frontier nw4e_unmatched={} nwxe_unmatched={}",
        report.left_unmatched, report.right_unmatched
    );
    println!(
        "  held_out propagated_graded={} correct={} wrong={} precision={:.4}%",
        propagated,
        correct,
        wrong,
        precision * 100.0
    );
    for example in &wrong_examples {
        println!("  wrong_example {example}");
    }

    // A propagated WRONG match is the failure this gate exists to catch. The
    // uniqueness rule is meant to keep propagation precision at the pairwise
    // body-hash baseline (~98%+). Fail loudly if it regresses, so the rule
    // gets reverted rather than a false "done" recorded.
    if propagated > 0 && precision < 0.98 {
        return Err(format!(
            "propagation precision {:.4}% is below the 98% floor ({} wrong of {} graded) — \
             the uniqueness rule regressed and must be reverted",
            precision * 100.0,
            wrong,
            propagated
        ));
    }

    Ok(())
}

/// One side's loaded functions plus the per-function metadata the held-out
/// oracle needs (masked-body hash and real symbol name, if any).
struct Side {
    functions: Vec<FunctionBody>,
    names: Vec<String>,
}

struct Meta {
    /// entry VA -> relocation-masked whole-body hash.
    hash_by_va: BTreeMap<u32, u64>,
    /// entry VA -> real (non-`func_ADDR`) symbol name, if the dump names it.
    name_by_va: BTreeMap<u32, Option<String>>,
}

fn metadata(side: &Side) -> Meta {
    let mut hash_by_va = BTreeMap::new();
    let mut name_by_va = BTreeMap::new();
    for (function, name) in side.functions.iter().zip(&side.names) {
        hash_by_va.insert(function.va_start, masked_hash(&function.words));
        name_by_va.insert(function.va_start, real_name(name));
    }
    Meta {
        hash_by_va,
        name_by_va,
    }
}

/// A propagated pair is CORRECT iff independent evidence says the two
/// functions are the same one: either their relocation-masked bodies are
/// identical (proven same body — this is the pairwise-homology ground truth),
/// or both carry the same real symbol name.
fn pair_is_correct(pair: &MatchedPair, left: &Meta, right: &Meta) -> bool {
    let (Some(&lh), Some(&rh)) = (
        left.hash_by_va.get(&pair.left_va),
        right.hash_by_va.get(&pair.right_va),
    ) else {
        return false;
    };
    if lh == rh {
        return true;
    }
    match (
        left.name_by_va.get(&pair.left_va).and_then(|n| n.as_ref()),
        right
            .name_by_va
            .get(&pair.right_va)
            .and_then(|n| n.as_ref()),
    ) {
        (Some(ln), Some(rn)) => ln == rn,
        _ => false,
    }
}

/// A `func_ADDR` autoname is address-derived and differs between the two games
/// even for the same function, so it carries no correspondence signal; only a
/// hand-named symbol (e.g. `osSetIntMask`) does.
fn real_name(name: &str) -> Option<String> {
    if name.starts_with("func_") {
        None
    } else {
        Some(name.to_string())
    }
}

fn masked_hash(words: &[u32]) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for &word in words {
        hash ^= u64::from(relocation_masked_word(word));
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash ^= words.len() as u64;
    hash.wrapping_mul(0x0000_0100_0000_01b3)
}

fn load_side(label: &str, rom_path: &str, dump_path: &str) -> Result<Side, String> {
    for path in [rom_path, dump_path] {
        if !Path::new(path).exists() {
            return Err(format!("{label} input {path} does not exist"));
        }
    }
    let bytes = std::fs::read(rom_path).map_err(|error| format!("reading {rom_path}: {error}"))?;
    let rom = normalize(&bytes).map_err(|error| format!("{label} ROM: {error}"))?;
    let doc = parse_dump(dump_path)?;
    functions_from_dump(label, &rom, &doc)
}

fn parse_dump(path: &str) -> Result<SymbolsDoc, String> {
    let text = std::fs::read_to_string(path).map_err(|error| format!("reading {path}: {error}"))?;
    toml::from_str(&text).map_err(|error| format!("parsing {path}: {error}"))
}

/// Turn one dump into function bodies. Only resident-image functions (those
/// whose section ROM range lies in the physical ROM) are used; a function with
/// a non-word size or an out-of-ROM extent is a loud error, not a silent skip,
/// because the dump is a fixed input and a malformed one means the wrong file.
/// Functions shorter than two words carry no `jal` and cannot anchor the call
/// graph; they are kept so the entry index is complete (a call landing on one
/// still registers), but they simply never seed or propagate on their own.
fn functions_from_dump(label: &str, rom: &NormalizedRom, doc: &SymbolsDoc) -> Result<Side, String> {
    let mut functions = Vec::new();
    let mut names = Vec::new();
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
        // resident-relative VROM; skip them rather than read garbage. The
        // resident image is the shared engine surface this gate grades.
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
            // A dump can list the same vram twice across aliased sections;
            // the call graph requires unique entries, so keep the first.
            if !seen.insert(function.vram) {
                continue;
            }
            functions.push(FunctionBody {
                identity: format!("{}:{}", section.name, function.name),
                va_start: function.vram,
                words: words_from_be(bytes),
            });
            names.push(function.name.clone());
        }
    }
    Ok(Side { functions, names })
}

fn words_from_be(bytes: &[u8]) -> Vec<u32> {
    bytes
        .chunks_exact(4)
        .map(|word| u32::from_be_bytes(word.try_into().expect("four-byte chunk")))
        .collect()
}
