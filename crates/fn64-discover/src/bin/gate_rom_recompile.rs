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

use fn64_discover::banks::{BankNamePattern, BOOT_BANK};
use fn64_discover::block_pack::{
    emit_validated_block_pack_v2, materialize_block_pack, BlockPackV1, MaterializedPackedBank,
};
use fn64_discover::catalog_transfer_fixed_point::{
    compose_catalog_bound_direct_transfer_fixed_point_v1, CatalogTransferFixedPointLimitsV1,
};
use fn64_discover::closure::{
    classified_destinations, scoreboard, ClosureScoreboard, DestinationClass,
};
use fn64_discover::delta_vote::DeltaVoteConfig;
use fn64_discover::dense_aot_pack::{
    build_dense_aot_pack_v1, DenseAotGenerationInput, DENSE_AOT_SHARD_BYTES,
};
use fn64_discover::facts::{FunctionEntryEvidence, ProofState};
use fn64_discover::generation_topology::build_generation_topology_v1;
use fn64_discover::overlay_recipe::admitted_overlay_load_recipes_v1;
use fn64_discover::overlay_regions::SearchConfig;
use fn64_discover::runtime_generation_catalog::build_backed_dense_generation_catalog_v1;
use fn64_discover::snapshot::{
    compose_materialized_banks_validated_v2_with_limits, MaterializedBankInput,
    MultiBankCompositionLimits, ValidatedComposedSnapshotsV2,
};
use fn64_discover::{
    required_env_path, run_discovery_with_recovered_overlay_regions, DiscoveryStrategy, Fact,
    FactDb, RecoveredOverlayInput, RomAddressSpace,
};
use fn64_recomp_rs::execution::BankId;
use fn64_recomp_rs_codegen::{emit_dense_bank_shard_runner_function, DenseBankShardInput};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
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

/// Maximum words a single emitted *function* may cover, using the same 64
/// KiB dense-AOT shard convention as [`MAX_COMPILE_UNIT_WORDS`] (the two
/// constants happen to share a value, but bound different things: this one
/// bounds one `fn`'s own `match pc {}` arm count, that one bounds how many
/// already-small functions one rustc invocation compiles together).
///
/// `MAX_COMPILE_UNIT_WORDS` alone cannot fix compile time when a whole bank
/// is a single giant emitted function -- grouping compile units cannot
/// subdivide a function that was never split. Penny Racers is exactly this
/// case: one physical bank, 5,837 words, one `emit_materialized_bank_runner`
/// call previously produced one `match` with over a thousand arms, which hit
/// an LLVM `-O` `ConstraintEliminationPass` complexity cliff independent of
/// how many rustc invocations saw it. Splitting *emission* itself -- before
/// any function is generated, using the same shard-before-emission idiom
/// `gate_wm2000_recompile`'s dense-AOT path already relies on -- keeps every
/// emitted function small regardless of how large the source bank is.
const MAX_SHARD_WORDS: usize = (DENSE_AOT_SHARD_BYTES / 4) as usize;

/// Content-hashing salt for this gate's own resident-tail generation identity
/// (see `build_generation_topology_v1`'s doc comment on
/// `resident_tail_identity_domain`: any fixed, gate-distinguishing byte
/// string is correct). Distinct from `gate_wm2000_recompile`'s own constant
/// so the two gates' identity hashes can never collide.
const ROM_RECOMPILE_RESIDENT_TAIL_IDENTITY_DOMAIN_V1: &[u8] =
    b"fn64:rom-recompile-resident-tail-generation:v1:";

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

    // Two composition paths, chosen by what discovery actually proved:
    //
    // The single-pass composer (`compose_materialized_banks_validated_v2_with_limits`)
    // trusts only `proven_function_entries`/`proven_hardware_function_entries`
    // as CFG-closure authority (see `build_cross_bank_authority_closure` in
    // `snapshot.rs`), and a hardware entrypoint exists only for the boot bank
    // (`Fact::FunctionEntryClaim` with `RomHeaderEntrypoint` evidence, proven
    // from the ROM's own header). An overlay bank has no such entry -- nothing
    // ever calls it from inside its own byte range -- so every one of its
    // `FunctionEntryClaim` roots (however many `callable_roots` collects) is
    // rejected as `EntryNotAuthoritative` and the bank composes with zero
    // proven blocks, regardless of how many roots were seeded. This is a
    // proof-propagation gap, not a geometry gap: passing the overlay's whole
    // bank extent as if it were one flat text region (as this gate did before)
    // changes nothing here, because the single-pass composer never looks at a
    // text/data split at all -- `MaterializedBankInput` has none.
    //
    // What actually supplies authority into an overlay bank is a PROVEN CALL
    // FROM the boot bank's own closure landing at the overlay's load address --
    // exactly what `compose_catalog_bound_direct_transfer_fixed_point_v1`
    // establishes by iterating composition to a fixed point over dense-AOT
    // generations wired through a generation topology and capability catalog.
    // That is the same mechanism `gate_wm2000_recompile` (this gate's
    // hand-configured, single-game predecessor) already relies on for the
    // identical AKI-family overlay shape; the only thing that gate hardcodes
    // and this one must not is the recipe geometry and bank count.
    let composed = if discovery.selected == DiscoveryStrategy::RecoveredOverlays {
        compose_catalog_bound_overlay_snapshots(&rom_bytes, &rom, &facts, &physical, &inputs)?
    } else {
        // Generic composition, not the catalog-bound generation fixed point:
        // the latter requires at least one overlay generation, which a
        // single-bank (or boot-bank-only) ROM does not have. This is the same
        // validated multi-bank composition `gate_rom_rebuild` proves across
        // the corpus, and it yields the same `ValidatedComposedSnapshotsV2`
        // the block-pack emitter consumes.
        compose_materialized_banks_validated_v2_with_limits(
            &rom,
            &facts,
            &inputs,
            MultiBankCompositionLimits::default(),
        )
        .map_err(|error| format!("composing snapshot banks: {error}"))?
    };
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

    let mut bank_shards = Vec::with_capacity(materialized.len());
    for (index, bank) in materialized.iter().enumerate() {
        bank_shards.push(
            plan_bank_shards(bank, index)
                .map_err(|error| format!("sharding bank {}: {error}", bank.bank))?,
        );
    }
    let runner_sha256 = sha256_hex(
        bank_shards
            .iter()
            .flatten()
            .map(|shard| shard.source.as_str())
            .collect::<Vec<_>>()
            .join("\n")
            .as_bytes(),
    );
    let harness_report = compile_and_run_harness(&bank_shards, &materialized)?;
    println!(
        "generated runners: banks={} sha256={runner_sha256} rustc_compiles=true harness_runs=true",
        materialized.len(),
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

/// Compose an AKI-family overlay ROM (`DiscoveryStrategy::RecoveredOverlays`)
/// through the catalog-bound direct-transfer fixed point, so cross-bank
/// authority (a proven call from the boot bank landing at an overlay's load
/// address) can reach each overlay's blocks. See the call site in `run()` for
/// why the single-pass composer cannot do this at all.
///
/// This re-runs `run_discovery_with_recovered_overlay_regions` to obtain the
/// `OverlayRecovery` object `run_discovery_auto` selected internally but does
/// not return (`AutoDiscovery` carries only the winning `FactDb`). The search
/// config mirrors exactly what `run_discovery_auto` tries for this strategy
/// (`lib.rs`, `overlay_input` in `run_discovery_auto_with_limits`): AKI-family
/// geometry, the same table/bank naming. Re-deriving is cheap (one more ROM
/// scan) and deterministic -- it is the same mechanical recovery over the
/// same bytes, so it reproduces the identical recovery `run_discovery_auto`
/// already selected, not a second guess. A generic gate must not hardcode
/// `aki_family()` for its OWN sake (the ROM might have selected the sibling
/// `RecoveredVrom`/`vrom_family()` strategy instead, which this function is
/// never called for); it is hardcoded here only because this function is
/// reached exclusively when `discovery.selected == RecoveredOverlays`, which
/// `run_discovery_auto` only ever concludes from the `aki_family()` attempt.
fn compose_catalog_bound_overlay_snapshots(
    rom_bytes: &[u8],
    rom: &fn64_discover::NormalizedRom,
    facts: &FactDb,
    physical: &[PhysicalBank],
    inputs: &[MaterializedBankInput<'_>],
) -> Result<ValidatedComposedSnapshotsV2, String> {
    let search = SearchConfig::aki_family();
    let overlay_input = RecoveredOverlayInput {
        min_mapped_regions: search.min_records,
        search,
        delta_vote: DeltaVoteConfig::default(),
        table_name: "recovered_overlay_descriptors".to_string(),
        bank_name: BankNamePattern::new("recovered_overlay_", 0, ""),
    };
    let (_, _, recovery) = run_discovery_with_recovered_overlay_regions(rom_bytes, &overlay_input)
        .map_err(|error| format!("re-deriving overlay recovery: {error:?}"))?;
    let recipes = admitted_overlay_load_recipes_v1(rom_bytes, &recovery)
        .map_err(|error| format!("recovering complete overlay load recipes: {error:?}"))?;

    let resident = physical
        .iter()
        .find(|bank| bank.bank == BOOT_BANK)
        .ok_or_else(|| {
            "recovered-overlay composition requires a resident boot bank".to_string()
        })?;
    // The boot bank's whole physical extent is trusted as one flat text
    // region (no data/bss split), matching what this gate's single-bank path
    // already assumed for every non-overlay ROM -- the IPL3 boot copy is a
    // fixed-size, fixed-offset physical span (`BOOT_COPY_ROM_START`/`SIZE` in
    // `banks.rs`), never a table-described load image with its own text/data
    // boundary the way an overlay recipe is.
    let mut dense_inputs = vec![DenseAotGenerationInput {
        name: BOOT_BANK,
        source_rom_start: resident.rom_start,
        source_rom_end: resident.rom_end,
        load_start: resident.va_start,
        text_start: resident.va_start,
        text_end: resident.va_end,
        data_start: resident.va_end,
        data_end: resident.va_end,
        bss_start: resident.va_end,
        bss_end: resident.va_end,
    }];

    // Match each overlay `PhysicalBank` to its recipe by ROM source interval,
    // not by list position: `physical_banks` derives its order from
    // `proven_rom_mappings()` (bank-name sorted), while `admitted_overlay_
    // load_recipes_v1` preserves the admitted table's own raw record order,
    // which `scan_recovered_overlay_regions` deduplicates/renames through a
    // `BTreeMap` keyed by `(rom_start, rom_end)` -- a different sort key than
    // bank name. The two orders coincide whenever bank names sort the same
    // way their source intervals do, but nothing proves they always must;
    // matching by the ROM interval both sides actually share is exact
    // regardless, and a bank with no matching recipe fails loudly instead of
    // silently pairing with the wrong overlay's text/data/bss split.
    let overlay_banks: Vec<&PhysicalBank> = physical
        .iter()
        .filter(|bank| bank.bank != BOOT_BANK)
        .collect();
    if overlay_banks.len() != recipes.len() {
        return Err(format!(
            "recovered {} overlay bank(s) but {} admitted load recipe(s)",
            overlay_banks.len(),
            recipes.len()
        ));
    }
    // `build_generation_topology_v1` below zips its own `dense_pack.generations`
    // (built from `dense_inputs`, in `overlay_banks` order) against whatever
    // recipe slice it is given, POSITIONALLY (`overlays.iter().zip(recipes)` in
    // `generation_topology.rs`) -- so the recipe list passed to it must already
    // be reordered into that same order, not the admitted table's raw record
    // order `recipes` started in. `matched_recipes` carries that reordering
    // alongside `dense_inputs` so the two stay in lockstep by construction.
    let mut matched_recipes = Vec::with_capacity(overlay_banks.len());
    for bank in &overlay_banks {
        let recipe = recipes
            .iter()
            .find(|recipe| recipe.rom_start == bank.rom_start && recipe.rom_end == bank.rom_end)
            .ok_or_else(|| {
                format!(
                    "{} ROM interval [{:#x},{:#x}) has no matching admitted overlay recipe",
                    bank.bank, bank.rom_start, bank.rom_end
                )
            })?;
        dense_inputs.push(DenseAotGenerationInput::from((bank.bank.as_str(), recipe)));
        matched_recipes.push(recipe.clone());
    }

    let dense_pack = build_dense_aot_pack_v1(rom, &dense_inputs)
        .map_err(|error| format!("building dense AOT pack: {error:?}"))?;
    // The identity domain is a content-hashing salt, not game-specific logic
    // (see `build_generation_topology_v1`'s own doc comment); it only needs
    // to be a fixed distinguishing tag for this gate's own resident-tail
    // generation identity, the same role `gate_wm2000_recompile`'s WM-specific
    // constant plays for its one hardcoded game.
    let topology = build_generation_topology_v1(
        rom,
        &dense_pack,
        BOOT_BANK,
        ROM_RECOMPILE_RESIDENT_TAIL_IDENTITY_DOMAIN_V1,
        &matched_recipes,
    )
    .map_err(|error| format!("building generation topology: {error}"))?;
    let generation_catalog = build_backed_dense_generation_catalog_v1(rom, &dense_pack, &topology)
        .map_err(|error| format!("building runtime generation catalog: {error}"))?;
    let catalog_fixed_point = compose_catalog_bound_direct_transfer_fixed_point_v1(
        rom,
        facts,
        inputs,
        &dense_pack,
        &topology,
        &generation_catalog,
        CatalogTransferFixedPointLimitsV1::default(),
    )
    .map_err(|error| format!("composing catalog-bound overlay transfer closure: {error:?}"))?;
    Ok(catalog_fixed_point.into_validated())
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

/// One 64 KiB-bounded slice of one bank's materialized words, already
/// rendered to Rust source by the dense shard emitter.
///
/// This is the actual fix for the compile-time cliff: emission itself is
/// split before any function is generated, so no single emitted `fn` -- and
/// therefore no single rustc `-O` pass -- ever sees more than
/// [`MAX_SHARD_WORDS`] words, regardless of how large the source bank is.
#[derive(Clone)]
struct BankShard {
    /// Index into `materialized`/`bank_shards` identifying the owning bank.
    bank_index: usize,
    /// This shard's ordinal within its bank, used to name its function.
    shard_index: usize,
    /// `[start_va, end_va)`: the guest PC range this shard's own generated
    /// `match pc {}` arms cover. Used by the bank's dispatcher wrapper to
    /// route a PC to the shard that owns it.
    start_va: u32,
    end_va: u32,
    /// The rendered `fn run_rom_bank_{bank_index}_shard_{shard_index}(...)`.
    source: String,
}

impl BankShard {
    fn fn_name(&self) -> String {
        format!(
            "run_rom_bank_{}_shard_{}",
            self.bank_index, self.shard_index
        )
    }
}

/// Split one materialized bank into dense shard runners, each covering at
/// most [`MAX_SHARD_WORDS`] words of one block.
///
/// A block is split at shard boundaries, never across blocks -- blocks may
/// have gaps between them (proven code spans are not required to be
/// contiguous), and a gap PC must remain unmapped, not silently absorbed
/// into an adjacent block's shard. Splitting *inside* a block reuses the
/// exact idiom `dense_aot_pack.rs` already uses for contiguous ROM/RDRAM
/// shards: `delay_lookahead` is the word immediately following this slice
/// within the same block, so a control transfer that is a shard's last
/// owned word still gets its correct architectural delay-slot word; the
/// true last word of a block that dangles mid control-transfer pair (no
/// admitted successor in the block at all) is a malformed proven span the
/// dense emitter is right to reject loudly, since executing it would fault
/// identically under the sparse emitter too.
///
/// `artifact_vram_start/end` is set to the *whole bank's* extent (its first
/// block's start through its last block's end), not this shard's own
/// narrower range. That is what keeps every in-bank control transfer --
/// including ones that land in a different shard of the same bank -- typed
/// as `BlockExit::Transfer(same BankId, target)` rather than
/// `ResolveTransfer` (see `emit.rs` `ExecutionDomain::contains`): the
/// generated dispatcher wrapper below is what actually owns routing that
/// `Transfer` to the shard function whose range contains `target`, so no
/// resolver or per-shard `BankId` is needed to stitch shards of one bank
/// back together.
///
/// `verify_live_words: false`: this gate is a compile-and-probe
/// certification run against a freshly allocated, mostly-zeroed probe
/// `Rdram`, not a live-RDRAM boot (see `emit.rs:79-82` -- "compile-only
/// generic emitter probes may leave code out of guest memory"). The probe
/// harness never loads bank words into its `Rdram`, so live-word
/// verification would spuriously fault on every probed instruction.
fn plan_bank_shards(
    bank: &MaterializedPackedBank,
    bank_index: usize,
) -> Result<Vec<BankShard>, String> {
    plan_bank_shards_with_bound(bank, bank_index, MAX_SHARD_WORDS)
}

/// [`plan_bank_shards`] with an explicit word bound.
///
/// The bound is a parameter so the within-block split path can be exercised
/// without a ROM whose blocks exceed [`MAX_SHARD_WORDS`]. Every corpus ROM
/// measured so far has blocks far smaller than the production bound, so the
/// split loop's second iteration would otherwise never run under test.
fn plan_bank_shards_with_bound(
    bank: &MaterializedPackedBank,
    bank_index: usize,
    max_shard_words: usize,
) -> Result<Vec<BankShard>, String> {
    assert!(max_shard_words > 0, "shard bound must admit at least one word");
    let bank_id = BankId::new(bank.bank_id);
    let artifact_start = bank
        .blocks
        .first()
        .map(|block| block.start_va)
        .ok_or_else(|| "bank has no blocks to shard".to_string())?;
    let artifact_end = bank
        .blocks
        .last()
        .map(|block| {
            block
                .start_va
                .checked_add(
                    u32::try_from(block.words.len())
                        .expect("block word count exceeds u32")
                        .checked_mul(4)
                        .expect("block byte length exceeds u32"),
                )
                .expect("block virtual extent exceeds u32")
        })
        .expect("bank blocks non-empty, checked above");

    // A block whose final word is a control transfer has its architectural
    // delay slot in the NEXT block when the two are adjacent. `block_pack`
    // normally folds that word into the block, but only when no other block
    // already owns it; where one does, the severed pair survives into here
    // and the emitter refuses with MissingArchitecturalDelayWord. Indexing
    // adjacency lets the last shard of such a block borrow the successor's
    // first word as its lookahead, which is exactly what that field is for.
    let next_block_first_word: BTreeMap<u32, u32> = bank
        .blocks
        .iter()
        .filter_map(|block| {
            let end_va = block.start_va.checked_add(
                u32::try_from(block.words.len()).ok()?.checked_mul(4)?,
            )?;
            Some((end_va, *block.words.first()?))
        })
        .collect();

    let mut shards = Vec::new();
    for block in &bank.blocks {
        let mut offset = 0usize;
        while offset < block.words.len() {
            let shard_len = max_shard_words.min(block.words.len() - offset);
            let shard_words = &block.words[offset..offset + shard_len];
            let shard_start = block
                .start_va
                .checked_add(u32::try_from(offset).unwrap() * 4)
                .expect("shard start exceeds u32");
            // Within a block the lookahead is simply the next word. At a
            // block end it is the adjacent block's first word, and only when
            // the two are contiguous -- a gap between blocks is not an
            // architectural successor, so a non-adjacent block never lends
            // its word.
            let delay_lookahead = block
                .words
                .get(offset + shard_len)
                .copied()
                .or_else(|| {
                    let shard_end = shard_start
                        .checked_add(u32::try_from(shard_len).ok()?.checked_mul(4)?)?;
                    next_block_first_word.get(&shard_end).copied()
                });
            let shard_index = shards.len();
            let name = format!("run_rom_bank_{bank_index}_shard_{shard_index}");
            let source = emit_dense_bank_shard_runner_function(&DenseBankShardInput {
                name: &name,
                bank: bank_id,
                image_vram_start: artifact_start,
                image_vram_end: artifact_end,
                artifact_vram_start: artifact_start,
                artifact_vram_end: artifact_end,
                shard_vram_start: shard_start,
                words: shard_words,
                delay_lookahead,
                verify_live_words: false,
            })
            .map_err(|error| {
                format!(
                    "bank {} shard {shard_index} at {shard_start:#010X}: {error:?}",
                    bank.bank
                )
            })?;
            shards.push(BankShard {
                bank_index,
                shard_index,
                start_va: shard_start,
                end_va: shard_start + shard_len as u32 * 4,
                source,
            });
            offset += shard_len;
        }
    }
    Ok(shards)
}

/// One bounded compile unit: a contiguous run of shards (in `bank_shards`
/// flattened order) whose combined emitted-code word count does not exceed
/// [`MAX_COMPILE_UNIT_WORDS`]. Since every shard is already bounded by
/// [`MAX_SHARD_WORDS`] (which equals [`MAX_COMPILE_UNIT_WORDS`]), a unit
/// holds either exactly one shard or several small ones -- never a fraction
/// of a shard, and no single shard is ever itself oversized relative to the
/// bound the way a whole unsharded bank used to be.
struct CompileUnit {
    /// Indices into the flattened shard list, in ascending order.
    shard_indices: Vec<usize>,
}

/// Partition shards into compile units by accumulated word count, in
/// deterministic flattened order. Grouping is geometry-derived (word counts
/// from the already-planned shards, iterated in the caller's fixed order) so
/// two runs on the same ROM produce identical unit boundaries.
fn plan_compile_units(shards: &[BankShard]) -> Vec<CompileUnit> {
    let mut units = Vec::new();
    let mut current = Vec::new();
    let mut current_words = 0usize;
    for (index, shard) in shards.iter().enumerate() {
        let shard_words = ((shard.end_va - shard.start_va) / 4) as usize;
        if !current.is_empty() && current_words + shard_words > MAX_COMPILE_UNIT_WORDS {
            units.push(CompileUnit {
                shard_indices: std::mem::take(&mut current),
            });
            current_words = 0;
        }
        current.push(index);
        current_words += shard_words;
    }
    if !current.is_empty() {
        units.push(CompileUnit {
            shard_indices: current,
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

/// Render one compile unit's source: this unit's shard runner functions,
/// `pub` so the driver crate (compiled and linked separately) can call them
/// directly by their `--extern` path.
fn render_shard_source(unit: &CompileUnit, shards: &[BankShard]) -> String {
    let mut source = String::from(GENERATED_PRELUDE);
    for &index in &unit.shard_indices {
        // The emitter's own output is `pub fn <name>(...)`; nothing to add.
        source.push_str(&shards[index].source);
        source.push('\n');
    }
    source
}

/// Render the driver binary's source: per-bank span tables and `CodeBank`
/// constructors, a per-bank dispatcher wrapper that routes a `pc` to the
/// shard function owning it (calling into whichever shard crate emitted
/// that shard, by its `--extern` name), the arbitrary-PC probe harness, and
/// `main`, which registers every bank's wrapper and asserts the same probes
/// and typed unaligned-PC architectural fault the unsharded gate always has.
///
/// The wrapper is what makes shard splitting transparent to `BlockProgram`:
/// `BlockProgram::register` allows exactly one runner function per `BankId`
/// (a second `register` on the same id is `ProgramError::DuplicateBank`), so
/// a bank's several shard functions cannot each be registered directly.
/// Every in-bank control transfer the shard emitter produces targets the
/// same real `BankId` (because `plan_bank_shards` sets each shard's
/// `artifact_vram_start/end` to the whole bank's extent), so it always
/// re-enters through `BlockProgram::run`, which looks the wrapper up by
/// that one `BankId` and re-dispatches by range -- no `ResolveTransfer` or
/// per-shard synthetic `BankId` is needed for shards of the *same* bank.
fn render_driver_source(
    units: &[CompileUnit],
    shards: &[BankShard],
    banks: &[MaterializedPackedBank],
) -> String {
    let mut source = String::from(GENERATED_PRELUDE);

    // Map each flattened shard index to the compile unit (crate) that holds
    // it, so the wrapper can call `shard_{unit}::run_rom_bank_{b}_shard_{s}`.
    let mut unit_of_shard = vec![0usize; shards.len()];
    for (unit_index, unit) in units.iter().enumerate() {
        for &index in &unit.shard_indices {
            unit_of_shard[index] = unit_index;
        }
    }

    for (bank_index, bank) in banks.iter().enumerate() {
        let mut bank_shards: Vec<(usize, &BankShard)> = shards
            .iter()
            .enumerate()
            .filter(|(_, shard)| shard.bank_index == bank_index)
            .collect();
        bank_shards.sort_by_key(|(_, shard)| shard.start_va);

        writeln!(source, "const SPANS_{bank_index}: &[(u32, &[u32])] = &[")
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
            "];\n\npub fn code_bank_{bank_index}() -> CodeBank {{\n    let id = BankId::new({:#018X});\n    let spans = SPANS_{bank_index}.iter().map(|(va, words)| CodeSpan::new(id, GuestPc::new(*va), words.to_vec()).unwrap()).collect();\n    CodeBank::from_spans(id, spans).unwrap()\n}}\n",
            bank.bank_id
        )
        .expect("writing generated CodeBank constructor");

        // Dispatcher wrapper: routes `entry.pc` to the shard whose owned
        // range contains it. A PC in a gap between blocks (or otherwise
        // outside every shard's range) is not architecturally reachable
        // code, matching the single-function emitter's own `_ =>` arm.
        writeln!(
            source,
            "#[inline(never)]\n#[allow(unused_variables, unused_mut)]\npub fn run_rom_bank_{bank_index}(entry: ExecutionKey, budget: InstructionBudget, ctx: &mut RecompContext, mem: &mut Rdram) -> BlockRun {{"
        )
        .expect("writing generated dispatcher wrapper header");
        writeln!(source, "    let pc = entry.pc.get();")
            .expect("writing generated dispatcher wrapper body");
        for &(flat_index, shard) in &bank_shards {
            let unit_index = unit_of_shard[flat_index];
            writeln!(
                source,
                "    if pc >= {:#010X} && pc < {:#010X} {{ return shard_{unit_index}::{}(entry, budget, ctx, mem); }}",
                shard.start_va,
                shard.end_va,
                shard.fn_name(),
            )
            .expect("writing generated dispatcher wrapper range check");
        }
        writeln!(
            source,
            "    BlockRun::new(BlockExit::Fault(CpuFault {{ at: entry, kind: CpuFaultKind::UnmappedPc {{ bank_start: {:#010X}, bank_end: {:#010X} }} }}), 0)",
            bank.blocks.first().map(|block| block.start_va).unwrap_or(0),
            bank.blocks
                .last()
                .map(|block| block.start_va + block.words.len() as u32 * 4)
                .unwrap_or(0),
        )
        .expect("writing generated dispatcher wrapper fallback");
        writeln!(source, "}}\n").expect("writing generated dispatcher wrapper footer");

        writeln!(
            source,
            "pub fn register_run_rom_bank_{bank_index}(program: &mut BlockProgram, code: CodeBank) -> Result<(), ProgramError> {{\n    program.register(code, GeneratedBankRunner::new(BankId::new({:#018X}), run_rom_bank_{bank_index}))\n}}\n",
            bank.bank_id
        )
        .expect("writing generated registration helper");
    }

    source.push_str(
        "fn probe(program: &BlockProgram, bank: BankId, pc: u32) -> BlockRun {\n    let mut storage = vec![0u8; 8 * 1024 * 1024];\n    let mut mem = Rdram::new(&mut storage);\n    let mut ctx = RecompContext::new();\n    // A fresh context zeroes every TLB entry, which makes all 32 share\n    // entry_hi = 0 and therefore match any address -- the VR4300 calls that\n    // undefined, and the runtime correctly traps it. Give each entry a\n    // distinct VPN so an arbitrary-PC probe faults on the guest code's own\n    // behavior instead of on unconfigured probe state.\n    ctx.initialize_invalid_tlb_entries();\n    ctx.set_r(29, 0x8070_0000);\n    program.run(ExecutionKey::new(bank, GuestPc::new(pc)), InstructionBudget::new(4096).unwrap(), &mut ctx, &mut mem)\n}\n\nfn main() {\n    let mut program = BlockProgram::new();\n",
    );
    for bank_index in 0..banks.len() {
        writeln!(
            source,
            "    register_run_rom_bank_{bank_index}(&mut program, code_bank_{bank_index}()).unwrap();"
        )
        .expect("writing generated registration");
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
        // silent miss: the emitter's own soundness check. `BlockProgram::run`
        // rejects an unaligned entry through `CodeCatalog::resolve` before any
        // generated runner (wrapper or not) is ever called, and that path
        // produces the architecturally precise `CpuFaultKind::Exception {
        // exception: AddressErrorLoad, .. }` (see
        // `CpuFault::instruction_address_error`'s doc comment: `UnalignedPc`
        // is reserved for the separate interpreter-fallback compatibility
        // path, which this gate never exercises). `BlockProgram::run` reports
        // this as one attempted-fetch instruction, not zero (see
        // `attempted_fetch = u32::from(matches!(fault.kind,
        // CpuFaultKind::Exception { .. }))` in `execution.rs`). Sharding did
        // not change this path or its output; the previous assertion had
        // simply never been exercised end-to-end before (see
        // task-1-report.md for two other bugs of this exact kind).
        writeln!(
            source,
            "    let unaligned = probe(&program, BankId::new({:#018X}), {:#010X});\n    assert!(matches!(unaligned.exit, BlockExit::Fault(CpuFault {{ kind: CpuFaultKind::Exception {{ exception: CpuException::AddressErrorLoad, .. }}, .. }})));\n    assert_eq!(unaligned.instructions, 1);\n    println!(\"bank={} unaligned_pc={:#010x} typed_fault=AddressErrorLoad\");",
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
    bank_shards: &[Vec<BankShard>],
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

    // Flatten in fixed (bank, shard) order -- deterministic across runs
    // since `bank_shards` was built by iterating `materialized` in its own
    // already-deterministic order.
    let shards: Vec<BankShard> = bank_shards.iter().flatten().cloned().collect();

    // Split the generated code into bounded compile units (see
    // MAX_COMPILE_UNIT_WORDS) rather than one translation unit: rustc's
    // compile time on the whole-ROM dispatch match does not scale linearly,
    // and one unbounded unit does not finish on real ROMs. Splitting
    // *emission* itself (see MAX_SHARD_WORDS / plan_bank_shards) is what
    // keeps any one shard from being the giant single function that used to
    // hit rustc's LLVM `-O` cliff regardless of unit grouping; this pass is
    // the compile-parallelism/incrementality layer on top of that.
    let units = plan_compile_units(&shards);

    let mut shard_rlibs = Vec::with_capacity(units.len());
    for (unit_index, unit) in units.iter().enumerate() {
        let shard_source = render_shard_source(unit, &shards);
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

    let driver_source = render_driver_source(&units, &shards, banks);
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

#[cfg(test)]
mod tests {
    use super::*;
    use fn64_discover::block_pack::MaterializedPackedBlock;

    /// One block of `words` MIPS words at `start_va`. The final word is a
    /// `jr $ra`, so a split whose boundary lands just before it must supply
    /// the following word as `delay_lookahead`.
    fn bank(start_va: u32, words: usize) -> MaterializedPackedBank {
        let mut body: Vec<u32> = (0..words).map(|index| 0x2408_0000 | index as u32).collect();
        if words >= 2 {
            body[words - 2] = 0x03e0_0008; // jr $ra
            body[words - 1] = 0x0000_0000; // its delay slot
        }
        MaterializedPackedBank {
            bank: "boot".to_string(),
            bank_id: 0x1234_5678_9abc_def0,
            blocks: vec![MaterializedPackedBlock {
                start_va,
                words: body,
            }],
        }
    }

    #[test]
    fn a_block_within_the_bound_emits_exactly_one_shard() {
        let shards = plan_bank_shards_with_bound(&bank(0x8000_0400, 16), 0, 64).unwrap();
        assert_eq!(shards.len(), 1);
        assert_eq!(shards[0].start_va, 0x8000_0400);
        assert_eq!(shards[0].end_va, 0x8000_0400 + 16 * 4);
    }

    /// The path the production bound never reaches: every corpus ROM's blocks
    /// are far smaller than 16,384 words, so without an explicit bound the
    /// split loop's second iteration is dead code under test.
    #[test]
    fn an_oversized_block_splits_into_contiguous_non_overlapping_shards() {
        let shards = plan_bank_shards_with_bound(&bank(0x8000_0400, 20), 0, 8).unwrap();
        assert_eq!(shards.len(), 3, "20 words at a bound of 8 is 8 + 8 + 4");

        // Contiguous, ascending, half-open, gapless: a PC in the block must
        // route to exactly one shard.
        assert_eq!(shards[0].start_va, 0x8000_0400);
        for pair in shards.windows(2) {
            assert_eq!(
                pair[0].end_va, pair[1].start_va,
                "a gap or overlap would make dispatcher routing ambiguous"
            );
        }
        assert_eq!(shards[2].end_va, 0x8000_0400 + 20 * 4);
    }

    #[test]
    fn every_shard_names_a_distinct_runner_function() {
        let shards = plan_bank_shards_with_bound(&bank(0x8000_0400, 20), 3, 8).unwrap();
        let names: std::collections::BTreeSet<String> =
            shards.iter().map(BankShard::fn_name).collect();
        assert_eq!(names.len(), shards.len());
        assert!(names.iter().all(|name| name.starts_with("run_rom_bank_3_shard_")));
    }

    /// A bank with no blocks is a caller error, not a silent empty plan.
    #[test]
    fn an_empty_bank_is_rejected() {
        let empty = MaterializedPackedBank {
            bank: "boot".to_string(),
            bank_id: 1,
            blocks: Vec::new(),
        };
        assert!(plan_bank_shards_with_bound(&empty, 0, 64).is_err());
    }
}
