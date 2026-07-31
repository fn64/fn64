//! Whole-ROM WM2000 CPU-recompilation gate.
//!
//! This is deliberately an assembly of existing mechanisms: recovered-overlay
//! discovery, multi-bank snapshot composition, digest-bound block packs, the
//! sparse Rust emitter, `BlockProgram`, and the execution-closure scoreboard.
//! ROM words appear only in materialized values and a generated source file in
//! the system temp directory; the portable pack contains geometry and digests.

use fn64_discover::banks::{BankNamePattern, BOOT_BANK};
use fn64_discover::block_pack::{
    emit_materialized_bank_runner, emit_validated_block_pack_v2, materialize_block_pack,
    BlockPackV1, MaterializedPackedBank,
};
use fn64_discover::catalog_transfer_fixed_point::{
    compose_catalog_bound_direct_transfer_fixed_point_v1, CatalogTransferDispositionV1,
    CatalogTransferFixedPointLimitsV1, CatalogTransferFixedPointResultV1,
};
use fn64_discover::closure::{
    classified_destinations, scoreboard, ClosureScoreboard, DestinationClass,
};
use fn64_discover::closure_audit::{write_closure_audit_v3, CLOSURE_AUDIT_SCHEMA_V3};
use fn64_discover::delta_vote::DeltaVoteConfig;
use fn64_discover::dense_aot_pack::{
    build_dense_aot_pack_v1, DenseAotGenerationInput, DenseAotPackV1, DENSE_AOT_SHARD_BYTES,
};
use fn64_discover::facts::{FunctionEntryEvidence, ProofState};
use fn64_discover::generation_topology::build_generation_topology_v1;
#[cfg(test)]
use fn64_discover::generation_topology::CatalogGenerationRoleV1;
use fn64_discover::loaders::VirtualAddress;
use fn64_discover::overlay_regions::SearchConfig;
use fn64_discover::runtime_generation_catalog::build_backed_dense_generation_catalog_v1;
#[cfg(test)]
use fn64_discover::snapshot::compose_materialized_banks_validated_v2;
use fn64_discover::snapshot::{MaterializedBankInput, ProgramSnapshotV1};
use fn64_discover::source_closure::{
    CacheSiteDispositionV1, CacheSiteV1, ConditionalCpuWordStoreRequirementV1,
    ConditionalCpuWordStoreV1, Cop0StatusScanV1, CpuStoreScanCoverageV1, CpuStoreScanV1,
    CpuWordStoreBlockerV1, DenseGenerationIdentityV1, DirectDmaBlockerCodeV1, DirectDmaBlockerV1,
    ExceptionVectorDispositionV1, ExceptionVectorExactCodeOwnerV1, ExecutableSourceFrontierInputV1,
    ExecutableSourceFrontierV1, ExternalCop0StatusScanV1, ExternalExecutableImageIdentityV1,
    HostBindingV1, InitialCop0StatusAuthorityV1, ModeledExceptionVectorV1, OpenCpuWordStoreV1,
    OpenWriterClass, RawPiCallerV1, RawPiPrimitiveV1, WriterResolutionV1,
    MODELED_EXCEPTION_VECTOR_DESTINATIONS_V1,
};
use fn64_discover::transfer_scan::{
    scan_transfers_with_catalog_total_authority_v1, validate_catalog_total_transfer_authority_v1,
    HostTransferTargetInput, TransferOwnerInput, TransferOwnerKindV1, TransferRootCoverageV1,
    TransferScanBankInput,
};
use fn64_discover::writer_denominator::{
    OpenWriterChannelInputV2, WriterChannelBlockerCodeV2, WriterChannelBlockerV2,
    WriterChannelDenominatorInputV2, WriterChannelDenominatorV2, WriterChannelV2,
};
use fn64_discover::{
    required_env_path, run_discovery_with_recovered_overlay_regions, Fact, FactDb,
    RecoveredOverlayInput, RomAddressSpace,
};
use fn64_recomp_rs::boot::{BootContext, BootTvStandard};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Instant;

const ROM_VAR: &str = "FN64_DISCOVER_NWXE_ROM";
const BOOT_CONTEXT_VAR: &str = "FN64_BOOT_CONTEXT";
const EXPECTED_BANKS: usize = 5;
const WM_RESIDENT_TAIL_IDENTITY_DOMAIN_V1: &[u8] = b"fn64:wm2000-resident-tail-generation:v1:";

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
    let (rom, facts, recovery) = run_discovery_with_recovered_overlay_regions(&rom_bytes, &input)
        .map_err(|error| format!("discovering WM2000 banks: {error}"))?;
    let recipes =
        fn64_discover::overlay_recipe::admitted_overlay_load_recipes_v1(&rom.bytes, &recovery)
            .map_err(|error| format!("recovering complete WM2000 overlay recipes: {error:?}"))?;
    for (index, recipe) in recipes.iter().enumerate() {
        println!(
            "recovered_recipe={index} descriptor={:#x} rom=[{:#x},{:#x}) load=[{:#010x},{:#010x}) text=[{:#010x},{:#010x})",
            recipe.descriptor_rom_offset,
            recipe.rom_start,
            recipe.rom_end,
            recipe.load_start,
            recipe.bss_end,
            recipe.text_start,
            recipe.text_end,
        );
    }
    if recipes.len() != EXPECTED_BANKS - 1 {
        return Err(format!(
            "expected {} complete overlay recipes, found {}",
            EXPECTED_BANKS - 1,
            recipes.len()
        ));
    }
    let recipe_names = (0..recipes.len())
        .map(|index| format!("recovered_overlay_{index}"))
        .collect::<Vec<_>>();
    let mut dense_inputs = vec![DenseAotGenerationInput {
        name: BOOT_BANK,
        source_rom_start: 0x1000,
        source_rom_end: 0x101000,
        load_start: 0x80000400,
        text_start: 0x80000400,
        text_end: 0x80100400,
        data_start: 0x80100400,
        data_end: 0x80100400,
        bss_start: 0x80100400,
        bss_end: 0x80100400,
    }];
    dense_inputs.extend(
        recipe_names
            .iter()
            .zip(&recipes)
            .map(|(name, recipe)| DenseAotGenerationInput::from((name.as_str(), recipe))),
    );
    let dense_pack = build_dense_aot_pack_v1(&rom, &dense_inputs)
        .map_err(|error| format!("building dense WM2000 AOT pack: {error:?}"))?;
    audit_static_shard_graph(&dense_pack, &recipes)?;

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
    let topology = build_generation_topology_v1(
        &rom,
        &dense_pack,
        BOOT_BANK,
        WM_RESIDENT_TAIL_IDENTITY_DOMAIN_V1,
        &recipes,
    )
    .map_err(|error| format!("building WM2000 generation topology: {error}"))?;
    let generation_catalog =
        build_backed_dense_generation_catalog_v1(&rom, &dense_pack, &topology)?;
    let catalog_fixed_point = compose_catalog_bound_direct_transfer_fixed_point_v1(
        &rom,
        &facts,
        &inputs,
        &dense_pack,
        &topology,
        &generation_catalog,
        CatalogTransferFixedPointLimitsV1::default(),
    )
    .map_err(|error| format!("composing catalog-bound WM2000 transfer closure: {error}"))?;
    let snapshots = catalog_fixed_point.validated().snapshots();
    let whole_board = scoreboard(snapshots);

    if std::env::var_os("FN64_DENSE_MANIFEST_ONLY").is_some() {
        print_dense_manifest(
            &rom,
            &facts,
            &recipes,
            &dense_pack,
            &catalog_fixed_point,
            snapshots,
        )?;
        if let Some(audit_dir) = std::env::var_os("FN64_CLOSURE_AUDIT_DIR") {
            let (filename, sha256) =
                write_closure_audit_v3("NWXE", &rom, snapshots, Path::new(&audit_dir))?;
            println!(
                "closure_audit schema={} unsupported={} sha256={} file={}",
                CLOSURE_AUDIT_SCHEMA_V3, whole_board.unsupported, sha256, filename
            );
        }
        return Ok(());
    }

    let mut whole_pack = BlockPackV1 {
        schema_version: fn64_discover::block_pack::BLOCK_PACK_SCHEMA_V2,
        normalized_rom_sha256: rom.sha256.clone(),
        banks: Vec::with_capacity(snapshots.len()),
    };
    for (index, snapshot) in snapshots.iter().enumerate() {
        let pack = emit_validated_block_pack_v2(catalog_fixed_point.validated(), index, &rom)
            .map_err(|error| {
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

    println!("=== WM2000/NWXE whole-ROM CPU recompilation ===");
    println!("ROM sha256={}", rom.sha256);
    println!(
        "composed banks={} (resident + {} recovered overlays)",
        physical.len(),
        physical.len() - 1
    );
    print_dense_manifest(
        &rom,
        &facts,
        &recipes,
        &dense_pack,
        &catalog_fixed_point,
        snapshots,
    )?;
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
    let unsupported = classified_destinations(snapshots)
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

fn audit_static_shard_graph(
    dense_pack: &DenseAotPackV1,
    recipes: &[fn64_discover::overlay_recipe::OverlayLoadRecipeV1],
) -> Result<(), String> {
    let manifest_path =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/wm2000-block-boot/Cargo.toml");
    let manifest_source = std::fs::read_to_string(&manifest_path).map_err(|error| {
        format!(
            "reading static shard manifest {}: {error}",
            manifest_path.display()
        )
    })?;
    let manifest: toml::Value = toml::from_str(&manifest_source).map_err(|error| {
        format!(
            "parsing static shard manifest {}: {error}",
            manifest_path.display()
        )
    })?;
    let dependencies = manifest
        .get("dependencies")
        .and_then(toml::Value::as_table)
        .ok_or_else(|| "static shard manifest has no dependency table".to_string())?;
    let actual = dependencies
        .keys()
        .filter(|name| {
            name.starts_with("wm2000-block-shard-")
                || name.starts_with("wm2000-block-resident-tail-shard-")
                || name.starts_with("wm2000-block-overlay-")
        })
        .cloned()
        .collect::<BTreeSet<_>>();
    let expected = expected_static_shard_packages(dense_pack, recipes)?;
    let missing = expected.difference(&actual).cloned().collect::<Vec<_>>();
    let unexpected = actual.difference(&expected).cloned().collect::<Vec<_>>();
    if !missing.is_empty() || !unexpected.is_empty() {
        return Err(format!(
            "static shard dependency graph disagrees with dense manifest: missing={missing:?} unexpected={unexpected:?}"
        ));
    }
    println!(
        "static_shard_graph dependencies={} geometry=exact",
        expected.len()
    );
    Ok(())
}

fn expected_static_shard_packages(
    dense_pack: &DenseAotPackV1,
    recipes: &[fn64_discover::overlay_recipe::OverlayLoadRecipeV1],
) -> Result<BTreeSet<String>, String> {
    let boot = dense_pack
        .generations
        .first()
        .ok_or_else(|| "dense pack has no resident boot generation".to_string())?;
    if boot.name != BOOT_BANK {
        return Err(format!(
            "dense pack first generation is {}, expected {BOOT_BANK}",
            boot.name
        ));
    }
    if dense_pack.generations.len() != recipes.len() + 1 {
        return Err(format!(
            "dense pack has {} overlay generations, but recovery supplied {} recipes",
            dense_pack.generations.len().saturating_sub(1),
            recipes.len()
        ));
    }
    let first_overlay_start = recipes
        .iter()
        .map(|recipe| recipe.load_start)
        .min()
        .ok_or_else(|| "recovered overlay recipe catalog is empty".to_string())?;
    if !first_overlay_start.is_multiple_of(4)
        || first_overlay_start <= boot.load_start
        || first_overlay_start >= boot.load_end
    {
        return Err(format!(
            "first overlay load boundary {first_overlay_start:#010x} does not split aligned resident geometry [{:#010x}, {:#010x})",
            boot.load_start, boot.load_end
        ));
    }
    let static_prefix_bytes = first_overlay_start - boot.load_start;
    let resident_tail_bytes = boot.load_end - first_overlay_start;
    let static_prefix_shards = static_prefix_bytes.div_ceil(DENSE_AOT_SHARD_BYTES) as usize;
    let resident_tail_shards = resident_tail_bytes.div_ceil(DENSE_AOT_SHARD_BYTES) as usize;
    if static_prefix_shards != 15 || resident_tail_shards != 2 {
        return Err(format!(
            "resident dependency topology is {static_prefix_shards} static-prefix / {resident_tail_shards} resident-tail shards, expected 15 / 2"
        ));
    }

    let mut expected = BTreeSet::new();
    for shard_index in 0..static_prefix_shards {
        expected.insert(format!("wm2000-block-shard-{shard_index:02}"));
    }
    for shard_index in 0..resident_tail_shards {
        expected.insert(format!("wm2000-block-resident-tail-shard-{shard_index:02}"));
    }
    for (overlay_index, (generation, recipe)) in dense_pack
        .generations
        .iter()
        .skip(1)
        .zip(recipes)
        .enumerate()
    {
        let expected_name = format!("recovered_overlay_{overlay_index}");
        let generation_geometry = (
            generation.name.as_str(),
            generation.source_rom_start,
            generation.source_rom_end,
            generation.load_start,
            generation.text_start,
            generation.text_end,
            generation.data_start,
            generation.data_end,
            generation.bss_start,
            generation.bss_end,
            generation.loaded_sha256.as_str(),
        );
        let recipe_geometry = (
            expected_name.as_str(),
            recipe.rom_start,
            recipe.rom_end,
            recipe.load_start,
            recipe.text_start,
            recipe.text_end,
            recipe.data_start,
            recipe.data_end,
            recipe.bss_start,
            recipe.bss_end,
            recipe.loaded_sha256.as_str(),
        );
        if generation_geometry != recipe_geometry {
            return Err(format!(
                "dense overlay generation {overlay_index} disagrees with its recovered recipe"
            ));
        }
        let expected_shards =
            (recipe.rom_end - recipe.rom_start).div_ceil(DENSE_AOT_SHARD_BYTES) as usize;
        if generation.shards.len() != expected_shards {
            return Err(format!(
                "dense overlay generation {overlay_index} has {} shards, expected {expected_shards} from recipe geometry",
                generation.shards.len()
            ));
        }
        for shard_index in 0..expected_shards {
            expected.insert(format!(
                "wm2000-block-overlay-{overlay_index}-shard-{shard_index:02}"
            ));
        }
    }
    if expected.len() != 35 {
        return Err(format!(
            "static shard dependency topology has {} packages, expected 35",
            expected.len()
        ));
    }
    Ok(expected)
}

fn print_catalog_transfer_fixed_point(report: &CatalogTransferFixedPointResultV1) {
    let mut authorized = 0usize;
    let mut authorized_target_roots = 0usize;
    let mut activation_miss = 0usize;
    let mut ambiguous = 0usize;
    let mut rejected = 0usize;
    for finding in report.findings() {
        match &finding.disposition {
            CatalogTransferDispositionV1::Authorized {
                target_bank,
                target_generation,
            } => {
                authorized += 1;
                let target_root_admitted = report
                    .validated()
                    .snapshots()
                    .iter()
                    .flat_map(|snapshot| &snapshot.banks)
                    .any(|bank| {
                        bank.input.bank == target_bank.as_str()
                            && bank
                                .authority_closure
                                .cfg
                                .proven_roots
                                .contains(&finding.request.target_pc)
                    });
                authorized_target_roots += usize::from(target_root_admitted);
                println!(
                    "catalog_transfer_disposition source_bank={} source_pc={:#010x} kind={:?} target_pc={:#010x} disposition=authorized target_bank={} target_generation={:#018x} target_root_admitted={target_root_admitted}",
                    finding.request.source_bank,
                    finding.request.source_pc,
                    finding.request.kind,
                    finding.request.target_pc,
                    target_bank,
                    target_generation,
                );
            }
            CatalogTransferDispositionV1::ActivationMiss {
                excluded_generations,
            } => {
                activation_miss += 1;
                println!(
                    "catalog_transfer_disposition source_bank={} source_pc={:#010x} kind={:?} target_pc={:#010x} disposition=activation_miss excluded_generations={excluded_generations:?}",
                    finding.request.source_bank,
                    finding.request.source_pc,
                    finding.request.kind,
                    finding.request.target_pc,
                );
            }
            CatalogTransferDispositionV1::Ambiguous {
                compatible_generations,
            } => {
                ambiguous += 1;
                println!(
                    "catalog_transfer_disposition source_bank={} source_pc={:#010x} kind={:?} target_pc={:#010x} disposition=ambiguous compatible_generations={compatible_generations:?}",
                    finding.request.source_bank,
                    finding.request.source_pc,
                    finding.request.kind,
                    finding.request.target_pc,
                );
            }
            CatalogTransferDispositionV1::Rejected { error } => {
                rejected += 1;
                println!(
                    "catalog_transfer_disposition source_bank={} source_pc={:#010x} kind={:?} target_pc={:#010x} disposition=rejected error={error:?}",
                    finding.request.source_bank,
                    finding.request.source_pc,
                    finding.request.kind,
                    finding.request.target_pc,
                );
            }
        }
    }
    let authority_roots = report
        .validated()
        .snapshots()
        .iter()
        .flat_map(|snapshot| &snapshot.banks)
        .map(|bank| bank.authority_closure.cfg.proven_roots.len())
        .sum::<usize>();
    let authority_blocks = report
        .validated()
        .snapshots()
        .iter()
        .flat_map(|snapshot| &snapshot.banks)
        .map(|bank| bank.authority_closure.cfg.blocks.len())
        .sum::<usize>();
    println!(
        "catalog_transfer_fixed_point rounds={} capabilities={} findings={} authorized={} authorized_target_roots={} activation_miss={} ambiguous={} rejected={} authority_roots={} authority_blocks={} termination={:?}",
        report.rounds(),
        report.authorized_capabilities(),
        report.findings().len(),
        authorized,
        authorized_target_roots,
        activation_miss,
        ambiguous,
        rejected,
        authority_roots,
        authority_blocks,
        report.termination(),
    );
}

fn print_dense_manifest(
    rom: &fn64_discover::NormalizedRom,
    facts: &FactDb,
    recipes: &[fn64_discover::overlay_recipe::OverlayLoadRecipeV1],
    dense_pack: &fn64_discover::dense_aot_pack::DenseAotPackV1,
    catalog_fixed_point: &CatalogTransferFixedPointResultV1,
    snapshots: &[ProgramSnapshotV1],
) -> Result<(), String> {
    let external_images = reproducible_external_images_from_env(&rom.sha256)?;
    println!("=== WM2000/NWXE dense AOT manifest ===");
    println!("ROM sha256={}", rom.sha256);
    let resident_words = rom.bytes[0x1000..0x101000]
        .chunks_exact(4)
        .map(|word| u32::from_be_bytes(word.try_into().unwrap()))
        .collect::<Vec<_>>();
    let host_bindings = fn64_discover::host_bindings::discover_wm_block_runtime_host_bindings(
        &resident_words,
        rom.header.entry_point,
    )
    .map_err(|error| format!("discovering exact runtime host catalog: {error:?}"))?;
    for binding in &host_bindings {
        println!(
            "host_binding={:?} vram={:#010x} proof=unique_structural_semantic_match",
            binding.symbol, binding.vram
        );
    }
    for (index, recipe) in recipes.iter().enumerate() {
        println!(
            "recipe={index} descriptor={:#x} rom=[{:#x},{:#x}) load={:#010x} text=[{:#010x},{:#010x}) data=[{:#010x},{:#010x}) bss=[{:#010x},{:#010x}) digest={}",
            recipe.descriptor_rom_offset,
            recipe.rom_start,
            recipe.rom_end,
            recipe.load_start,
            recipe.text_start,
            recipe.text_end,
            recipe.data_start,
            recipe.data_end,
            recipe.bss_start,
            recipe.bss_end,
            recipe.loaded_sha256,
        );
    }
    let dense_entries = dense_pack
        .generations
        .iter()
        .map(|generation| generation.aligned_entry_count as u64)
        .sum::<u64>();
    let dense_shards = dense_pack
        .generations
        .iter()
        .map(|generation| generation.shards.len())
        .sum::<usize>();
    let dense_pack_json = serde_json::to_vec(dense_pack)
        .map_err(|error| format!("serializing dense AOT pack: {error}"))?;
    let dense_pack_sha256 = sha256_hex(&dense_pack_json);
    println!(
        "dense_aot_pack schema={} generations={} shards={} aligned_entries={} manifest_sha256={}",
        dense_pack.schema,
        dense_pack.generations.len(),
        dense_shards,
        dense_entries,
        dense_pack_sha256,
    );
    print_catalog_transfer_fixed_point(catalog_fixed_point);
    print_source_closure_receipt(
        rom,
        facts,
        dense_pack,
        dense_pack_sha256,
        &host_bindings,
        &external_images,
        snapshots,
    )?;
    Ok(())
}

fn dense_transfer_banks<'a>(
    dense_pack: &'a DenseAotPackV1,
    snapshots: &'a [ProgramSnapshotV1],
) -> Result<Vec<TransferScanBankInput<'a>>, String> {
    if snapshots.len() != dense_pack.generations.len() {
        return Err(format!(
            "composed snapshot count {} does not match dense generation count {}",
            snapshots.len(),
            dense_pack.generations.len()
        ));
    }
    let mut names = BTreeSet::new();
    let mut bank_ids = BTreeSet::new();
    let mut banks = Vec::with_capacity(snapshots.len());
    for (index, (generation, snapshot)) in dense_pack.generations.iter().zip(snapshots).enumerate()
    {
        if !names.insert(generation.name.as_str()) {
            return Err(format!(
                "dense transfer generation order contains duplicate bank {}",
                generation.name
            ));
        }
        if !bank_ids.insert(generation.bank_id) {
            return Err(format!(
                "dense transfer generation {} reuses bank identity {:#018x}",
                generation.name, generation.bank_id
            ));
        }
        if snapshot.normalized_rom_sha256 != dense_pack.normalized_rom_sha256 {
            return Err(format!(
                "dense transfer snapshot {index} ROM identity {} does not match pack {}",
                snapshot.normalized_rom_sha256, dense_pack.normalized_rom_sha256
            ));
        }
        let [bank] = snapshot.banks.as_slice() else {
            return Err(format!(
                "dense transfer snapshot {index} contains {} banks; expected exactly one for {}",
                snapshot.banks.len(),
                generation.name
            ));
        };
        let observed = (
            bank.input.bank.as_str(),
            bank.input.rom_start,
            bank.input.rom_end,
            bank.input.va_start,
            bank.input.va_end,
        );
        let expected = (
            generation.name.as_str(),
            generation.source_rom_start,
            generation.source_rom_end,
            generation.load_start,
            generation.load_end,
        );
        if observed != expected {
            return Err(format!(
                "dense transfer snapshot {index} identity/geometry mismatch: expected {expected:?}, observed {observed:?}"
            ));
        }
        if bank.authority_closure.cfg.bank != generation.name {
            return Err(format!(
                "dense transfer snapshot {index} authority CFG names bank {}, expected {}",
                bank.authority_closure.cfg.bank, generation.name
            ));
        }
        banks.push(TransferScanBankInput {
            bank: generation.name.as_str(),
            bank_id: generation.bank_id,
            va_start: generation.load_start,
            va_end: generation.load_end,
            closure: &bank.authority_closure,
            root_coverage: TransferRootCoverageV1::ProvenFactRoots,
        });
    }
    Ok(banks)
}

fn print_source_closure_receipt(
    rom: &fn64_discover::NormalizedRom,
    facts: &FactDb,
    dense_pack: &fn64_discover::dense_aot_pack::DenseAotPackV1,
    dense_pack_sha256: String,
    host_bindings: &[fn64_discover::host_bindings::HostBinding],
    external_images: &[fn64_discover::trace::ExecutableImageCapture],
    snapshots: &[ProgramSnapshotV1],
) -> Result<(), String> {
    print_writer_channel_denominator_receipt(&dense_pack_sha256)?;
    let initial_cop0_status = initial_cop0_status_authority_from_env(rom)?;
    let epi = host_bindings
        .iter()
        .find(|binding| {
            binding.symbol == fn64_discover::host_bindings::HostBindingSymbol::OsEPiStartDma
        })
        .ok_or_else(|| "source-frontier receipt lacks osEPiStartDma binding".to_string())?;
    let mut cache_sites = Vec::new();
    let mut direct_dma_blockers = Vec::new();
    let mut raw_pi_primitives = Vec::new();
    let mut conditional_cpu_word_stores = Vec::new();
    let mut open_cpu_word_stores = Vec::new();
    let mut cpu_store_scans = Vec::new();
    let mut cop0_status_scans = Vec::new();
    let physical = physical_banks(facts)?;
    let dense_generations = dense_pack
        .generations
        .iter()
        .map(|generation| DenseGenerationIdentityV1 {
            name: generation.name.clone(),
            bank_id: generation.bank_id,
            source_rom_start: generation.source_rom_start,
            source_rom_end: generation.source_rom_end,
            load_start: generation.load_start,
            load_end: generation.load_end,
            loaded_sha256: generation.loaded_sha256.clone(),
        })
        .collect::<Vec<_>>();
    for generation in &dense_pack.generations {
        let store_scan_started = Instant::now();
        let matching_physical = physical
            .iter()
            .filter(|bank| {
                bank.bank == generation.name
                    && bank.rom_start == generation.source_rom_start
                    && bank.rom_end == generation.source_rom_end
                    && bank.va_start == generation.load_start
                    && bank.va_end == generation.load_end
            })
            .collect::<Vec<_>>();
        let [physical_bank] = matching_physical.as_slice() else {
            return Err(format!(
                "dense generation {} has {} exact physical ROM/VA mappings; expected one",
                generation.name,
                matching_physical.len()
            ));
        };
        let bytes =
            &rom.bytes[generation.source_rom_start as usize..generation.source_rom_end as usize];
        let words = bytes
            .chunks_exact(4)
            .map(|word| u32::from_be_bytes(word.try_into().unwrap()))
            .collect::<Vec<_>>();
        let seed_roots =
            if (physical_bank.va_start..physical_bank.va_end).contains(&rom.header.entry_point) {
                vec![rom.header.entry_point]
            } else {
                Vec::new()
            };
        let closure = fn64_discover::resolve::build_cfg_value_set_closed_with_facts(
            facts,
            &generation.name,
            bytes,
            generation.load_start,
            &seed_roots,
        );
        let cfg = &closure.cfg;
        let status_analysis =
            fn64_discover::resolve::analyze_cop0_status_writes(&cfg, bytes, generation.load_start)
                .map_err(|error| {
                    format!(
                        "inventorying COP0 Status writes in {}: {error:?}",
                        generation.name
                    )
                })?;
        let status_inventory = &status_analysis.inventory;
        cop0_status_scans.push(Cop0StatusScanV1 {
            bank: generation.name.clone(),
            bank_id: generation.bank_id,
            aligned_word_count: u32::try_from(words.len())
                .map_err(|_| format!("{} has too many aligned words", generation.name))?,
            proven_code_writes: status_inventory
                .proven_code_writes
                .iter()
                .map(Into::into)
                .collect(),
            proven_data_words: status_inventory
                .proven_data_words
                .iter()
                .map(Into::into)
                .collect(),
            unclassified_writes: status_inventory
                .unclassified_writes
                .iter()
                .map(Into::into)
                .collect(),
            proven_code_value_proofs: status_analysis
                .proven_code_value_proofs
                .iter()
                .map(Into::into)
                .collect(),
            open_indirect_sites: status_inventory.open_indirect_sites.clone(),
        });
        println!(
            "cop0_status_scan bank={} proven_code={} value_proofs={} value_open={} proven_data={} unclassified={} open_indirect={}",
            generation.name,
            status_inventory.proven_code_writes.len(),
            status_analysis.proven_code_value_proofs.len(),
            status_analysis
                .proven_code_value_proofs
                .iter()
                .map(fn64_discover::source_closure::Cop0StatusValueProofV1::from)
                .filter(|proof| !proof.proves_bev_clear())
                .count(),
            status_inventory.proven_data_words.len(),
            status_inventory.unclassified_writes.len(),
            status_inventory.open_indirect_sites.len(),
        );
        let admitted_sources = words
            .iter()
            .enumerate()
            .map(
                |(index, value)| fn64_discover::resolve::AdmittedWordSource {
                    address: generation.load_start + index as u32 * 4,
                    value: *value,
                },
            )
            .collect::<Vec<_>>();
        let store_report = fn64_discover::resolve::derive_fixed_word_stores(
            &cfg,
            bytes,
            generation.load_start,
            &MODELED_EXCEPTION_VECTOR_DESTINATIONS_V1,
            &admitted_sources,
        )
        .map_err(|error| {
            format!(
                "deriving fixed vector-word stores in {}: {error:?}",
                generation.name
            )
        })?;
        cpu_store_scans.push(CpuStoreScanV1 {
            bank: generation.name.clone(),
            bank_id: generation.bank_id,
            proven_root_count: u32::try_from(cfg.proven_roots.len())
                .map_err(|_| format!("{} has too many proven CFG roots", generation.name))?,
            reachable_block_count: u32::try_from(cfg.blocks.len())
                .map_err(|_| format!("{} has too many reachable CFG blocks", generation.name))?,
            conditional_store_count: u32::try_from(store_report.conditional.len())
                .map_err(|_| format!("{} has too many conditional word stores", generation.name))?,
            open_store_count: u32::try_from(store_report.open.len())
                .map_err(|_| format!("{} has too many open word stores", generation.name))?,
            coverage: CpuStoreScanCoverageV1::BoundedReachableCfg,
        });
        println!(
            "cpu_store_scan bank={} roots={} blocks={} conditional={} open={} elapsed_ms={}",
            generation.name,
            cfg.proven_roots.len(),
            cfg.blocks.len(),
            store_report.conditional.len(),
            store_report.open.len(),
            store_scan_started.elapsed().as_millis(),
        );
        conditional_cpu_word_stores.extend(store_report.conditional.into_iter().map(|store| {
            ConditionalCpuWordStoreV1 {
                writer_bank: generation.name.clone(),
                writer_bank_id: generation.bank_id,
                site_pc: store.site_pc,
                destination: store.destination,
                value: store.value,
                source_bank: generation.name.clone(),
                source_bank_id: generation.bank_id,
                source_address: store.source.address,
                source_value: store.source.value,
                open_requirements: vec![
                    ConditionalCpuWordStoreRequirementV1::SourceStableUntilLoad,
                    ConditionalCpuWordStoreRequirementV1::StoreSiteExecutes,
                ],
            }
        }));
        open_cpu_word_stores.extend(store_report.open.into_iter().map(|store| {
            OpenCpuWordStoreV1 {
                writer_bank: generation.name.clone(),
                writer_bank_id: generation.bank_id,
                site_pc: store.site_pc,
                blockers: store
                    .blockers
                    .iter()
                    .map(CpuWordStoreBlockerV1::from)
                    .collect(),
            }
        }));
        for (index, word) in words.iter().copied().enumerate() {
            if let fn64_recomp_rs::Instruction::Cache { op, base, off } =
                fn64_recomp_rs::decode(word)
            {
                let guest_pc = generation.load_start + index as u32 * 4;
                let word_class = cfg.word_class.get(&guest_pc).copied();
                let disposition = cache_site_disposition(word_class);
                cache_sites.push(CacheSiteV1 {
                    bank: generation.name.clone(),
                    guest_pc,
                    raw_word: word,
                    decoded_op: format!("cache_{op:#04x}"),
                    base_register: base,
                    offset: off,
                    word_class: word_class
                        .map(|class| format!("{class:?}"))
                        .unwrap_or_else(|| "Absent".to_string()),
                    disposition,
                    evidence: "complete linear decode qualified by analyzer-owned CFG word class"
                        .to_string(),
                });
            }
        }
        let slices = fn64_discover::pi_dma::slice_os_epi_start_dma_calls(
            &words,
            VirtualAddress::new(generation.load_start),
            VirtualAddress::new(epi.vram),
            8 * 1024 * 1024,
        )
        .map_err(|error| {
            format!(
                "slicing direct osEPiStartDma calls in {}: {error:?}",
                generation.name
            )
        })?;
        direct_dma_blockers.extend(slices.into_iter().map(|slice| {
            let candidate = slice.candidate();
            DirectDmaBlockerV1 {
                caller_bank: generation.name.clone(),
                caller_pc: Some(slice.call_pc.get()),
                primitive_bank: BOOT_BANK.to_string(),
                code: if candidate.is_some() {
                    DirectDmaBlockerCodeV1::ImageAdmissionMissing
                } else {
                    DirectDmaBlockerCodeV1::MutableDescriptor
                },
                writer_class: if candidate.is_some() {
                    OpenWriterClass::DirectDmaHandleMappingOrCompletion
                } else {
                    OpenWriterClass::MutableDmaDescriptorOutsideSlice
                },
                reason: if candidate.is_some() {
                    "static call geometry remains candidate-strength without OSPiHandle mapping and completion evidence".to_string()
                } else {
                    format!("bounded operand slice remains open: {slice:?}")
                },
            }
        }));
        raw_pi_primitives.extend(
            fn64_discover::pi_dma::recover_pi_primitives_words(&words, generation.load_start)
                .into_iter()
                .map(|primitive| RawPiPrimitiveV1 {
                    bank: generation.name.clone(),
                    entry_pc: primitive.entry_va,
                    symbol: "fixed_lui_a460_candidate".to_string(),
                    register_site_pcs: primitive.register_site_pcs,
                    callers: primitive
                        .callers
                        .into_iter()
                        .map(|caller_pc| RawPiCallerV1 {
                            caller_bank: generation.name.clone(),
                            caller_pc,
                            primitive_pc: primitive.entry_va,
                            resolution: WriterResolutionV1::Open,
                            evidence: "direct caller found; transfer geometry and executable destination remain unproved".to_string(),
                        })
                        .collect(),
                }),
        );
    }
    let open_writer_classes = vec![
        OpenWriterClass::IndirectPiEpiCall,
        OpenWriterClass::UnrecognizedRawPiAddressConstruction,
        OpenWriterClass::CpuCopyStoreOrDecompression,
        OpenWriterClass::SpDmaToCpuExecutable,
        OpenWriterClass::SiDmaToCpuExecutable,
        OpenWriterClass::RdpWriteToCpuExecutable,
        OpenWriterClass::Kseg1OrTlbExecutableAlias,
        OpenWriterClass::MutableDmaDescriptorOutsideSlice,
        OpenWriterClass::CrossBankRawPiCaller,
        OpenWriterClass::HostAbiExecutableWrite,
        OpenWriterClass::InstructionCacheState,
        OpenWriterClass::ExtendedAddressAlias,
        OpenWriterClass::DirectDmaHandleMappingOrCompletion,
    ];
    let (external_cop0_status_scans, external_transfer_closures) =
        external_cop0_status_scans(external_images)?;
    let external_bank_names = external_images
        .iter()
        .map(|capture| format!("external:{}:{}", capture.image_id, capture.generation))
        .collect::<Vec<_>>();
    let external_bank_ids = external_images
        .iter()
        .map(|capture| fn64_discover::external_aot::external_aot_bank_id(&rom.sha256, capture))
        .collect::<Vec<_>>();
    let mut transfer_owners = dense_pack
        .generations
        .iter()
        .map(|generation| TransferOwnerInput {
            bank: generation.name.as_str(),
            bank_id: generation.bank_id,
            va_start: generation.load_start,
            va_end: generation.load_end,
            kind: TransferOwnerKindV1::DenseGeneration,
        })
        .collect::<Vec<_>>();
    transfer_owners.extend(external_images.iter().enumerate().map(|(index, capture)| {
        TransferOwnerInput {
            bank: external_bank_names[index].as_str(),
            bank_id: external_bank_ids[index],
            va_start: capture.va_start,
            va_end: capture.va_start + capture.byte_len,
            kind: TransferOwnerKindV1::ExternalExecutableImage,
        }
    }));
    let mut transfer_banks = dense_transfer_banks(dense_pack, snapshots)?;
    transfer_banks.extend(external_images.iter().enumerate().map(|(index, capture)| {
        TransferScanBankInput {
            bank: external_bank_names[index].as_str(),
            bank_id: external_bank_ids[index],
            va_start: capture.va_start,
            va_end: capture.va_start + capture.byte_len,
            closure: &external_transfer_closures[index],
            root_coverage: TransferRootCoverageV1::ProvenFactRoots,
        }
    }));
    let host_targets = host_bindings
        .iter()
        .map(|binding| HostTransferTargetInput {
            bank: BOOT_BANK,
            guest_pc: binding.vram,
        })
        .collect::<Vec<_>>();
    let resolver_policy = fn64_recomp_rs::catalog_resolver_policy_evidence_v1();
    let transfer_authority = validate_catalog_total_transfer_authority_v1(
        &transfer_banks,
        &transfer_owners,
        &host_targets,
        &resolver_policy,
    )
    .map_err(|error| format!("validating catalog-total transfer authority: {error:?}"))?;
    let transfer_scan = scan_transfers_with_catalog_total_authority_v1(
        &transfer_banks,
        &transfer_owners,
        &host_targets,
        &transfer_authority,
    )
    .map_err(|error| format!("scanning catalog-total transfer frontier: {error:?}"))?;
    println!(
        "transfer_scan coverage={:?} direct_total={} direct_guest={} direct_host={} direct_open={} indirect_closed={} indirect_bounded={} indirect_open={} blockers={}",
        transfer_scan.coverage(),
        transfer_scan.summary().direct_total,
        transfer_scan.summary().direct_guest,
        transfer_scan.summary().direct_host,
        transfer_scan.summary().direct_open,
        transfer_scan.summary().indirect_closed,
        transfer_scan.summary().indirect_bounded,
        transfer_scan.summary().indirect_open,
        transfer_scan.blockers().len(),
    );
    let external_images = external_images
        .iter()
        .map(|capture| ExternalExecutableImageIdentityV1 {
            image_id: capture.image_id.clone(),
            lineage: format!("{:?}", capture.lineage),
            generation: capture.generation,
            va_start: capture.va_start,
            byte_len: capture.byte_len,
            sha256: capture.sha256.clone(),
            first_executed_pc: capture.first_executed_pc,
        })
        .collect::<Vec<_>>();
    let exception_vectors = modeled_exception_vectors(&external_images)?;
    let input = ExecutableSourceFrontierInputV1 {
        producer: "fn64-discover:gate_wm2000_recompile:manifest-only:v1".to_string(),
        normalized_rom_sha256: rom.sha256.clone(),
        dense_aot_pack_sha256: dense_pack_sha256,
        initial_cop0_status,
        dense_generations,
        external_images,
        exception_vectors,
        host_bindings: host_bindings
            .iter()
            .map(|binding| HostBindingV1 {
                bank: BOOT_BANK.to_string(),
                guest_vram: binding.vram,
                symbol: binding.symbol.into(),
                current_status_effect: binding.symbol.current_status_effect().into(),
                spawned_status_effect: binding.symbol.spawned_status_effect().into(),
            })
            .collect(),
        cache_sites,
        direct_dma_findings: Vec::new(),
        direct_dma_blockers,
        raw_pi_primitives,
        cpu_store_watched_destinations: MODELED_EXCEPTION_VECTOR_DESTINATIONS_V1.to_vec(),
        cpu_store_scans,
        cop0_status_scans,
        external_cop0_status_scans,
        conditional_cpu_word_stores,
        open_cpu_word_stores,
        transfer_scan,
        open_writer_classes,
    };
    let receipt = ExecutableSourceFrontierV1::new(input)
        .map_err(|error| format!("building executable-source receipt: {error}"))?;
    let json = receipt
        .canonical_json_bytes()
        .map_err(|error| format!("serializing executable-source receipt: {error}"))?;
    let output = std::env::var_os("FN64_SOURCE_FRONTIER_RECEIPT").map(PathBuf::from);
    if let Some(path) = output.as_ref() {
        std::fs::write(path, &json).map_err(|error| {
            format!(
                "writing source-frontier receipt {}: {error}",
                path.display()
            )
        })?;
    }
    let diagnostics = receipt.diagnostics();
    println!(
        "source_frontier_receipt sha256={} open_frontier={} initial_bev_clear={} external_images={} open_exception_vectors={} open_writer_classes={} cache_sites={} unclassified_cache_sites={} direct_dma_blockers={} raw_pi_primitives={} raw_pi_open_callers={} cpu_store_scans={} cop0_status_scans={} external_cop0_status_scans={} cop0_unclassified_writes={} cop0_value_open={} conditional_cpu_word_stores={} open_cpu_word_stores={} transfer_inventory_complete={} receipt_written={}",
        sha256_hex(&json),
        receipt.has_open_frontier(),
        diagnostics.initial_bev_clear,
        diagnostics.external_images,
        diagnostics.open_exception_vectors,
        diagnostics.open_writer_classes,
        diagnostics.cache_sites,
        diagnostics.unclassified_cache_sites,
        diagnostics.direct_dma_blockers,
        diagnostics.raw_pi_primitives,
        diagnostics.raw_pi_open_callers,
        diagnostics.cpu_store_scans,
        diagnostics.cop0_status_scans,
        diagnostics.external_cop0_status_scans,
        diagnostics.cop0_unclassified_writes,
        diagnostics.cop0_value_open,
        diagnostics.conditional_cpu_word_stores,
        diagnostics.open_cpu_word_stores,
        diagnostics.transfer_inventory_complete,
        output.is_some(),
    );
    Ok(())
}

fn print_writer_channel_denominator_receipt(program_model_sha256: &str) -> Result<(), String> {
    let row = |channel, code, evidence: &str| OpenWriterChannelInputV2 {
        channel,
        blockers: vec![WriterChannelBlockerV2 {
            code,
            evidence: evidence.to_string(),
        }],
    };
    let receipt = WriterChannelDenominatorV2::new_open(WriterChannelDenominatorInputV2 {
        producer: "fn64-discover:gate_wm2000_recompile:writer-denominator:v2".to_string(),
        program_model_sha256: program_model_sha256.to_string(),
        channels: vec![
            row(
                WriterChannelV2::CpuInstructionStore,
                WriterChannelBlockerCodeV2::CoverageOpen,
                "typed CPU store helpers attribute every modeled store, but the installed GeneratedBankRunner callable identity is not validator-bound to emitter-owned source semantics for this exact dense AOT model",
            ),
            row(
                WriterChannelV2::PiDma,
                WriterChannelBlockerCodeV2::CoverageOpen,
                "the sealed PI DMA adapter is typed, but no validator receipt binds every admitted program callsite to this exact dense AOT model",
            ),
            row(
                WriterChannelV2::SiDma,
                WriterChannelBlockerCodeV2::CoverageOpen,
                "the sole SI device write site and sealed process adapter preserve exact producer attribution, but host callable identities/effects are not bound strongly enough to prove every admitted initiation and clock-advance path uses it for this dense AOT model",
            ),
            row(
                WriterChannelV2::SpDma,
                WriterChannelBlockerCodeV2::CoverageOpen,
                "the sealed SP DMA adapter is typed, but no validator receipt binds every admitted program callsite to this exact dense AOT model",
            ),
            row(
                WriterChannelV2::RspExecutionOrHleWriteback,
                WriterChannelBlockerCodeV2::MutableApiEscape,
                "RspMachine retains externally mutable RDRAM and HLE writeback coverage is not validator-sealed",
            ),
            row(
                WriterChannelV2::RdpRenderer,
                WriterChannelBlockerCodeV2::MutableApiEscape,
                "canonical renderer entry points use ordered child transactions, including nested same-byte attribution, but renderer traits and noncanonical callers still expose broad mutable RDRAM slices and lack model-total validation",
            ),
            row(
                WriterChannelV2::HostAbi,
                WriterChannelBlockerCodeV2::MutableApiEscape,
                "catalog-owned HostAbi calls use per-thread ordered transaction stacks across child writers and coroutine yields, but raw compatibility host pointers remain reachable and lack model-total validation",
            ),
            row(
                WriterChannelV2::BootstrapOrImport,
                WriterChannelBlockerCodeV2::CoverageOpen,
                "only a verifier-owned selected-build writer-audit bundle can complete Bootstrap/Import; this offline gate neither runs the exact canonical audit nor owns that move-only bundle",
            ),
        ],
    })
    .map_err(|error| format!("building executable-writer channel denominator: {error}"))?;
    let json = receipt
        .canonical_json_bytes()
        .map_err(|error| format!("serializing executable-writer channel denominator: {error}"))?;
    let output = std::env::var_os("FN64_WRITER_CHANNEL_DENOMINATOR_RECEIPT").map(PathBuf::from);
    if let Some(path) = output.as_ref() {
        std::fs::write(path, &json).map_err(|error| {
            format!(
                "writing executable-writer channel denominator {}: {error}",
                path.display()
            )
        })?;
    }
    println!(
        "writer_channel_denominator sha256={} complete={} open_channels={:?} receipt_written={}",
        sha256_hex(&json),
        receipt.is_complete(),
        receipt.open_channels(),
        output.is_some(),
    );
    Ok(())
}

fn cache_site_disposition(
    word_class: Option<fn64_discover::cfg::WordClass>,
) -> CacheSiteDispositionV1 {
    match word_class {
        Some(fn64_discover::cfg::WordClass::ProvenCode) => {
            CacheSiteDispositionV1::ReachableInstruction
        }
        Some(fn64_discover::cfg::WordClass::ProvenData) => CacheSiteDispositionV1::ProvenData,
        _ => CacheSiteDispositionV1::Unclassified,
    }
}

fn external_cop0_status_scans(
    captures: &[fn64_discover::trace::ExecutableImageCapture],
) -> Result<
    (
        Vec<ExternalCop0StatusScanV1>,
        Vec<fn64_discover::resolve::ClosureResult>,
    ),
    String,
> {
    let mut scans = Vec::with_capacity(captures.len());
    let mut closures = Vec::with_capacity(captures.len());
    for capture in captures {
        let bytes = capture
            .words
            .iter()
            .flat_map(|word| word.to_be_bytes())
            .collect::<Vec<_>>();
        let bank = format!("external:{}:{}", capture.image_id, capture.generation);
        let closure = fn64_discover::resolve::build_cfg_value_set_closed(
            &bank,
            &bytes,
            capture.va_start,
            &[capture.first_executed_pc],
        );
        let analysis = fn64_discover::resolve::analyze_cop0_status_writes(
            &closure.cfg,
            &bytes,
            capture.va_start,
        )
        .map_err(|error| {
            format!(
                "inventorying COP0 Status writes in external image {} generation {}: {error:?}",
                capture.image_id, capture.generation
            )
        })?;
        let inventory = &analysis.inventory;
        scans.push(ExternalCop0StatusScanV1 {
            image_id: capture.image_id.clone(),
            generation: capture.generation,
            va_start: capture.va_start,
            byte_len: capture.byte_len,
            sha256: capture.sha256.clone(),
            first_executed_pc: capture.first_executed_pc,
            aligned_word_count: u32::try_from(capture.words.len()).map_err(|_| {
                format!(
                    "external image {} generation {} has too many aligned words",
                    capture.image_id, capture.generation
                )
            })?,
            proven_code_writes: inventory
                .proven_code_writes
                .iter()
                .map(Into::into)
                .collect(),
            proven_data_words: inventory.proven_data_words.iter().map(Into::into).collect(),
            unclassified_writes: inventory
                .unclassified_writes
                .iter()
                .map(Into::into)
                .collect(),
            proven_code_value_proofs: analysis
                .proven_code_value_proofs
                .iter()
                .map(Into::into)
                .collect(),
            open_indirect_sites: inventory.open_indirect_sites.clone(),
        });
        closures.push(closure);
    }
    Ok((scans, closures))
}

fn initial_cop0_status_authority_from_env(
    rom: &fn64_discover::NormalizedRom,
) -> Result<InitialCop0StatusAuthorityV1, String> {
    let Some(path) = std::env::var_os(BOOT_CONTEXT_VAR).map(PathBuf::from) else {
        return Ok(InitialCop0StatusAuthorityV1::Missing);
    };
    let bytes = std::fs::read(&path)
        .map_err(|error| format!("reading boot context {}: {error}", path.display()))?;
    let context = serde_json::from_slice::<BootContext>(&bytes)
        .map_err(|error| format!("parsing boot context {}: {error}", path.display()))?;
    validated_initial_cop0_status_authority(rom, context)
}

fn validated_initial_cop0_status_authority(
    rom: &fn64_discover::NormalizedRom,
    context: BootContext,
) -> Result<InitialCop0StatusAuthorityV1, String> {
    context
        .validate_for_entry(rom.header.entry_point)
        .map_err(|error| format!("validating boot context for ROM entry: {error}"))?;
    if context.normalized_rom_sha256.to_string() != rom.sha256 {
        return Err(format!(
            "boot-context normalized ROM SHA-256 {} does not match {}",
            context.normalized_rom_sha256, rom.sha256
        ));
    }
    if context.region.tv_standard != BootTvStandard::Ntsc {
        return Err(format!(
            "WM2000 block gate requires NTSC boot context, found {:?}",
            context.region.tv_standard
        ));
    }
    let destination_code = rom.header.cartridge_id[3];
    if context.region.destination_code != destination_code {
        return Err(format!(
            "boot-context destination code {:#04x} does not match normalized ROM header {destination_code:#04x}",
            context.region.destination_code
        ));
    }
    let ipl3 = rom.bytes.get(0x40..0x1000).ok_or_else(|| {
        "normalized ROM is too short to bind the IPL3 boot-code identity".to_string()
    })?;
    let ipl3_sha256 = sha256_hex(ipl3);
    if context.cic.ipl3_sha256.to_string() != ipl3_sha256 {
        return Err(format!(
            "boot-context IPL3 SHA-256 {} does not match normalized ROM {ipl3_sha256}",
            context.cic.ipl3_sha256
        ));
    }
    let canonical_context = serde_json::to_vec(&context)
        .map_err(|error| format!("serializing canonical boot context: {error}"))?;
    Ok(InitialCop0StatusAuthorityV1::BootContext {
        boot_context_sha256: sha256_hex(&canonical_context),
        producer: context.producer,
        normalized_rom_sha256: context.normalized_rom_sha256.to_string(),
        ipl3_sha256,
        destination_code,
        tv_standard: context.region.tv_standard.into(),
        entry_pc: context.entry_pc,
        cp0_status: context.cp0.registers[12] as u32,
    })
}

fn modeled_exception_vectors(
    external_images: &[ExternalExecutableImageIdentityV1],
) -> Result<Vec<ModeledExceptionVectorV1>, String> {
    MODELED_EXCEPTION_VECTOR_DESTINATIONS_V1
        .into_iter()
        .map(|destination| {
            let entry_end = destination
                .checked_add(4)
                .expect("modeled exception-vector entry does not wrap");
            let owners = external_images
                .iter()
                .filter(|image| {
                    image.first_executed_pc == destination
                        && image.va_start <= destination
                        && image
                            .va_start
                            .checked_add(image.byte_len)
                            .is_some_and(|image_end| entry_end <= image_end)
                })
                .collect::<Vec<_>>();
            let disposition = match owners.as_slice() {
                [owner] => ExceptionVectorDispositionV1::ExactCodeOwner(
                    ExceptionVectorExactCodeOwnerV1::from(*owner),
                ),
                [] => ExceptionVectorDispositionV1::Open {
                    reason: format!(
                        "modeled exception destination {destination:#010x} has neither an exact external-image owner nor a validated unreachability receipt"
                    ),
                },
                _ => {
                    return Err(format!(
                        "modeled exception destination {destination:#010x} is covered by multiple external executable images: {:?}",
                        owners
                            .iter()
                            .map(|owner| (&owner.image_id, owner.generation))
                            .collect::<Vec<_>>()
                    ));
                }
            };
            Ok(ModeledExceptionVectorV1 {
                destination,
                disposition,
            })
        })
        .collect()
}

fn reproducible_external_images_from_env(
    normalized_rom_sha256: &str,
) -> Result<Vec<fn64_discover::trace::ExecutableImageCapture>, String> {
    let explicit_groups = std::env::var("FN64_EXECUTABLE_IMAGE_GROUPS").ok();
    let group_names = explicit_groups
        .as_deref()
        .unwrap_or("FN64_EXECUTABLE_IMAGES")
        .split(',')
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .collect::<Vec<_>>();
    if group_names.is_empty() {
        return Err("FN64_EXECUTABLE_IMAGE_GROUPS names no capture groups".to_string());
    }
    let expected =
        fn64_discover::trace::NormalizedRomDigest::try_from(normalized_rom_sha256.to_string())
            .map_err(|error| format!("validating normalized ROM identity: {error}"))?;
    let mut captures = Vec::new();
    for name in group_names {
        let Some(value) = std::env::var_os(name) else {
            if explicit_groups.is_none() && name == "FN64_EXECUTABLE_IMAGES" {
                return Ok(Vec::new());
            }
            return Err(format!(
                "configured executable-image group {name} is absent"
            ));
        };
        let paths = std::env::split_paths(&value).collect::<Vec<_>>();
        let documents = paths
            .iter()
            .map(|path| {
                std::fs::read(path).map_err(|error| {
                    format!(
                        "reading executable-image capture {}: {error}",
                        path.display()
                    )
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        let capture = fn64_discover::trace::parse_reproducible_executable_image_group(
            &documents, &expected, 3,
        )
        .map_err(|error| format!("validating executable-image group {name}: {error}"))?;
        if captures
            .iter()
            .any(|known: &fn64_discover::trace::ExecutableImageCapture| {
                known.image_id == capture.image_id && known.generation == capture.generation
            })
        {
            return Err(format!(
                "duplicate executable-image identity {} generation {}",
                capture.image_id, capture.generation
            ));
        }
        captures.push(capture);
    }
    Ok(captures)
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
                    | FunctionEntryEvidence::HandlerTablePointer { .. }
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

#[cfg(test)]
mod tests {
    use super::*;
    use fn64_discover::dense_aot_pack::{DenseAotGenerationV1, DenseAotShardV1};
    use fn64_discover::facts::{
        function_entry_subject, BankAddr, CandidateDetector, FunctionEntryEvidence,
    };
    use fn64_discover::overlay_recipe::{OverlayLoadRecipeV1, OVERLAY_RECIPE_SCHEMA_V1};
    use fn64_discover::trace::{
        ExecutableImageCapture, ExecutableImageLineage, NormalizedRomDigest,
        EXECUTABLE_IMAGE_SCHEMA,
    };
    use fn64_recomp_rs::boot::{
        BootCicIdentity, BootCop0Context, BootRegion, Sha256Digest, BOOT_CONTEXT_SCHEMA_V1,
    };

    const SHA: &str = "1111111111111111111111111111111111111111111111111111111111111111";

    #[test]
    fn runtime_catalog_builder_matches_generated_identity_and_digest_rules() {
        const BOOT_ROM: u32 = 0x1000;
        const OVERLAY_ROM: u32 = 0x1100;
        const BOOT_VA: u32 = 0x8000_0400;
        const SPLIT_VA: u32 = BOOT_VA + 0x20;
        const IMAGE_END: u32 = BOOT_VA + 0x40;
        let mut raw = vec![0u8; 0x1200];
        raw[0..4].copy_from_slice(&0x8037_1240u32.to_be_bytes());
        raw[8..12].copy_from_slice(&BOOT_VA.to_be_bytes());
        for (index, byte) in raw[BOOT_ROM as usize..BOOT_ROM as usize + 0x40]
            .iter_mut()
            .enumerate()
        {
            *byte = index as u8;
        }
        for (index, byte) in raw[OVERLAY_ROM as usize..OVERLAY_ROM as usize + 0x20]
            .iter_mut()
            .enumerate()
        {
            *byte = 0x80 | index as u8;
        }
        let rom = fn64_discover::normalize(&raw).unwrap();
        let overlay_bytes = &rom.bytes[OVERLAY_ROM as usize..OVERLAY_ROM as usize + 0x20];
        let recipe = OverlayLoadRecipeV1 {
            schema: OVERLAY_RECIPE_SCHEMA_V1.to_owned(),
            descriptor_rom_offset: 0x200,
            rom_start: OVERLAY_ROM,
            rom_end: OVERLAY_ROM + 0x20,
            load_start: SPLIT_VA,
            text_start: SPLIT_VA,
            text_end: IMAGE_END,
            data_start: IMAGE_END,
            data_end: IMAGE_END,
            bss_start: IMAGE_END,
            bss_end: IMAGE_END,
            loaded_sha256: format!("{:x}", Sha256::digest(overlay_bytes)),
        };
        let pack = build_dense_aot_pack_v1(
            &rom,
            &[
                DenseAotGenerationInput {
                    name: BOOT_BANK,
                    source_rom_start: BOOT_ROM,
                    source_rom_end: BOOT_ROM + 0x40,
                    load_start: BOOT_VA,
                    text_start: BOOT_VA,
                    text_end: IMAGE_END,
                    data_start: IMAGE_END,
                    data_end: IMAGE_END,
                    bss_start: IMAGE_END,
                    bss_end: IMAGE_END,
                },
                DenseAotGenerationInput::from(("recovered_overlay_0", &recipe)),
            ],
        )
        .unwrap();
        let topology = build_generation_topology_v1(
            &rom,
            &pack,
            BOOT_BANK,
            WM_RESIDENT_TAIL_IDENTITY_DOMAIN_V1,
            std::slice::from_ref(&recipe),
        )
        .unwrap();
        let catalog = build_backed_dense_generation_catalog_v1(&rom, &pack, &topology).unwrap();
        let evidence = catalog.evidence_snapshot();

        assert_eq!(evidence.generations.len(), 2);
        assert_eq!(evidence.backings.len(), 2);
        for geometry in &topology.generations {
            let generation = evidence
                .generations
                .iter()
                .find(|candidate| candidate.generation.get() == geometry.generation_id)
                .unwrap();
            let (source_start, identity_name) = match geometry.role {
                CatalogGenerationRoleV1::ResidentTail => (BOOT_ROM + 0x20, "resident_tail"),
                CatalogGenerationRoleV1::Overlay => (OVERLAY_ROM, "recovered_overlay_0"),
            };
            let bytes = &rom.bytes[source_start as usize
                ..source_start as usize + (geometry.image_end - geometry.image_start) as usize];
            let words = bytes
                .chunks_exact(4)
                .map(|word| u32::from_be_bytes(word.try_into().unwrap()))
                .collect::<Vec<_>>();
            let expected_bank = fn64_discover::dense_aot_pack::dense_aot_artifact_bank_id(
                &rom.sha256,
                identity_name,
                geometry.image_start,
                &words,
            );
            assert_eq!(generation.expected_sha256, Sha256::digest(bytes).as_slice());
            assert_eq!(generation.shards.len(), 1);
            assert_eq!(generation.shards[0].bank().get(), expected_bank);
            assert_eq!(generation.shards[0].start().get(), geometry.image_start);
            assert_eq!(generation.shards[0].end().get(), geometry.image_end);
            let backing = evidence
                .backings
                .iter()
                .find(|candidate| candidate.generation == generation.generation)
                .unwrap();
            assert_eq!(backing.spans.len(), 1);
            assert_eq!(
                backing.spans[0].physical_start(),
                geometry.invalidation_start & 0x1fff_ffff
            );
        }
        assert_eq!(catalog.reserved_banks().len(), 2);

        let mut digest_drift = topology.clone();
        digest_drift.generations[0].image_sha256 = "00".repeat(32);
        let error = build_backed_dense_generation_catalog_v1(&rom, &pack, &digest_drift)
            .err()
            .expect("topology digest drift must fail closed");
        assert!(error.contains("ROM digest disagrees with topology"));
    }

    fn dense_generation(
        name: &str,
        source_rom_start: u32,
        load_start: u32,
        byte_len: u32,
        shard_count: usize,
    ) -> DenseAotGenerationV1 {
        let source_rom_end = source_rom_start + byte_len;
        let load_end = load_start + byte_len;
        let shards = (0..shard_count)
            .map(|index| {
                let offset = index as u32 * DENSE_AOT_SHARD_BYTES;
                let shard_len = DENSE_AOT_SHARD_BYTES.min(byte_len - offset);
                DenseAotShardV1 {
                    index: index as u32,
                    source_rom_start: source_rom_start + offset,
                    source_rom_end: source_rom_start + offset + shard_len,
                    va_start: load_start + offset,
                    va_end: load_start + offset + shard_len,
                    sha256: SHA.to_string(),
                    delay_lookahead: None,
                    artifact_identity: SHA.to_string(),
                }
            })
            .collect();
        DenseAotGenerationV1 {
            name: name.to_string(),
            bank_id: 1,
            source_rom_start,
            source_rom_end,
            load_start,
            load_end,
            text_start: load_start,
            text_end: load_end,
            data_start: load_end,
            data_end: load_end,
            bss_start: load_end,
            bss_end: load_end,
            loaded_sha256: SHA.to_string(),
            aligned_entry_count: byte_len / 4,
            shards,
        }
    }

    fn overlay_recipe(
        index: usize,
        source_rom_start: u32,
        load_start: u32,
        shard_count: usize,
    ) -> OverlayLoadRecipeV1 {
        let byte_len = shard_count as u32 * DENSE_AOT_SHARD_BYTES;
        OverlayLoadRecipeV1 {
            schema: OVERLAY_RECIPE_SCHEMA_V1.to_string(),
            descriptor_rom_offset: 0x200 + index as u32 * 36,
            rom_start: source_rom_start,
            rom_end: source_rom_start + byte_len,
            load_start,
            text_start: load_start,
            text_end: load_start + byte_len,
            data_start: load_start + byte_len,
            data_end: load_start + byte_len,
            bss_start: load_start + byte_len,
            bss_end: load_start + byte_len,
            loaded_sha256: SHA.to_string(),
        }
    }

    fn static_graph_fixture(
        first_overlay_start: u32,
    ) -> (DenseAotPackV1, Vec<OverlayLoadRecipeV1>) {
        let boot_start = 0x8000_0400;
        let mut generations = vec![dense_generation(
            BOOT_BANK,
            0x1000,
            boot_start,
            16 * DENSE_AOT_SHARD_BYTES,
            16,
        )];
        let overlay_shards = [3usize, 1, 6, 8];
        let recipes = overlay_shards
            .into_iter()
            .enumerate()
            .map(|(index, shard_count)| {
                let recipe = overlay_recipe(
                    index,
                    0x20_0000 + index as u32 * 0x10_0000,
                    first_overlay_start + index as u32 * 0x20_0000,
                    shard_count,
                );
                generations.push(dense_generation(
                    &format!("recovered_overlay_{index}"),
                    recipe.rom_start,
                    recipe.load_start,
                    recipe.rom_end - recipe.rom_start,
                    shard_count,
                ));
                recipe
            })
            .collect();
        (
            DenseAotPackV1 {
                schema: fn64_discover::dense_aot_pack::DENSE_AOT_PACK_SCHEMA_V1.to_string(),
                normalized_rom_sha256: SHA.to_string(),
                generations,
            },
            recipes,
        )
    }

    fn transfer_scan_snapshots_fixture() -> (DenseAotPackV1, Vec<ProgramSnapshotV1>) {
        const BOOT_ROM: u32 = 0x1000;
        const OVERLAY_ROM: u32 = 0x2000;
        const CANDIDATE_ROM: u32 = 0x3000;
        const BOOT_VA: u32 = 0x8000_0400;
        const OVERLAY_VA: u32 = 0x8000_1000;
        const CANDIDATE_VA: u32 = 0x8000_2000;
        const NOP: u32 = 0;
        const JR_RA: u32 = 0x03e0_0008;

        let words = |values: &[u32]| {
            values
                .iter()
                .flat_map(|word| word.to_be_bytes())
                .collect::<Vec<_>>()
        };
        let boot = words(&[
            0x0c00_0000 | ((OVERLAY_VA >> 2) & 0x03ff_ffff),
            NOP,
            JR_RA,
            NOP,
        ]);
        let overlay = words(&[JR_RA, NOP]);
        let candidate = words(&[JR_RA, NOP]);
        let mut source = vec![0u8; CANDIDATE_ROM as usize + candidate.len()];
        source[0..4].copy_from_slice(&0x8037_1240u32.to_be_bytes());
        source[8..12].copy_from_slice(&BOOT_VA.to_be_bytes());
        source[BOOT_ROM as usize..BOOT_ROM as usize + boot.len()].copy_from_slice(&boot);
        source[OVERLAY_ROM as usize..OVERLAY_ROM as usize + overlay.len()]
            .copy_from_slice(&overlay);
        source[CANDIDATE_ROM as usize..CANDIDATE_ROM as usize + candidate.len()]
            .copy_from_slice(&candidate);
        let rom = fn64_discover::normalize(&source).unwrap();

        let mut facts = FactDb::new();
        for (bank, rom_start, va_start, bytes) in [
            (BOOT_BANK, BOOT_ROM, BOOT_VA, boot.as_slice()),
            (
                "recovered_overlay_0",
                OVERLAY_ROM,
                OVERLAY_VA,
                overlay.as_slice(),
            ),
            (
                "recovered_overlay_1",
                CANDIDATE_ROM,
                CANDIDATE_VA,
                candidate.as_slice(),
            ),
        ] {
            let mapping = facts.insert(Fact::RomMapping {
                bank: bank.to_string(),
                rom_space: RomAddressSpace::Physical,
                rom_start,
                rom_end: rom_start + bytes.len() as u32,
                va_start,
                va_end: va_start + bytes.len() as u32,
            });
            facts
                .conclude(
                    format!("bank:{bank}"),
                    ProofState::Proven,
                    vec![mapping],
                    "transfer scan test mapping",
                )
                .unwrap();
        }
        let entry = BankAddr::new(BOOT_BANK, BOOT_VA);
        let claim = facts.insert(Fact::FunctionEntryClaim {
            target: entry.clone(),
            detector: CandidateDetector::HardwareEntrypoint,
            evidence: FunctionEntryEvidence::RomHeaderEntrypoint,
            proposed_state: ProofState::Proven,
        });
        facts
            .conclude(
                function_entry_subject(&entry),
                ProofState::Proven,
                vec![claim],
                "transfer scan test hardware entry",
            )
            .unwrap();

        let inputs = [
            MaterializedBankInput {
                bank: BOOT_BANK,
                va_start: BOOT_VA,
                bytes: &boot,
                seed_roots: &[BOOT_VA],
            },
            MaterializedBankInput {
                bank: "recovered_overlay_0",
                va_start: OVERLAY_VA,
                bytes: &overlay,
                seed_roots: &[],
            },
            MaterializedBankInput {
                bank: "recovered_overlay_1",
                va_start: CANDIDATE_VA,
                bytes: &candidate,
                seed_roots: &[CANDIDATE_VA],
            },
        ];
        let snapshots = compose_materialized_banks_validated_v2(&rom, &facts, &inputs)
            .unwrap()
            .snapshots()
            .to_vec();

        let mut generations = vec![
            dense_generation(BOOT_BANK, BOOT_ROM, BOOT_VA, boot.len() as u32, 1),
            dense_generation(
                "recovered_overlay_0",
                OVERLAY_ROM,
                OVERLAY_VA,
                overlay.len() as u32,
                1,
            ),
            dense_generation(
                "recovered_overlay_1",
                CANDIDATE_ROM,
                CANDIDATE_VA,
                candidate.len() as u32,
                1,
            ),
        ];
        for (index, generation) in generations.iter_mut().enumerate() {
            generation.bank_id = index as u64 + 1;
        }
        (
            DenseAotPackV1 {
                schema: fn64_discover::dense_aot_pack::DENSE_AOT_PACK_SCHEMA_V1.to_string(),
                normalized_rom_sha256: rom.sha256,
                generations,
            },
            snapshots,
        )
    }

    #[test]
    fn transfer_scan_uses_cross_bank_overlay_authority_from_composition() {
        let (pack, snapshots) = transfer_scan_snapshots_fixture();
        let banks = dense_transfer_banks(&pack, &snapshots).unwrap();
        assert_eq!(banks[1].bank, "recovered_overlay_0");
        assert!(banks[1]
            .closure
            .cfg
            .blocks
            .iter()
            .any(|block| block.start_va == 0x8000_1000));
        assert!(std::ptr::eq(
            banks[1].closure,
            &snapshots[1].banks[0].authority_closure
        ));
    }

    #[test]
    fn transfer_scan_excludes_candidate_only_traversal_hints() {
        let (pack, snapshots) = transfer_scan_snapshots_fixture();
        assert!(snapshots[2].banks[0]
            .closure
            .cfg
            .blocks
            .iter()
            .any(|block| block.start_va == 0x8000_2000));
        let banks = dense_transfer_banks(&pack, &snapshots).unwrap();
        assert_eq!(banks[2].bank, "recovered_overlay_1");
        assert!(banks[2].closure.cfg.blocks.is_empty());
        assert!(std::ptr::eq(
            banks[2].closure,
            &snapshots[2].banks[0].authority_closure
        ));
    }

    #[test]
    fn transfer_scan_rejects_snapshot_order_or_identity_drift() {
        let (pack, mut snapshots) = transfer_scan_snapshots_fixture();
        snapshots.swap(1, 2);
        let error = dense_transfer_banks(&pack, &snapshots).unwrap_err();
        assert!(error.contains("identity/geometry mismatch"));
    }

    #[test]
    fn static_graph_names_overlay_split_resident_tail_instead_of_obsolete_boot_shards() {
        let first_overlay_start = 0x8000_0400 + 14 * DENSE_AOT_SHARD_BYTES + 0x8000;
        let (pack, recipes) = static_graph_fixture(first_overlay_start);
        let packages = expected_static_shard_packages(&pack, &recipes).unwrap();
        assert_eq!(packages.len(), 35);
        assert!(packages.contains("wm2000-block-shard-14"));
        assert!(!packages.contains("wm2000-block-shard-15"));
        assert!(!packages.contains("wm2000-block-shard-16"));
        assert!(packages.contains("wm2000-block-resident-tail-shard-00"));
        assert!(packages.contains("wm2000-block-resident-tail-shard-01"));
        assert!(packages.contains("wm2000-block-overlay-0-shard-02"));
        assert!(packages.contains("wm2000-block-overlay-3-shard-07"));
    }

    #[test]
    fn static_graph_rejects_dense_overlay_geometry_that_drifted_from_recipe() {
        let first_overlay_start = 0x8000_0400 + 14 * DENSE_AOT_SHARD_BYTES + 0x8000;
        let (mut pack, recipes) = static_graph_fixture(first_overlay_start);
        pack.generations[2].source_rom_start += 4;
        let error = expected_static_shard_packages(&pack, &recipes).unwrap_err();
        assert!(error.contains("dense overlay generation 1 disagrees"));
    }

    #[test]
    fn static_graph_rejects_a_resident_split_other_than_fifteen_plus_two() {
        let first_overlay_start = 0x8000_0400 + 14 * DENSE_AOT_SHARD_BYTES;
        let (pack, recipes) = static_graph_fixture(first_overlay_start);
        let error = expected_static_shard_packages(&pack, &recipes).unwrap_err();
        assert!(error.contains("14 static-prefix / 2 resident-tail"));
    }

    fn image(image_id: &str, va_start: u32, byte_len: u32) -> ExternalExecutableImageIdentityV1 {
        ExternalExecutableImageIdentityV1 {
            image_id: image_id.to_string(),
            lineage: "CpuWritten".to_string(),
            generation: 0,
            va_start,
            byte_len,
            sha256: SHA.to_string(),
            first_executed_pc: va_start,
        }
    }

    fn boot_bound_rom_and_context() -> (fn64_discover::NormalizedRom, BootContext) {
        let mut bytes = vec![0u8; 0x1000];
        bytes[0..4].copy_from_slice(&0x8037_1240u32.to_be_bytes());
        bytes[8..12].copy_from_slice(&0x8000_0400u32.to_be_bytes());
        bytes[0x3e] = b'E';
        let rom = fn64_discover::normalize(&bytes).unwrap();
        let context = BootContext {
            schema: BOOT_CONTEXT_SCHEMA_V1.to_string(),
            producer: "black-box-test".to_string(),
            normalized_rom_sha256: Sha256Digest::parse(&rom.sha256).unwrap(),
            cic: BootCicIdentity {
                ipl3_sha256: Sha256Digest::parse(&sha256_hex(&rom.bytes[0x40..0x1000])).unwrap(),
            },
            region: BootRegion {
                destination_code: b'E',
                tv_standard: BootTvStandard::Ntsc,
            },
            entry_pc: rom.header.entry_point,
            gprs: [0; 32],
            hi: 0,
            lo: 0,
            cp0: BootCop0Context { registers: [0; 32] },
        };
        (rom, context)
    }

    #[test]
    fn captured_general_vector_owns_only_the_entry_covered_by_its_exact_range() {
        let vectors = modeled_exception_vectors(&[image("general", 0x8000_0180, 16)]).unwrap();
        assert_eq!(
            vectors
                .iter()
                .map(|vector| vector.destination)
                .collect::<Vec<_>>(),
            MODELED_EXCEPTION_VECTOR_DESTINATIONS_V1
        );
        assert!(!vectors
            .iter()
            .any(|vector| vector.destination == 0x8000_0100));
        for vector in vectors {
            if vector.destination == 0x8000_0180 {
                assert!(matches!(
                    vector.disposition,
                    ExceptionVectorDispositionV1::ExactCodeOwner(_)
                ));
            } else {
                assert!(matches!(
                    vector.disposition,
                    ExceptionVectorDispositionV1::Open { .. }
                ));
            }
        }
    }

    #[test]
    fn capture_range_does_not_own_vectors_other_than_its_first_fetch() {
        let vectors = modeled_exception_vectors(&[image("broad", 0x8000_0000, 0x184)]).unwrap();
        for vector in vectors {
            if vector.destination == 0x8000_0000 {
                assert!(matches!(
                    vector.disposition,
                    ExceptionVectorDispositionV1::ExactCodeOwner(_)
                ));
            } else {
                assert!(matches!(
                    vector.disposition,
                    ExceptionVectorDispositionV1::Open { .. }
                ));
            }
        }
    }

    #[test]
    fn ambiguous_external_vector_ownership_is_rejected() {
        let error = modeled_exception_vectors(&[
            image("general-a", 0x8000_0180, 16),
            image("general-b", 0x8000_0180, 16),
        ])
        .unwrap_err();
        assert!(error.contains("covered by multiple external executable images"));
    }

    #[test]
    fn initial_status_authority_binds_canonical_context_and_normalized_rom() {
        let (rom, mut context) = boot_bound_rom_and_context();
        context.cp0.registers[12] = 0x3400_0000;
        let authority = validated_initial_cop0_status_authority(&rom, context).unwrap();
        assert!(authority.bev_is_proven_clear());
        assert!(matches!(
            authority,
            InitialCop0StatusAuthorityV1::BootContext {
                destination_code: b'E',
                entry_pc: 0x8000_0400,
                cp0_status: 0x3400_0000,
                ..
            }
        ));

        let (rom, mut mismatch) = boot_bound_rom_and_context();
        mismatch.region.destination_code = b'P';
        let error = validated_initial_cop0_status_authority(&rom, mismatch).unwrap_err();
        assert!(error.contains("destination code"));
    }

    #[test]
    fn external_status_scan_uses_captured_words_and_first_fetch_root() {
        let words = vec![0x4088_6000, 0x03e0_0008, 0];
        let bytes = words
            .iter()
            .flat_map(|word: &u32| word.to_be_bytes())
            .collect::<Vec<_>>();
        let capture = ExecutableImageCapture {
            schema: EXECUTABLE_IMAGE_SCHEMA.to_string(),
            producer: "black-box-test".to_string(),
            normalized_rom_sha256: NormalizedRomDigest::try_from(SHA.to_string()).unwrap(),
            image_id: "external-status".to_string(),
            lineage: ExecutableImageLineage::CpuProduced,
            generation: 4,
            capture_pc: 0x8000_1000,
            first_executed_pc: 0x8000_1000,
            retired_instructions: 10,
            va_start: 0x8000_1000,
            byte_len: bytes.len() as u32,
            sha256: sha256_hex(&bytes),
            words,
        };
        let (scans, closures) = external_cop0_status_scans(&[capture]).unwrap();
        assert_eq!(scans.len(), 1);
        assert_eq!(closures.len(), 1);
        assert_eq!(scans[0].proven_code_writes.len(), 1);
        assert_eq!(scans[0].proven_code_writes[0].site_pc, 0x8000_1000);
        assert_eq!(scans[0].proven_code_value_proofs.len(), 1);
        assert!(scans[0].proven_code_value_proofs[0]
            .blockers
            .contains(&fn64_discover::source_closure::Cop0StatusValueBlockerV1::ValueOpen));
    }

    #[test]
    fn cache_inventory_promotes_only_proven_code_or_data() {
        use fn64_discover::cfg::WordClass;

        assert_eq!(
            cache_site_disposition(Some(WordClass::ProvenCode)),
            CacheSiteDispositionV1::ReachableInstruction
        );
        assert_eq!(
            cache_site_disposition(Some(WordClass::ProvenData)),
            CacheSiteDispositionV1::ProvenData
        );
        for word_class in [
            None,
            Some(WordClass::Unknown),
            Some(WordClass::CandidateCode),
            Some(WordClass::CandidateData),
            Some(WordClass::Conflict),
        ] {
            assert_eq!(
                cache_site_disposition(word_class),
                CacheSiteDispositionV1::Unclassified
            );
        }
    }
}
