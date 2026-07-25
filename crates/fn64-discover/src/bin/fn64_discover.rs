use fn64_discover::block_pack::{emit_block_program_source, BlockPackV1, BlockProgramSourceConfig};
use fn64_discover::evidence::EvidenceManifest;
use fn64_discover::trace::PiDmaFoldReport;
use fn64_discover::{
    run_discovery_auto, run_discovery_with_manifest, AutoDiscovery, DiscoveryStrategy, FactDb,
    NormalizedRom, StrategyOutcome,
};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use std::ffi::OsString;
use std::io::{BufReader, Write as IoWrite};
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
    #[serde(skip_serializing_if = "Vec::is_empty")]
    strategy_outcomes: Vec<StrategyOutcome>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    traces: Vec<fn64_discover::trace::IngestReport>,
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
    if auto.selected == DiscoveryStrategy::BootBankOnly {
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
    if args.first().and_then(|argument| argument.to_str()) == Some("emit-block-program") {
        return emit_block_program_command(args.into_iter().skip(1)).map(Some);
    }
    run_discovery_command(args.into_iter())?;
    Ok(None)
}

fn run_discovery_command(mut args: impl Iterator<Item = OsString>) -> Result<(), String> {
    let rom_path = args.next().map(PathBuf::from).ok_or_else(usage)?;
    let mut evidence_path = None;
    let mut output_path = None;
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

    let rom_bytes = std::fs::read(&rom_path)
        .map_err(|error| format!("reading ROM {}: {error}", rom_path.display()))?;
    let (rom, facts, selected_strategy, strategy_outcomes) = if let Some(path) = evidence_path {
        let text = std::fs::read_to_string(&path)
            .map_err(|error| format!("reading evidence {}: {error}", path.display()))?;
        let manifest = EvidenceManifest::from_toml(&text).map_err(|error| error.to_string())?;
        let (rom, facts) =
            run_discovery_with_manifest(&rom_bytes, &manifest).map_err(|error| error.to_string())?;
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
        let fold =
            fn64_discover::trace::fold_pi_dmas_into_fact_db(&mut facts, &report.header.trace_id, &report.facts);
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
    let artifact = DiscoveryArtifact {
        // v2 adds selected_strategy / strategy_outcomes.
        schema_version: 2,
        rom: &rom,
        facts: &facts,
        coverage: fn64_discover::coverage::report(rom.len(), &facts),
        selected_strategy,
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
        "usage: fn64-discover <rom> [--evidence manifest.toml] [--trace events.jsonl]... [--out facts.json]\n       {}",
        emit_block_program_usage()
    )
}

fn emit_block_program_usage() -> String {
    "fn64-discover emit-block-program <rom> <block-pack.json> --entry-bank 0xNNNNNNNNNNNNNNNN --entry-pc 0xNNNNNNNN --instruction-budget N --out generated.rs".to_owned()
}
