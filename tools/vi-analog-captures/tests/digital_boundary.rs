use fn64_vi_analog_captures::{
    analyze_digital_boundary_json, DigitalBoundaryEvidenceStatus, DigitalBoundaryPointIntent,
    DIGITAL_BOUNDARY_ANALYSIS_SCHEMA, DIGITAL_BOUNDARY_CAPTURE_SCHEMA,
};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::fs;
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);

fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn blob(bytes: &[u8]) -> Value {
    json!({
        "byte_len": bytes.len(),
        "sha256": digest(bytes),
        "bytes_hex": bytes.iter().map(|byte| format!("{byte:02x}")).collect::<String>(),
    })
}

fn registers(status: u32, current: u32) -> Value {
    json!({
        "status": status,
        "origin": 0x0010_0000,
        "width": 4,
        "intr": 2,
        "current": current,
        "burst": 0x03e5_2239_u32,
        "v_sync": 525,
        "h_sync": 0x0c15,
        "leap": 0x0c15_0c15_u32,
        "h_start": 0x006c_02ec_u32,
        "v_start": 0x0025_01ff_u32,
        "v_burst": 0x000e_0204_u32,
        "x_scale": 0x0000_0400_u32,
        "y_scale": 0x0000_0400_u32,
    })
}

fn profile(profile_id: &str, field: &str, status: u32, current: u32) -> Value {
    json!({
        "profile_id": profile_id,
        "registers": registers(status, current),
        "filters": {
            "pixel_type": "rgba16",
            "gamma": false,
            "gamma_dither": false,
            "divot": false,
            "dither_filter": false
        },
        "region": "ntsc",
        "field": field
    })
}

fn source_blobs(coverage_count: u8) -> (Value, Value) {
    let mut bytes = Vec::with_capacity(32);
    let mut coverage = vec![8u8; 16];
    coverage[0] = coverage_count;
    for (pixel, &count) in coverage.iter().enumerate() {
        bytes.push(pixel as u8);
        bytes.push((count - 1) >> 2);
    }
    (blob(&bytes), blob(&coverage))
}

fn push_case(
    cases: &mut Vec<Value>,
    profile_id: &str,
    observed_field: &str,
    observed_current: u32,
    intent: Value,
) {
    let index = cases.len() as u32;
    let coverage_count = intent
        .get("coverage_count_u4")
        .and_then(Value::as_u64)
        .unwrap_or(8) as u8;
    let (source_framebuffer_contents, source_coverage_counts) = source_blobs(coverage_count);
    cases.push(json!({
        "case_id": format!("digital-boundary-{index:02}"),
        "description": format!("Synthetic digital VI boundary point {index}; no silicon or parity claim."),
        "profile_id": profile_id,
        "intent": intent,
        "timing": {
            "replay_from_reset": true,
            "reset_kind": "power_cycle",
            "reset_event_id_sha256": digest(format!("digital boundary reset {index}").as_bytes()),
            "repeat_index": index,
            "retrace_event_id_sha256": digest(format!("digital boundary retrace {index}").as_bytes()),
            "retrace_index": 8,
            "observed_field": observed_field,
            "observed_current": observed_current
        },
        "source_framebuffer_contents": source_framebuffer_contents,
        "source_coverage_counts": source_coverage_counts,
        "post_vi_output_contents": blob(&[index as u8, index.wrapping_add(1) as u8, 0x80])
    }));
}

fn fixture() -> Value {
    let mut cases = Vec::with_capacity(44);
    for axis in ["horizontal", "vertical"] {
        for (edge, boundary) in [("start", 100i32), ("end", 200i32)] {
            for (position, offset) in [("before", -1i32), ("on", 0), ("after", 1)] {
                push_case(
                    &mut cases,
                    "progressive",
                    "progressive",
                    0,
                    json!({
                        "kind": "active_window_boundary",
                        "axis": axis,
                        "edge": edge,
                        "position": position,
                        "boundary_coordinate_i32": boundary,
                        "sample_coordinate_i32": boundary + offset
                    }),
                );
            }
        }
    }
    for (side, boundary) in [
        ("left", 0i32),
        ("right", 3i32),
        ("top", 0i32),
        ("bottom", 3i32),
    ] {
        for (position, offset) in [("before", -1i32), ("on", 0), ("after", 1)] {
            push_case(
                &mut cases,
                "progressive",
                "progressive",
                0,
                json!({
                    "kind": "border_fetch_boundary",
                    "side": side,
                    "position": position,
                    "boundary_coordinate_i32": boundary,
                    "sample_coordinate_i32": boundary + offset
                }),
            );
        }
    }
    for axis in ["horizontal", "vertical"] {
        for edge in ["start", "end"] {
            for available in 1..=2u8 {
                push_case(
                    &mut cases,
                    "progressive",
                    "progressive",
                    0,
                    json!({
                        "kind": "insufficient_three_sample_neighborhood",
                        "axis": axis,
                        "edge": edge,
                        "available_samples_u8": available
                    }),
                );
            }
        }
    }
    for candidate in 0..8u8 {
        push_case(
            &mut cases,
            "progressive",
            "progressive",
            0,
            json!({
                "kind": "partial_coverage_aa_centroid_candidate",
                "candidate_sample_u3": candidate,
                "candidate_x_q2_i16": i16::from(candidate),
                "candidate_y_q2_i16": 7i16 - i16::from(candidate),
                "coverage_mask_u8": 1u8 << candidate,
                "coverage_count_u4": 1
            }),
        );
    }
    for (field, profile_id, current) in [
        ("interlaced_even", "interlaced-even", 8u32),
        ("interlaced_odd", "interlaced-odd", 9u32),
    ] {
        for (line_span, offset) in [("one_line", 1i32), ("two_lines", 2i32)] {
            push_case(
                &mut cases,
                profile_id,
                field,
                current,
                json!({
                    "kind": "interlaced_line_phase",
                    "field": field,
                    "line_span": line_span,
                    "phase_origin_line_i32": 10,
                    "sample_line_i32": 10 + offset
                }),
            );
        }
    }
    assert_eq!(cases.len(), 44);
    json!({
        "schema": DIGITAL_BOUNDARY_CAPTURE_SCHEMA,
        "sweep_id": "vi-digital-boundaries-v1",
        "content_class": "synthetic_vi_probe",
        "producer": {
            "kind": "synthetic_fixture",
            "name": "fn64-test",
            "version": "1",
            "platform": "host",
            "producer_binary_sha256": digest(b"digital boundary producer"),
            "settings_sha256": digest(b"digital boundary settings")
        },
        "controls": {
            "reset_sequence_id": "power-on-program-vi-wait-retrace-read-digital-output",
            "retrace_sequence_id": "eighth-retrace-after-program",
            "progressive_profile_id": "progressive",
            "interlaced_even_profile_id": "interlaced-even",
            "interlaced_odd_profile_id": "interlaced-odd",
            "profiles": [
                profile("progressive", "progressive", 2, 0),
                profile("interlaced-even", "interlaced_even", 2 | (1 << 6), 8),
                profile("interlaced-odd", "interlaced_odd", 2 | (1 << 6), 9)
            ],
            "source_geometry": {
                "encoding": "rgba16_big_endian",
                "width": 4,
                "height": 4,
                "row_stride_bytes": 8
            },
            "post_vi_geometry": {
                "encoding": "rgb8",
                "width": 1,
                "height": 1,
                "row_stride_bytes": 3
            }
        },
        "cases": cases
    })
}

fn analyze(value: &Value) -> Result<fn64_vi_analog_captures::DigitalBoundaryAnalysis, String> {
    analyze_digital_boundary_json(&serde_json::to_vec(value).unwrap())
        .map_err(|error| error.to_string())
}

#[test]
fn digital_boundary_analysis_is_complete_deterministic_and_explicitly_non_parity() {
    let value = fixture();
    let first = analyze(&value).unwrap();
    let second = analyze(&value).unwrap();
    assert_eq!(first, second);
    assert_eq!(first.schema, DIGITAL_BOUNDARY_ANALYSIS_SCHEMA);
    assert_eq!(first.bundle_sha256.len(), 64);
    assert_eq!(first.analysis_sha256.len(), 64);
    assert_eq!(first.observations.len(), 44);
    assert_eq!(
        first.evidence_status,
        DigitalBoundaryEvidenceStatus::NonParityCaptureEnvelope
    );
    assert!(!first.parity_claimed);
    assert!(!first.base_matrix_row_closed);
    assert_eq!(
        first.observations[0].source_framebuffer.contents.byte_len,
        32
    );
    assert_eq!(
        first.observations[0]
            .source_framebuffer
            .coverage_counts
            .byte_len,
        16
    );
    assert_eq!(first.observations[0].post_vi_output.contents.byte_len, 3);
    assert!(matches!(
        first.observations[40].intent,
        DigitalBoundaryPointIntent::InterlacedLinePhase { .. }
    ));
}

#[test]
fn digital_boundary_analysis_preserves_exact_source_and_output_divergence() {
    let baseline = analyze(&fixture()).unwrap();
    let mut changed = fixture();
    let (source, coverage) = source_blobs(4);
    changed["cases"][0]["source_framebuffer_contents"] = source;
    changed["cases"][0]["source_coverage_counts"] = coverage;
    changed["cases"][0]["post_vi_output_contents"] = blob(&[0xfe, 0xdc, 0xba]);
    let changed = analyze(&changed).unwrap();
    assert_ne!(baseline.analysis_sha256, changed.analysis_sha256);
    assert_ne!(
        baseline.observations[0].source_framebuffer.contents,
        changed.observations[0].source_framebuffer.contents
    );
    assert_eq!(
        changed.observations[0].post_vi_output.contents.bytes_hex,
        "fedcba"
    );
    assert!(!changed.parity_claimed);
}

#[test]
fn digital_boundary_analysis_rejects_missing_duplicate_reset_and_timing_drift() {
    let mut missing = fixture();
    missing["cases"].as_array_mut().unwrap().remove(0);
    assert!(analyze(&missing)
        .unwrap_err()
        .contains("missing matrix point"));

    let mut duplicate = fixture();
    let mut repeated = duplicate["cases"][0].clone();
    repeated["case_id"] = json!("duplicate-logical-point");
    repeated["timing"]["repeat_index"] = json!(44);
    repeated["timing"]["reset_event_id_sha256"] = json!(digest(b"extra reset"));
    repeated["timing"]["retrace_event_id_sha256"] = json!(digest(b"extra retrace"));
    duplicate["cases"].as_array_mut().unwrap().push(repeated);
    assert!(analyze(&duplicate)
        .unwrap_err()
        .contains("duplicate digital boundary matrix point"));

    let mut reset = fixture();
    reset["cases"][0]["timing"]["replay_from_reset"] = json!(false);
    assert!(analyze(&reset)
        .unwrap_err()
        .contains("replay from power_cycle"));

    let mut retrace = fixture();
    retrace["cases"][1]["timing"]["retrace_event_id_sha256"] =
        retrace["cases"][0]["timing"]["retrace_event_id_sha256"].clone();
    assert!(analyze(&retrace)
        .unwrap_err()
        .contains("duplicate retrace event identity"));

    let mut field = fixture();
    field["cases"][40]["timing"]["observed_current"] = json!(9);
    assert!(analyze(&field)
        .unwrap_err()
        .contains("observed field/CURRENT provenance differs"));
}

#[test]
fn digital_boundary_analysis_rejects_boundary_profile_neighborhood_and_centroid_drift() {
    let mut adjacent = fixture();
    adjacent["cases"][0]["intent"]["sample_coordinate_i32"] = json!(98);
    assert!(analyze(&adjacent)
        .unwrap_err()
        .contains("sample must be exactly 99"));

    let mut profile = fixture();
    profile["cases"][0]["profile_id"] = json!("interlaced-even");
    profile["cases"][0]["timing"]["observed_field"] = json!("interlaced_even");
    profile["cases"][0]["timing"]["observed_current"] = json!(8);
    assert!(analyze(&profile)
        .unwrap_err()
        .contains("must use the declared progressive profile"));

    let mut neighborhood = fixture();
    neighborhood["cases"][24]["intent"]["available_samples_u8"] = json!(3);
    assert!(analyze(&neighborhood)
        .unwrap_err()
        .contains("must declare one or two available samples"));

    let mut centroid = fixture();
    centroid["cases"][32]["intent"]["coverage_count_u4"] = json!(2);
    assert!(analyze(&centroid)
        .unwrap_err()
        .contains("mask count is 1, not declared 2"));

    let mut controls = fixture();
    controls["controls"]["profiles"][0]["registers"]["width"] = json!(5);
    assert!(analyze(&controls)
        .unwrap_err()
        .contains("width must match source geometry"));
}

#[test]
fn cli_analyzes_digital_boundaries_with_stable_digest_and_non_parity_status() {
    let unique = NEXT_TEMP.fetch_add(1, Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!(
        "fn64-vi-digital-boundaries-{}-{unique}.json",
        std::process::id()
    ));
    fs::write(&path, serde_json::to_vec(&fixture()).unwrap()).unwrap();
    let binary = env!("CARGO_BIN_EXE_fn64-vi-analog-captures");
    let first = Command::new(binary)
        .arg("analyze-digital-boundaries")
        .arg(&path)
        .output()
        .unwrap();
    let second = Command::new(binary)
        .arg("analyze-digital-boundaries")
        .arg(&path)
        .output()
        .unwrap();
    assert!(
        first.status.success(),
        "{}",
        String::from_utf8_lossy(&first.stderr)
    );
    assert!(second.status.success());
    assert_eq!(first.stdout, second.stdout);
    let analysis: Value = serde_json::from_slice(&first.stdout).unwrap();
    assert_eq!(analysis["schema"], DIGITAL_BOUNDARY_ANALYSIS_SCHEMA);
    assert_eq!(analysis["analysis_sha256"].as_str().unwrap().len(), 64);
    assert_eq!(analysis["observations"].as_array().unwrap().len(), 44);
    assert_eq!(analysis["evidence_status"], "non_parity_capture_envelope");
    assert_eq!(analysis["parity_claimed"], false);
    assert_eq!(analysis["base_matrix_row_closed"], false);
    fs::remove_file(path).unwrap();
}
