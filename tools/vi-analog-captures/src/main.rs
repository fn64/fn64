use fn64_vi_analog_captures::{
    analyze_digital_boundary_file, analyze_pixel_comparison_file, generate_digital_vector_corpus,
    plan_capture_campaign, validate_hardware_consensus, validate_manifest_file, AnalogSignal,
    ConsoleRegion, MIN_CLOSURE_RUNS,
};
use std::path::PathBuf;

const HELP: &str = "usage:\n  fn64-vi-analog-captures generate-vectors --region ntsc OUTPUT_DIR\n  fn64-vi-analog-captures plan-campaign --campaign-id ID --vector VECTOR_ID --signal composite|s-video --runs N CORPUS_DIR\n  fn64-vi-analog-captures validate MANIFEST.json\n  fn64-vi-analog-captures consensus [--min-runs N] MANIFEST.json...\n  fn64-vi-analog-captures compare-pixels COMPARISON.json\n  fn64-vi-analog-captures analyze-digital-boundaries BUNDLE.json\n\ngenerate-vectors emits the deterministic public synthetic corpus; PAL/MPAL generation remains unsupported until an allowed register preset is documented. plan-campaign emits a non-evidence operator handoff and never fabricates capture or hardware identities. validate checks referenced files but never certifies one run. consensus defaults to ten controlled hardware runs. compare-pixels recomputes cohort admission and emits exact integer residual metrics without applying a tolerance or claiming parity. analyze-digital-boundaries requires the complete 44-point reset-isolated digital boundary matrix and always reports non-parity status.";

fn run() -> Result<(), String> {
    let mut arguments = std::env::args().skip(1).collect::<Vec<_>>();
    let command = arguments.first().map(String::as_str).unwrap_or("");
    match command {
        "generate-vectors" => {
            if arguments.len() != 4 || arguments[1] != "--region" {
                return Err(HELP.to_owned());
            }
            let region = match arguments[2].as_str() {
                "ntsc" => ConsoleRegion::Ntsc,
                "pal" => ConsoleRegion::Pal,
                "mpal" => ConsoleRegion::Mpal,
                value => {
                    return Err(format!(
                        "unsupported region {value:?}; expected ntsc, pal, or mpal"
                    ))
                }
            };
            let corpus =
                generate_digital_vector_corpus(region).map_err(|error| error.to_string())?;
            corpus
                .write_new(&PathBuf::from(&arguments[3]))
                .map_err(|error| error.to_string())?;
            serde_json::to_writer_pretty(std::io::stdout().lock(), &corpus.index)
                .map_err(|error| format!("write corpus receipt: {error}"))?;
            println!();
        }
        "plan-campaign" => {
            if arguments.len() != 10
                || arguments[1] != "--campaign-id"
                || arguments[3] != "--vector"
                || arguments[5] != "--signal"
                || arguments[7] != "--runs"
            {
                return Err(HELP.to_owned());
            }
            let signal = match arguments[6].as_str() {
                "composite" => AnalogSignal::Composite,
                "s-video" => AnalogSignal::SVideo,
                value => {
                    return Err(format!(
                        "unsupported signal {value:?}; expected composite or s-video"
                    ))
                }
            };
            let runs = arguments[8]
                .parse::<usize>()
                .map_err(|_| "--runs must be an integer".to_owned())?;
            let plan = plan_capture_campaign(
                &PathBuf::from(&arguments[9]),
                &arguments[2],
                &arguments[4],
                signal,
                runs,
            )
            .map_err(|error| error.to_string())?;
            serde_json::to_writer_pretty(std::io::stdout().lock(), &plan)
                .map_err(|error| format!("write campaign plan: {error}"))?;
            println!();
        }
        "validate" => {
            if arguments.len() != 2 {
                return Err(HELP.to_owned());
            }
            let capture = validate_manifest_file(&PathBuf::from(&arguments[1]))
                .map_err(|error| error.to_string())?;
            serde_json::to_writer_pretty(std::io::stdout().lock(), capture.receipt())
                .map_err(|error| format!("write validation receipt: {error}"))?;
            println!();
        }
        "consensus" => {
            arguments.remove(0);
            let mut minimum_runs = MIN_CLOSURE_RUNS;
            if arguments
                .first()
                .is_some_and(|argument| argument == "--min-runs")
            {
                if arguments.len() < 3 {
                    return Err(HELP.to_owned());
                }
                minimum_runs = arguments[1]
                    .parse::<usize>()
                    .map_err(|_| "--min-runs must be an integer".to_owned())?;
                arguments.drain(0..2);
            }
            if arguments.is_empty() {
                return Err(HELP.to_owned());
            }
            let captures = arguments
                .iter()
                .map(|path| {
                    validate_manifest_file(&PathBuf::from(path)).map_err(|error| error.to_string())
                })
                .collect::<Result<Vec<_>, _>>()?;
            let consensus = validate_hardware_consensus(&captures, minimum_runs)
                .map_err(|error| error.to_string())?;
            serde_json::to_writer_pretty(std::io::stdout().lock(), &consensus)
                .map_err(|error| format!("write consensus: {error}"))?;
            println!();
        }
        "compare-pixels" => {
            if arguments.len() != 2 {
                return Err(HELP.to_owned());
            }
            let report = analyze_pixel_comparison_file(&PathBuf::from(&arguments[1]))
                .map_err(|error| error.to_string())?;
            serde_json::to_writer_pretty(std::io::stdout().lock(), &report)
                .map_err(|error| format!("write pixel comparison report: {error}"))?;
            println!();
        }
        "analyze-digital-boundaries" => {
            if arguments.len() != 2 {
                return Err(HELP.to_owned());
            }
            let analysis = analyze_digital_boundary_file(&PathBuf::from(&arguments[1]))
                .map_err(|error| error.to_string())?;
            serde_json::to_writer_pretty(std::io::stdout().lock(), &analysis)
                .map_err(|error| format!("write digital boundary analysis: {error}"))?;
            println!();
        }
        _ => return Err(HELP.to_owned()),
    }
    Ok(())
}

fn main() {
    if let Err(error) = run() {
        eprintln!("vi-analog-captures: {error}");
        std::process::exit(1);
    }
}
