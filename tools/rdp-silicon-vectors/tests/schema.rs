use fn64_rdp_silicon_vectors::{
    analyze_alpha_coverage_product_sweep, analyze_alpha_dither_sweep,
    analyze_average_filter_output_tie_sweep, analyze_blender_precision_sweep,
    analyze_coverage_to_alpha_sweep, analyze_narrow_edge_coverage_correction_sweep,
    analyze_reciprocal_s10_5_boundary_sweep, analyze_representative_sample_selector_sweep,
    analyze_rgb_dither_sweep, analyze_texture_filter_tie_sweep, analyze_texture_lod_boundary_sweep,
    analyze_zmode_inter_coverage_sweep, validate_hardware_consensus, validate_json,
    AverageFilterChannel, AverageFilterTiePosition, BlenderProbeMode, FilterTieBoundaryPosition,
    NarrowEdgeBoundaryPosition, ProbeCycleType, ReciprocalBoundaryPosition,
    RepresentativeSampleObservable, RgbDitherChannel, RgbDitherMode, TextureLodBoundaryPosition,
    TextureLodMode, ZModeInterRelation, SCHEMA,
};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::fs;
use std::process::Command;

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

fn fixture() -> Value {
    let command = [0xe9, 0, 0, 0, 0, 0, 0, 0];
    let framebuffer = [0u8; 8];
    let depth = [0xffu8; 8];
    let coverage = [3u8; 4];
    json!({
        "schema": SCHEMA,
        "suite_id": "synthetic.full-sync.zero",
        "content_class": "synthetic_raw_dpc",
        "producer": {
            "kind": "synthetic_fixture",
            "name": "fn64-test",
            "version": "1",
            "platform": "host",
            "adapter": "unit-test",
            "adapter_version": "1",
            "producer_binary_sha256": digest(b"synthetic producer"),
            "settings_sha256": digest(b"synthetic settings"),
            "capture_method": "constructed fixture; no execution",
            "recorded_at_utc": "2026-07-19T00:00:00Z"
        },
        "cases": [{
            "case_id": "full-sync-zero",
            "description": "Schema-only fixture; no silicon claim.",
            "command_bytes": blob(&command),
            "setup": {
                "registers": [
                    {"name": "dpc_start", "value": 4096},
                    {"name": "dpc_end", "value": 4104},
                    {"name": "dpc_status", "value": 0}
                ],
                "initial_memory": []
            },
            "expected": {
                "framebuffer": {
                    "address": 8192,
                    "width": 2,
                    "height": 2,
                    "row_stride_bytes": 4,
                    "encoding": "rgba16_big_endian",
                    "contents": blob(&framebuffer)
                },
                "depth": {
                    "address": 12288,
                    "width": 2,
                    "height": 2,
                    "row_stride_bytes": 4,
                    "contents": blob(&depth)
                },
                "coverage": {
                    "color_image_address": 8192,
                    "width": 2,
                    "height": 2,
                    "encoding": "rgba16_hidden_bits_u2",
                    "contents": blob(&coverage)
                }
            }
        }]
    })
}

fn validate(value: &Value) -> Result<(), String> {
    validate_json(&serde_json::to_vec(value).unwrap())
        .map(|_| ())
        .map_err(|error| error.to_string())
}

fn hardware_fixture(run: usize) -> Value {
    let mut value = fixture();
    value["producer"]["kind"] = json!("hardware");
    value["producer"]["name"] = json!("n64-retail-console");
    value["producer"]["platform"] = json!("retail-unit-hash-01");
    value["producer"]["recorded_at_utc"] = json!(format!("2026-07-19T00:00:{run:02}Z"));
    value["producer"]["producer_binary_sha256"] = json!(digest(b"controlled capture binary"));
    value["producer"]["settings_sha256"] = json!(digest(b"controlled capture settings"));
    value
}

fn hardware_runs(count: usize) -> Vec<fn64_rdp_silicon_vectors::ValidatedBundle> {
    (0..count)
        .map(|run| validate_json(&serde_json::to_vec(&hardware_fixture(run)).unwrap()).unwrap())
        .collect()
}

fn alpha_dither_sweep(one_cycle_transition: u8, two_cycle_transition: u8) -> Value {
    let mut value = fixture();
    value["suite_id"] = json!("synthetic.alpha-dither.threshold-sweep");
    let template = value["cases"][0].clone();
    let mut cases = Vec::with_capacity(512);
    for (cycle_type, transition) in [
        ("one_cycle", one_cycle_transition),
        ("two_cycle", two_cycle_transition),
    ] {
        for alpha in 0u8..=u8::MAX {
            let mut case = template.clone();
            case["case_id"] = json!(format!("alpha-dither-{cycle_type}-{alpha:03}"));
            case["description"] = json!(format!(
                "Synthetic {cycle_type} alpha-dither point at combined alpha {alpha}; no silicon claim."
            ));
            case["capture_intent"] = json!({
                "kind": "alpha_compare_dither_sweep",
                "sweep_id": "sample-zero",
                "cycle_type": cycle_type,
                "combined_alpha": alpha,
                "replay_from_reset": true,
                "sample_index": 0,
                "pass_rgba16_be": 0x7c01,
                "reject_rgba16_be": 0x0001
            });
            let marker = if alpha >= transition {
                [0x7c, 0x01]
            } else {
                [0x00, 0x01]
            };
            case["expected"]["framebuffer"] = json!({
                "address": 8192,
                "width": 1,
                "height": 1,
                "row_stride_bytes": 2,
                "encoding": "rgba16_big_endian",
                "contents": blob(&marker)
            });
            case["expected"]["depth"] = json!({
                "address": 12288,
                "width": 1,
                "height": 1,
                "row_stride_bytes": 2,
                "contents": blob(&[0xff, 0xff])
            });
            case["expected"]["coverage"] = json!({
                "color_image_address": 8192,
                "width": 1,
                "height": 1,
                "encoding": "rgba16_hidden_bits_u2",
                "contents": blob(&[3])
            });
            cases.push(case);
        }
    }
    value["cases"] = Value::Array(cases);
    value
}

fn rgb_dither_sweep() -> Value {
    let mut value = fixture();
    value["suite_id"] = json!("synthetic.rgb-dither.magic-square-red");
    let template = value["cases"][0].clone();
    let mut cases = Vec::with_capacity(512);
    for (cycle_type, cycle_bias) in [("one_cycle", 0u16), ("two_cycle", 1u16)] {
        for channel_value in 0u8..=u8::MAX {
            let mut case = template.clone();
            case["case_id"] = json!(format!("rgb-dither-{cycle_type}-{channel_value:03}"));
            case["description"] = json!(format!(
                "Synthetic {cycle_type} RGB-dither point at red {channel_value}; no silicon claim."
            ));
            case["capture_intent"] = json!({
                "kind": "rgb_dither_sweep",
                "sweep_id": "magic-red-v1",
                "cycle_type": cycle_type,
                "mode": "magic_square",
                "swept_channel": "red",
                "input_rgb8": [channel_value, 128, 128],
                "channel_value": channel_value,
                "origin_x": 12,
                "origin_y": 20,
                "replay_from_reset": true,
                "sample_index": 0
            });
            let mut framebuffer = Vec::with_capacity(32);
            for pixel in 0u16..16 {
                let dither = (pixel % 4 + pixel / 4 + cycle_bias) & 7;
                let red = (u16::from(channel_value) + dither).min(255) >> 3;
                let rgba16 = (red << 11) | (16 << 6) | (16 << 1) | 1;
                framebuffer.extend_from_slice(&rgba16.to_be_bytes());
            }
            case["expected"]["framebuffer"] = json!({
                "address": 8192,
                "width": 4,
                "height": 4,
                "row_stride_bytes": 8,
                "encoding": "rgba16_big_endian",
                "contents": blob(&framebuffer)
            });
            case["expected"]["depth"] = json!({
                "address": 12288,
                "width": 4,
                "height": 4,
                "row_stride_bytes": 8,
                "contents": blob(&[0xff; 32])
            });
            case["expected"]["coverage"] = json!({
                "color_image_address": 8192,
                "width": 4,
                "height": 4,
                "encoding": "rgba16_hidden_bits_u2",
                "contents": blob(&[3; 16])
            });
            cases.push(case);
        }
    }
    value["cases"] = Value::Array(cases);
    value
}

fn alpha_coverage_product_sweep() -> Value {
    let mut value = fixture();
    value["suite_id"] = json!("synthetic.alpha-coverage.product-sweep");
    let template = value["cases"][0].clone();
    let mut cases = Vec::with_capacity(4096);
    for cycle_type in ["one_cycle", "two_cycle"] {
        for input_coverage in 1u8..=8 {
            for alpha in 0u8..=u8::MAX {
                let mut case = template.clone();
                case["case_id"] = json!(format!(
                    "alpha-coverage-{cycle_type}-{input_coverage}-{alpha:03}"
                ));
                case["description"] = json!(format!(
                    "Synthetic {cycle_type} alpha-coverage point for coverage {input_coverage}, alpha {alpha}; no silicon claim."
                ));
                case["capture_intent"] = json!({
                    "kind": "alpha_coverage_product_sweep",
                    "sweep_id": "product-v1",
                    "cycle_type": cycle_type,
                    "input_coverage": input_coverage,
                    "combined_alpha": alpha,
                    "replay_from_reset": true
                });
                let output = if alpha == 0 {
                    8
                } else {
                    ((u16::from(input_coverage) * u16::from(alpha) + 127) / 255) as u8
                };
                case["expected"]["coverage"] = json!({
                    "color_image_address": 8192,
                    "width": 1,
                    "height": 1,
                    "encoding": "coverage_count_u4",
                    "contents": blob(&[output])
                });
                case["expected"]["framebuffer"] = json!({
                    "address": 8192,
                    "width": 1,
                    "height": 1,
                    "row_stride_bytes": 2,
                    "encoding": "rgba16_big_endian",
                    "contents": blob(&[0, 1])
                });
                case["expected"]["depth"] = json!({
                    "address": 12288,
                    "width": 1,
                    "height": 1,
                    "row_stride_bytes": 2,
                    "contents": blob(&[0xff, 0xff])
                });
                cases.push(case);
            }
        }
    }
    value["cases"] = Value::Array(cases);
    value
}

fn coverage_to_alpha_sweep() -> Value {
    let mut value = fixture();
    value["suite_id"] = json!("synthetic.coverage-to-alpha.threshold-sweep");
    let template = value["cases"][0].clone();
    let selected_alpha = [32u8, 64, 96, 128, 159, 191, 223, 255];
    let mut cases = Vec::with_capacity(4096);
    for cycle_type in ["one_cycle", "two_cycle"] {
        for input_coverage in 1u8..=8 {
            for threshold in 0u8..=u8::MAX {
                let mut case = template.clone();
                case["case_id"] = json!(format!(
                    "coverage-alpha-{cycle_type}-{input_coverage}-{threshold:03}"
                ));
                case["description"] = json!(format!(
                    "Synthetic {cycle_type} coverage-to-alpha point for coverage {input_coverage}, threshold {threshold}; no silicon claim."
                ));
                case["capture_intent"] = json!({
                    "kind": "coverage_to_alpha_sweep",
                    "sweep_id": "coverage-alpha-v1",
                    "cycle_type": cycle_type,
                    "input_coverage": input_coverage,
                    "threshold_alpha": threshold,
                    "replay_from_reset": true,
                    "pass_rgba16_be": 0x7c01,
                    "reject_rgba16_be": 0x0001
                });
                let passed = threshold <= selected_alpha[usize::from(input_coverage - 1)];
                case["expected"]["framebuffer"] = json!({
                    "address": 8192,
                    "width": 1,
                    "height": 1,
                    "row_stride_bytes": 2,
                    "encoding": "rgba16_big_endian",
                    "contents": blob(if passed { &[0x7c, 0x01] } else { &[0x00, 0x01] })
                });
                case["expected"]["depth"] = json!({
                    "address": 12288,
                    "width": 1,
                    "height": 1,
                    "row_stride_bytes": 2,
                    "contents": blob(&[0xff, 0xff])
                });
                case["expected"]["coverage"] = json!({
                    "color_image_address": 8192,
                    "width": 1,
                    "height": 1,
                    "encoding": "coverage_count_u4",
                    "contents": blob(&[input_coverage])
                });
                cases.push(case);
            }
        }
    }
    value["cases"] = Value::Array(cases);
    value
}

fn zmode_inter_sweep() -> Value {
    let mut value = fixture();
    value["suite_id"] = json!("synthetic.zmode-inter.coverage-sweep");
    let template = value["cases"][0].clone();
    let mut cases = Vec::with_capacity(384);
    for cycle_type in ["one_cycle", "two_cycle"] {
        for (relation, incoming_z, memory_z, incoming_dz, memory_dz) in [
            ("in_front_control", 0x08000, 0x10000, 0x0100, 0x0200),
            ("interpenetrating", 0x10020, 0x10000, 0x0200, 0x0200),
            ("behind_control", 0x18000, 0x10000, 0x0300, 0x0200),
        ] {
            for incoming_coverage in 1u8..=8 {
                for initial_stored_coverage in 0u8..=7 {
                    let mut case = template.clone();
                    case["case_id"] = json!(format!(
                        "zmode-inter-{cycle_type}-{relation}-{incoming_coverage}-{initial_stored_coverage}"
                    ));
                    case["description"] = json!(format!(
                        "Synthetic {cycle_type} {relation} ZMODE_INTER point; no silicon claim."
                    ));
                    case["capture_intent"] = json!({
                        "kind": "z_mode_inter_coverage_sweep",
                        "sweep_id": "inter-v1",
                        "cycle_type": cycle_type,
                        "relation": relation,
                        "incoming_coverage": incoming_coverage,
                        "initial_stored_coverage": initial_stored_coverage,
                        "replay_from_reset": true,
                        "pass_rgba16_be": 0x7c01,
                        "reject_rgba16_be": 0x0001,
                        "incoming_z_u18": incoming_z,
                        "memory_z_u18": memory_z,
                        "incoming_delta_z_u16": incoming_dz,
                        "memory_delta_z_u16": memory_dz
                    });
                    let (admitted, stored_coverage) = match relation {
                        "in_front_control" => (true, incoming_coverage - 1),
                        "interpenetrating" => (
                            initial_stored_coverage < incoming_coverage - 1,
                            (initial_stored_coverage + 1) & 7,
                        ),
                        "behind_control" => (false, initial_stored_coverage),
                        _ => unreachable!(),
                    };
                    case["expected"]["framebuffer"] = json!({
                        "address": 8192,
                        "width": 1,
                        "height": 1,
                        "row_stride_bytes": 2,
                        "encoding": "rgba16_big_endian",
                        "contents": blob(if admitted { &[0x7c, 0x01] } else { &[0x00, 0x01] })
                    });
                    case["expected"]["depth"] = json!({
                        "address": 12288,
                        "width": 1,
                        "height": 1,
                        "row_stride_bytes": 2,
                        "contents": blob(&[0xff, 0xff])
                    });
                    case["expected"]["coverage"] = json!({
                        "color_image_address": 8192,
                        "width": 1,
                        "height": 1,
                        "encoding": "stored_coverage_u3",
                        "contents": blob(&[stored_coverage])
                    });
                    cases.push(case);
                }
            }
        }
    }
    value["cases"] = Value::Array(cases);
    value
}

fn representative_sample_sweep() -> Value {
    let mut value = fixture();
    value["suite_id"] = json!("synthetic.representative-sample.selector-sweep");
    let template = value["cases"][0].clone();
    let shade_markers = [
        0x1100_00ffu32,
        0x2200_00ff,
        0x3300_00ff,
        0x4400_00ff,
        0x5500_00ff,
        0x6600_00ff,
        0x7700_00ff,
        0x8800_00ff,
    ];
    let texture_markers = [
        0x0011_00ffu32,
        0x0022_00ff,
        0x0033_00ff,
        0x0044_00ff,
        0x0055_00ff,
        0x0066_00ff,
        0x0077_00ff,
        0x0088_00ff,
    ];
    let depth_markers = [
        0x1001u16, 0x1002, 0x1003, 0x1004, 0x1005, 0x1006, 0x1007, 0x1008,
    ];
    let color_control = 0x0102_03ffu32;
    let depth_control = 0x7fffu16;
    let controls = json!({
        "pixel_x": 41,
        "pixel_y": 73,
        "markers": {
            "shade_rgba32_be": shade_markers,
            "texture_rgba32_be": texture_markers,
            "depth_u16_be": depth_markers,
            "depth_observable_color_control_rgba32_be": color_control,
            "color_observable_depth_control_u16_be": depth_control
        }
    });
    let mut cases = Vec::with_capacity(1_530);
    for cycle_type in ["one_cycle", "two_cycle"] {
        for observable in ["shade", "texture", "depth"] {
            for coverage_mask in 1u16..=255 {
                let coverage_mask = coverage_mask as u8;
                let selected = coverage_mask.trailing_zeros() as usize;
                let mut case = template.clone();
                case["case_id"] = json!(format!(
                    "representative-{cycle_type}-{observable}-{coverage_mask:02x}"
                ));
                case["description"] = json!(format!(
                    "Synthetic {cycle_type} {observable} representative-sample mask 0x{coverage_mask:02x}; no silicon claim."
                ));
                case["capture_intent"] = json!({
                    "kind": "representative_sample_selector_sweep",
                    "sweep_id": "selector-v1",
                    "cycle_type": cycle_type,
                    "observable": observable,
                    "coverage_mask_u8": coverage_mask,
                    "replay_from_reset": true,
                    "controls": controls
                });
                let framebuffer_word = match observable {
                    "shade" => shade_markers[selected],
                    "texture" => texture_markers[selected],
                    "depth" => color_control,
                    _ => unreachable!(),
                };
                let depth_word = if observable == "depth" {
                    depth_markers[selected]
                } else {
                    depth_control
                };
                case["expected"]["framebuffer"] = json!({
                    "address": 8192,
                    "width": 1,
                    "height": 1,
                    "row_stride_bytes": 4,
                    "encoding": "rgba32_big_endian",
                    "contents": blob(&framebuffer_word.to_be_bytes())
                });
                case["expected"]["depth"] = json!({
                    "address": 12288,
                    "width": 1,
                    "height": 1,
                    "row_stride_bytes": 2,
                    "contents": blob(&depth_word.to_be_bytes())
                });
                case["expected"]["coverage"] = json!({
                    "color_image_address": 8192,
                    "width": 1,
                    "height": 1,
                    "encoding": "coverage_count_u4",
                    "contents": blob(&[coverage_mask.count_ones() as u8])
                });
                cases.push(case);
            }
        }
    }
    value["cases"] = Value::Array(cases);
    value
}

fn narrow_edge_coverage_sweep() -> Value {
    let mut value = fixture();
    value["suite_id"] = json!("synthetic.coverage.narrow-edge-correction");
    let template = value["cases"][0].clone();
    let shade_markers = [
        0x1100_00ffu32,
        0x2200_00ff,
        0x3300_00ff,
        0x4400_00ff,
        0x5500_00ff,
        0x6600_00ff,
        0x7700_00ff,
        0x8800_00ff,
    ];
    let texture_markers = [
        0x0011_00ffu32,
        0x0022_00ff,
        0x0033_00ff,
        0x0044_00ff,
        0x0055_00ff,
        0x0066_00ff,
        0x0077_00ff,
        0x0088_00ff,
    ];
    let depth_markers = [
        0x1001u16, 0x1002, 0x1003, 0x1004, 0x1005, 0x1006, 0x1007, 0x1008,
    ];
    let color_control = 0x0102_03ffu32;
    let depth_control = 0x7fffu16;
    let selected_boundaries = [-65_536i64, 65_536i64];
    let controls = json!({
        "pixel_x": 41,
        "pixel_y": 73,
        "edge_fractional_bits_u8": 16,
        "selected_boundaries_i64": selected_boundaries,
        "markers": {
            "shade_rgba32_be": shade_markers,
            "texture_rgba32_be": texture_markers,
            "depth_u16_be": depth_markers,
            "depth_observable_color_control_rgba32_be": color_control,
            "color_observable_depth_control_u16_be": depth_control
        }
    });
    let mut cases = Vec::with_capacity(36);
    for (boundary_index, boundary) in selected_boundaries.into_iter().enumerate() {
        for (cycle_index, cycle_type) in ["one_cycle", "two_cycle"].into_iter().enumerate() {
            for (position_index, (boundary_position, offset)) in
                [("below", -1i64), ("on", 0), ("above", 1)]
                    .into_iter()
                    .enumerate()
            {
                let sample = boundary_index * 4;
                let coverage_mask = ([1u8, 3, 7][position_index]) << sample;
                let coverage_count = coverage_mask.count_ones() as u8;
                for (observable_index, observable) in
                    ["shade", "texture", "depth"].into_iter().enumerate()
                {
                    let mut case = template.clone();
                    case["case_id"] = json!(format!(
                        "narrow-edge-{boundary_index}-{cycle_type}-{boundary_position}-{observable}"
                    ));
                    case["description"] = json!(format!(
                        "Synthetic narrow-edge boundary {boundary} {cycle_type} {boundary_position} {observable}; no silicon claim."
                    ));
                    case["capture_intent"] = json!({
                        "kind": "narrow_edge_coverage_correction_sweep",
                        "sweep_id": "narrow-edge-v1",
                        "cycle_type": cycle_type,
                        "observable": observable,
                        "boundary_position": boundary_position,
                        "replay_from_reset": true,
                        "controls": controls,
                        "edge_boundary_i64": boundary,
                        "edge_accumulator_i64": boundary + offset,
                        "coverage_mask_u8": coverage_mask,
                        "coverage_count_u4": coverage_count
                    });
                    let command = [
                        0xe9,
                        boundary_index as u8,
                        cycle_index as u8,
                        position_index as u8,
                        observable_index as u8,
                        0,
                        0,
                        0,
                    ];
                    case["command_bytes"] = blob(&command);
                    let framebuffer_word = match observable {
                        "shade" => shade_markers[sample],
                        "texture" => texture_markers[sample],
                        "depth" => color_control,
                        _ => unreachable!(),
                    };
                    let depth_word = if observable == "depth" {
                        depth_markers[sample]
                    } else {
                        depth_control
                    };
                    case["expected"]["framebuffer"] = json!({
                        "address": 8192,
                        "width": 1,
                        "height": 1,
                        "row_stride_bytes": 4,
                        "encoding": "rgba32_big_endian",
                        "contents": blob(&framebuffer_word.to_be_bytes())
                    });
                    case["expected"]["depth"] = json!({
                        "address": 12288,
                        "width": 1,
                        "height": 1,
                        "row_stride_bytes": 2,
                        "contents": blob(&depth_word.to_be_bytes())
                    });
                    case["expected"]["coverage"] = json!({
                        "color_image_address": 8192,
                        "width": 1,
                        "height": 1,
                        "encoding": "coverage_count_u4",
                        "contents": blob(&[coverage_count])
                    });
                    cases.push(case);
                }
            }
        }
    }
    value["cases"] = Value::Array(cases);
    value
}

fn texture_filter_tie_sweep() -> Value {
    let mut value = fixture();
    value["suite_id"] = json!("synthetic.texture-filter.three-nearest-tie");
    let template = value["cases"][0].clone();
    let texels = [0x0843u16, 0x420f, 0x841f, 0xc63f];
    let texture_bytes = texels
        .iter()
        .flat_map(|texel| texel.to_be_bytes())
        .collect::<Vec<_>>();
    let mut cases = Vec::with_capacity(6);
    for (cycle_index, cycle_type) in ["one_cycle", "two_cycle"].into_iter().enumerate() {
        for (position_index, (boundary_position, t_fraction)) in
            [("below", 15u8), ("on", 16), ("above", 17)]
                .into_iter()
                .enumerate()
        {
            let mut case = template.clone();
            case["case_id"] = json!(format!("texture-filter-{cycle_type}-{boundary_position}"));
            case["description"] = json!(format!(
                "Synthetic {cycle_type} three-nearest {boundary_position} tie point; no silicon claim."
            ));
            case["capture_intent"] = json!({
                "kind": "texture_filter_tie_sweep",
                "sweep_id": "three-nearest-diagonal-v1",
                "cycle_type": cycle_type,
                "boundary_position": boundary_position,
                "replay_from_reset": true,
                "sample_x": 23,
                "sample_y": 47,
                "texture_address": 0x4000,
                "texel_rgba16_be": texels,
                "s_texel_i10": -3,
                "t_texel_i10": 7,
                "s_fraction_u5": 16,
                "t_fraction_u5": t_fraction,
                "diagonal_boundary_u6": 32
            });
            let mut command = [0xe9, 0, 0, 0, 0, 0, 0, 0];
            command[1] = cycle_index as u8;
            command[2] = position_index as u8;
            case["command_bytes"] = blob(&command);
            case["setup"]["initial_memory"] = json!([{
                "region_id": "texture-2x2-rgba16",
                "role": "texture",
                "address": 0x4000,
                "contents": blob(&texture_bytes)
            }]);
            let output = [
                [0x1020_30ffu32, 0x4050_60ff, 0x7080_90ff],
                [0x1020_30ffu32, 0x4150_60ff, 0x7080_90ff],
            ][cycle_index][position_index];
            let depth = 0x1200u16 + (cycle_index as u16) * 0x10 + position_index as u16;
            case["expected"]["framebuffer"] = json!({
                "address": 8192,
                "width": 1,
                "height": 1,
                "row_stride_bytes": 4,
                "encoding": "rgba32_big_endian",
                "contents": blob(&output.to_be_bytes())
            });
            case["expected"]["depth"] = json!({
                "address": 12288,
                "width": 1,
                "height": 1,
                "row_stride_bytes": 2,
                "contents": blob(&depth.to_be_bytes())
            });
            case["expected"]["coverage"] = json!({
                "color_image_address": 8192,
                "width": 1,
                "height": 1,
                "encoding": "stored_coverage_u3",
                "contents": blob(&[(position_index + 2) as u8])
            });
            cases.push(case);
        }
    }
    value["cases"] = Value::Array(cases);
    value
}

fn reciprocal_s10_5_boundary_sweep() -> Value {
    let mut value = fixture();
    value["suite_id"] = json!("synthetic.texture.reciprocal-s10-5-boundary");
    let template = value["cases"][0].clone();
    let boundary = 64i16;
    let denominator = 32u64;
    let points = [
        ("below", 2047i64, 63i16, 0x1020_30ffu32),
        ("on", 2048i64, 64i16, 0x4050_60ffu32),
        ("above", 2049i64, 64i16, 0x4050_60ffu32),
    ];
    let mut cases = Vec::with_capacity(6);
    for (cycle_index, cycle_type) in ["one_cycle", "two_cycle"].into_iter().enumerate() {
        for (position_index, (boundary_position, numerator, expected_output, marker)) in
            points.into_iter().enumerate()
        {
            let mut case = template.clone();
            case["case_id"] = json!(format!("reciprocal-s10-5-{cycle_type}-{boundary_position}"));
            case["description"] = json!(format!(
                "Synthetic {cycle_type} reciprocal-S10.5 {boundary_position} point; no silicon claim."
            ));
            case["capture_intent"] = json!({
                "kind": "reciprocal_s10_5_boundary_sweep",
                "sweep_id": "reciprocal-grid-v1",
                "cycle_type": cycle_type,
                "boundary_position": boundary_position,
                "replay_from_reset": true,
                "sample_x": 19,
                "sample_y": 37,
                "boundary_s10_5_i16": boundary,
                "perspective_numerator_i64": numerator,
                "perspective_denominator_u64": denominator,
                "producer_expected_output_s10_5_i16": expected_output,
                "producer_expected_framebuffer_rgba32_be": marker,
                "depth_control_u16_be": 0x7fff,
                "stored_coverage_control_u3": 7
            });
            let mut command = [0xe9, 0, 0, 0, 0, 0, 0, 0];
            command[1] = cycle_index as u8;
            command[2] = position_index as u8;
            case["command_bytes"] = blob(&command);
            let observed = if cycle_index == 1 && position_index == 2 {
                0xabcd_efffu32
            } else {
                marker
            };
            case["expected"]["framebuffer"] = json!({
                "address": 8192,
                "width": 1,
                "height": 1,
                "row_stride_bytes": 4,
                "encoding": "rgba32_big_endian",
                "contents": blob(&observed.to_be_bytes())
            });
            case["expected"]["depth"] = json!({
                "address": 12288,
                "width": 1,
                "height": 1,
                "row_stride_bytes": 2,
                "contents": blob(&0x7fffu16.to_be_bytes())
            });
            case["expected"]["coverage"] = json!({
                "color_image_address": 8192,
                "width": 1,
                "height": 1,
                "encoding": "stored_coverage_u3",
                "contents": blob(&[7])
            });
            cases.push(case);
        }
    }
    value["cases"] = Value::Array(cases);
    value
}

fn average_filter_output_tie_sweep() -> Value {
    let mut value = fixture();
    value["suite_id"] = json!("synthetic.texture.average-filter-output-tie");
    let template = value["cases"][0].clone();
    let texels = [0x0843u16, 0x420f, 0x841f, 0xc63f];
    let texture_bytes = texels
        .iter()
        .flat_map(|texel| texel.to_be_bytes())
        .collect::<Vec<_>>();
    let points = [
        ("below", 15u8, 16u8, 509i64, 127u8, 0x1020_30ffu32),
        ("on", 16u8, 16u8, 510i64, 128u8, 0x4050_60ffu32),
        ("above", 17u8, 16u8, 511i64, 128u8, 0x4050_60ffu32),
    ];
    let mut cases = Vec::with_capacity(6);
    for (cycle_index, cycle_type) in ["one_cycle", "two_cycle"].into_iter().enumerate() {
        for (
            position_index,
            (tie_position, s_fraction, t_fraction, numerator, expected_output, marker),
        ) in points.into_iter().enumerate()
        {
            let mut case = template.clone();
            case["case_id"] = json!(format!("average-filter-{cycle_type}-{tie_position}"));
            case["description"] = json!(format!(
                "Synthetic {cycle_type} average-filter {tie_position} output tie; no silicon claim."
            ));
            case["capture_intent"] = json!({
                "kind": "average_filter_output_tie_sweep",
                "sweep_id": "average-red-tie-v1",
                "cycle_type": cycle_type,
                "tie_position": tie_position,
                "replay_from_reset": true,
                "sample_x": 29,
                "sample_y": 43,
                "texture_address": 0x4000,
                "texel_rgba16_be": texels,
                "s_texel_i10": -2,
                "t_texel_i10": 5,
                "s_fraction_u5": s_fraction,
                "t_fraction_u5": t_fraction,
                "isolated_channel": "red",
                "accumulator_numerator_i64": numerator,
                "accumulator_denominator_u64": 4,
                "tie_numerator_i64": 510,
                "producer_expected_output_u8": expected_output,
                "producer_expected_framebuffer_rgba32_be": marker,
                "depth_control_u16_be": 0x6fff,
                "stored_coverage_control_u3": 6
            });
            let mut command = [0xe9, 0, 0, 0, 0, 0, 0, 0];
            command[1] = cycle_index as u8;
            command[2] = position_index as u8;
            case["command_bytes"] = blob(&command);
            case["setup"]["initial_memory"] = json!([{
                "region_id": "average-filter-2x2-rgba16",
                "role": "texture",
                "address": 0x4000,
                "contents": blob(&texture_bytes)
            }]);
            let observed = if cycle_index == 1 && position_index == 2 {
                0xabcd_efffu32
            } else {
                marker
            };
            case["expected"]["framebuffer"] = json!({
                "address": 8192,
                "width": 1,
                "height": 1,
                "row_stride_bytes": 4,
                "encoding": "rgba32_big_endian",
                "contents": blob(&observed.to_be_bytes())
            });
            case["expected"]["depth"] = json!({
                "address": 12288,
                "width": 1,
                "height": 1,
                "row_stride_bytes": 2,
                "contents": blob(&0x6fffu16.to_be_bytes())
            });
            case["expected"]["coverage"] = json!({
                "color_image_address": 8192,
                "width": 1,
                "height": 1,
                "encoding": "stored_coverage_u3",
                "contents": blob(&[6])
            });
            cases.push(case);
        }
    }
    value["cases"] = Value::Array(cases);
    value
}

fn texture_lod_boundary_sweep() -> Value {
    let mut value = fixture();
    value["suite_id"] = json!("synthetic.texture.lod-derivative-boundary");
    let template = value["cases"][0].clone();
    let positions = [
        ("below", 95i16, 31i32, 31i64),
        ("on", 96i16, 32i32, 32i64),
        ("above", 97i16, 33i32, 33i64),
    ];
    let modes = [
        (
            "mip",
            [
                (2u8, 2u8, 0i16, 0x1020_30ffu32),
                (2, 3, 0, 0x4050_60ff),
                (2, 3, 1, 0x7080_90ff),
            ],
        ),
        (
            "detail",
            [
                (3u8, 4u8, 255i16, 0x1122_33ffu32),
                (3, 4, 0, 0x4455_66ff),
                (3, 4, 1, 0x7788_99ff),
            ],
        ),
        (
            "sharpen",
            [
                (2u8, 3u8, -1i16, 0xaabb_ccffu32),
                (2, 3, 0, 0x4050_60ff),
                (2, 3, 1, 0x7080_90ff),
            ],
        ),
    ];
    let mut cases = Vec::with_capacity(18);
    for (cycle_index, cycle_type) in ["one_cycle", "two_cycle"].into_iter().enumerate() {
        for (mode_index, (lod_mode, expectations)) in modes.into_iter().enumerate() {
            for (position_index, (boundary_position, x_neighbor_s, dsdx, metric_numerator)) in
                positions.into_iter().enumerate()
            {
                let (expected_tile0, expected_tile1, expected_fraction, marker) =
                    expectations[position_index];
                let mut case = template.clone();
                case["case_id"] = json!(format!(
                    "texture-lod-{lod_mode}-{cycle_type}-{boundary_position}"
                ));
                case["description"] = json!(format!(
                    "Synthetic {lod_mode} {cycle_type} texture-LOD {boundary_position} point; no silicon claim."
                ));
                case["capture_intent"] = json!({
                    "kind": "texture_lod_boundary_sweep",
                    "sweep_id": "lod-boundary-v1",
                    "cycle_type": cycle_type,
                    "lod_mode": lod_mode,
                    "boundary_position": boundary_position,
                    "replay_from_reset": true,
                    "sample_x": 31,
                    "sample_y": 47,
                    "center_s_s10_5_i16": 64,
                    "center_t_s10_5_i16": 96,
                    "x_neighbor_s_s10_5_i16": x_neighbor_s,
                    "x_neighbor_t_s10_5_i16": 96,
                    "y_neighbor_s_s10_5_i16": 64,
                    "y_neighbor_t_s10_5_i16": 96,
                    "dsdx_s10_5_i32": dsdx,
                    "dtdx_s10_5_i32": 0,
                    "dsdy_s10_5_i32": 0,
                    "dtdy_s10_5_i32": 0,
                    "lod_metric_numerator_i64": metric_numerator,
                    "lod_metric_denominator_u64": 32,
                    "lod_boundary_numerator_i64": 32,
                    "primitive_tile_u3": 2,
                    "max_mip_level_u3": 4,
                    "min_lod_u8": 32,
                    "producer_expected_tile0_u3": expected_tile0,
                    "producer_expected_tile1_u3": expected_tile1,
                    "producer_expected_lod_fraction_s9_8_i16": expected_fraction,
                    "producer_expected_framebuffer_rgba32_be": marker,
                    "depth_control_u16_be": 0x5fff,
                    "stored_coverage_control_u3": 5
                });
                let mut command = [0xe9, 0, 0, 0, 0, 0, 0, 0];
                command[1] = cycle_index as u8;
                command[2] = mode_index as u8;
                command[3] = position_index as u8;
                case["command_bytes"] = blob(&command);
                let observed = if cycle_index == 1 && mode_index == 2 && position_index == 2 {
                    0xabcd_efffu32
                } else {
                    marker
                };
                case["expected"]["framebuffer"] = json!({
                    "address": 8192,
                    "width": 1,
                    "height": 1,
                    "row_stride_bytes": 4,
                    "encoding": "rgba32_big_endian",
                    "contents": blob(&observed.to_be_bytes())
                });
                case["expected"]["depth"] = json!({
                    "address": 12288,
                    "width": 1,
                    "height": 1,
                    "row_stride_bytes": 2,
                    "contents": blob(&0x5fffu16.to_be_bytes())
                });
                case["expected"]["coverage"] = json!({
                    "color_image_address": 8192,
                    "width": 1,
                    "height": 1,
                    "encoding": "stored_coverage_u3",
                    "contents": blob(&[5])
                });
                cases.push(case);
            }
        }
    }
    value["cases"] = Value::Array(cases);
    value
}

fn blender_precision_sweep() -> Value {
    let mut value = fixture();
    value["suite_id"] = json!("synthetic.blender.precision-memory-feedback");
    let template = value["cases"][0].clone();
    let modes = ["ordinary", "force_blend", "fog_pass"];
    let cycles = ["one_cycle", "two_cycle"];
    let alphas = [0u8, 1, 30, 31];
    let positions = [("below", 30u8), ("on", 31), ("above", 32)];
    let mut cases = Vec::with_capacity(75);
    for (mode_index, mode) in modes.into_iter().enumerate() {
        for (cycle_index, cycle_type) in cycles.into_iter().enumerate() {
            for alpha in alphas {
                for (position_index, (denominator_position, denominator)) in
                    positions.into_iter().enumerate()
                {
                    let mut case = template.clone();
                    case["case_id"] = json!(format!(
                        "blender-precision-{mode}-{cycle_type}-{alpha:02}-{denominator_position}"
                    ));
                    case["description"] = json!(format!(
                        "Synthetic {mode} {cycle_type} blender alpha {alpha} denominator {denominator_position} point; no silicon claim."
                    ));
                    let marker = 0x1000_00ffu32
                        | ((mode_index as u32) << 28)
                        | ((u32::from(alpha)) << 16)
                        | ((position_index as u32) << 8);
                    case["capture_intent"] = json!({
                        "kind": "blender_precision_boundary_sweep",
                        "sweep_id": "blender-precision-v1",
                        "cycle_type": cycle_type,
                        "mode": mode,
                        "isolated_alpha_u5": alpha,
                        "denominator_position": denominator_position,
                        "replay_from_reset": true,
                        "sample_x": 40,
                        "sample_y": 24,
                        "denominator_boundary_u6": 31,
                        "producer_declared_denominator_u6": denominator,
                        "pixel_color_rgba32_be": 0x102030ffu32,
                        "memory_color_rgba32_be": 0x405060ffu32,
                        "fog_color_rgba32_be": 0x708090ffu32,
                        "producer_expected_framebuffer_rgba32_be": marker,
                        "depth_control_u16_be": 0x6aaau16,
                        "stored_coverage_control_u3": 4
                    });
                    let mut command = [0xe9, 0, 0, 0, 0, 0, 0, 0];
                    command[1] = mode_index as u8;
                    command[2] = cycle_index as u8;
                    command[3] = alpha;
                    command[4] = position_index as u8;
                    case["command_bytes"] = blob(&command);
                    let observed = if mode_index == 2
                        && cycle_index == 1
                        && alpha == 31
                        && position_index == 2
                    {
                        0xdead_beffu32
                    } else {
                        marker
                    };
                    case["expected"]["framebuffer"] = json!({
                        "address": 8192,
                        "width": 1,
                        "height": 1,
                        "row_stride_bytes": 4,
                        "encoding": "rgba32_big_endian",
                        "contents": blob(&observed.to_be_bytes())
                    });
                    case["expected"]["depth"] = json!({
                        "address": 12288,
                        "width": 1,
                        "height": 1,
                        "row_stride_bytes": 2,
                        "contents": blob(&0x6aaau16.to_be_bytes())
                    });
                    case["expected"]["coverage"] = json!({
                        "color_image_address": 8192,
                        "width": 1,
                        "height": 1,
                        "encoding": "stored_coverage_u3",
                        "contents": blob(&[4])
                    });
                    cases.push(case);
                }
            }
        }
    }
    for (mode_index, mode) in modes.into_iter().enumerate() {
        let mut case = template.clone();
        case["case_id"] = json!(format!("blender-feedback-{mode}"));
        case["description"] = json!(format!(
            "Synthetic {mode} two-adjacent-pixel feedback sequence; no silicon claim."
        ));
        let command = [0xe9, mode_index as u8, 0x2a, 0x2b, 0, 0, 0, 0];
        case["command_bytes"] = blob(&command);
        case["capture_intent"] = json!({
            "kind": "blender_memory_feedback_pair",
            "sweep_id": "blender-precision-v1",
            "mode": mode,
            "cycle_type": "two_cycle",
            "replay_from_reset": true,
            "first_pixel_x": 40,
            "first_pixel_y": 24,
            "second_pixel_x": 41,
            "second_pixel_y": 24,
            "ordered_pair_command_sha256": digest(&command),
            "cycle_one_handoff_color_rgba32_be": 0x112233ffu32,
            "prior_memory_color_rgba32_be": 0x445566ffu32,
            "cycle_one_handoff_coverage_u3": 6,
            "prior_memory_coverage_u3": 2
        });
        let first = 0xa000_00ffu32 | ((mode_index as u32) << 16);
        let (second, second_coverage) = match mode_index {
            0 => (0x1122_33ffu32, 6u8),
            1 => (0x4455_66ffu32, 2u8),
            _ => (0x7788_99ffu32, 4u8),
        };
        let mut framebuffer = Vec::new();
        framebuffer.extend_from_slice(&first.to_be_bytes());
        framebuffer.extend_from_slice(&second.to_be_bytes());
        case["expected"]["framebuffer"] = json!({
            "address": 8192,
            "width": 2,
            "height": 1,
            "row_stride_bytes": 8,
            "encoding": "rgba32_big_endian",
            "contents": blob(&framebuffer)
        });
        case["expected"]["depth"] = json!({
            "address": 12288,
            "width": 2,
            "height": 1,
            "row_stride_bytes": 4,
            "contents": blob(&[0x60, mode_index as u8, 0x61, mode_index as u8])
        });
        case["expected"]["coverage"] = json!({
            "color_image_address": 8192,
            "width": 2,
            "height": 1,
            "encoding": "stored_coverage_u3",
            "contents": blob(&[3, second_coverage])
        });
        cases.push(case);
    }
    value["cases"] = Value::Array(cases);
    value
}

#[test]
fn accepts_complete_synthetic_bundle_deterministically() {
    let bytes = serde_json::to_vec(&fixture()).unwrap();
    let first = validate_json(&bytes).unwrap();
    let second = validate_json(&bytes).unwrap();
    assert_eq!(first.canonical_sha256(), second.canonical_sha256());
    assert_eq!(first.bundle().cases.len(), 1);
}

#[test]
fn blender_precision_analysis_is_complete_deterministic_and_preserves_divergence() {
    let value = blender_precision_sweep();
    let bundle = validate_json(&serde_json::to_vec(&value).unwrap()).unwrap();
    assert_eq!(bundle.bundle().cases.len(), 75);
    let first = analyze_blender_precision_sweep(&bundle, "blender-precision-v1").unwrap();
    let second = analyze_blender_precision_sweep(&bundle, "blender-precision-v1").unwrap();
    assert_eq!(first, second);
    assert_eq!(first.schema, "fn64.rdp-blender-precision-analysis.v1");
    assert_eq!(first.analysis_sha256.len(), 64);
    assert_eq!(
        first.producer_kind,
        fn64_rdp_silicon_vectors::ProducerKind::SyntheticFixture
    );
    assert!(!first.base_matrix_row_closed);
    assert_eq!(first.alpha_values_u5, [0, 1, 30, 31]);
    assert_eq!(first.denominator_boundary_u6, 31);
    assert_eq!(first.modes.len(), 3);
    assert_eq!(first.modes[0].mode, BlenderProbeMode::Ordinary);
    assert_eq!(first.modes[1].mode, BlenderProbeMode::ForceBlend);
    assert_eq!(first.modes[2].mode, BlenderProbeMode::FogPass);
    assert_eq!(first.modes[0].cycles[0].observations.len(), 12);
    assert_eq!(first.total_cycle_divergence_count, 1);
    assert!(!first.all_cycle_results_match);
    assert_eq!(first.unexpected_output_count, 1);
    assert_eq!(first.unexpected_depth_count, 0);
    assert_eq!(first.unexpected_coverage_count, 0);
    assert_eq!(first.feedback_pairs.len(), 3);
    assert!(first.feedback_pairs[0].second_color_matches_cycle_one_handoff);
    assert!(first.feedback_pairs[0].second_coverage_matches_cycle_one_handoff);
    assert!(first.feedback_pairs[1].second_color_matches_prior_memory);
    assert!(first.feedback_pairs[1].second_coverage_matches_prior_memory);
    assert!(!first.feedback_pairs[2].second_color_matches_cycle_one_handoff);
    assert!(!first.feedback_pairs[2].second_color_matches_prior_memory);
    assert_eq!(
        first.feedback_pairs[2].framebuffer_rgba32_be[1],
        0x7788_99ff
    );
    assert_eq!(first.feedback_pairs[2].depth_u16_be, [0x6002, 0x6102]);
    assert_eq!(first.feedback_pairs[2].stored_coverage_u3, [3, 4]);
}

#[test]
fn blender_precision_rejects_missing_duplicate_reset_and_control_drift() {
    let mut missing = blender_precision_sweep();
    missing["cases"].as_array_mut().unwrap().remove(0);
    let missing = validate_json(&serde_json::to_vec(&missing).unwrap()).unwrap();
    assert!(
        analyze_blender_precision_sweep(&missing, "blender-precision-v1")
            .unwrap_err()
            .to_string()
            .contains("exactly 72 precision points")
    );

    let mut duplicate = blender_precision_sweep();
    let mut repeated = duplicate["cases"][0].clone();
    repeated["case_id"] = json!("duplicate-blender-precision-point");
    duplicate["cases"].as_array_mut().unwrap().push(repeated);
    let duplicate = validate_json(&serde_json::to_vec(&duplicate).unwrap()).unwrap();
    assert!(
        analyze_blender_precision_sweep(&duplicate, "blender-precision-v1")
            .unwrap_err()
            .to_string()
            .contains("duplicate blender-precision point")
    );

    let mut reset = blender_precision_sweep();
    reset["cases"][0]["capture_intent"]["replay_from_reset"] = json!(false);
    let reset = validate_json(&serde_json::to_vec(&reset).unwrap()).unwrap();
    assert!(
        analyze_blender_precision_sweep(&reset, "blender-precision-v1")
            .unwrap_err()
            .to_string()
            .contains("must replay from reset")
    );

    let mut common = blender_precision_sweep();
    common["cases"][1]["capture_intent"]["pixel_color_rgba32_be"] = json!(0x010203ffu32);
    let common = validate_json(&serde_json::to_vec(&common).unwrap()).unwrap();
    assert!(
        analyze_blender_precision_sweep(&common, "blender-precision-v1")
            .unwrap_err()
            .to_string()
            .contains("differs within sweep")
    );

    let mut pair_drift = blender_precision_sweep();
    pair_drift["cases"][73]["capture_intent"]["first_pixel_x"] = json!(41);
    pair_drift["cases"][73]["capture_intent"]["second_pixel_x"] = json!(42);
    let pair_drift = validate_json(&serde_json::to_vec(&pair_drift).unwrap()).unwrap();
    assert!(
        analyze_blender_precision_sweep(&pair_drift, "blender-precision-v1")
            .unwrap_err()
            .to_string()
            .contains("ordered-pair geometry differs")
    );
}

#[test]
fn blender_precision_intent_rejects_invalid_boundaries_and_pair_provenance() {
    let mut alpha = blender_precision_sweep();
    alpha["cases"][0]["capture_intent"]["isolated_alpha_u5"] = json!(2);
    assert!(validate(&alpha)
        .unwrap_err()
        .contains("exact 5-bit extrema"));

    let mut denominator = blender_precision_sweep();
    denominator["cases"][0]["capture_intent"]["producer_declared_denominator_u6"] = json!(29);
    assert!(validate(&denominator)
        .unwrap_err()
        .contains("denominator must be exactly 30"));

    let mut digest_drift = blender_precision_sweep();
    digest_drift["cases"][72]["capture_intent"]["ordered_pair_command_sha256"] =
        json!("00".repeat(32));
    assert!(validate(&digest_drift)
        .unwrap_err()
        .contains("ordered pair digest must equal"));

    let mut nonadjacent = blender_precision_sweep();
    nonadjacent["cases"][72]["capture_intent"]["second_pixel_x"] = json!(42);
    assert!(validate(&nonadjacent)
        .unwrap_err()
        .contains("horizontally adjacent"));

    let mut wrong_cycle = blender_precision_sweep();
    wrong_cycle["cases"][72]["capture_intent"]["cycle_type"] = json!("one_cycle");
    assert!(validate(&wrong_cycle)
        .unwrap_err()
        .contains("must declare two_cycle"));

    let mut ambiguous = blender_precision_sweep();
    ambiguous["cases"][72]["capture_intent"]["prior_memory_color_rgba32_be"] =
        ambiguous["cases"][72]["capture_intent"]["cycle_one_handoff_color_rgba32_be"].clone();
    assert!(validate(&ambiguous)
        .unwrap_err()
        .contains("color markers must differ"));
}

#[test]
fn blender_precision_hash_binds_exact_raw_planes() {
    let value = blender_precision_sweep();
    let baseline_bundle = validate_json(&serde_json::to_vec(&value).unwrap()).unwrap();
    let baseline =
        analyze_blender_precision_sweep(&baseline_bundle, "blender-precision-v1").unwrap();

    let mut changed = value;
    changed["cases"][0]["expected"]["depth"]["contents"] = blob(&0x6aabu16.to_be_bytes());
    let changed_bundle = validate_json(&serde_json::to_vec(&changed).unwrap()).unwrap();
    let changed = analyze_blender_precision_sweep(&changed_bundle, "blender-precision-v1").unwrap();
    assert_ne!(changed.analysis_sha256, baseline.analysis_sha256);
    assert_eq!(changed.unexpected_depth_count, 1);
    assert_eq!(
        changed.modes[0].cycles[0].observations[0].depth_u16_be,
        0x6aab
    );
}

#[test]
fn texture_filter_tie_analysis_is_complete_deterministic_and_preserves_divergence() {
    let value = texture_filter_tie_sweep();
    let bundle = validate_json(&serde_json::to_vec(&value).unwrap()).unwrap();
    let first = analyze_texture_filter_tie_sweep(&bundle, "three-nearest-diagonal-v1").unwrap();
    let second = analyze_texture_filter_tie_sweep(&bundle, "three-nearest-diagonal-v1").unwrap();
    assert_eq!(first, second);
    assert_eq!(first.analysis_sha256.len(), 64);
    assert_eq!(first.diagonal_boundary_u6, 32);
    assert_eq!(first.s_texel_i10, -3);
    assert_eq!(first.t_texel_i10, 7);
    assert_eq!(first.cycles.len(), 2);
    assert_eq!(first.cycles[0].cycle_type, ProbeCycleType::OneCycle);
    assert_eq!(first.cycles[1].cycle_type, ProbeCycleType::TwoCycle);
    assert_eq!(first.cycles[0].observations.len(), 3);
    assert_eq!(
        first.cycles[0].observations[0].boundary_position,
        FilterTieBoundaryPosition::Below
    );
    assert_eq!(
        first.cycles[0].observations[1].boundary_position,
        FilterTieBoundaryPosition::On
    );
    assert_eq!(
        first.cycles[0].observations[2].boundary_position,
        FilterTieBoundaryPosition::Above
    );
    assert_eq!(
        first.cycles[0].observations[1].framebuffer_rgba32_be,
        0x4050_60ff
    );
    assert_eq!(
        first.cycles[1].observations[1].framebuffer_rgba32_be,
        0x4150_60ff
    );
    assert!(!first.cycle_results_match);
}

#[test]
fn texture_filter_tie_analysis_rejects_missing_duplicate_reset_and_control_drift() {
    let mut missing = texture_filter_tie_sweep();
    missing["cases"].as_array_mut().unwrap().remove(2);
    let missing = validate_json(&serde_json::to_vec(&missing).unwrap()).unwrap();
    assert!(
        analyze_texture_filter_tie_sweep(&missing, "three-nearest-diagonal-v1")
            .unwrap_err()
            .to_string()
            .contains("missing OneCycle Above point")
    );

    let mut duplicate = texture_filter_tie_sweep();
    let mut repeated = duplicate["cases"][0].clone();
    repeated["case_id"] = json!("duplicate-filter-point");
    duplicate["cases"].as_array_mut().unwrap().push(repeated);
    let duplicate = validate_json(&serde_json::to_vec(&duplicate).unwrap()).unwrap();
    assert!(
        analyze_texture_filter_tie_sweep(&duplicate, "three-nearest-diagonal-v1")
            .unwrap_err()
            .to_string()
            .contains("duplicate texture-filter-tie point")
    );

    let mut reset = texture_filter_tie_sweep();
    reset["cases"][0]["capture_intent"]["replay_from_reset"] = json!(false);
    let reset = validate_json(&serde_json::to_vec(&reset).unwrap()).unwrap();
    assert!(
        analyze_texture_filter_tie_sweep(&reset, "three-nearest-diagonal-v1")
            .unwrap_err()
            .to_string()
            .contains("replay from reset")
    );

    let mut controls = texture_filter_tie_sweep();
    controls["cases"][1]["capture_intent"]["sample_x"] = json!(24);
    let controls = validate_json(&serde_json::to_vec(&controls).unwrap()).unwrap();
    assert!(
        analyze_texture_filter_tie_sweep(&controls, "three-nearest-diagonal-v1")
            .unwrap_err()
            .to_string()
            .contains("differs within sweep")
    );

    let mut pair_drift = texture_filter_tie_sweep();
    pair_drift["cases"][3]["capture_intent"]["s_fraction_u5"] = json!(15);
    pair_drift["cases"][3]["capture_intent"]["t_fraction_u5"] = json!(16);
    let pair_drift = validate_json(&serde_json::to_vec(&pair_drift).unwrap()).unwrap();
    assert!(
        analyze_texture_filter_tie_sweep(&pair_drift, "three-nearest-diagonal-v1")
            .unwrap_err()
            .to_string()
            .contains("different Below fraction pairs across cycle modes")
    );
}

#[test]
fn texture_filter_tie_intent_rejects_false_boundary_and_texture_controls() {
    let mut boundary = texture_filter_tie_sweep();
    boundary["cases"][0]["capture_intent"]["t_fraction_u5"] = json!(14);
    assert!(validate(&boundary)
        .unwrap_err()
        .contains("Below fractions must sum to 31"));

    let mut fraction = texture_filter_tie_sweep();
    fraction["cases"][0]["capture_intent"]["s_fraction_u5"] = json!(32);
    assert!(validate(&fraction)
        .unwrap_err()
        .contains("fractions must fit unsigned 5-bit"));

    let mut texture = texture_filter_tie_sweep();
    texture["cases"][1]["setup"]["initial_memory"][0]["contents"] = blob(&[0; 8]);
    let texture = validate_json(&serde_json::to_vec(&texture).unwrap()).unwrap();
    assert!(
        analyze_texture_filter_tie_sweep(&texture, "three-nearest-diagonal-v1")
            .unwrap_err()
            .to_string()
            .contains("do not equal declared RGBA16 texels")
    );
}

#[test]
fn reciprocal_s10_5_analysis_is_complete_deterministic_and_preserves_divergence() {
    let value = reciprocal_s10_5_boundary_sweep();
    let bundle = validate_json(&serde_json::to_vec(&value).unwrap()).unwrap();
    let first = analyze_reciprocal_s10_5_boundary_sweep(&bundle, "reciprocal-grid-v1").unwrap();
    let second = analyze_reciprocal_s10_5_boundary_sweep(&bundle, "reciprocal-grid-v1").unwrap();
    assert_eq!(first, second);
    assert_eq!(first.analysis_sha256.len(), 64);
    assert_eq!(first.boundary_s10_5_i16, 64);
    assert_eq!(first.cycles.len(), 2);
    assert_eq!(first.cycles[0].cycle_type, ProbeCycleType::OneCycle);
    assert_eq!(first.cycles[1].cycle_type, ProbeCycleType::TwoCycle);
    assert_eq!(first.cycles[0].observations.len(), 3);
    assert_eq!(
        first.cycles[0].observations[0].boundary_position,
        ReciprocalBoundaryPosition::Below
    );
    assert_eq!(
        first.cycles[0].observations[0].observed_output_s10_5_i16,
        Some(63)
    );
    assert_eq!(
        first.cycles[0].observations[2].observed_output_s10_5_i16,
        Some(64)
    );
    assert_eq!(
        first.cycles[1].observations[2].observed_output_s10_5_i16,
        None
    );
    assert!(!first.cycles[1].observations[2].output_matches_producer_expectation);
    assert_eq!(first.unexpected_output_count, 1);
    assert!(!first.cycle_results_match);
}

#[test]
fn reciprocal_s10_5_analysis_rejects_missing_duplicate_reset_and_cross_cycle_coordinates() {
    let mut missing = reciprocal_s10_5_boundary_sweep();
    missing["cases"].as_array_mut().unwrap().remove(2);
    let missing = validate_json(&serde_json::to_vec(&missing).unwrap()).unwrap();
    assert!(
        analyze_reciprocal_s10_5_boundary_sweep(&missing, "reciprocal-grid-v1")
            .unwrap_err()
            .to_string()
            .contains("missing OneCycle Above point")
    );

    let mut duplicate = reciprocal_s10_5_boundary_sweep();
    let mut repeated = duplicate["cases"][0].clone();
    repeated["case_id"] = json!("duplicate-reciprocal-point");
    duplicate["cases"].as_array_mut().unwrap().push(repeated);
    let duplicate = validate_json(&serde_json::to_vec(&duplicate).unwrap()).unwrap();
    assert!(
        analyze_reciprocal_s10_5_boundary_sweep(&duplicate, "reciprocal-grid-v1")
            .unwrap_err()
            .to_string()
            .contains("duplicate reciprocal-S10.5 point")
    );

    let mut reset = reciprocal_s10_5_boundary_sweep();
    reset["cases"][0]["capture_intent"]["replay_from_reset"] = json!(false);
    let reset = validate_json(&serde_json::to_vec(&reset).unwrap()).unwrap();
    assert!(
        analyze_reciprocal_s10_5_boundary_sweep(&reset, "reciprocal-grid-v1")
            .unwrap_err()
            .to_string()
            .contains("replay from reset")
    );

    let mut coordinate_drift = reciprocal_s10_5_boundary_sweep();
    coordinate_drift["cases"][3]["capture_intent"]["perspective_numerator_i64"] = json!(2046);
    let coordinate_drift = validate_json(&serde_json::to_vec(&coordinate_drift).unwrap()).unwrap();
    assert!(
        analyze_reciprocal_s10_5_boundary_sweep(&coordinate_drift, "reciprocal-grid-v1")
            .unwrap_err()
            .to_string()
            .contains("different Below input/output controls across cycle modes")
    );
}

#[test]
fn reciprocal_s10_5_intent_rejects_false_relations_and_nonadjacent_controls() {
    let mut false_relation = reciprocal_s10_5_boundary_sweep();
    false_relation["cases"][0]["capture_intent"]["perspective_numerator_i64"] = json!(2048);
    assert!(validate(&false_relation)
        .unwrap_err()
        .contains("does not have the declared exact relation"));

    let mut zero_denominator = reciprocal_s10_5_boundary_sweep();
    zero_denominator["cases"][0]["capture_intent"]["perspective_denominator_u64"] = json!(0);
    assert!(validate(&zero_denominator)
        .unwrap_err()
        .contains("denominator must be nonzero"));

    let mut nonadjacent = reciprocal_s10_5_boundary_sweep();
    for index in [0usize, 3] {
        nonadjacent["cases"][index]["capture_intent"]["perspective_numerator_i64"] = json!(2046);
    }
    let nonadjacent = validate_json(&serde_json::to_vec(&nonadjacent).unwrap()).unwrap();
    assert!(
        analyze_reciprocal_s10_5_boundary_sweep(&nonadjacent, "reciprocal-grid-v1")
            .unwrap_err()
            .to_string()
            .contains("boundary-1/boundary/boundary+1 inputs")
    );

    let mut ambiguous_marker = reciprocal_s10_5_boundary_sweep();
    for index in [2usize, 5] {
        ambiguous_marker["cases"][index]["capture_intent"]["producer_expected_output_s10_5_i16"] =
            json!(65);
    }
    let ambiguous_marker = validate_json(&serde_json::to_vec(&ambiguous_marker).unwrap()).unwrap();
    assert!(
        analyze_reciprocal_s10_5_boundary_sweep(&ambiguous_marker, "reciprocal-grid-v1")
            .unwrap_err()
            .to_string()
            .contains("one output marker to different expected S10.5 values")
    );
}

#[test]
fn reciprocal_s10_5_analysis_rejects_geometry_and_output_control_drift() {
    let mut geometry = reciprocal_s10_5_boundary_sweep();
    geometry["cases"][1]["expected"]["framebuffer"]["row_stride_bytes"] = json!(8);
    geometry["cases"][1]["expected"]["framebuffer"]["contents"] = blob(&[0; 8]);
    let geometry = validate_json(&serde_json::to_vec(&geometry).unwrap()).unwrap();
    assert!(
        analyze_reciprocal_s10_5_boundary_sweep(&geometry, "reciprocal-grid-v1")
            .unwrap_err()
            .to_string()
            .contains("requires exact 1x1 RGBA32")
    );

    let mut depth = reciprocal_s10_5_boundary_sweep();
    depth["cases"][0]["expected"]["depth"]["contents"] = blob(&0x7ffeu16.to_be_bytes());
    let depth = validate_json(&serde_json::to_vec(&depth).unwrap()).unwrap();
    assert!(
        analyze_reciprocal_s10_5_boundary_sweep(&depth, "reciprocal-grid-v1")
            .unwrap_err()
            .to_string()
            .contains("depth or coverage output changed")
    );

    let mut common = reciprocal_s10_5_boundary_sweep();
    common["cases"][1]["capture_intent"]["sample_x"] = json!(20);
    let common = validate_json(&serde_json::to_vec(&common).unwrap()).unwrap();
    assert!(
        analyze_reciprocal_s10_5_boundary_sweep(&common, "reciprocal-grid-v1")
            .unwrap_err()
            .to_string()
            .contains("geometry differs within sweep")
    );
}

#[test]
fn average_filter_tie_analysis_is_complete_deterministic_and_preserves_divergence() {
    let value = average_filter_output_tie_sweep();
    let bundle = validate_json(&serde_json::to_vec(&value).unwrap()).unwrap();
    let first = analyze_average_filter_output_tie_sweep(&bundle, "average-red-tie-v1").unwrap();
    let second = analyze_average_filter_output_tie_sweep(&bundle, "average-red-tie-v1").unwrap();
    assert_eq!(first, second);
    assert_eq!(first.analysis_sha256.len(), 64);
    assert_eq!(first.isolated_channel, AverageFilterChannel::Red);
    assert_eq!(first.tie_numerator_i64, 510);
    assert_eq!(first.accumulator_denominator_u64, 4);
    assert_eq!(first.cycles.len(), 2);
    assert_eq!(first.cycles[0].cycle_type, ProbeCycleType::OneCycle);
    assert_eq!(first.cycles[1].cycle_type, ProbeCycleType::TwoCycle);
    assert_eq!(first.cycles[0].observations.len(), 3);
    assert_eq!(
        first.cycles[0].observations[0].tie_position,
        AverageFilterTiePosition::Below
    );
    assert_eq!(
        first.cycles[0].observations[0].observed_output_u8,
        Some(127)
    );
    assert_eq!(
        first.cycles[0].observations[2].observed_output_u8,
        Some(128)
    );
    assert_eq!(first.cycles[1].observations[2].observed_output_u8, None);
    assert!(!first.cycles[1].observations[2].output_matches_producer_expectation);
    assert_eq!(first.unexpected_output_count, 1);
    assert!(!first.cycle_results_match);
}

#[test]
fn average_filter_tie_rejects_missing_duplicate_reset_and_cross_cycle_drift() {
    let mut missing = average_filter_output_tie_sweep();
    missing["cases"].as_array_mut().unwrap().remove(2);
    let missing = validate_json(&serde_json::to_vec(&missing).unwrap()).unwrap();
    assert!(
        analyze_average_filter_output_tie_sweep(&missing, "average-red-tie-v1")
            .unwrap_err()
            .to_string()
            .contains("missing OneCycle Above point")
    );

    let mut duplicate = average_filter_output_tie_sweep();
    let mut repeated = duplicate["cases"][0].clone();
    repeated["case_id"] = json!("duplicate-average-filter-point");
    duplicate["cases"].as_array_mut().unwrap().push(repeated);
    let duplicate = validate_json(&serde_json::to_vec(&duplicate).unwrap()).unwrap();
    assert!(
        analyze_average_filter_output_tie_sweep(&duplicate, "average-red-tie-v1")
            .unwrap_err()
            .to_string()
            .contains("duplicate average-filter-tie point")
    );

    let mut reset = average_filter_output_tie_sweep();
    reset["cases"][0]["capture_intent"]["replay_from_reset"] = json!(false);
    let reset = validate_json(&serde_json::to_vec(&reset).unwrap()).unwrap();
    assert!(
        analyze_average_filter_output_tie_sweep(&reset, "average-red-tie-v1")
            .unwrap_err()
            .to_string()
            .contains("replay from reset")
    );

    let mut cross_cycle = average_filter_output_tie_sweep();
    cross_cycle["cases"][3]["capture_intent"]["s_fraction_u5"] = json!(14);
    let cross_cycle = validate_json(&serde_json::to_vec(&cross_cycle).unwrap()).unwrap();
    assert!(
        analyze_average_filter_output_tie_sweep(&cross_cycle, "average-red-tie-v1")
            .unwrap_err()
            .to_string()
            .contains("different Below coordinate/accumulator/output controls")
    );
}

#[test]
fn average_filter_tie_intent_rejects_false_relations_nonadjacency_and_ambiguity() {
    let mut false_relation = average_filter_output_tie_sweep();
    false_relation["cases"][0]["capture_intent"]["accumulator_numerator_i64"] = json!(510);
    assert!(validate(&false_relation)
        .unwrap_err()
        .contains("does not have the declared exact relation"));

    let mut denominator = average_filter_output_tie_sweep();
    denominator["cases"][0]["capture_intent"]["accumulator_denominator_u64"] = json!(0);
    assert!(validate(&denominator)
        .unwrap_err()
        .contains("denominator must be nonzero"));

    let mut nonadjacent = average_filter_output_tie_sweep();
    for index in [0usize, 3] {
        nonadjacent["cases"][index]["capture_intent"]["accumulator_numerator_i64"] = json!(508);
    }
    let nonadjacent = validate_json(&serde_json::to_vec(&nonadjacent).unwrap()).unwrap();
    assert!(
        analyze_average_filter_output_tie_sweep(&nonadjacent, "average-red-tie-v1")
            .unwrap_err()
            .to_string()
            .contains("tie-1/tie/tie+1 inputs")
    );

    let mut duplicate_coordinates = average_filter_output_tie_sweep();
    for index in [2usize, 5] {
        duplicate_coordinates["cases"][index]["capture_intent"]["s_fraction_u5"] = json!(16);
    }
    let duplicate_coordinates =
        validate_json(&serde_json::to_vec(&duplicate_coordinates).unwrap()).unwrap();
    assert!(
        analyze_average_filter_output_tie_sweep(&duplicate_coordinates, "average-red-tie-v1")
            .unwrap_err()
            .to_string()
            .contains("distinct below/on/above fractional coordinate pairs")
    );

    let mut ambiguous_marker = average_filter_output_tie_sweep();
    for index in [2usize, 5] {
        ambiguous_marker["cases"][index]["capture_intent"]["producer_expected_output_u8"] =
            json!(129);
    }
    let ambiguous_marker = validate_json(&serde_json::to_vec(&ambiguous_marker).unwrap()).unwrap();
    assert!(
        analyze_average_filter_output_tie_sweep(&ambiguous_marker, "average-red-tie-v1")
            .unwrap_err()
            .to_string()
            .contains("one output marker to different expected channel values")
    );
}

#[test]
fn average_filter_tie_rejects_texture_geometry_and_output_control_drift() {
    let mut texture = average_filter_output_tie_sweep();
    texture["cases"][0]["setup"]["initial_memory"][0]["contents"] = blob(&[0; 8]);
    let texture = validate_json(&serde_json::to_vec(&texture).unwrap()).unwrap();
    assert!(
        analyze_average_filter_output_tie_sweep(&texture, "average-red-tie-v1")
            .unwrap_err()
            .to_string()
            .contains("do not equal declared average-filter RGBA16 texels")
    );

    let mut geometry = average_filter_output_tie_sweep();
    geometry["cases"][1]["expected"]["framebuffer"]["row_stride_bytes"] = json!(8);
    geometry["cases"][1]["expected"]["framebuffer"]["contents"] = blob(&[0; 8]);
    let geometry = validate_json(&serde_json::to_vec(&geometry).unwrap()).unwrap();
    assert!(
        analyze_average_filter_output_tie_sweep(&geometry, "average-red-tie-v1")
            .unwrap_err()
            .to_string()
            .contains("requires exact 1x1 RGBA32")
    );

    let mut coverage = average_filter_output_tie_sweep();
    coverage["cases"][0]["expected"]["coverage"]["contents"] = blob(&[5]);
    let coverage = validate_json(&serde_json::to_vec(&coverage).unwrap()).unwrap();
    assert!(
        analyze_average_filter_output_tie_sweep(&coverage, "average-red-tie-v1")
            .unwrap_err()
            .to_string()
            .contains("depth or coverage output changed")
    );

    let mut setup = average_filter_output_tie_sweep();
    setup["cases"][1]["setup"]["registers"][2]["value"] = json!(1);
    let setup = validate_json(&serde_json::to_vec(&setup).unwrap()).unwrap();
    assert!(
        analyze_average_filter_output_tie_sweep(&setup, "average-red-tie-v1")
            .unwrap_err()
            .to_string()
            .contains("differs within sweep")
    );
}

#[test]
fn texture_lod_boundary_analysis_is_complete_deterministic_and_preserves_divergence() {
    let value = texture_lod_boundary_sweep();
    let bundle = validate_json(&serde_json::to_vec(&value).unwrap()).unwrap();
    let first = analyze_texture_lod_boundary_sweep(&bundle, "lod-boundary-v1").unwrap();
    let second = analyze_texture_lod_boundary_sweep(&bundle, "lod-boundary-v1").unwrap();
    assert_eq!(first, second);
    assert_eq!(first.analysis_sha256.len(), 64);
    assert_eq!(first.lod_boundary_numerator_i64, 32);
    assert_eq!(first.lod_metric_denominator_u64, 32);
    assert_eq!(first.modes.len(), 3);
    assert_eq!(first.modes[0].lod_mode, TextureLodMode::Mip);
    assert_eq!(first.modes[1].lod_mode, TextureLodMode::Detail);
    assert_eq!(first.modes[2].lod_mode, TextureLodMode::Sharpen);
    assert_eq!(
        first.modes[0].cycles[0].cycle_type,
        ProbeCycleType::OneCycle
    );
    assert_eq!(first.modes[0].cycles[0].observations.len(), 3);
    assert_eq!(
        first.modes[0].cycles[0].observations[0].boundary_position,
        TextureLodBoundaryPosition::Below
    );
    assert_eq!(
        first.modes[0].cycles[0].observations[0]
            .observed_selection
            .unwrap()
            .tile0_u3,
        2
    );
    assert!(first.modes[0].cycle_results_match);
    assert!(first.modes[1].cycle_results_match);
    assert!(!first.modes[2].cycle_results_match);
    assert_eq!(
        first.modes[2].cycles[1].observations[2].observed_selection,
        None
    );
    assert_eq!(first.unexpected_output_count, 1);
    assert!(!first.all_cycle_results_match);
}

#[test]
fn texture_lod_boundary_rejects_missing_duplicate_reset_and_mislabeled_cases() {
    let mut missing = texture_lod_boundary_sweep();
    missing["cases"].as_array_mut().unwrap().remove(8);
    let missing = validate_json(&serde_json::to_vec(&missing).unwrap()).unwrap();
    assert!(
        analyze_texture_lod_boundary_sweep(&missing, "lod-boundary-v1")
            .unwrap_err()
            .to_string()
            .contains("missing Sharpen OneCycle Above point")
    );

    let mut duplicate = texture_lod_boundary_sweep();
    let mut repeated = duplicate["cases"][0].clone();
    repeated["case_id"] = json!("duplicate-texture-lod-point");
    duplicate["cases"].as_array_mut().unwrap().push(repeated);
    let duplicate = validate_json(&serde_json::to_vec(&duplicate).unwrap()).unwrap();
    assert!(
        analyze_texture_lod_boundary_sweep(&duplicate, "lod-boundary-v1")
            .unwrap_err()
            .to_string()
            .contains("duplicate texture-LOD point")
    );

    let mut reset = texture_lod_boundary_sweep();
    reset["cases"][0]["capture_intent"]["replay_from_reset"] = json!(false);
    let reset = validate_json(&serde_json::to_vec(&reset).unwrap()).unwrap();
    assert!(
        analyze_texture_lod_boundary_sweep(&reset, "lod-boundary-v1")
            .unwrap_err()
            .to_string()
            .contains("replay from reset")
    );

    let mut mislabeled = texture_lod_boundary_sweep();
    mislabeled["cases"][0]["capture_intent"]["boundary_position"] = json!("on");
    assert!(validate(&mislabeled)
        .unwrap_err()
        .contains("does not have the declared exact relation"));
}

#[test]
fn texture_lod_boundary_rejects_derivative_metric_and_cross_mode_drift() {
    let mut derivative = texture_lod_boundary_sweep();
    derivative["cases"][0]["capture_intent"]["dsdx_s10_5_i32"] = json!(30);
    assert!(validate(&derivative)
        .unwrap_err()
        .contains("derivatives must exactly equal neighbor minus center"));

    let mut denominator = texture_lod_boundary_sweep();
    denominator["cases"][0]["capture_intent"]["lod_metric_denominator_u64"] = json!(0);
    assert!(validate(&denominator)
        .unwrap_err()
        .contains("metric denominator must be nonzero"));

    let mut nonadjacent = texture_lod_boundary_sweep();
    for index in [0usize, 3, 6, 9, 12, 15] {
        nonadjacent["cases"][index]["capture_intent"]["lod_metric_numerator_i64"] = json!(30);
    }
    let nonadjacent = validate_json(&serde_json::to_vec(&nonadjacent).unwrap()).unwrap();
    assert!(
        analyze_texture_lod_boundary_sweep(&nonadjacent, "lod-boundary-v1")
            .unwrap_err()
            .to_string()
            .contains("boundary-1/boundary/boundary+1 inputs")
    );

    let mut cross_mode = texture_lod_boundary_sweep();
    cross_mode["cases"][3]["capture_intent"]["x_neighbor_s_s10_5_i16"] = json!(94);
    cross_mode["cases"][3]["capture_intent"]["dsdx_s10_5_i32"] = json!(30);
    let cross_mode = validate_json(&serde_json::to_vec(&cross_mode).unwrap()).unwrap();
    assert!(
        analyze_texture_lod_boundary_sweep(&cross_mode, "lod-boundary-v1")
            .unwrap_err()
            .to_string()
            .contains("different Below coordinates, derivatives, or metric across modes/cycles")
    );

    let mut cross_cycle_expected = texture_lod_boundary_sweep();
    cross_cycle_expected["cases"][9]["capture_intent"]["producer_expected_tile1_u3"] = json!(3);
    let cross_cycle_expected =
        validate_json(&serde_json::to_vec(&cross_cycle_expected).unwrap()).unwrap();
    assert!(
        analyze_texture_lod_boundary_sweep(&cross_cycle_expected, "lod-boundary-v1")
            .unwrap_err()
            .to_string()
            .contains("different Mip Below expected selection/output across cycle modes")
    );
}

#[test]
fn texture_lod_boundary_rejects_marker_geometry_and_preserves_output_divergence() {
    let mut marker = texture_lod_boundary_sweep();
    for index in [5usize, 14] {
        marker["cases"][index]["capture_intent"]["producer_expected_framebuffer_rgba32_be"] =
            json!(0x4050_60ffu32);
    }
    let marker = validate_json(&serde_json::to_vec(&marker).unwrap()).unwrap();
    assert!(
        analyze_texture_lod_boundary_sweep(&marker, "lod-boundary-v1")
            .unwrap_err()
            .to_string()
            .contains("one output marker to different expected selections")
    );

    let mut geometry = texture_lod_boundary_sweep();
    geometry["cases"][1]["expected"]["framebuffer"]["row_stride_bytes"] = json!(8);
    geometry["cases"][1]["expected"]["framebuffer"]["contents"] = blob(&[0; 8]);
    let geometry = validate_json(&serde_json::to_vec(&geometry).unwrap()).unwrap();
    assert!(
        analyze_texture_lod_boundary_sweep(&geometry, "lod-boundary-v1")
            .unwrap_err()
            .to_string()
            .contains("requires exact 1x1 RGBA32")
    );

    let mut coverage = texture_lod_boundary_sweep();
    coverage["cases"][0]["expected"]["coverage"]["contents"] = blob(&[4]);
    let coverage = validate_json(&serde_json::to_vec(&coverage).unwrap()).unwrap();
    let coverage = analyze_texture_lod_boundary_sweep(&coverage, "lod-boundary-v1").unwrap();
    assert_eq!(
        coverage.modes[0].cycles[0].observations[0].stored_coverage_u3,
        4
    );
    assert!(!coverage.modes[0].cycles[0].observations[0].coverage_matches_producer_control);
    assert_eq!(coverage.unexpected_coverage_count, 1);
    assert!(!coverage.modes[0].cycle_results_match);

    let mut depth = texture_lod_boundary_sweep();
    depth["cases"][1]["expected"]["depth"]["contents"] = blob(&0x5ffeu16.to_be_bytes());
    let depth = validate_json(&serde_json::to_vec(&depth).unwrap()).unwrap();
    let depth = analyze_texture_lod_boundary_sweep(&depth, "lod-boundary-v1").unwrap();
    assert_eq!(
        depth.modes[0].cycles[0].observations[1].depth_u16_be,
        0x5ffe
    );
    assert!(!depth.modes[0].cycles[0].observations[1].depth_matches_producer_control);
    assert_eq!(depth.unexpected_depth_count, 1);
    assert!(!depth.modes[0].cycle_results_match);

    let mut setup = texture_lod_boundary_sweep();
    setup["cases"][1]["setup"]["registers"][2]["value"] = json!(1);
    let setup = validate_json(&serde_json::to_vec(&setup).unwrap()).unwrap();
    assert!(
        analyze_texture_lod_boundary_sweep(&setup, "lod-boundary-v1")
            .unwrap_err()
            .to_string()
            .contains("differs within sweep")
    );
}

#[test]
fn rgb_dither_sweep_preserves_complete_tiles_and_cycle_difference() {
    let value = rgb_dither_sweep();
    let bundle = validate_json(&serde_json::to_vec(&value).unwrap()).unwrap();
    let analysis = analyze_rgb_dither_sweep(&bundle, "magic-red-v1").unwrap();
    assert_eq!(analysis.mode, RgbDitherMode::MagicSquare);
    assert_eq!(analysis.swept_channel, RgbDitherChannel::Red);
    assert_eq!(analysis.fixed_rgb8, [0, 128, 128]);
    assert_eq!((analysis.origin_x, analysis.origin_y), (12, 20));
    assert_eq!(analysis.cycles.len(), 2);
    assert_eq!(analysis.cycles[0].cycle_type, ProbeCycleType::OneCycle);
    assert_eq!(analysis.cycles[0].channel_u5_hex.len(), 256 * 16 * 2);
    assert_eq!(analysis.cycles[0].distinct_channel_codes, 32);
    assert!(analysis.cycles[0].monotonic_per_pixel);
    assert!(!analysis.cycle_results_match);
}

#[test]
fn rgb_dither_sweep_rejects_missing_reset_and_control_drift() {
    let mut missing = rgb_dither_sweep();
    missing["cases"].as_array_mut().unwrap().remove(300);
    let missing = validate_json(&serde_json::to_vec(&missing).unwrap()).unwrap();
    assert!(analyze_rgb_dither_sweep(&missing, "magic-red-v1")
        .unwrap_err()
        .to_string()
        .contains("missing TwoCycle channel value 44"));

    let mut reset = rgb_dither_sweep();
    reset["cases"][0]["capture_intent"]["replay_from_reset"] = json!(false);
    let reset = validate_json(&serde_json::to_vec(&reset).unwrap()).unwrap();
    assert!(analyze_rgb_dither_sweep(&reset, "magic-red-v1")
        .unwrap_err()
        .to_string()
        .contains("replay from reset"));

    let mut drift = rgb_dither_sweep();
    drift["cases"][200]["capture_intent"]["origin_x"] = json!(13);
    let drift = validate_json(&serde_json::to_vec(&drift).unwrap()).unwrap();
    assert!(analyze_rgb_dither_sweep(&drift, "magic-red-v1")
        .unwrap_err()
        .to_string()
        .contains("differs within sweep"));
}

#[test]
fn rgb_dither_intent_rejects_channel_value_mismatch() {
    let mut mismatch = rgb_dither_sweep();
    mismatch["cases"][0]["capture_intent"]["channel_value"] = json!(1);
    assert!(validate(&mismatch)
        .unwrap_err()
        .contains("channel_value must equal the selected"));
}

#[test]
fn alpha_dither_sweep_reports_exact_cycle_transitions_without_hiding_difference() {
    let value = alpha_dither_sweep(129, 130);
    let bundle = validate_json(&serde_json::to_vec(&value).unwrap()).unwrap();
    let analysis = analyze_alpha_dither_sweep(&bundle, "sample-zero").unwrap();
    assert_eq!(analysis.sample_index, 0);
    assert_eq!(analysis.pass_rgba16_be, 0x7c01);
    assert_eq!(analysis.reject_rgba16_be, 0x0001);
    assert_eq!(analysis.transitions.len(), 2);
    assert_eq!(analysis.transitions[0].cycle_type, ProbeCycleType::OneCycle);
    assert_eq!(analysis.transitions[0].first_passing_alpha, Some(129));
    assert_eq!(analysis.transitions[0].first_reject_after_pass, None);
    assert_eq!(analysis.transitions[0].pass_count, 127);
    assert_eq!(analysis.transitions[0].transition_count, 1);
    assert!(analysis.transitions[0].monotonic_reject_then_pass);
    assert_eq!(analysis.transitions[1].cycle_type, ProbeCycleType::TwoCycle);
    assert_eq!(analysis.transitions[1].first_passing_alpha, Some(130));
    assert!(!analysis.cycle_transitions_match);

    let mut all_rejected = alpha_dither_sweep(129, 129);
    for case in all_rejected["cases"].as_array_mut().unwrap() {
        case["expected"]["framebuffer"]["contents"] = blob(&[0x00, 0x01]);
    }
    let all_rejected = validate_json(&serde_json::to_vec(&all_rejected).unwrap()).unwrap();
    let analysis = analyze_alpha_dither_sweep(&all_rejected, "sample-zero").unwrap();
    assert_eq!(analysis.transitions[0].first_passing_alpha, None);
    assert_eq!(analysis.transitions[1].first_passing_alpha, None);
    assert!(analysis.cycle_transitions_match);
}

#[test]
fn alpha_dither_sweep_rejects_missing_or_changed_controls() {
    let mut missing = alpha_dither_sweep(129, 129);
    missing["cases"].as_array_mut().unwrap().remove(300);
    let missing = validate_json(&serde_json::to_vec(&missing).unwrap()).unwrap();
    assert!(analyze_alpha_dither_sweep(&missing, "sample-zero")
        .unwrap_err()
        .to_string()
        .contains("missing TwoCycle alpha 44"));

    let mut replay = alpha_dither_sweep(129, 129);
    replay["cases"][0]["capture_intent"]["replay_from_reset"] = json!(false);
    let replay = validate_json(&serde_json::to_vec(&replay).unwrap()).unwrap();
    assert!(analyze_alpha_dither_sweep(&replay, "sample-zero")
        .unwrap_err()
        .to_string()
        .contains("replay from reset"));

    let mut changed_sample = alpha_dither_sweep(129, 129);
    changed_sample["cases"][200]["capture_intent"]["sample_index"] = json!(1);
    let changed_sample = validate_json(&serde_json::to_vec(&changed_sample).unwrap()).unwrap();
    assert!(analyze_alpha_dither_sweep(&changed_sample, "sample-zero")
        .unwrap_err()
        .to_string()
        .contains("sample index or pass/reject markers differ"));
}

#[test]
fn alpha_dither_sweep_preserves_nonmonotonic_hardware_observations() {
    let mut nonmonotonic = alpha_dither_sweep(129, 129);
    nonmonotonic["cases"][200]["expected"]["framebuffer"]["contents"] = blob(&[0x00, 0x01]);
    let nonmonotonic = validate_json(&serde_json::to_vec(&nonmonotonic).unwrap()).unwrap();
    let analysis = analyze_alpha_dither_sweep(&nonmonotonic, "sample-zero").unwrap();
    let one_cycle = &analysis.transitions[0];
    assert_eq!(one_cycle.first_passing_alpha, Some(129));
    assert_eq!(one_cycle.first_reject_after_pass, Some(200));
    assert_eq!(one_cycle.pass_count, 126);
    assert_eq!(one_cycle.transition_count, 3);
    assert!(!one_cycle.monotonic_reject_then_pass);
    assert_eq!(one_cycle.pass_bitmap_hex.len(), 64);
    assert!(!analysis.cycle_transitions_match);
}

#[test]
fn alpha_dither_intent_rejects_ambiguous_markers_and_unrecognized_output() {
    let mut ambiguous = alpha_dither_sweep(129, 129);
    ambiguous["cases"][0]["capture_intent"]["pass_rgba16_be"] = json!(1);
    assert!(validate(&ambiguous)
        .unwrap_err()
        .contains("pass and reject RGBA16 markers must differ"));

    let mut output = alpha_dither_sweep(129, 129);
    output["cases"][0]["expected"]["framebuffer"]["contents"] = blob(&[0x12, 0x35]);
    let output = validate_json(&serde_json::to_vec(&output).unwrap()).unwrap();
    assert!(analyze_alpha_dither_sweep(&output, "sample-zero")
        .unwrap_err()
        .to_string()
        .contains("neither pass marker"));
}

#[test]
fn alpha_coverage_sweep_preserves_all_curves_and_cycle_comparison() {
    let value = alpha_coverage_product_sweep();
    let bundle = validate_json(&serde_json::to_vec(&value).unwrap()).unwrap();
    let analysis = analyze_alpha_coverage_product_sweep(&bundle, "product-v1").unwrap();
    assert_eq!(analysis.curves.len(), 16);
    assert!(analysis.cycle_curves_match);
    let coverage_one = &analysis.curves[0];
    assert_eq!(coverage_one.cycle_type, ProbeCycleType::OneCycle);
    assert_eq!(coverage_one.input_coverage, 1);
    assert_eq!(coverage_one.alpha_zero_output_coverage, 8);
    assert_eq!(coverage_one.first_nonzero_alpha, Some(128));
    assert_eq!(coverage_one.first_full_alpha, Some(128));
    assert_eq!(coverage_one.transition_count, 2);
    assert_eq!(coverage_one.coverage_u4_hex.len(), 256);
    let coverage_eight = &analysis.curves[7];
    assert_eq!(coverage_eight.first_nonzero_alpha, Some(16));
    assert_eq!(coverage_eight.first_full_alpha, Some(240));
    assert_eq!(coverage_eight.transition_count, 9);
    assert!(coverage_eight.monotonic_nondecreasing_from_alpha_one);
}

#[test]
fn alpha_coverage_sweep_rejects_incomplete_or_invalid_inputs() {
    let mut missing = alpha_coverage_product_sweep();
    missing["cases"].as_array_mut().unwrap().remove(600);
    let missing = validate_json(&serde_json::to_vec(&missing).unwrap()).unwrap();
    assert!(analyze_alpha_coverage_product_sweep(&missing, "product-v1")
        .unwrap_err()
        .to_string()
        .contains("input coverage 3 alpha 88"));

    let mut invalid = alpha_coverage_product_sweep();
    invalid["cases"][0]["capture_intent"]["input_coverage"] = json!(0);
    assert!(validate(&invalid)
        .unwrap_err()
        .contains("input_coverage must be in 1..=8"));
}

#[test]
fn coverage_to_alpha_sweep_preserves_threshold_ties_and_cycle_comparison() {
    let value = coverage_to_alpha_sweep();
    let bundle = validate_json(&serde_json::to_vec(&value).unwrap()).unwrap();
    let analysis = analyze_coverage_to_alpha_sweep(&bundle, "coverage-alpha-v1").unwrap();
    assert_eq!(analysis.curves.len(), 16);
    assert!(analysis.cycle_curves_match);
    assert_eq!(analysis.pass_rgba16_be, 0x7c01);
    assert_eq!(analysis.reject_rgba16_be, 0x0001);
    assert_eq!(analysis.curves[0].greatest_passing_threshold, Some(32));
    assert_eq!(analysis.curves[0].pass_count, 33);
    assert_eq!(analysis.curves[0].transition_count, 1);
    assert!(analysis.curves[0].monotonic_pass_then_reject);
    assert_eq!(analysis.curves[7].greatest_passing_threshold, Some(255));
    assert_eq!(analysis.curves[7].transition_count, 0);
    assert_eq!(analysis.curves[7].pass_bitmap_hex.len(), 64);
}

#[test]
fn coverage_to_alpha_sweep_rejects_missing_and_changed_controls() {
    let mut missing = coverage_to_alpha_sweep();
    missing["cases"].as_array_mut().unwrap().remove(777);
    let missing = validate_json(&serde_json::to_vec(&missing).unwrap()).unwrap();
    assert!(
        analyze_coverage_to_alpha_sweep(&missing, "coverage-alpha-v1")
            .unwrap_err()
            .to_string()
            .contains("input coverage 4 threshold 9")
    );

    let mut changed = coverage_to_alpha_sweep();
    changed["cases"][20]["capture_intent"]["pass_rgba16_be"] = json!(0x7801);
    let changed = validate_json(&serde_json::to_vec(&changed).unwrap()).unwrap();
    assert!(
        analyze_coverage_to_alpha_sweep(&changed, "coverage-alpha-v1")
            .unwrap_err()
            .to_string()
            .contains("pass/reject markers differ")
    );
}

#[test]
fn zmode_inter_analysis_is_deterministic_and_complete() {
    let value = zmode_inter_sweep();
    let bundle = validate_json(&serde_json::to_vec(&value).unwrap()).unwrap();
    let first = analyze_zmode_inter_coverage_sweep(&bundle, "inter-v1").unwrap();
    let second = analyze_zmode_inter_coverage_sweep(&bundle, "inter-v1").unwrap();
    assert_eq!(first, second);
    assert_eq!(first.schema, "fn64.rdp-zmode-inter-analysis.v1");
    assert_eq!(first.analysis_sha256.len(), 64);
    assert_eq!(first.relations.len(), 6);
    assert!(first.cycle_results_match);

    let front = &first.relations[0];
    assert_eq!(front.cycle_type, ProbeCycleType::OneCycle);
    assert_eq!(front.relation, ZModeInterRelation::InFrontControl);
    assert_eq!(front.admitted_count, 64);
    assert_eq!(front.admission_bitmap_hex, "ffffffffffffffff");
    assert_eq!(
        front.stored_coverage_u3_hex,
        "0000000011111111222222223333333344444444555555556666666677777777"
    );
    assert_eq!(front.changed_from_initial_count, 56);
    assert_eq!(front.rejected_coverage_changed_count, 0);

    let inter = &first.relations[1];
    assert_eq!(inter.relation, ZModeInterRelation::Interpenetrating);
    assert_eq!(inter.admitted_count, 28);
    assert_eq!(inter.admission_bitmap_hex, "000103070f1f3f7f");
    assert_eq!(inter.stored_coverage_u3_hex, "12345670".repeat(8));
    assert_eq!(inter.changed_from_initial_count, 64);
    assert_eq!(inter.rejected_coverage_changed_count, 36);

    let behind = &first.relations[2];
    assert_eq!(behind.relation, ZModeInterRelation::BehindControl);
    assert_eq!(behind.admitted_count, 0);
    assert_eq!(behind.admission_bitmap_hex, "0000000000000000");
    assert_eq!(behind.stored_coverage_u3_hex, "01234567".repeat(8));
    assert_eq!(behind.changed_from_initial_count, 0);
}

#[test]
fn zmode_inter_analysis_retains_cycle_divergence_and_rejected_changes() {
    let mut value = zmode_inter_sweep();
    let point = &mut value["cases"][256];
    point["expected"]["framebuffer"]["contents"] = blob(&[0x7c, 0x01]);
    point["expected"]["coverage"]["contents"] = blob(&[7]);
    let bundle = validate_json(&serde_json::to_vec(&value).unwrap()).unwrap();
    let analysis = analyze_zmode_inter_coverage_sweep(&bundle, "inter-v1").unwrap();
    assert!(!analysis.cycle_results_match);
    assert_eq!(analysis.relations[1].rejected_coverage_changed_count, 36);
    assert_eq!(analysis.relations[4].admitted_count, 29);
    assert_eq!(
        analysis.relations[4].stored_coverage_u3_hex.as_bytes()[0],
        b'7'
    );
}

#[test]
fn zmode_inter_analysis_rejects_missing_duplicate_and_reset_violations() {
    let mut missing = zmode_inter_sweep();
    missing["cases"].as_array_mut().unwrap().remove(70);
    let missing = validate_json(&serde_json::to_vec(&missing).unwrap()).unwrap();
    assert!(analyze_zmode_inter_coverage_sweep(&missing, "inter-v1")
        .unwrap_err()
        .to_string()
        .contains("incoming coverage 1 initial stored coverage 6"));

    let mut duplicate = zmode_inter_sweep();
    let mut repeated = duplicate["cases"][0].clone();
    repeated["case_id"] = json!("different-id-same-zmode-point");
    duplicate["cases"].as_array_mut().unwrap().push(repeated);
    let duplicate = validate_json(&serde_json::to_vec(&duplicate).unwrap()).unwrap();
    assert!(analyze_zmode_inter_coverage_sweep(&duplicate, "inter-v1")
        .unwrap_err()
        .to_string()
        .contains("duplicate ZMODE_INTER point"));

    let mut reset = zmode_inter_sweep();
    reset["cases"][0]["capture_intent"]["replay_from_reset"] = json!(false);
    let reset = validate_json(&serde_json::to_vec(&reset).unwrap()).unwrap();
    assert!(analyze_zmode_inter_coverage_sweep(&reset, "inter-v1")
        .unwrap_err()
        .to_string()
        .contains("replay from reset"));
}

#[test]
fn zmode_inter_analysis_rejects_control_geometry_and_cross_label_drift() {
    let mut markers = zmode_inter_sweep();
    markers["cases"][1]["capture_intent"]["pass_rgba16_be"] = json!(0x7801);
    let markers = validate_json(&serde_json::to_vec(&markers).unwrap()).unwrap();
    assert!(analyze_zmode_inter_coverage_sweep(&markers, "inter-v1")
        .unwrap_err()
        .to_string()
        .contains("pass/reject markers differ"));

    let mut controls = zmode_inter_sweep();
    controls["cases"][1]["capture_intent"]["incoming_z_u18"] = json!(0x08001);
    let controls = validate_json(&serde_json::to_vec(&controls).unwrap()).unwrap();
    assert!(analyze_zmode_inter_coverage_sweep(&controls, "inter-v1")
        .unwrap_err()
        .to_string()
        .contains("numeric controls differ within relation"));

    let mut geometry = zmode_inter_sweep();
    geometry["cases"][1]["expected"]["framebuffer"]["address"] = json!(8200);
    geometry["cases"][1]["expected"]["coverage"]["color_image_address"] = json!(8200);
    let geometry = validate_json(&serde_json::to_vec(&geometry).unwrap()).unwrap();
    assert!(analyze_zmode_inter_coverage_sweep(&geometry, "inter-v1")
        .unwrap_err()
        .to_string()
        .contains("output addresses differ"));

    let mut cross_label = zmode_inter_sweep();
    for case in cross_label["cases"].as_array_mut().unwrap() {
        if case["capture_intent"]["relation"] == "interpenetrating" {
            case["capture_intent"]["incoming_z_u18"] = json!(0x08000);
            case["capture_intent"]["memory_z_u18"] = json!(0x10000);
            case["capture_intent"]["incoming_delta_z_u16"] = json!(0x0100);
            case["capture_intent"]["memory_delta_z_u16"] = json!(0x0200);
        }
    }
    let cross_label = validate_json(&serde_json::to_vec(&cross_label).unwrap()).unwrap();
    assert!(analyze_zmode_inter_coverage_sweep(&cross_label, "inter-v1")
        .unwrap_err()
        .to_string()
        .contains("reuse the same numeric controls"));
}

#[test]
fn zmode_inter_intent_rejects_invalid_domains() {
    let mut incoming_coverage = zmode_inter_sweep();
    incoming_coverage["cases"][0]["capture_intent"]["incoming_coverage"] = json!(0);
    assert!(validate(&incoming_coverage)
        .unwrap_err()
        .contains("incoming_coverage must be in 1..=8"));

    let mut stored_coverage = zmode_inter_sweep();
    stored_coverage["cases"][0]["capture_intent"]["initial_stored_coverage"] = json!(8);
    assert!(validate(&stored_coverage)
        .unwrap_err()
        .contains("initial_stored_coverage must be in 0..=7"));

    let mut z = zmode_inter_sweep();
    z["cases"][0]["capture_intent"]["incoming_z_u18"] = json!(0x40000);
    assert!(validate(&z)
        .unwrap_err()
        .contains("must fit unsigned 18-bit values"));
}

#[test]
fn representative_sample_analysis_is_deterministic_and_complete() {
    let value = representative_sample_sweep();
    let bundle = validate_json(&serde_json::to_vec(&value).unwrap()).unwrap();
    let first = analyze_representative_sample_selector_sweep(&bundle, "selector-v1").unwrap();
    let second = analyze_representative_sample_selector_sweep(&bundle, "selector-v1").unwrap();
    assert_eq!(first, second);
    assert_eq!(first.schema, "fn64.rdp-representative-sample-analysis.v1");
    assert_eq!(first.analysis_sha256.len(), 64);
    assert_eq!(first.tables.len(), 6);
    assert!(first.all_cycle_results_match);
    assert!(first.all_observable_results_match);
    let expected = (1u16..=255)
        .map(|mask| char::from_digit(mask.trailing_zeros(), 16).unwrap())
        .collect::<String>();
    for table in &first.tables {
        assert_eq!(table.selected_sample_u3_hex, expected);
        assert_eq!(table.selected_sample_counts, [128, 64, 32, 16, 8, 4, 2, 1]);
        assert_eq!(table.uncovered_selection_count, 0);
    }
    assert_eq!(
        first.tables[0].observable,
        RepresentativeSampleObservable::Shade
    );
    assert_eq!(
        first.tables[1].observable,
        RepresentativeSampleObservable::Texture
    );
    assert_eq!(
        first.tables[2].observable,
        RepresentativeSampleObservable::Depth
    );
    assert_eq!(first.tables[3].cycle_type, ProbeCycleType::TwoCycle);
}

#[test]
fn narrow_edge_coverage_analysis_is_deterministic_complete_and_exact() {
    let value = narrow_edge_coverage_sweep();
    let bundle = validate_json(&serde_json::to_vec(&value).unwrap()).unwrap();
    let first = analyze_narrow_edge_coverage_correction_sweep(&bundle, "narrow-edge-v1").unwrap();
    let second = analyze_narrow_edge_coverage_correction_sweep(&bundle, "narrow-edge-v1").unwrap();
    assert_eq!(first, second);
    assert_eq!(first.schema, "fn64.rdp-narrow-edge-coverage-analysis.v1");
    assert_eq!(first.analysis_sha256.len(), 64);
    assert_eq!(first.controls.edge_fractional_bits_u8, 16);
    assert_eq!(first.boundaries.len(), 2);
    assert!(first.all_cycle_results_match);
    assert!(first.all_observable_sample_indices_match);
    assert_eq!(first.boundaries[0].points.len(), 6);
    let below = &first.boundaries[0].points[0];
    assert_eq!(below.cycle_type, ProbeCycleType::OneCycle);
    assert_eq!(below.boundary_position, NarrowEdgeBoundaryPosition::Below);
    assert_eq!(below.edge_accumulator_i64, -65_537);
    assert_eq!(below.coverage_mask_u8, 1);
    assert_eq!(below.coverage_count_u4, 1);
    assert_eq!(below.observations.len(), 3);
    assert_eq!(
        below.observations[0].observable,
        RepresentativeSampleObservable::Shade
    );
    assert_eq!(below.observations[0].observed_sample_index_u3, 0);
}

#[test]
fn narrow_edge_coverage_analysis_retains_cycle_and_observable_divergence() {
    let mut value = narrow_edge_coverage_sweep();
    value["cases"][10]["expected"]["framebuffer"]["contents"] = blob(&0x0022_00ffu32.to_be_bytes());
    let bundle = validate_json(&serde_json::to_vec(&value).unwrap()).unwrap();
    let analysis =
        analyze_narrow_edge_coverage_correction_sweep(&bundle, "narrow-edge-v1").unwrap();
    assert!(!analysis.all_cycle_results_match);
    assert!(!analysis.all_observable_sample_indices_match);
    let two_cycle_below = &analysis.boundaries[0].points[3];
    assert!(!two_cycle_below.observable_sample_indices_match);
    assert_eq!(two_cycle_below.observations[1].observed_sample_index_u3, 1);
}

#[test]
fn narrow_edge_coverage_analysis_rejects_incomplete_duplicate_reset_and_drift() {
    let mut missing = narrow_edge_coverage_sweep();
    missing["cases"].as_array_mut().unwrap().truncate(18);
    let missing = validate_json(&serde_json::to_vec(&missing).unwrap()).unwrap();
    assert!(
        analyze_narrow_edge_coverage_correction_sweep(&missing, "narrow-edge-v1")
            .unwrap_err()
            .to_string()
            .contains("missing boundary 65536")
    );

    let mut duplicate = narrow_edge_coverage_sweep();
    let mut repeated = duplicate["cases"][0].clone();
    repeated["case_id"] = json!("different-id-same-narrow-edge-point");
    duplicate["cases"].as_array_mut().unwrap().push(repeated);
    let duplicate = validate_json(&serde_json::to_vec(&duplicate).unwrap()).unwrap();
    assert!(
        analyze_narrow_edge_coverage_correction_sweep(&duplicate, "narrow-edge-v1")
            .unwrap_err()
            .to_string()
            .contains("duplicate narrow-edge-coverage point")
    );

    let mut reset = narrow_edge_coverage_sweep();
    reset["cases"][0]["capture_intent"]["replay_from_reset"] = json!(false);
    let reset = validate_json(&serde_json::to_vec(&reset).unwrap()).unwrap();
    assert!(
        analyze_narrow_edge_coverage_correction_sweep(&reset, "narrow-edge-v1")
            .unwrap_err()
            .to_string()
            .contains("replay from reset")
    );

    let mut declaration_drift = narrow_edge_coverage_sweep();
    declaration_drift["cases"][1]["capture_intent"]["coverage_mask_u8"] = json!(2);
    let declaration_drift =
        validate_json(&serde_json::to_vec(&declaration_drift).unwrap()).unwrap();
    assert!(
        analyze_narrow_edge_coverage_correction_sweep(&declaration_drift, "narrow-edge-v1")
            .unwrap_err()
            .to_string()
            .contains("differs across observables")
    );

    let mut observed_count = narrow_edge_coverage_sweep();
    observed_count["cases"][0]["expected"]["coverage"]["contents"] = blob(&[2]);
    let observed_count = validate_json(&serde_json::to_vec(&observed_count).unwrap()).unwrap();
    assert!(
        analyze_narrow_edge_coverage_correction_sweep(&observed_count, "narrow-edge-v1")
            .unwrap_err()
            .to_string()
            .contains("declared coverage count 1, observed 2")
    );

    let mut setup = narrow_edge_coverage_sweep();
    setup["cases"][1]["setup"]["registers"][2]["value"] = json!(1);
    let setup = validate_json(&serde_json::to_vec(&setup).unwrap()).unwrap();
    assert!(
        analyze_narrow_edge_coverage_correction_sweep(&setup, "narrow-edge-v1")
            .unwrap_err()
            .to_string()
            .contains("setup differs")
    );
}

#[test]
fn narrow_edge_coverage_intent_rejects_invalid_lsb_mask_count_and_markers() {
    let mut lsb = narrow_edge_coverage_sweep();
    lsb["cases"][0]["capture_intent"]["edge_accumulator_i64"] = json!(-65_538i64);
    assert!(validate(&lsb)
        .unwrap_err()
        .contains("accumulator must be exactly -65537"));

    let mut count = narrow_edge_coverage_sweep();
    count["cases"][0]["capture_intent"]["coverage_count_u4"] = json!(2);
    assert!(validate(&count)
        .unwrap_err()
        .contains("has count 1, not declared count 2"));

    let mut manifest = narrow_edge_coverage_sweep();
    manifest["cases"][0]["capture_intent"]["controls"]["selected_boundaries_i64"] =
        json!([65_536i64, -65_536i64]);
    assert!(validate(&manifest)
        .unwrap_err()
        .contains("must be strictly increasing"));

    let mut markers = narrow_edge_coverage_sweep();
    markers["cases"][0]["capture_intent"]["controls"]["markers"]["depth_u16_be"][1] =
        json!(0x1001u16);
    assert!(validate(&markers)
        .unwrap_err()
        .contains("uniquely identify all eight samples"));
}

#[test]
fn representative_sample_analysis_retains_cycle_observable_and_uncovered_divergence() {
    let mut value = representative_sample_sweep();
    value["cases"][257]["expected"]["framebuffer"]["contents"] =
        blob(&0x0022_00ffu32.to_be_bytes());
    value["cases"][1275]["expected"]["depth"]["contents"] = blob(&0x1008u16.to_be_bytes());
    let bundle = validate_json(&serde_json::to_vec(&value).unwrap()).unwrap();
    let analysis = analyze_representative_sample_selector_sweep(&bundle, "selector-v1").unwrap();
    assert!(!analysis.all_cycle_results_match);
    assert!(!analysis.all_observable_results_match);
    assert!(!analysis.cycle_comparisons[1].matches);
    assert!(!analysis.cycle_comparisons[2].matches);
    assert!(!analysis.observable_comparisons[0].all_match);
    assert!(!analysis.observable_comparisons[1].all_match);
    assert_eq!(
        analysis.tables[1].selected_sample_u3_hex.as_bytes()[2],
        b'1'
    );
    assert_eq!(
        analysis.tables[5].selected_sample_u3_hex.as_bytes()[0],
        b'7'
    );
    assert_eq!(analysis.tables[5].uncovered_selection_count, 1);
}

#[test]
fn representative_sample_analysis_rejects_missing_duplicate_and_reset_violations() {
    let mut missing = representative_sample_sweep();
    missing["cases"].as_array_mut().unwrap().remove(700);
    let missing = validate_json(&serde_json::to_vec(&missing).unwrap()).unwrap();
    assert!(
        analyze_representative_sample_selector_sweep(&missing, "selector-v1")
            .unwrap_err()
            .to_string()
            .contains("missing OneCycle Depth mask 0xbf")
    );

    let mut duplicate = representative_sample_sweep();
    let mut repeated = duplicate["cases"][0].clone();
    repeated["case_id"] = json!("different-id-same-representative-point");
    duplicate["cases"].as_array_mut().unwrap().push(repeated);
    let duplicate = validate_json(&serde_json::to_vec(&duplicate).unwrap()).unwrap();
    assert!(
        analyze_representative_sample_selector_sweep(&duplicate, "selector-v1")
            .unwrap_err()
            .to_string()
            .contains("duplicate representative-sample point")
    );

    let mut reset = representative_sample_sweep();
    reset["cases"][0]["capture_intent"]["replay_from_reset"] = json!(false);
    let reset = validate_json(&serde_json::to_vec(&reset).unwrap()).unwrap();
    assert!(
        analyze_representative_sample_selector_sweep(&reset, "selector-v1")
            .unwrap_err()
            .to_string()
            .contains("replay from reset")
    );
}

#[test]
fn representative_sample_analysis_rejects_control_geometry_count_and_cross_label_drift() {
    let mut controls = representative_sample_sweep();
    controls["cases"][1]["capture_intent"]["controls"]["pixel_x"] = json!(42);
    let controls = validate_json(&serde_json::to_vec(&controls).unwrap()).unwrap();
    assert!(
        analyze_representative_sample_selector_sweep(&controls, "selector-v1")
            .unwrap_err()
            .to_string()
            .contains("fixed controls differ")
    );

    let mut geometry = representative_sample_sweep();
    geometry["cases"][1]["expected"]["framebuffer"]["address"] = json!(8200);
    geometry["cases"][1]["expected"]["coverage"]["color_image_address"] = json!(8200);
    let geometry = validate_json(&serde_json::to_vec(&geometry).unwrap()).unwrap();
    assert!(
        analyze_representative_sample_selector_sweep(&geometry, "selector-v1")
            .unwrap_err()
            .to_string()
            .contains("output addresses differ")
    );

    let mut count = representative_sample_sweep();
    count["cases"][2]["expected"]["coverage"]["contents"] = blob(&[1]);
    let count = validate_json(&serde_json::to_vec(&count).unwrap()).unwrap();
    assert!(
        analyze_representative_sample_selector_sweep(&count, "selector-v1")
            .unwrap_err()
            .to_string()
            .contains("has coverage count 2, observed 1")
    );

    let mut cross_label = representative_sample_sweep();
    cross_label["cases"][0]["capture_intent"]["observable"] = json!("texture");
    let cross_label = validate_json(&serde_json::to_vec(&cross_label).unwrap()).unwrap();
    assert!(
        analyze_representative_sample_selector_sweep(&cross_label, "selector-v1")
            .unwrap_err()
            .to_string()
            .contains("cross-label markers are not admitted")
    );
}

#[test]
fn representative_sample_intent_rejects_zero_mask_and_ambiguous_markers() {
    let mut zero = representative_sample_sweep();
    zero["cases"][0]["capture_intent"]["coverage_mask_u8"] = json!(0);
    assert!(validate(&zero)
        .unwrap_err()
        .contains("coverage_mask_u8 must be nonzero"));

    let mut duplicate = representative_sample_sweep();
    duplicate["cases"][0]["capture_intent"]["controls"]["markers"]["shade_rgba32_be"][1] =
        json!(0x1100_00ffu32);
    assert!(validate(&duplicate)
        .unwrap_err()
        .contains("uniquely identify all eight samples"));

    let mut cross_label = representative_sample_sweep();
    cross_label["cases"][0]["capture_intent"]["controls"]["markers"]["texture_rgba32_be"][1] =
        json!(0x1100_00ffu32);
    assert!(validate(&cross_label)
        .unwrap_err()
        .contains("domains must be disjoint"));
}

#[test]
fn rejects_missing_and_duplicate_cases() {
    let mut missing = fixture();
    missing["cases"] = json!([]);
    assert!(validate(&missing).unwrap_err().contains("no cases"));

    let mut duplicate = fixture();
    let case = duplicate["cases"][0].clone();
    duplicate["cases"].as_array_mut().unwrap().push(case);
    assert!(validate(&duplicate)
        .unwrap_err()
        .contains("duplicate case_id"));
}

#[test]
fn rejects_malformed_unknown_and_rom_class_inputs() {
    assert!(validate_json(b"{")
        .unwrap_err()
        .to_string()
        .contains("malformed"));
    let mut unknown = fixture();
    unknown["unexpected"] = json!(true);
    assert!(validate(&unknown).unwrap_err().contains("unknown field"));
    let mut content = fixture();
    content["content_class"] = json!("game_rom");
    assert!(validate(&content).unwrap_err().contains("ROM/game-derived"));
}

#[test]
fn rejects_digest_length_and_setup_ambiguity() {
    let mut bad_digest = fixture();
    bad_digest["cases"][0]["command_bytes"]["bytes_hex"] = json!("0000000000000000");
    assert!(validate(&bad_digest)
        .unwrap_err()
        .contains("SHA-256 mismatch"));

    let mut bad_length = fixture();
    bad_length["cases"][0]["setup"]["registers"][1]["value"] = json!(4112);
    assert!(validate(&bad_length)
        .unwrap_err()
        .contains("does not equal command_bytes"));

    let mut duplicate_register = fixture();
    let register = duplicate_register["cases"][0]["setup"]["registers"][0].clone();
    duplicate_register["cases"][0]["setup"]["registers"]
        .as_array_mut()
        .unwrap()
        .push(register);
    assert!(validate(&duplicate_register)
        .unwrap_err()
        .contains("duplicate setup register"));
}

#[test]
fn rejects_missing_output_bad_geometry_and_invalid_hidden_bits() {
    let mut missing = fixture();
    missing["cases"][0]["expected"]
        .as_object_mut()
        .unwrap()
        .remove("depth");
    assert!(validate(&missing)
        .unwrap_err()
        .contains("missing field `depth`"));

    let mut geometry = fixture();
    geometry["cases"][0]["expected"]["framebuffer"]["width"] = json!(3);
    assert!(validate(&geometry).unwrap_err().contains("row stride"));

    let mut coverage = fixture();
    let invalid = [4u8; 4];
    coverage["cases"][0]["expected"]["coverage"]["contents"] = blob(&invalid);
    assert!(validate(&coverage)
        .unwrap_err()
        .contains("encoding maximum 3"));
}

#[test]
fn rejects_duplicate_or_overlapping_initial_memory() {
    let mut duplicate = fixture();
    let region = json!({
        "region_id": "texture-a",
        "role": "texture",
        "address": 16384,
        "contents": blob(&[1, 2, 3, 4])
    });
    duplicate["cases"][0]["setup"]["initial_memory"] = json!([region.clone(), region]);
    assert!(validate(&duplicate)
        .unwrap_err()
        .contains("duplicate memory region_id"));

    let mut overlap = fixture();
    overlap["cases"][0]["setup"]["initial_memory"] = json!([{
        "region_id": "command-alias",
        "role": "auxiliary",
        "address": 4096,
        "contents": blob(&[0u8; 8])
    }]);
    assert!(validate(&overlap)
        .unwrap_err()
        .contains("overlaps command_bytes"));
}

#[test]
fn hardware_consensus_preserves_provenance_and_is_order_independent() {
    let runs = hardware_runs(10);
    let first = validate_hardware_consensus(&runs, 10).unwrap();
    assert_eq!(first.minimum_runs, 10);
    assert_eq!(first.run_count, 10);
    assert_eq!(first.runs.len(), 10);
    assert_eq!(
        first
            .runs
            .iter()
            .map(|run| &run.producer.recorded_at_utc)
            .collect::<std::collections::BTreeSet<_>>()
            .len(),
        10
    );
    assert_eq!(
        first
            .runs
            .iter()
            .map(|run| &run.producer.producer_binary_sha256)
            .collect::<std::collections::BTreeSet<_>>()
            .len(),
        1
    );
    assert_eq!(
        first
            .runs
            .iter()
            .map(|run| &run.producer.settings_sha256)
            .collect::<std::collections::BTreeSet<_>>()
            .len(),
        1
    );

    let reversed = runs.iter().cloned().rev().collect::<Vec<_>>();
    let second = validate_hardware_consensus(&reversed, 10).unwrap();
    assert_eq!(first.consensus_sha256, second.consensus_sha256);
    assert_eq!(first.runs, second.runs);
}

#[test]
fn hardware_consensus_rejects_too_few_nonhardware_and_duplicate_runs() {
    let runs = hardware_runs(9);
    assert!(validate_hardware_consensus(&runs, 10)
        .unwrap_err()
        .to_string()
        .contains("requires at least 10 runs; received 9"));

    let mut nonhardware = hardware_runs(10);
    nonhardware[3] = validate_json(&serde_json::to_vec(&fixture()).unwrap()).unwrap();
    assert!(validate_hardware_consensus(&nonhardware, 10)
        .unwrap_err()
        .to_string()
        .contains("run 4 producer kind is SyntheticFixture"));

    let mut duplicate = hardware_runs(10);
    duplicate[9] = duplicate[0].clone();
    assert!(validate_hardware_consensus(&duplicate, 10)
        .unwrap_err()
        .to_string()
        .contains("run 10 duplicates an earlier capture"));
}

#[test]
fn hardware_consensus_requires_controlled_producer_and_distinct_timestamps() {
    let changes = [
        ("name", json!("another-console")),
        ("version", json!("2")),
        ("platform", json!("another-unit")),
        ("adapter", json!("another-adapter")),
        ("adapter_version", json!("2")),
        ("producer_binary_sha256", json!(digest(b"another binary"))),
        ("settings_sha256", json!(digest(b"another settings"))),
        ("capture_method", json!("another method")),
    ];
    for (field, value) in changes {
        let mut runs = (0..10).map(hardware_fixture).collect::<Vec<_>>();
        runs[3]["producer"][field] = value;
        let runs = runs
            .iter()
            .map(|run| validate_json(&serde_json::to_vec(run).unwrap()).unwrap())
            .collect::<Vec<_>>();
        assert!(validate_hardware_consensus(&runs, 10)
            .unwrap_err()
            .to_string()
            .contains(&format!("run 4 mismatch at producer.{field}")));
    }

    let mut timestamps = (0..10).map(hardware_fixture).collect::<Vec<_>>();
    timestamps[5]["producer"]["recorded_at_utc"] = json!("2026-07-19T00:00:00Z");
    timestamps[5]["cases"][0]["expected"]["coverage"]["contents"] = blob(&[2, 3, 3, 3]);
    let timestamps = timestamps
        .iter()
        .map(|run| validate_json(&serde_json::to_vec(run).unwrap()).unwrap())
        .collect::<Vec<_>>();
    assert!(validate_hardware_consensus(&timestamps, 10)
        .unwrap_err()
        .to_string()
        .contains("run 6 duplicates recorded_at_utc"));
}

#[test]
fn hardware_consensus_reports_first_input_and_geometry_mismatch() {
    let mut setup_runs = (0..10).map(hardware_fixture).collect::<Vec<_>>();
    setup_runs[4]["cases"][0]["setup"]["registers"][2]["value"] = json!(1);
    let setup_runs = setup_runs
        .iter()
        .map(|run| validate_json(&serde_json::to_vec(run).unwrap()).unwrap())
        .collect::<Vec<_>>();
    assert!(validate_hardware_consensus(&setup_runs, 10)
        .unwrap_err()
        .to_string()
        .contains("run 5 mismatch at case[0].setup.registers[2].value"));

    let mut geometry_runs = (0..10).map(hardware_fixture).collect::<Vec<_>>();
    geometry_runs[2]["cases"][0]["expected"]["framebuffer"]["address"] = json!(8200);
    geometry_runs[2]["cases"][0]["expected"]["coverage"]["color_image_address"] = json!(8200);
    let geometry_runs = geometry_runs
        .iter()
        .map(|run| validate_json(&serde_json::to_vec(run).unwrap()).unwrap())
        .collect::<Vec<_>>();
    assert!(validate_hardware_consensus(&geometry_runs, 10)
        .unwrap_err()
        .to_string()
        .contains("run 3 mismatch at case[0].expected.framebuffer.address"));
}

#[test]
fn hardware_consensus_binds_typed_capture_intent() {
    let intent = json!({
        "kind": "alpha_compare_dither_sweep",
        "sweep_id": "sample-zero",
        "cycle_type": "one_cycle",
        "combined_alpha": 0,
        "replay_from_reset": true,
        "sample_index": 0,
        "pass_rgba16_be": 0x7c01,
        "reject_rgba16_be": 0x0001
    });
    let mut values = (0..10).map(hardware_fixture).collect::<Vec<_>>();
    for value in &mut values {
        value["cases"][0]["capture_intent"] = intent.clone();
    }
    values[4]["cases"][0]["capture_intent"]["sample_index"] = json!(1);
    let runs = values
        .iter()
        .map(|run| validate_json(&serde_json::to_vec(run).unwrap()).unwrap())
        .collect::<Vec<_>>();
    assert!(validate_hardware_consensus(&runs, 10)
        .unwrap_err()
        .to_string()
        .contains("run 5 mismatch at case[0].capture_intent"));
}

#[test]
fn hardware_consensus_reports_first_output_byte_mismatch() {
    for (channel, bytes, offset, expected, found) in [
        ("framebuffer", vec![0, 0, 0, 1, 0, 0, 0, 0], 3, "00", "01"),
        (
            "depth",
            vec![0xff, 0xff, 0xfe, 0xff, 0xff, 0xff, 0xff, 0xff],
            2,
            "ff",
            "fe",
        ),
        ("coverage", vec![3, 3, 2, 3], 2, "03", "02"),
    ] {
        let mut values = (0..10).map(hardware_fixture).collect::<Vec<_>>();
        values[6]["cases"][0]["expected"][channel]["contents"] = blob(&bytes);
        let runs = values
            .iter()
            .map(|run| validate_json(&serde_json::to_vec(run).unwrap()).unwrap())
            .collect::<Vec<_>>();
        let error = validate_hardware_consensus(&runs, 10)
            .unwrap_err()
            .to_string();
        assert!(error.contains(&format!(
            "run 7 mismatch at case[0].expected.{channel}.byte[{offset}]"
        )));
        assert!(error.contains(&format!("expected 0x{expected}, found 0x{found}")));
    }
}

#[test]
fn cli_help_documents_the_ten_run_default() {
    let output = Command::new(env!("CARGO_BIN_EXE_fn64-rdp-silicon-vectors"))
        .arg("--help")
        .output()
        .unwrap();
    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("--min-runs defaults to 10"));
}

#[test]
fn cli_validate_accepts_one_synthetic_bundle_and_default_mode_does_not() {
    let path = std::env::temp_dir().join(format!(
        "fn64-rdp-vector-validate-{}-{}.json",
        std::process::id(),
        digest(b"cli validate fixture")
    ));
    let bytes = serde_json::to_vec(&fixture()).unwrap();
    fs::write(&path, &bytes).unwrap();
    let expected = validate_json(&bytes).unwrap().canonical_sha256().to_owned();
    let binary = env!("CARGO_BIN_EXE_fn64-rdp-silicon-vectors");

    let validate = Command::new(binary)
        .arg("validate")
        .arg(&path)
        .output()
        .unwrap();
    assert!(validate.status.success());
    assert!(String::from_utf8(validate.stdout)
        .unwrap()
        .contains(&expected));

    let consensus = Command::new(binary).arg(&path).output().unwrap();
    assert!(!consensus.status.success());
    assert!(String::from_utf8(consensus.stderr)
        .unwrap()
        .contains("requires at least 10 runs"));
    fs::remove_file(path).unwrap();
}

#[test]
fn cli_analyzes_a_complete_alpha_dither_sweep() {
    let path = std::env::temp_dir().join(format!(
        "fn64-rdp-alpha-dither-{}-{}.json",
        std::process::id(),
        digest(b"cli alpha dither fixture")
    ));
    fs::write(
        &path,
        serde_json::to_vec(&alpha_dither_sweep(129, 129)).unwrap(),
    )
    .unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_fn64-rdp-silicon-vectors"))
        .arg("analyze-alpha-dither")
        .arg("sample-zero")
        .arg(&path)
        .output()
        .unwrap();
    assert!(output.status.success());
    let analysis: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(analysis["transitions"][0]["first_passing_alpha"], 129);
    assert_eq!(analysis["transitions"][1]["first_passing_alpha"], 129);
    assert_eq!(analysis["cycle_transitions_match"], true);
    fs::remove_file(path).unwrap();
}

#[test]
fn cli_analyzes_a_complete_rgb_dither_sweep() {
    let path = std::env::temp_dir().join(format!(
        "fn64-rdp-rgb-dither-{}-{}.json",
        std::process::id(),
        digest(b"cli RGB dither fixture")
    ));
    fs::write(&path, serde_json::to_vec(&rgb_dither_sweep()).unwrap()).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_fn64-rdp-silicon-vectors"))
        .arg("analyze-rgb-dither")
        .arg("magic-red-v1")
        .arg(&path)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let analysis: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(analysis["schema"], "fn64.rdp-rgb-dither-analysis.v1");
    assert_eq!(analysis["cycles"].as_array().unwrap().len(), 2);
    assert_eq!(analysis["cycles"][0]["distinct_channel_codes"], 32);
    assert_eq!(analysis["cycle_results_match"], false);
    fs::remove_file(path).unwrap();
}

#[test]
fn cli_analyzes_a_complete_alpha_coverage_sweep() {
    let path = std::env::temp_dir().join(format!(
        "fn64-rdp-alpha-coverage-{}-{}.json",
        std::process::id(),
        digest(b"cli alpha coverage fixture")
    ));
    fs::write(
        &path,
        serde_json::to_vec(&alpha_coverage_product_sweep()).unwrap(),
    )
    .unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_fn64-rdp-silicon-vectors"))
        .arg("analyze-alpha-coverage")
        .arg("product-v1")
        .arg(&path)
        .output()
        .unwrap();
    assert!(output.status.success());
    let analysis: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(analysis["curves"].as_array().unwrap().len(), 16);
    assert_eq!(analysis["curves"][7]["first_nonzero_alpha"], 16);
    assert_eq!(analysis["curves"][7]["first_full_alpha"], 240);
    assert_eq!(analysis["cycle_curves_match"], true);
    fs::remove_file(path).unwrap();
}

#[test]
fn cli_analyzes_a_complete_coverage_to_alpha_sweep() {
    let path = std::env::temp_dir().join(format!(
        "fn64-rdp-coverage-alpha-{}-{}.json",
        std::process::id(),
        digest(b"cli coverage alpha fixture")
    ));
    fs::write(
        &path,
        serde_json::to_vec(&coverage_to_alpha_sweep()).unwrap(),
    )
    .unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_fn64-rdp-silicon-vectors"))
        .arg("analyze-coverage-alpha")
        .arg("coverage-alpha-v1")
        .arg(&path)
        .output()
        .unwrap();
    assert!(output.status.success());
    let analysis: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(analysis["curves"].as_array().unwrap().len(), 16);
    assert_eq!(analysis["curves"][0]["greatest_passing_threshold"], 32);
    assert_eq!(analysis["curves"][7]["greatest_passing_threshold"], 255);
    assert_eq!(analysis["cycle_curves_match"], true);
    fs::remove_file(path).unwrap();
}

#[test]
fn cli_analyzes_a_complete_zmode_inter_sweep_with_a_stable_hash() {
    let path = std::env::temp_dir().join(format!(
        "fn64-rdp-zmode-inter-{}-{}.json",
        std::process::id(),
        digest(b"cli zmode inter fixture")
    ));
    fs::write(&path, serde_json::to_vec(&zmode_inter_sweep()).unwrap()).unwrap();
    let binary = env!("CARGO_BIN_EXE_fn64-rdp-silicon-vectors");
    let first = Command::new(binary)
        .arg("analyze-zmode-inter")
        .arg("inter-v1")
        .arg(&path)
        .output()
        .unwrap();
    let second = Command::new(binary)
        .arg("analyze-zmode-inter")
        .arg("inter-v1")
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
    assert_eq!(analysis["schema"], "fn64.rdp-zmode-inter-analysis.v1");
    assert_eq!(analysis["analysis_sha256"].as_str().unwrap().len(), 64);
    assert_eq!(analysis["relations"].as_array().unwrap().len(), 6);
    assert_eq!(analysis["cycle_results_match"], true);
    fs::remove_file(path).unwrap();
}

#[test]
fn cli_analyzes_a_complete_representative_sample_sweep_with_a_stable_hash() {
    let path = std::env::temp_dir().join(format!(
        "fn64-rdp-representative-sample-{}-{}.json",
        std::process::id(),
        digest(b"cli representative sample fixture")
    ));
    fs::write(
        &path,
        serde_json::to_vec(&representative_sample_sweep()).unwrap(),
    )
    .unwrap();
    let binary = env!("CARGO_BIN_EXE_fn64-rdp-silicon-vectors");
    let first = Command::new(binary)
        .arg("analyze-representative-sample")
        .arg("selector-v1")
        .arg(&path)
        .output()
        .unwrap();
    let second = Command::new(binary)
        .arg("analyze-representative-sample")
        .arg("selector-v1")
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
    assert_eq!(
        analysis["schema"],
        "fn64.rdp-representative-sample-analysis.v1"
    );
    assert_eq!(analysis["analysis_sha256"].as_str().unwrap().len(), 64);
    assert_eq!(analysis["tables"].as_array().unwrap().len(), 6);
    assert_eq!(analysis["all_cycle_results_match"], true);
    assert_eq!(analysis["all_observable_results_match"], true);
    fs::remove_file(path).unwrap();
}

#[test]
fn cli_analyzes_a_complete_narrow_edge_coverage_matrix_with_a_stable_hash() {
    let path = std::env::temp_dir().join(format!(
        "fn64-rdp-narrow-edge-coverage-{}-{}.json",
        std::process::id(),
        digest(b"cli narrow edge coverage fixture")
    ));
    fs::write(
        &path,
        serde_json::to_vec(&narrow_edge_coverage_sweep()).unwrap(),
    )
    .unwrap();
    let binary = env!("CARGO_BIN_EXE_fn64-rdp-silicon-vectors");
    let first = Command::new(binary)
        .arg("analyze-narrow-edge-coverage")
        .arg("narrow-edge-v1")
        .arg(&path)
        .output()
        .unwrap();
    let second = Command::new(binary)
        .arg("analyze-narrow-edge-coverage")
        .arg("narrow-edge-v1")
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
    assert_eq!(
        analysis["schema"],
        "fn64.rdp-narrow-edge-coverage-analysis.v1"
    );
    assert_eq!(analysis["analysis_sha256"].as_str().unwrap().len(), 64);
    assert_eq!(analysis["boundaries"].as_array().unwrap().len(), 2);
    assert_eq!(analysis["all_cycle_results_match"], true);
    assert_eq!(analysis["all_observable_sample_indices_match"], true);
    fs::remove_file(path).unwrap();
}

#[test]
fn cli_analyzes_a_complete_texture_filter_tie_matrix_with_a_stable_hash() {
    let path = std::env::temp_dir().join(format!(
        "fn64-rdp-texture-filter-tie-{}-{}.json",
        std::process::id(),
        digest(b"cli texture filter tie fixture")
    ));
    fs::write(
        &path,
        serde_json::to_vec(&texture_filter_tie_sweep()).unwrap(),
    )
    .unwrap();
    let binary = env!("CARGO_BIN_EXE_fn64-rdp-silicon-vectors");
    let first = Command::new(binary)
        .arg("analyze-texture-filter-tie")
        .arg("three-nearest-diagonal-v1")
        .arg(&path)
        .output()
        .unwrap();
    let second = Command::new(binary)
        .arg("analyze-texture-filter-tie")
        .arg("three-nearest-diagonal-v1")
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
    assert_eq!(
        analysis["schema"],
        "fn64.rdp-texture-filter-tie-analysis.v1"
    );
    assert_eq!(analysis["analysis_sha256"].as_str().unwrap().len(), 64);
    assert_eq!(analysis["cycles"].as_array().unwrap().len(), 2);
    assert_eq!(
        analysis["cycles"][0]["observations"]
            .as_array()
            .unwrap()
            .len(),
        3
    );
    assert_eq!(analysis["cycle_results_match"], false);
    fs::remove_file(path).unwrap();
}

#[test]
fn cli_analyzes_a_complete_reciprocal_s10_5_matrix_with_a_stable_hash() {
    let path = std::env::temp_dir().join(format!(
        "fn64-rdp-reciprocal-s10-5-{}-{}.json",
        std::process::id(),
        digest(b"cli reciprocal S10.5 fixture")
    ));
    fs::write(
        &path,
        serde_json::to_vec(&reciprocal_s10_5_boundary_sweep()).unwrap(),
    )
    .unwrap();
    let binary = env!("CARGO_BIN_EXE_fn64-rdp-silicon-vectors");
    let first = Command::new(binary)
        .arg("analyze-reciprocal-s10-5")
        .arg("reciprocal-grid-v1")
        .arg(&path)
        .output()
        .unwrap();
    let second = Command::new(binary)
        .arg("analyze-reciprocal-s10-5")
        .arg("reciprocal-grid-v1")
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
    assert_eq!(analysis["schema"], "fn64.rdp-reciprocal-s10-5-analysis.v1");
    assert_eq!(analysis["analysis_sha256"].as_str().unwrap().len(), 64);
    assert_eq!(analysis["cycles"].as_array().unwrap().len(), 2);
    assert_eq!(analysis["unexpected_output_count"], 1);
    assert_eq!(analysis["cycle_results_match"], false);
    fs::remove_file(path).unwrap();
}

#[test]
fn cli_analyzes_a_complete_average_filter_tie_matrix_with_a_stable_hash() {
    let path = std::env::temp_dir().join(format!(
        "fn64-rdp-average-filter-tie-{}-{}.json",
        std::process::id(),
        digest(b"cli average filter tie fixture")
    ));
    fs::write(
        &path,
        serde_json::to_vec(&average_filter_output_tie_sweep()).unwrap(),
    )
    .unwrap();
    let binary = env!("CARGO_BIN_EXE_fn64-rdp-silicon-vectors");
    let first = Command::new(binary)
        .arg("analyze-average-filter-tie")
        .arg("average-red-tie-v1")
        .arg(&path)
        .output()
        .unwrap();
    let second = Command::new(binary)
        .arg("analyze-average-filter-tie")
        .arg("average-red-tie-v1")
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
    assert_eq!(
        analysis["schema"],
        "fn64.rdp-average-filter-output-tie-analysis.v1"
    );
    assert_eq!(analysis["analysis_sha256"].as_str().unwrap().len(), 64);
    assert_eq!(analysis["cycles"].as_array().unwrap().len(), 2);
    assert_eq!(analysis["unexpected_output_count"], 1);
    assert_eq!(analysis["cycle_results_match"], false);
    fs::remove_file(path).unwrap();
}

#[test]
fn cli_analyzes_a_complete_texture_lod_boundary_matrix_with_a_stable_hash() {
    let path = std::env::temp_dir().join(format!(
        "fn64-rdp-texture-lod-boundary-{}-{}.json",
        std::process::id(),
        digest(b"cli texture LOD boundary fixture")
    ));
    fs::write(
        &path,
        serde_json::to_vec(&texture_lod_boundary_sweep()).unwrap(),
    )
    .unwrap();
    let binary = env!("CARGO_BIN_EXE_fn64-rdp-silicon-vectors");
    let first = Command::new(binary)
        .arg("analyze-texture-lod-boundary")
        .arg("lod-boundary-v1")
        .arg(&path)
        .output()
        .unwrap();
    let second = Command::new(binary)
        .arg("analyze-texture-lod-boundary")
        .arg("lod-boundary-v1")
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
    assert_eq!(
        analysis["schema"],
        "fn64.rdp-texture-lod-boundary-analysis.v1"
    );
    assert_eq!(analysis["analysis_sha256"].as_str().unwrap().len(), 64);
    assert_eq!(analysis["modes"].as_array().unwrap().len(), 3);
    assert_eq!(analysis["unexpected_output_count"], 1);
    assert_eq!(analysis["all_cycle_results_match"], false);
    fs::remove_file(path).unwrap();
}

#[test]
fn cli_analyzes_complete_blender_precision_matrix_with_stable_hash() {
    let path = std::env::temp_dir().join(format!(
        "fn64-rdp-blender-precision-{}-{}.json",
        std::process::id(),
        digest(b"cli blender precision fixture")
    ));
    fs::write(
        &path,
        serde_json::to_vec(&blender_precision_sweep()).unwrap(),
    )
    .unwrap();
    let binary = env!("CARGO_BIN_EXE_fn64-rdp-silicon-vectors");
    let first = Command::new(binary)
        .arg("analyze-blender-precision")
        .arg("blender-precision-v1")
        .arg(&path)
        .output()
        .unwrap();
    let second = Command::new(binary)
        .arg("analyze-blender-precision")
        .arg("blender-precision-v1")
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
    assert_eq!(analysis["schema"], "fn64.rdp-blender-precision-analysis.v1");
    assert_eq!(analysis["analysis_sha256"].as_str().unwrap().len(), 64);
    assert_eq!(analysis["modes"].as_array().unwrap().len(), 3);
    assert_eq!(analysis["feedback_pairs"].as_array().unwrap().len(), 3);
    assert_eq!(analysis["base_matrix_row_closed"], false);
    assert_eq!(analysis["total_cycle_divergence_count"], 1);
    fs::remove_file(path).unwrap();
}
