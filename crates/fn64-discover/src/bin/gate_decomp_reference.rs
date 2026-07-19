//! Validate a decomp answer-key dump (any game) against its ROM.
//!
//! Game-agnostic sibling of `gate_oot_reference`: same dump format (see
//! `scripts/import_splat_syms.py` and `scripts/import_decomp_map.py`), but
//! no baked-in load-table geometry — binding runs against whatever banks
//! ROM-only discovery proves (at minimum the boot bank), and every range a
//! proven bank can't own is reported as unresolved rather than guessed.
//!
//! Env:
//!   FN64_DISCOVER_ROM    the game's .z64
//!   FN64_DISCOVER_DUMP   the matching answer-key dump.toml

use fn64_discover::evidence::{EvidenceManifest, EVIDENCE_SCHEMA_VERSION};
use fn64_discover::oot_reference::{
    bind_ranges_to_fact_db_partial, executable_ranges_from_oot_dump,
};
use fn64_discover::{normalize, required_env_path, run_discovery, run_discovery_with_manifest};

fn main() {
    let rom_path =
        required_env_path("FN64_DISCOVER_ROM", "the game's .z64").unwrap_or_else(|error| {
            eprintln!("gate_decomp_reference: {error}");
            std::process::exit(1);
        });
    let dump_path = required_env_path("FN64_DISCOVER_DUMP", "the answer-key dump.toml")
        .unwrap_or_else(|error| {
            eprintln!("gate_decomp_reference: {error}");
            std::process::exit(1);
        });
    let rom_bytes =
        std::fs::read(&rom_path).unwrap_or_else(|error| panic!("reading {rom_path}: {error}"));
    let rom = normalize(&rom_bytes).expect("normalizing ROM");
    let dump = std::fs::read_to_string(&dump_path).expect("reading symbol dump");
    let ranges = executable_ranges_from_oot_dump(&dump, &dump_path).expect("parsing symbol dump");
    assert!(!ranges.is_empty(), "dump produced no executable ranges");
    let bytes: u64 = ranges
        .iter()
        .map(|range| u64::from(range.va_end - range.va_start))
        .sum();
    let (_baseline_rom, baseline_db) =
        run_discovery(&rom_bytes, None).expect("baseline ROM-only discovery");
    let (bound_ranges, unresolved) = bind_ranges_to_fact_db_partial(&dump, &dump_path, &baseline_db)
        .expect("binding dump ranges to native mappings");
    let bound_count = bound_ranges.len();
    let unresolved_count = unresolved.len();
    let manifest = EvidenceManifest {
        schema_version: EVIDENCE_SCHEMA_VERSION,
        rom_sha256: rom.sha256.clone(),
        descriptor_tables: Vec::new(),
        load_image_tables: Vec::new(),
        executable_ranges: bound_ranges,
    };
    let (_validated_rom, db) = run_discovery_with_manifest(&rom_bytes, &manifest)
        .expect("ingesting bound executable-range evidence");
    let proven_ranges = db.proven_rom_mappings().len();
    println!("decomp reference adapter PASSED");
    println!("  normalized sha256={}", rom.sha256);
    println!("  function-bearing sections={}", ranges.len());
    println!("  executable candidate bytes={bytes}");
    println!("  ranges bound to exactly one native bank={bound_count}");
    println!("  ranges unresolved by native mapping={unresolved_count}");
    if let Some(first) = unresolved.first() {
        println!("  first unresolved range={first}");
    }
    println!("  native mapped banks after evidence={proven_ranges}");
}
