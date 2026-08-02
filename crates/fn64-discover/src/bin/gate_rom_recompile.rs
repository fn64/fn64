//! Whole-ROM CPU-recompilation gate for any ROM automatic discovery accepts.
//!
//! `gate_wm2000_recompile` certifies one hand-configured game: its bank count,
//! boot geometry, overlay family, and shard graph are constants. This gate
//! makes the same certification generic — every input comes from
//! `run_discovery_auto`, so a ROM nobody has studied is admitted on exactly
//! the terms a studied one is.
//!
//! The certification is: every proven physical bank is packed with
//! digest-bound block geometry, emitted as Rust through the sparse emitter,
//! compiled by a real `rustc`, and probed at arbitrary guest PCs — plus the
//! execution-closure scoreboard, whose `unsupported` count is the release
//! blocker `docs/DISCOVER-PLAN.md` names ("a full-game gate requires zero
//! `unsupported` destinations").
//!
//! This is a CPU-recompilation milestone, not a booting game: RSP audio and
//! RDP graphics are separate runtime subsystems, and the boot harness needs
//! host-binding recognizers this gate never consults.
//!
//! Environment:
//! * `FN64_DISCOVER_ROM` — the ROM to certify (required).
//! * `FN64_RECOMPILE_REPORT` — optional content-free JSON receipt path.

use fn64_discover::block_pack::{
    emit_materialized_bank_runner, emit_validated_block_pack_v2, materialize_block_pack,
    BlockPackV1, MaterializedPackedBank,
};
use fn64_discover::closure::{
    classified_destinations, scoreboard, ClosureScoreboard, DestinationClass,
};
use fn64_discover::dense_aot_pack::DENSE_AOT_SHARD_BYTES;
use fn64_discover::facts::{FunctionEntryEvidence, ProofState};
use fn64_discover::snapshot::{
    compose_materialized_banks_validated_v2_with_limits, MaterializedBankInput,
    MultiBankCompositionLimits,
};
use fn64_discover::{required_env_path, Fact, FactDb, RomAddressSpace};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::Command;

const MAX_ROM_BYTES: u64 = 128 * 1024 * 1024;

/// Maximum emitted-code words a single generated-runner compile unit may
/// hold, derived from the existing 64 KiB dense-AOT shard convention
/// (`DENSE_AOT_SHARD_BYTES / 4` = 16,384 words). rustc's compile time on the
/// generated dispatch match is superlinear in single-unit size: one 111 MB /
/// 1,886,522-line translation unit (Clay Fighter, unsharded) had not
/// finished `rustc -O` after 32 minutes. Splitting emission at this bound
/// keeps every rustc invocation small enough to finish in practice, without
/// changing which words are emitted or what the harness proves.
const MAX_COMPILE_UNIT_WORDS: usize = (DENSE_AOT_SHARD_BYTES / 4) as usize;

struct PhysicalBank {
    bank: String,
    rom_start: u32,
    rom_end: u32,
    va_start: u32,
    va_end: u32,
}

/// Content-free certification receipt. No ROM bytes, no local paths.
#[derive(Debug, Serialize)]
struct RecompileReportV1 {
    schema: &'static str,
    schema_version: u32,
    normalized_rom_sha256: String,
    internal_name: String,
    banks: usize,
    pack_blocks: usize,
    pack_words: usize,
    emitted_code_bytes: usize,
    exact_aot_bytes: u64,
    block_aot_bytes: u64,
    dynamic_mips_destinations: u64,
    unsupported_destinations: u64,
    total_destinations: u64,
    runner_sha256: String,
    rustc_compiles: bool,
    harness_runs: bool,
}

fn main() {
    if let Err(error) = run() {
        eprintln!("gate_rom_recompile: FAILED: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let rom_path = required_env_path("FN64_DISCOVER_ROM", "a .z64 ROM to recompile")?;
    let metadata =
        std::fs::metadata(&rom_path).map_err(|error| format!("reading ROM metadata: {error}"))?;
    if metadata.len() > MAX_ROM_BYTES {
        return Err(format!(
            "ROM input is {} bytes, exceeding the {MAX_ROM_BYTES}-byte limit",
            metadata.len()
        ));
    }
    let rom_bytes = std::fs::read(&rom_path).map_err(|error| format!("reading ROM: {error}"))?;
    let discovery = fn64_discover::run_discovery_auto(&rom_bytes)
        .map_err(|error| format!("automatic discovery rejected the ROM: {error:?}"))?;
    let rom = discovery.rom;
    let facts = discovery.facts;

    let physical = physical_banks(&facts)?;
    if physical.is_empty() {
        return Err("discovery proved no physical bank to recompile".to_owned());
    }
    // The resident bank is the one containing the ROM entry point; generation
    // topology needs it named, and a generic ROM cannot assume "boot".
    let resident = physical
        .iter()
        .find(|bank| {
            rom.header.entry_point >= bank.va_start && rom.header.entry_point < bank.va_end
        })
        .map(|bank| bank.bank.clone())
        .unwrap_or_else(|| physical[0].bank.clone());

    let mut bank_bytes = Vec::with_capacity(physical.len());
    let mut bank_roots = Vec::with_capacity(physical.len());
    for bank in &physical {
        let bytes = rom
            .bytes
            .get(bank.rom_start as usize..bank.rom_end as usize)
            .ok_or_else(|| {
                format!(
                    "{} ROM interval [{:#x},{:#x}) is outside the normalized image",
                    bank.bank, bank.rom_start, bank.rom_end
                )
            })?;
        bank_bytes.push(bytes);
        bank_roots.push(callable_roots(&facts, bank));
    }
    let inputs: Vec<MaterializedBankInput<'_>> = physical
        .iter()
        .enumerate()
        .map(|(index, bank)| MaterializedBankInput {
            bank: &bank.bank,
            va_start: bank.va_start,
            bytes: bank_bytes[index],
            seed_roots: &bank_roots[index],
        })
        .collect();

    // Generic composition, not the catalog-bound generation fixed point: the
    // latter requires at least one overlay generation, which a single-bank
    // ROM does not have. This is the same validated multi-bank composition
    // `gate_rom_rebuild` proves across the corpus, and it yields the same
    // `ValidatedComposedSnapshotsV2` the block-pack emitter consumes.
    let composed = compose_materialized_banks_validated_v2_with_limits(
        &rom,
        &facts,
        &inputs,
        MultiBankCompositionLimits::default(),
    )
    .map_err(|error| format!("composing snapshot banks: {error}"))?;
    let snapshots = composed.snapshots();
    let board = scoreboard(snapshots);

    // Each snapshot packs its own bank; the whole-ROM pack is their union in
    // stable bank order so the portable JSON is generation-independent.
    let mut whole_pack = BlockPackV1 {
        schema_version: fn64_discover::block_pack::BLOCK_PACK_SCHEMA_V2,
        normalized_rom_sha256: rom.sha256.clone(),
        banks: Vec::with_capacity(snapshots.len()),
    };
    for (index, snapshot) in snapshots.iter().enumerate() {
        let pack = emit_validated_block_pack_v2(&composed, index, &rom).map_err(|error| {
            format!(
                "emitting block pack for {}: {error:?}",
                snapshot.banks[0].input.bank
            )
        })?;
        if pack.banks.len() != 1 {
            return Err(format!(
                "one-bank snapshot for {} emitted {} pack banks",
                snapshot.banks[0].input.bank,
                pack.banks.len()
            ));
        }
        whole_pack.banks.extend(pack.banks);
    }
    whole_pack
        .banks
        .sort_by(|left, right| left.bank.cmp(&right.bank));
    let pack_json = serde_json::to_vec(&whole_pack)
        .map_err(|error| format!("serializing whole-ROM BlockPack: {error}"))?;
    let materialized = materialize_block_pack(&whole_pack, &rom)
        .map_err(|error| format!("materializing whole-ROM BlockPack: {error}"))?;

    let total_blocks: usize = materialized.iter().map(|bank| bank.blocks.len()).sum();
    let total_words: usize = materialized
        .iter()
        .flat_map(|bank| &bank.blocks)
        .map(|block| block.words.len())
        .sum();

    println!("=== whole-ROM CPU recompilation ===");
    println!("rom_sha256={}", rom.sha256);
    println!("internal_name={}", rom.header.name);
    println!("resident_bank={resident}");
    println!("composed_banks={}", physical.len());
    for (snapshot, bank) in snapshots.iter().zip(materialized.iter()) {
        let bank_board = scoreboard(std::slice::from_ref(snapshot));
        let words: usize = bank.blocks.iter().map(|block| block.words.len()).sum();
        print_scoreboard(
            &format!(
                "bank={} bank_id={:#018x} pack_blocks={} pack_words={words}",
                bank.bank,
                bank.bank_id,
                bank.blocks.len(),
            ),
            &bank_board,
        );
    }
    print_scoreboard("whole_rom", &board);
    let exact = board.tally(DestinationClass::ExactAot);
    let block = board.tally(DestinationClass::BlockAot);
    println!(
        "HEADLINE unsupported={} total_recompiled_exact_plus_block_aot_bytes={}",
        board.unsupported,
        exact.bytes + block.bytes
    );
    println!(
        "whole-ROM BlockPack v{}: blocks={total_blocks} words={total_words} emitted_code_bytes={} portable_json_bytes={}",
        whole_pack.schema_version,
        total_words * 4,
        pack_json.len(),
    );
    let unsupported: Vec<String> = classified_destinations(snapshots)
        .into_iter()
        .filter(|destination| destination.class() == DestinationClass::Unsupported)
        .map(|destination| format!("{:#010x}:{:?}", destination.va, destination.reason))
        .collect();
    println!("unsupported_punch_list=[{}]", unsupported.join(", "));

    let mut runners = Vec::with_capacity(materialized.len());
    for (index, bank) in materialized.iter().enumerate() {
        runners.push(emit_materialized_bank_runner(
            bank,
            &format!("run_rom_bank_{index}"),
        ));
    }
    let runner_sha256 = sha256_hex(runners.join("\n").as_bytes());
    let harness_report = compile_and_run_harness(&runners, &materialized)?;
    println!(
        "generated runners: banks={} sha256={runner_sha256} rustc_compiles=true harness_runs=true",
        runners.len(),
    );
    for line in harness_report.lines() {
        println!("runner: {line}");
    }
    println!(
        "scope=CPU recompilation milestone: all discovered code banks emitted, digest-verified, compiled, and arbitrary-PC probed"
    );
    println!(
        "not_a_booting_game=true (RSP audio and RDP graphics are separate runtime subsystems)"
    );

    if let Ok(path) = std::env::var("FN64_RECOMPILE_REPORT") {
        let report = RecompileReportV1 {
            schema: "fn64.rom-recompile-report.v1",
            schema_version: 1,
            normalized_rom_sha256: rom.sha256.clone(),
            internal_name: rom.header.name.clone(),
            banks: physical.len(),
            pack_blocks: total_blocks,
            pack_words: total_words,
            emitted_code_bytes: total_words * 4,
            exact_aot_bytes: exact.bytes,
            block_aot_bytes: block.bytes,
            dynamic_mips_destinations: board.tally(DestinationClass::DynamicMips).destinations,
            unsupported_destinations: board.unsupported,
            total_destinations: board.total_destinations,
            runner_sha256,
            rustc_compiles: true,
            harness_runs: true,
        };
        std::fs::write(
            &path,
            serde_json::to_string_pretty(&report)
                .map_err(|error| format!("serializing recompile report: {error}"))?,
        )
        .map_err(|error| format!("writing recompile report to {path}: {error}"))?;
    }

    if board.unsupported != 0 {
        return Err(format!(
            "{} unsupported destination(s) block a full-game gate",
            board.unsupported
        ));
    }
    Ok(())
}

fn print_scoreboard(label: &str, board: &ClosureScoreboard) {
    println!("{label}");
    for class in DestinationClass::ALL {
        let tally = board.tally(class);
        println!(
            "  {:<12} destinations={} bytes={}",
            class.label(),
            tally.destinations,
            tally.bytes
        );
    }
    println!("  total_destinations={}", board.total_destinations);
}

fn physical_banks(facts: &FactDb) -> Result<Vec<PhysicalBank>, String> {
    let mut banks = Vec::new();
    for fact in facts.proven_rom_mappings() {
        let Fact::RomMapping {
            bank,
            rom_space,
            rom_start,
            rom_end,
            va_start,
            va_end,
        } = fact
        else {
            unreachable!("proven_rom_mappings returned a non-mapping fact")
        };
        if *rom_space != RomAddressSpace::Physical {
            continue;
        }
        if rom_end.checked_sub(*rom_start) != va_end.checked_sub(*va_start) {
            return Err(format!(
                "physical bank {bank} has unequal ROM and VA extents"
            ));
        }
        banks.push(PhysicalBank {
            bank: bank.clone(),
            rom_start: *rom_start,
            rom_end: *rom_end,
            va_start: *va_start,
            va_end: *va_end,
        });
    }
    banks.sort_by(|left, right| left.bank.cmp(&right.bank));
    Ok(banks)
}

fn callable_roots(facts: &FactDb, bank: &PhysicalBank) -> Vec<u32> {
    let mut roots: BTreeSet<u32> = facts
        .proven_function_entries(&bank.bank)
        .into_iter()
        .collect();
    for fact in facts.facts() {
        let Fact::FunctionEntryClaim {
            target,
            evidence,
            proposed_state,
            ..
        } = fact
        else {
            continue;
        };
        if target.bank != bank.bank
            || target.pc < bank.va_start
            || target.pc >= bank.va_end
            || !matches!(
                proposed_state,
                ProofState::Candidate | ProofState::Supported | ProofState::Proven
            )
            || !matches!(
                evidence,
                FunctionEntryEvidence::DirectJal { .. }
                    | FunctionEntryEvidence::ResolvedJalr { .. }
                    | FunctionEntryEvidence::ExhaustiveIndirectCall { .. }
                    | FunctionEntryEvidence::TableEntry { .. }
                    | FunctionEntryEvidence::HandlerTablePointer { .. }
            )
        {
            continue;
        }
        roots.insert(target.pc);
    }
    roots.into_iter().collect()
}

/// One bounded compile unit: a contiguous run of whole banks (in `banks`
/// order) whose combined emitted-code word count does not exceed
/// [`MAX_COMPILE_UNIT_WORDS`]. A bank is never split across units -- its
/// runner function is one opaque string from the sparse emitter -- so a
/// single bank larger than the bound still gets its own (oversized) unit
/// rather than breaking the bound guarantee for every other unit.
struct CompileUnit {
    /// Indices into `banks`/`runners`, in ascending order.
    bank_indices: Vec<usize>,
}

/// Partition banks into compile units by accumulated word count, in
/// deterministic `banks`-order. Grouping is geometry-derived (word counts
/// from the materialized pack, iterated in the caller's already-sorted bank
/// order) so two runs on the same ROM produce identical unit boundaries.
fn plan_compile_units(banks: &[MaterializedPackedBank]) -> Vec<CompileUnit> {
    let mut units = Vec::new();
    let mut current = Vec::new();
    let mut current_words = 0usize;
    for (index, bank) in banks.iter().enumerate() {
        let bank_words: usize = bank.blocks.iter().map(|block| block.words.len()).sum();
        if !current.is_empty() && current_words + bank_words > MAX_COMPILE_UNIT_WORDS {
            units.push(CompileUnit {
                bank_indices: std::mem::take(&mut current),
            });
            current_words = 0;
        }
        current.push(index);
        current_words += bank_words;
    }
    if !current.is_empty() {
        units.push(CompileUnit {
            bank_indices: current,
        });
    }
    units
}

/// Shared prelude every generated `.rs` file (shard or driver) opens with.
// `CpuException` is required here even though the unsharded gate's identical
// prelude omitted it: that omission was a latent bug masked by never being
// exercised end-to-end before (see task-1-report.md). The sparse emitter
// writes bare `CpuException::Variant` paths for trap/break/cop-unusable
// exception exits, so any compile unit containing such a block needs the
// import.
const GENERATED_PRELUDE: &str = "#![allow(clippy::all, unused)]\nuse fn64_recomp_rs::{BankId, BlockExit, BlockProgram, BlockRun, CodeBank, CodeSpan, CpuException, CpuFault, CpuFaultKind, ExecutionKey, GeneratedBankRunner, GuestPc, InstructionBudget, ProgramError, Rdram, RecompContext};\n\n";

/// Render one compile unit's source: the bank runner functions the sparse
/// emitter already produced for this unit's banks, plus a `pub` span table
/// and `CodeBank` constructor per bank so the driver crate (compiled and
/// linked separately) can call into them.
fn render_shard_source(
    unit: &CompileUnit,
    runners: &[String],
    banks: &[MaterializedPackedBank],
) -> String {
    let mut source = String::from(GENERATED_PRELUDE);
    for &index in &unit.bank_indices {
        source.push_str(&runners[index]);
        source.push('\n');
    }
    for &index in &unit.bank_indices {
        let bank = &banks[index];
        writeln!(source, "const SPANS_{index}: &[(u32, &[u32])] = &[")
            .expect("writing generated span table");
        for block in &bank.blocks {
            let words = block
                .words
                .iter()
                .map(|word| format!("{word:#010X}"))
                .collect::<Vec<_>>()
                .join(", ");
            writeln!(source, "    ({:#010X}, &[{words}]),", block.start_va)
                .expect("writing generated span");
        }
        writeln!(
            source,
            "];\n\npub fn code_bank_{index}() -> CodeBank {{\n    let id = BankId::new({:#018X});\n    let spans = SPANS_{index}.iter().map(|(va, words)| CodeSpan::new(id, GuestPc::new(*va), words.to_vec()).unwrap()).collect();\n    CodeBank::from_spans(id, spans).unwrap()\n}}\n",
            bank.bank_id
        )
        .expect("writing generated CodeBank constructor");
    }
    source
}

/// Render the driver binary's source: the arbitrary-PC probe harness plus
/// `main`, which registers every bank (calling into the shard crates by
/// their `--extern` names) and asserts the same probes and typed
/// `UnalignedPc` fault the unsharded gate always has.
fn render_driver_source(units: &[CompileUnit], banks: &[MaterializedPackedBank]) -> String {
    let mut source = String::from(GENERATED_PRELUDE);
    source.push_str(
        "fn probe(program: &BlockProgram, bank: BankId, pc: u32) -> BlockRun {\n    let mut storage = vec![0u8; 8 * 1024 * 1024];\n    let mut mem = Rdram::new(&mut storage);\n    let mut ctx = RecompContext::new();\n    ctx.set_r(29, 0x8070_0000);\n    program.run(ExecutionKey::new(bank, GuestPc::new(pc)), InstructionBudget::new(4096).unwrap(), &mut ctx, &mut mem)\n}\n\nfn main() {\n    let mut program = BlockProgram::new();\n",
    );
    for (unit_index, unit) in units.iter().enumerate() {
        for &index in &unit.bank_indices {
            writeln!(
                source,
                "    shard_{unit_index}::register_run_rom_bank_{index}(&mut program, shard_{unit_index}::code_bank_{index}()).unwrap();"
            )
            .expect("writing generated registration");
        }
    }
    for bank in banks {
        let Some(first) = bank.blocks.first() else {
            continue;
        };
        let middle = &bank.blocks[bank.blocks.len() / 2];
        for (kind, pc) in [("first", first.start_va), ("middle", middle.start_va)] {
            writeln!(
                source,
                "    let run = probe(&program, BankId::new({:#018X}), {pc:#010X});\n    assert!(run.instructions > 0 || matches!(run.exit, BlockExit::Fault(_)));\n    println!(\"bank={} bank_id={:#018x} {kind}_pc={pc:#010x} instructions={{}} exit={{:?}}\", run.instructions, run.exit);",
                bank.bank_id,
                bank.bank,
                bank.bank_id
            )
            .expect("writing generated arbitrary-PC probe");
        }
        // An unaligned PC must produce the typed architectural fault, never a
        // silent miss: the emitter's own soundness check.
        writeln!(
            source,
            "    let unaligned = probe(&program, BankId::new({:#018X}), {:#010X});\n    assert!(matches!(unaligned.exit, BlockExit::Fault(CpuFault {{ kind: CpuFaultKind::UnalignedPc, .. }})));\n    assert_eq!(unaligned.instructions, 0);\n    println!(\"bank={} unaligned_pc={:#010x} typed_fault=UnalignedPc\");",
            bank.bank_id,
            first.start_va | 1,
            bank.bank,
            first.start_va | 1,
        )
        .expect("writing generated unaligned probe");
    }
    source.push_str("}\n");
    source
}

fn compile_and_run_harness(
    runners: &[String],
    banks: &[MaterializedPackedBank],
) -> Result<String, String> {
    let executable_dir = std::env::current_exe()
        .map_err(|error| format!("finding gate executable: {error}"))?
        .parent()
        .ok_or("gate executable has no parent directory")?
        .to_path_buf();
    let deps = if executable_dir.ends_with("deps") {
        executable_dir
    } else {
        executable_dir.join("deps")
    };
    let rlib = current_recomp_rlib(&deps)?;
    let temp = std::env::temp_dir().join(format!("fn64-rom-recompile-{}", std::process::id()));
    std::fs::create_dir_all(&temp)
        .map_err(|error| format!("creating generated-runner temp directory: {error}"))?;

    // Split the generated code into bounded compile units (see
    // MAX_COMPILE_UNIT_WORDS) rather than one translation unit: rustc's
    // compile time on the whole-ROM dispatch match does not scale linearly,
    // and one unbounded unit does not finish on real ROMs.
    let units = plan_compile_units(banks);

    let mut shard_rlibs = Vec::with_capacity(units.len());
    for (unit_index, unit) in units.iter().enumerate() {
        let shard_source = render_shard_source(unit, runners, banks);
        let shard_source_path = temp.join(format!("shard_{unit_index}.rs"));
        std::fs::write(&shard_source_path, &shard_source)
            .map_err(|error| format!("writing shard {unit_index} source: {error}"))?;
        // Each shard only references `fn64_recomp_rs` (the emitted bank
        // runners and CodeBank constructors are self-contained); shards
        // never call into one another, so only the driver links them all.
        let shard_rlib_path = temp.join(format!("libshard_{unit_index}.rlib"));
        let mut command = Command::new("rustc");
        command
            .arg("--edition=2021")
            .arg("-O")
            .arg("--crate-type=rlib")
            .arg("--crate-name")
            .arg(format!("shard_{unit_index}"))
            .arg("--extern")
            .arg(format!("fn64_recomp_rs={}", rlib.display()))
            .arg("-L")
            .arg(&deps)
            .arg("-o")
            .arg(&shard_rlib_path)
            .arg(&shard_source_path);
        let output = command
            .output()
            .map_err(|error| format!("invoking rustc on shard {unit_index}: {error}"))?;
        if !output.status.success() {
            return Err(format!(
                "generated shard {unit_index} failed to compile: {}",
                String::from_utf8_lossy(&output.stderr)
                    .trim()
                    .replace('\n', " | ")
                    .chars()
                    .take(2000)
                    .collect::<String>()
            ));
        }
        shard_rlibs.push(shard_rlib_path);
    }

    let driver_source = render_driver_source(&units, banks);
    let driver_source_path = temp.join("generated_runner.rs");
    std::fs::write(&driver_source_path, &driver_source)
        .map_err(|error| format!("writing generated runner source: {error}"))?;
    let binary = temp.join("generated_runner");
    let mut command = Command::new("rustc");
    command
        .arg("--edition=2021")
        .arg("-O")
        .arg("--extern")
        .arg(format!("fn64_recomp_rs={}", rlib.display()));
    for (unit_index, shard_rlib_path) in shard_rlibs.iter().enumerate() {
        command
            .arg("--extern")
            .arg(format!("shard_{unit_index}={}", shard_rlib_path.display()));
    }
    command
        .arg("-L")
        .arg(&deps)
        .arg("-o")
        .arg(&binary)
        .arg(&driver_source_path);
    let output = command
        .output()
        .map_err(|error| format!("invoking rustc on the generated runner: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "generated runner failed to compile: {}",
            String::from_utf8_lossy(&output.stderr)
                .trim()
                .replace('\n', " | ")
                .chars()
                .take(2000)
                .collect::<String>()
        ));
    }
    let run = Command::new(&binary)
        .output()
        .map_err(|error| format!("running the generated runner: {error}"))?;
    if !run.status.success() {
        return Err(format!(
            "generated runner exited {}: {}",
            run.status,
            String::from_utf8_lossy(&run.stderr)
                .trim()
                .replace('\n', " | ")
        ));
    }
    let _ = std::fs::remove_dir_all(&temp);
    Ok(String::from_utf8_lossy(&run.stdout).trim().to_owned())
}

fn current_recomp_rlib(deps: &Path) -> Result<PathBuf, String> {
    let mut newest: Option<(std::time::SystemTime, PathBuf)> = None;
    let entries =
        std::fs::read_dir(deps).map_err(|error| format!("reading {}: {error}", deps.display()))?;
    for entry in entries.flatten() {
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        // "libfn64_recomp_rs" alone also matches
        // "libfn64_recomp_rs_codegen-*.rlib" (a sibling crate in the same
        // deps directory); require the hyphen that starts the hash suffix
        // so the codegen crate's rlib is never mistaken for the runtime
        // crate's, which -- because both can share a build-batch mtime --
        // was a nondeterministic wrong-crate pick depending on directory
        // iteration order.
        if !name.starts_with("libfn64_recomp_rs-") || !name.ends_with(".rlib") {
            continue;
        }
        let modified = entry
            .metadata()
            .and_then(|metadata| metadata.modified())
            .map_err(|error| format!("reading rlib metadata: {error}"))?;
        if newest
            .as_ref()
            .is_none_or(|(current, _)| modified > *current)
        {
            newest = Some((modified, path));
        }
    }
    newest
        .map(|(_, path)| path)
        .ok_or_else(|| format!("no libfn64_recomp_rs rlib in {}", deps.display()))
}

fn sha256_hex(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
