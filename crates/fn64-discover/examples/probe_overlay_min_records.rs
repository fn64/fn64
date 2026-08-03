//! Probe: what does the AKI overlay family search admit at a given
//! `min_records` floor?
//!
//! WCW/nWo Revenge's real descriptor table (ROM 0x37a30, stride 0x1c) holds
//! exactly TWO records, so the default floor of 3 rejects it and the whole
//! game falls back to boot-bank-only discovery. This probe measures the
//! floor's effect per ROM before anyone proposes changing it.
//!
//! Usage: `cargo run --release -p fn64-discover --example
//! probe_overlay_min_records -- <rom.z64> [min_records]`
use fn64_discover::overlay_regions::{recover_overlay_regions, SearchConfig};

fn main() {
    let rom = std::env::args().nth(1).expect("usage: <rom> [min_records]");
    let min_records: u32 = std::env::args()
        .nth(2)
        .and_then(|value| value.parse().ok())
        .unwrap_or(3);
    let bytes = std::fs::read(&rom).expect("read rom");
    let mut config = SearchConfig::aki_family();
    config.min_records = min_records;
    // The reference callers derive the mapped-region floor from the search
    // floor rather than picking an independent number; keep that coupling so
    // the probe measures what production would do.
    let min_mapped_regions = config.min_records;
    let recovery =
        recover_overlay_regions(&bytes, &config, &Default::default(), min_mapped_regions);
    let admitted = recovery
        .admissions
        .iter()
        .filter(|admission| admission.admitted)
        .count();
    let intervals = recovery.admitted_intervals();
    println!(
        "min_records={min_records} candidate_tables={} admitted_tables={} admitted_intervals={}",
        recovery.candidate_tables.len(),
        admitted,
        intervals.len(),
    );
    for (start, end) in intervals.iter().take(10) {
        println!("  rom {start:#x}..{end:#x}");
    }
    for admission in &recovery.admissions {
        println!(
            "  table rom={:#x} stride={:#x} records={} mapped={} admitted={} deltas={:?}",
            admission.table.table_rom_offset,
            admission.table.record_stride,
            admission.table.records.len(),
            admission.mapped_regions,
            admission.admitted,
            admission.region_deltas,
        );
        for record in admission.table.records.iter().take(4) {
            println!(
                "    rec rom {:#x}..{:#x} dest {:#010x}",
                record.rom_start, record.rom_end, record.vram_dest
            );
        }
    }
}
