//! Execution-closure scoreboard gate: the concrete "distance to recompilable"
//! metric.
//!
//! The full-game gate (DISCOVER-PLAN, UNIVERSAL-RUNTIME-PLAN) is "zero
//! `unsupported` execution destinations." This gate is the SCOREBOARD that
//! measures that number. For each supplied ROM it runs the real discovery
//! pipeline, composes every proven physically-resident bank through Phase 4-6
//! closure + owner/block proof (reusing [`snapshot::compose_materialized_banks_v1`],
//! not reimplementing anything), then classifies every reachable CPU transfer
//! destination as `exact_aot` / `block_aot` / `dynamic_mips` / `unsupported`
//! and reports the counts. The headline `unsupported` count is the number that
//! must reach zero for a full-game build.
//!
//! Honest scope: this is a STATIC reachability scoreboard from proven roots. It
//! measures what discovery has CLOSED, not a live run. Destinations reachable
//! only at runtime surface as `dynamic_mips` (open indirects) or are simply
//! never reached; nothing here claims runtime closure. The honest-scope
//! argument for that distinction lives in `crate::closure`'s module doc.
//! (This line previously cited docs/CLOSURE_EVIDENCE.md, which does not
//! exist and never has.)
//!
//! Grading (held-out): where a ROM dump is supplied it is opened ONLY AFTER
//! classification, and used solely to reject a bug — an `exact_aot`/`block_aot`
//! destination the dump says is data. It never becomes a root, fact, or class.
//!
//! ROM/dump paths come from named, declared environment variables. An unset var
//! is a loud skip line, never a silent omission:
//!   FN64_DISCOVER_NW4E_ROM  [FN64_DISCOVER_NW4E_DUMP]
//!   FN64_DISCOVER_NWXE_ROM  [FN64_DISCOVER_NWXE_DUMP]
//!   FN64_DISCOVER_OOT_ROM   [FN64_DISCOVER_OOT_DUMP]

use fn64_discover::banks::{self, BankNamePattern};
use fn64_discover::closure::{
    classified_destinations, scoreboard, ClosureScoreboard, DestinationClass,
};
use fn64_discover::delta_vote::DeltaVoteConfig;
use fn64_discover::facts::{FunctionEntryEvidence, ProofState};
use fn64_discover::overlay_regions::SearchConfig;
use fn64_discover::block_pack::{
    emit_block_pack_v1, emit_block_program_source_with_facts, materialize_block_pack_with_facts,
    BlockPackV1, BlockProgramSourceConfig,
};
use fn64_discover::snapshot::{compose_materialized_banks_v1, MaterializedBankInput};
use fn64_discover::{
    aki_reference, run_discovery, run_discovery_with_recovered_vrom_and_request_dma,
    run_discovery_with_recovered_overlay_regions, DescriptorTableInput, Fact, FactDb,
    NormalizedRom, RecoveredOverlayInput, RecoveredVromOverlayInput, RomAddressSpace,
};
use serde::Deserialize;
use std::collections::BTreeSet;

const NW4E_ROM_VAR: &str = "FN64_DISCOVER_NW4E_ROM";
const NW4E_DUMP_VAR: &str = "FN64_DISCOVER_NW4E_DUMP";
const NWXE_ROM_VAR: &str = "FN64_DISCOVER_NWXE_ROM";
const NWXE_DUMP_VAR: &str = "FN64_DISCOVER_NWXE_DUMP";
const OOT_ROM_VAR: &str = "FN64_DISCOVER_OOT_ROM";
const OOT_DUMP_VAR: &str = "FN64_DISCOVER_OOT_DUMP";
const EMIT_BLOCK_PROGRAM_VAR: &str = "FN64_EMIT_BLOCK_PROGRAM";

/// Which discovery composition a ROM uses. All three reuse the crate's own
/// `run_discovery*` entry points; none reimplements discovery.
enum Discovery {
    /// Boot bank + an AKI-family descriptor table (NW4E).
    Descriptor(Option<DescriptorTableInput>),
    /// Boot bank + mechanically recovered overlay descriptor table (NWXE).
    RecoveredOverlays(RecoveredOverlayInput),
    /// Boot bank + mechanically recovered VROM overlay geometry, plus the
    /// static request-DMA claims for images loaded by an explicit DMA call
    /// (OoT). No table location, stride, record count, or destination is a
    /// caller-supplied game fact.
    RecoveredVrom(
        Box<RecoveredVromOverlayInput>,
        Vec<banks::StaticRequestDmaInput>,
    ),
}

struct RomSpec {
    label: &'static str,
    rom_var: &'static str,
    dump_var: &'static str,
    discovery: fn() -> Discovery,
}

/// One proven physically-resident bank the composer can materialize.
struct PhysicalBank {
    bank: String,
    rom_space: RomAddressSpace,
    rom_start: u32,
    rom_end: u32,
    va_start: u32,
    va_end: u32,
}

fn main() {
    if let Err(error) = run() {
        eprintln!("gate_closure FAILED: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    println!("=== fn64-discover execution-closure scoreboard ===");
    println!("(static reachability from proven roots; unsupported is the release blocker)");
    println!();

    let roms = [
        RomSpec {
            label: "NW4E",
            rom_var: NW4E_ROM_VAR,
            dump_var: NW4E_DUMP_VAR,
            discovery: nw4e_discovery,
        },
        RomSpec {
            label: "NWXE",
            rom_var: NWXE_ROM_VAR,
            dump_var: NWXE_DUMP_VAR,
            discovery: nwxe_discovery,
        },
        RomSpec {
            label: "OoT",
            rom_var: OOT_ROM_VAR,
            dump_var: OOT_DUMP_VAR,
            discovery: oot_discovery,
        },
    ];

    let mut failed = false;
    for spec in roms {
        match std::env::var_os(spec.rom_var) {
            None => {
                println!("skip {}: {} unset", spec.label, spec.rom_var);
                println!();
            }
            Some(path) => {
                let path = path.to_string_lossy().into_owned();
                match score_rom(&spec, &path) {
                    Ok(()) => println!(),
                    Err(error) => {
                        failed = true;
                        eprintln!("FAIL {}: {error}", spec.label);
                    }
                }
            }
        }
    }

    if failed {
        std::process::exit(1);
    }
    Ok(())
}

fn score_rom(spec: &RomSpec, path: &str) -> Result<(), String> {
    let rom_bytes = std::fs::read(path).map_err(|error| format!("reading {path}: {error}"))?;
    let (rom, facts) = discover(&rom_bytes, (spec.discovery)())?;

    let physical = physical_banks(&facts)?;
    if physical.is_empty() {
        return Err("no proven physically-resident bank to compose".to_string());
    }

    // Materialize every physical bank's bytes and roots; `MaterializedBankInput`
    // borrows both, so they must outlive the composition call. Banks are
    // composed together so cross-bank direct-call authority is available.
    let mut bank_bytes: Vec<Vec<u8>> = Vec::with_capacity(physical.len());
    let mut bank_roots: Vec<Vec<u32>> = Vec::with_capacity(physical.len());
    for bank in &physical {
        // Resolve through the shared range materializer: a physically-resident
        // bank slices the image, while a VROM (DMA-loaded, possibly compressed)
        // overlay resolves through its one proven file-table record.
        let materialized = banks::materialize_rom_range(
            &rom,
            &facts,
            bank.rom_space,
            bank.rom_start,
            bank.rom_end,
        )
        .map_err(|error| {
            format!(
                "{} ROM interval [0x{:x},0x{:x}): {error}",
                bank.bank, bank.rom_start, bank.rom_end
            )
        })?;
        bank_bytes.push(materialized.bytes);
        bank_roots.push(callable_roots(&facts, bank));
    }

    let inputs: Vec<MaterializedBankInput> = physical
        .iter()
        .enumerate()
        .map(|(index, bank)| MaterializedBankInput {
            bank: &bank.bank,
            va_start: bank.va_start,
            bytes: &bank_bytes[index],
            seed_roots: &bank_roots[index],
        })
        .collect();

    let snapshots = compose_materialized_banks_v1(&rom, &facts, &inputs)
        .map_err(|error| format!("composing banks: {error}"))?;

    // Opt-in: emit the composed program as a compilable BlockProgram source
    // module. This is measurement, not part of the scoreboard, so it never
    // changes the classification below.
    if let Some(out) = std::env::var_os(EMIT_BLOCK_PROGRAM_VAR) {
        emit_block_program(spec.label, &rom, &facts, &snapshots, &out.to_string_lossy())?;
    }

    let board = scoreboard(&snapshots);

    // Held-out grading boundary: every discovery, composition, proof, and
    // classification pass above is COMPLETE. Nothing parsed from the dump below
    // can become a root, fact, range, owner, or class — it can only reject a
    // destination this gate classified exact_aot/block_aot that the dump calls
    // data.
    let dump_check = match std::env::var_os(spec.dump_var) {
        None => None,
        Some(dump_path) => {
            let dump_path = dump_path.to_string_lossy().into_owned();
            let dump_text = std::fs::read_to_string(&dump_path)
                .map_err(|error| format!("reading {dump_path}: {error}"))?;
            Some(grade_against_dump(&snapshots, &dump_text)?)
        }
    };

    print_scoreboard(spec.label, &rom, &physical, &snapshots, &board, &dump_check);

    if let Some(check) = &dump_check {
        if check.misclassified != 0 {
            return Err(format!(
                "held-out grade: {} exact_aot/block_aot destinations land where the dump says data",
                check.misclassified
            ));
        }
    }
    Ok(())
}

fn discover(rom_bytes: &[u8], discovery: Discovery) -> Result<(NormalizedRom, FactDb), String> {
    match discovery {
        Discovery::Descriptor(descriptor) => {
            run_discovery(rom_bytes, descriptor).map_err(|error| error.to_string())
        }
        Discovery::RecoveredOverlays(input) => {
            run_discovery_with_recovered_overlay_regions(rom_bytes, &input)
                .map(|(rom, facts, _recovery)| (rom, facts))
                .map_err(|error| error.to_string())
        }
        Discovery::RecoveredVrom(input, request_dma) => {
            run_discovery_with_recovered_vrom_and_request_dma(rom_bytes, &input, &request_dma)
                .map(|(rom, facts, _recovery, _report)| (rom, facts))
                .map_err(|error| error.to_string())
        }
    }
}

/// Emit the composed snapshots as one compilable `BlockProgram` source module
/// and report its size. The entry is the lowest-VA block of the first composed
/// bank, which is the boot bank in bank order.
fn emit_block_program(
    label: &str,
    rom: &NormalizedRom,
    facts: &FactDb,
    snapshots: &[fn64_discover::snapshot::ProgramSnapshotV1],
    out_path: &str,
) -> Result<(), String> {
    let mut pack = BlockPackV1 {
        schema_version: fn64_discover::block_pack::BLOCK_PACK_SCHEMA_V1,
        normalized_rom_sha256: rom.sha256.clone(),
        banks: Vec::with_capacity(snapshots.len()),
    };
    // A composed bank with no proven blocks is an overlay that static closure
    // never reached. It carries no executable code to emit, so it is counted
    // and skipped rather than failing the whole program.
    let mut empty_banks = 0usize;
    for snapshot in snapshots {
        match emit_block_pack_v1(snapshot, rom) {
            Ok(emitted) => pack.banks.extend(emitted.banks),
            Err(fn64_discover::block_pack::BlockPackError::NoProvenBlocks { .. }) => {
                empty_banks += 1;
            }
            Err(error) => {
                return Err(format!(
                    "emitting block pack for {}: {error:?}",
                    snapshot.banks[0].input.bank
                ));
            }
        }
    }
    pack.banks.sort_by(|left, right| left.bank.cmp(&right.bank));

    let materialized = materialize_block_pack_with_facts(&pack, rom, Some(facts))
        .map_err(|error| format!("materializing whole-ROM BlockPack: {error:?}"))?;
    let blocks: usize = materialized.iter().map(|bank| bank.blocks.len()).sum();

    // Entry: the first block of the boot bank (lowest VA across the pack).
    let entry_bank = pack
        .banks
        .iter()
        .min_by_key(|bank| {
            bank.blocks
                .iter()
                .map(|block| block.start_va)
                .min()
                .unwrap_or(u32::MAX)
        })
        .ok_or_else(|| "empty block pack".to_string())?;
    let entry_pc = entry_bank
        .blocks
        .iter()
        .map(|block| block.start_va)
        .min()
        .ok_or_else(|| "boot bank has no blocks".to_string())?;

    let source = emit_block_program_source_with_facts(
        &pack,
        rom,
        Some(facts),
        BlockProgramSourceConfig {
            entry: fn64_recomp_rs::ExecutionKey::new(
                fn64_recomp_rs::BankId::new(entry_bank.bank_id),
                fn64_recomp_rs::GuestPc::new(entry_pc),
            ),
            instruction_budget: fn64_recomp_rs::InstructionBudget::new(1024)
                .ok_or_else(|| "invalid instruction budget".to_string())?,
        },
    )
    .map_err(|error| format!("emitting block program source: {error:?}"))?;

    std::fs::write(out_path, &source)
        .map_err(|error| format!("writing {out_path}: {error}"))?;
    println!(
        "  emitted {label} BlockProgram: banks={} (+{empty_banks} unreached) blocks={} source_bytes={} entry={}@{:#010x} -> {out_path}",
        pack.banks.len(),
        blocks,
        source.len(),
        entry_bank.bank,
        entry_pc,
    );
    Ok(())
}

/// Every proven ROM mapping, in deterministic bank order. Physically-resident
/// banks slice the image; VROM (DMA-loaded) overlays resolve through their
/// proven file-table record, so OoT's overlays compose here too.
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
        // A DMA-loaded overlay's VA extent may exceed its ROM extent by a
        // load-time `.bss` tail. Compose only the ROM-backed prefix: bss holds
        // no instructions, so excluding it loses no executable code.
        let (Some(rom_extent), Some(va_extent)) = (
            rom_end.checked_sub(*rom_start),
            va_end.checked_sub(*va_start),
        ) else {
            return Err(format!("bank {bank} has an inverted ROM or VA interval"));
        };
        if rom_extent > va_extent {
            return Err(format!(
                "bank {bank} carries more ROM bytes ({rom_extent}) than VA extent ({va_extent})"
            ));
        }
        banks.push(PhysicalBank {
            bank: bank.clone(),
            rom_space: *rom_space,
            rom_start: *rom_start,
            rom_end: *rom_end,
            va_start: *va_start,
            va_end: va_start.saturating_add(rom_extent),
        });
    }
    banks.sort_by(|left, right| left.bank.cmp(&right.bank));
    Ok(banks)
}

/// Traversal roots come only from ROM-derived discovery claims: proven
/// entries plus direct/exhaustive-indirect/table callable-entry claims. This
/// mirrors [`gate_owners_overlays`]'s rule; the composer itself decides
/// authority. Candidate prologues are deliberately not bulk-seeded.
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

// ---- Held-out dump grading ---------------------------------------------------

#[derive(Debug, Deserialize)]
struct SymbolsDoc {
    #[serde(default)]
    section: Vec<SectionDoc>,
}

#[derive(Debug, Deserialize)]
struct SectionDoc {
    vram: u32,
    size: u32,
    #[serde(default)]
    functions: Vec<FunctionDoc>,
}

#[derive(Debug, Deserialize)]
struct FunctionDoc {
    vram: u32,
    size: u32,
}

struct DumpCheck {
    /// Distinct code VAs in the dump's function extents used for the cross-check.
    code_va_count: usize,
    /// exact_aot/block_aot destinations classified inside a dump function
    /// extent (positive corroboration).
    corroborated: usize,
    /// exact_aot/block_aot destinations that the dump's section/function layout
    /// says are DATA (outside every function extent but inside a graded
    /// section). A nonzero count is a real bug this gate surfaces.
    misclassified: usize,
}

/// Build the set of VAs the dump attributes to CODE (any function body) and the
/// set of VAs inside a graded SECTION at all. A destination classified AOT that
/// is inside a section but not any function is a data hit — a real bug.
fn grade_against_dump(
    snapshots: &[fn64_discover::snapshot::ProgramSnapshotV1],
    dump_text: &str,
) -> Result<DumpCheck, String> {
    let doc: SymbolsDoc = toml::from_str(dump_text).map_err(|error| error.to_string())?;
    let mut code: BTreeSet<u32> = BTreeSet::new();
    let mut section_ranges: Vec<(u32, u32)> = Vec::new();
    for section in &doc.section {
        if section.size == 0 {
            continue;
        }
        let Some(section_end) = section.vram.checked_add(section.size) else {
            continue;
        };
        section_ranges.push((section.vram, section_end));
        for function in &section.functions {
            if function.size == 0 {
                continue;
            }
            let Some(end) = function.vram.checked_add(function.size) else {
                continue;
            };
            for va in (function.vram..end).step_by(4) {
                code.insert(va);
            }
        }
    }
    let in_section = |va: u32| section_ranges.iter().any(|&(s, e)| va >= s && va < e);

    let mut corroborated = 0usize;
    let mut misclassified = 0usize;
    for dest in classified_destinations(snapshots) {
        if !matches!(
            dest.class(),
            DestinationClass::ExactAot | DestinationClass::BlockAot
        ) {
            continue;
        }
        if code.contains(&dest.va) {
            corroborated += 1;
        } else if in_section(dest.va) {
            // The dump has an opinion about this VA (it is inside a graded
            // section) and that opinion is "not the start of a function body
            // word here" -> surface it. Only count VAs the dump actually
            // covers, so overlay banks the dump lacks never produce false bugs.
            misclassified += 1;
        }
    }
    Ok(DumpCheck {
        code_va_count: code.len(),
        corroborated,
        misclassified,
    })
}

// ---- Reporting ---------------------------------------------------------------

fn print_scoreboard(
    label: &str,
    rom: &NormalizedRom,
    physical: &[PhysicalBank],
    snapshots: &[fn64_discover::snapshot::ProgramSnapshotV1],
    board: &ClosureScoreboard,
    dump_check: &Option<DumpCheck>,
) {
    let reached_blocks: u64 = snapshots
        .iter()
        .flat_map(|snapshot| snapshot.banks.iter())
        .map(|bank| bank.block_proof.proven_blocks)
        .sum();
    println!("{label}: ROM SHA-256 {}", rom.sha256);
    println!(
        "  composed_physical_banks={} reached_proven_blocks={} total_reachable_destinations={}",
        physical.len(),
        reached_blocks,
        board.total_destinations
    );
    for class in DestinationClass::ALL {
        let tally = board.tally(class);
        println!(
            "  {:<12} destinations={:>8} bytes={:>10}",
            class.label(),
            tally.destinations,
            tally.bytes
        );
    }
    println!(
        "  HEADLINE unsupported={}  (release blocker; must reach 0)",
        board.unsupported
    );
    println!(
        "  dynamic_mips={}  (classified interpreter-coverable, not a release blocker; the\n                    fallback lane itself is still unimplemented -- see DISCOVER-PLAN)",
        board.dynamic_mips
    );
    println!(
        "  reasons={}",
        serde_json::to_string(&board.per_reason).unwrap_or_else(|_| "<error>".to_string())
    );
    // Why block proof REFUSED, which is the actionable half of the score:
    // `dynamic_mips` says the interpreter has to cover a destination, and this
    // says what stopped it being AOT. One block can carry several blockers, so
    // the total exceeds the refused-block count.
    println!(
        "  block_proof_blockers={}",
        serde_json::to_string(&fn64_discover::block_proof::blocker_histogram(snapshots))
            .unwrap_or_else(|_| "<error>".to_string())
    );
    // `open_indirect_site` counts only sites inside blocks this composed
    // program actually built. It is NOT the ROM's open-indirect frontier --
    // the D1/D2 measurements report several hundred for the same ROM.
    println!("  (open_indirect_site counts composed-block sites only, not the ROM's frontier)");
    // The concrete VAs that block release, so the number has an address list
    // the reachability work can chase. Bounded so the gate stays readable.
    let unsupported_vas: Vec<u32> = classified_destinations(snapshots)
        .into_iter()
        .filter(|dest| dest.class() == DestinationClass::Unsupported)
        .map(|dest| dest.va)
        .collect();
    const SHOWN: usize = 24;
    let shown: Vec<String> = unsupported_vas
        .iter()
        .take(SHOWN)
        .map(|va| format!("{va:#010x}"))
        .collect();
    let suffix = if unsupported_vas.len() > SHOWN {
        format!(" (+{} more)", unsupported_vas.len() - SHOWN)
    } else {
        String::new()
    };
    println!(
        "  unsupported_destinations=[{}]{}",
        shown.join(", "),
        suffix
    );
    match dump_check {
        None => println!("  held-out grade: no dump supplied (classification ungraded)"),
        Some(check) => println!(
            "  held-out grade: dump_code_vas={} corroborated_aot={} misclassified_as_code={}",
            check.code_va_count, check.corroborated, check.misclassified
        ),
    }
}

// ---- Per-ROM discovery inputs (cited geometry, no answer keys) ---------------

fn nw4e_discovery() -> Discovery {
    Discovery::Descriptor(Some((
        aki_reference::NW4E_DESCRIPTOR_TABLE,
        aki_reference::nw4e_bank_name,
    )))
}

fn nwxe_discovery() -> Discovery {
    let search = SearchConfig::aki_family();
    Discovery::RecoveredOverlays(RecoveredOverlayInput {
        min_mapped_regions: search.min_records,
        search,
        delta_vote: DeltaVoteConfig::default(),
        table_name: "recovered_overlay_descriptors".to_string(),
        bank_name: BankNamePattern::new("recovered_overlay_", 0, ""),
    })
}

/// OoT's resident `code`/`n64dd` images are loaded by the game's own
/// `DmaMgr_RequestSync(ram, vrom, size)` call, not by any table, so no
/// load-image table supplies their VRAM destination. The request-DMA scan
/// reads that geometry out of the boot image's own instruction bytes; the
/// callee VA is a cited anchor claim (see the reference TOML's header).
fn oot_discovery() -> Discovery {
    // Fully mechanical: overlay geometry is recovered from the ROM, and the
    // DMA-request routine that loads the resident image is recovered from its
    // own corroborated call-site operands. No per-ROM table geometry and no
    // cited callee address.
    Discovery::RecoveredVrom(
        Box::new(RecoveredVromOverlayInput {
            search: SearchConfig::vrom_family(),
            delta_vote: DeltaVoteConfig::default(),
            file_table_search: fn64_discover::file_table::FileTableSearchConfig::n64_family(),
            vrom_min_records: 2,
            min_mapped_regions: 2,
            file_table_name: "recovered_file_table".to_string(),
            table_name: "recovered_vrom_overlay_descriptors".to_string(),
            bank_name: BankNamePattern::new("recovered_overlay_", 0, ""),
        }),
        Vec::new(),
    )
}
