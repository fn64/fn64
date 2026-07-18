//! Generalization gate for the two-stage file-table/VROM overlay search on
//! non-AKI engines. The unchanged physical descriptor path is reported beside
//! the new path so an off-engine physical table remains observable too.
//!
//! Recovery first enumerates physical
//! `(vrom_start,vrom_end,rom_start,rom_end)` file-table families and admits
//! only one distinct contiguous mapping run. It then resolves/materializes
//! VROM files and applies the descriptor family plus `delta_vote` there. No
//! ROM identity, table offset, stride, or field layout is supplied per title.
//!
//! OoT is graded only after recovery against its byte-verified dmadata and
//! effect/actor/gamestate/Kaleido geometry. The held-out comparison reports
//! dmadata agreement, load-table count, and exact
//! `(VROM start,VROM end,VRAM destination)` region precision/recall. GoldenEye
//! and Perfect Dark have no key wired into this crate and remain explicitly
//! ungraded raw measurements. Super Mario 64 is the hard negative control:
//! because it has no overlays, any admission in either address-space path is
//! a precision regression and exits nonzero.
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

use fn64_discover::banks::{DestinationEnd, LoadImageTableInput};
use fn64_discover::delta_vote::DeltaVoteConfig;
use fn64_discover::file_table::FileTableSearchConfig;
use fn64_discover::normalize;
use fn64_discover::oot_reference::oot_load_image_tables;
use fn64_discover::overlay_regions::{
    recover_overlay_regions, recover_vrom_overlay_regions, OverlayRecovery, SearchConfig,
    VromOverlayRecovery,
};
use std::collections::BTreeSet;

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
    /// after both recovery stages complete. Geometry is never search input.
    Oot { dump_env: &'static str },
    /// No answer key wired into this crate for this title: report raw
    /// recovery only, explicitly labeled ungraded.
    None,
    /// SM64 has no overlays. Any admission in either address-space path is a
    /// precision regression and fails loudly.
    NegativeControl,
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
        graded: Graded::NegativeControl,
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
    let vrom_config = SearchConfig::vrom_family();
    let delta_config = DeltaVoteConfig::default();
    let file_table_config = FileTableSearchConfig::n64_family();

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
        "VROM search: region_len=[0x{:x},0x{:x}] link_va=[0x{:08x},0x{:08x}) min_records=2",
        vrom_config.min_region_len,
        vrom_config.max_region_len,
        vrom_config.vram_lo,
        vrom_config.vram_hi,
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
        run_target(
            target,
            &config,
            &vrom_config,
            &delta_config,
            &file_table_config,
        )?;
    }

    println!(
        "\nSame PI-DMA caveat as gate_overlay_regions applies transitively (not evaluated further \
here): this gate only exercises route 1, the descriptor-family search."
    );

    Ok(())
}

fn run_target(
    target: &RomTarget,
    physical_config: &SearchConfig,
    vrom_config: &SearchConfig,
    delta_config: &DeltaVoteConfig,
    file_table_config: &FileTableSearchConfig,
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

    let physical = recover_overlay_regions(
        &rom.bytes,
        physical_config,
        delta_config,
        min_mapped(physical_config.min_records as usize),
    );
    let vrom = recover_vrom_overlay_regions(
        &rom.bytes,
        vrom_config,
        delta_config,
        file_table_config,
        2,
        2,
    );
    println!("\n--- physical-ROM descriptor path (unchanged AKI path) ---");
    report_recovery(&physical);
    report_file_table(&vrom);
    report_vrom_recovery(&vrom);

    match &target.graded {
        Graded::Oot { dump_env } => grade_against_oot(dump_env, &rom.bytes, &vrom)?,
        Graded::None => {
            let physical_admitted = physical.admitted_intervals().len();
            let vrom_admitted = vrom.admitted_intervals().len();
            println!(
                "\nRESULT ({}): ungraded, no key -- no answer-key table geometry is wired into \
this crate for this title. Reporting raw recovery only: physical raw/admitted={}/{}, VROM \
raw/admitted={}/{}. {}",
                target.label,
                physical.candidate_tables.len(),
                physical_admitted,
                vrom.candidate_tables.len(),
                vrom_admitted,
                if physical_admitted + vrom_admitted == 0 {
                    "Outcome (b)-shaped: the search proposed nothing the delta_vote discipline \
would admit."
                } else {
                    "Admitted intervals exist but cannot be checked against a real load image \
without a reference dump for this title -- reported as a candidate, not a finding."
                }
            );
        }
        Graded::NegativeControl => {
            let physical_admitted = physical.admitted_intervals().len();
            let vrom_admitted = vrom.admitted_intervals().len();
            if physical_admitted != 0 || vrom_admitted != 0 {
                return Err(format!(
                    "SM64 NEGATIVE-CONTROL FAILURE: admitted {physical_admitted} physical and \
{vrom_admitted} VROM overlay intervals in a title with no overlays"
                ));
            }
            println!(
                "\nRESULT (Super Mario 64): PASS negative control -- zero physical and zero \
VROM overlay intervals admitted (file-table recovery is reported independently above)."
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
fn grade_against_oot(
    dump_env: &str,
    rom_bytes: &[u8],
    recovery: &VromOverlayRecovery,
) -> Result<(), String> {
    let known_tables = oot_load_image_tables();
    let dump_path = std::env::var(dump_env)
        .map_err(|_| format!("{dump_env} is required when grading an OoT ROM"))?;
    let dump = std::fs::read_to_string(&dump_path)
        .map_err(|error| format!("reading held-out OoT dump {dump_path}: {error}"))?;
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

    println!(
        "\nOoT dump.toml grading key opened after recovery: present ({} bytes)",
        dump.len()
    );

    let known_dmadata = &known_tables[0];
    let file_match = recovery
        .file_table
        .admitted_table
        .as_ref()
        .is_some_and(|table| {
            table.table_rom_offset == known_dmadata.shape.location.offset
                && table.record_stride == known_dmadata.shape.record_stride
                && table.field_vrom_start == known_dmadata.shape.source.field_start
                && table.field_vrom_end == known_dmadata.shape.source.field_end
                && table.field_rom_start == known_dmadata.shape.destination.field_start
                && matches!(
                    known_dmadata.shape.destination.end,
                    DestinationEnd::FieldOrSourceLength(field) if table.field_rom_end == field
                )
        });
    println!(
        "file table recovered: {} (matches dmadata geometry: {})",
        if recovery.file_table.admitted_table.is_some() {
            "yes"
        } else {
            "no"
        },
        if file_match { "yes" } else { "NO" }
    );

    let known_overlay_tables = &known_tables[1..];
    let candidate_table_count = known_overlay_tables
        .iter()
        .filter(|known| candidate_table(known, recovery))
        .count();
    let admitted_table_count = known_overlay_tables
        .iter()
        .filter(|known| recovered_table(known, recovery))
        .count();
    for known in known_overlay_tables {
        println!(
            "  overlay table '{}': candidate_geometry={} delta_admitted={}",
            known.name,
            candidate_table(known, recovery),
            recovered_table(known, recovery)
        );
    }

    let key_regions = known_oot_regions(rom_bytes, recovery, known_overlay_tables)?;
    let recovered_regions: BTreeSet<_> = recovery
        .admissions
        .iter()
        .filter(|admission| admission.admitted)
        .flat_map(|admission| {
            admission
                .table
                .records
                .iter()
                .map(|record| (record.rom_start, record.rom_end, record.vram_dest))
        })
        .collect();
    let true_positive = recovered_regions.intersection(&key_regions).count();
    let spurious = recovered_regions.len() - true_positive;
    let missed = key_regions.len() - true_positive;
    let precision = if recovered_regions.is_empty() {
        0.0
    } else {
        100.0 * true_positive as f64 / recovered_regions.len() as f64
    };
    let recall = if key_regions.is_empty() {
        0.0
    } else {
        100.0 * true_positive as f64 / key_regions.len() as f64
    };
    println!(
        "OoT candidate load-table geometry recovered: {}/5 (dmadata={} + overlay descriptor tables={}/4)",
        usize::from(file_match) + candidate_table_count,
        usize::from(file_match),
        candidate_table_count,
    );
    println!(
        "OoT delta-admitted load tables: {}/5 (dmadata={} + overlay descriptor tables={}/4)",
        usize::from(file_match) + admitted_table_count,
        usize::from(file_match),
        admitted_table_count,
    );
    println!(
        "OoT regions: recovered={} true_positive={} spurious={} key={} missed={}",
        recovered_regions.len(),
        true_positive,
        spurious,
        key_regions.len(),
        missed,
    );
    println!("OoT region precision={precision:.4}% recall={recall:.4}%");

    Ok(())
}

fn recovered_table(known: &LoadImageTableInput, recovery: &VromOverlayRecovery) -> bool {
    recovery
        .admissions
        .iter()
        .filter(|admission| admission.admitted)
        .any(|admission| table_location_matches(known, &admission.table))
}

fn candidate_table(known: &LoadImageTableInput, recovery: &VromOverlayRecovery) -> bool {
    recovery
        .candidate_tables
        .iter()
        .any(|candidate| table_location_matches(known, candidate))
}

fn table_location_matches(
    known: &LoadImageTableInput,
    candidate: &fn64_discover::overlay_regions::VromCandidateTable,
) -> bool {
    let first_source = known.shape.location.offset + known.shape.source.field_start;
    let table_end = known.shape.location.offset.saturating_add(
        known
            .shape
            .record_count
            .saturating_mul(known.shape.record_stride),
    );
    let candidate_source = candidate.table_vrom_offset + candidate.field_rom_start;
    candidate.record_stride == known.shape.record_stride
        && candidate_source >= first_source
        && candidate_source < table_end
        && (candidate_source - first_source).is_multiple_of(known.shape.record_stride)
}

fn known_oot_regions(
    rom_bytes: &[u8],
    recovery: &VromOverlayRecovery,
    tables: &[LoadImageTableInput],
) -> Result<BTreeSet<(u32, u32, u32)>, String> {
    let file_table = recovery.file_table.admitted_table.as_ref().ok_or_else(|| {
        "cannot materialize held-out OoT tables without a unique recovered file table".to_string()
    })?;
    let mut regions = BTreeSet::new();
    for table in tables {
        let shape = table.shape;
        let destination_end_field = match shape.destination.end {
            DestinationEnd::Field(field) | DestinationEnd::FieldOrSourceLength(field) => field,
            DestinationEnd::SourceLength => shape.destination.field_start,
        };
        let max_field = [
            shape.source.field_start,
            shape.source.field_end,
            shape.destination.field_start,
            destination_end_field,
        ]
        .into_iter()
        .max()
        .unwrap();
        let table_len = shape
            .record_count
            .saturating_sub(1)
            .saturating_mul(shape.record_stride)
            .saturating_add(max_field)
            .saturating_add(4);
        let table_bytes = file_table.materialize_vrom_range(
            rom_bytes,
            shape.location.offset,
            shape.location.offset.saturating_add(table_len),
        )?;
        for index in 0..shape.record_count {
            let base = (index * shape.record_stride) as usize;
            let read = |field: u32| -> Option<u32> {
                let offset = base.checked_add(field as usize)?;
                Some(u32::from_be_bytes(
                    table_bytes.get(offset..offset + 4)?.try_into().ok()?,
                ))
            };
            let (Some(source_start), Some(source_end), Some(destination_start)) = (
                read(shape.source.field_start),
                read(shape.source.field_end),
                read(shape.destination.field_start),
            ) else {
                continue;
            };
            let destination_end = match shape.destination.end {
                DestinationEnd::Field(field) => read(field),
                DestinationEnd::SourceLength => {
                    destination_start.checked_add(source_end.saturating_sub(source_start))
                }
                DestinationEnd::FieldOrSourceLength(field) => read(field).and_then(|end| {
                    if end == 0 {
                        destination_start.checked_add(source_end.saturating_sub(source_start))
                    } else {
                        Some(end)
                    }
                }),
            };
            if source_end > source_start
                && destination_end.is_some_and(|end| end > destination_start)
                && file_table.contains_vrom_range(source_start, source_end)
            {
                regions.insert((source_start, source_end, destination_start));
            }
        }
    }
    Ok(regions)
}

fn report_file_table(recovery: &VromOverlayRecovery) {
    println!("\n--- physical file-table recovery ---");
    println!(
        "file-table tightening: {} distinct candidate(s) -> admitted={}",
        recovery.file_table.candidate_tables.len(),
        recovery.file_table.admitted_table.is_some(),
    );
    for table in &recovery.file_table.candidate_tables {
        println!(
            "file table @ROM 0x{:x} stride=0x{:x} vrom_alignment=0x{:x} fields(vrom=+0x{:x}..+0x{:x},rom=+0x{:x}..+0x{:x}) records={} max_vrom=0x{:x}",
            table.table_rom_offset,
            table.record_stride,
            table.vrom_alignment,
            table.field_vrom_start,
            table.field_vrom_end,
            table.field_rom_start,
            table.field_rom_end,
            table.records.len(),
            table.max_vrom_end(),
        );
    }
}

fn report_vrom_recovery(recovery: &VromOverlayRecovery) {
    println!("\n--- VROM-located descriptor path ---");
    let admitted_tables = recovery
        .admissions
        .iter()
        .filter(|admission| admission.admitted)
        .count();
    println!(
        "tightening: {} raw VROM family table(s) -> {admitted_tables} delta_vote-admitted table(s) ({} distinct admitted region interval(s)); min_records={} mapped_floor={}",
        recovery.candidate_tables.len(),
        recovery.admitted_intervals().len(),
        recovery.vrom_min_records,
        recovery.min_mapped_regions,
    );
    for admission in &recovery.admissions {
        println!(
            "table @VROM 0x{:x} stride=0x{:x} fields(start=+0x{:x},end=+0x{:x},vram=+0x{:x}) records={} mapped={}/{} required={} admitted={}",
            admission.table.table_vrom_offset,
            admission.table.record_stride,
            admission.table.field_rom_start,
            admission.table.field_rom_end,
            admission.table.field_vram_dest,
            admission.table.records.len(),
            admission.mapped_regions,
            admission.table.records.len(),
            admission.required_mapped_regions,
            admission.admitted,
        );
    }
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
