//! Measure the pure mechanical overlay-discovery phases on one ROM.
//!
//! The stable receipt hashes the complete recovery and materialized recipes,
//! so performance changes can be compared without treating timing alone as a
//! correctness claim. ROM bytes and recovered game content are never written.

use fn64_discover::delta_vote::DeltaVoteConfig;
use fn64_discover::overlay_recipe::admitted_overlay_load_recipes_v1;
use fn64_discover::overlay_regions::{
    admit_overlay_region_tables, enumerate_family_tables, SearchConfig,
};
use sha2::{Digest, Sha256};
use std::hint::black_box;
use std::path::PathBuf;
use std::time::{Duration, Instant};

#[derive(Clone, Copy, Debug)]
struct PhaseTimes {
    normalize: Duration,
    enumerate: Duration,
    admit: Duration,
    recipes: Duration,
}

impl PhaseTimes {
    fn total(self) -> Duration {
        self.normalize + self.enumerate + self.admit + self.recipes
    }
}

fn usage() -> String {
    "usage: profile_overlay_regions <ROM> [--runs N]".to_string()
}

fn main() {
    if let Err(error) = run() {
        eprintln!("profile_overlay_regions: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let mut arguments = std::env::args_os().skip(1);
    let rom_path = arguments.next().map(PathBuf::from).ok_or_else(usage)?;
    let mut runs = 1usize;
    while let Some(argument) = arguments.next() {
        match argument.to_str() {
            Some("--runs") => {
                runs = arguments
                    .next()
                    .and_then(|value| value.to_str().and_then(|value| value.parse().ok()))
                    .filter(|&value| value > 0)
                    .ok_or_else(|| "--runs requires a positive integer".to_string())?;
            }
            Some(other) => return Err(format!("unknown argument {other:?}\n{}", usage())),
            None => return Err("arguments must be valid UTF-8".to_string()),
        }
    }

    let read_start = Instant::now();
    let source = std::fs::read(&rom_path)
        .map_err(|error| format!("reading {}: {error}", rom_path.display()))?;
    let read = read_start.elapsed();
    println!(
        "rom={} bytes={} read_ms={}",
        rom_path.display(),
        source.len(),
        read.as_millis()
    );

    let config = SearchConfig::aki_family();
    let delta_config = DeltaVoteConfig::default();
    let mut baseline_receipt = None;
    let mut samples = Vec::with_capacity(runs);
    for run_index in 0..runs {
        let start = Instant::now();
        let rom = fn64_discover::normalize(black_box(&source))
            .map_err(|error| format!("normalizing ROM: {error}"))?;
        let normalize = start.elapsed();

        let start = Instant::now();
        let candidates = enumerate_family_tables(black_box(&rom.bytes), &config);
        let candidate_count = candidates.len();
        let enumerate = start.elapsed();

        let start = Instant::now();
        let recovery = admit_overlay_region_tables(
            black_box(&rom.bytes),
            &config,
            &delta_config,
            config.min_records,
            candidates,
        );
        let admit = start.elapsed();

        let start = Instant::now();
        let recipes = admitted_overlay_load_recipes_v1(black_box(&rom.bytes), &recovery)
            .map_err(|error| format!("materializing recipes: {error:?}"))?;
        let recipes_time = start.elapsed();

        if run_index == 0 {
            for (index, recipe) in recipes.iter().enumerate() {
                println!(
                    "recipe={} descriptor_rom={:#x} rom=[{:#x},{:#x}) load=[{:#x},{:#x}) text=[{:#x},{:#x}) data=[{:#x},{:#x}) bss=[{:#x},{:#x}) sha256={}",
                    index,
                    recipe.descriptor_rom_offset,
                    recipe.rom_start,
                    recipe.rom_end,
                    recipe.load_start,
                    recipe.data_end,
                    recipe.text_start,
                    recipe.text_end,
                    recipe.data_start,
                    recipe.data_end,
                    recipe.bss_start,
                    recipe.bss_end,
                    recipe.loaded_sha256,
                );
            }
        }

        let receipt_bytes = serde_json::to_vec(&(&recovery, &recipes))
            .map_err(|error| format!("serializing stable receipt: {error}"))?;
        let receipt = format!("{:x}", Sha256::digest(receipt_bytes));
        match &baseline_receipt {
            None => baseline_receipt = Some(receipt.clone()),
            Some(expected) if expected == &receipt => {}
            Some(expected) => {
                return Err(format!(
                    "run {} receipt {receipt} differs from first run {expected}",
                    run_index + 1
                ));
            }
        }
        let admitted_count = recovery
            .admissions
            .iter()
            .filter(|admission| admission.admitted)
            .count();
        let times = PhaseTimes {
            normalize,
            enumerate,
            admit,
            recipes: recipes_time,
        };
        println!(
            "run={} normalize_ms={} enumerate_ms={} admit_ms={} recipes_ms={} total_ms={} candidates={} admitted={} recipes={} receipt={}",
            run_index + 1,
            times.normalize.as_millis(),
            times.enumerate.as_millis(),
            times.admit.as_millis(),
            times.recipes.as_millis(),
            times.total().as_millis(),
            candidate_count,
            admitted_count,
            recipes.len(),
            receipt,
        );
        samples.push(times.total());
    }

    samples.sort_unstable();
    println!(
        "summary runs={} min_ms={} median_ms={} max_ms={} receipt={}",
        samples.len(),
        samples.first().unwrap().as_millis(),
        samples[samples.len() / 2].as_millis(),
        samples.last().unwrap().as_millis(),
        baseline_receipt.unwrap(),
    );
    Ok(())
}
