//! Grading-only gate for the content-consumer discriminator
//! ([`fn64_discover::content_consumer`], the Ramblr concept: classify an
//! undecided word by who consumes it rather than by CFG reachability
//! alone).
//!
//! This gate runs the real discovery pipeline's boot-bank CFG (entrypoint
//! seed only, Phase 6's bounded HI/LO closure via
//! `resolve::build_cfg_closed_with_facts` -- same construction `gate_b2`
//! uses, never reimplemented here) over OoT and, when present, NWXE, then
//! runs the consumer discriminator over every word the CFG left open
//! (`Unknown`/`CandidateData`/`CandidateCode`) and reports pointer/code/
//! ambiguous counts.
//!
//! # Held-out grading (never a pipeline input)
//!
//! The bundled answer-key CSVs (`testdata/oot_boot_functions.csv`,
//! `testdata/nwxe_boot_functions.csv` -- the same mechanically-extracted
//! keys `grade_oot_functions`/`grade_nwxe_functions` use) are opened only
//! AFTER classification, purely to label each classified word as
//! "inside a known function extent" or "outside every known extent." A
//! `Code` classification for a word outside every function extent, or a
//! `Pointer` classification for a word strictly inside one, is a WRONG
//! candidate; the gate reports the wrong rate honestly rather than hiding
//! it -- see `CONSUMER_EVIDENCE.md` for the resulting keep/drop call. This
//! is candidate-quality reporting, not a hard pass/fail gate: a nonzero
//! wrong rate does not fail the process, an unset ROM env var does (loud
//! skip, never silent).
//!
//! ROM paths: FN64_DISCOVER_OOT_ROM (required), FN64_DISCOVER_NWXE_ROM
//! (optional -- printed as skipped if absent, matching gate_b2's posture).

use fn64_discover::banks;
use fn64_discover::content_consumer::{classify_consumers, CandidateContentClass};
use fn64_discover::grade_nwxe_functions::{self, parse_function_csv as parse_nwxe_csv};
use fn64_discover::grade_oot_functions::parse_function_csv as parse_oot_csv;
use fn64_discover::resolve::build_cfg_closed_with_facts;
use fn64_discover::{run_discovery, Fact};
use std::path::Path;

const OOT_BOOT_CODE_END: u32 = 0x8000_6230;
const OOT_BOOT_FUNCTIONS_CSV: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/testdata/oot_boot_functions.csv"
);
const NWXE_BOOT_FUNCTIONS_CSV: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/testdata/nwxe_boot_functions.csv"
);

/// One [start, end) extent this ROM's answer key calls a real function.
struct FunctionExtent {
    start: u32,
    end: u32,
}

/// Held-out grading counts for one ROM's classified word set.
#[derive(Default)]
struct GradeCounts {
    pointer_total: usize,
    code_total: usize,
    ambiguous_total: usize,
    pointer_agree: usize,
    pointer_wrong: usize,
    code_agree: usize,
    code_wrong: usize,
    unlabeled: usize,
}

fn main() {
    let mut exit_code = 0;
    println!("=== fn64-discover content-consumer discriminator gate ===\n");
    println!("(candidate-quality report, not a hard pass/fail bar -- see CONSUMER_EVIDENCE.md)\n");

    match run_oot() {
        Ok(()) => println!(),
        Err(error) => {
            eprintln!("OoT content-consumer gate FAILED: {error}\n");
            exit_code = 1;
        }
    }

    match run_nwxe() {
        Ok(()) => println!(),
        Err(error) => {
            eprintln!("NWXE content-consumer run FAILED: {error}\n");
            exit_code = 1;
        }
    }

    std::process::exit(exit_code);
}

fn run_oot() -> Result<(), String> {
    let oot_rom =
        fn64_discover::required_env_path("FN64_DISCOVER_OOT_ROM", "an OoT NTSC 1.0 .z64")?;
    let rom_bytes = std::fs::read(&oot_rom).map_err(|e| format!("reading {oot_rom}: {e}"))?;
    let (rom, db) =
        run_discovery(&rom_bytes, None).map_err(|e| format!("normalizing OoT ROM: {e}"))?;
    println!("OoT ROM: {} bytes, sha256={}", rom.len(), rom.sha256);

    let (rom_start, va_start) = boot_rom_start_va(&db)?;
    let code_len = (OOT_BOOT_CODE_END - va_start) as usize;
    let bank_bytes = rom
        .bytes
        .get(rom_start as usize..rom_start as usize + code_len)
        .ok_or("OoT boot code interval falls outside normalized ROM")?;
    let entrypoint = rom.header.entry_point;

    let (cfg, _resolved) =
        build_cfg_closed_with_facts(&db, "boot", bank_bytes, va_start, &[entrypoint]);
    let report = classify_consumers(&cfg, bank_bytes, va_start);
    print_report(&report);

    let csv = std::fs::read_to_string(OOT_BOOT_FUNCTIONS_CSV)
        .map_err(|e| format!("reading {OOT_BOOT_FUNCTIONS_CSV}: {e}"))?;
    let parsed = parse_oot_csv(&csv);
    if !parsed.skipped.is_empty() {
        println!(
            "  (skipped {} malformed OoT answer-key rows)",
            parsed.skipped.len()
        );
    }
    // OoT's key is a contiguous linker-map layout: each row's extent runs to
    // the next row's start, and the last row runs to the boot code region's
    // proven end -- exactly the convention grade_oot_functions.rs documents
    // and already relies on, reused here rather than reinvented.
    let mut extents = Vec::with_capacity(parsed.functions.len());
    for window in parsed.functions.windows(2) {
        extents.push(FunctionExtent {
            start: window[0].va_start,
            end: window[1].va_start,
        });
    }
    if let Some(last) = parsed.functions.last() {
        extents.push(FunctionExtent {
            start: last.va_start,
            end: OOT_BOOT_CODE_END,
        });
    }

    let counts = grade_held_out(&report, &extents);
    print_grade("OoT", &counts);
    Ok(())
}

fn run_nwxe() -> Result<(), String> {
    let nwxe_rom = match fn64_discover::required_env_path("FN64_DISCOVER_NWXE_ROM", "the NWXE .z64")
    {
        Ok(path) if Path::new(&path).exists() => path,
        Ok(path) => {
            println!("NWXE content-consumer run SKIPPED: {path} not found");
            return Ok(());
        }
        Err(error) => {
            println!("NWXE content-consumer run SKIPPED: {error}");
            return Ok(());
        }
    };
    let rom_bytes = std::fs::read(&nwxe_rom).map_err(|e| format!("reading {nwxe_rom}: {e}"))?;
    let (rom, db) =
        run_discovery(&rom_bytes, None).map_err(|e| format!("normalizing NWXE ROM: {e}"))?;
    println!("NWXE ROM: {} bytes, sha256={}", rom.len(), rom.sha256);

    let (rom_start, rom_end, va_start) = boot_rom_start_end_va(&db)?;
    let bank_bytes = rom
        .bytes
        .get(rom_start as usize..rom_end as usize)
        .ok_or("NWXE boot-copy interval falls outside normalized ROM")?;
    let entrypoint = rom.header.entry_point;

    let (cfg, _resolved) =
        build_cfg_closed_with_facts(&db, "boot", bank_bytes, va_start, &[entrypoint]);
    let report = classify_consumers(&cfg, bank_bytes, va_start);
    print_report(&report);

    let csv = std::fs::read_to_string(NWXE_BOOT_FUNCTIONS_CSV)
        .map_err(|e| format!("reading {NWXE_BOOT_FUNCTIONS_CSV}: {e}"))?;
    let parsed = parse_nwxe_csv(&csv);
    if !parsed.skipped.is_empty() {
        println!(
            "  (skipped {} malformed NWXE answer-key rows)",
            parsed.skipped.len()
        );
    }
    let extents: Vec<FunctionExtent> = parsed
        .functions
        .iter()
        .map(|f: &grade_nwxe_functions::AnswerFunction| FunctionExtent {
            start: f.va_start,
            end: f.va_end(),
        })
        .collect();

    let counts = grade_held_out(&report, &extents);
    print_grade("NWXE", &counts);
    Ok(())
}

fn boot_rom_start_va(db: &fn64_discover::FactDb) -> Result<(u32, u32), String> {
    let mapping = db
        .proven_rom_mappings()
        .into_iter()
        .find(|f| matches!(f, Fact::RomMapping { bank, .. } if bank == banks::BOOT_BANK))
        .ok_or("boot bank not proven")?;
    match mapping {
        Fact::RomMapping {
            rom_start,
            va_start,
            ..
        } => Ok((*rom_start, *va_start)),
        _ => unreachable!(),
    }
}

fn boot_rom_start_end_va(db: &fn64_discover::FactDb) -> Result<(u32, u32, u32), String> {
    let mapping = db
        .proven_rom_mappings()
        .into_iter()
        .find(|f| matches!(f, Fact::RomMapping { bank, .. } if bank == banks::BOOT_BANK))
        .ok_or("boot bank not proven")?;
    match mapping {
        Fact::RomMapping {
            rom_start,
            rom_end,
            va_start,
            ..
        } => Ok((*rom_start, *rom_end, *va_start)),
        _ => unreachable!(),
    }
}

fn print_report(report: &fn64_discover::content_consumer::ConsumerReport) {
    println!(
        "  consumer discriminator: pointer={} code={} ambiguous={} (of {} previously-open words)",
        report.pointer_count,
        report.code_count,
        report.ambiguous_count,
        report.classifications.len()
    );
    if std::env::var("FN64_DISCOVER_CONSUMER_VERBOSE").is_ok() {
        for c in &report.classifications {
            println!(
                "    0x{:08x} prior={:?} class={:?} evidence={:?}",
                c.va, c.prior_class, c.class, c.evidence
            );
        }
    }
}

/// Label every non-ambiguous classification against the held-out answer-key
/// extents: `Code` agrees when its address falls inside a known function
/// extent; `Pointer` agrees when it falls outside every extent. A word this
/// ROM's key simply never documents (neither inside nor a clean "known gap")
/// is counted as `unlabeled`, never silently folded into either agree/wrong
/// bucket.
fn grade_held_out(
    report: &fn64_discover::content_consumer::ConsumerReport,
    extents: &[FunctionExtent],
) -> GradeCounts {
    let mut counts = GradeCounts::default();
    let inside_known_function = |va: u32| extents.iter().any(|e| va >= e.start && va < e.end);
    for classification in &report.classifications {
        match classification.class {
            CandidateContentClass::Pointer => {
                counts.pointer_total += 1;
                if inside_known_function(classification.va) {
                    counts.pointer_wrong += 1;
                } else {
                    counts.pointer_agree += 1;
                }
            }
            CandidateContentClass::Code => {
                counts.code_total += 1;
                if inside_known_function(classification.va) {
                    counts.code_agree += 1;
                } else {
                    counts.code_wrong += 1;
                }
            }
            CandidateContentClass::Ambiguous => {
                counts.ambiguous_total += 1;
                counts.unlabeled += 1;
            }
        }
    }
    counts
}

fn print_grade(label: &str, counts: &GradeCounts) {
    let scored = counts.pointer_total + counts.code_total;
    let agree = counts.pointer_agree + counts.code_agree;
    let wrong = counts.pointer_wrong + counts.code_wrong;
    let agreement_pct = if scored == 0 {
        0.0
    } else {
        100.0 * agree as f64 / scored as f64
    };
    println!(
        "  {label} held-out: scored={scored} agree={agree} wrong={wrong} agreement={agreement_pct:.1}% (pointer: agree={} wrong={}; code: agree={} wrong={}; ambiguous(not scored)={})",
        counts.pointer_agree,
        counts.pointer_wrong,
        counts.code_agree,
        counts.code_wrong,
        counts.ambiguous_total,
    );
    if wrong > 0 {
        println!(
            "  {label}: {wrong}/{scored} non-ambiguous candidates disagree with the answer key -- see CONSUMER_EVIDENCE.md for the honest keep/drop assessment"
        );
    }
}
