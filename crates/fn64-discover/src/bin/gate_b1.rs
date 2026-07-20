//! B1 gate runner: runs Phase 1+2 discovery deterministically against the
//! real OoT and NW4E ROMs, grades OoT bank discovery against the decomp's
//! segment answer key, cross-checks NW4E against its hand-verified
//! `overlays.json`, and prints the exact counts the task's gate requires.
//!
//! Never committed to reading these paths speculatively -- every input is
//! a fixed, explicitly-named file the task specified. This binary is the
//! auditable "did the gate actually pass on real bytes" artifact; the
//! unit tests in each module are the fast, synthetic-data correctness
//! proof for the mechanism itself.

use fn64_discover::banks::DescriptorTableShape;
use fn64_discover::grade_nw4e::{cross_check_nw4e, parse_overlays_json};
use fn64_discover::grade_oot::{grade_against_oot, parse_segments_csv};
use fn64_discover::{banks, run_discovery, DescriptorTableInput};
use std::path::Path;

fn required(variable: &str, what: &str) -> Result<String, String> {
    fn64_discover::required_env_path(variable, what)
}

/// NW4E's overlay descriptor table, per `games/NW4E/overlays.json`'s own
/// provenance comment: five fixed 9-word x 0x24-byte records at ROM
/// 0x0539a0, fields `[rom_start, rom_end, dma_dest_load_va, ...]` at word
/// offsets 0, 1, 2. This shape is an explicit, cited external claim (prior
/// byte-verified RE), supplied here as input -- never rediscovered by
/// scanning, per this crate's "no guessed table location" discipline.
fn nw4e_descriptor_table_shape() -> DescriptorTableShape {
    fn64_discover::aki_reference::NW4E_DESCRIPTOR_TABLE
}

fn main() {
    let mut exit_code = 0;

    println!("=== fn64-discover B1 gate ===\n");

    match run_oot_gate() {
        Ok(found_frac) => {
            println!(
                "OoT bank-discovery grade: {:.1}% of applicable segments found\n",
                found_frac * 100.0
            );
        }
        Err(e) => {
            eprintln!("OoT gate FAILED: {e}\n");
            exit_code = 1;
        }
    }

    match run_nw4e_gate() {
        Ok(()) => println!(),
        Err(e) => {
            eprintln!("NW4E gate FAILED: {e}\n");
            exit_code = 1;
        }
    }

    match run_determinism_gate() {
        Ok(()) => println!("Determinism gate: PASS (byte-identical re-run for both ROMs)\n"),
        Err(e) => {
            eprintln!("Determinism gate FAILED: {e}\n");
            exit_code = 1;
        }
    }

    std::process::exit(exit_code);
}

fn run_oot_gate() -> Result<f64, String> {
    let oot_rom = required("FN64_DISCOVER_OOT_ROM", "an OoT NTSC 1.0 .z64")?;
    let rom_bytes = std::fs::read(&oot_rom).map_err(|e| format!("reading {oot_rom}: {e}"))?;
    let (rom, db) =
        run_discovery(&rom_bytes, None).map_err(|e| format!("normalizing OoT ROM: {e}"))?;

    println!("OoT ROM: {} bytes, sha256={}", rom.len(), rom.sha256);
    println!(
        "  header entry_point=0x{:08x} name={:?}",
        rom.header.entry_point, rom.header.name
    );

    let segments_csv = required(
        "FN64_DISCOVER_OOT_SEGMENTS_CSV",
        "the OoT decomp baserom segments.csv answer key",
    )?;
    let csv = std::fs::read_to_string(&segments_csv)
        .map_err(|e| format!("reading {segments_csv}: {e}"))?;
    let answer_key = parse_segments_csv(&csv);
    if !answer_key.skipped.is_empty() {
        println!(
            "  (skipped {} malformed answer-key rows)",
            answer_key.skipped.len()
        );
    }

    let report = grade_against_oot(&db, &answer_key.segments);
    println!(
        "  applicable segments: {}  found: {}  missed: {}  ambiguous: {}  wrong: {}",
        report.total_applicable, report.found, report.missed, report.ambiguous, report.wrong
    );
    if report.wrong != 0 {
        return Err(format!("gate requires wrong=0, got {}", report.wrong));
    }

    Ok(report.found_fraction())
}

fn run_nw4e_gate() -> Result<(), String> {
    let nw4e_rom = required("FN64_DISCOVER_NW4E_ROM", "the NW4E .z64")?;
    if !Path::new(&nw4e_rom).exists() {
        return Err(format!("{nw4e_rom} not found"));
    }
    let rom_bytes = std::fs::read(&nw4e_rom).map_err(|e| format!("reading {nw4e_rom}: {e}"))?;
    let shape = nw4e_descriptor_table_shape();
    let (rom, db) = run_discovery(&rom_bytes, Some((shape, |i| format!("R{}", i + 1))))
        .map_err(|e| format!("normalizing NW4E ROM: {e}"))?;

    println!("NW4E ROM: {} bytes, sha256={}", rom.len(), rom.sha256);
    println!(
        "  header entry_point=0x{:08x} name={:?}",
        rom.header.entry_point, rom.header.name
    );
    println!(
        "  boot bank: {:?}",
        db.conclusion(&format!("bank:{}", banks::BOOT_BANK))
            .map(|c| c.state)
    );

    let overlays_json = required("FN64_DISCOVER_NW4E_OVERLAYS", "the NW4E overlays.json")?;
    let json = std::fs::read_to_string(&overlays_json)
        .map_err(|e| format!("reading {overlays_json}: {e}"))?;
    let answer_banks = parse_overlays_json(&json).map_err(|e| e.to_string())?;

    let report = cross_check_nw4e(&db, &answer_banks);
    println!(
        "  overlay banks: {}  exact_match: {}  va_mismatch: {}  missed: {}",
        report.total, report.exact_matches, report.va_mismatches, report.missed
    );
    for (name, grade) in &report.per_bank {
        println!("    {name}: {grade:?}");
    }

    Ok(())
}

fn run_determinism_gate() -> Result<(), String> {
    let oot_rom = required("FN64_DISCOVER_OOT_ROM", "an OoT NTSC 1.0 .z64")?;
    let nw4e_rom = required("FN64_DISCOVER_NW4E_ROM", "the NW4E .z64")?;
    for (label, path, descriptor) in [
        ("OoT", oot_rom.as_str(), None::<DescriptorTableInput>),
        (
            "NW4E",
            nw4e_rom.as_str(),
            Some((
                nw4e_descriptor_table_shape(),
                (|i: u32| format!("R{}", i + 1)) as fn(u32) -> String,
            )),
        ),
    ] {
        if !Path::new(path).exists() {
            println!("  ({label} not present, skipping determinism check for it)");
            continue;
        }
        let bytes = std::fs::read(path).map_err(|e| format!("reading {path}: {e}"))?;
        let (_rom_a, db_a) = run_discovery(&bytes, descriptor)
            .map_err(|e| format!("{label} run A normalize: {e}"))?;
        let (_rom_b, db_b) = run_discovery(&bytes, descriptor)
            .map_err(|e| format!("{label} run B normalize: {e}"))?;
        let json_a = serde_json::to_string(&db_a).map_err(|e| e.to_string())?;
        let json_b = serde_json::to_string(&db_b).map_err(|e| e.to_string())?;
        if json_a != json_b {
            return Err(format!(
                "{label}: repeated run produced different FactDb JSON"
            ));
        }
        println!(
            "  {label}: byte-identical across two runs ({} bytes of fact-db JSON)",
            json_a.len()
        );
    }
    Ok(())
}
