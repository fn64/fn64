use fn64_vi_analog_captures::{
    analyze_pixel_comparison_file, analyze_pixel_comparison_json, generate_digital_vector_corpus,
    plan_capture_campaign, validate_hardware_consensus, validate_json, validate_manifest_file,
    AnalogSignal, CampaignRequirement, CaptureManifest, ConsoleRegion, CAMPAIGN_PLAN_SCHEMA,
    PIXEL_COMPARISON_REPORT_SCHEMA, PIXEL_COMPARISON_SCHEMA, SCHEMA,
};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);

fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

struct FixtureDir(PathBuf);

impl FixtureDir {
    fn new() -> Self {
        let unique = NEXT_TEMP.fetch_add(1, Ordering::Relaxed);
        let path =
            std::env::temp_dir().join(format!("fn64-vi-analog-{}-{unique}", std::process::id()));
        fs::create_dir(&path).unwrap();
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }

    fn write(&self, name: &str, bytes: &[u8]) {
        fs::write(self.0.join(name), bytes).unwrap();
    }
}

impl Drop for FixtureDir {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}

fn artifact(path: &str, bytes: &[u8]) -> Value {
    json!({
        "path": path,
        "byte_len": bytes.len(),
        "sha256": digest(bytes),
    })
}

fn fixture(run: u32, hardware: bool, output: &[u8]) -> (FixtureDir, Value) {
    let directory = FixtureDir::new();
    let framebuffer_bytes = [0x00, 0x01, 0x7c, 0x01, 0x03, 0xe1, 0xff, 0xff];
    let registers = json!({
        "status": 0x0001_001e_u32,
        "origin": 0x0010_0000,
        "width": 2,
        "intr": 2,
        "current": 0,
        "burst": 0x03e5_2239_u32,
        "v_sync": 525,
        "h_sync": 0x0c15,
        "leap": 0x0c15_0c15_u32,
        "h_start": 0x006c_02ec_u32,
        "v_start": 0x0025_01ff_u32,
        "v_burst": 0x000e_0204_u32,
        "x_scale": 0x0000_0400_u32,
        "y_scale": 0x0000_0400_u32,
    });
    let filters = json!({
        "pixel_type": "rgba16",
        "gamma": true,
        "gamma_dither": true,
        "divot": true,
        "dither_filter": true,
    });
    let vector_value = json!({
        "schema": "fn64.vi-digital-input.v2",
        "vector_id": "partial-edge-half-step-v1",
        "content_class": "synthetic_vi_probe",
        "framebuffer": {
            "encoding": "rgba16_big_endian",
            "width": 2,
            "height": 2,
            "row_stride_bytes": 4,
            "contents": {
                "byte_len": framebuffer_bytes.len(),
                "sha256": digest(&framebuffer_bytes),
                "bytes_hex": framebuffer_bytes.iter().map(|byte| format!("{byte:02x}")).collect::<String>(),
            },
            "coverage_counts": {
                "byte_len": 4,
                "sha256": digest(&[8, 8, 8, 8]),
                "bytes_hex": "08080808",
            },
        },
        "registers": registers.clone(),
        "filters": filters.clone(),
        "region": "ntsc",
        "field": "progressive",
    });
    let vector = serde_json::to_vec(&vector_value).unwrap();
    directory.write("vector.json", &vector);
    directory.write("capture.mkv", output);
    let provenance = if hardware {
        json!({
            "kind": "hardware",
            "console_class": "retail_nintendo64",
            "console_unit_id_sha256": digest(b"retail unit 1"),
            "motherboard_revision": "NUS-CPU-05",
            "video_encoder_revision": "DENC-NUS",
            "modification_state": "stock video path",
            "operator": "fn64 capture operator",
            "recorded_at_utc": format!("2026-07-20T00:00:{run:02}Z"),
        })
    } else {
        json!({
            "kind": "synthetic_fixture",
            "reason": "schema test; no console execution",
            "recorded_at_utc": format!("2026-07-20T00:00:{run:02}Z"),
        })
    };
    let value = json!({
        "schema": SCHEMA,
        "suite_id": "vi-aa-resampling-analog.ntsc.progressive.composite",
        "run_id": format!("run-{run:02}"),
        "content_class": "synthetic_vi_probe",
        "provenance": provenance,
        "digital_input": {
            "vector_id": "partial-edge-half-step-v1",
            "vector_artifact": artifact("vector.json", &vector),
            "framebuffer": {
                "encoding": "rgba16_big_endian",
                "width": 2,
                "height": 2,
                "row_stride_bytes": 4,
                "framebuffer_sha256": digest(&framebuffer_bytes),
            },
            "registers": registers,
            "filters": filters,
            "region": "ntsc",
            "field": "progressive",
            "reset_and_repeat": {
                "kind": "power_cycle",
                "sequence_id": "power-on-vector-submit-fullsync-field-8",
                "reset_event_id_sha256": digest(format!("power cycle event {run}").as_bytes()),
                "repeat_index": run,
            },
        },
        "analog_output": {
            "signal": "composite",
            "chain": {
                "device": {
                    "manufacturer": "CaptureCo",
                    "model": "Lossless 1",
                    "unit_id_sha256": digest(b"capture unit 1"),
                    "firmware": "1.2.3",
                },
                "cable": "shielded 75-ohm composite",
                "termination_ohms": 75,
                "sample_rate_hz": 54_000_000_u64,
                "encoding": "ffv1_matroska",
                "tool_name": "capture-cli",
                "tool_version": "2.0",
                "tool_binary_sha256": digest(b"capture binary"),
                "settings_sha256": digest(b"capture settings"),
            },
            "first_field": 8,
            "field_count": 4,
            "capture_artifact": artifact("capture.mkv", output),
        },
    });
    (directory, value)
}

fn validated(
    run: u32,
    hardware: bool,
    output: &[u8],
) -> (FixtureDir, fn64_vi_analog_captures::ValidatedCapture) {
    let (directory, value) = fixture(run, hardware, output);
    let capture = validate_json(&serde_json::to_vec(&value).unwrap(), directory.path()).unwrap();
    (directory, capture)
}

fn rewrite_vector(directory: &FixtureDir, manifest: &mut Value, mutate: impl FnOnce(&mut Value)) {
    let mut vector: Value =
        serde_json::from_slice(&fs::read(directory.path().join("vector.json")).unwrap()).unwrap();
    mutate(&mut vector);
    let bytes = serde_json::to_vec(&vector).unwrap();
    directory.write("vector.json", &bytes);
    manifest["digital_input"]["vector_artifact"] = artifact("vector.json", &bytes);
}

fn pixel_comparison_fixture() -> (FixtureDir, Value) {
    let directory = FixtureDir::new();
    let runs_dir = directory.path().join("runs");
    fs::create_dir(&runs_dir).unwrap();
    let reference_pixels = [10u8, 20, 30, 10, 20, 30, 10, 20, 30, 10, 20, 30];
    directory.write("reference.rgb", &reference_pixels);
    let sample_domain = br#"{"schema":"reviewed-rgb-code-domain.v1","meaning":"opaque integer code values; no parity tolerance"}"#;
    directory.write("sample-domain.json", sample_domain);

    let extractor = json!({
        "name": "reviewed-capture-extractor",
        "version": "1.0",
        "binary_sha256": digest(b"extractor binary"),
        "settings_sha256": digest(b"extractor settings"),
    });
    let mut observations = Vec::new();
    let mut captures = Vec::new();
    for run in 0..10u32 {
        let output = format!("physical analog capture {run}");
        let (source, capture_value) = fixture(run, true, output.as_bytes());
        let run_dir = runs_dir.join(format!("run-{run:02}"));
        fs::create_dir(&run_dir).unwrap();
        fs::copy(
            source.path().join("vector.json"),
            run_dir.join("vector.json"),
        )
        .unwrap();
        fs::copy(
            source.path().join("capture.mkv"),
            run_dir.join("capture.mkv"),
        )
        .unwrap();
        let capture_manifest = serde_json::to_vec_pretty(&capture_value).unwrap();
        fs::write(run_dir.join("manifest.json"), &capture_manifest).unwrap();

        let mut pixels = reference_pixels;
        pixels[11] = pixels[11].saturating_add(run as u8);
        fs::write(run_dir.join("pixels.rgb"), pixels).unwrap();
        let capture = validate_manifest_file(&run_dir.join("manifest.json")).unwrap();
        observations.push(json!({
            "run_id": format!("run-{run:02}"),
            "capture_manifest": artifact(
                &format!("runs/run-{run:02}/manifest.json"),
                &capture_manifest,
            ),
            "source_capture_sha256": capture.receipt().output_artifact_sha256,
            "extractor": extractor,
            "plane": {
                "artifact": artifact(&format!("runs/run-{run:02}/pixels.rgb"), &pixels),
                "encoding": "rgb8",
                "width": 2,
                "height": 2,
                "row_stride_bytes": 6,
            },
            "extraction": {
                "source_window": {
                    "field_number": 8,
                    "first_line": 0,
                    "line_count": 2,
                    "first_sample": 0,
                    "samples_per_line": 2,
                },
                "active_output": {
                    "x": 0,
                    "y": 0,
                    "width": 2,
                    "height": 2,
                },
            },
            "alignment": {
                "reference_x": 0,
                "reference_y": 0,
                "observation_x": 0,
                "observation_y": 0,
                "width": 2,
                "height": 2,
            },
        }));
        captures.push(capture);
    }
    let consensus = validate_hardware_consensus(&captures, 10).unwrap();
    let comparison = json!({
        "schema": PIXEL_COMPARISON_SCHEMA,
        "analysis_id": "ntsc-progressive-reviewed-rgb-v1",
        "content_class": "synthetic_vi_probe",
        "expected_consensus_sha256": consensus.consensus_sha256,
        "sample_domain_id": "reviewed-rgb-code-domain-v1",
        "sample_domain_spec": artifact("sample-domain.json", sample_domain),
        "reference": {
            "input_vector_sha256": consensus.input_vector_sha256,
            "producer": {
                "name": "fn64-reference-pixel-export",
                "version": "1.0",
                "binary_sha256": digest(b"fn64 pixel exporter"),
                "settings_sha256": digest(b"fn64 pixel settings"),
            },
            "plane": {
                "artifact": artifact("reference.rgb", &reference_pixels),
                "encoding": "rgb8",
                "width": 2,
                "height": 2,
                "row_stride_bytes": 6,
            },
            "active_output": {
                "x": 0,
                "y": 0,
                "width": 2,
                "height": 2,
            },
        },
        "observations": observations,
        "alignment_review": {
            "reviewer": "capture review operator",
            "reviewed_at_utc": "2026-07-20T01:00:00Z",
            "method": "integer crop selected from sync-locked decoded fields; no resampling",
        },
    });
    (directory, comparison)
}

#[test]
fn reviewed_pixel_comparison_emits_exact_integer_residuals_without_parity_claim() {
    let (directory, comparison) = pixel_comparison_fixture();
    let bytes = serde_json::to_vec(&comparison).unwrap();
    let first = analyze_pixel_comparison_json(&bytes, directory.path()).unwrap();
    let second = analyze_pixel_comparison_json(&bytes, directory.path()).unwrap();
    assert_eq!(first, second);
    assert_eq!(first.schema, PIXEL_COMPARISON_REPORT_SCHEMA);
    assert_eq!(first.run_count, 10);
    assert_eq!(first.aggregate.compared_pixel_count, 40);
    assert_eq!(first.aggregate.compared_sample_count, 120);
    assert_eq!(first.aggregate.exact_pixel_count, 31);
    assert_eq!(first.aggregate.exact_sample_count, 111);
    assert_eq!(first.aggregate.channels[2].absolute_error_max, 9);
    assert_eq!(first.aggregate.channels[2].sum_absolute_error, 45);
    assert_eq!(first.aggregate.channels[2].sum_squared_error, 285);
    assert_eq!(first.hardware_extractor.name, "reviewed-capture-extractor");
    assert_eq!(first.hardware_extractor.settings_sha256.len(), 64);
    assert_eq!(first.reference_active_output.width, 2);
    assert_eq!(first.runs[0].extraction.source_window.field_number, 8);
    assert!(!first.tolerance_applied);
    assert!(!first.hardware_parity_claimed);
    assert!(!first.base_matrix_row_closed);
    assert_eq!(first.report_sha256.len(), 64);
}

#[test]
fn pixel_comparison_fails_closed_on_unbound_cohort_extractor_and_alignment() {
    let (directory, comparison) = pixel_comparison_fixture();

    let mut wrong_consensus = comparison.clone();
    wrong_consensus["expected_consensus_sha256"] = json!("0".repeat(64));
    assert!(analyze_pixel_comparison_json(
        &serde_json::to_vec(&wrong_consensus).unwrap(),
        directory.path(),
    )
    .unwrap_err()
    .to_string()
    .contains("does not match recomputed hardware cohort"));

    let mut changed_extractor = comparison.clone();
    changed_extractor["observations"][9]["extractor"]["settings_sha256"] =
        json!(digest(b"different extraction"));
    assert!(analyze_pixel_comparison_json(
        &serde_json::to_vec(&changed_extractor).unwrap(),
        directory.path(),
    )
    .unwrap_err()
    .to_string()
    .contains("changes the extraction producer or settings"));

    let mut legacy = comparison.clone();
    legacy["schema"] = json!("fn64.vi-pixel-comparison.v1");
    assert!(
        analyze_pixel_comparison_json(&serde_json::to_vec(&legacy).unwrap(), directory.path(),)
            .unwrap_err()
            .to_string()
            .contains("unsupported comparison schema")
    );

    let mut changed_extraction = comparison.clone();
    changed_extraction["observations"][9]["extraction"]["source_window"]["first_sample"] = json!(1);
    assert!(analyze_pixel_comparison_json(
        &serde_json::to_vec(&changed_extraction).unwrap(),
        directory.path(),
    )
    .unwrap_err()
    .to_string()
    .contains("changes the reviewed extraction or alignment"));

    let mut changed_alignment = comparison.clone();
    changed_alignment["observations"][9]["alignment"]["width"] = json!(1);
    assert!(analyze_pixel_comparison_json(
        &serde_json::to_vec(&changed_alignment).unwrap(),
        directory.path(),
    )
    .unwrap_err()
    .to_string()
    .contains("changes the reviewed extraction or alignment"));

    let mut wrong_field = comparison.clone();
    for observation in wrong_field["observations"].as_array_mut().unwrap() {
        observation["extraction"]["source_window"]["field_number"] = json!(12);
    }
    assert!(analyze_pixel_comparison_json(
        &serde_json::to_vec(&wrong_field).unwrap(),
        directory.path(),
    )
    .unwrap_err()
    .to_string()
    .contains("source field 12 is outside captured range 8..12"));

    let mut partial_active_output = comparison.clone();
    partial_active_output["reference"]["active_output"]["width"] = json!(1);
    for observation in partial_active_output["observations"]
        .as_array_mut()
        .unwrap()
    {
        observation["extraction"]["active_output"]["width"] = json!(1);
    }
    assert!(analyze_pixel_comparison_json(
        &serde_json::to_vec(&partial_active_output).unwrap(),
        directory.path(),
    )
    .unwrap_err()
    .to_string()
    .contains("alignment must exactly cover both declared active outputs"));

    let mut out_of_bounds = comparison.clone();
    out_of_bounds["observations"][0]["alignment"]["width"] = json!(3);
    assert!(analyze_pixel_comparison_json(
        &serde_json::to_vec(&out_of_bounds).unwrap(),
        directory.path(),
    )
    .unwrap_err()
    .to_string()
    .contains("alignment window exceeds"));

    let mut short = comparison;
    short["observations"].as_array_mut().unwrap().pop();
    assert!(
        analyze_pixel_comparison_json(&serde_json::to_vec(&short).unwrap(), directory.path(),)
            .unwrap_err()
            .to_string()
            .contains("at least 10 hardware observations")
    );
}

#[test]
fn compare_pixels_cli_emits_the_same_nonclosing_report() {
    let (directory, comparison) = pixel_comparison_fixture();
    let manifest = directory.path().join("comparison.json");
    fs::write(&manifest, serde_json::to_vec_pretty(&comparison).unwrap()).unwrap();
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_fn64-vi-analog-captures"))
        .args(["compare-pixels", manifest.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["schema"], PIXEL_COMPARISON_REPORT_SCHEMA);
    assert_eq!(report["run_count"], 10);
    assert_eq!(report["tolerance_applied"], false);
    assert_eq!(report["hardware_parity_claimed"], false);
    assert_eq!(report["base_matrix_row_closed"], false);
}

#[cfg(unix)]
#[test]
fn comparison_manifest_symlink_is_rejected_before_analysis() {
    use std::os::unix::fs::symlink;
    let (directory, comparison) = pixel_comparison_fixture();
    let manifest = directory.path().join("comparison.json");
    fs::write(&manifest, serde_json::to_vec_pretty(&comparison).unwrap()).unwrap();
    let link = directory.path().join("comparison-link.json");
    symlink("comparison.json", &link).unwrap();
    assert!(analyze_pixel_comparison_file(&link)
        .unwrap_err()
        .to_string()
        .contains("regular non-symlink"));
}

#[test]
fn synthetic_manifest_is_valid_but_explicitly_noncertifying() {
    let (directory, value) = fixture(0, false, b"synthetic analog fixture");
    let capture = validate_json(&serde_json::to_vec(&value).unwrap(), directory.path()).unwrap();
    assert!(!capture.receipt().hardware_provenance);
    assert!(!capture.receipt().closure_eligible);
    assert_eq!(
        capture.receipt().input_vector_sha256,
        value["digital_input"]["vector_artifact"]["sha256"]
    );
    assert_eq!(
        capture.receipt().output_artifact_sha256,
        value["analog_output"]["capture_artifact"]["sha256"]
    );
}

#[test]
fn capture_v2_requires_a_canonical_reset_event_identity() {
    let (directory, mut legacy) = fixture(0, false, b"capture");
    legacy["schema"] = json!("fn64.vi-analog-capture.v1");
    assert!(
        validate_json(&serde_json::to_vec(&legacy).unwrap(), directory.path())
            .unwrap_err()
            .to_string()
            .contains("unsupported schema")
    );

    let (invalid_directory, mut invalid) = fixture(0, false, b"capture");
    invalid["digital_input"]["reset_and_repeat"]["reset_event_id_sha256"] = json!("not-a-hash");
    assert!(validate_json(
        &serde_json::to_vec(&invalid).unwrap(),
        invalid_directory.path()
    )
    .unwrap_err()
    .to_string()
    .contains("reset_event_id_sha256 must be 64 lowercase hexadecimal characters"));
}

#[test]
fn complete_coverage_plane_is_required_and_coherent_with_visible_storage() {
    let (directory, mut value) = fixture(0, false, b"capture");
    rewrite_vector(&directory, &mut value, |vector| {
        vector["schema"] = json!("fn64.vi-digital-input.v1");
    });
    assert!(
        validate_json(&serde_json::to_vec(&value).unwrap(), directory.path())
            .unwrap_err()
            .to_string()
            .contains("unsupported digital vector schema")
    );

    rewrite_vector(&directory, &mut value, |vector| {
        vector["schema"] = json!("fn64.vi-digital-input.v2");
        vector["framebuffer"]["coverage_counts"] = json!({
            "byte_len": 4,
            "sha256": digest(&[0, 8, 8, 8]),
            "bytes_hex": "00080808",
        });
    });
    assert!(
        validate_json(&serde_json::to_vec(&value).unwrap(), directory.path())
            .unwrap_err()
            .to_string()
            .contains("expected 1..=8")
    );

    rewrite_vector(&directory, &mut value, |vector| {
        vector["framebuffer"]["coverage_counts"] = json!({
            "byte_len": 4,
            "sha256": digest(&[4, 8, 8, 8]),
            "bytes_hex": "04080808",
        });
    });
    assert!(
        validate_json(&serde_json::to_vec(&value).unwrap(), directory.path())
            .unwrap_err()
            .to_string()
            .contains("coverage encoding disagrees")
    );
}

#[test]
fn rgba32_memory_alpha_is_coherent_with_complete_coverage() {
    let (directory, mut value) = fixture(0, false, b"capture");
    let framebuffer = [
        0x10, 0x20, 0x30, 0xe0, 0x40, 0x50, 0x60, 0xe0, 0x70, 0x80, 0x90, 0xe0, 0xa0, 0xb0, 0xc0,
        0xe0,
    ];
    rewrite_vector(&directory, &mut value, |vector| {
        vector["framebuffer"]["encoding"] = json!("rgba32_big_endian");
        vector["framebuffer"]["row_stride_bytes"] = json!(8);
        vector["framebuffer"]["contents"] = json!({
            "byte_len": framebuffer.len(),
            "sha256": digest(&framebuffer),
            "bytes_hex": framebuffer.iter().map(|byte| format!("{byte:02x}")).collect::<String>(),
        });
        vector["registers"]["status"] = json!(0x0001_001f_u32);
        vector["filters"]["pixel_type"] = json!("rgba32");
    });
    value["digital_input"]["framebuffer"]["encoding"] = json!("rgba32_big_endian");
    value["digital_input"]["framebuffer"]["row_stride_bytes"] = json!(8);
    value["digital_input"]["framebuffer"]["framebuffer_sha256"] = json!(digest(&framebuffer));
    value["digital_input"]["registers"]["status"] = json!(0x0001_001f_u32);
    value["digital_input"]["filters"]["pixel_type"] = json!("rgba32");

    validate_json(&serde_json::to_vec(&value).unwrap(), directory.path()).unwrap();
}

#[test]
fn artifact_files_lengths_and_digests_are_mandatory() {
    let (directory, mut value) = fixture(0, false, b"capture bytes");
    fs::remove_file(directory.path().join("capture.mkv")).unwrap();
    assert!(
        validate_json(&serde_json::to_vec(&value).unwrap(), directory.path())
            .unwrap_err()
            .to_string()
            .contains("missing artifact")
    );

    directory.write("capture.mkv", b"changed");
    value["analog_output"]["capture_artifact"]["byte_len"] = json!(7);
    assert!(
        validate_json(&serde_json::to_vec(&value).unwrap(), directory.path())
            .unwrap_err()
            .to_string()
            .contains("SHA-256 mismatch")
    );
}

#[test]
fn malformed_unknown_rom_and_path_escape_fail_closed() {
    let (directory, mut value) = fixture(0, false, b"capture");
    value["unexpected"] = json!(true);
    assert!(
        validate_json(&serde_json::to_vec(&value).unwrap(), directory.path())
            .unwrap_err()
            .to_string()
            .contains("unknown field")
    );

    let (_, mut rom) = fixture(1, false, b"capture");
    rom["content_class"] = json!("game_rom");
    assert!(
        validate_json(&serde_json::to_vec(&rom).unwrap(), directory.path())
            .unwrap_err()
            .to_string()
            .contains("ROM/game-derived")
    );

    let (_, mut escape) = fixture(2, false, b"capture");
    escape["digital_input"]["vector_artifact"]["path"] = json!("../vector.json");
    assert!(
        validate_json(&serde_json::to_vec(&escape).unwrap(), directory.path())
            .unwrap_err()
            .to_string()
            .contains("contained relative path")
    );
}

#[test]
fn digital_and_analog_artifacts_cannot_be_conflated() {
    let (directory, mut value) = fixture(0, false, b"capture");
    let vector = fs::read(directory.path().join("vector.json")).unwrap();
    value["analog_output"]["capture_artifact"] = artifact("vector.json", &vector);
    assert!(
        validate_json(&serde_json::to_vec(&value).unwrap(), directory.path())
            .unwrap_err()
            .to_string()
            .contains("must be distinct artifacts")
    );
}

#[test]
fn typed_filters_and_field_must_match_raw_status() {
    let (directory, mut value) = fixture(0, false, b"capture");
    value["digital_input"]["filters"]["gamma"] = json!(false);
    assert!(
        validate_json(&serde_json::to_vec(&value).unwrap(), directory.path())
            .unwrap_err()
            .to_string()
            .contains("filters do not match")
    );

    let (_, mut field) = fixture(1, false, b"capture");
    field["digital_input"]["field"] = json!("interlaced_even");
    assert!(
        validate_json(&serde_json::to_vec(&field).unwrap(), directory.path())
            .unwrap_err()
            .to_string()
            .contains("field identity")
    );

    let (_, mut parity) = fixture(2, false, b"capture");
    parity["digital_input"]["registers"]["status"] = json!(0x0001_005e_u32);
    parity["digital_input"]["field"] = json!("interlaced_odd");
    assert!(
        validate_json(&serde_json::to_vec(&parity).unwrap(), directory.path())
            .unwrap_err()
            .to_string()
            .contains("CURRENT parity")
    );
}

#[test]
fn framebuffer_range_must_fit_physical_rdram_through_the_active_last_row() {
    let (directory, mut exact) = fixture(0, false, b"capture");
    let set_origin = |manifest: &mut Value, origin: u32| {
        manifest["digital_input"]["registers"]["origin"] = json!(origin);
        let mut vector: Value =
            serde_json::from_slice(&fs::read(directory.path().join("vector.json")).unwrap())
                .unwrap();
        vector["registers"]["origin"] = json!(origin);
        let bytes = serde_json::to_vec(&vector).unwrap();
        directory.write("vector.json", &bytes);
        manifest["digital_input"]["vector_artifact"] = artifact("vector.json", &bytes);
    };
    set_origin(&mut exact, 0x007f_fff8);
    let accepted = validate_json(&serde_json::to_vec(&exact).unwrap(), directory.path()).unwrap();
    assert_eq!(
        accepted.manifest().digital_input.registers.origin,
        0x007f_fff8
    );

    set_origin(&mut exact, 0x007f_fff9);
    assert!(
        validate_json(&serde_json::to_vec(&exact).unwrap(), directory.path())
            .unwrap_err()
            .to_string()
            .contains("exceeds the eight-MiB physical RDRAM aperture")
    );
}

#[test]
fn ten_controlled_hardware_runs_form_a_cohort_without_closing_the_row() {
    let mut directories = Vec::new();
    let mut captures = Vec::new();
    for run in 0..10 {
        let output = format!("physical analog capture {run}");
        let (directory, capture) = validated(run, true, output.as_bytes());
        directories.push(directory);
        captures.push(capture);
    }
    let consensus = validate_hardware_consensus(&captures, 10).unwrap();
    assert_eq!(consensus.run_count, 10);
    assert_eq!(consensus.distinct_output_count, 10);
    assert_eq!(consensus.exact_output_sha256, None);
    assert!(!consensus.base_matrix_row_closed);
    assert_eq!(consensus.runs.len(), 10);
    assert_eq!(directories.len(), 10);

    let reversed = captures.iter().cloned().rev().collect::<Vec<_>>();
    assert_eq!(
        consensus.consensus_sha256,
        validate_hardware_consensus(&reversed, 10)
            .unwrap()
            .consensus_sha256
    );
}

#[test]
fn consensus_rejects_less_than_ten_and_synthetic_provenance() {
    let mut directories = Vec::new();
    let mut captures = Vec::new();
    for run in 0..9 {
        let (directory, capture) = validated(run, true, b"same output");
        directories.push(directory);
        captures.push(capture);
    }
    assert!(validate_hardware_consensus(&captures, 10)
        .unwrap_err()
        .to_string()
        .contains("requires at least 10 runs"));
    assert!(validate_hardware_consensus(&captures, 9)
        .unwrap_err()
        .to_string()
        .contains("minimum_runs must be at least 10"));

    let (directory, synthetic) = validated(9, false, b"same output");
    directories.push(directory);
    captures.push(synthetic);
    assert!(validate_hardware_consensus(&captures, 10)
        .unwrap_err()
        .to_string()
        .contains("lacks hardware provenance"));
}

#[test]
fn consensus_rejects_duplicate_repeat_and_changed_control() {
    let mut directories = Vec::new();
    let mut captures = Vec::new();
    for run in 0..10 {
        let (directory, capture) = validated(run, true, b"same output");
        directories.push(directory);
        captures.push(capture);
    }
    let duplicate = captures[0].clone();
    captures[9] = duplicate;
    assert!(validate_hardware_consensus(&captures, 10)
        .unwrap_err()
        .to_string()
        .contains("duplicates manifest digest"));

    let (changed_dir, mut changed_value) = fixture(9, true, b"same output");
    changed_value["analog_output"]["signal"] = json!("s_video");
    let changed = validate_json(
        &serde_json::to_vec(&changed_value).unwrap(),
        changed_dir.path(),
    )
    .unwrap();
    captures[9] = changed;
    assert!(validate_hardware_consensus(&captures, 10)
        .unwrap_err()
        .to_string()
        .contains("mismatch at analog_output.signal"));
    directories.push(changed_dir);
}

#[test]
fn consensus_requires_complete_distinct_power_cycle_events() {
    let mut directories = Vec::new();
    let mut captures = Vec::new();
    for run in 0..10 {
        let (directory, capture) = validated(run, true, b"same output");
        directories.push(directory);
        captures.push(capture);
    }

    let (gap_directory, mut gap_value) = fixture(9, true, b"same output");
    gap_value["digital_input"]["reset_and_repeat"]["repeat_index"] = json!(10);
    gap_value["digital_input"]["reset_and_repeat"]["reset_event_id_sha256"] =
        json!(digest(b"power cycle event 10"));
    captures[9] = validate_json(
        &serde_json::to_vec(&gap_value).unwrap(),
        gap_directory.path(),
    )
    .unwrap();
    assert!(validate_hardware_consensus(&captures, 10)
        .unwrap_err()
        .to_string()
        .contains("repeat_index values must exactly cover 0..10"));
    directories.push(gap_directory);

    let (duplicate_directory, mut duplicate_value) = fixture(9, true, b"same output");
    duplicate_value["digital_input"]["reset_and_repeat"]["reset_event_id_sha256"] =
        json!(digest(b"power cycle event 0"));
    captures[9] = validate_json(
        &serde_json::to_vec(&duplicate_value).unwrap(),
        duplicate_directory.path(),
    )
    .unwrap();
    assert!(validate_hardware_consensus(&captures, 10)
        .unwrap_err()
        .to_string()
        .contains("duplicates reset_event_id_sha256"));
    directories.push(duplicate_directory);
    assert_eq!(directories.len(), 12);
}

#[test]
fn manifest_file_and_cli_emit_a_noncertifying_receipt() {
    let (directory, value) = fixture(0, false, b"capture");
    let manifest = directory.path().join("manifest.json");
    fs::write(&manifest, serde_json::to_vec_pretty(&value).unwrap()).unwrap();
    assert!(
        !validate_manifest_file(&manifest)
            .unwrap()
            .receipt()
            .closure_eligible
    );

    let output = std::process::Command::new(env!("CARGO_BIN_EXE_fn64-vi-analog-captures"))
        .args(["validate", manifest.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(output.status.success());
    let receipt: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(receipt["hardware_provenance"], false);
    assert_eq!(receipt["closure_eligible"], false);
}

#[test]
fn cli_generates_a_deterministic_ntsc_corpus_and_rejects_unsupported_regions() {
    let unique = NEXT_TEMP.fetch_add(1, Ordering::Relaxed);
    let base = std::env::temp_dir().join(format!(
        "fn64-vi-corpus-cli-{}-{unique}",
        std::process::id()
    ));
    let ntsc = base.join("ntsc");
    let pal = base.join("pal");

    let generated = std::process::Command::new(env!("CARGO_BIN_EXE_fn64-vi-analog-captures"))
        .args([
            "generate-vectors",
            "--region",
            "ntsc",
            ntsc.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(generated.status.success());
    let stdout: Value = serde_json::from_slice(&generated.stdout).unwrap();
    let disk: Value = serde_json::from_slice(&fs::read(ntsc.join("corpus.json")).unwrap()).unwrap();
    assert_eq!(stdout, disk);
    assert_eq!(disk["schema"], "fn64.vi-digital-corpus.v1");
    assert_eq!(disk["vectors"].as_array().unwrap().len(), 19);

    let existing = std::process::Command::new(env!("CARGO_BIN_EXE_fn64-vi-analog-captures"))
        .args([
            "generate-vectors",
            "--region",
            "ntsc",
            ntsc.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(!existing.status.success());
    assert!(String::from_utf8(existing.stderr)
        .unwrap()
        .contains("already exists"));

    let unsupported = std::process::Command::new(env!("CARGO_BIN_EXE_fn64-vi-analog-captures"))
        .args(["generate-vectors", "--region", "pal", pal.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(!unsupported.status.success());
    assert!(String::from_utf8(unsupported.stderr)
        .unwrap()
        .contains("no PAL/MPAL register preset"));
    assert!(!pal.exists());
    fs::remove_dir_all(base).unwrap();
}

#[test]
fn campaign_plan_binds_real_public_input_without_fabricating_capture_evidence() {
    let directory = FixtureDir::new();
    let corpus = directory.path().join("corpus");
    generate_digital_vector_corpus(ConsoleRegion::Ntsc)
        .unwrap()
        .write_new(&corpus)
        .unwrap();

    let first = plan_capture_campaign(
        &corpus,
        "ntsc-aa-campaign-v1",
        "partial-aa-dither-filter-off",
        AnalogSignal::Composite,
        10,
    )
    .unwrap();
    let second = plan_capture_campaign(
        &corpus,
        "ntsc-aa-campaign-v1",
        "partial-aa-dither-filter-off",
        AnalogSignal::Composite,
        10,
    )
    .unwrap();
    assert_eq!(first, second);
    assert_eq!(first.schema, CAMPAIGN_PLAN_SCHEMA);
    assert!(!first.capture_manifests_emitted);
    assert_eq!(first.run_count, 10);
    assert_eq!(first.runs[0].run_index, 0);
    assert_eq!(first.runs[9].repeat_index, 9);
    assert!(first
        .required_hardware_provenance
        .contains(&CampaignRequirement::ConsoleUnitIdSha256));
    assert!(first
        .required_per_run_observation
        .contains(&CampaignRequirement::CaptureArtifactSha256));

    let value = serde_json::to_value(&first).unwrap();
    assert_eq!(value["evidence_status"], "planned_not_captured");
    assert_eq!(
        value["selected_vector"]["vector_id"],
        "partial-aa-dither-filter-off"
    );
    assert!(value.get("provenance").is_none());
    assert!(value.get("analog_output").is_none());
    assert!(serde_json::from_value::<CaptureManifest>(value).is_err());
}

#[test]
fn campaign_plan_fails_closed_on_short_runs_missing_vectors_and_changed_artifacts() {
    let directory = FixtureDir::new();
    let corpus = directory.path().join("corpus");
    generate_digital_vector_corpus(ConsoleRegion::Ntsc)
        .unwrap()
        .write_new(&corpus)
        .unwrap();

    let short = plan_capture_campaign(
        &corpus,
        "campaign",
        "field-progressive",
        AnalogSignal::Composite,
        9,
    )
    .unwrap_err();
    assert!(short.to_string().contains("at least 10"));

    let missing = plan_capture_campaign(
        &corpus,
        "campaign",
        "not-a-corpus-vector",
        AnalogSignal::Composite,
        10,
    )
    .unwrap_err();
    assert!(missing.to_string().contains("absent from corpus"));

    fs::write(corpus.join("vectors/field-progressive.json"), b"changed").unwrap();
    let changed = plan_capture_campaign(
        &corpus,
        "campaign",
        "field-progressive",
        AnalogSignal::Composite,
        10,
    )
    .unwrap_err();
    assert!(changed.to_string().contains("byte_len mismatch"));
}

#[test]
fn cli_emits_a_non_evidence_campaign_handoff() {
    let directory = FixtureDir::new();
    let corpus = directory.path().join("corpus");
    generate_digital_vector_corpus(ConsoleRegion::Ntsc)
        .unwrap()
        .write_new(&corpus)
        .unwrap();
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_fn64-vi-analog-captures"))
        .args([
            "plan-campaign",
            "--campaign-id",
            "ntsc-divot-v1",
            "--vector",
            "divot-on",
            "--signal",
            "s-video",
            "--runs",
            "10",
            corpus.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let plan: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(plan["schema"], CAMPAIGN_PLAN_SCHEMA);
    assert_eq!(plan["evidence_status"], "planned_not_captured");
    assert_eq!(plan["capture_manifests_emitted"], false);
    assert_eq!(plan["signal"], "s_video");
    assert_eq!(plan["runs"].as_array().unwrap().len(), 10);
}

#[cfg(unix)]
#[test]
fn artifact_symlinks_are_rejected() {
    use std::os::unix::fs::symlink;
    let (directory, mut value) = fixture(0, false, b"capture");
    symlink("capture.mkv", directory.path().join("capture-link.mkv")).unwrap();
    value["analog_output"]["capture_artifact"]["path"] = json!("capture-link.mkv");
    assert!(
        validate_json(&serde_json::to_vec(&value).unwrap(), directory.path())
            .unwrap_err()
            .to_string()
            .contains("non-symlink")
    );
}

#[cfg(unix)]
#[test]
fn artifact_parent_symlink_cannot_escape_the_canonical_base() {
    use std::os::unix::fs::symlink;
    let (directory, mut value) = fixture(0, false, b"capture");
    let outside = FixtureDir::new();
    outside.write("capture.mkv", b"outside capture");
    symlink(outside.path(), directory.path().join("outside-link")).unwrap();
    value["analog_output"]["capture_artifact"] =
        artifact("outside-link/capture.mkv", b"outside capture");
    assert!(
        validate_json(&serde_json::to_vec(&value).unwrap(), directory.path())
            .unwrap_err()
            .to_string()
            .contains("escapes canonical artifact base")
    );
}

#[cfg(unix)]
#[test]
fn manifest_symlink_is_rejected_before_reading() {
    use std::os::unix::fs::symlink;
    let (directory, value) = fixture(0, false, b"capture");
    let manifest = directory.path().join("manifest.json");
    fs::write(&manifest, serde_json::to_vec_pretty(&value).unwrap()).unwrap();
    let link = directory.path().join("manifest-link.json");
    symlink("manifest.json", &link).unwrap();
    assert!(validate_manifest_file(&link)
        .unwrap_err()
        .to_string()
        .contains("regular non-symlink"));
}
