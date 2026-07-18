//! Validate the OOT reference-dump adapter against the canonical ROM.

use fn64_discover::evidence::{EvidenceManifest, EVIDENCE_SCHEMA_VERSION};
use fn64_discover::oot_reference::{
    bind_ranges_to_fact_db_partial, executable_ranges_from_oot_dump,
};
use fn64_discover::{
    normalize, required_env_path, run_discovery_with_load_image_tables, run_discovery_with_manifest,
};

fn main() {
    let rom_path = required_env_path("FN64_DISCOVER_OOT_ROM", "an OoT NTSC 1.0 .z64")
        .unwrap_or_else(|error| {
            eprintln!("gate_oot_reference: {error}");
            std::process::exit(1);
        });
    let dump_path = required_env_path("FN64_DISCOVER_OOT_DUMP", "the OoT reference dump.toml")
        .unwrap_or_else(|error| {
            eprintln!("gate_oot_reference: {error}");
            std::process::exit(1);
        });
    let rom_bytes =
        std::fs::read(&rom_path).unwrap_or_else(|error| panic!("reading {rom_path}: {error}"));
    let rom = normalize(&rom_bytes).expect("normalizing OOT ROM");
    let dump = std::fs::read_to_string(&dump_path).expect("reading OOT symbol dump");
    let ranges =
        executable_ranges_from_oot_dump(&dump, &dump_path).expect("parsing OOT symbol dump");
    assert!(
        !ranges.is_empty(),
        "OOT reference produced no executable ranges"
    );
    let bytes: u64 = ranges
        .iter()
        .map(|range| u64::from(range.va_end - range.va_start))
        .sum();
    let oot_tables = fn64_discover::oot_reference::oot_load_image_tables();
    let (_baseline_rom, baseline_db) =
        run_discovery_with_load_image_tables(&rom_bytes, None, &oot_tables)
            .expect("baseline OOT discovery with load tables");
    let (bound_ranges, unresolved) = bind_ranges_to_fact_db_partial(&dump, &dump_path, &baseline_db)
        .expect("binding OOT ranges to native mappings");
    let bound_count = bound_ranges.len();
    let unresolved_count = unresolved.len();
    let manifest = EvidenceManifest {
        schema_version: EVIDENCE_SCHEMA_VERSION,
        rom_sha256: rom.sha256.clone(),
        descriptor_tables: Vec::new(),
        load_image_tables: oot_tables.to_vec(),
        executable_ranges: bound_ranges,
    };
    let (_validated_rom, db) = run_discovery_with_manifest(&rom_bytes, &manifest)
        .expect("ingesting OOT boot executable-range evidence");
    let proven_ranges = db.proven_rom_mappings().len();
    println!("OOT reference adapter PASSED");
    println!("  normalized sha256={}", rom.sha256);
    println!("  function-bearing sections={}", ranges.len());
    println!("  executable candidate bytes={bytes}");
    println!("  ranges bound to exactly one native bank={}", bound_count);
    println!("  ranges unresolved by native mapping={unresolved_count}");
    if let Some(first) = unresolved.first() {
        println!("  first unresolved range={first}");
    }
    println!("  native mapped banks after evidence={proven_ranges}");
}
