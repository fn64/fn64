//! Validate a decomp answer-key dump (any game) against its ROM.
//!
//! Game-agnostic sibling of `gate_oot_reference`: same dump format (see
//! `scripts/import_splat_syms.py` and `scripts/import_decomp_map.py`), but
//! no baked-in load-table geometry — binding runs against whatever banks
//! ROM-only discovery proves (at minimum the boot bank), and every range a
//! proven bank can't own is reported as unresolved rather than guessed.
//!
//! Env:
//!   FN64_DISCOVER_ROM      the game's .z64
//!   FN64_DISCOVER_DUMP     the matching answer-key dump.toml
//!   FN64_DISCOVER_REQUEST_DMA  optional static request-DMA claims TOML
//!                          (cited load-request callee + arg registers,
//!                          e.g. reference/mm-n64-us-request-dma.toml);
//!                          recovered banks come from constant operands
//!                          at call sites in the proven boot image
//!   FN64_DISCOVER_TABLES   optional load-image table geometry TOML
//!                          (explicit cited claims, e.g.
//!                          crates/fn64-discover/reference/
//!                          mm-n64-us-load-tables.toml); without it only
//!                          ROM-only banks (the boot bank) can own ranges

use fn64_discover::banks::{LoadImageTableInput, StaticRequestDmaInput};
use fn64_discover::evidence::{EvidenceManifest, EVIDENCE_SCHEMA_VERSION};
use fn64_discover::oot_reference::{
    bind_ranges_to_fact_db_partial, executable_ranges_from_oot_dump,
};
use fn64_discover::{
    normalize, required_env_path, run_discovery_with_manifest_and_request_dma,
    run_discovery_with_tables_and_request_dma,
};
use serde::Deserialize;

#[derive(Deserialize)]
struct TablesFile {
    load_image_tables: Vec<LoadImageTableInput>,
}

#[derive(Deserialize)]
struct RequestDmaFile {
    request_dma: Vec<StaticRequestDmaInput>,
}

fn load_toml_env<T: serde::de::DeserializeOwned>(variable: &str) -> Option<T> {
    let path = std::env::var(variable).ok()?;
    let text =
        std::fs::read_to_string(&path).unwrap_or_else(|error| panic!("reading {path}: {error}"));
    Some(toml::from_str(&text).unwrap_or_else(|error| panic!("parsing {path}: {error}")))
}

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
    let tables: Vec<LoadImageTableInput> = load_toml_env::<TablesFile>("FN64_DISCOVER_TABLES")
        .map(|file| file.load_image_tables)
        .unwrap_or_default();
    let request_dma: Vec<StaticRequestDmaInput> =
        load_toml_env::<RequestDmaFile>("FN64_DISCOVER_REQUEST_DMA")
            .map(|file| file.request_dma)
            .unwrap_or_default();
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
    let (_baseline_rom, baseline_db, request_report) =
        run_discovery_with_tables_and_request_dma(&rom_bytes, None, &tables, &request_dma)
            .expect("baseline discovery");
    if !request_dma.is_empty() {
        println!(
            "  request-dma banks proven={} open sites={}",
            request_report.proven_banks.len(),
            request_report.open.len()
        );
        for line in &request_report.open {
            println!("  request-dma open: {line}");
        }
    }
    let (mut bound_ranges, unresolved) =
        bind_ranges_to_fact_db_partial(&dump, &dump_path, &baseline_db)
            .expect("binding dump ranges to native mappings");
    // A section whose only function is unsized yields an empty extent —
    // no evidence to ingest; report it rather than assert an empty range.
    let before = bound_ranges.len();
    bound_ranges.retain(|range| range.va_end > range.va_start);
    let empty_extent = before - bound_ranges.len();
    let bound_count = bound_ranges.len();
    let unresolved_count = unresolved.len();
    let manifest = EvidenceManifest {
        schema_version: EVIDENCE_SCHEMA_VERSION,
        rom_sha256: rom.sha256.clone(),
        descriptor_tables: Vec::new(),
        load_image_tables: tables.clone(),
        executable_ranges: bound_ranges,
    };
    let (_validated_rom, db, _manifest_report) =
        run_discovery_with_manifest_and_request_dma(&rom_bytes, &manifest, &request_dma)
            .expect("ingesting bound executable-range evidence");
    let proven_ranges = db.proven_rom_mappings().len();
    println!("decomp reference adapter PASSED");
    println!("  normalized sha256={}", rom.sha256);
    println!("  function-bearing sections={}", ranges.len());
    println!("  executable candidate bytes={bytes}");
    println!("  ranges bound to exactly one native bank={bound_count}");
    if empty_extent != 0 {
        println!("  bound ranges dropped for empty extent (unsized-only sections)={empty_extent}");
    }
    println!("  ranges unresolved by native mapping={unresolved_count}");
    for line in &unresolved {
        println!("  unresolved: {line}");
    }
    println!("  native mapped banks after evidence={proven_ranges}");
}
