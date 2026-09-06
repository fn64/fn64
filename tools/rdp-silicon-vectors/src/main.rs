use fn64_rdp_silicon_vectors::{
    analyze_alpha_coverage_product_sweep, analyze_alpha_dither_sweep,
    analyze_average_filter_output_tie_sweep, analyze_blender_precision_sweep,
    analyze_coverage_to_alpha_sweep, analyze_narrow_edge_coverage_correction_sweep,
    analyze_rdp_completion_timing_series, analyze_reciprocal_s10_5_boundary_sweep,
    analyze_representative_sample_selector_sweep, analyze_rgb_dither_sweep,
    analyze_texture_filter_tie_sweep, analyze_texture_lod_boundary_sweep,
    analyze_zmode_inter_coverage_sweep, validate_hardware_consensus, validate_json,
};
use std::env;
use std::fs;
use std::process::ExitCode;

const HELP: &str = "Usage:\n\
  fn64-rdp-silicon-vectors validate BUNDLE.json...\n\
  fn64-rdp-silicon-vectors analyze-rgb-dither SWEEP_ID BUNDLE.json\n\
  fn64-rdp-silicon-vectors analyze-alpha-dither SWEEP_ID BUNDLE.json\n\
  fn64-rdp-silicon-vectors analyze-alpha-coverage SWEEP_ID BUNDLE.json\n\
  fn64-rdp-silicon-vectors analyze-coverage-alpha SWEEP_ID BUNDLE.json\n\
  fn64-rdp-silicon-vectors analyze-zmode-inter SWEEP_ID BUNDLE.json\n\
  fn64-rdp-silicon-vectors analyze-representative-sample SWEEP_ID BUNDLE.json\n\
  fn64-rdp-silicon-vectors analyze-narrow-edge-coverage SWEEP_ID BUNDLE.json\n\
  fn64-rdp-silicon-vectors analyze-texture-filter-tie SWEEP_ID BUNDLE.json\n\
  fn64-rdp-silicon-vectors analyze-reciprocal-s10-5 SWEEP_ID BUNDLE.json\n\
  fn64-rdp-silicon-vectors analyze-average-filter-tie SWEEP_ID BUNDLE.json\n\
  fn64-rdp-silicon-vectors analyze-texture-lod-boundary SWEEP_ID BUNDLE.json\n\
  fn64-rdp-silicon-vectors analyze-blender-precision SWEEP_ID BUNDLE.json\n\
  fn64-rdp-silicon-vectors analyze-rdp-timing EXPERIMENT_ID BUNDLE.json\n\
  fn64-rdp-silicon-vectors [consensus] [--min-runs N] BUNDLE.json...\n\
Validate accepts any producer kind and prints each canonical bundle digest.\n\
RGB-dither analysis requires 256 input codes over a 4x4 tile in both cycles.\n\
Alpha-dither analysis requires a complete controlled 1/2-cycle alpha sweep.\n\
Alpha-coverage analysis requires all 8 coverages and 256 alphas in both cycles.\n\
Coverage-alpha analysis requires all 8 coverages and 256 thresholds in both cycles.\n\
ZMODE_INTER analysis requires 384 reset-isolated admission/coverage points.\n\
Representative-sample analysis requires 1,530 mask/cycle/observable points.\n\
Narrow-edge analysis requires 18 reset-isolated points per selected boundary.\n\
Texture-filter-tie analysis requires six reset-isolated below/on/above points.\n\
Reciprocal-S10.5 analysis requires six reset-isolated below/on/above points.\n\
Average-filter-tie analysis requires six reset-isolated below/on/above points.\n\
Texture-LOD analysis requires 18 reset-isolated mode/boundary/cycle points.\n\
Blender analysis requires 72 reset-isolated precision points and 3 ordered pairs.\n\
RDP timing analysis defaults to 10 independent hardware runs and preserves 24-bit counters.\n\
Consensus requires controlled byte-identical hardware captures.\n\
--min-runs defaults to 10 (the AGENTS.md deterministic bar).";

#[derive(Clone, Debug, PartialEq, Eq)]
enum Mode {
    Validate,
    AnalyzeRgbDither(String),
    AnalyzeAlphaDither(String),
    AnalyzeAlphaCoverage(String),
    AnalyzeCoverageAlpha(String),
    AnalyzeZModeInter(String),
    AnalyzeRepresentativeSample(String),
    AnalyzeNarrowEdgeCoverage(String),
    AnalyzeTextureFilterTie(String),
    AnalyzeReciprocalS10_5(String),
    AnalyzeAverageFilterTie(String),
    AnalyzeTextureLodBoundary(String),
    AnalyzeBlenderPrecision(String),
    AnalyzeRdpTiming(String),
    Consensus,
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("fn64-rdp-silicon-vectors: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<(), String> {
    let mut arguments = env::args().skip(1).peekable();
    let mode =
        match arguments.peek().map(String::as_str) {
            Some("validate") => {
                arguments.next();
                Mode::Validate
            }
            Some("consensus") => {
                arguments.next();
                Mode::Consensus
            }
            Some("analyze-alpha-dither") => {
                arguments.next();
                Mode::AnalyzeAlphaDither(
                    arguments
                        .next()
                        .ok_or_else(|| format!("analyze-alpha-dither requires SWEEP_ID\n{HELP}"))?,
                )
            }
            Some("analyze-rgb-dither") => {
                arguments.next();
                Mode::AnalyzeRgbDither(
                    arguments
                        .next()
                        .ok_or_else(|| format!("analyze-rgb-dither requires SWEEP_ID\n{HELP}"))?,
                )
            }
            Some("analyze-alpha-coverage") => {
                arguments.next();
                Mode::AnalyzeAlphaCoverage(
                    arguments.next().ok_or_else(|| {
                        format!("analyze-alpha-coverage requires SWEEP_ID\n{HELP}")
                    })?,
                )
            }
            Some("analyze-coverage-alpha") => {
                arguments.next();
                Mode::AnalyzeCoverageAlpha(
                    arguments.next().ok_or_else(|| {
                        format!("analyze-coverage-alpha requires SWEEP_ID\n{HELP}")
                    })?,
                )
            }
            Some("analyze-zmode-inter") => {
                arguments.next();
                Mode::AnalyzeZModeInter(
                    arguments
                        .next()
                        .ok_or_else(|| format!("analyze-zmode-inter requires SWEEP_ID\n{HELP}"))?,
                )
            }
            Some("analyze-representative-sample") => {
                arguments.next();
                Mode::AnalyzeRepresentativeSample(arguments.next().ok_or_else(|| {
                    format!("analyze-representative-sample requires SWEEP_ID\n{HELP}")
                })?)
            }
            Some("analyze-narrow-edge-coverage") => {
                arguments.next();
                Mode::AnalyzeNarrowEdgeCoverage(arguments.next().ok_or_else(|| {
                    format!("analyze-narrow-edge-coverage requires SWEEP_ID\n{HELP}")
                })?)
            }
            Some("analyze-texture-filter-tie") => {
                arguments.next();
                Mode::AnalyzeTextureFilterTie(arguments.next().ok_or_else(|| {
                    format!("analyze-texture-filter-tie requires SWEEP_ID\n{HELP}")
                })?)
            }
            Some("analyze-reciprocal-s10-5") => {
                arguments.next();
                Mode::AnalyzeReciprocalS10_5(
                    arguments.next().ok_or_else(|| {
                        format!("analyze-reciprocal-s10-5 requires SWEEP_ID\n{HELP}")
                    })?,
                )
            }
            Some("analyze-average-filter-tie") => {
                arguments.next();
                Mode::AnalyzeAverageFilterTie(arguments.next().ok_or_else(|| {
                    format!("analyze-average-filter-tie requires SWEEP_ID\n{HELP}")
                })?)
            }
            Some("analyze-texture-lod-boundary") => {
                arguments.next();
                Mode::AnalyzeTextureLodBoundary(arguments.next().ok_or_else(|| {
                    format!("analyze-texture-lod-boundary requires SWEEP_ID\n{HELP}")
                })?)
            }
            Some("analyze-blender-precision") => {
                arguments.next();
                Mode::AnalyzeBlenderPrecision(arguments.next().ok_or_else(|| {
                    format!("analyze-blender-precision requires SWEEP_ID\n{HELP}")
                })?)
            }
            Some("analyze-rdp-timing") => {
                arguments.next();
                Mode::AnalyzeRdpTiming(
                    arguments.next().ok_or_else(|| {
                        format!("analyze-rdp-timing requires EXPERIMENT_ID\n{HELP}")
                    })?,
                )
            }
            _ => Mode::Consensus,
        };
    let mut minimum_runs = 10usize;
    let mut paths = Vec::new();
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "-h" | "--help" => {
                println!("{HELP}");
                return Ok(());
            }
            "--min-runs" => {
                if !matches!(mode, Mode::Consensus | Mode::AnalyzeRdpTiming(_)) {
                    return Err(format!(
                        "--min-runs is only valid in consensus or RDP timing mode\n{HELP}"
                    ));
                }
                let value = arguments
                    .next()
                    .ok_or_else(|| "--min-runs requires a positive integer".to_owned())?;
                minimum_runs = value
                    .parse()
                    .map_err(|_| "--min-runs requires a positive integer".to_owned())?;
            }
            _ if argument.starts_with('-') => {
                return Err(format!("unknown option {argument:?}\n{HELP}"));
            }
            _ => paths.push(argument),
        }
    }
    if paths.is_empty() {
        return Err(format!("no capture bundles supplied\n{HELP}"));
    }
    if matches!(
        mode,
        Mode::AnalyzeRgbDither(_)
            | Mode::AnalyzeAlphaDither(_)
            | Mode::AnalyzeAlphaCoverage(_)
            | Mode::AnalyzeCoverageAlpha(_)
            | Mode::AnalyzeZModeInter(_)
            | Mode::AnalyzeRepresentativeSample(_)
            | Mode::AnalyzeNarrowEdgeCoverage(_)
            | Mode::AnalyzeTextureFilterTie(_)
            | Mode::AnalyzeReciprocalS10_5(_)
            | Mode::AnalyzeAverageFilterTie(_)
            | Mode::AnalyzeTextureLodBoundary(_)
            | Mode::AnalyzeBlenderPrecision(_)
    ) && paths.len() != 1
    {
        return Err(format!(
            "analysis modes require exactly one capture bundle\n{HELP}"
        ));
    }

    let bundles = paths
        .iter()
        .map(|path| {
            let bytes = fs::read(path).map_err(|error| format!("read {path:?}: {error}"))?;
            validate_json(&bytes).map_err(|error| format!("validate {path:?}: {error}"))
        })
        .collect::<Result<Vec<_>, _>>()?;
    match mode {
        Mode::Validate => {
            for (path, bundle) in paths.iter().zip(&bundles) {
                println!("{}  {path}", bundle.canonical_sha256());
            }
        }
        Mode::AnalyzeRgbDither(sweep_id) => {
            let analysis = analyze_rgb_dither_sweep(&bundles[0], &sweep_id)
                .map_err(|error| error.to_string())?;
            serde_json::to_writer_pretty(std::io::stdout().lock(), &analysis)
                .map_err(|error| format!("write RGB-dither analysis: {error}"))?;
            println!();
        }
        Mode::AnalyzeAlphaDither(sweep_id) => {
            let analysis = analyze_alpha_dither_sweep(&bundles[0], &sweep_id)
                .map_err(|error| error.to_string())?;
            serde_json::to_writer_pretty(std::io::stdout().lock(), &analysis)
                .map_err(|error| format!("write alpha-dither analysis: {error}"))?;
            println!();
        }
        Mode::AnalyzeAlphaCoverage(sweep_id) => {
            let analysis = analyze_alpha_coverage_product_sweep(&bundles[0], &sweep_id)
                .map_err(|error| error.to_string())?;
            serde_json::to_writer_pretty(std::io::stdout().lock(), &analysis)
                .map_err(|error| format!("write alpha-coverage analysis: {error}"))?;
            println!();
        }
        Mode::AnalyzeCoverageAlpha(sweep_id) => {
            let analysis = analyze_coverage_to_alpha_sweep(&bundles[0], &sweep_id)
                .map_err(|error| error.to_string())?;
            serde_json::to_writer_pretty(std::io::stdout().lock(), &analysis)
                .map_err(|error| format!("write coverage-alpha analysis: {error}"))?;
            println!();
        }
        Mode::AnalyzeZModeInter(sweep_id) => {
            let analysis = analyze_zmode_inter_coverage_sweep(&bundles[0], &sweep_id)
                .map_err(|error| error.to_string())?;
            serde_json::to_writer_pretty(std::io::stdout().lock(), &analysis)
                .map_err(|error| format!("write ZMODE_INTER analysis: {error}"))?;
            println!();
        }
        Mode::AnalyzeRepresentativeSample(sweep_id) => {
            let analysis = analyze_representative_sample_selector_sweep(&bundles[0], &sweep_id)
                .map_err(|error| error.to_string())?;
            serde_json::to_writer_pretty(std::io::stdout().lock(), &analysis)
                .map_err(|error| format!("write representative-sample analysis: {error}"))?;
            println!();
        }
        Mode::AnalyzeNarrowEdgeCoverage(sweep_id) => {
            let analysis = analyze_narrow_edge_coverage_correction_sweep(&bundles[0], &sweep_id)
                .map_err(|error| error.to_string())?;
            serde_json::to_writer_pretty(std::io::stdout().lock(), &analysis)
                .map_err(|error| format!("write narrow-edge-coverage analysis: {error}"))?;
            println!();
        }
        Mode::AnalyzeTextureFilterTie(sweep_id) => {
            let analysis = analyze_texture_filter_tie_sweep(&bundles[0], &sweep_id)
                .map_err(|error| error.to_string())?;
            serde_json::to_writer_pretty(std::io::stdout().lock(), &analysis)
                .map_err(|error| format!("write texture-filter-tie analysis: {error}"))?;
            println!();
        }
        Mode::AnalyzeReciprocalS10_5(sweep_id) => {
            let analysis = analyze_reciprocal_s10_5_boundary_sweep(&bundles[0], &sweep_id)
                .map_err(|error| error.to_string())?;
            serde_json::to_writer_pretty(std::io::stdout().lock(), &analysis)
                .map_err(|error| format!("write reciprocal-S10.5 analysis: {error}"))?;
            println!();
        }
        Mode::AnalyzeAverageFilterTie(sweep_id) => {
            let analysis = analyze_average_filter_output_tie_sweep(&bundles[0], &sweep_id)
                .map_err(|error| error.to_string())?;
            serde_json::to_writer_pretty(std::io::stdout().lock(), &analysis)
                .map_err(|error| format!("write average-filter-tie analysis: {error}"))?;
            println!();
        }
        Mode::AnalyzeTextureLodBoundary(sweep_id) => {
            let analysis = analyze_texture_lod_boundary_sweep(&bundles[0], &sweep_id)
                .map_err(|error| error.to_string())?;
            serde_json::to_writer_pretty(std::io::stdout().lock(), &analysis)
                .map_err(|error| format!("write texture-LOD analysis: {error}"))?;
            println!();
        }
        Mode::AnalyzeBlenderPrecision(sweep_id) => {
            let analysis = analyze_blender_precision_sweep(&bundles[0], &sweep_id)
                .map_err(|error| error.to_string())?;
            serde_json::to_writer_pretty(std::io::stdout().lock(), &analysis)
                .map_err(|error| format!("write blender-precision analysis: {error}"))?;
            println!();
        }
        Mode::AnalyzeRdpTiming(experiment_id) => {
            let analysis =
                analyze_rdp_completion_timing_series(&bundles, &experiment_id, minimum_runs)
                    .map_err(|error| error.to_string())?;
            serde_json::to_writer_pretty(std::io::stdout().lock(), &analysis)
                .map_err(|error| format!("write RDP completion timing analysis: {error}"))?;
            println!();
        }
        Mode::Consensus => {
            let consensus = validate_hardware_consensus(&bundles, minimum_runs)
                .map_err(|error| error.to_string())?;
            serde_json::to_writer_pretty(std::io::stdout().lock(), &consensus)
                .map_err(|error| format!("write consensus: {error}"))?;
            println!();
        }
    }
    Ok(())
}
