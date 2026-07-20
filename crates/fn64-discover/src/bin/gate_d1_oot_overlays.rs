//! Held-out OoT D1 re-grade for mechanically recovered VROM overlays.
//!
//! All three discovery runs finish before the grading dump is opened. The
//! mechanical run receives only generic family-search configuration; the
//! byte-verified OoT table geometry is consumed solely by the explicit
//! hand-geometry ceiling run.

use fn64_discover::banks::{self, BankNamePattern};
use fn64_discover::delta_vote::DeltaVoteConfig;
use fn64_discover::file_table::FileTableSearchConfig;
use fn64_discover::grade_candidates::{grade_candidates, parse_symbol_dump, CandidateGradeReport};
use fn64_discover::oot_reference::oot_load_image_tables;
use fn64_discover::overlay_regions::{SearchConfig, VromOverlayRecovery};
use fn64_discover::{
    required_env_path, run_discovery, run_discovery_with_load_image_tables,
    run_discovery_with_recovered_vrom_overlay_regions, Fact, FactDb, RecoveredVromOverlayInput,
};
use serde::Deserialize;
use std::collections::{BTreeMap, BTreeSet};

const OOT_SHA1: &str = "ad69c91157f6705e8ab06c79fe08aad47bb57ba7";
const OOT_FUNCTIONS: usize = 13_358;
const OOT_SECTIONS: usize = 472;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct RegionKey {
    rom_start: u32,
    rom_end: u32,
    vram_start: u32,
}

#[derive(Debug, Clone)]
struct HandMapping {
    region: RegionKey,
    vram_end: u32,
    family: &'static str,
}

#[derive(Debug, Default)]
struct FamilyAccounting {
    banks: usize,
    recovered_banks: usize,
    answer_functions: usize,
    recovered_functions: usize,
}

#[derive(Debug, Deserialize)]
struct GradingDump {
    #[serde(rename = "section")]
    sections: Vec<GradingSection>,
}

#[derive(Debug, Deserialize)]
struct GradingSection {
    rom: u32,
    vram: u32,
    #[serde(default)]
    functions: Vec<GradingFunction>,
}

#[derive(Debug, Deserialize)]
struct GradingFunction {
    vram: u32,
}

fn main() {
    if let Err(error) = run() {
        eprintln!("gate_d1_oot_overlays: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let rom_path = required_env_path("FN64_DISCOVER_OOT_ROM", "an OoT NTSC 1.0 .z64")?;
    let dump_path = required_env_path(
        "FN64_DISCOVER_OOT_DUMP",
        "the grading-only OoT reference dump.toml",
    )?;
    let rom_bytes =
        std::fs::read(&rom_path).map_err(|error| format!("reading {rom_path}: {error}"))?;

    eprintln!("gate_d1_oot_overlays: running A boot-only discovery");
    let (boot_rom, boot_db) = run_discovery(&rom_bytes, None).map_err(|error| error.to_string())?;

    let mechanical_input = RecoveredVromOverlayInput {
        search: SearchConfig::vrom_family(),
        delta_vote: DeltaVoteConfig::default(),
        file_table_search: FileTableSearchConfig::n64_family(),
        vrom_min_records: 2,
        min_mapped_regions: 2,
        file_table_name: "recovered_file_table".to_string(),
        table_name: "recovered_vrom_overlay_descriptors".to_string(),
        bank_name: BankNamePattern::new("recovered_overlay_", 0, ""),
    };
    eprintln!("gate_d1_oot_overlays: running B mechanical VROM-overlay discovery");
    let (mechanical_rom, mechanical_db, recovery) =
        run_discovery_with_recovered_vrom_overlay_regions(&rom_bytes, &mechanical_input)
            .map_err(|error| error.to_string())?;

    // This geometry is intentionally not constructed until the mechanical
    // discovery run is immutable. It is the comparison ceiling, never input B.
    let hand_tables = oot_load_image_tables();
    eprintln!("gate_d1_oot_overlays: running C hand-geometry discovery");
    let (hand_rom, hand_db) = run_discovery_with_load_image_tables(&rom_bytes, None, &hand_tables)
        .map_err(|error| error.to_string())?;

    if boot_rom.sha256 != mechanical_rom.sha256 || boot_rom.sha256 != hand_rom.sha256 {
        return Err("A/B/C normalized to different ROMs".into());
    }
    if boot_rom.sha1 != OOT_SHA1 {
        return Err(format!(
            "answer key is bound to OoT SHA-1 {OOT_SHA1}, got {}",
            boot_rom.sha1
        ));
    }

    // HELD-OUT BOUNDARY: the answer key is opened only after A, B, and C
    // discovery have all completed. Nothing below can feed either FactDb.
    let key_text = std::fs::read_to_string(&dump_path)
        .map_err(|error| format!("reading {dump_path}: {error}"))?;
    eprintln!("gate_d1_oot_overlays: grading completed A/B/C discovery results");
    let key = parse_symbol_dump(&key_text)?;
    if key.function_count != OOT_FUNCTIONS || key.section_count != OOT_SECTIONS {
        return Err(format!(
            "expected {OOT_SECTIONS} sections / {OOT_FUNCTIONS} functions, got {} / {}",
            key.section_count, key.function_count
        ));
    }
    let grading_dump: GradingDump = toml::from_str(&key_text).map_err(|error| error.to_string())?;

    let boot_grade = grade_candidates(&boot_db, &key);
    let mechanical_grade = grade_candidates(&mechanical_db, &key);
    let hand_grade = grade_candidates(&hand_db, &key);
    let mechanical_regions = overlay_regions(&mechanical_db);
    let hand_regions = overlay_regions(&hand_db);
    let wrong_regions: Vec<_> = mechanical_regions
        .difference(&hand_regions)
        .copied()
        .collect();
    let missed_regions: Vec<_> = hand_regions
        .difference(&mechanical_regions)
        .copied()
        .collect();
    let hand_mappings = hand_mappings(&hand_db);
    let accounting = account_families(&grading_dump, &hand_mappings, &mechanical_regions)?;

    println!("gate_d1_oot_overlays: OoT held-out mechanical VROM-overlay re-grade");
    println!("ROM SHA-256 {}", boot_rom.sha256);
    report_recovery(&recovery);
    print_grade("A boot-only", &boot_grade, &boot_db, 0);
    print_grade(
        "B mechanically-recovered-overlays",
        &mechanical_grade,
        &mechanical_db,
        recovery.admitted_intervals().len(),
    );
    print_grade(
        "C hand-supplied-table-geometry",
        &hand_grade,
        &hand_db,
        hand_regions.len(),
    );

    let recall_ratio = ratio(
        mechanical_grade.combined.recalled_functions,
        hand_grade.combined.recalled_functions,
    );
    let mechanical_gain = mechanical_grade
        .combined
        .recalled_functions
        .saturating_sub(boot_grade.combined.recalled_functions);
    let hand_gain = hand_grade
        .combined
        .recalled_functions
        .saturating_sub(boot_grade.combined.recalled_functions);
    println!(
        "headline: B recall={:.6}% vs C recall={:.6}% ({recall_ratio:.4}% of C recalled functions); gap={} functions / {:.6} points",
        mechanical_grade.combined.recall() * 100.0,
        hand_grade.combined.recall() * 100.0,
        hand_grade.combined.recalled_functions as i64
            - mechanical_grade.combined.recalled_functions as i64,
        (hand_grade.combined.recall() - mechanical_grade.combined.recall()) * 100.0,
    );
    println!(
        "overlay payoff: B captures {:.4}% of C's recall gain over A ({} / {} added functions)",
        ratio(mechanical_gain, hand_gain),
        mechanical_gain,
        hand_gain,
    );

    println!("per-family answer-key accounting:");
    for family in ["effect", "actor", "gamestate", "kaleido"] {
        let row = accounting
            .get(family)
            .expect("all hand overlay families are represented");
        println!(
            "  {family}: banks={} mechanically_recovered_banks={} answer_key_functions={} recovered_functions={} missed_functions={}",
            row.banks,
            row.recovered_banks,
            row.answer_functions,
            row.recovered_functions,
            row.answer_functions - row.recovered_functions,
        );
    }
    let recovered_actor_kaleido =
        accounting["actor"].recovered_functions + accounting["kaleido"].recovered_functions;
    let unrecovered_effect_gamestate = accounting["effect"].answer_functions
        - accounting["effect"].recovered_functions
        + accounting["gamestate"].answer_functions
        - accounting["gamestate"].recovered_functions;
    println!(
        "bank accounting headline: recovered actor+kaleido answer-key functions={recovered_actor_kaleido}; unrecovered effect+gamestate answer-key functions={unrecovered_effect_gamestate}"
    );
    println!(
        "wrong/spurious: wrong_recovered_regions={} missed_hand_regions={} B_spurious_candidates={} C_spurious_candidates={}",
        wrong_regions.len(),
        missed_regions.len(),
        mechanical_grade.combined.false_positives,
        hand_grade.combined.false_positives,
    );
    for region in wrong_regions.iter().take(12) {
        println!(
            "  WRONG region VROM [0x{:x},0x{:x}) -> VRAM 0x{:08x}",
            region.rom_start, region.rom_end, region.vram_start
        );
    }

    if !wrong_regions.is_empty() {
        return Err("mechanical recovery promoted a region absent from the hand geometry".into());
    }
    Ok(())
}

fn report_recovery(recovery: &VromOverlayRecovery) {
    let admitted_tables = recovery
        .admissions
        .iter()
        .filter(|admission| admission.admitted)
        .count();
    println!(
        "recovery: file_table_candidates={} file_table_admitted={} raw_overlay_tables={} admitted_overlay_tables={} recovered_regions={}",
        recovery.file_table.candidate_tables.len(),
        recovery.file_table.admitted_table.is_some(),
        recovery.candidate_tables.len(),
        admitted_tables,
        recovery.admitted_intervals().len(),
    );
    if let Some(table) = &recovery.file_table.admitted_table {
        println!(
            "  file table @ROM 0x{:x}: stride=0x{:x} records={}",
            table.table_rom_offset,
            table.record_stride,
            table.records.len(),
        );
    }
    for admission in recovery
        .admissions
        .iter()
        .filter(|admission| admission.admitted)
    {
        println!(
            "  admitted overlay table @VROM 0x{:x}: stride=0x{:x} records={} delta_mapped={} required={}",
            admission.table.table_vrom_offset,
            admission.table.record_stride,
            admission.table.records.len(),
            admission.mapped_regions,
            admission.required_mapped_regions,
        );
    }
}

fn print_grade(label: &str, report: &CandidateGradeReport, db: &FactDb, regions: usize) {
    let metrics = &report.combined;
    println!(
        "{label}: candidates={} tp_entries={} recalled_functions={} fp={} fn={} precision={:.6}% recall={:.6}% proven_overlay_banks={} recovered_regions={} ungradable={}",
        metrics.candidates,
        metrics.true_positives,
        metrics.recalled_functions,
        metrics.false_positives,
        metrics.false_negatives,
        metrics.precision() * 100.0,
        metrics.recall() * 100.0,
        proven_overlay_banks(db),
        regions,
        report.combined_ungradable,
    );
}

fn ratio(numerator: usize, denominator: usize) -> f64 {
    if denominator == 0 {
        0.0
    } else {
        numerator as f64 * 100.0 / denominator as f64
    }
}

fn overlay_regions(db: &FactDb) -> BTreeSet<RegionKey> {
    db.proven_rom_mappings()
        .into_iter()
        .filter_map(|fact| match fact {
            Fact::RomMapping {
                bank,
                rom_start,
                rom_end,
                va_start,
                ..
            } if bank != banks::BOOT_BANK => Some(RegionKey {
                rom_start: *rom_start,
                rom_end: *rom_end,
                vram_start: *va_start,
            }),
            _ => None,
        })
        .collect()
}

fn proven_overlay_banks(db: &FactDb) -> usize {
    db.proven_rom_mappings()
        .into_iter()
        .filter_map(|fact| match fact {
            Fact::RomMapping { bank, .. } if bank != banks::BOOT_BANK => Some(bank),
            _ => None,
        })
        .collect::<BTreeSet<_>>()
        .len()
}

fn hand_mappings(db: &FactDb) -> Vec<HandMapping> {
    db.proven_rom_mappings()
        .into_iter()
        .filter_map(|fact| match fact {
            Fact::RomMapping {
                bank,
                rom_start,
                rom_end,
                va_start,
                va_end,
                ..
            } => family(bank).map(|family| HandMapping {
                region: RegionKey {
                    rom_start: *rom_start,
                    rom_end: *rom_end,
                    vram_start: *va_start,
                },
                vram_end: *va_end,
                family,
            }),
            _ => None,
        })
        .collect()
}

fn family(bank: &str) -> Option<&'static str> {
    ["effect", "actor", "gamestate", "kaleido"]
        .into_iter()
        .find(|family| bank.starts_with(&format!("{family}_overlay_")))
}

fn account_families(
    dump: &GradingDump,
    mappings: &[HandMapping],
    mechanical_regions: &BTreeSet<RegionKey>,
) -> Result<BTreeMap<&'static str, FamilyAccounting>, String> {
    let mut out: BTreeMap<_, _> = ["effect", "actor", "gamestate", "kaleido"]
        .into_iter()
        .map(|family| (family, FamilyAccounting::default()))
        .collect();
    for mapping in mappings {
        let row = out.get_mut(mapping.family).unwrap();
        row.banks += 1;
        row.recovered_banks += usize::from(mechanical_regions.contains(&mapping.region));
    }

    for section in &dump.sections {
        for function in &section.functions {
            let offset = function.vram.checked_sub(section.vram).ok_or_else(|| {
                format!(
                    "grading function at 0x{:08x} precedes section at 0x{:08x}",
                    function.vram, section.vram
                )
            })?;
            let rom = section
                .rom
                .checked_add(offset)
                .ok_or_else(|| "grading function ROM address overflowed u32".to_string())?;
            let matches: Vec<_> = mappings
                .iter()
                .filter(|mapping| {
                    rom >= mapping.region.rom_start
                        && rom < mapping.region.rom_end
                        && function.vram >= mapping.region.vram_start
                        && function.vram < mapping.vram_end
                })
                .collect();
            match matches.as_slice() {
                [] => {}
                [mapping] => {
                    let row = out.get_mut(mapping.family).unwrap();
                    row.answer_functions += 1;
                    row.recovered_functions +=
                        usize::from(mechanical_regions.contains(&mapping.region));
                }
                many => {
                    return Err(format!(
                        "answer-key entry (VROM 0x{rom:x}, VRAM 0x{:08x}) belongs to {} hand overlay mappings",
                        function.vram,
                        many.len()
                    ));
                }
            }
        }
    }
    Ok(out)
}
