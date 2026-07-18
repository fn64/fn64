//! D1 gate: grade Phase 3 function-entry candidates against answer keys that
//! are consumed only after discovery. OoT's zeldaret-derived key is the
//! required 13,358-function measurement; NW4E/NWXE use the same harness when
//! their local ROM/key files are present.

use fn64_discover::banks::{
    BankNamePattern, DestinationEnd, DestinationRangeFields, DestinationSpace, LoadImageTableInput,
    LoadImageTableShape, SourceRangeFields, TableLocation,
};
use fn64_discover::evidence::EvidenceManifest;
use fn64_discover::facts::load_image_table_record_subject;
use fn64_discover::grade_candidates::{grade_candidates, parse_symbol_dump, DetectorGrade};
use fn64_discover::{
    run_discovery, run_discovery_with_load_image_tables, run_discovery_with_manifest,
    DescriptorTableInput, RomAddressSpace,
};
use std::collections::BTreeMap;
use std::path::Path;

fn oot_rom() -> Result<String, String> {
    fn64_discover::required_env_path("FN64_DISCOVER_OOT_ROM", "an OoT NTSC 1.0 .z64")
}
fn oot_dump() -> Result<String, String> {
    fn64_discover::required_env_path("FN64_DISCOVER_OOT_DUMP", "the OoT reference dump.toml")
}
const OOT_SHA1: &str = "ad69c91157f6705e8ab06c79fe08aad47bb57ba7";
const OOT_FUNCTIONS: usize = 13_358;
const OOT_SECTIONS: usize = 472;

fn aki_inputs() -> Result<[(String, String); 2], String> {
    Ok([
        (
            fn64_discover::required_env_path("FN64_DISCOVER_NW4E_ROM", "the NW4E .z64")?,
            fn64_discover::required_env_path("FN64_DISCOVER_NW4E_DUMP", "the NW4E dump.toml")?,
        ),
        (
            fn64_discover::required_env_path("FN64_DISCOVER_NWXE_ROM", "the NWXE .z64")?,
            fn64_discover::required_env_path("FN64_DISCOVER_NWXE_DUMP", "the NWXE dump.toml")?,
        ),
    ])
}

fn nw4e_descriptor() -> DescriptorTableInput {
    (
        fn64_discover::aki_reference::NW4E_DESCRIPTOR_TABLE,
        fn64_discover::aki_reference::nw4e_bank_name,
    )
}

/// OoT NTSC 1.0 table geometry, supplied as shape/data input. Locations and
/// field use come from the allowed N64Recomp-generated C/section table:
/// DmaMgr_Init names physical dmadata at 0x7430; the generated code section
/// maps VROM 0xA87000 to VRAM 0x800110A0; table consumers name the four VAs,
/// strides, counts, and field offsets below. No dump.toml value enters this
/// input; that file is opened only after discovery for grading.
fn oot_load_image_tables() -> [LoadImageTableInput; 5] {
    [
        LoadImageTableInput {
            name: "dmadata".to_string(),
            shape: LoadImageTableShape {
                location: TableLocation {
                    space: RomAddressSpace::Physical,
                    offset: 0x0000_7430,
                },
                record_count: 0x5f6,
                record_stride: 0x10,
                source: SourceRangeFields {
                    space: RomAddressSpace::Virtual,
                    field_start: 0x00,
                    field_end: 0x04,
                },
                destination: DestinationRangeFields {
                    space: DestinationSpace::PhysicalRom,
                    field_start: 0x08,
                    end: DestinationEnd::FieldOrSourceLength(0x0c),
                },
            },
            bank_name: None,
        },
        LoadImageTableInput {
            name: "effect_overlays".to_string(),
            shape: LoadImageTableShape {
                location: TableLocation {
                    space: RomAddressSpace::Virtual,
                    offset: 0x00b5_dba0,
                },
                record_count: 0x25,
                record_stride: 0x1c,
                source: SourceRangeFields {
                    space: RomAddressSpace::Virtual,
                    field_start: 0x00,
                    field_end: 0x04,
                },
                destination: DestinationRangeFields {
                    space: DestinationSpace::Vram,
                    field_start: 0x08,
                    end: DestinationEnd::Field(0x0c),
                },
            },
            bank_name: Some(BankNamePattern::new("effect_overlay_", 0, "")),
        },
        LoadImageTableInput {
            name: "actor_overlays".to_string(),
            shape: LoadImageTableShape {
                location: TableLocation {
                    space: RomAddressSpace::Virtual,
                    offset: 0x00b5_e490,
                },
                record_count: 0x1d7,
                record_stride: 0x20,
                source: SourceRangeFields {
                    space: RomAddressSpace::Virtual,
                    field_start: 0x00,
                    field_end: 0x04,
                },
                destination: DestinationRangeFields {
                    space: DestinationSpace::Vram,
                    field_start: 0x08,
                    end: DestinationEnd::Field(0x0c),
                },
            },
            bank_name: Some(BankNamePattern::new("actor_overlay_", 0, "")),
        },
        LoadImageTableInput {
            name: "gamestate_overlays".to_string(),
            shape: LoadImageTableShape {
                location: TableLocation {
                    space: RomAddressSpace::Virtual,
                    offset: 0x00b6_72a0,
                },
                record_count: 6,
                record_stride: 0x30,
                source: SourceRangeFields {
                    space: RomAddressSpace::Virtual,
                    field_start: 0x04,
                    field_end: 0x08,
                },
                destination: DestinationRangeFields {
                    space: DestinationSpace::Vram,
                    field_start: 0x0c,
                    end: DestinationEnd::Field(0x10),
                },
            },
            bank_name: Some(BankNamePattern::new("gamestate_overlay_", 0, "")),
        },
        LoadImageTableInput {
            name: "kaleido_overlays".to_string(),
            shape: LoadImageTableShape {
                location: TableLocation {
                    space: RomAddressSpace::Virtual,
                    offset: 0x00b7_43e0,
                },
                record_count: 2,
                record_stride: 0x1c,
                source: SourceRangeFields {
                    space: RomAddressSpace::Virtual,
                    field_start: 0x04,
                    field_end: 0x08,
                },
                destination: DestinationRangeFields {
                    space: DestinationSpace::Vram,
                    field_start: 0x0c,
                    end: DestinationEnd::Field(0x10),
                },
            },
            bank_name: Some(BankNamePattern::new("kaleido_overlay_", 0, "")),
        },
    ]
}

fn main() {
    println!("=== fn64-discover D1 candidate grade ===\n");
    if let Err(error) = grade_oot() {
        eprintln!("OoT D1 gate FAILED: {error}");
        std::process::exit(1);
    }

    let aki = aki_inputs().unwrap_or_else(|error| {
        eprintln!("gate_d1: {error}");
        std::process::exit(1);
    });
    let [(nw4e_rom, nw4e_dump), (nwxe_rom, nwxe_dump)] = aki;
    for (label, rom, dump, descriptor) in [
        (
            "NW4E",
            nw4e_rom.as_str(),
            nw4e_dump.as_str(),
            Some(nw4e_descriptor()),
        ),
        ("NWXE", nwxe_rom.as_str(), nwxe_dump.as_str(), None),
    ] {
        if !Path::new(rom).exists() || !Path::new(dump).exists() {
            println!("{label}: optional grade skipped (ROM or answer key absent)\n");
            continue;
        }
        let evidence_var = format!("FN64_DISCOVER_{label}_EVIDENCE");
        let result = match std::env::var_os(&evidence_var) {
            Some(path) => grade_one_with_manifest(label, rom, dump, Path::new(&path)),
            None => grade_one(label, rom, dump, descriptor),
        };
        match result {
            Ok(()) => {}
            Err(error) => println!("{label}: optional grade unavailable: {error}\n"),
        }
    }
}

fn grade_one_with_manifest(
    label: &str,
    rom_path: &str,
    dump_path: &str,
    evidence_path: &Path,
) -> Result<(), String> {
    let rom_bytes =
        std::fs::read(rom_path).map_err(|error| format!("reading {rom_path}: {error}"))?;
    let evidence_text = std::fs::read_to_string(evidence_path)
        .map_err(|error| format!("reading {}: {error}", evidence_path.display()))?;
    let manifest =
        EvidenceManifest::from_toml(&evidence_text).map_err(|error| error.to_string())?;
    let (_rom, db) =
        run_discovery_with_manifest(&rom_bytes, &manifest).map_err(|error| error.to_string())?;
    let key_text = std::fs::read_to_string(dump_path)
        .map_err(|error| format!("reading {dump_path}: {error}"))?;
    let key = parse_symbol_dump(&key_text)?;
    println!("{label}: evidence manifest {}", evidence_path.display());
    print_report(label, &db, &key);
    Ok(())
}

fn grade_oot() -> Result<(), String> {
    let oot_rom_path = oot_rom()?;
    let rom_bytes =
        std::fs::read(&oot_rom_path).map_err(|error| format!("reading {oot_rom_path}: {error}"))?;
    let (rom, db) =
        run_discovery_with_load_image_tables(&rom_bytes, None, &oot_load_image_tables())
            .map_err(|error| format!("normalizing OoT ROM: {error}"))?;
    if rom.sha1 != OOT_SHA1 {
        return Err(format!(
            "answer key is bound to OoT SHA-1 {OOT_SHA1}, got {}",
            rom.sha1
        ));
    }
    print_oot_phase2(&db);
    let oot_dump_path = oot_dump()?;
    let key_text = std::fs::read_to_string(&oot_dump_path)
        .map_err(|error| format!("reading {oot_dump_path}: {error}"))?;
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

fn print_oot_phase2(db: &fn64_discover::FactDb) {
    for table in [
        "dmadata",
        "effect_overlays",
        "actor_overlays",
        "gamestate_overlays",
        "kaleido_overlays",
    ] {
        let mut states = BTreeMap::new();
        for fact in db.facts() {
            let fn64_discover::Fact::LoadImageTableRecord {
                table: fact_table,
                index,
                ..
            } = fact
            else {
                continue;
            };
            if fact_table != table {
                continue;
            }
            let state = db
                .conclusion(&load_image_table_record_subject(table, *index))
                .expect("every parsed table record has a conclusion")
                .state;
            *states.entry(format!("{state:?}")).or_insert(0usize) += 1;
        }
        println!("  Phase 2 {table}: {states:?}");
    }
    println!(
        "  Phase 2 proven load images: {}",
        db.proven_rom_mappings().len()
    );
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
    let mut indirect = BTreeMap::new();
    for fact in db.facts() {
        if let fn64_discover::Fact::IndirectTransferAnalysis {
            via_call,
            state,
            kind,
            ..
        } = fact
        {
            *indirect
                .entry(format!(
                    "{}:{state:?}:{}",
                    if *via_call { "call" } else { "jump" },
                    kind.map_or_else(|| "None".to_string(), |kind| format!("{kind:?}"))
                ))
                .or_insert(0usize) += 1;
        }
    }
    println!("  Phase 6 indirect sites: {indirect:?}");
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
    if grade.metrics.false_positives != 0 {
        let breakdown = &grade.false_positive_breakdown;
        println!(
            "    fp breakdown: target_interior={} target_outside_functions={} source_inside_function={} source_outside_functions={}",
            breakdown.target_interior,
            breakdown.target_outside_functions,
            breakdown.source_inside_function,
            breakdown.source_outside_functions,
        );
    }
}
