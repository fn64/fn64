//! Timing-diff gate: diff two device-event traces (fn64's under-test stream vs
//! a reference emulator's oracle stream), both in the producer-neutral
//! `timing_trace` schema, and report the first divergence under a two-tier
//! tolerance (zero-tolerance event ORDERING; a per-device cycle-count BAND).
//!
//! This is component 3 of the differential timing oracle (design spec
//! `docs/superpowers/specs/2026-07-23-timing-oracle-design.md`): the acceptance
//! gate every timing-refinement item (U2 PI latency, U5 AI drain / EEPROM /
//! Flash busy, U6 RSP) is graded against. It runs NO emulator — it ingests two
//! already-produced JSONL traces and diffs them.
//! A zero exit additionally requires both trace envelopes to end as
//! `completed`; matching aborted traces are failed evidence, even when both
//! event streams are empty.
//!
//! Usage:
//!   gate_timing_diff <fn64.jsonl> <reference.jsonl> [pi si ai mi vi]
//!
//! The two paths are the fn64 stream and the reference stream. The optional
//! five trailing integers override the per-device cycle bands (in R4300 CPU master cycles,
//! in order PI SI AI MI VI); omitted, the documented initial-loose bands from
//! `TimingTolerance::initial_loose()` are used.
//!
//! Exit status: 0 if the streams AGREE within tolerance, 1 if they DIVERGE (a
//! real, actionable timing-diff outcome) or on any I/O / ingest error. The
//! human report is printed either way.

use fn64_discover::timing_diff::{diff_ingests, DiffReport, TimingTolerance};
use fn64_discover::timing_trace::{ingest_jsonl, DeviceTraceIngest};
use std::io::BufReader;

fn main() {
    match run() {
        Ok(report) => {
            print!("{}", report.to_human());
            if report.agrees() {
                std::process::exit(0);
            } else {
                // A divergence is the gate's whole point, not a crash: report
                // it and exit non-zero so CI treats "out of tolerance" as fail.
                std::process::exit(1);
            }
        }
        Err(error) => {
            eprintln!("timing-diff gate FAILED: {error}");
            std::process::exit(1);
        }
    }
}

fn run() -> Result<DiffReport, String> {
    let mut args = std::env::args().skip(1);
    let fn64_path = args
        .next()
        .ok_or("usage: gate_timing_diff <fn64.jsonl> <reference.jsonl> [pi si ai mi vi]")?;
    let reference_path = args
        .next()
        .ok_or("usage: gate_timing_diff <fn64.jsonl> <reference.jsonl> [pi si ai mi vi]")?;

    let bands: Vec<u64> = args
        .map(|arg| {
            arg.parse::<u64>()
                .map_err(|_| format!("tolerance band must be a non-negative integer, got {arg:?}"))
        })
        .collect::<Result<_, _>>()?;
    let tolerance = match bands.as_slice() {
        [] => TimingTolerance::initial_loose(),
        [pi, si, ai, mi, vi] => TimingTolerance {
            pi_cycles: *pi,
            si_cycles: *si,
            ai_cycles: *ai,
            mi_cycles: *mi,
            vi_cycles: *vi,
        },
        other => {
            return Err(format!(
                "expected exactly 5 tolerance bands (pi si ai mi vi) or none, got {}",
                other.len()
            ))
        }
    };

    let fn64 = ingest_trace(&fn64_path)?;
    let reference = ingest_trace(&reference_path)?;

    // Deterministic (spec acceptance: same input -> same report). Compute twice
    // and confirm identical before returning, matching the other gates.
    let first = diff_ingests(&fn64, &reference, &tolerance);
    let second = diff_ingests(&fn64, &reference, &tolerance);
    if first != second {
        return Err("timing-diff report differed across two in-process runs".into());
    }
    Ok(first)
}

fn ingest_trace(path: &str) -> Result<DeviceTraceIngest, String> {
    let file = std::fs::File::open(path).map_err(|error| format!("opening {path}: {error}"))?;
    ingest_jsonl(BufReader::new(file)).map_err(|error| format!("ingesting {path}: {error}"))
}
