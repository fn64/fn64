//! Generalization test: does the descriptor-family overlay-region search
//! (`overlay_regions::SearchConfig::aki_family` + `delta_vote`) recover
//! anything on NON-AKI-engine titles?
//!
//! NW4E and NWXE (graded by `gate_overlay_regions`/`gate_d1_overlays`) both
//! share the AKI engine's overlay-loader shape: a single flat table of
//! `(rom_start, rom_end, vram_dest)` triples read directly out of physical
//! ROM bytes by a table-driven copy loop. Their 100% overlay-recovery success
//! might be an artifact of that shared shape rather than evidence the
//! pipeline generalizes. This gate runs the identical, unmodified family
//! search against four non-AKI titles and reports exactly what happens for
//! each:
//!
//!  - OoT: uses overlay tables (actor, effect, gamestate, kaleido) but with a
//!    different table shape than AKI's -- the real generalization test, and
//!    the only one of the four with a held-out grading key available here.
//!  - GoldenEye 007 / Perfect Dark: RARE's engine, its own overlay/section
//!    structure, distinct from both AKI and OoT. No answer-key dump is wired
//!    up for these titles in this crate, so their recovery is reported
//!    ungraded -- raw candidate/admission counts and regions only, clearly
//!    labeled "ungraded, no key".
//!  - Super Mario 64: a single static image with NO overlay tables at all.
//!    The honest, correct outcome here is that the family search finds
//!    nothing to admit -- a negative-control check that the search does not
//!    hallucinate a table where none exists.
//!
//! # What is held out
//!
//! The family search runs on raw ROM bytes only: no table offset, stride, or
//! field layout is supplied per title. For OoT,
//! `oot_reference::oot_load_image_tables` (OoT's real, byte-verified table
//! geometry) is opened ONLY after the search and delta_vote admission are
//! complete, and used exclusively to grade whether any admitted region
//! happens to correspond to a real OoT overlay load image. It is never fed
//! into the search. GoldenEye, Perfect Dark, and SM64 have no such reference
//! wired into this crate, so their results are reported as raw recovery
//! output with no grading step at all (not even an attempted one).
//!
//! # Honest outcomes per ROM (see module doc in `overlay_regions.rs` for the
//! discipline this gate is bound by)
//!
//! (a) Admitted regions match real overlay load images -> the pipeline
//!     generalizes beyond the AKI engine family for that title (strong
//!     positive). Only checkable for OoT here.
//! (b) The family search finds no qualifying table (or finds tables that
//!     don't match the title's known geometry, where known) -> the
//!     descriptor family as coded is AKI-shaped for that title; for OoT this
//!     gate reports the precise shape gap against
//!     `oot_reference::oot_load_image_tables`. For SM64 this is the EXPECTED
//!     and correct outcome (no overlays exist to find).
//! (c) The search finds candidate tables but delta_vote correctly declines to
//!     admit them -> the discipline holds even off-distribution (also a good
//!     sign, reported as such).
//!
//! None of these is forced. The exit code is 0 in every case; this is a
//! measurement, not a pass/fail gate over an outcome we don't get to pick.
//!
//! Usage:
//!   FN64_DISCOVER_OOT_ROM=<oot.z64> FN64_DISCOVER_OOT_DUMP=<oot dump.toml> \
//!   FN64_DISCOVER_GE_ROM=<goldeneye.z64> \
//!   FN64_DISCOVER_PD_ROM=<perfect_dark.z64> \
//!   FN64_DISCOVER_SM64_ROM=<sm64.z64> \
//!   gate_overlay_generalize
//! Any unset ROM var yields a loud `skip` for that title only, never a
//! silent omission. The determinism digest of this gate's stdout is fixed
//! only for a stated, fully-populated set of these env vars; a run with
//! fewer ROMs present legitimately produces different (shorter) output.

use fn64_discover::delta_vote::DeltaVoteConfig;
use fn64_discover::normalize;
use fn64_discover::oot_reference::oot_load_image_tables;
use fn64_discover::overlay_regions::{recover_overlay_regions, OverlayRecovery, SearchConfig};

/// One title to run the family search against.
struct RomTarget {
    /// Short label used in report headers.
    label: &'static str,
    /// Env var naming the ROM path.
    rom_env: &'static str,
    /// Whether this title has a held-out grading key wired up in this crate.
    graded: Graded,
}

enum Graded {
    /// OoT: `FN64_DISCOVER_OOT_DUMP` names a reference dump.toml; opened only
    /// after discovery, and only `oot_reference::oot_load_image_tables` (not
    /// the dump itself) is needed for the shape-gap/overlap report.
    Oot { dump_env: &'static str },
    /// No answer key wired into this crate for this title: report raw
    /// recovery only, explicitly labeled ungraded.
    None,
}

const TARGETS: &[RomTarget] = &[
    RomTarget {
        label: "OoT",
        rom_env: "FN64_DISCOVER_OOT_ROM",
        graded: Graded::Oot {
            dump_env: "FN64_DISCOVER_OOT_DUMP",
        },
    },
    RomTarget {
        label: "GoldenEye 007",
        rom_env: "FN64_DISCOVER_GE_ROM",
        graded: Graded::None,
    },
    RomTarget {
        label: "Perfect Dark",
        rom_env: "FN64_DISCOVER_PD_ROM",
        graded: Graded::None,
    },
    RomTarget {
        label: "Super Mario 64",
        rom_env: "FN64_DISCOVER_SM64_ROM",
        graded: Graded::None,
    },
];

fn main() {
    if let Err(error) = run() {
        eprintln!("gate_overlay_generalize: {error}");
        std::process::exit(1);
    }
}

/// A table is admitted when delta_vote uniquely maps a strict majority of its
/// records (floored at 2). Same rule shape as `gate_overlay_regions` /
/// `gate_d1_overlays`; not tuned to any of these titles' real record counts.
fn min_mapped(records: usize) -> u32 {
    ((records / 2) + 1).max(2) as u32
}

fn run() -> Result<(), String> {
    let config = SearchConfig::aki_family();
    let delta_config = DeltaVoteConfig::default();

    println!(
        "gate_overlay_generalize: does the AKI-derived descriptor-family search generalize beyond the AKI engine family?"
    );
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
    println!(
        "note: the determinism digest of this gate's stdout is fixed only for a stated, \
fully-populated set of ROM env vars below -- an unset var is a loud skip that legitimately \
shortens output."
    );

    for target in TARGETS {
        run_target(target, &config, &delta_config)?;
    }

    println!(
        "\nSame PI-DMA caveat as gate_overlay_regions applies transitively (not evaluated further \
here): this gate only exercises route 1, the descriptor-family search."
    );

    Ok(())
}

fn run_target(
    target: &RomTarget,
    config: &SearchConfig,
    delta_config: &DeltaVoteConfig,
) -> Result<(), String> {
    let rom_path = match std::env::var(target.rom_env) {
        Ok(path) => path,
        Err(_) => {
            println!("\n=== {} === skip ({} unset)", target.label, target.rom_env);
            return Ok(());
        }
    };

    let rom_bytes =
        std::fs::read(&rom_path).map_err(|error| format!("reading {rom_path}: {error}"))?;
    let rom = normalize(&rom_bytes).map_err(|error| error.to_string())?;
    println!("\n=== {} ===", target.label);
    println!("ROM SHA-256 {}", rom.sha256);

    let recovery = recover_overlay_regions(
        &rom.bytes,
        config,
        delta_config,
        min_mapped(config.min_records as usize),
    );
    report_recovery(&recovery);

    match &target.graded {
        Graded::Oot { dump_env } => grade_against_oot(dump_env, &recovery)?,
        Graded::None => {
            let admitted = recovery.admitted_intervals().len();
            println!(
                "\nRESULT ({}): ungraded, no key -- no answer-key table geometry is wired into \
this crate for this title. Reporting raw recovery only: {} raw candidate table(s), {admitted} \
admitted region interval(s). {}",
                target.label,
                recovery.candidate_tables.len(),
                if admitted == 0 {
                    "Outcome (b)-shaped: the search proposed nothing the delta_vote discipline \
would admit."
                } else {
                    "Admitted intervals exist but cannot be checked against a real load image \
without a reference dump for this title -- reported as a candidate, not a finding."
                }
            );
        }
    }

    Ok(())
}

/// Grade OoT's recovery against its known table geometry. The dump.toml is
/// opened here (only after discovery is complete) purely to confirm the
/// grading-key file is present; the actual comparison uses
/// `oot_reference::oot_load_image_tables`, which is the byte-verified table
/// shape this crate already carries for OoT and is not search input.
fn grade_against_oot(dump_env: &str, recovery: &OverlayRecovery) -> Result<(), String> {
    let known_tables = oot_load_image_tables();
    println!("\n--- held-out comparison: OoT's real table geometry (opened after discovery) ---");
    for table in &known_tables {
        let loc = table.shape.location;
        println!(
            "  known table '{}': space={:?} offset=0x{:x} record_count=0x{:x} stride=0x{:x} \
source(space={:?}, +0x{:x}..+0x{:x}) dest(space={:?}, +0x{:x})",
            table.name,
            loc.space,
            loc.offset,
            table.shape.record_count,
            table.shape.record_stride,
            table.shape.source.space,
            table.shape.source.field_start,
            table.shape.source.field_end,
            table.shape.destination.space,
            table.shape.destination.field_start,
        );
    }

    let dump_path_check = match std::env::var(dump_env) {
        Ok(path) => std::fs::metadata(&path).is_ok(),
        Err(_) => false,
    };
    println!(
        "\nOoT dump.toml presence check (grading key, opened but not required beyond geometry above): {}",
        if dump_path_check { "present" } else { "MISSING" }
    );

    let admitted_intervals = recovery.admitted_intervals();
    if admitted_intervals.is_empty() {
        println!(
            "\nRESULT (OoT): (b) the family search admitted ZERO region intervals. \
Raw candidate tables: {}. See shape-gap analysis below.",
            recovery.candidate_tables.len()
        );
    } else {
        println!(
            "\nRESULT (OoT): family search admitted {} region interval(s). Cross-checking \
against known table geometry below.",
            admitted_intervals.len()
        );
        for &(start, end) in &admitted_intervals {
            // dmadata is the only table located in physical ROM space in
            // oot_load_image_tables(); its records are VROM entries and its
            // own table location is physical, so we can directly compare.
            let overlaps_dmadata_table = known_tables.iter().any(|table| {
                table.name == "dmadata"
                    && table.shape.location.offset < end
                    && start < table_end_estimate(table)
            });
            println!(
                "  admitted [0x{start:06x},0x{end:06x}): overlaps dmadata table location range = {overlaps_dmadata_table}"
            );
        }
    }

    println!("\n--- shape-gap analysis (AKI family search vs OoT known geometry) ---");
    print_shape_gap(&SearchConfig::aki_family(), &known_tables);

    Ok(())
}

/// Rough end-of-table physical byte estimate for the overlap check above --
/// intentionally coarse (a generous upper bound), used only to decide whether
/// an admitted interval's *start* falls inside the table's own footprint, not
/// to claim record-level agreement.
fn table_end_estimate(table: &fn64_discover::banks::LoadImageTableInput) -> u32 {
    table.shape.location.offset.saturating_add(
        table
            .shape
            .record_count
            .saturating_mul(table.shape.record_stride),
    )
}

fn report_recovery(recovery: &OverlayRecovery) {
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
carried in-bounds, ordered, code-sized ROM intervals with a plausible RDRAM destination VA \
in physical ROM bytes. Stated as evidence, not forced.",
            recovery.config.min_records
        );
        return;
    }

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
}

/// Print the precise mismatch between what the AKI family search enumerates
/// and OoT's known table shapes, field by field. This is the actionable
/// finding when outcome (b) holds.
fn print_shape_gap(
    config: &SearchConfig,
    known_tables: &[fn64_discover::banks::LoadImageTableInput],
) {
    for table in known_tables {
        let loc = table.shape.location;
        let stride_in_family = config.strides.contains(&table.shape.record_stride);
        println!(
            "  '{}': location.space={:?} (family search reads ONLY physical rom_bytes offsets; \
Virtual/VROM offsets require resolving through the file table first -- structurally out of \
reach of a flat byte-offset scan) | stride=0x{:x} in searched strides {:x?}? {} | \
record_count={} (family min_records={}, {}) | source.field=+0x{:x}..+0x{:x} in {:?} space \
(family assumes rom_start/rom_end are the FIRST TWO consecutive u32 fields, sliding phase \
0..stride-12, in the SAME physical-ROM space as the table itself) | destination.field=+0x{:x} \
in {:?} space (family assumes the destination is ALWAYS a resident RDRAM VA at \
field_rom_end+4, i.e. immediately after rom_end; OoT's non-dmadata tables place it there too, \
but dmadata's destination is PhysicalRom, not Vram -- a space the family search's \
vram_lo/vram_hi window would reject outright)",
            table.name,
            loc.space,
            table.shape.record_stride,
            config.strides,
            stride_in_family,
            table.shape.record_count,
            config.min_records,
            if table.shape.record_count >= config.min_records {
                "satisfies min_records"
            } else {
                "BELOW min_records"
            },
            table.shape.source.field_start,
            table.shape.source.field_end,
            table.shape.source.space,
            table.shape.destination.field_start,
            table.shape.destination.space,
        );
    }
}
