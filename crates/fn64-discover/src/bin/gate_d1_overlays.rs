//! Held-out D1 re-grade for NWXE with ROM-only recovered overlay mappings.
//!
//! Discovery reads the ROM first and runs both the boot-only baseline and the
//! descriptor-family recovery. The dump is opened only after both fact
//! databases are complete, so answer-key geometry cannot enter either run.

use fn64_discover::banks::{self, BankNamePattern};
use fn64_discover::delta_vote::DeltaVoteConfig;
use fn64_discover::grade_candidates::{grade_candidates, parse_symbol_dump, PrecisionRecall};
use fn64_discover::overlay_regions::SearchConfig;
use fn64_discover::{
    required_env_path, run_discovery, run_discovery_with_recovered_overlay_regions, Fact,
    RecoveredOverlayInput,
};
use std::collections::BTreeSet;

const NWXE_FUNCTIONS: usize = 2_442;
const NWXE_SECTIONS: usize = 6;
const PRECISION_RED_FLAG_FLOOR: f64 = 0.45;

fn main() {
    if let Err(error) = run() {
        eprintln!("gate_d1_overlays: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let rom_path = required_env_path("FN64_DISCOVER_NWXE_ROM", "the NWXE .z64")?;
    let dump_path =
        required_env_path("FN64_DISCOVER_NWXE_DUMP", "the NWXE grading-only dump.toml")?;
    let rom_bytes =
        std::fs::read(&rom_path).map_err(|error| format!("reading {rom_path}: {error}"))?;

    eprintln!("gate_d1_overlays: running boot-only discovery");
    let (baseline_rom, baseline_db) =
        run_discovery(&rom_bytes, None).map_err(|error| error.to_string())?;
    let search = SearchConfig::aki_family();
    let min_mapped_regions = search.min_records;
    let input = RecoveredOverlayInput {
        search,
        delta_vote: DeltaVoteConfig::default(),
        // Require at least as many uniquely mapped records as the family
        // search requires to recognize any table. This is derived from the
        // search configuration, not an expected NWXE record count.
        min_mapped_regions,
        table_name: "recovered_overlay_descriptors".to_string(),
        bank_name: BankNamePattern::new("recovered_overlay_", 0, ""),
    };
    eprintln!("gate_d1_overlays: running recovered-overlay discovery");
    let (recovered_rom, recovered_db, recovery) =
        run_discovery_with_recovered_overlay_regions(&rom_bytes, &input)
            .map_err(|error| error.to_string())?;
    if recovered_rom.sha256 != baseline_rom.sha256 {
        return Err("baseline and recovered-overlay runs normalized to different ROMs".into());
    }

    let admitted_tables = recovery
        .admissions
        .iter()
        .filter(|admission| admission.admitted)
        .count();
    let recovered_banks: BTreeSet<_> = recovered_db
        .proven_rom_mappings()
        .into_iter()
        .filter_map(|fact| match fact {
            Fact::RomMapping { bank, .. } if bank != banks::BOOT_BANK => Some(bank.as_str()),
            _ => None,
        })
        .collect();

    // Held-out boundary: neither discovery run above can observe this text.
    let key_text = std::fs::read_to_string(&dump_path)
        .map_err(|error| format!("reading {dump_path}: {error}"))?;
    eprintln!("gate_d1_overlays: grading completed discovery results");
    let key = parse_symbol_dump(&key_text)?;
    if key.function_count != NWXE_FUNCTIONS || key.section_count != NWXE_SECTIONS {
        return Err(format!(
            "expected {NWXE_SECTIONS} sections / {NWXE_FUNCTIONS} functions, got {} / {}",
            key.section_count, key.function_count
        ));
    }

    let before = grade_candidates(&baseline_db, &key);
    let after = grade_candidates(&recovered_db, &key);

    println!("gate_d1_overlays: NWXE held-out mechanical-overlay re-grade");
    println!("ROM SHA-256 {}", recovered_rom.sha256);
    println!(
        "recovery: raw_tables={} admitted_tables={} proven_overlay_banks={}",
        recovery.candidate_tables.len(),
        admitted_tables,
        recovered_banks.len()
    );
    for admission in recovery
        .admissions
        .iter()
        .filter(|admission| admission.admitted)
    {
        println!(
            "  admitted table @ROM 0x{:x}: records={} delta_mapped={}",
            admission.table.table_rom_offset,
            admission.table.records.len(),
            admission.mapped_regions
        );
    }
    print_metrics("boot-only", &before.combined, before.combined_ungradable);
    print_metrics(
        "mechanically-recovered-overlays",
        &after.combined,
        after.combined_ungradable,
    );
    println!(
        "movement: precision={:+.6} points recall={:+.6} points candidates={:+} recalled_functions={:+}",
        (after.combined.precision() - before.combined.precision()) * 100.0,
        (after.combined.recall() - before.combined.recall()) * 100.0,
        after.combined.candidates as i64 - before.combined.candidates as i64,
        after.combined.recalled_functions as i64 - before.combined.recalled_functions as i64,
    );

    let precision_held = after.combined.precision() >= before.combined.precision()
        && after.combined.precision() >= PRECISION_RED_FLAG_FLOOR;
    println!(
        "precision guard: {} (after >= boot-only and after >= {:.0}% red-flag floor)",
        if precision_held { "HELD" } else { "FAILED" },
        PRECISION_RED_FLAG_FLOOR * 100.0
    );
    if !precision_held {
        return Err("mapping recovered overlays collapsed held-out precision".into());
    }
    Ok(())
}

fn print_metrics(label: &str, metrics: &PrecisionRecall, ungradable: usize) {
    println!(
        "{label}: candidates={} tp_entries={} recalled_functions={} fp={} fn={} precision={:.6}% recall={:.6}% ungradable={}",
        metrics.candidates,
        metrics.true_positives,
        metrics.recalled_functions,
        metrics.false_positives,
        metrics.false_negatives,
        metrics.precision() * 100.0,
        metrics.recall() * 100.0,
        ungradable,
    );
}
