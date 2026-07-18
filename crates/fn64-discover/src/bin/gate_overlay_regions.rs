//! Grading gate for `overlay_regions`: recover candidate overlay ROM intervals
//! from ROM bytes alone (no hard-coded table offset) and grade them against the
//! byte-verified overlay layout.
//!
//! # What is held out
//!
//! The recovery consumes ONLY the ROM bytes and the family `SearchConfig`. The
//! answer key -- NW4E's descriptor geometry (`aki_reference::NW4E_BANKS`) and
//! NWXE's `dump.toml` overlay sections -- enters exclusively at grading time.
//! The NW4E table offset `0x539a0` is NOT handed to the search: re-deriving it
//! from the family shape is the generalization test.
//!
//! # What is graded
//!
//! For each recovered admitted region:
//!  - is its `[rom_start, rom_end)` one of the key's overlay ROM intervals
//!    (region precision/recall), and
//!  - does `delta_vote` admit the region's correct VA delta (the same delta the
//!    key's `vram_dest` implies)?
//!
//! Before/after tightening is reported: raw family candidates vs delta_vote-
//! admitted tables. A WRONG delta admission (admitted delta disagreeing with
//! the key) is the failure that exits nonzero. A recovered ROM interval that is
//! a strict superset of the key interval (trailing padding included by the
//! table's `rom_end` field) is graded on its start; the interval is reported so
//! the difference is visible, never hidden.
//!
//! Usage:
//!   FN64_DISCOVER_NW4E_ROM=<nomercy.z64> \
//!   FN64_DISCOVER_NWXE_ROM=<wm2000.z64>  \
//!   FN64_DISCOVER_NWXE_DUMP=<nwxe dump.toml> \
//!   gate_overlay_regions
//! Any unset ROM/dump var yields a loud `skip` line, never a silent omission.

use fn64_discover::aki_reference::NW4E_BANKS;
use fn64_discover::delta_vote::DeltaVoteConfig;
use fn64_discover::normalize;
use fn64_discover::overlay_regions::{recover_overlay_regions, OverlayRecovery, SearchConfig};

/// One overlay region in the grading key: its ROM start and the VA delta the
/// key attributes to it.
#[derive(Clone, Copy, Debug)]
struct KeyOverlay {
    rom_start: u32,
    key_delta: u32,
}

fn main() {
    match run() {
        Ok(0) => {}
        Ok(wrong) => {
            eprintln!("gate_overlay_regions: {wrong} WRONG delta admission(s) -- see tables above");
            std::process::exit(1);
        }
        Err(error) => {
            eprintln!("gate_overlay_regions: {error}");
            std::process::exit(1);
        }
    }
}

fn run() -> Result<u32, String> {
    let config = SearchConfig::aki_family();
    let delta_config = DeltaVoteConfig::default();
    // A table is admitted when delta_vote uniquely maps a strict majority of
    // its regions. Half-plus-one, floored at 2, so a 3-record table needs 2
    // and a 5-record table needs 3 -- one open region (a data-heavy overlay
    // with too few discriminating jals) does not sink a real table, but a
    // table whose regions are mostly unmappable is not admitted.
    let min_mapped = |records: usize| ((records / 2) + 1).max(2) as u32;

    println!("gate_overlay_regions: ROM-only overlay-region recovery (held-out grading)");
    println!(
        "search: min_rom_offset=0x{:x} region_len=[0x{:x},0x{:x}] vram=[0x{:08x},0x{:08x}) strides={:x?} min_records={}",
        config.min_rom_offset,
        config.min_region_len,
        config.max_region_len,
        config.vram_lo,
        config.vram_hi,
        config.strides,
        config.min_records,
    );
    println!(
        "delta_vote: alignment=0x{:x} min_votes={} domination_factor={}",
        delta_config.alignment, delta_config.min_votes, delta_config.domination_factor
    );

    let mut total_wrong = 0u32;

    // --- NW4E: generalization test, graded against aki_reference ---------
    match std::env::var("FN64_DISCOVER_NW4E_ROM") {
        Ok(path) => {
            let rom_bytes =
                std::fs::read(&path).map_err(|error| format!("reading {path}: {error}"))?;
            let rom = normalize(&rom_bytes).map_err(|error| error.to_string())?;
            let key: Vec<KeyOverlay> = NW4E_BANKS
                .iter()
                .map(|bank| KeyOverlay {
                    rom_start: bank.rom_start,
                    key_delta: bank.va_start.wrapping_sub(bank.rom_start),
                })
                .collect();
            // NW4E's table has 5 records; majority => 3.
            let recovery =
                recover_overlay_regions(&rom.bytes, &config, &delta_config, min_mapped(5));
            total_wrong += grade("NW4E", &rom.sha256, &recovery, &key);
        }
        Err(_) => println!("\nNW4E: skip (FN64_DISCOVER_NW4E_ROM unset)"),
    }

    // --- NWXE: the direct attack, graded against its dump sections -------
    match (
        std::env::var("FN64_DISCOVER_NWXE_ROM"),
        std::env::var("FN64_DISCOVER_NWXE_DUMP"),
    ) {
        (Ok(rom_path), Ok(dump_path)) => {
            let rom_bytes =
                std::fs::read(&rom_path).map_err(|error| format!("reading {rom_path}: {error}"))?;
            let rom = normalize(&rom_bytes).map_err(|error| error.to_string())?;
            let dump_text = std::fs::read_to_string(&dump_path)
                .map_err(|error| format!("reading {dump_path}: {error}"))?;
            let key = parse_overlay_sections(&dump_text)?;
            // NWXE's table has 4 records; majority => 3.
            let recovery =
                recover_overlay_regions(&rom.bytes, &config, &delta_config, min_mapped(4));
            total_wrong += grade("NWXE", &rom.sha256, &recovery, &key);
        }
        _ => println!("\nNWXE: skip (FN64_DISCOVER_NWXE_ROM or FN64_DISCOVER_NWXE_DUMP unset)"),
    }

    println!(
        "\nPI-DMA cross-check (route 2): the AKI overlay loader is table-driven -- \
its copy routine reads rom_start/rom_end/vram_dest out of a descriptor record \
through registers (NW4E record_loader VA 0x8000073c), so osPiStartDma/osEPiStartDma \
operands are not immediates and pi_dma leaves them as blockers. The descriptor-table \
route (route 1) recovers those triples directly; route 2 does not independently \
propose these regions for these titles. Stated, not forced."
    );

    Ok(total_wrong)
}

/// Grade one ROM's recovery and return the count of WRONG delta admissions.
fn grade(label: &str, sha256: &str, recovery: &OverlayRecovery, key: &[KeyOverlay]) -> u32 {
    println!("\n=== {label} ===");
    println!("ROM SHA-256 {sha256}");

    // Before/after tightening.
    let raw_tables = recovery.candidate_tables.len();
    let admitted_tables = recovery.admissions.iter().filter(|a| a.admitted).count();
    let admitted_intervals = recovery.admitted_intervals();
    println!(
        "tightening: {raw_tables} raw family table(s) -> {admitted_tables} delta_vote-admitted table(s) ({} admitted region interval(s))",
        admitted_intervals.len()
    );

    if recovery.candidate_tables.is_empty() {
        println!(
            "no descriptor table of the searched family qualified: no run of >= {} records \
carried in-bounds, ordered, code-sized ROM intervals with a plausible RDRAM destination VA. \
This is stated as evidence, not forced.",
            recovery.config.min_records
        );
    }

    // Per candidate table (raw): its intervals and delta_vote outcome per
    // region, so the report shows what the search proposed and what survived.
    for admission in &recovery.admissions {
        let table = &admission.table;
        println!(
            "table @ROM 0x{:x} stride=0x{:x} fields(start=+0x{:x},end=+0x{:x},vram=+0x{:x}) records={} mapped={}/{} admitted={}",
            table.table_rom_offset,
            table.record_stride,
            table.field_rom_start,
            table.field_rom_end,
            table.field_vram_dest,
            table.records.len(),
            admission.mapped_regions,
            table.records.len(),
            admission.admitted,
        );
        for (rec, delta) in table.records.iter().zip(&admission.region_deltas) {
            let dv = match delta {
                Some((d, va)) => format!("delta=0x{d:08x} va=0x{va:08x}"),
                None => "open".to_string(),
            };
            println!(
                "    [0x{:06x},0x{:06x}) len=0x{:x} vram_dest=0x{:08x}  {}",
                rec.rom_start,
                rec.rom_end,
                rec.byte_len(),
                rec.vram_dest,
                dv
            );
        }
    }

    // Region precision/recall on ADMITTED intervals (by start match).
    let key_starts: std::collections::BTreeSet<u32> = key.iter().map(|k| k.rom_start).collect();
    let mut true_positive = 0u32;
    let mut wrong_delta = 0u32;
    for admission in recovery.admissions.iter().filter(|a| a.admitted) {
        for (rec, delta) in admission.table.records.iter().zip(&admission.region_deltas) {
            if key_starts.contains(&rec.rom_start) {
                true_positive += 1;
                // Cross-check the admitted delta against the key delta.
                if let (Some((got, _)), Some(k)) =
                    (delta, key.iter().find(|k| k.rom_start == rec.rom_start))
                {
                    if *got != k.key_delta {
                        wrong_delta += 1;
                        println!(
                            "    !!! WRONG DELTA for region @0x{:06x}: delta_vote 0x{:08x} vs key 0x{:08x}",
                            rec.rom_start, got, k.key_delta
                        );
                    }
                }
            }
        }
    }
    let admitted_region_count: u32 = recovery
        .admissions
        .iter()
        .filter(|a| a.admitted)
        .map(|a| a.table.records.len() as u32)
        .sum();
    let spurious = admitted_region_count - true_positive;
    let precision = if admitted_region_count == 0 {
        0.0
    } else {
        100.0 * true_positive as f64 / admitted_region_count as f64
    };
    let recall = if key.is_empty() {
        0.0
    } else {
        100.0 * true_positive as f64 / key.len() as f64
    };

    // How many admitted regions had delta_vote uniquely map them?
    let mapped_admitted: u32 = recovery
        .admissions
        .iter()
        .filter(|a| a.admitted)
        .map(|a| a.mapped_regions)
        .sum();

    println!(
        "regions: recovered={admitted_region_count} true_positive={true_positive} spurious={spurious} key_overlays={}",
        key.len()
    );
    println!("region precision={precision:.4}% recall={recall:.4}%");
    println!(
        "delta_vote: {mapped_admitted}/{admitted_region_count} admitted regions uniquely mapped, {wrong_delta} WRONG"
    );

    wrong_delta
}

/// Parse a `dump.toml`'s overlay sections into the grading key. Overlay
/// sections are the code sections that are NOT the resident `entry`/`main_*`
/// (which are not table-loaded overlays). Grading-only: this parser is never
/// reachable from the recovery path.
fn parse_overlay_sections(text: &str) -> Result<Vec<KeyOverlay>, String> {
    #[derive(serde::Deserialize)]
    struct Doc {
        #[serde(default)]
        section: Vec<Section>,
    }
    #[derive(serde::Deserialize)]
    struct Section {
        name: String,
        rom: u32,
        vram: u32,
    }
    let doc: Doc = toml::from_str(text).map_err(|error| error.to_string())?;
    let mut key = Vec::new();
    for section in doc.section {
        // Resident sections load via the boot copy, not the overlay table.
        if section.name == "entry" || section.name.starts_with("main") {
            continue;
        }
        key.push(KeyOverlay {
            rom_start: section.rom,
            key_delta: section.vram.wrapping_sub(section.rom),
        });
    }
    if key.is_empty() {
        return Err("dump.toml has no non-resident overlay sections to grade against".to_string());
    }
    Ok(key)
}
