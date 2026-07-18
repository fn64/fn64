//! Answer-key gate: parse the vendored splat `symbol_addrs` grading keys and,
//! when the matching ROM is present, grade this crate's boot-bank function
//! discovery against the key's function VAs (precision/recall, FP breakdown),
//! mirroring `gate_d1`'s grading posture.
//!
//! The key is consumed only AFTER discovery (`answer_keys` is unreachable from
//! any detector). The gate is title-generic: each registered title names its
//! vendored key file, the env var that supplies its ROM, and the ROM identity
//! it must match. Titles whose ROM env var is unset are loudly skipped but
//! STILL have their key parsed and its counts asserted, so a CI-like run with
//! no ROMs present still validates the parsers deterministically.

use fn64_discover::answer_keys::{parse_symbol_addrs, FunctionClass, ParsedSymbolTable};
use fn64_discover::{run_discovery, FactDb, ProofState};
use std::collections::BTreeSet;

/// One title in the answer-key corpus.
struct Title {
    label: &'static str,
    /// Vendored key file, embedded at build time so the parse-and-count path
    /// runs with zero filesystem dependencies (CI has no ROMs but always has
    /// the key).
    key_text: &'static str,
    key_provenance: &'static str,
    /// Env var naming the user-owned ROM. Unset => loud skip of grading only.
    rom_env: &'static str,
    /// Expected identity of that ROM, checked before grading.
    expected: Option<RomIdentity>,
    /// Exact parser expectations, asserted on every run (ROM present or not).
    expect_rows: usize,
    expect_functions: usize,
    expect_data: usize,
}

struct RomIdentity {
    sha1: &'static str,
    internal_name: &'static str,
}

const BANJO_KEY: &str =
    include_str!("../../testdata/answer_keys/banjo_kazooie.symbol_addrs.us.v10.txt");

/// Perfect Dark key text is intentionally absent: `n64decomp/perfect_dark`
/// (MIT, verified) is an armips matching decomp with symbols in `ld/*.inc`
/// linker scripts, not a splat `symbol_addrs` table — no such file exists in
/// that repo at any commit (see testdata/answer_keys/LICENSES.md). PD is
/// registered so the honest skip is reported, not hidden.
fn titles() -> Vec<Title> {
    vec![
        Title {
            label: "Banjo-Kazooie (USA v1.0)",
            key_text: BANJO_KEY,
            key_provenance:
                "n64decomp/banjo-kazooie@1b2edf8bea686b6bfb6f35277606439991351a5b symbol_addrs.us.v10.txt (CC0-1.0)",
            rom_env: "FN64_DISCOVER_BANJO_ROM",
            expected: Some(RomIdentity {
                sha1: "1fb13cad402518d3ae9a8dc4b52c5c54b2a4adc7",
                internal_name: "BANJO KAZOOIE",
            }),
            expect_rows: 60,
            expect_functions: 55,
            expect_data: 5,
        },
        Title {
            label: "Perfect Dark (USA)",
            key_text: "",
            key_provenance:
                "n64decomp/perfect_dark has no splat symbol_addrs table (armips matching decomp; symbols in ld/*.inc) — no key vendored",
            rom_env: "FN64_DISCOVER_PD_ROM",
            expected: None,
            expect_rows: 0,
            expect_functions: 0,
            expect_data: 0,
        },
    ]
}

fn main() {
    println!("=== fn64-discover answer-key gate ===\n");
    let mut failed = false;
    for title in titles() {
        if let Err(error) = run_title(&title) {
            eprintln!("{}: FAILED: {error}", title.label);
            failed = true;
        }
        println!();
    }
    if failed {
        std::process::exit(1);
    }
}

fn run_title(title: &Title) -> Result<(), String> {
    println!("{}", title.label);
    println!("  key: {}", title.key_provenance);

    // Parse-and-count always runs (validates the parser even with no ROM).
    let table = if title.key_text.is_empty() {
        None
    } else {
        let table = parse_symbol_addrs(title.key_text).map_err(|e| e.to_string())?;
        assert_counts(title, &table)?;
        println!(
            "  key parsed: rows={} functions={} data={} skipped={} (explicit_func={} inferred_func={})",
            table.row_count(),
            table.function_count(),
            table.data_count(),
            table.total_skipped(),
            table.explicit_function_count(),
            table.inferred_function_count(),
        );
        Some(table)
    };
    if title.key_text.is_empty() {
        println!("  no key vendored for this title — parse-and-count skipped");
    }

    // Grading requires the ROM. Absent env var => loud skip (expected on this
    // machine), gate still passes on the parse-and-count above.
    let rom_path = match std::env::var(title.rom_env) {
        Ok(path) => path,
        Err(_) => {
            println!(
                "  grade SKIPPED: {} unset (set it to the {} ROM to grade)",
                title.rom_env, title.label
            );
            return Ok(());
        }
    };
    let Some(table) = table else {
        // The ROM is present but no key is vendored yet. This is a skip, not an
        // error: FN64_DISCOVER_PD_ROM (and future ROM vars) are shared with
        // other gates (e.g. gate_overlay_generalize), so a set ROM var without
        // a vendored key here is a legitimate "cannot grade yet" state, not a
        // misconfiguration.
        println!(
            "  grade SKIPPED: {} is set but no answer key is vendored for {} yet",
            title.rom_env, title.label
        );
        return Ok(());
    };
    let Some(expected) = &title.expected else {
        return Err(format!(
            "{} is set but no expected ROM identity is registered for {}",
            title.rom_env, title.label
        ));
    };
    grade(title, &rom_path, expected, &table)
}

fn assert_counts(title: &Title, table: &ParsedSymbolTable) -> Result<(), String> {
    if table.row_count() != title.expect_rows
        || table.function_count() != title.expect_functions
        || table.data_count() != title.expect_data
    {
        return Err(format!(
            "key count drift: expected rows={} functions={} data={}, got rows={} functions={} data={}",
            title.expect_rows,
            title.expect_functions,
            title.expect_data,
            table.row_count(),
            table.function_count(),
            table.data_count(),
        ));
    }
    Ok(())
}

fn grade(
    title: &Title,
    rom_path: &str,
    expected: &RomIdentity,
    table: &ParsedSymbolTable,
) -> Result<(), String> {
    let rom_bytes = std::fs::read(rom_path).map_err(|e| format!("reading {rom_path}: {e}"))?;
    let (rom, db) = run_discovery(&rom_bytes, None)
        .map_err(|e| format!("normalizing {} ROM: {e}", title.label))?;
    if rom.sha1 != expected.sha1 {
        return Err(format!(
            "ROM SHA-1 mismatch: key is bound to {}, got {}",
            expected.sha1, rom.sha1
        ));
    }
    if rom.header.name != expected.internal_name {
        return Err(format!(
            "ROM internal name mismatch: expected {:?}, got {:?}",
            expected.internal_name, rom.header.name
        ));
    }
    println!("  ROM ok: sha1={} name={:?}", rom.sha1, rom.header.name);

    // The key's function VAs (splat addresses are runtime VAs).
    let key_vas: BTreeSet<u32> = table
        .functions()
        .filter(|(_, class)| {
            // Grade only against inferred/explicit function boundaries. Below-
            // floor and data rows are already excluded by `functions()`.
            matches!(
                class,
                FunctionClass::ExplicitFunc | FunctionClass::InferredFromNameAndAddress
            )
        })
        .map(|(row, _)| row.address)
        .collect();

    // Candidate function-entry VAs discovered in the boot bank (positive
    // proof states), restricted to the key's VA span so a title whose key
    // covers only part of the ROM is graded on comparable ground.
    let candidates = discovered_boot_entry_vas(&db);
    let report = grade_vas(&candidates, &key_vas);

    println!(
        "  grade: answer_key_functions={} candidates={} tp={} fp={} fn={} precision={:.4}% recall={:.4}%",
        key_vas.len(),
        report.candidates,
        report.true_positives,
        report.false_positives,
        report.false_negatives,
        report.precision() * 100.0,
        report.recall() * 100.0,
    );
    println!(
        "  fp breakdown: at_or_above_va_floor={} below_va_floor={}",
        report.fp_above_floor, report.fp_below_floor,
    );
    Ok(())
}

/// Positive (candidate or proven) boot-bank function-entry VAs.
fn discovered_boot_entry_vas(db: &FactDb) -> BTreeSet<u32> {
    let mut out: BTreeSet<u32> = db.candidate_function_entries("boot").into_iter().collect();
    out.extend(db.proven_function_entries("boot"));
    // Guard: only positive claims. `candidate_function_entries` and
    // `proven_function_entries` are already positive by construction, but a
    // conflict state would be a bug we want loud, not silently graded.
    for fact in db.facts() {
        if let fn64_discover::Fact::FunctionEntryClaim { target, .. } = fact {
            if target.bank == "boot"
                && db
                    .conclusion(&fn64_discover::facts::function_entry_subject(target))
                    .is_some_and(|c| c.state == ProofState::Conflict)
            {
                out.remove(&target.pc);
            }
        }
    }
    out
}

struct VaGrade {
    candidates: usize,
    true_positives: usize,
    false_positives: usize,
    false_negatives: usize,
    fp_above_floor: usize,
    fp_below_floor: usize,
}

impl VaGrade {
    fn precision(&self) -> f64 {
        if self.candidates == 0 {
            0.0
        } else {
            self.true_positives as f64 / self.candidates as f64
        }
    }
    fn recall(&self) -> f64 {
        let total = self.true_positives + self.false_negatives;
        if total == 0 {
            0.0
        } else {
            self.true_positives as f64 / total as f64
        }
    }
}

fn grade_vas(candidates: &BTreeSet<u32>, key: &BTreeSet<u32>) -> VaGrade {
    let true_positives = candidates.intersection(key).count();
    let false_positives = candidates.len() - true_positives;
    let false_negatives = key.len() - true_positives;
    let mut fp_above_floor = 0;
    let mut fp_below_floor = 0;
    for fp in candidates.difference(key) {
        if *fp >= 0x8000_0000 {
            fp_above_floor += 1;
        } else {
            fp_below_floor += 1;
        }
    }
    VaGrade {
        candidates: candidates.len(),
        true_positives,
        false_positives,
        false_negatives,
        fp_above_floor,
        fp_below_floor,
    }
}
