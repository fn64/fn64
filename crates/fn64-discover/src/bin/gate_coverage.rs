//! Coverage grade: emit the metric-ladder quantities
//! (docs/DISCOVER-PLAN.md) for each supplied ROM, straight from the real
//! discovery pipeline. Numbers are read out of the fact database that
//! `run_discovery` produces; nothing here is a per-ROM constant baked into the
//! engine. The per-ROM table geometry consumed as input is the same cited,
//! answer-key-free geometry the other gates use.
//!
//! This gate reports MEASURED coverage, which is not the same as PROOF. A
//! mapped or executable byte count says what evidence established for an
//! interval, not that the interval is authoritative for emission; the proof
//! gates (owner proof, block proof) remain the arbiters of that. Owner-proof
//! and pack lines therefore read `not_run` / `none` here: this gate runs the
//! generic pipeline (normalize -> boot/descriptor/load-table mapping ->
//! candidate harvest) and does not perform the game-specific per-bank interval
//! selection those later phases require.
//!
//! ROM paths come from named, declared environment variables. An unset var is
//! a loud skip line, never a silent omission:
//!   FN64_DISCOVER_NW4E_ROM  FN64_DISCOVER_NWXE_ROM  FN64_DISCOVER_OOT_ROM

use fn64_discover::coverage::{render_report, report, CoverageReport, RomIdentity};
use fn64_discover::{
    aki_reference, oot_reference, run_discovery, run_discovery_with_load_image_tables,
    DescriptorTableInput, FactDb, NormalizedRom,
};

const NW4E_ROM_VAR: &str = "FN64_DISCOVER_NW4E_ROM";
const NWXE_ROM_VAR: &str = "FN64_DISCOVER_NWXE_ROM";
const OOT_ROM_VAR: &str = "FN64_DISCOVER_OOT_ROM";

/// One ROM's discovery inputs. `descriptor` and `load_image_tables` are cited
/// table geometry, never answer keys; either may be empty.
enum RomInputs {
    /// Boot bank plus an optional AKI-family descriptor table.
    Descriptor(Option<DescriptorTableInput>),
    /// Boot bank plus explicitly located load-image tables (OoT).
    LoadImageTables(Vec<fn64_discover::banks::LoadImageTableInput>),
}

/// A ROM to grade: display label, the env var naming its path, and the factory
/// that yields its cited discovery inputs.
struct RomSpec {
    label: &'static str,
    var: &'static str,
    inputs: fn() -> RomInputs,
}

fn main() {
    println!("=== fn64-discover coverage grade ===");
    println!();

    // Deterministic ROM order, independent of environment iteration order.
    let roms = [
        RomSpec {
            label: "NW4E",
            var: NW4E_ROM_VAR,
            inputs: nw4e_inputs,
        },
        RomSpec {
            label: "NWXE",
            var: NWXE_ROM_VAR,
            inputs: nwxe_inputs,
        },
        RomSpec {
            label: "OoT",
            var: OOT_ROM_VAR,
            inputs: oot_inputs,
        },
    ];

    let mut failed = false;
    for RomSpec { label, var, inputs } in roms {
        match std::env::var_os(var) {
            None => {
                println!("skip {label}: {var} unset");
                println!();
            }
            Some(path) => {
                let path = path.to_string_lossy().into_owned();
                match grade_rom(label, &path, inputs()) {
                    Ok(lines) => {
                        for line in lines {
                            println!("{line}");
                        }
                        println!();
                    }
                    Err(error) => {
                        failed = true;
                        eprintln!("FAIL {label}: {error}");
                    }
                }
            }
        }
    }

    if failed {
        std::process::exit(1);
    }
}

fn grade_rom(label: &str, path: &str, inputs: RomInputs) -> Result<Vec<String>, String> {
    let rom_bytes = std::fs::read(path).map_err(|error| format!("reading {path}: {error}"))?;
    let (rom, db) = discover(&rom_bytes, inputs)
        .map_err(|error| format!("running discovery on {path}: {error}"))?;
    Ok(render_phase(label, &rom, &db))
}

fn discover(rom_bytes: &[u8], inputs: RomInputs) -> Result<(NormalizedRom, FactDb), String> {
    match inputs {
        RomInputs::Descriptor(descriptor) => {
            run_discovery(rom_bytes, descriptor).map_err(|error| error.to_string())
        }
        RomInputs::LoadImageTables(tables) => {
            run_discovery_with_load_image_tables(rom_bytes, None, &tables)
                .map_err(|error| error.to_string())
        }
    }
}

/// Render the coverage report for the phase this generic pipeline reaches:
/// phase 3 (candidate harvest) over a fact database that already carries the
/// phase-2 boot/descriptor/load-table mappings.
fn render_phase(label: &str, rom: &NormalizedRom, db: &FactDb) -> Vec<String> {
    let identity = RomIdentity {
        label: label.to_string(),
        sha256: rom.sha256.clone(),
    };
    let coverage: CoverageReport = report(rom.len(), db);
    render_report(&identity, "phase-3-harvest", &coverage, None)
}

fn nw4e_inputs() -> RomInputs {
    RomInputs::Descriptor(Some((
        aki_reference::NW4E_DESCRIPTOR_TABLE,
        aki_reference::nw4e_bank_name,
    )))
}

fn nwxe_inputs() -> RomInputs {
    RomInputs::Descriptor(None)
}

fn oot_inputs() -> RomInputs {
    RomInputs::LoadImageTables(oot_reference::oot_load_image_tables().to_vec())
}
