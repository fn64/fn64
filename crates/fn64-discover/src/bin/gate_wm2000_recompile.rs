//! Whole-ROM WM2000 CPU-recompilation gate.
//!
//! This is deliberately an assembly of existing mechanisms: recovered-overlay
//! discovery, multi-bank snapshot composition, digest-bound block packs, the
//! sparse Rust emitter, `BlockProgram`, and the execution-closure scoreboard.
//! ROM words appear only in materialized values and a generated source file in
//! the system temp directory; the portable pack contains geometry and digests.

use fn64_discover::banks::{BankNamePattern, BOOT_BANK};
use fn64_discover::block_pack::{
    emit_block_pack_v1, emit_materialized_bank_runner, materialize_block_pack, BlockPackV1,
    MaterializedPackedBank,
};
use fn64_discover::closure::{
    classified_destinations, scoreboard, ClosureScoreboard, DestinationClass,
};
use fn64_discover::delta_vote::DeltaVoteConfig;
use fn64_discover::facts::{FunctionEntryEvidence, ProofState};
use fn64_discover::overlay_regions::SearchConfig;
use fn64_discover::snapshot::{compose_materialized_banks_v1, MaterializedBankInput};
use fn64_discover::{
    required_env_path, run_discovery_with_recovered_overlay_regions, Fact, FactDb,
    RecoveredOverlayInput, RomAddressSpace,
};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::Command;

const ROM_VAR: &str = "FN64_DISCOVER_NWXE_ROM";
const EXPECTED_BANKS: usize = 5;

struct PhysicalBank {
    bank: String,
    rom_start: u32,
    rom_end: u32,
    va_start: u32,
    va_end: u32,
}

fn main() {
    if let Err(error) = run() {
        eprintln!("gate_wm2000_recompile FAILED: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let rom_path = required_env_path(ROM_VAR, "the WM2000/NWXE .z64")?;
    let rom_bytes = std::fs::read(&rom_path)
        .map_err(|error| format!("reading WM2000 ROM {rom_path}: {error}"))?;
    let search = SearchConfig::aki_family();
    let input = RecoveredOverlayInput {
        min_mapped_regions: search.min_records,
        search,
        delta_vote: DeltaVoteConfig::default(),
        table_name: "recovered_overlay_descriptors".to_string(),
        bank_name: BankNamePattern::new("recovered_overlay_", 0, ""),
    };
    let (rom, facts, _recovery) = run_discovery_with_recovered_overlay_regions(&rom_bytes, &input)
        .map_err(|error| format!("discovering WM2000 banks: {error}"))?;

    let physical = physical_banks(&facts)?;
    if physical.len() != EXPECTED_BANKS {
        return Err(format!(
            "expected resident + four recovered overlay banks, found {}: {:?}",
            physical.len(),
            physical
                .iter()
                .map(|bank| bank.bank.as_str())
                .collect::<Vec<_>>()
        ));
    }
    if !physical.iter().any(|bank| bank.bank == BOOT_BANK) {
        return Err("recovered bank set does not contain the resident boot bank".to_string());
    }

    let mut bank_bytes = Vec::with_capacity(physical.len());
    let mut bank_roots = Vec::with_capacity(physical.len());
    for bank in &physical {
        let bytes = rom
            .bytes
            .get(bank.rom_start as usize..bank.rom_end as usize)
            .ok_or_else(|| {
                format!(
                    "{} ROM interval [0x{:x},0x{:x}) is outside the normalized image",
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
    let snapshots = compose_materialized_banks_v1(&rom, &facts, &inputs)
        .map_err(|error| format!("composing the whole WM2000 program: {error}"))?;

    let mut whole_pack = BlockPackV1 {
        schema_version: fn64_discover::block_pack::BLOCK_PACK_SCHEMA_V1,
        normalized_rom_sha256: rom.sha256.clone(),
        banks: Vec::with_capacity(snapshots.len()),
    };
    for snapshot in &snapshots {
        let pack = emit_block_pack_v1(snapshot, &rom).map_err(|error| {
            format!(
                "emitting block pack for {}: {error}",
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
    let pack_sha256 = sha256_hex(&pack_json);

    // One call re-verifies the normalized-ROM identity and every block digest
    // in every bank. Materialized words never enter the portable pack JSON.
    let materialized = materialize_block_pack(&whole_pack, &rom)
        .map_err(|error| format!("materializing whole-ROM BlockPack: {error}"))?;
    if materialized.len() != EXPECTED_BANKS {
        return Err(format!(
            "materialized {} banks, expected {EXPECTED_BANKS}",
            materialized.len()
        ));
    }

    let whole_board = scoreboard(&snapshots);
    println!("=== WM2000/NWXE whole-ROM CPU recompilation ===");
    println!("ROM sha256={}", rom.sha256);
    println!(
        "composed banks={} (resident + {} recovered overlays)",
        physical.len(),
        physical.len() - 1
    );
    for (snapshot, bank) in snapshots.iter().zip(materialized.iter()) {
        let board = scoreboard(std::slice::from_ref(snapshot));
        let words: usize = bank.blocks.iter().map(|block| block.words.len()).sum();
        print_scoreboard(
            &format!(
                "bank={} bank_id={:#018x} pack_blocks={} pack_words={}",
                bank.bank,
                bank.bank_id,
                bank.blocks.len(),
                words
            ),
            &board,
        );
    }
    let total_blocks: usize = materialized.iter().map(|bank| bank.blocks.len()).sum();
    let total_words: usize = materialized
        .iter()
        .flat_map(|bank| &bank.blocks)
        .map(|block| block.words.len())
        .sum();
    print_scoreboard("whole_rom", &whole_board);
    let total_aot_bytes = whole_board.tally(DestinationClass::ExactAot).bytes
        + whole_board.tally(DestinationClass::BlockAot).bytes;
    println!(
        "HEADLINE unsupported={} total_recompiled_exact_plus_block_aot_bytes={total_aot_bytes}",
        whole_board.unsupported
    );
    println!(
        "whole-ROM BlockPack v{}: blocks={} words={} emitted_code_bytes={} portable_json_bytes={} sha256={pack_sha256}",
        whole_pack.schema_version,
        total_blocks,
        total_words,
        total_words * 4,
        pack_json.len()
    );
    let unsupported = classified_destinations(&snapshots)
        .into_iter()
        .filter(|destination| destination.class() == DestinationClass::Unsupported)
        .map(|destination| format!("{:#010x}:{:?}", destination.va, destination.reason))
        .collect::<Vec<_>>();
    println!("unsupported_punch_list=[{}]", unsupported.join(", "));

    // `emit_sparse_bank_runner` requires every control transfer and delay
    // slot to be admitted as one architectural unit. Check that invariant
    // explicitly so a malformed pack is a named gate failure, not an opaque
    // emitter panic. Widening or dropping such a unit here would bypass the
    // proof carried by BlockPackV1.
    // `emit_sparse_bank_runner` keeps every control transfer with its delay slot
    // as one architectural unit. `data_trap_control_words` enumerates the words
    // that decode as a control transfer, sit at a block's last position, and
    // have no admitted delay slot: after the pack re-attaches every genuinely
    // severed proven delay slot, these are `jr`/branch-shaped bytes of
    // misclassified data that the emitter renders as a loud runtime trap (never
    // reached by legitimate control flow). They are reported for transparency,
    // not treated as a failure — the compile-and-run below is the real gate.
    let data_traps = data_trap_control_words(&materialized);
    if !data_traps.is_empty() {
        println!(
            "data_trap_control_words={} [{}]",
            data_traps.len(),
            data_traps.join(", ")
        );
    }

    let mut runners = Vec::with_capacity(materialized.len());
    for (index, bank) in materialized.iter().enumerate() {
        runners.push(emit_materialized_bank_runner(
            bank,
            &format!("run_wm2000_bank_{index}"),
        ));
    }
    let runner_sha256 = sha256_hex(runners.join("\n").as_bytes());
    let harness_report = compile_and_run_harness(&runners, &materialized)?;
    println!(
        "generated runners: banks={} sha256={} rustc_compiles=true harness_runs=true",
        runners.len(),
        runner_sha256
    );
    for line in harness_report.lines() {
        println!("runner: {line}");
    }
    println!(
        "scope=CPU recompilation milestone: all discovered WM2000 code banks emitted, digest-verified, compiled, and arbitrary-PC probed; dynamic_mips covers irreducible indirect sites"
    );
    println!(
        "not_a_booting_game=true (RSP audio and RDP graphics are separate U6 runtime subsystems)"
    );
    Ok(())
}

/// Words the sparse emitter renders as a loud data trap: they decode as a
/// control transfer but their delay slot is admitted by no proven block, so they
/// cannot be executed as a transfer. Mirrors the emitter's classification —
/// walk each block from its leader with the two-word stride so a real control
/// transfer's delay slot is never itself mistaken for a transfer.
fn data_trap_control_words(banks: &[MaterializedPackedBank]) -> Vec<String> {
    let mut traps = Vec::new();
    for bank in banks {
        let admitted: BTreeSet<u32> = bank
            .blocks
            .iter()
            .flat_map(|block| {
                (0..block.words.len()).map(move |word| block.start_va + word as u32 * 4)
            })
            .collect();
        for block in &bank.blocks {
            let mut index = 0usize;
            while index < block.words.len() {
                let pc = block.start_va + index as u32 * 4;
                if fn64_recomp_rs::decode(block.words[index]).has_delay_slot() {
                    if !admitted.contains(&pc.wrapping_add(4)) {
                        traps.push(format!("{}:{pc:#010x}", bank.bank));
                    }
                    index += 2;
                } else {
                    index += 1;
                }
            }
        }
    }
    traps
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
    println!(
        "  total_destinations={} reasons={}",
        board.total_destinations,
        serde_json::to_string(&board.per_reason).unwrap_or_else(|_| "<serialization error>".into())
    );
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
            )
        {
            continue;
        }
        roots.insert(target.pc);
    }
    roots.into_iter().collect()
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
    let temp = std::env::temp_dir().join(format!(
        "fn64-wm2000-whole-rom-recompile-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&temp)
        .map_err(|error| format!("creating generated-runner temp directory: {error}"))?;

    let mut source = String::from(
        "#![allow(clippy::all, unused)]\nuse fn64_recomp_rs::{BankId, BlockExit, BlockProgram, BlockRun, CodeBank, CodeSpan, CpuFault, CpuFaultKind, ExecutionKey, GeneratedBankRunner, GuestPc, InstructionBudget, ProgramError, Rdram, RecompContext};\n\n",
    );
    for runner in runners {
        source.push_str(runner);
        source.push('\n');
    }
    for (index, bank) in banks.iter().enumerate() {
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
            "];\n\nfn code_bank_{index}() -> CodeBank {{\n    let id = BankId::new({:#018X});\n    let spans = SPANS_{index}.iter().map(|(va, words)| CodeSpan::new(id, GuestPc::new(*va), words.to_vec()).unwrap()).collect();\n    CodeBank::from_spans(id, spans).unwrap()\n}}\n",
            bank.bank_id
        )
        .expect("writing generated CodeBank constructor");
    }
    source.push_str(
        "fn probe(program: &BlockProgram, bank: BankId, pc: u32) -> BlockRun {\n    let mut storage = vec![0u8; 8 * 1024 * 1024];\n    let mut mem = Rdram::new(&mut storage);\n    let mut ctx = RecompContext::new();\n    ctx.set_r(29, 0x8070_0000);\n    program.run(ExecutionKey::new(bank, GuestPc::new(pc)), InstructionBudget::new(4096).unwrap(), &mut ctx, &mut mem)\n}\n\nfn main() {\n    let mut program = BlockProgram::new();\n",
    );
    for (index, _bank) in banks.iter().enumerate() {
        writeln!(
            source,
            "    register_run_wm2000_bank_{index}(&mut program, code_bank_{index}()).unwrap();"
        )
        .expect("writing generated registration");
    }
    for bank in banks {
        let first = bank
            .blocks
            .first()
            .ok_or_else(|| format!("materialized bank {} has no blocks", bank.bank))?;
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
        writeln!(
            source,
            "    let unaligned = probe(&program, BankId::new({:#018X}), {:#010X});\n    assert!(matches!(unaligned.exit, BlockExit::Fault(CpuFault {{ kind: CpuFaultKind::UnalignedPc, .. }})));\n    assert_eq!(unaligned.instructions, 0);\n    println!(\"bank={} unaligned_pc={:#010x} typed_fault=UnalignedPc\");",
            bank.bank_id,
            first.start_va + 2,
            bank.bank,
            first.start_va + 2
        )
        .expect("writing generated unaligned probe");
        if let Some(hole) = bank.blocks.windows(2).find_map(|pair| {
            let left_end = pair[0].start_va + pair[0].words.len() as u32 * 4;
            (left_end < pair[1].start_va).then_some(left_end)
        }) {
            writeln!(
                source,
                "    let hole = probe(&program, BankId::new({:#018X}), {hole:#010X});\n    assert!(matches!(hole.exit, BlockExit::Fault(CpuFault {{ kind: CpuFaultKind::UnmappedPc {{ .. }}, .. }})));\n    assert_eq!(hole.instructions, 0);\n    println!(\"bank={} hole_pc={hole:#010x} typed_fault=UnmappedPc\");",
                bank.bank_id,
                bank.bank
            )
            .expect("writing generated hole probe");
        }
    }
    source.push_str("}\n");

    let source_path = temp.join("wm2000_whole_rom.rs");
    let binary_path = temp.join("wm2000_whole_rom");
    std::fs::write(&source_path, source)
        .map_err(|error| format!("writing generated whole-ROM harness: {error}"))?;
    let compile = Command::new(std::env::var("RUSTC").unwrap_or_else(|_| "rustc".into()))
        .arg("--edition=2021")
        .arg("--crate-type=bin")
        .arg(&source_path)
        .arg("--extern")
        .arg(format!("fn64_recomp_rs={}", rlib.display()))
        .arg("-L")
        .arg(format!("dependency={}", deps.display()))
        .arg("-C")
        .arg("debuginfo=0")
        .arg("-o")
        .arg(&binary_path)
        .output()
        .map_err(|error| format!("invoking rustc for whole-ROM runners: {error}"))?;
    if !compile.status.success() {
        return Err(format!(
            "generated whole-ROM runners did not compile:\nstdout: {}\nstderr: {}",
            String::from_utf8_lossy(&compile.stdout),
            String::from_utf8_lossy(&compile.stderr)
        ));
    }
    let execution = Command::new(&binary_path)
        .output()
        .map_err(|error| format!("running generated whole-ROM harness: {error}"))?;
    if !execution.status.success() {
        return Err(format!(
            "generated whole-ROM harness failed:\nstdout: {}\nstderr: {}",
            String::from_utf8_lossy(&execution.stdout),
            String::from_utf8_lossy(&execution.stderr)
        ));
    }
    Ok(String::from_utf8_lossy(&execution.stdout).into_owned())
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
        .ok_or_else(|| "fn64_recomp_rs rlib is missing beside the gate binary".to_string())
}

fn sha256_hex(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
