use fn64_discover::block_pack::{emit_block_program_source, BlockPackV1, BlockProgramSourceConfig};
use fn64_discover::evidence::EvidenceManifest;
use fn64_discover::trace::PiDmaFoldReport;
use fn64_discover::{
    run_discovery_auto, run_discovery_with_manifest, AutoDiscovery, DiscoveryStrategy, FactDb,
    NormalizedRom, ProofState, StrategyOutcome,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use std::ffi::OsString;
use std::io::{BufReader, Read, Write as IoWrite};
use std::path::{Path, PathBuf};

#[derive(Serialize)]
struct DiscoveryArtifact<'a> {
    schema_version: u32,
    rom: &'a NormalizedRom,
    facts: &'a FactDb,
    coverage: fn64_discover::coverage::CoverageReport,
    /// Which composition strategy was selected, and what every attempted
    /// strategy found. Absent when an evidence manifest supplied the
    /// composition, because then nothing was selected.
    #[serde(skip_serializing_if = "Option::is_none")]
    selected_strategy: Option<DiscoveryStrategy>,
    /// What each ingested trace's observed PI DMAs contributed. Absent when no
    /// trace was supplied.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    observed_load_images: Vec<PiDmaFoldReport>,
    /// Byte-level accounting: every ROM byte carries a typed claim.
    ledger: fn64_discover::ledger::RomLedger,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    strategy_outcomes: Vec<StrategyOutcome>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    traces: Vec<fn64_discover::trace::IngestReport>,
}

const LAYOUT_STUDY_MAX_ANSWER_KEY_BYTES: u64 = 8 * 1024 * 1024;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LayoutStudyDump {
    #[serde(rename = "section")]
    sections: Vec<LayoutStudySection>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LayoutStudySection {
    #[serde(rename = "name")]
    _name: String,
    rom: u32,
    vram: u32,
    #[serde(rename = "size")]
    _size: u32,
    #[serde(default)]
    functions: Vec<LayoutStudyFunction>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LayoutStudyFunction {
    #[serde(rename = "name")]
    _name: String,
    vram: u32,
    size: u32,
}

#[derive(Serialize)]
struct LayoutStudyMeasurementV1 {
    schema: &'static str,
    normalized_rom_sha256: String,
    cold_receipt_sha256: String,
    answer_key_sha256: String,
    banks_with_answer_functions: usize,
    banks_with_two_exact_owners: usize,
    answer_function_count: usize,
    exact_owner_count: usize,
    candidate_gap_count: usize,
    answer_positive_gap_count: usize,
    answer_empty_gap_count: usize,
    singleton_function_gap_count: usize,
    multi_function_gap_count: usize,
    answer_functions_in_positive_gaps: usize,
    candidate_gap_bytes: u64,
    answer_positive_gap_bytes: u64,
    answer_empty_gap_bytes: u64,
}

#[derive(Serialize)]
struct LayoutStudyReceiptV1 {
    measurement: LayoutStudyMeasurementV1,
    receipt_sha256: String,
}

/// Compact, content-free feedback receipt for one discovery run.
///
/// Unlike [`DiscoveryArtifact`], this deliberately omits the fact log, byte
/// ledger, trace identities, and all paths.  It is for deciding which
/// mechanical strategy to investigate next, not an interchangeable discovery
/// artifact or a source of admission authority.
#[derive(Serialize)]
struct DiscoverySummary<'a> {
    schema_version: u32,
    normalized_rom_sha256: &'a str,
    fact_count: usize,
    coverage: fn64_discover::coverage::CoverageReport,
    #[serde(skip_serializing_if = "Option::is_none")]
    selected_strategy: Option<DiscoveryStrategy>,
    strategy_outcomes: Vec<StrategyOutcome>,
    trace_count: usize,
    observed_load_image_reports: usize,
}

#[derive(Serialize)]
struct DiscoverySummaryReceipt<'a> {
    summary: DiscoverySummary<'a>,
    receipt_sha256: String,
}

#[derive(Serialize)]
struct BootTlbAliasCfgMeasurementV1 {
    transfer_pc: u32,
    target_va: u32,
    alias_va_start: u32,
    alias_va_end: u32,
    physical_start: u32,
    physical_end: u32,
    byte_count: u32,
    admissible: bool,
    target_reached: bool,
    block_count: usize,
    proven_code_words: usize,
    exhaustive_indirect_sites: usize,
    bounded_indirect_sites: usize,
    open_indirect_sites: usize,
    invalid_or_incomplete_blocks: usize,
}

#[derive(Serialize)]
struct BootTlbAliasMeasurementV1 {
    schema: &'static str,
    normalized_rom_sha256: String,
    selected_strategy: DiscoveryStrategy,
    proven_rom_mapping_count: usize,
    boot_va_start: u32,
    boot_va_end: u32,
    boot_physical_start: u32,
    diagnostic: fn64_discover::boot_tlb_alias::BootTlbAliasDiagnosticV1,
    alias_cfg: Vec<BootTlbAliasCfgMeasurementV1>,
}

#[derive(Serialize)]
struct BootTlbAliasReceiptV1 {
    measurement: BootTlbAliasMeasurementV1,
    receipt_sha256: String,
}

/// Report every strategy attempt on stderr, so a run that recovered nothing
/// says so out loud instead of returning a quiet boot-bank-only artifact that
/// looks the same as a successful one.
fn report_strategies(auto: &AutoDiscovery) {
    eprintln!("strategy: {} (selected)", auto.selected.label());
    for outcome in &auto.outcomes {
        eprintln!(
            "  {:<20} candidates={:<5} admitted={:<4} intervals={:<5} proven_mappings={}",
            outcome.strategy.label(),
            outcome.candidate_tables,
            outcome.admitted_tables,
            outcome.admitted_intervals,
            outcome.proven_mappings,
        );
    }
    if auto
        .facts
        .conclusion("bank:boot")
        .is_some_and(|conclusion| conclusion.state == ProofState::Open)
    {
        let detail = auto
            .facts
            .conclusion("bank:boot")
            .expect("checked above")
            .rule
            .as_str();
        eprintln!(
            "  NOTE: the boot bank remains Open ({detail}); no zero-delta fallback was used."
        );
    } else if auto.selected == DiscoveryStrategy::BootBankOnly {
        eprintln!(
            "  NOTE: no overlay geometry corroborated -- this ROM produced the IPL3 boot copy only."
        );
    }
}

fn main() {
    match run() {
        Ok(Some(receipt)) => println!("{receipt}"),
        Ok(None) => {}
        Err(error) => {
            eprintln!("fn64-discover: {error}");
            std::process::exit(1);
        }
    }
}

fn run() -> Result<Option<String>, String> {
    let args = std::env::args_os().skip(1).collect::<Vec<_>>();
    if args.first().and_then(|argument| argument.to_str()) == Some("study-layout") {
        return run_layout_study(args.into_iter().skip(1)).map(Some);
    }
    if args.first().and_then(|argument| argument.to_str()) == Some("__cold-rom-child") {
        return run_cold_rom_child(args.into_iter().skip(1)).map(Some);
    }
    if args.first().and_then(|argument| argument.to_str()) == Some("emit-block-program") {
        return emit_block_program_command(args.into_iter().skip(1)).map(Some);
    }
    if args.first().and_then(|argument| argument.to_str()) == Some("diagnose-boot-tlb-alias") {
        return diagnose_boot_tlb_alias(args.into_iter().skip(1)).map(Some);
    }
    run_discovery_command(args.into_iter())?;
    Ok(None)
}

fn diagnose_boot_tlb_alias(mut args: impl Iterator<Item = OsString>) -> Result<String, String> {
    let rom_path = PathBuf::from(
        args.next()
            .ok_or_else(|| "diagnose-boot-tlb-alias requires one ROM path".to_owned())?,
    );
    if args.next().is_some() {
        return Err("usage: fn64-discover diagnose-boot-tlb-alias <rom>".to_owned());
    }
    let rom_bytes = read_stable_regular_bounded(
        &rom_path,
        fn64_discover::cold_sweep::COLD_ROM_MAX_INPUT_BYTES as u64,
        "boot-TLB diagnostic ROM",
    )?;
    let auto = run_discovery_auto(&rom_bytes).map_err(|error| error.to_string())?;
    let prepared = fn64_discover::snapshot_inputs::prepare_snapshot_banks(&auto.rom, &auto.facts)
        .map_err(|error| format!("preparing boot bank: {error:?}"))?;
    let boot = prepared
        .banks()
        .iter()
        .find(|bank| bank.bank == fn64_discover::banks::BOOT_BANK)
        .ok_or_else(|| "discovery produced no proven boot bank".to_owned())?;
    let closure = fn64_discover::resolve::build_cfg_value_set_closed(
        &boot.bank,
        &boot.bytes,
        boot.va_start,
        &boot.traversal_seeds,
    );
    let tlb = fn64_discover::resolve::analyze_constant_tlb_transfers(
        &closure.cfg,
        &boot.bytes,
        boot.va_start,
    );
    let boot_physical_start = boot.va_start & 0x1fff_ffff;
    let boot_bytes = boot
        .va_end
        .checked_sub(boot.va_start)
        .ok_or_else(|| "boot bank VA interval is inverted".to_owned())?;
    let diagnostic = fn64_discover::boot_tlb_alias::derive_boot_tlb_alias_diagnostic(
        &tlb,
        boot_physical_start,
        boot_bytes,
    );
    let mut alias_cfg = Vec::new();
    for transfer in &diagnostic.transfers {
        let Some(alias) = &transfer.conditional_alias else {
            continue;
        };
        let byte_start = alias
            .physical_start
            .checked_sub(boot_physical_start)
            .ok_or_else(|| "alias begins before boot backing".to_owned())?
            as usize;
        let byte_end = alias
            .physical_end
            .checked_sub(boot_physical_start)
            .ok_or_else(|| "alias ends before boot backing".to_owned())?
            as usize;
        let alias_bytes = boot
            .bytes
            .get(byte_start..byte_end)
            .ok_or_else(|| "alias physical interval exceeds boot bytes".to_owned())?;
        let alias_closure = fn64_discover::resolve::build_cfg_value_set_closed(
            "boot_tlb_alias_diagnostic",
            alias_bytes,
            alias.alias_va_start,
            &[alias.target_va],
        );
        let mut exhaustive_indirect_sites = 0;
        let mut bounded_indirect_sites = 0;
        let mut open_indirect_sites = 0;
        for indirect in &alias_closure.indirect {
            match indirect.state {
                fn64_discover::resolve::IndirectProofState::Exhaustive => {
                    exhaustive_indirect_sites += 1
                }
                fn64_discover::resolve::IndirectProofState::Bounded => bounded_indirect_sites += 1,
                fn64_discover::resolve::IndirectProofState::Open => open_indirect_sites += 1,
            }
        }
        let invalid_or_incomplete_blocks = alias_closure
            .cfg
            .blocks
            .iter()
            .filter(|block| {
                matches!(
                    block.terminator,
                    fn64_discover::cfg::BlockTerminator::InvalidInstruction { .. }
                        | fn64_discover::cfg::BlockTerminator::MissingDelaySlot { .. }
                        | fn64_discover::cfg::BlockTerminator::RanOffEnd
                )
            })
            .count();
        alias_cfg.push(BootTlbAliasCfgMeasurementV1 {
            transfer_pc: alias.transfer_pc,
            target_va: alias.target_va,
            alias_va_start: alias.alias_va_start,
            alias_va_end: alias.alias_va_end,
            physical_start: alias.physical_start,
            physical_end: alias.physical_end,
            byte_count: alias.physical_end - alias.physical_start,
            admissible: transfer.blockers.is_empty(),
            target_reached: alias_closure
                .cfg
                .blocks
                .iter()
                .any(|block| block.start_va == alias.target_va),
            block_count: alias_closure.cfg.blocks.len(),
            proven_code_words: alias_closure
                .cfg
                .word_class
                .values()
                .filter(|class| **class == fn64_discover::cfg::WordClass::ProvenCode)
                .count(),
            exhaustive_indirect_sites,
            bounded_indirect_sites,
            open_indirect_sites,
            invalid_or_incomplete_blocks,
        });
    }
    let measurement = BootTlbAliasMeasurementV1 {
        schema: "fn64.boot-tlb-alias-diagnostic.v1",
        normalized_rom_sha256: auto.rom.sha256,
        selected_strategy: auto.selected,
        proven_rom_mapping_count: auto.facts.proven_rom_mappings().len(),
        boot_va_start: boot.va_start,
        boot_va_end: boot.va_end,
        boot_physical_start,
        diagnostic,
        alias_cfg,
    };
    let receipt_sha256 = format!(
        "{:x}",
        Sha256::digest(
            serde_json::to_vec(&measurement)
                .map_err(|error| format!("serializing boot-TLB measurement: {error}"))?
        )
    );
    serde_json::to_string(&BootTlbAliasReceiptV1 {
        measurement,
        receipt_sha256,
    })
    .map_err(|error| format!("serializing boot-TLB receipt: {error}"))
}

fn run_layout_study(mut args: impl Iterator<Item = OsString>) -> Result<String, String> {
    let rom_path = PathBuf::from(
        args.next()
            .ok_or_else(|| "study-layout requires ROM and dump paths".to_owned())?,
    );
    let dump_path = PathBuf::from(
        args.next()
            .ok_or_else(|| "study-layout requires an answer-key dump path".to_owned())?,
    );
    if args.next().is_some() {
        return Err("usage: fn64-discover study-layout <rom> <dump.toml>".to_owned());
    }
    let rom_bytes = read_stable_regular_bounded(
        &rom_path,
        fn64_discover::cold_sweep::COLD_ROM_MAX_INPUT_BYTES as u64,
        "layout-study ROM",
    )?;

    // Seal a complete ROM-only receipt before opening the answer key. The
    // subsequent rerun uses the same ROM-only API; the dump can only grade the
    // already-produced exact-owner geometry.
    let cold = fn64_discover::cold_sweep::measure_cold_rom(&rom_bytes)
        .map_err(|error| error.to_string())?;
    cold.receipt.verify()?;
    let limits = cold.receipt.measurement.limits;
    let auto = fn64_discover::run_discovery_auto_with_limits(
        &rom_bytes,
        fn64_discover::AutoDiscoveryLimits {
            vrom_materialization: fn64_discover::file_table::VromMaterializationLimits {
                max_decoded_file_bytes: limits.max_decoded_vrom_file_bytes as usize,
            },
        },
    )
    .map_err(|error| error.to_string())?;
    if auto.rom.sha256 != cold.receipt.measurement.normalized_rom_sha256
        || auto.selected != cold.receipt.measurement.selected_strategy
        || auto.outcomes != cold.receipt.measurement.strategy_outcomes
        || auto.facts.facts().len() != cold.receipt.measurement.fact_count
        || auto.facts.proven_bank_images().len() != cold.receipt.measurement.proven_bank_count
    {
        return Err("layout-study discovery rerun differs from its sealed cold receipt".to_owned());
    }
    let prepared = fn64_discover::snapshot_inputs::prepare_snapshot_banks_with_limits(
        &auto.rom,
        &auto.facts,
        fn64_discover::snapshot_inputs::PrepareSnapshotBanksLimits {
            max_banks: limits.max_banks as usize,
            max_aggregate_materialized_bytes: limits.max_aggregate_materialized_bytes,
            materialized_image: fn64_discover::materialized_image::MaterializedImageLimitsV1 {
                max_source_bytes: limits.max_decoded_vrom_file_bytes as usize,
                max_decoded_vrom_file_bytes: limits.max_decoded_vrom_file_bytes as usize,
                max_stream_output_bytes: limits.max_decoded_vrom_file_bytes as usize,
                max_aggregate_output_bytes: limits.max_decoded_vrom_file_bytes as usize,
                max_streams: 4096,
            },
        },
    )
    .map_err(|error| error.to_string())?;
    let inputs = prepared.materialized_inputs();
    let composed = fn64_discover::snapshot::compose_materialized_banks_validated_v2_with_limits(
        &auto.rom,
        &auto.facts,
        &inputs,
        fn64_discover::snapshot::MultiBankCompositionLimits {
            max_projected_fact_rows: limits.max_projected_fact_rows,
            max_projected_fact_bytes: limits.max_projected_fact_bytes,
            max_aggregate_materialized_bytes: limits.max_aggregate_materialized_bytes,
            max_cross_bank_authority_records: limits.max_cross_bank_authority_records,
        },
    )
    .map_err(|error| error.to_string())?;
    let sealed_scoreboard = match &cold.receipt.measurement.closure {
        fn64_discover::cold_sweep::ColdClosureMeasurementV2::Measured { scoreboard } => scoreboard,
        fn64_discover::cold_sweep::ColdClosureMeasurementV2::Open { .. } => {
            return Err("layout study requires a measured cold closure".to_owned());
        }
    };
    if &fn64_discover::closure::scoreboard(composed.snapshots()) != sealed_scoreboard {
        return Err(
            "layout-study composition rerun differs from its sealed cold receipt".to_owned(),
        );
    }

    let dump_bytes = read_stable_regular_bounded(
        &dump_path,
        LAYOUT_STUDY_MAX_ANSWER_KEY_BYTES,
        "layout-study answer key",
    )?;
    let answer_key_sha256 = format!("{:x}", Sha256::digest(&dump_bytes));
    let dump_text = std::str::from_utf8(&dump_bytes)
        .map_err(|_| "layout-study answer key is not UTF-8".to_owned())?;
    let dump: LayoutStudyDump =
        toml::from_str(dump_text).map_err(|error| format!("parsing layout-study key: {error}"))?;

    let mut measurement = LayoutStudyMeasurementV1 {
        schema: "fn64.linker-layout-study.v1",
        normalized_rom_sha256: auto.rom.sha256,
        cold_receipt_sha256: cold.receipt.receipt_sha256,
        answer_key_sha256,
        banks_with_answer_functions: 0,
        banks_with_two_exact_owners: 0,
        answer_function_count: 0,
        exact_owner_count: 0,
        candidate_gap_count: 0,
        answer_positive_gap_count: 0,
        answer_empty_gap_count: 0,
        singleton_function_gap_count: 0,
        multi_function_gap_count: 0,
        answer_functions_in_positive_gaps: 0,
        candidate_gap_bytes: 0,
        answer_positive_gap_bytes: 0,
        answer_empty_gap_bytes: 0,
    };

    for snapshot in composed.snapshots() {
        for bank in &snapshot.banks {
            let mut answer_functions = dump
                .sections
                .iter()
                .filter(|section| {
                    section.rom >= bank.input.rom_start
                        && section.rom < bank.input.rom_end
                        && section.vram
                            == bank
                                .input
                                .va_start
                                .saturating_add(section.rom - bank.input.rom_start)
                })
                .flat_map(|section| &section.functions)
                .filter(|function| {
                    function.vram >= bank.input.va_start
                        && function.vram < bank.input.va_end
                        && function
                            .vram
                            .saturating_add(function.size)
                            .min(bank.input.va_end)
                            > function.vram
                })
                .collect::<Vec<_>>();
            answer_functions.sort_by_key(|function| function.vram);
            answer_functions.dedup_by_key(|function| function.vram);
            if answer_functions.is_empty() {
                continue;
            }
            measurement.banks_with_answer_functions += 1;
            measurement.answer_function_count += answer_functions.len();

            let mut exact_owners = bank
                .owner_proof
                .assessments
                .iter()
                .filter_map(|assessment| match assessment {
                    fn64_discover::owner_proof::OwnerAssessment::Proven { owner } => Some(owner),
                    _ => None,
                })
                .collect::<Vec<_>>();
            exact_owners.sort_by_key(|owner| owner.entry.pc);
            exact_owners.dedup_by_key(|owner| owner.entry.pc);
            measurement.exact_owner_count += exact_owners.len();
            if exact_owners.len() < 2 {
                continue;
            }
            measurement.banks_with_two_exact_owners += 1;

            for pair in exact_owners.windows(2) {
                let gap_start = pair[0].va_end;
                let gap_end = pair[1].entry.pc;
                if gap_start >= gap_end {
                    continue;
                }
                let gap_bytes = u64::from(gap_end - gap_start);
                let functions_in_gap = answer_functions
                    .iter()
                    .filter(|function| function.vram >= gap_start && function.vram < gap_end)
                    .count();
                measurement.candidate_gap_count += 1;
                measurement.candidate_gap_bytes += gap_bytes;
                if functions_in_gap == 0 {
                    measurement.answer_empty_gap_count += 1;
                    measurement.answer_empty_gap_bytes += gap_bytes;
                } else {
                    measurement.answer_positive_gap_count += 1;
                    measurement.answer_positive_gap_bytes += gap_bytes;
                    measurement.answer_functions_in_positive_gaps += functions_in_gap;
                    if functions_in_gap == 1 {
                        measurement.singleton_function_gap_count += 1;
                    } else {
                        measurement.multi_function_gap_count += 1;
                    }
                }
            }
        }
    }

    let receipt_sha256 = format!(
        "{:x}",
        Sha256::digest(
            serde_json::to_vec(&measurement)
                .map_err(|error| format!("serializing layout study: {error}"))?
        )
    );
    serde_json::to_string(&LayoutStudyReceiptV1 {
        measurement,
        receipt_sha256,
    })
    .map_err(|error| format!("serializing layout-study receipt: {error}"))
}

fn run_cold_rom_child(mut args: impl Iterator<Item = OsString>) -> Result<String, String> {
    let rom_path = PathBuf::from(
        args.next()
            .ok_or_else(|| "internal cold-ROM child requires one ROM path".to_owned())?,
    );
    let expected_sha256 = args
        .next()
        .and_then(|argument| argument.into_string().ok())
        .ok_or_else(|| {
            "internal cold-ROM child requires the expected normalized SHA-256".to_owned()
        })?;
    if args.next().is_some() {
        return Err(
            "internal cold-ROM child accepts one ROM path and one expected digest".to_owned(),
        );
    }
    let rom_bytes = read_stable_regular_bounded(
        &rom_path,
        fn64_discover::cold_sweep::COLD_ROM_MAX_INPUT_BYTES as u64,
        "isolated ROM input",
    )?;
    let run = fn64_discover::cold_sweep::measure_cold_rom(&rom_bytes)
        .map_err(|error| error.to_string())?;
    run.receipt.verify()?;
    if run.receipt.measurement.normalized_rom_sha256 != expected_sha256 {
        return Err(format!(
            "normalized ROM digest mismatch: expected {expected_sha256}, got {}",
            run.receipt.measurement.normalized_rom_sha256
        ));
    }
    if let Some(diagnostic) = run.composition_diagnostic {
        eprintln!("cold ROM analysis remains open: {diagnostic}");
    }
    let encoded = serde_json::to_string(&run.receipt)
        .map_err(|error| format!("serializing cold ROM receipt: {error}"))?;
    if encoded.len() > 1024 * 1024 {
        return Err("cold ROM receipt exceeds its 1 MiB output bound".to_owned());
    }
    Ok(encoded)
}

fn read_stable_regular_bounded(path: &Path, limit: u64, label: &str) -> Result<Vec<u8>, String> {
    let before =
        std::fs::symlink_metadata(path).map_err(|error| format!("inspecting {label}: {error}"))?;
    if !before.file_type().is_file() {
        return Err(format!("{label} must be a regular file, not a symlink"));
    }
    if before.len() > limit {
        return Err(format!("{label} exceeds its {limit}-byte bound"));
    }
    let file = std::fs::File::open(path).map_err(|error| format!("opening {label}: {error}"))?;
    let opened = file
        .metadata()
        .map_err(|error| format!("inspecting opened {label}: {error}"))?;
    if !opened.is_file() || !same_file_snapshot(&before, &opened) {
        return Err(format!("{label} changed between inspection and open"));
    }
    let mut bytes = Vec::with_capacity(opened.len().min(limit) as usize);
    file.take(limit + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| format!("reading {label}: {error}"))?;
    if bytes.len() as u64 > limit {
        return Err(format!("{label} exceeds its {limit}-byte bound"));
    }
    let after =
        std::fs::symlink_metadata(path).map_err(|error| format!("rechecking {label}: {error}"))?;
    if !after.file_type().is_file()
        || !same_file_snapshot(&before, &after)
        || !same_file_snapshot(&opened, &after)
        || bytes.len() as u64 != opened.len()
    {
        return Err(format!("{label} changed while it was read"));
    }
    Ok(bytes)
}

#[cfg(unix)]
fn same_file_snapshot(left: &std::fs::Metadata, right: &std::fs::Metadata) -> bool {
    use std::os::unix::fs::MetadataExt;
    left.dev() == right.dev()
        && left.ino() == right.ino()
        && left.len() == right.len()
        && left.mtime() == right.mtime()
        && left.mtime_nsec() == right.mtime_nsec()
        && left.ctime() == right.ctime()
        && left.ctime_nsec() == right.ctime_nsec()
}

#[cfg(not(unix))]
fn same_file_snapshot(left: &std::fs::Metadata, right: &std::fs::Metadata) -> bool {
    left.len() == right.len()
        && left.modified().ok() == right.modified().ok()
        && left.created().ok() == right.created().ok()
}

fn run_discovery_command(mut args: impl Iterator<Item = OsString>) -> Result<(), String> {
    let rom_path = args.next().map(PathBuf::from).ok_or_else(usage)?;
    let mut evidence_path = None;
    let mut output_path = None;
    let mut summary_only = false;
    let mut trace_paths = Vec::new();
    while let Some(argument) = args.next() {
        match argument.to_str() {
            Some("--evidence") => {
                evidence_path =
                    Some(PathBuf::from(args.next().ok_or_else(|| {
                        "--evidence requires a TOML path".to_string()
                    })?));
            }
            Some("--out") => {
                output_path = Some(PathBuf::from(
                    args.next()
                        .ok_or_else(|| "--out requires a JSON path".to_string())?,
                ));
            }
            Some("--summary") => {
                if summary_only {
                    return Err("--summary may be supplied exactly once".to_string());
                }
                summary_only = true;
            }
            Some("--trace") => {
                trace_paths.push(PathBuf::from(
                    args.next()
                        .ok_or_else(|| "--trace requires a JSONL path".to_string())?,
                ));
            }
            Some(other) => return Err(format!("unknown argument {other:?}\n{}", usage())),
            None => return Err("arguments must be valid UTF-8".to_string()),
        }
    }
    if summary_only && output_path.is_some() {
        return Err("--summary and --out are mutually exclusive".to_string());
    }

    let rom_bytes = std::fs::read(&rom_path)
        .map_err(|error| format!("reading ROM {}: {error}", rom_path.display()))?;
    let (rom, facts, selected_strategy, strategy_outcomes) = if let Some(path) = evidence_path {
        let text = std::fs::read_to_string(&path)
            .map_err(|error| format!("reading evidence {}: {error}", path.display()))?;
        let manifest = EvidenceManifest::from_toml(&text).map_err(|error| error.to_string())?;
        let (rom, facts) = run_discovery_with_manifest(&rom_bytes, &manifest)
            .map_err(|error| error.to_string())?;
        (rom, facts, None, Vec::new())
    } else {
        let auto = run_discovery_auto(&rom_bytes).map_err(|error| error.to_string())?;
        report_strategies(&auto);
        let AutoDiscovery {
            rom,
            facts,
            selected,
            outcomes,
        } = auto;
        (rom, facts, Some(selected), outcomes)
    };
    trace_paths.sort();
    trace_paths.dedup();
    let expected_digest = fn64_discover::trace::NormalizedRomDigest::try_from(rom.sha256.clone())
        .map_err(str::to_string)?;
    let mut traces = Vec::new();
    let mut trace_ids = BTreeSet::new();
    for path in trace_paths {
        let file = std::fs::File::open(&path)
            .map_err(|error| format!("opening trace {}: {error}", path.display()))?;
        let report = fn64_discover::trace::ingest_jsonl(BufReader::new(file), &expected_digest)
            .map_err(|error| format!("ingesting trace {}: {error}", path.display()))?;
        if !trace_ids.insert(report.header.trace_id.clone()) {
            return Err(format!(
                "duplicate trace_id {:?} in trace inputs",
                report.header.trace_id
            ));
        }
        traces.push(report);
    }

    // Fold observed load images into the SAME database the static strategies
    // built. This is the point of ingesting a trace at all: a static strategy
    // has to recognise a structure and is bound to the engine families whose
    // structure it knows, while an observed transfer is bound to nothing. Until
    // now the CLI ingested traces and then dropped every observation on the
    // floor.
    let mut facts = facts;
    let mut observed_load_images = Vec::new();
    for report in &traces {
        let fold = fn64_discover::trace::fold_pi_dmas_into_fact_db(
            &mut facts,
            &report.header.trace_id,
            &report.facts,
        );
        eprintln!(
            "observed load images ({}): {} new, {} corroborating a proven mapping, {} conflicts, \
             {} chunks coalesced, {} transfers into {} reused destinations (buffers, not load \
             images), {} reloads, {} off-cartridge, {} write-backs, {} degenerate",
            report.header.trace_id,
            fold.new_mappings.len(),
            fold.corroborated.len(),
            fold.conflicts.len(),
            fold.coalesced_transfers,
            fold.reused_destination_skipped,
            fold.reused_destinations.len(),
            fold.repeated,
            fold.off_cartridge_skipped,
            fold.non_load_skipped,
            fold.degenerate_skipped,
        );
        for conflict in &fold.conflicts {
            eprintln!(
                "  CONFLICT seq={} VA 0x{:08x}: observed from ROM 0x{:08x}, proven bank {:?} \
                 backs it from ROM 0x{:08x}",
                conflict.sequence,
                conflict.va_start,
                conflict.observed_rom_start,
                conflict.proven_bank,
                conflict.proven_rom_start,
            );
        }
        observed_load_images.push(fold);
    }
    let coverage = fn64_discover::coverage::report(rom.len(), &facts);
    if summary_only {
        let summary = DiscoverySummary {
            // v1 is intentionally an observation-only, path-free fast-loop
            // receipt; it is not the full artifact schema.
            schema_version: 1,
            normalized_rom_sha256: &rom.sha256,
            fact_count: facts.facts().len(),
            coverage,
            selected_strategy,
            strategy_outcomes,
            trace_count: traces.len(),
            observed_load_image_reports: observed_load_images.len(),
        };
        let encoded = serde_json::to_vec(&summary)
            .map_err(|error| format!("serializing discovery summary: {error}"))?;
        let receipt = DiscoverySummaryReceipt {
            summary,
            receipt_sha256: format!("{:x}", Sha256::digest(encoded)),
        };
        println!(
            "{}",
            serde_json::to_string(&receipt)
                .map_err(|error| format!("serializing discovery summary receipt: {error}"))?
        );
        return Ok(());
    }

    // Byte accounting. Coverage as "% of ROM mapped" counts assets against the
    // score and cannot say how much CODE is undiscovered; this can.  Keep this
    // deliberately out of --summary: it owns a byte-granular working map and
    // a full ROM traversal, neither needed for strategy feedback.
    let ledger = fn64_discover::ledger::build_ledger(&rom.bytes, &facts);
    eprintln!("byte ledger ({} MiB):", ledger.total_bytes / 1_048_576);
    for (class, bytes) in &ledger.bytes_by_class {
        eprintln!(
            "  {:<16} {:>10} B  {:>5.1}%",
            class,
            bytes,
            100.0 * *bytes as f64 / ledger.total_bytes as f64
        );
    }
    eprintln!(
        "  UNDISCOVERED CODE: {} B ({:.2}% of ROM)",
        ledger.undiscovered_code_bytes(),
        100.0 * ledger.undiscovered_code_bytes() as f64 / ledger.total_bytes as f64
    );

    let artifact = DiscoveryArtifact {
        // v2 adds selected_strategy / strategy_outcomes.
        schema_version: 2,
        rom: &rom,
        facts: &facts,
        coverage,
        selected_strategy,
        ledger,
        observed_load_images,
        strategy_outcomes,
        traces,
    };
    let json = serde_json::to_string_pretty(&artifact)
        .map_err(|error| format!("serializing discovery artifact: {error}"))?;
    if let Some(path) = output_path {
        std::fs::write(&path, format!("{json}\n"))
            .map_err(|error| format!("writing {}: {error}", path.display()))?;
    } else {
        println!("{json}");
    }
    Ok(())
}

fn emit_block_program_command(mut args: impl Iterator<Item = OsString>) -> Result<String, String> {
    let rom_path = args
        .next()
        .map(PathBuf::from)
        .ok_or_else(emit_block_program_usage)?;
    let pack_path = args
        .next()
        .map(PathBuf::from)
        .ok_or_else(emit_block_program_usage)?;
    let mut entry_bank = None;
    let mut entry_pc = None;
    let mut instruction_budget = None;
    let mut output_path = None;
    while let Some(argument) = args.next() {
        match argument.to_str() {
            Some("--entry-bank") => {
                if entry_bank.is_some() {
                    return Err("--entry-bank may be supplied exactly once".to_owned());
                }
                let value = next_utf8(&mut args, "--entry-bank", "a canonical u64 hex value")?;
                entry_bank = Some(parse_fixed_upper_hex("--entry-bank", &value, 16)?);
            }
            Some("--entry-pc") => {
                if entry_pc.is_some() {
                    return Err("--entry-pc may be supplied exactly once".to_owned());
                }
                let value = next_utf8(&mut args, "--entry-pc", "a canonical u32 hex value")?;
                entry_pc = Some(
                    u32::try_from(parse_fixed_upper_hex("--entry-pc", &value, 8)?)
                        .expect("eight hexadecimal digits fit u32"),
                );
            }
            Some("--instruction-budget") => {
                if instruction_budget.is_some() {
                    return Err("--instruction-budget may be supplied exactly once".to_owned());
                }
                let value = next_utf8(&mut args, "--instruction-budget", "a decimal u32 value")?;
                if value.is_empty() || !value.bytes().all(|byte| byte.is_ascii_digit()) {
                    return Err(format!(
                        "--instruction-budget must be canonical unsigned decimal, got {value:?}"
                    ));
                }
                if value.len() > 1 && value.starts_with('0') {
                    return Err(format!(
                        "--instruction-budget must not contain leading zeros, got {value:?}"
                    ));
                }
                let value = value.parse::<u32>().map_err(|_| {
                    format!("--instruction-budget exceeds u32 or is malformed: {value:?}")
                })?;
                instruction_budget = Some(
                    fn64_recomp_rs::InstructionBudget::new(value).ok_or_else(|| {
                        format!(
                            "--instruction-budget must be at least {}, got {value}",
                            fn64_recomp_rs::InstructionBudget::MIN
                        )
                    })?,
                );
            }
            Some("--out") => {
                if output_path.is_some() {
                    return Err("--out may be supplied exactly once".to_owned());
                }
                output_path = Some(PathBuf::from(args.next().ok_or_else(|| {
                    "--out requires an explicit generated Rust source path".to_owned()
                })?));
            }
            Some(other) => {
                return Err(format!(
                    "unknown emit-block-program argument {other:?}\n{}",
                    emit_block_program_usage()
                ));
            }
            None => return Err("emit-block-program option names must be valid UTF-8".to_owned()),
        }
    }
    let entry_bank =
        entry_bank.ok_or_else(|| "emit-block-program requires --entry-bank".to_owned())?;
    let entry_pc = entry_pc.ok_or_else(|| "emit-block-program requires --entry-pc".to_owned())?;
    let instruction_budget = instruction_budget
        .ok_or_else(|| "emit-block-program requires --instruction-budget".to_owned())?;
    let output_path = output_path.ok_or_else(|| {
        "emit-block-program requires explicit --out because generated source contains ROM-derived instruction words"
            .to_owned()
    })?;

    let rom_bytes = std::fs::read(&rom_path)
        .map_err(|error| format!("reading ROM {}: {error}", rom_path.display()))?;
    let rom = fn64_discover::normalize(&rom_bytes)
        .map_err(|error| format!("normalizing ROM {}: {error}", rom_path.display()))?;
    let pack_bytes = std::fs::read(&pack_path)
        .map_err(|error| format!("reading block pack {}: {error}", pack_path.display()))?;
    let pack: BlockPackV1 = serde_json::from_slice(&pack_bytes)
        .map_err(|error| format!("parsing block pack {}: {error}", pack_path.display()))?;
    let source = emit_block_program_source(
        &pack,
        &rom,
        BlockProgramSourceConfig {
            entry: fn64_recomp_rs::ExecutionKey::new(
                fn64_recomp_rs::BankId::new(entry_bank),
                fn64_recomp_rs::GuestPc::new(entry_pc),
            ),
            instruction_budget,
        },
    )
    .map_err(|error| format!("emitting block program: {error}"))?;
    atomic_write(&output_path, source.as_bytes())?;
    let digest = lowercase_hex(Sha256::digest(source.as_bytes()).into());
    Ok(format!(
        "fn64-discover emit-block-program: sha256={digest} bytes={} out={}",
        source.len(),
        output_path.display()
    ))
}

fn next_utf8(
    args: &mut impl Iterator<Item = OsString>,
    option: &str,
    expected: &str,
) -> Result<String, String> {
    args.next()
        .ok_or_else(|| format!("{option} requires {expected}"))?
        .into_string()
        .map_err(|_| format!("{option} value must be valid UTF-8"))
}

fn parse_fixed_upper_hex(option: &str, value: &str, digits: usize) -> Result<u64, String> {
    let Some(hex) = value.strip_prefix("0x") else {
        return Err(format!(
            "{option} must use canonical 0x-prefixed uppercase hexadecimal"
        ));
    };
    if hex.len() != digits {
        return Err(format!(
            "{option} must contain exactly {digits} hexadecimal digits after 0x, got {}",
            hex.len()
        ));
    }
    if hex.bytes().any(|byte| matches!(byte, b'a'..=b'f')) {
        return Err(format!(
            "{option} must use uppercase hexadecimal digits, got {value:?}"
        ));
    }
    if !hex
        .bytes()
        .all(|byte| byte.is_ascii_digit() || matches!(byte, b'A'..=b'F'))
    {
        return Err(format!(
            "{option} contains a non-hexadecimal digit: {value:?}"
        ));
    }
    u64::from_str_radix(hex, 16)
        .map_err(|_| format!("{option} exceeds its declared hexadecimal width"))
}

fn atomic_write(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let file_name = path.file_name().ok_or_else(|| {
        format!(
            "--out must name a generated Rust source file, got {}",
            path.display()
        )
    })?;
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    for attempt in 0..128u32 {
        let temporary = parent.join(format!(
            ".{}.fn64-tmp-{}-{attempt}",
            file_name.to_string_lossy(),
            std::process::id()
        ));
        let mut file = match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)
        {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => {
                return Err(format!(
                    "creating temporary output beside {}: {error}",
                    path.display()
                ));
            }
        };
        let staged = file
            .write_all(bytes)
            .and_then(|()| file.sync_all())
            .map_err(|error| format!("staging generated source for {}: {error}", path.display()));
        drop(file);
        if let Err(error) = staged {
            let _ = std::fs::remove_file(&temporary);
            return Err(error);
        }
        if let Err(error) = std::fs::hard_link(&temporary, path) {
            let _ = std::fs::remove_file(&temporary);
            return Err(format!(
                "publishing generated source without clobber at {}: {error}",
                path.display()
            ));
        }
        std::fs::remove_file(&temporary).map_err(|error| {
            format!(
                "generated source was published at {}, but removing staging file {} failed: {error}",
                path.display(),
                temporary.display()
            )
        })?;
        return Ok(());
    }
    Err(format!(
        "could not reserve a temporary output name beside {} after 128 attempts",
        path.display()
    ))
}

fn lowercase_hex(bytes: [u8; 32]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(64);
    for byte in bytes {
        output.push(DIGITS[(byte >> 4) as usize] as char);
        output.push(DIGITS[(byte & 0x0f) as usize] as char);
    }
    output
}

fn usage() -> String {
    format!(
        "usage: fn64-discover <rom> [--evidence manifest.toml] [--trace events.jsonl]... [--summary | --out facts.json]\n       fn64-discover study-layout <rom> <dump.toml>\n       {}",
        emit_block_program_usage()
    )
}

fn emit_block_program_usage() -> String {
    "fn64-discover emit-block-program <rom> <block-pack.json> --entry-bank 0xNNNNNNNNNNNNNNNN --entry-pc 0xNNNNNNNN --instruction-budget N --out generated.rs".to_owned()
}
