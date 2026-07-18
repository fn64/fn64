//! B2 gate runner: Phase 4 (delay-slot-aware CFG) + Phase 5 (recursive-
//! descent owner partitioning) + Phase 6's bounded HI/LO indirect resolver
//! (`resolve::build_cfg_closed_with_facts`) over the real OoT, NW4E, and NWXE ROMs'
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
use fn64_discover::block_pack::{
    emit_block_pack_v1, emit_materialized_bank_runner, materialize_block_pack,
    materialized_code_bank,
};
use fn64_discover::grade_nw4e_symbols::{grade_grind_collapse, parse_symbol_addrs};
use fn64_discover::grade_nwxe_functions::{
    grade_functions as grade_nwxe_functions, parse_function_csv as parse_nwxe_function_csv,
};
use fn64_discover::grade_oot_functions::{grade_functions, parse_function_csv};
use fn64_discover::partition::{partition, same_bank_overlaps, Owner};
use fn64_discover::resolve::{build_cfg_closed_with_facts, build_cfg_exploratory_with_candidates};
use fn64_discover::snapshot::{compose_materialized_bank_v1, MaterializedBankInput};
use fn64_discover::{run_discovery, DescriptorTableInput};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

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
/// input: Phase 6's bounded HI/LO resolver ([`build_cfg_closed_with_facts`]) now
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
/// ([`build_cfg_closed_with_facts`]) recover the resident C entry (`bootproc` at
/// 0x80000498) mechanically from the IPL3 stub's `lui/addiu -> jr $t2`
/// construction -- no hand-supplied "main entry" address. Phase 3 candidates
/// remain non-authoritative unless their merged proof state is `Proven`, so
/// unresolved function pointers remain `open`; that is a reported limitation,
/// not a hidden one.
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

    let (cfg, resolved) =
        build_cfg_closed_with_facts(&db, "boot", bank_bytes, va_start, &[entrypoint]);
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
/// to be 36/36 (the remaining table/value-set Phase 6 cases are not yet
/// implemented), matching the design
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
        // resolver ([`build_cfg_closed_with_facts`]) now recovers that target
        // mechanically, so the `main_entry_vram` this gate previously
        // hand-seeded is no longer a pipeline input. Overlay banks have no
        // proven candidate root yet. Phase 3's heuristic jal/prologue claims
        // deliberately remain Candidate/Supported, so an honest empty seed
        // still produces a valid (possibly empty) CFG.
        let seed: Vec<u32> = if bank == banks::BOOT_BANK {
            vec![rom.header.entry_point]
        } else {
            vec![]
        };
        let (cfg, resolved) =
            build_cfg_exploratory_with_candidates(&db, bank, bank_bytes, *va_start, &seed);
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
        println!(
            "  exploratory owner overlaps={total_overlaps} (candidate CFG only; exact admission remains unchanged)"
        );
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
    let unresolved = report
        .per_rung
        .iter()
        .filter_map(|(address, grade)| {
            matches!(
                grade,
                fn64_discover::grade_nw4e_symbols::RungGrade::NotRecovered
            )
            .then_some(format!("0x{address:08x}"))
        })
        .collect::<Vec<_>>();
    if !unresolved.is_empty() {
        println!("    unresolved rung addresses={unresolved:?}");
    }
    if report.recovered == 0 {
        // Honest, expected result at B2's current scope, not a failure:
        // every one of these 36 rungs is either (a) in an overlay bank with
        // no Proven table-entry claim (heuristic Phase 3 claims cannot become
        // partition roots), or (b) in the resident bank but reached only via
        // a function pointer / thread registration the remaining Phase 6
        // value-set work cannot yet resolve. Verified directly: none of the 8 resident-bank
        // rungs fall inside any block this run's recursive descent
        // reaches at all. Reporting 0/36 truthfully is exactly the
        // "honest limit" the design doc requires -- fabricating a
        // recovery here would be the guessed-symbol-file failure mode
        // this crate exists to prevent.
        println!(
            "    (0 recovered is expected: these rungs need Proven table entries or later Phase 6 value-set closure)"
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

    let input = || MaterializedBankInput {
        bank: banks::BOOT_BANK,
        va_start,
        bytes: bank_bytes,
        seed_roots: std::slice::from_ref(&entrypoint),
    };
    let snapshot = compose_materialized_bank_v1(&rom, &db, input())
        .map_err(|error| format!("composing NWXE program snapshot: {error}"))?;
    let canonical = serde_json::to_vec(&snapshot)
        .map_err(|error| format!("serializing NWXE program snapshot: {error}"))?;
    let snapshot_sha256 = format!("{:x}", Sha256::digest(&canonical));
    for run in 1..10 {
        let repeated = compose_materialized_bank_v1(&rom, &db, input())
            .map_err(|error| format!("composing NWXE program snapshot run {run}: {error}"))?;
        let bytes = serde_json::to_vec(&repeated)
            .map_err(|error| format!("serializing NWXE program snapshot run {run}: {error}"))?;
        if bytes != canonical {
            return Err(format!(
                "NWXE program snapshot serialization changed on run {}",
                run + 1
            ));
        }
    }
    let bank_snapshot = &snapshot.banks[0];
    let block_pack = emit_block_pack_v1(&snapshot, &rom)
        .map_err(|error| format!("emitting NWXE Block Pack: {error}"))?;
    let block_pack_json = serde_json::to_vec(&block_pack)
        .map_err(|error| format!("serializing NWXE Block Pack: {error}"))?;
    let block_pack_sha256 = format!("{:x}", Sha256::digest(&block_pack_json));
    let materialized_pack = materialize_block_pack(&block_pack, &rom)
        .map_err(|error| format!("materializing NWXE Block Pack: {error}"))?;
    let packed_words: usize = materialized_pack[0]
        .blocks
        .iter()
        .map(|block| block.words.len())
        .sum();
    let code_bank = materialized_code_bank(&materialized_pack[0])
        .map_err(|error| format!("admitting NWXE sparse code bank: {error}"))?;
    if code_bank.instruction_count() != packed_words {
        return Err(format!(
            "sparse catalog lost words: pack has {packed_words}, catalog has {}",
            code_bank.instruction_count()
        ));
    }
    let catalog_id = code_bank.id();
    let mut catalog = fn64_recomp_rs::CodeCatalog::new();
    catalog
        .register(code_bank)
        .map_err(|error| format!("registering NWXE sparse code bank: {error}"))?;
    for block in &materialized_pack[0].blocks {
        for (index, expected) in block.words.iter().copied().enumerate() {
            let pc = block.start_va + index as u32 * 4;
            let resolved = catalog
                .resolve(fn64_recomp_rs::ExecutionKey::new(
                    catalog_id,
                    fn64_recomp_rs::GuestPc::new(pc),
                ))
                .map_err(|error| format!("resolving packed NWXE word at {pc:#010x}: {error}"))?;
            if resolved.word != expected {
                return Err(format!(
                    "sparse catalog word mismatch at {pc:#010x}: expected {expected:#010x}, got {:#010x}",
                    resolved.word
                ));
            }
        }
    }
    let first_hole = materialized_pack[0].blocks.windows(2).find_map(|pair| {
        let left_end = pair[0].start_va + pair[0].words.len() as u32 * 4;
        (left_end < pair[1].start_va).then_some(left_end)
    });
    let hole = first_hole.ok_or("NWXE sparse pack unexpectedly has no gap to validate")?;
    if !matches!(
        catalog.resolve(fn64_recomp_rs::ExecutionKey::new(
            catalog_id,
            fn64_recomp_rs::GuestPc::new(hole),
        )),
        Err(fn64_recomp_rs::CpuFault {
            kind: fn64_recomp_rs::CpuFaultKind::UnmappedPc { .. },
            ..
        })
    ) {
        return Err(format!("sparse catalog admitted pack hole at {hole:#010x}"));
    }
    let sparse_runner = emit_materialized_bank_runner(&materialized_pack[0], "run_nwxe_boot");
    let sparse_runner_sha256 = format!("{:x}", Sha256::digest(sparse_runner.as_bytes()));
    compile_sparse_runner(&sparse_runner)?;
    let cfg = &bank_snapshot.closure.cfg;
    let part = &bank_snapshot.partition;
    let recovered_entry = bank_snapshot
        .closure
        .indirect
        .iter()
        .any(|resolution| resolution.targets.contains(&NWXE_MAIN_ENTRY_VRAM));
    println!(
        "  resident boot stub: Phase 6 analyzed {} indirect site(s): {}; main entry 0x{NWXE_MAIN_ENTRY_VRAM:x} recovered mechanically: {recovered_entry}",
        bank_snapshot.closure.indirect.len(),
        bank_snapshot.closure.indirect
            .iter()
            .filter(|resolution| !resolution.targets.is_empty())
            .map(|resolution| format!("0x{:x}->{:x?}", resolution.site_pc, resolution.targets))
            .collect::<Vec<_>>()
            .join(", ")
    );
    println!(
        "  BlockPack v{}: sha256={}, {} blocks / {} words, ROM digests reverified; sparse catalog re-resolved every word and rejected hole {:#010x}; sparse runner sha256={} (rustc accepted)",
        block_pack.schema_version,
        block_pack_sha256,
        materialized_pack[0].blocks.len(),
        packed_words,
        hole,
        sparse_runner_sha256,
    );
    if !recovered_entry {
        return Err(format!(
            "Phase 6 failed to mechanically recover resident entry 0x{NWXE_MAIN_ENTRY_VRAM:x} from header-entry-only seed (resolved: {:?})",
            bank_snapshot.closure.indirect
        ));
    }

    println!(
        "  ProgramSnapshot v{}: sha256={} (10/10 byte-identical), block proof={}/{} blocks ({} bytes), owner blockers={}",
        snapshot.schema_version,
        snapshot_sha256,
        bank_snapshot.block_proof.proven_blocks,
        bank_snapshot.block_proof.assessments.len(),
        bank_snapshot.block_proof.proven_bytes,
        serde_json::to_string(&bank_snapshot.blocker_histogram)
            .map_err(|error| format!("serializing NWXE blocker histogram: {error}"))?
    );
    let overlaps = same_bank_overlaps(part, cfg);
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

fn compile_sparse_runner(runner: &str) -> Result<(), String> {
    let executable_dir = std::env::current_exe()
        .map_err(|error| format!("finding B2 executable: {error}"))?
        .parent()
        .ok_or("B2 executable has no parent directory")?
        .to_path_buf();
    let deps = if executable_dir.ends_with("deps") {
        executable_dir
    } else {
        executable_dir.join("deps")
    };
    let rlib = current_recomp_rlib(&deps)?;
    let temp = std::env::temp_dir().join(format!("fn64-nwxe-b2-{}", std::process::id()));
    std::fs::create_dir_all(&temp)
        .map_err(|error| format!("creating sparse-runner gate directory: {error}"))?;
    let source_path = temp.join("runner.rs");
    let metadata_path = temp.join("runner.rmeta");
    let source = format!(
        "#![allow(clippy::all, unused)]\nuse fn64_recomp_rs::{{BankId, BlockExit, BlockProgram, BlockRun, CodeBank, CpuFault, CpuFaultKind, ExecutionKey, GeneratedBankRunner, GuestPc, InstructionBudget, ProgramError, Rdram, RecompContext}};\n\n{runner}"
    );
    std::fs::write(&source_path, source)
        .map_err(|error| format!("writing sparse-runner gate source: {error}"))?;
    let compile = Command::new(std::env::var("RUSTC").unwrap_or_else(|_| "rustc".into()))
        .arg("--edition=2021")
        .arg("--crate-type=lib")
        .arg(&source_path)
        .arg("--extern")
        .arg(format!("fn64_recomp_rs={}", rlib.display()))
        .arg("-L")
        .arg(format!("dependency={}", deps.display()))
        .arg("--emit=metadata")
        .arg("-o")
        .arg(&metadata_path)
        .output()
        .map_err(|error| format!("invoking rustc for sparse runner: {error}"))?;
    if !compile.status.success() {
        return Err(format!(
            "generated NWXE sparse runner did not compile:\nstdout: {}\nstderr: {}",
            String::from_utf8_lossy(&compile.stdout),
            String::from_utf8_lossy(&compile.stderr)
        ));
    }
    Ok(())
}

fn current_recomp_rlib(deps: &Path) -> Result<PathBuf, String> {
    std::fs::read_dir(deps)
        .map_err(|error| format!("reading target dependency directory: {error}"))?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| {
                    name.starts_with("libfn64_recomp_rs-") && name.ends_with(".rlib")
                })
        })
        .max_by_key(|path| {
            path.metadata()
                .and_then(|metadata| metadata.modified())
                .ok()
        })
        .ok_or_else(|| "fn64_recomp_rs rlib is missing beside B2 gate".to_string())
}
