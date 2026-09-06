//! Single entry point for every fn64-discover gate/tool. Was 51 separate
//! `[[bin]]` targets, each linking its own copy of the workspace; now one
//! binary with a subcommand per former bin name (kebab-cased), each
//! forwarding its raw trailing arguments unchanged to that former bin's
//! `main` body (relocated to `commands::<name>::run`). This keeps every
//! flag name, usage string, and exit code byte-identical to the old bin —
//! see docs/plans/CLEANUP-2026-09.md Task 2.3.

mod commands;

use clap::{Parser, Subcommand};
use std::ffi::OsString;

/// A former `[[bin]]` target's error type, unified so `commands::*::run`
/// can share one signature. This is a plain enum with `Display`, per the
/// Task 2.3 ruling not to add thiserror in this task (Task 3.1 converts
/// crate-wide error types later).
#[derive(Debug)]
pub enum CommandError {
    Message(String),
}

impl std::fmt::Display for CommandError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CommandError::Message(message) => write!(f, "{message}"),
        }
    }
}

impl std::error::Error for CommandError {}

impl From<String> for CommandError {
    fn from(message: String) -> Self {
        CommandError::Message(message)
    }
}

impl From<&str> for CommandError {
    fn from(message: &str) -> Self {
        CommandError::Message(message.to_owned())
    }
}

/// Raw passthrough args for a subcommand whose former bin hand-parsed
/// `std::env::args()`/`args_os()` itself. Kept as a `Vec<OsString>` (not a
/// clap-derived struct per flag) because most of these bins accept
/// positional paths, `--flag value` pairs with bespoke validation, repeated
/// flags, or ROM-list varargs that a generic derive would have to
/// reimplement flag-by-flag at high risk of behavior drift for zero
/// benefit -- clap here only selects *which* command runs; each command
/// keeps its own exact, already-tested argument grammar.
#[derive(Parser, Debug)]
struct PassthroughArgs {
    #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
    args: Vec<OsString>,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// The original `fn64-discover` binary's own pipeline entry point
    /// (`<rom> [--evidence ...] [--trace ...] [--prove-owners] [--summary
    /// | --out ...]`, plus its `study-layout` / `emit-block-program`
    /// internal subcommands). Named `run` here since the crate's own name
    /// is now the outer binary's name.
    Run(PassthroughArgs),
    CandidateCfgProbe(PassthroughArgs),
    CompareComputedFlows(PassthroughArgs),
    DiagnoseOpenIndirects(PassthroughArgs),
    DiagnoseColdUnsupported(PassthroughArgs),
    ValidateCandidateReceipts(PassthroughArgs),
    GateAsmRoundtrip(PassthroughArgs),
    GateRomRecompile(PassthroughArgs),
    GateRomRebuild(PassthroughArgs),
    GateB1(PassthroughArgs),
    GateB2(PassthroughArgs),
    GateCallgraphMatch(PassthroughArgs),
    GateClosure(PassthroughArgs),
    GateContentConsumer(PassthroughArgs),
    CorpusIndex(PassthroughArgs),
    GateCorpusHomology(PassthroughArgs),
    GateCoverage(PassthroughArgs),
    GateD1(PassthroughArgs),
    GateD1OotOverlays(PassthroughArgs),
    GateD1Overlays(PassthroughArgs),
    GateDecompFunctions(PassthroughArgs),
    GateDecompReference(PassthroughArgs),
    GateDeltaVote(PassthroughArgs),
    GateGpBase(PassthroughArgs),
    GateHomology(PassthroughArgs),
    GateKeys(PassthroughArgs),
    GateLoaders(PassthroughArgs),
    GateOotReference(PassthroughArgs),
    GateOverlayGeneralize(PassthroughArgs),
    GateOverlayRegions(PassthroughArgs),
    GateOwnersOverlays(PassthroughArgs),
    GateRecompilerLint(PassthroughArgs),
    GateRecoverBoundaries(PassthroughArgs),
    GateRegions(PassthroughArgs),
    GateRelocAccuracy(PassthroughArgs),
    GateSelector(PassthroughArgs),
    GateStaticClosure(PassthroughArgs),
    GateTimingDiff(PassthroughArgs),
    GateToolJsonl(PassthroughArgs),
    GateTrace(PassthroughArgs),
    HeadlessBridge(PassthroughArgs),
    IngestToolClaims(PassthroughArgs),
    ProfileOverlayRegions(PassthroughArgs),
    ProduceSnapshotWorkspace(PassthroughArgs),
    RomIdentity(PassthroughArgs),
    RunWmWriterAudit(PassthroughArgs),
    StageSnapshotBank(PassthroughArgs),
    ValidateExecutableImageGroup(PassthroughArgs),
    ValidateTrainingWorkspace(PassthroughArgs),
    AttributeKnownFunctions(PassthroughArgs),
    ClassifyCallerless(PassthroughArgs),
}

#[derive(Parser, Debug)]
#[command(name = "fn64-discover", disable_help_subcommand = true)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

/// Every kebab-case subcommand word `Command` accepts, used only to decide
/// whether argv[1] is a subcommand selector or the start of the legacy
/// bare-invocation form (`fn64-discover <rom> ...`, handled by `Run`). Kept
/// as a literal list (not derived from clap at runtime) so this stays a
/// simple, auditable gate ahead of the real parser.
const SUBCOMMAND_NAMES: &[&str] = &[
    "run",
    "candidate-cfg-probe",
    "compare-computed-flows",
    "diagnose-open-indirects",
    "diagnose-cold-unsupported",
    "validate-candidate-receipts",
    "gate-asm-roundtrip",
    "gate-rom-recompile",
    "gate-rom-rebuild",
    "gate-b1",
    "gate-b2",
    "gate-callgraph-match",
    "gate-closure",
    "gate-content-consumer",
    "corpus-index",
    "gate-corpus-homology",
    "gate-coverage",
    "gate-d1",
    "gate-d1-oot-overlays",
    "gate-d1-overlays",
    "gate-decomp-functions",
    "gate-decomp-reference",
    "gate-delta-vote",
    "gate-gp-base",
    "gate-homology",
    "gate-keys",
    "gate-loaders",
    "gate-oot-reference",
    "gate-overlay-generalize",
    "gate-overlay-regions",
    "gate-owners-overlays",
    "gate-recompiler-lint",
    "gate-recover-boundaries",
    "gate-regions",
    "gate-reloc-accuracy",
    "gate-selector",
    "gate-static-closure",
    "gate-timing-diff",
    "gate-tool-jsonl",
    "gate-trace",
    "headless-bridge",
    "ingest-tool-claims",
    "profile-overlay-regions",
    "produce-snapshot-workspace",
    "rom-identity",
    "run-wm-writer-audit",
    "stage-snapshot-bank",
    "validate-executable-image-group",
    "validate-training-workspace",
    "attribute-known-functions",
    "classify-callerless",
];

fn main() {
    // Legacy compatibility: the original `fn64-discover` binary took no
    // subcommand word at all (`fn64-discover <rom> [--flags]`), and several
    // real callers (scripts/capture-boot-context.zsh via
    // run-black-box-trace.zsh, plus tests/emit_block_program_cli.rs's
    // `legacy_discovery_invocation_remains_available_without_a_subcommand`)
    // depend on that continuing to work unchanged. So: if argv[1] is absent,
    // or isn't one of this binary's known subcommand words, treat the whole
    // argv (minus argv[0]) as `Run`'s args instead of making clap reject it.
    let mut raw_args = std::env::args_os();
    let program = raw_args.next();
    let rest: Vec<OsString> = raw_args.collect();
    let first_is_known_subcommand = rest
        .first()
        .and_then(|arg| arg.to_str())
        .is_some_and(|first| SUBCOMMAND_NAMES.contains(&first));

    let result = if !first_is_known_subcommand {
        commands::fn64_discover_run::run(rest)
    } else {
        let mut full_args: Vec<OsString> = Vec::with_capacity(rest.len() + 1);
        full_args.extend(program);
        full_args.extend(rest);
        dispatch(Cli::parse_from(full_args).command)
    };
    if let Err(error) = result {
        eprintln!("{error}");
        std::process::exit(1);
    }
}

fn dispatch(command: Command) -> Result<(), CommandError> {
    match command {
        Command::Run(a) => commands::fn64_discover_run::run(a.args),
        Command::CandidateCfgProbe(a) => commands::candidate_cfg_probe::run(a.args),
        Command::CompareComputedFlows(a) => commands::compare_computed_flows::run(a.args),
        Command::DiagnoseOpenIndirects(a) => commands::diagnose_open_indirects::run(a.args),
        Command::DiagnoseColdUnsupported(a) => commands::diagnose_cold_unsupported::run(a.args),
        Command::ValidateCandidateReceipts(a) => commands::validate_candidate_receipts::run(a.args),
        Command::GateAsmRoundtrip(a) => commands::gate_asm_roundtrip::run(a.args),
        Command::GateRomRecompile(a) => commands::gate_rom_recompile::run(a.args),
        Command::GateRomRebuild(a) => commands::gate_rom_rebuild::run(a.args),
        Command::GateB1(a) => commands::gate_b1::run(a.args),
        Command::GateB2(a) => commands::gate_b2::run(a.args),
        Command::GateCallgraphMatch(a) => commands::gate_callgraph_match::run(a.args),
        Command::GateClosure(a) => commands::gate_closure::run(a.args),
        Command::GateContentConsumer(a) => commands::gate_content_consumer::run(a.args),
        Command::CorpusIndex(a) => commands::corpus_index::run(a.args),
        Command::GateCorpusHomology(a) => commands::gate_corpus_homology::run(a.args),
        Command::GateCoverage(a) => commands::gate_coverage::run(a.args),
        Command::GateD1(a) => commands::gate_d1::run(a.args),
        Command::GateD1OotOverlays(a) => commands::gate_d1_oot_overlays::run(a.args),
        Command::GateD1Overlays(a) => commands::gate_d1_overlays::run(a.args),
        Command::GateDecompFunctions(a) => commands::gate_decomp_functions::run(a.args),
        Command::GateDecompReference(a) => commands::gate_decomp_reference::run(a.args),
        Command::GateDeltaVote(a) => commands::gate_delta_vote::run(a.args),
        Command::GateGpBase(a) => commands::gate_gp_base::run(a.args),
        Command::GateHomology(a) => commands::gate_homology::run(a.args),
        Command::GateKeys(a) => commands::gate_keys::run(a.args),
        Command::GateLoaders(a) => commands::gate_loaders::run(a.args),
        Command::GateOotReference(a) => commands::gate_oot_reference::run(a.args),
        Command::GateOverlayGeneralize(a) => commands::gate_overlay_generalize::run(a.args),
        Command::GateOverlayRegions(a) => commands::gate_overlay_regions::run(a.args),
        Command::GateOwnersOverlays(a) => commands::gate_owners_overlays::run(a.args),
        Command::GateRecompilerLint(a) => commands::gate_recompiler_lint::run(a.args),
        Command::GateRecoverBoundaries(a) => commands::gate_recover_boundaries::run(a.args),
        Command::GateRegions(a) => commands::gate_regions::run(a.args),
        Command::GateRelocAccuracy(a) => commands::gate_reloc_accuracy::run(a.args),
        Command::GateSelector(a) => commands::gate_selector::run(a.args),
        Command::GateStaticClosure(a) => commands::gate_static_closure::run(a.args),
        Command::GateTimingDiff(a) => commands::gate_timing_diff::run(a.args),
        Command::GateToolJsonl(a) => commands::gate_tool_jsonl::run(a.args),
        Command::GateTrace(a) => commands::gate_trace::run(a.args),
        Command::HeadlessBridge(a) => commands::headless_bridge::run(a.args),
        Command::IngestToolClaims(a) => commands::ingest_tool_claims::run(a.args),
        Command::ProfileOverlayRegions(a) => commands::profile_overlay_regions::run(a.args),
        Command::ProduceSnapshotWorkspace(a) => commands::produce_snapshot_workspace::run(a.args),
        Command::RomIdentity(a) => commands::rom_identity::run(a.args),
        Command::RunWmWriterAudit(a) => commands::run_wm_writer_audit::run(a.args),
        Command::StageSnapshotBank(a) => commands::stage_snapshot_bank::run(a.args),
        Command::ValidateExecutableImageGroup(a) => {
            commands::validate_executable_image_group::run(a.args)
        }
        Command::ValidateTrainingWorkspace(a) => commands::validate_training_workspace::run(a.args),
        Command::AttributeKnownFunctions(a) => commands::attribute_known_functions::run(a.args),
        Command::ClassifyCallerless(a) => commands::classify_callerless::run(a.args),
    }
}
