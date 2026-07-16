//! B2 gate runner: Phase 4 (delay-slot-aware CFG) + Phase 5 (recursive-
//! descent owner partitioning) + Phase 6's bounded HI/LO indirect resolver
//! (`resolve::build_cfg_closed`) over the real OoT, NW4E, and NWXE ROMs'
//! resident `boot` banks. Every bank is seeded with ONLY the header
//! entrypoint --
//! the resident C entry each IPL3 stub `jr`s to is now recovered
//! mechanically, no hand-supplied "main entry" address. Graded against:
//!
//! - OoT's real linked `boot`-bank function boundaries (mechanically
//!   pulled from the decomp's own linker map; never hand-curated -- see
//!   `testdata/oot_boot_functions.csv` and the extraction script cited in
//!   its sibling doc comment in `grade_oot_functions.rs`). Gate requires
//!   `wrong == 0`.
//! - aki-recomp's hand-fixed NW4E `symbol_addrs.txt` rungs (the
//!   "grind-collapse" measure): how many of the ~36 rows this crate
//!   recovers mechanically, with zero LLM/human input.
//! - WM2000/NWXE's complete resident function extents, mechanically
//!   extracted from aki-recomp's generated `syms/dump.toml`. NWXE is the
//!   third-ROM generalization key: its symbols grade output only and never
//!   seed discovery.
//!
//! Same posture as `gate_b1`: this binary is the auditable "did the gate
//! actually pass on real bytes" artifact; the unit tests in `cfg.rs` and
//! `partition.rs` are the fast synthetic-data correctness proof for the
//! mechanism itself.

use fn64_discover::banks::{self, DescriptorTableShape};
use fn64_discover::grade_nw4e_symbols::{grade_grind_collapse, parse_symbol_addrs};
use fn64_discover::grade_nwxe_functions::{
    grade_functions as grade_nwxe_functions, parse_function_csv as parse_nwxe_function_csv,
};
use fn64_discover::grade_oot_functions::{grade_functions, parse_function_csv};
use fn64_discover::partition::{partition, same_bank_overlaps, Owner};
use fn64_discover::resolve::build_cfg_closed;
use fn64_discover::{run_discovery, DescriptorTableInput};
use std::collections::BTreeMap;
use std::path::Path;

const OOT_ROM: &str = "/Users/jer/Downloads/Legend of Zelda, The - Ocarina of Time (USA).z64";
const OOT_BOOT_FUNCTIONS_CSV: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/testdata/oot_boot_functions.csv"
);
/// The decomp's own linker map places `.boot`'s real code region
/// `[0x80000400, 0x80006230)` -- see `grade_oot_functions.rs`'s doc comment
/// and the extraction script for provenance; used only as the CFG scan's
/// upper bound (a cited external fact about where THIS run's code region
/// ends, not a discovered value).
const OOT_BOOT_CODE_END: u32 = 0x8000_6230;

const NW4E_ROM: &str = "/Users/jer/Code/aki-recomp/games/NW4E/nomercy.z64";
/// NW4E's `overlays.json` `resident.main_entry_vram` -- a prior
/// byte-verified RE fact (scan.py section 2). It is NO LONGER a pipeline
/// input: Phase 6's bounded HI/LO resolver ([`build_cfg_closed`]) now
/// recovers this exact address mechanically from the IPL3 boot stub's
/// `lui $t2,0x8000 ; addiu $t2,$t2,0x0460 ; jr $t2` construction. It is kept
/// here ONLY as a grading assertion -- proof the mechanical resolution
/// reproduces the hand-verified value -- never as a seed.
const NW4E_MAIN_ENTRY_VRAM: u32 = 0x8000_0460;
const NW4E_SYMBOL_ADDRS: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/testdata/nw4e_hand_fixed_symbol_addrs.txt"
);

const NWXE_ROM: &str = "/Users/jer/Code/aki-recomp/games/NWXE/wm2000.z64";
const NWXE_MD5: &str = "d9030ca30e4d1af805acce1bfed988cc";
const NWXE_MAIN_ENTRY_VRAM: u32 = 0x8000_0460;
const NWXE_BOOT_FUNCTIONS_CSV: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/testdata/nwxe_boot_functions.csv"
);
/// NWXE's key covers all 847 resident functions, while entrypoint-only
/// traversal currently reaches 28 owners before Phase 3 function-pointer
/// harvesting. Three percent therefore requires at least 26 answer-key
/// starts, in addition to OoT's stricter correctness condition `wrong == 0`.
/// This is stronger than NW4E's current coverage gate (0/36 is explicitly
/// allowed until Phase 3) without hiding NWXE's large honest-open frontier.
const NWXE_MIN_MATCHED_FRACTION: f64 = 0.03;

fn nw4e_descriptor_table_shape() -> DescriptorTableShape {
    DescriptorTableShape {
        table_rom_offset: 0x0539a0,
        record_count: 5,
        record_stride: 0x24,
        field_rom_start: 0x00,
        field_rom_end: 0x04,
        field_vram_dest: 0x08,
    }
}

fn main() {
    let mut exit_code = 0;

    println!("=== fn64-discover B2 gate ===\n");

    match run_oot_function_gate() {
        Ok(()) => println!(),
        Err(e) => {
            eprintln!("OoT function-boundary gate FAILED: {e}\n");
            exit_code = 1;
        }
    }

    match run_nw4e_grind_collapse_gate() {
        Ok(()) => println!(),
        Err(e) => {
            eprintln!("NW4E grind-collapse gate FAILED: {e}\n");
            exit_code = 1;
        }
    }

    match run_nwxe_function_gate() {
        Ok(()) => println!(),
        Err(e) => {
            eprintln!("NWXE function-boundary gate FAILED: {e}\n");
            exit_code = 1;
        }
    }

    std::process::exit(exit_code);
}

/// Build the boot bank's CFG + partition from the real OoT ROM, seeding
/// ONLY the header entrypoint and letting Phase 6's bounded HI/LO resolver
/// ([`build_cfg_closed`]) recover the resident C entry (`bootproc` at
/// 0x80000498) mechanically from the IPL3 stub's `lui/addiu -> jr $t2`
/// construction -- no hand-supplied "main entry" address. Phase 3 candidate
/// harvesting is still unimplemented, so functions reached only via function
/// pointers remain `open`; that is a reported limitation, not a hidden one.
fn run_oot_function_gate() -> Result<(), String> {
    let rom_bytes = std::fs::read(OOT_ROM).map_err(|e| format!("reading {OOT_ROM}: {e}"))?;
    let (rom, db) =
        run_discovery(&rom_bytes, None).map_err(|e| format!("normalizing OoT ROM: {e}"))?;
    println!("OoT ROM: {} bytes, sha256={}", rom.len(), rom.sha256);

    let boot_mapping = db
        .proven_rom_mappings()
        .into_iter()
        .find(|f| matches!(f, fn64_discover::Fact::RomMapping { bank, .. } if bank == banks::BOOT_BANK))
        .ok_or("boot bank not proven")?;
    let (rom_start, va_start) = match boot_mapping {
        fn64_discover::Fact::RomMapping {
            rom_start,
            va_start,
            ..
        } => (*rom_start, *va_start),
        _ => unreachable!(),
    };

    let code_len = (OOT_BOOT_CODE_END - va_start) as usize;
    let bank_bytes = &rom.bytes[rom_start as usize..rom_start as usize + code_len];
    let entrypoint = rom.header.entry_point;
    println!(
        "  boot bank: rom_start=0x{rom_start:x} va_start=0x{va_start:x} code_end=0x{OOT_BOOT_CODE_END:x} entrypoint=0x{entrypoint:x}"
    );

    let (cfg, resolved) = build_cfg_closed("boot", bank_bytes, va_start, &[entrypoint]);
    let part = partition(&cfg);
    let overlaps = same_bank_overlaps(&part, &cfg);

    println!(
        "  Phase 6 resolved {} bounded indirect target(s): {}",
        resolved.len(),
        resolved
            .iter()
            .map(|r| format!("0x{:x}->0x{:x}", r.site_pc, r.target))
            .collect::<Vec<_>>()
            .join(", ")
    );
    println!(
        "  CFG: {} blocks, {} direct calls, {} tail transfers, {} indirect sites",
        cfg.blocks.len(),
        cfg.direct_calls.len(),
        cfg.tail_transfers.len(),
        cfg.indirect_sites.len()
    );
    println!(
        "  Partition: {} owners, {} ambiguous blocks, {} unowned blocks, {} same-bank overlaps",
        part.owners.len(),
        part.ambiguous.len(),
        part.unowned.len(),
        overlaps.len()
    );
    if !overlaps.is_empty() {
        return Err(format!(
            "same-bank owner overlaps must be zero, got {}: {overlaps:?}",
            overlaps.len()
        ));
    }

    let csv = std::fs::read_to_string(OOT_BOOT_FUNCTIONS_CSV)
        .map_err(|e| format!("reading {OOT_BOOT_FUNCTIONS_CSV}: {e}"))?;
    let parsed = parse_function_csv(&csv);
    if !parsed.skipped.is_empty() {
        println!(
            "  (skipped {} malformed answer-key rows)",
            parsed.skipped.len()
        );
    }

    // A proven `jal` target is a machine-checkable interior callable entry;
    // an owner rooted there inside a coarser answer-key function is correct
    // finer-grained discovery, not a mis-split (see grade_oot_functions).
    let jal_targets: fn64_discover::grade_oot_functions::JalTargets =
        cfg.direct_calls.iter().map(|(_, t)| *t).collect();
    let report = grade_functions(
        &part.owners,
        &parsed.functions,
        OOT_BOOT_CODE_END,
        &jal_targets,
    );
    println!(
        "  OoT function-boundary grade: total={} matched_exact={} matched_coarse={} interior_entries={} open={} wrong={}",
        report.total, report.matched_exact, report.matched_coarse, report.interior_entries, report.open, report.wrong
    );
    println!(
        "    matched%={:.1}% open%={:.1}%",
        report.matched_fraction() * 100.0,
        (report.open as f64 / report.total as f64) * 100.0
    );
    if report.wrong != 0 {
        let wrong_examples: Vec<_> = report
            .per_function
            .iter()
            .filter(|(_, g)| {
                matches!(
                    g,
                    fn64_discover::grade_oot_functions::FunctionGrade::WrongSplit { .. }
                )
            })
            .take(5)
            .collect();
        return Err(format!(
            "gate requires wrong=0, got {} (examples: {wrong_examples:?})",
            report.wrong
        ));
    }

    Ok(())
}

/// Build the NW4E boot bank's CFG from the real ROM (entrypoint-only root
/// set, same honest limitation as the OoT gate above) plus each overlay
/// bank's own descriptor-table-proven mapping, then diff owners against
/// the hand-fixed rungs. Reports the recovery count; does not require it
/// to be 36/36 (Phase 3/6 aren't implemented yet), matching the design
/// doc's "honest limit" -- what's gated here is that the mechanism runs
/// end-to-end on real bytes and reports a real, non-fabricated count.
fn run_nw4e_grind_collapse_gate() -> Result<(), String> {
    if !Path::new(NW4E_ROM).exists() {
        return Err(format!("{NW4E_ROM} not found"));
    }
    let rom_bytes = std::fs::read(NW4E_ROM).map_err(|e| format!("reading {NW4E_ROM}: {e}"))?;
    let shape = nw4e_descriptor_table_shape();
    let descriptor: DescriptorTableInput = (shape, |i: u32| format!("R{}", i + 1));
    let (rom, db) = run_discovery(&rom_bytes, Some(descriptor))
        .map_err(|e| format!("normalizing NW4E ROM: {e}"))?;
    println!("NW4E ROM: {} bytes, sha256={}", rom.len(), rom.sha256);

    let mut owners_by_bank: BTreeMap<Option<String>, Vec<Owner>> = BTreeMap::new();
    let mut total_overlaps = 0usize;

    for mapping in db.proven_rom_mappings() {
        let fn64_discover::Fact::RomMapping {
            bank,
            rom_start,
            rom_end,
            va_start,
            ..
        } = mapping
        else {
            continue;
        };
        let bank_bytes = &rom.bytes[*rom_start as usize..*rom_end as usize];
        // Seed ONLY the header entrypoint. The IPL3 boot stub ends in an
        // indirect `jr $t2` to the resident C entry; Phase 6's bounded HI/LO
        // resolver ([`build_cfg_closed`]) now recovers that target
        // mechanically, so the `main_entry_vram` this gate previously
        // hand-seeded is no longer a pipeline input. Overlay banks have no
        // proven candidate root yet (Phase 3 unimplemented) -- an honest
        // empty seed still produces a valid (possibly empty) CFG.
        let seed: Vec<u32> = if bank == banks::BOOT_BANK {
            vec![rom.header.entry_point]
        } else {
            vec![]
        };
        let (cfg, resolved) = build_cfg_closed(bank, bank_bytes, *va_start, &seed);
        if bank == banks::BOOT_BANK {
            // Prove the mechanical resolution reproduced the hand-verified
            // resident entry address (grading assertion, not a seed).
            let recovered_entry = resolved.iter().any(|r| r.target == NW4E_MAIN_ENTRY_VRAM);
            println!(
                "  resident boot stub: Phase 6 resolved {} target(s); main entry 0x{NW4E_MAIN_ENTRY_VRAM:x} recovered mechanically: {recovered_entry}",
                resolved.len()
            );
            if !recovered_entry {
                return Err(format!(
                    "Phase 6 failed to mechanically recover the resident entry 0x{NW4E_MAIN_ENTRY_VRAM:x} (resolved: {resolved:?})"
                ));
            }
        }
        let part = partition(&cfg);
        let overlaps = same_bank_overlaps(&part, &cfg);
        total_overlaps += overlaps.len();

        // Rungs use `None` for the always-resident bank and the raw
        // `segment:` name (e.g. "R4_text") for overlays -- map our bank
        // name to that same convention for the lookup key.
        let key = if bank == banks::BOOT_BANK {
            None
        } else {
            Some(format!("{bank}_text"))
        };
        owners_by_bank.insert(key, part.owners);
    }

    if total_overlaps != 0 {
        return Err(format!(
            "same-bank owner overlaps must be zero across all banks, got {total_overlaps}"
        ));
    }

    let text = std::fs::read_to_string(NW4E_SYMBOL_ADDRS)
        .map_err(|e| format!("reading {NW4E_SYMBOL_ADDRS}: {e}"))?;
    let parsed = parse_symbol_addrs(&text);
    if !parsed.skipped.is_empty() {
        return Err(format!(
            "{} unparsed rows in hand-fixed symbol_addrs.txt: {:?}",
            parsed.skipped.len(),
            parsed.skipped
        ));
    }

    let report = grade_grind_collapse(&owners_by_bank, &parsed.rungs);
    println!(
        "  NW4E grind-collapse: total_rungs={} recovered={} partial={} not_recovered={}",
        report.total_rungs, report.recovered, report.partial, report.not_recovered
    );
    println!("    recovered%={:.1}%", report.recovered_fraction() * 100.0);
    if report.recovered == 0 {
        // Honest, expected result at B2's current scope, not a failure:
        // every one of these 36 rungs is either (a) in an overlay bank,
        // where this gate seeds zero candidate roots because Phase 3
        // (candidate harvesting) is not implemented yet, or (b) in the
        // resident bank but reached only via a function pointer / thread
        // registration this crate cannot yet resolve (Phase 6, also not
        // implemented). Verified directly: none of the 8 resident-bank
        // rungs fall inside any block this run's recursive descent
        // reaches at all. Reporting 0/36 truthfully is exactly the
        // "honest limit" the design doc requires -- fabricating a
        // recovery here would be the guessed-symbol-file failure mode
        // this crate exists to prevent.
        println!(
            "    (0 recovered is expected at B2's scope: these rungs need Phase 3/6, not yet implemented)"
        );
    }

    Ok(())
}

/// Grade WM2000/NWXE as an independent third ROM. Only the normalized
/// header entrypoint seeds the CFG. The expected resident entry and symbol
/// catalog are assertions over the resulting output, never discovery input.
fn run_nwxe_function_gate() -> Result<(), String> {
    if !Path::new(NWXE_ROM).exists() {
        return Err(format!("{NWXE_ROM} not found"));
    }
    let rom_bytes = std::fs::read(NWXE_ROM).map_err(|e| format!("reading {NWXE_ROM}: {e}"))?;
    let (rom, db) =
        run_discovery(&rom_bytes, None).map_err(|e| format!("normalizing NWXE ROM: {e}"))?;
    println!(
        "NWXE ROM: {} bytes, md5={}, sha256={}",
        rom.len(),
        rom.md5,
        rom.sha256
    );
    if rom.md5 != NWXE_MD5 {
        return Err(format!(
            "answer key is bound to canonical MD5 {NWXE_MD5}, got {}",
            rom.md5
        ));
    }

    let boot_mapping = db
        .proven_rom_mappings()
        .into_iter()
        .find(|fact| {
            matches!(fact, fn64_discover::Fact::RomMapping { bank, .. } if bank == banks::BOOT_BANK)
        })
        .ok_or("boot bank not proven")?;
    let (rom_start, rom_end, va_start, va_end) = match boot_mapping {
        fn64_discover::Fact::RomMapping {
            rom_start,
            rom_end,
            va_start,
            va_end,
            ..
        } => (*rom_start, *rom_end, *va_start, *va_end),
        _ => unreachable!(),
    };
    let bank_bytes = rom
        .bytes
        .get(rom_start as usize..rom_end as usize)
        .ok_or("header-derived boot-copy interval falls outside normalized ROM")?;
    let entrypoint = rom.header.entry_point;
    println!(
        "  boot bank: rom=[0x{rom_start:x},0x{rom_end:x}) va=[0x{va_start:x},0x{va_end:x}) entrypoint=0x{entrypoint:x}"
    );

    let (cfg, resolved) = build_cfg_closed("boot", bank_bytes, va_start, &[entrypoint]);
    let recovered_entry = resolved
        .iter()
        .any(|target| target.target == NWXE_MAIN_ENTRY_VRAM);
    println!(
        "  resident boot stub: Phase 6 resolved {} target(s): {}; main entry 0x{NWXE_MAIN_ENTRY_VRAM:x} recovered mechanically: {recovered_entry}",
        resolved.len(),
        resolved
            .iter()
            .map(|target| format!("0x{:x}->0x{:x}", target.site_pc, target.target))
            .collect::<Vec<_>>()
            .join(", ")
    );
    if !recovered_entry {
        return Err(format!(
            "Phase 6 failed to mechanically recover resident entry 0x{NWXE_MAIN_ENTRY_VRAM:x} from header-entry-only seed (resolved: {resolved:?})"
        ));
    }

    let part = partition(&cfg);
    let overlaps = same_bank_overlaps(&part, &cfg);
    println!(
        "  CFG: {} blocks, {} direct calls, {} tail transfers, {} indirect sites",
        cfg.blocks.len(),
        cfg.direct_calls.len(),
        cfg.tail_transfers.len(),
        cfg.indirect_sites.len()
    );
    println!(
        "  Partition: {} owners, {} ambiguous blocks, {} unowned blocks, {} same-bank overlaps",
        part.owners.len(),
        part.ambiguous.len(),
        part.unowned.len(),
        overlaps.len()
    );
    if !overlaps.is_empty() {
        return Err(format!(
            "same-bank owner overlaps must be zero, got {}: {overlaps:?}",
            overlaps.len()
        ));
    }

    let csv = std::fs::read_to_string(NWXE_BOOT_FUNCTIONS_CSV)
        .map_err(|e| format!("reading {NWXE_BOOT_FUNCTIONS_CSV}: {e}"))?;
    let parsed = parse_nwxe_function_csv(&csv);
    if !parsed.skipped.is_empty() {
        return Err(format!(
            "{} malformed NWXE answer-key rows: {:?}",
            parsed.skipped.len(),
            parsed.skipped
        ));
    }
    let jal_targets: fn64_discover::grade_nwxe_functions::JalTargets =
        cfg.direct_calls.iter().map(|(_, target)| *target).collect();
    let report = grade_nwxe_functions(&part.owners, &parsed.functions, &jal_targets);
    println!(
        "  NWXE function-boundary grade: total={} matched_exact={} matched_coarse={} interior_entries={} open={} wrong={}",
        report.total,
        report.matched_exact,
        report.matched_coarse,
        report.interior_entries,
        report.open,
        report.wrong
    );
    println!(
        "    matched%={:.1}% open%={:.1}% (required matched% >= {:.1}%, wrong=0)",
        report.matched_fraction() * 100.0,
        (report.open as f64 / report.total as f64) * 100.0,
        NWXE_MIN_MATCHED_FRACTION * 100.0
    );
    if report.wrong != 0 {
        let examples: Vec<_> = report
            .per_function
            .iter()
            .filter(|(_, grade)| {
                matches!(
                    grade,
                    fn64_discover::grade_nwxe_functions::FunctionGrade::WrongSplit { .. }
                )
            })
            .take(5)
            .collect();
        return Err(format!(
            "gate requires wrong=0, got {} (examples: {examples:?})",
            report.wrong
        ));
    }
    if report.matched_fraction() < NWXE_MIN_MATCHED_FRACTION {
        return Err(format!(
            "gate requires matched fraction >= {:.1}%, got {:.1}%",
            NWXE_MIN_MATCHED_FRACTION * 100.0,
            report.matched_fraction() * 100.0
        ));
    }

    Ok(())
}
