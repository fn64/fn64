//! D1 gate: grade Phase 3 function-entry candidates against answer keys that
//! are consumed only after discovery. OoT's zeldaret-derived key is the
//! required 13,358-function measurement; NW4E/NWXE use the same harness when
//! their local ROM/key files are present.

use fn64_discover::banks::DescriptorTableShape;
use fn64_discover::grade_candidates::{grade_candidates, parse_symbol_dump, DetectorGrade};
use fn64_discover::{run_discovery, DescriptorTableInput};
use std::path::Path;

const OOT_ROM: &str = "/Users/jer/Downloads/Legend of Zelda, The - Ocarina of Time (USA).z64";
const OOT_DUMP: &str = "/Users/jer/Code/aki-recomp/games/OOTU/syms/dump.toml";
const OOT_SHA1: &str = "ad69c91157f6705e8ab06c79fe08aad47bb57ba7";
const OOT_FUNCTIONS: usize = 13_358;
const OOT_SECTIONS: usize = 472;

const NW4E_ROM: &str = "/Users/jer/Code/aki-recomp/games/NW4E/nomercy.z64";
const NW4E_DUMP: &str = "/Users/jer/Code/aki-recomp/games/NW4E/syms/dump.toml";
const NWXE_ROM: &str = "/Users/jer/Code/aki-recomp/games/NWXE/wm2000.z64";
const NWXE_DUMP: &str = "/Users/jer/Code/aki-recomp/games/NWXE/syms/dump.toml";

fn nw4e_descriptor() -> DescriptorTableInput {
    (
        DescriptorTableShape {
            table_rom_offset: 0x0539a0,
            record_count: 5,
            record_stride: 0x24,
            field_rom_start: 0x00,
            field_rom_end: 0x04,
            field_vram_dest: 0x08,
        },
        |index| format!("R{}", index + 1),
    )
}

fn main() {
    println!("=== fn64-discover D1 candidate grade ===\n");
    if let Err(error) = grade_oot() {
        eprintln!("OoT D1 gate FAILED: {error}");
        std::process::exit(1);
    }

    for (label, rom, dump, descriptor) in [
        ("NW4E", NW4E_ROM, NW4E_DUMP, Some(nw4e_descriptor())),
        ("NWXE", NWXE_ROM, NWXE_DUMP, None),
    ] {
        if !Path::new(rom).exists() || !Path::new(dump).exists() {
            println!("{label}: optional grade skipped (ROM or answer key absent)\n");
            continue;
        }
        match grade_one(label, rom, dump, descriptor) {
            Ok(()) => {}
            Err(error) => println!("{label}: optional grade unavailable: {error}\n"),
        }
    }
}

fn grade_oot() -> Result<(), String> {
    let rom_bytes =
        std::fs::read(OOT_ROM).map_err(|error| format!("reading {OOT_ROM}: {error}"))?;
    let (rom, db) =
        run_discovery(&rom_bytes, None).map_err(|error| format!("normalizing OoT ROM: {error}"))?;
    if rom.sha1 != OOT_SHA1 {
        return Err(format!(
            "answer key is bound to OoT SHA-1 {OOT_SHA1}, got {}",
            rom.sha1
        ));
    }
    let key_text = std::fs::read_to_string(OOT_DUMP)
        .map_err(|error| format!("reading {OOT_DUMP}: {error}"))?;
    let key = parse_symbol_dump(&key_text)?;
    if key.function_count != OOT_FUNCTIONS || key.section_count != OOT_SECTIONS {
        return Err(format!(
            "expected {OOT_SECTIONS} sections / {OOT_FUNCTIONS} functions, got {} / {}",
            key.section_count, key.function_count
        ));
    }
    print_report("OoT", &db, &key);
    Ok(())
}

fn grade_one(
    label: &str,
    rom_path: &str,
    dump_path: &str,
    descriptor: Option<DescriptorTableInput>,
) -> Result<(), String> {
    let rom_bytes =
        std::fs::read(rom_path).map_err(|error| format!("reading {rom_path}: {error}"))?;
    let (_rom, db) = run_discovery(&rom_bytes, descriptor)
        .map_err(|error| format!("normalizing {label} ROM: {error}"))?;
    let key_text = std::fs::read_to_string(dump_path)
        .map_err(|error| format!("reading {dump_path}: {error}"))?;
    let key = parse_symbol_dump(&key_text)?;
    print_report(label, &db, &key);
    Ok(())
}

fn print_report(
    label: &str,
    db: &fn64_discover::FactDb,
    key: &fn64_discover::grade_candidates::CandidateAnswerKey,
) {
    let report = grade_candidates(db, key);
    println!(
        "{label}: answer_key={} functions / {} sections",
        report.answer_key_total, key.section_count
    );
    for grade in &report.per_detector {
        print_detector(grade);
    }
    println!(
        "  combined: candidates={} tp_entries={} recalled_functions={} fp={} fn={} precision={:.6}% recall={:.6}% ungradable={}",
        report.combined.candidates,
        report.combined.true_positives,
        report.combined.recalled_functions,
        report.combined.false_positives,
        report.combined.false_negatives,
        report.combined.precision() * 100.0,
        report.combined.recall() * 100.0,
        report.combined_ungradable,
    );
    println!();
}

fn print_detector(grade: &DetectorGrade) {
    println!(
        "  {:?}: candidates={} tp_entries={} recalled_functions={} fp={} fn={} precision={:.6}% recall={:.6}% ungradable={}",
        grade.detector,
        grade.metrics.candidates,
        grade.metrics.true_positives,
        grade.metrics.recalled_functions,
        grade.metrics.false_positives,
        grade.metrics.false_negatives,
        grade.metrics.precision() * 100.0,
        grade.metrics.recall() * 100.0,
        grade.ungradable,
    );
}
