//! Driver for the black-box headless-emulator bridge.
//!
//! `crates/fn64-discover/src/headless.rs` has carried both halves of this
//! bridge -- `prepare_headless_run` and `normalize_headless_jsonl` -- with no
//! consumer. This binary is the consumer, and it exists to feed exactly one
//! thing: `trace::fold_pi_dmas_into_fact_db`, the only composition mechanism
//! in the repo that is not bound to an engine family.
//!
//! Two subcommands, deliberately separated by an out-of-process step:
//!
//! ```text
//!   headless-bridge plan      --rom ROM --trace-id ID --out bundle.json
//!       [ an emulator-specific wrapper reads bundle.json, runs the ROM,
//!         and writes observation JSONL -- see tools/mupen-trace/ ]
//!   headless-bridge normalize --rom ROM --trace-id ID --observations obs.jsonl
//!                             --out trace.jsonl
//! ```
//!
//! The wrapper never links fn64 and fn64 never links the emulator. The
//! emulator side is GPL (mupen64plus-core); keeping the boundary at a JSONL
//! file is what keeps that license off fn64's tree.
//!
//! `normalize` rebuilds the SAME plan `plan` emitted, because the bundle
//! SHA-256 that the observation stream is checked against is a hash of that
//! plan. Plan construction is therefore deterministic from (rom, trace_id)
//! alone -- if it were not, a wrapper's output could never be normalized.

use fn64_discover::headless::{
    normalize_headless_jsonl, prepare_headless_run, HeadlessArtifactDigest, HeadlessLaunchIdentity,
    HeadlessProducerIdentity, HeadlessRegion, HeadlessResetKind, PreparedHeadlessRun,
};
use fn64_discover::probe::{
    ExpectedInformationGain, InputTimelineDigest, Probe, ProbeBudget, ProbePlan, ProbeTarget,
    ScenarioIdentity, ValidatedProbePlan,
};
use fn64_discover::trace::{NormalizedRomDigest, PiDmaDirection};
use sha2::{Digest, Sha256};
use std::io::BufReader;
use std::path::PathBuf;

/// Generous but mandatory: the plan schema requires all three, and a producer
/// stops at the first limit and reports which. Boot plus early level loads is
/// where load-image DMAs concentrate, so this is sized for "well past boot"
/// rather than for a whole playthrough.
const MAX_INSTRUCTIONS: u64 = 4_000_000_000;
const MAX_EVENTS: u64 = 1_000_000;
const MAX_EMULATED_TIME_NS: u64 = 600_000_000_000;

fn usage() -> String {
    "usage: headless-bridge plan      --rom ROM --trace-id ID --emulator LIB --out bundle.json\n\
     \x20      headless-bridge normalize --rom ROM --trace-id ID --emulator LIB \
     --observations obs.jsonl --out trace.jsonl"
        .to_string()
}

fn main() {
    if let Err(error) = run() {
        eprintln!("headless-bridge: {error}");
        std::process::exit(1);
    }
}

struct Args {
    rom: PathBuf,
    trace_id: String,
    out: PathBuf,
    observations: Option<PathBuf>,
    /// The emulator binary the wrapper drives. Hashed into the run bundle, so
    /// an observation stream is bound to the exact build that produced it --
    /// and so `normalize` refuses a stream from a different one.
    emulator: PathBuf,
}

fn parse_args(mut args: impl Iterator<Item = String>) -> Result<Args, String> {
    let (mut rom, mut trace_id, mut out, mut observations, mut emulator) =
        (None, None, None, None, None);
    while let Some(argument) = args.next() {
        let mut take = |what: &str| args.next().ok_or_else(|| format!("{what} requires a value"));
        match argument.as_str() {
            "--rom" => rom = Some(PathBuf::from(take("--rom")?)),
            "--trace-id" => trace_id = Some(take("--trace-id")?),
            "--out" => out = Some(PathBuf::from(take("--out")?)),
            "--observations" => observations = Some(PathBuf::from(take("--observations")?)),
            "--emulator" => emulator = Some(PathBuf::from(take("--emulator")?)),
            other => return Err(format!("unknown argument {other:?}\n{}", usage())),
        }
    }
    Ok(Args {
        rom: rom.ok_or_else(usage)?,
        trace_id: trace_id.ok_or_else(usage)?,
        out: out.ok_or_else(usage)?,
        observations,
        emulator: emulator.ok_or_else(usage)?,
    })
}

/// Build the probe plan for a ROM. Deterministic from `(rom_sha256, trace_id)`:
/// `normalize` must reproduce byte-identical bytes to `plan`, or the bundle
/// digest will not match and every observation is rejected.
fn build_plan(rom_sha256: &str) -> Result<ValidatedProbePlan, String> {
    // No input timeline: this scenario is "reset and let the game boot itself".
    // The digest field is mandatory, so it is the digest of the empty timeline
    // -- an honest identity for "no inputs", not a placeholder.
    let empty_timeline = format!("{:x}", Sha256::digest([]));
    let plan = ProbePlan {
        normalized_rom_sha256: NormalizedRomDigest::try_from(rom_sha256.to_string())
            .map_err(str::to_string)?,
        scenario: ScenarioIdentity {
            scenario_id: "boot_pi_dma_survey".to_string(),
            input_timeline_id: "no_input".to_string(),
            input_timeline_sha256: InputTimelineDigest::try_from(empty_timeline)
                .map_err(str::to_string)?,
            start_emulated_time_ns: 0,
        },
        budget: ProbeBudget {
            max_instructions: MAX_INSTRUCTIONS,
            max_events: MAX_EVENTS,
            max_emulated_time_ns: MAX_EMULATED_TIME_NS,
        },
        // One catch-all probe. Every field is None ("any value in this
        // domain") except the direction: a cart->RDRAM transfer is a load
        // image, an RDRAM->cart transfer is a save, and only the first is
        // composition evidence.
        probes: vec![Probe {
            probe_id: "pi_dma_loads".to_string(),
            target: ProbeTarget::PiDma {
                direction: Some(PiDmaDirection::CartToRdram),
                cart_range: None,
                dram_range: None,
            },
            expected_information_gain: ExpectedInformationGain {
                priority: 100,
                unresolved_question: "which ROM ranges are loaded to which RDRAM addresses, for \
                                      a ROM whose table shape no static strategy recognizes"
                    .to_string(),
            },
        }],
    };
    plan.validate().map_err(|error| error.message)
}

fn file_digest(path: &PathBuf) -> Result<HeadlessArtifactDigest, String> {
    let bytes =
        std::fs::read(path).map_err(|error| format!("reading {}: {error}", path.display()))?;
    HeadlessArtifactDigest::try_from(format!("{:x}", Sha256::digest(&bytes)))
        .map_err(str::to_string)
}

fn producer(emulator: &PathBuf) -> Result<HeadlessProducerIdentity, String> {
    // No settings file is passed to the wrapper, so the settings identity is
    // the digest of nothing. That is a real identity for "defaults", not a
    // placeholder -- a wrapper that does apply settings must hash them here or
    // the bundle would claim a configuration it did not run under.
    let empty = HeadlessArtifactDigest::try_from(format!("{:x}", Sha256::digest([])))
        .map_err(str::to_string)?;
    Ok(HeadlessProducerIdentity {
        adapter_id: "fn64_headless_bridge".to_string(),
        adapter_version: "1".to_string(),
        emulator: "mupen64plus".to_string(),
        emulator_version: "darwin_arm64_dynarec".to_string(),
        executable_sha256: file_digest(emulator)?,
        settings_sha256: empty,
    })
}

fn launch() -> HeadlessLaunchIdentity {
    HeadlessLaunchIdentity {
        reset: HeadlessResetKind::PowerOn,
        region: HeadlessRegion::Ntsc,
        initial_state_sha256: None,
    }
}

fn rom_digest(path: &PathBuf) -> Result<String, String> {
    let bytes =
        std::fs::read(path).map_err(|error| format!("reading ROM {}: {error}", path.display()))?;
    let rom = fn64_discover::normalize(&bytes).map_err(|error| format!("{error:?}"))?;
    Ok(rom.sha256)
}

fn run() -> Result<(), String> {
    let mut argv = std::env::args().skip(1);
    let command = argv.next().ok_or_else(usage)?;
    let args = parse_args(argv)?;
    let sha256 = rom_digest(&args.rom)?;
    let plan = build_plan(&sha256)?;
    let producer = producer(&args.emulator)?;
    let launch = launch();
    let prepared: PreparedHeadlessRun<'_> =
        prepare_headless_run(&args.trace_id, &producer, &launch, &plan)
            .map_err(|error| error.message)?;

    match command.as_str() {
        "plan" => {
            let file = std::fs::File::create(&args.out)
                .map_err(|error| format!("creating {}: {error}", args.out.display()))?;
            prepared
                .write_json(file)
                .map_err(|error| error.message)?;
            eprintln!(
                "wrote run bundle {} (bundle_sha256={})",
                args.out.display(),
                prepared.bundle_sha256()
            );
            eprintln!("  rom normalized sha256 = {sha256}");
            eprintln!("  a wrapper must echo that bundle_sha256 in its header record");
            Ok(())
        }
        "normalize" => {
            let observations = args
                .observations
                .ok_or_else(|| "normalize requires --observations".to_string())?;
            let file = std::fs::File::open(&observations)
                .map_err(|error| format!("opening {}: {error}", observations.display()))?;
            let normalized = normalize_headless_jsonl(BufReader::new(file), &prepared)
                .map_err(|error| format!("line {}: {}", error.line, error.message))?;
            let out = std::fs::File::create(&args.out)
                .map_err(|error| format!("creating {}: {error}", args.out.display()))?;
            let count = normalized.records.len();
            normalized.write_jsonl(out).map_err(|error| error.message)?;
            eprintln!("wrote {count} canonical trace records to {}", args.out.display());
            eprintln!("  feed with: fn64-discover <rom> --trace {}", args.out.display());
            Ok(())
        }
        other => Err(format!("unknown subcommand {other:?}\n{}", usage())),
    }
}
