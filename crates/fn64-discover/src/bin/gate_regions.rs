//! Experimental ROM-only region transition report. The boot mapping needs no
//! external input. An optional evidence manifest adds load mappings; its
//! executable ranges are held out and used only to label nearest-boundary
//! diagnostics, never to influence candidate ranking.

use fn64_discover::banks;
use fn64_discover::evidence::{self, EvidenceManifest};
use fn64_discover::regions;
use fn64_discover::{normalize, Fact, FactDb};
use std::path::PathBuf;

fn main() {
    if let Err(error) = run() {
        eprintln!("gate_regions: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let mut args = std::env::args_os().skip(1);
    let rom_path = PathBuf::from(args.next().ok_or_else(usage)?);
    let evidence_path = args.next().map(PathBuf::from);
    if args.next().is_some() {
        return Err(usage());
    }
    let rom_bytes = std::fs::read(&rom_path)
        .map_err(|error| format!("reading {}: {error}", rom_path.display()))?;
    let rom = normalize(&rom_bytes).map_err(|error| error.to_string())?;
    let mut db = FactDb::new();
    let _boot = banks::discover_boot_bank(&rom, &mut db);
    let manifest = if let Some(evidence_path) = evidence_path {
        let evidence_text = std::fs::read_to_string(&evidence_path)
            .map_err(|error| format!("reading {}: {error}", evidence_path.display()))?;
        let manifest =
            EvidenceManifest::from_toml(&evidence_text).map_err(|error| error.to_string())?;
        manifest
            .validate_identity(&rom)
            .map_err(|error| error.to_string())?;
        evidence::apply_mapping_evidence(&rom, &manifest, &mut db)
            .map_err(|error| error.to_string())?;
        Some(manifest)
    } else {
        None
    };

    println!("ROM SHA-256 {}", rom.sha256);
    println!("rank scores order named feature deltas; they are not proof states\n");
    for mapping in db.proven_rom_mappings() {
        let Fact::RomMapping {
            bank,
            rom_space,
            rom_start,
            rom_end,
            va_start,
            ..
        } = mapping
        else {
            unreachable!()
        };
        let bytes = banks::materialize_rom_range(&rom, &db, *rom_space, *rom_start, *rom_end)
            .map_err(|error| format!("materializing {bank}: {error}"))?
            .bytes;
        println!("{bank}: ROM [0x{rom_start:08x},0x{rom_end:08x}) VA 0x{va_start:08x}");
        let expected_boundary = manifest
            .iter()
            .flat_map(|manifest| &manifest.executable_ranges)
            .find(|range| {
                range.bank == *bank
                    && range.va_start >= *va_start
                    && range.va_end <= va_start.saturating_add(bytes.len() as u32)
            })
            .map(|range| rom_start.saturating_add(range.va_end.saturating_sub(*va_start)));
        let views = regions::analyze_multiscale(
            &bytes,
            *rom_start,
            *va_start,
            rom.len(),
            &[0x40, 0x100, 0x400, 0x1000],
        )?;
        let consensus = regions::consensus_boundaries(&views, 100)?;
        print!("  consensus top-10%-per-scale:");
        for item in consensus.iter().take(8) {
            print!(
                " 0x{:08x}=scales:{}/points:{}",
                item.boundary_rom, item.scale_support, item.rank_points
            );
        }
        if let Some(expected) = expected_boundary {
            if let Some(nearest) = consensus
                .iter()
                .min_by_key(|item| item.boundary_rom.abs_diff(expected))
            {
                let rank = consensus
                    .iter()
                    .position(|item| item.boundary_rom == nearest.boundary_rom)
                    .unwrap()
                    + 1;
                print!(
                    " | held-out expected=0x{expected:08x} nearest=0x{:08x} distance=0x{:x} rank={rank}/{} scales={} points={}",
                    nearest.boundary_rom,
                    nearest.boundary_rom.abs_diff(expected),
                    consensus.len(),
                    nearest.scale_support,
                    nearest.rank_points,
                );
            }
        }
        println!();
        for view in views {
            let window = view.window_bytes;
            let mut transitions = view.transitions;
            transitions.sort_by(|left, right| {
                right
                    .rank_score
                    .cmp(&left.rank_score)
                    .then_with(|| left.boundary_rom.cmp(&right.boundary_rom))
            });
            print!("  window 0x{window:04x}:");
            for item in transitions.iter().take(8) {
                print!(
                    " 0x{:08x}={}({}/{}/{}/{}/{})",
                    item.boundary_rom,
                    item.rank_score,
                    item.structured_control_delta_per_mille,
                    item.pointer_delta_per_mille,
                    item.zero_word_delta_per_mille,
                    item.nonzero_byte_delta_per_mille,
                    item.byte_diversity_delta_per_mille,
                );
            }
            if let Some(expected) = expected_boundary {
                let nearest = transitions
                    .iter()
                    .min_by_key(|item| item.boundary_rom.abs_diff(expected));
                if let Some(nearest) = nearest {
                    let rank = transitions
                        .iter()
                        .position(|item| item.boundary_rom == nearest.boundary_rom)
                        .unwrap()
                        + 1;
                    print!(
                        " | held-out expected=0x{expected:08x} nearest=0x{:08x} distance=0x{:x} rank={rank}/{} score={}",
                        nearest.boundary_rom,
                        nearest.boundary_rom.abs_diff(expected),
                        transitions.len(),
                        nearest.rank_score,
                    );
                }
            }
            println!();
            transitions.sort_by(|left, right| {
                right
                    .code_to_data_score
                    .cmp(&left.code_to_data_score)
                    .then_with(|| left.boundary_rom.cmp(&right.boundary_rom))
            });
            print!("    code-to-data:");
            for item in transitions.iter().take(8) {
                print!(
                    " 0x{:08x}={}({}/{}/{})",
                    item.boundary_rom,
                    item.code_to_data_score,
                    item.structured_control_drop_per_mille,
                    item.pointer_rise_per_mille,
                    item.zero_word_rise_per_mille,
                );
            }
            if let Some(expected) = expected_boundary {
                if let Some(nearest) = transitions
                    .iter()
                    .min_by_key(|item| item.boundary_rom.abs_diff(expected))
                {
                    let rank = transitions
                        .iter()
                        .position(|item| item.boundary_rom == nearest.boundary_rom)
                        .unwrap()
                        + 1;
                    print!(
                        " | held-out nearest=0x{:08x} distance=0x{:x} rank={rank}/{} score={}",
                        nearest.boundary_rom,
                        nearest.boundary_rom.abs_diff(expected),
                        transitions.len(),
                        nearest.code_to_data_score,
                    );
                }
            }
            println!();
        }
        println!();
    }
    Ok(())
}

fn usage() -> String {
    "usage: gate_regions <rom> [evidence.toml]".to_string()
}
