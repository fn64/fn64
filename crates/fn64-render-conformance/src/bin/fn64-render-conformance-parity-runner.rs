//! The parity differential: fn64's shipping wgpu backend against RT64, the
//! oracle. The reference backend is run and reported alongside, but it is
//! NOT an authority and never enters a verdict.
//!
//! **`fn64-render-reference` is a second fn64 implementation, not a third
//! opinion.** It is unproven against hardware, and it has been WRONG on this
//! corpus's own cases: on every textured case here it returns a third set of
//! values that matches neither the hand-derived key nor RT64
//! (`RT64-HANDOFF.md` §3f records the one-cycle case where wgpu was right and
//! the reference was wrong). Read its column as a hint about where to look,
//! never as evidence for or against wgpu. Every verdict below is computed
//! from the wgpu/RT64 pair and the key alone.
//!
//! # Why this binary exists
//!
//! `fn64-render-conformance-wgpu-runner sweep` already compares wgpu against
//! the reference backend. That answers "do fn64's two Rust backends agree",
//! which is a useful consistency check and is NOT a parity measurement. The
//! port's stated purpose is matching RT64. So the number worth reporting is
//! wgpu-vs-RT64 over one replay input, and that is what this binary computes.
//!
//! It is a separate binary rather than a subcommand of the wgpu runner
//! because it needs the `rt64` feature, which drags in a C++ build (macOS
//! and Linux; `.github/workflows/rt64-oracle.yml` runs it on a Linux
//! runner with Lavapipe). Keeping it separate leaves the wgpu runner
//! buildable without that toolchain. Unlike the deferred-history runner,
//! this binary needs no Metal-specific API, so it is not gated to macOS.
//!
//! # RT64 is NOT authoritative everywhere, and the metric must say so
//!
//! `docs/rt64/RT64-GUARD-AUDIT.md` established that RT64 stops modelling the
//! hardware downstream of coverage: memory alpha is hardcoded to `1.0f` under
//! the comment "Coverage is not emulated" (`hle/rt64_blender.h:355-357`),
//! there is no hidden-bits sidecar, and `AA_EN` / `ALPHA_CVG_SEL` reach only a
//! debugger text line. angrylion is the sole authority there.
//!
//! A parity percentage that silently included coverage-dependent cases would
//! be measuring the wrong thing: a wgpu-vs-RT64 difference in such a case is
//! evidence about RT64's modelling gap, not about wgpu. So every case
//! declares an [`Authority`], the two partitions are counted separately, and
//! they are never added together into one percentage.
//!
//! # The answer key is a third authority, not either backend
//!
//! Every case carries a hand-derived `expected` function computed as
//! arithmetic over its own display list from public RDP semantics. It exists
//! so that when two backends disagree, the row can say which one the key
//! blesses -- attribution, not merely a count. No key is ever captured from a
//! backend's output.

#![allow(unsafe_code)]

use std::{
    fs::File,
    io::{self, Write},
    os::fd::FromRawFd,
};

use fn64_render::{
    AspectTarget, RawDpcBackend, RenderAspectRatio, RenderBackend, RenderConfig, RenderFiltering,
    RenderGraphicsApi, RenderRuntimeSettings,
};
use fn64_render_reference::ReferenceBackend;
use fn64_render_rt64::Rt64Backend;
use fn64_render_wgpu::conformance::{ConformanceReplay, ConformanceSession};
use fn64_runtime::{RdramAddr, RdramViewMut};
use serde_json::{json, Value};

#[path = "fn64-render-conformance-parity-runner/backends.rs"]
mod backends;
#[path = "fn64-render-conformance-parity-runner/cases_fn.rs"]
mod cases_fn;
#[path = "fn64-render-conformance-parity-runner/fixtures.rs"]
mod fixtures;
#[path = "fn64-render-conformance-parity-runner/generated_cases_fn.rs"]
mod generated_cases_fn;
#[path = "fn64-render-conformance-parity-runner/generated_machinery.rs"]
mod generated_machinery;
#[path = "fn64-render-conformance-parity-runner/grading.rs"]
mod grading;

use backends::*;
use cases_fn::*;
use fixtures::*;
use generated_cases_fn::*;
use generated_machinery::*;
use grading::*;

/// Run the synthetic generator corpus, three-way comparing every case against
/// angrylion ground truth and classifying each per the triage rubric.
fn run_generated() -> Value {
    let mut cases = generated_cases();
    cases.sort_by_key(|c| (c.priority, c.name.clone()));
    // Optional substring filter for fast, targeted triage of a single slice
    // (e.g. FN64_ONLY=loadblock-deep). Absent, the whole corpus runs.
    if let Ok(filter) = std::env::var("FN64_ONLY") {
        if !filter.is_empty() {
            cases.retain(|c| c.name.contains(&filter));
        }
    }

    let mut rows = Vec::new();
    let mut counts: std::collections::BTreeMap<&'static str, usize> = std::collections::BTreeMap::new();

    for case in &cases {
        let angrylion = angrylion_bytes(&case.commands);
        let wgpu = wgpu_outcome(&case.commands);
        let rt64 = rt64_bytes(&case.commands);

        let classification = triage(&angrylion, &wgpu, &rt64);
        *counts.entry(classification).or_default() += 1;

        rows.push(json!({
            "case": case.name,
            "priority": case.priority,
            "intent": case.intent,
            "command_words": case.commands.len(),
            "classification": classification,
            "angrylion": if angrylion_is_skipped(&angrylion) {
                json!("skipped")
            } else {
                outcome_wire(&angrylion)
            },
            "wgpu": outcome_wire(&wgpu),
            "rt64": outcome_wire(&rt64),
            "wgpu_vs_angrylion_diff_pixels": diff_count(&angrylion, &wgpu),
            "rt64_vs_angrylion_diff_pixels": diff_count(&angrylion, &rt64),
            "wgpu_vs_angrylion_first_diff": first_diff(&angrylion, &wgpu),
            "rt64_vs_angrylion_first_diff": first_diff(&angrylion, &rt64),
        }));
    }

    json!({
        "schema": "fn64.render-conformance.parity.generated.v1",
        "oracle": "angrylion-rdp-plus (bit-accurate hardware ground truth)",
        "candidates": ["fn64-render-wgpu", "fn64-render-rt64"],
        "target": { "width": WIDTH, "height": HEIGHT, "format": "rgba16" },
        "corpus_provenance": "hand-derived synthetic streams; NO case captured from a running ROM",
        "triage_legend": {
            "pass-all-match-hardware": "wgpu==angrylion and rt64==angrylion",
            "fn64-defect": "wgpu != angrylion, rt64 == angrylion (fn64 wrong)",
            "rt64-hle-defect": "wgpu == angrylion, rt64 != angrylion (RT64 HLE wrong)",
            "shared-ported-bug": "wgpu != angrylion, rt64 != angrylion, wgpu == rt64",
            "all-three-differ-inspect-construction": "all three differ; suspect stream",
            "angrylion-skipped-fallback-wgpu-vs-rt64": "oracle missing; only wgpu-vs-rt64 known",
        },
        "triage_counts": counts,
        "case_count": cases.len(),
        "rows": rows,
    })
}

fn run() -> Value {
    // Debug hatch: FN64_DUMP_CASE=<name> writes that case's seeded RDRAM image
    // to <name>.rdram.bin and its command words to stderr, then exits. Used to
    // reproduce a single case through the standalone oracle while iterating on
    // the angrylion byte domain. Never on in normal runs.
    if let Ok(target) = std::env::var("FN64_DUMP_CASE") {
        for case in cases() {
            if case.name == target {
                let rdram = seeded(&case.commands);
                let path = format!("/tmp/{}.rdram.bin", case.name);
                std::fs::write(&path, &rdram).unwrap();
                eprintln!("wrote {path}");
                eprintln!("cmd_start=0x{COMMAND_START:x} cmd_end=0x{:x}", command_end(&case.commands));
                for (i, (w0, w1)) in case.commands.iter().enumerate() {
                    eprintln!("  [{i:2}] {w0:#010x} {w1:#010x}  op={:#04x}", w0 >> 24);
                }
                return json!({"dumped": case.name});
            }
        }
        return json!({"error": "case not found"});
    }

    // Generator mode: FN64_GENERATE=1 emits the synthetic corpus and compares
    // wgpu and RT64 against ANGRYLION as ground truth. There is no hand key.
    if std::env::var("FN64_GENERATE").as_deref() == Ok("1") {
        return run_generated();
    }

    let mut rows = Vec::new();
    let mut authoritative = Tally::default();
    let mut non_authoritative = Tally::default();

    for case in cases() {
        let key = pixels(&key_bytes(&case));
        let rt64 = rt64_bytes(&case.commands);
        let wgpu = wgpu_outcome(&case.commands);
        let reference = reference_outcome(&case.commands);
        let angrylion = angrylion_bytes(&case.commands);

        // angrylion is bit-accurate ground truth. Classify each backend's
        // agreement with it (when it produced a reading) so the report carries
        // the truth partition directly, not only the wgpu-vs-RT64 differential.
        let agrees_with_angrylion = |outcome: &Result<Vec<u8>, String>| -> Value {
            match (&angrylion, outcome) {
                (Ok(truth), Ok(bytes)) => json!(pixels(truth) == pixels(bytes)),
                _ => Value::Null,
            }
        };
        let angrylion_matches_key = match &angrylion {
            Ok(bytes) => json!(pixels(bytes) == key),
            Err(_) => Value::Null,
        };

        let verdict = Verdict::of(&rt64, &wgpu);
        match case.authority {
            Authority::Rt64Authoritative => authoritative.record(verdict),
            Authority::CoverageDependentRt64NotAuthoritative => non_authoritative.record(verdict),
            // Counted with the other non-authoritative partition for the same
            // reason: the oracle is the lane that is not modelling the
            // command, so a difference is not evidence against wgpu.
            Authority::RawTrianglePlaneScaleDisagreement => non_authoritative.record(verdict),
        }

        let matches_key = |outcome: &Result<Vec<u8>, String>| match outcome {
            Ok(bytes) => json!(pixels(bytes) == key),
            Err(_) => json!(null),
        };

        // First differing pixel, for attribution. The whole 320x240 delta is
        // far too large to emit and a count plus one located example is what
        // a reader actually acts on.
        let first_difference = match (&rt64, &wgpu) {
            (Ok(rt64), Ok(wgpu)) => {
                let (rt64, wgpu) = (pixels(rt64), pixels(wgpu));
                (0..PIXEL_COUNT as usize)
                    .find(|&index| rt64[index] != wgpu[index])
                    .map(|index| {
                        json!({
                            "pixel": index,
                            "x": index as u32 % WIDTH,
                            "y": index as u32 / WIDTH,
                            "key": format!("{:#06x}", key[index]),
                            "rt64": format!("{:#06x}", rt64[index]),
                            "wgpu": format!("{:#06x}", wgpu[index]),
                        })
                    })
                    .unwrap_or(Value::Null)
            }
            _ => Value::Null,
        };

        // Every differing pixel, capped. `first_difference` alone cannot
        // distinguish a wrong-texel fetch (a permutation of TEXTURE_TEXELS)
        // from a wrong-colour computation (a value not in the table at all),
        // because that needs the PATTERN across pixels, not one example.
        // Capped at 16 so a whole-target disagreement cannot flood the report.
        let differences = match (&rt64, &wgpu) {
            (Ok(rt64), Ok(wgpu)) => {
                let (rt64, wgpu) = (pixels(rt64), pixels(wgpu));
                let listed: Vec<Value> = (0..PIXEL_COUNT as usize)
                    .filter(|&index| rt64[index] != wgpu[index])
                    .take(16)
                    .map(|index| {
                        json!({
                            "x": index as u32 % WIDTH,
                            "y": index as u32 / WIDTH,
                            "key": format!("{:#06x}", key[index]),
                            "rt64": format!("{:#06x}", rt64[index]),
                            "wgpu": format!("{:#06x}", wgpu[index]),
                        })
                    })
                    .collect();
                json!(listed)
            }
            _ => Value::Null,
        };

        // The texel-sized window, every backend, every pixel. A wrong-texel
        // fetch and a wrong-colour computation are told apart by the PATTERN
        // over the whole window, which neither a first-difference nor a
        // differences-only list can show.
        let window = {
            let read = |outcome: &Result<Vec<u8>, String>| match outcome {
                Ok(bytes) => {
                    let got = pixels(bytes);
                    let mut out = Vec::new();
                    for y in 0..WIDE_HEIGHT {
                        for x in 0..WIDE_WIDTH {
                            out.push(format!("{:#06x}", got[(y * WIDTH + x) as usize]));
                        }
                    }
                    json!(out)
                }
                Err(_) => Value::Null,
            };
            let mut expected = Vec::new();
            for y in 0..WIDE_HEIGHT {
                for x in 0..WIDE_WIDTH {
                    expected.push(format!("{:#06x}", key[(y * WIDTH + x) as usize]));
                }
            }
            json!({
                "key": expected,
                "rt64": read(&rt64),
                "wgpu": read(&wgpu),
                "reference": read(&reference),
                "angrylion": read(&angrylion),
            })
        };

        rows.push(json!({
            "case": case.name,
            "window": window,
            "differences": differences,
            "intent": case.intent,
            "authority": case.authority.wire(),
            "verdict": verdict.wire(),
            "differing_pixels": match verdict {
                Verdict::Differs { pixels } => json!(pixels),
                Verdict::Identical => json!(0),
                _ => Value::Null,
            },
            "first_difference": first_difference,
            "rt64": outcome_wire(&rt64),
            "wgpu": outcome_wire(&wgpu),
            "reference": outcome_wire(&reference),
            "angrylion": if angrylion_is_skipped(&angrylion) {
                json!("skipped")
            } else {
                outcome_wire(&angrylion)
            },
            "rt64_matches_key": matches_key(&rt64),
            "wgpu_matches_key": matches_key(&wgpu),
            "reference_matches_key": matches_key(&reference),
            "angrylion_matches_key": angrylion_matches_key,
            "wgpu_agrees_with_angrylion": agrees_with_angrylion(&wgpu),
            "rt64_agrees_with_angrylion": agrees_with_angrylion(&rt64),
        }));
    }

    json!({
        "schema": "fn64.render-conformance.parity.v1",
        "oracle": "fn64-render-rt64",
        "candidate": "fn64-render-wgpu",
        "third_reading": "fn64-render-reference",
        "target": { "width": WIDTH, "height": HEIGHT, "format": "rgba16" },
        "corpus_provenance": "hand-authored; no case is captured from a running ROM",
        "parity": {
            "rt64_authoritative": authoritative.wire(),
            "rt64_not_authoritative_coverage": non_authoritative.wire(),
        },
        "captured_corpus": captured_row(),
        "note": "The two partitions are never summed. RT64 does not model \
                 coverage, AA or dither (docs/rt64/RT64-GUARD-AUDIT.md), so a \
                 difference in the second partition is evidence about RT64's \
                 modelling gap, not about wgpu.",
        "rows": rows,
    })
}

/// RT64's native side writes device diagnostics ("Device Name: Apple M5
/// Pro") straight to the process C stdout stream, which the C runtime flushes
/// at exit -- i.e. AFTER this runner has written its JSON. That appends
/// non-JSON lines to the document and makes the output unparseable, which is
/// exactly what happened on the first run of this binary.
///
/// The RT64 deferred-history runner solved this before us and this is its
/// approach, verbatim: duplicate the real stdout to a private descriptor to
/// write the protocol on, then point fd 1 at stderr so every native
/// diagnostic lands on the already-captured stderr pipe.
fn redirect_native_stdout() -> Result<File, Box<dyn std::error::Error>> {
    io::stdout().lock().flush()?;
    // SAFETY: flushing the process C stream before duplicating descriptors
    // prevents buffered native diagnostics from crossing the redirection.
    unsafe {
        libc::fflush(std::ptr::null_mut());
    }
    // SAFETY: dup returns a new owned descriptor or -1. No Rust owner exists
    // until the success branch constructs exactly one File below.
    let protocol_fd = unsafe { libc::dup(libc::STDOUT_FILENO) };
    if protocol_fd < 0 {
        return Err(io::Error::last_os_error().into());
    }
    // SAFETY: both descriptors are valid process descriptors. Native stdout
    // is sent to the already-captured stderr pipe before RT64 starts.
    if unsafe { libc::dup2(libc::STDERR_FILENO, libc::STDOUT_FILENO) } < 0 {
        let error = io::Error::last_os_error();
        // SAFETY: protocol_fd is the still-unowned successful dup above.
        unsafe {
            libc::close(protocol_fd);
        }
        return Err(error.into());
    }
    // SAFETY: protocol_fd is a unique owned descriptor after successful dup.
    Ok(unsafe { File::from_raw_fd(protocol_fd) })
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut protocol = redirect_native_stdout()?;
    let value = run();
    serde_json::to_writer_pretty(&mut protocol, &value)?;
    writeln!(protocol)?;
    protocol.flush()?;
    Ok(())
}

#[cfg(test)]
#[path = "fn64-render-conformance-parity-runner/tests.rs"]
mod tests;
