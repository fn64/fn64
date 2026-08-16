// This shared module is compiled once per binary; the exact fallback has no
// required native subtest and therefore never constructs exit status 78.
#[allow(dead_code)]
mod support;

use std::process::ExitCode;

use serde::Serialize;
use support::{DeviceContext, MetalAdapter, Status};

const SOURCE_PARTS: &[&str] = &[
    include_str!("../../Cargo.toml"),
    include_str!("../../Cargo.lock"),
    include_str!("../../build.rs"),
    include_str!("../../README.md"),
    include_str!("support/mod.rs"),
    include_str!("metal_coverage_fallback.rs"),
];
/// Public checkerboard positions from Ultra64 Programming Manual Chapter 15,
/// “Coverage Values,” in exact eighth-pixel units. This is the documented
/// sample-mask primitive, not the complete top-left/coverage pipeline or an
/// unpublished silicon edge-accumulator model.
const SAMPLE_POSITIONS_EIGHTH: [[i32; 2]; 8] = [
    [1, 1],
    [5, 1],
    [3, 3],
    [7, 3],
    [1, 5],
    [5, 5],
    [3, 7],
    [7, 7],
];
const HARDWARE_MSAA_SAMPLE_COUNT: u32 = 4;
const HARDWARE_MSAA_OUTPUT: [u8; 4] = [17, 34, 51, 255];

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
struct EdgeFixture {
    name: &'static str,
    a: i32,
    b: i32,
    c: i32,
    expected_mask: u8,
}

const EDGE_FIXTURES: [EdgeFixture; 12] = [
    EdgeFixture {
        name: "all",
        a: 0,
        b: 0,
        c: 1,
        expected_mask: 0xff,
    },
    EdgeFixture {
        name: "none",
        a: 0,
        b: 0,
        c: -1,
        expected_mask: 0x00,
    },
    EdgeFixture {
        name: "left_through_three_eighths",
        a: -1,
        b: 0,
        c: 3,
        expected_mask: 0x55,
    },
    EdgeFixture {
        name: "right_from_five_eighths",
        a: 1,
        b: 0,
        c: -5,
        expected_mask: 0xaa,
    },
    EdgeFixture {
        name: "top_through_three_eighths",
        a: 0,
        b: -1,
        c: 3,
        expected_mask: 0x0f,
    },
    EdgeFixture {
        name: "bottom_from_five_eighths",
        a: 0,
        b: 1,
        c: -5,
        expected_mask: 0xf0,
    },
    EdgeFixture {
        name: "upper_left_single_sample",
        a: -1,
        b: -1,
        c: 2,
        expected_mask: 0x01,
    },
    EdgeFixture {
        name: "lower_right_single_sample",
        a: 1,
        b: 1,
        c: -14,
        expected_mask: 0x80,
    },
    EdgeFixture {
        name: "main_diagonal_on_or_below",
        a: -1,
        b: 1,
        c: 0,
        expected_mask: 0xf5,
    },
    EdgeFixture {
        name: "main_diagonal_strictly_above",
        a: 1,
        b: -1,
        c: -1,
        expected_mask: 0x0a,
    },
    EdgeFixture {
        name: "anti_diagonal_on_upper_left",
        a: -1,
        b: -1,
        c: 8,
        expected_mask: 0x17,
    },
    EdgeFixture {
        name: "anti_diagonal_on_lower_right",
        a: 1,
        b: 1,
        c: -10,
        expected_mask: 0xe8,
    },
];

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
struct Configuration {
    authority: &'static str,
    qualification: &'static str,
    sample_positions_eighth: [[i32; 2]; 8],
    fixture_count: u32,
    edge_rule: &'static str,
    hardware_msaa_sample_count: u32,
    hardware_msaa_qualification: &'static str,
}

const CONFIGURATION: Configuration = Configuration {
    authority: "Ultra64 Programming Manual Chapter 15, Coverage Values",
    qualification: "documented_sample_mask_primitive_not_full_coverage_pipeline",
    sample_positions_eighth: SAMPLE_POSITIONS_EIGHTH,
    fixture_count: EDGE_FIXTURES.len() as u32,
    edge_rule: "a*x_eighth+b*y_eighth+c>=0",
    hardware_msaa_sample_count: HARDWARE_MSAA_SAMPLE_COUNT,
    hardware_msaa_qualification: "execution_only_not_n64_coverage_exactness",
};

#[derive(Serialize)]
struct Evidence {
    adapter_backend: Option<&'static str>,
    configuration: Configuration,
    fixtures: [EdgeFixture; 12],
    exact_shader_coverage: CoverageArm,
    hardware_msaa: HardwareMsaaArm,
    unexpected_validation_or_uncaptured_error_count: usize,
}

#[derive(Serialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
enum CoverageArm {
    Passed {
        expected_masks: [u8; 12],
        observed_masks: [u8; 12],
        expected_sha256: String,
        observed_sha256: String,
    },
    Mismatch {
        expected_masks: [u8; 12],
        observed_masks: [u8; 12],
        expected_sha256: String,
        observed_sha256: String,
    },
    Failed {
        stage: &'static str,
    },
    NotRun,
}

impl CoverageArm {
    fn passes_exact(&self) -> bool {
        let masks = EDGE_FIXTURES.map(|fixture| fixture.expected_mask);
        let digest = support::digest_chunks([expected_mask_bytes().as_slice()]);
        matches!(
            self,
            Self::Passed {
                expected_masks,
                observed_masks,
                expected_sha256,
                observed_sha256,
            } if expected_masks == &masks
                && observed_masks == &masks
                && expected_sha256 == &digest
                && observed_sha256 == &digest
        )
    }
}

#[derive(Serialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
enum HardwareMsaaArm {
    ExecutedNonExact {
        sample_count: u32,
        qualification: &'static str,
        observed_sha256: String,
        byte_count: usize,
    },
    OutputMismatchNonExact {
        sample_count: u32,
        qualification: &'static str,
        observed_sha256: String,
    },
    UnsupportedNonExactOptional {
        sample_count: u32,
        qualification: &'static str,
    },
    Failed {
        stage: &'static str,
    },
    NotRun,
}

impl HardwareMsaaArm {
    fn passes_non_exact_optional(&self) -> bool {
        let expected_digest = support::digest_chunks([HARDWARE_MSAA_OUTPUT.as_slice()]);
        matches!(
            self,
            Self::ExecutedNonExact {
                sample_count,
                qualification,
                observed_sha256,
                byte_count,
            } if *sample_count == HARDWARE_MSAA_SAMPLE_COUNT
                && *qualification == CONFIGURATION.hardware_msaa_qualification
                && observed_sha256 == &expected_digest
                && *byte_count == HARDWARE_MSAA_OUTPUT.len()
        ) || matches!(
            self,
            Self::UnsupportedNonExactOptional {
                sample_count,
                qualification,
            } if *sample_count == HARDWARE_MSAA_SAMPLE_COUNT
                && *qualification == CONFIGURATION.hardware_msaa_qualification
        )
    }
}

fn main() -> ExitCode {
    if std::env::args_os().len() != 1 {
        eprintln!("metal_coverage_fallback: this deterministic probe accepts no arguments");
        return ExitCode::from(support::EXIT_SEMANTIC_FAILURE as u8);
    }
    let output = run();
    println!("{}", output.json);
    ExitCode::from(output.exit_code as u8)
}

fn run() -> support::ProbeOutput {
    let (_instance, adapter) = match support::request_metal_adapter() {
        MetalAdapter::Available(instance, adapter) => (instance, adapter),
        MetalAdapter::Unavailable => return finish_no_adapter(),
    };
    let context = match support::request_device(
        &adapter,
        wgpu::Features::empty(),
        wgpu::Limits::default(),
        "fn64-metal-coverage-fallback",
    ) {
        Ok(context) => context,
        Err(stage) => {
            let evidence = Evidence {
                adapter_backend: Some("metal"),
                configuration: CONFIGURATION,
                fixtures: EDGE_FIXTURES,
                exact_shader_coverage: CoverageArm::Failed { stage },
                hardware_msaa: HardwareMsaaArm::NotRun,
                unexpected_validation_or_uncaptured_error_count: 0,
            };
            return support::finish(
                "fn64.m2-metal-coverage-fallback.v1",
                "metal_coverage_fallback",
                Some(&adapter),
                Status::SemanticOrValidationMismatch,
                &evidence,
                SOURCE_PARTS,
            );
        }
    };
    let exact_shader_coverage =
        coverage_arm(run_scoped(&context, || execute_exact_coverage(&context)));
    let rgba8 = adapter.get_texture_format_features(wgpu::TextureFormat::Rgba8Unorm);
    let hardware_msaa = if rgba8
        .flags
        .contains(wgpu::TextureFormatFeatureFlags::MULTISAMPLE_X4)
    {
        match run_scoped(&context, || execute_hardware_msaa(&context)) {
            Ok(bytes) if bytes == HARDWARE_MSAA_OUTPUT => HardwareMsaaArm::ExecutedNonExact {
                sample_count: HARDWARE_MSAA_SAMPLE_COUNT,
                qualification: CONFIGURATION.hardware_msaa_qualification,
                observed_sha256: support::digest_chunks([bytes.as_slice()]),
                byte_count: bytes.len(),
            },
            Ok(bytes) => HardwareMsaaArm::OutputMismatchNonExact {
                sample_count: HARDWARE_MSAA_SAMPLE_COUNT,
                qualification: CONFIGURATION.hardware_msaa_qualification,
                observed_sha256: support::digest_chunks([bytes.as_slice()]),
            },
            Err(stage) => HardwareMsaaArm::Failed { stage },
        }
    } else {
        HardwareMsaaArm::UnsupportedNonExactOptional {
            sample_count: HARDWARE_MSAA_SAMPLE_COUNT,
            qualification: CONFIGURATION.hardware_msaa_qualification,
        }
    };
    let errors = context.uncaptured_error_count();
    let evidence = Evidence {
        adapter_backend: Some("metal"),
        configuration: CONFIGURATION,
        fixtures: EDGE_FIXTURES,
        exact_shader_coverage,
        hardware_msaa,
        unexpected_validation_or_uncaptured_error_count: errors,
    };
    let status = classify(&evidence);
    support::finish(
        "fn64.m2-metal-coverage-fallback.v1",
        "metal_coverage_fallback",
        Some(&adapter),
        status,
        &evidence,
        SOURCE_PARTS,
    )
}

fn classify(evidence: &Evidence) -> Status {
    if evidence.unexpected_validation_or_uncaptured_error_count == 0
        && evidence.exact_shader_coverage.passes_exact()
        && evidence.hardware_msaa.passes_non_exact_optional()
    {
        Status::Pass
    } else {
        Status::SemanticOrValidationMismatch
    }
}

fn finish_no_adapter() -> support::ProbeOutput {
    let evidence = Evidence {
        adapter_backend: None,
        configuration: CONFIGURATION,
        fixtures: EDGE_FIXTURES,
        exact_shader_coverage: CoverageArm::NotRun,
        hardware_msaa: HardwareMsaaArm::NotRun,
        unexpected_validation_or_uncaptured_error_count: 0,
    };
    support::finish(
        "fn64.m2-metal-coverage-fallback.v1",
        "metal_coverage_fallback",
        None,
        Status::NoMetalAdapter,
        &evidence,
        SOURCE_PARTS,
    )
}

fn run_scoped(
    context: &DeviceContext,
    operation: impl FnOnce() -> Result<Vec<u8>, &'static str>,
) -> Result<Vec<u8>, &'static str> {
    let scope = context
        .device
        .push_error_scope(wgpu::ErrorFilter::Validation);
    let result = operation();
    if support::pop_validation_scope(scope) {
        Err("validation_scope")
    } else {
        result
    }
}

fn execute_exact_coverage(context: &DeviceContext) -> Result<Vec<u8>, &'static str> {
    const SHADER: &str = r#"
struct Edge {
    a: i32,
    b: i32,
    c: i32,
    padding: i32,
};

// Ultra64 Programming Manual Chapter 15, "Coverage Values". These are only
// the public sample centers; the fixture does not model the full RDP edge
// accumulator, top-left ownership, scissor, or coverage destination pipeline.
const SAMPLE_POSITIONS_EIGHTH: array<vec2<i32>, 8> = array(
    vec2<i32>(1, 1),
    vec2<i32>(5, 1),
    vec2<i32>(3, 3),
    vec2<i32>(7, 3),
    vec2<i32>(1, 5),
    vec2<i32>(5, 5),
    vec2<i32>(3, 7),
    vec2<i32>(7, 7),
);

@group(0) @binding(0) var<storage, read> edges: array<Edge>;
@group(0) @binding(1) var<storage, read_write> masks: array<u32>;

@compute @workgroup_size(12)
fn main(@builtin(global_invocation_id) gid: vec3<u32>) {
    let fixture = gid.x;
    let edge = edges[fixture];
    var mask = 0u;
    for (var sample = 0u; sample < 8u; sample++) {
        let position = SAMPLE_POSITIONS_EIGHTH[sample];
        let value = edge.a * position.x + edge.b * position.y + edge.c;
        if (value >= 0) {
            mask = mask | (1u << sample);
        }
    }
    masks[fixture] = mask;
}
"#;
    let shader = context
        .device
        .create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("fn64-exact-coverage-fallback"),
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });
    let pipeline = context
        .device
        .create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("fn64-exact-coverage-fallback"),
            layout: None,
            module: &shader,
            entry_point: Some("main"),
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            cache: None,
        });
    let mut fixture_bytes = Vec::with_capacity(EDGE_FIXTURES.len() * 16);
    for fixture in EDGE_FIXTURES {
        fixture_bytes.extend_from_slice(&fixture.a.to_le_bytes());
        fixture_bytes.extend_from_slice(&fixture.b.to_le_bytes());
        fixture_bytes.extend_from_slice(&fixture.c.to_le_bytes());
        fixture_bytes.extend_from_slice(&0i32.to_le_bytes());
    }
    let fixtures = context.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("fn64-coverage-edge-fixtures"),
        size: fixture_bytes.len() as u64,
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let output_size = (EDGE_FIXTURES.len() * 4) as u64;
    let output = context.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("fn64-coverage-masks"),
        size: output_size,
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
        mapped_at_creation: false,
    });
    let readback = context.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("fn64-coverage-readback"),
        size: output_size,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    context.queue.write_buffer(&fixtures, 0, &fixture_bytes);
    let layout = pipeline.get_bind_group_layout(0);
    let bind_group = context
        .device
        .create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("fn64-coverage-bind-group"),
            layout: &layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: fixtures.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: output.as_entire_binding(),
                },
            ],
        });
    let mut encoder = context
        .device
        .create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("fn64-exact-coverage-fallback"),
        });
    {
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("fn64-exact-coverage-fallback"),
            timestamp_writes: None,
        });
        pass.set_pipeline(&pipeline);
        pass.set_bind_group(0, &bind_group, &[]);
        pass.dispatch_workgroups(1, 1, 1);
    }
    encoder.copy_buffer_to_buffer(&output, 0, &readback, 0, output_size);
    support::submit_and_wait(&context.device, &context.queue, encoder)?;
    support::map_buffer(&context.device, &readback)
}

fn coverage_arm(result: Result<Vec<u8>, &'static str>) -> CoverageArm {
    let expected_masks = EDGE_FIXTURES.map(|fixture| fixture.expected_mask);
    let expected_bytes = expected_mask_bytes();
    match result {
        Ok(bytes) => {
            let observed_masks = match decode_masks(&bytes) {
                Ok(masks) => masks,
                Err(stage) => return CoverageArm::Failed { stage },
            };
            let observed_sha256 = support::digest_chunks([bytes.as_slice()]);
            let expected_sha256 = support::digest_chunks([expected_bytes.as_slice()]);
            if observed_masks == expected_masks {
                CoverageArm::Passed {
                    expected_masks,
                    observed_masks,
                    expected_sha256,
                    observed_sha256,
                }
            } else {
                CoverageArm::Mismatch {
                    expected_masks,
                    observed_masks,
                    expected_sha256,
                    observed_sha256,
                }
            }
        }
        Err(stage) => CoverageArm::Failed { stage },
    }
}

fn expected_mask_bytes() -> Vec<u8> {
    EDGE_FIXTURES
        .into_iter()
        .flat_map(|fixture| u32::from(fixture.expected_mask).to_le_bytes())
        .collect()
}

fn decode_masks(bytes: &[u8]) -> Result<[u8; 12], &'static str> {
    if bytes.len() != EDGE_FIXTURES.len() * 4 {
        return Err("coverage_readback_size");
    }
    let mut masks = [0u8; 12];
    for (index, chunk) in bytes.chunks_exact(4).enumerate() {
        let value = u32::from_le_bytes(chunk.try_into().expect("four-byte mask"));
        masks[index] = u8::try_from(value).map_err(|_| "coverage_mask_upper_bits")?;
    }
    Ok(masks)
}

fn execute_hardware_msaa(context: &DeviceContext) -> Result<Vec<u8>, &'static str> {
    const SHADER: &str = r#"
@vertex
fn vs_main(@builtin(vertex_index) vertex_index: u32) -> @builtin(position) vec4<f32> {
    let positions = array<vec2<f32>, 3>(
        vec2<f32>(-1.0, -1.0),
        vec2<f32>(3.0, -1.0),
        vec2<f32>(-1.0, 3.0),
    );
    return vec4<f32>(positions[vertex_index], 0.0, 1.0);
}

@fragment
fn fs_main() -> @location(0) vec4<f32> {
    return vec4<f32>(17.0 / 255.0, 34.0 / 255.0, 51.0 / 255.0, 1.0);
}
"#;
    let shader = context
        .device
        .create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("fn64-hardware-msaa-non-exact"),
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });
    let targets = [Some(wgpu::ColorTargetState {
        format: wgpu::TextureFormat::Rgba8Unorm,
        blend: None,
        write_mask: wgpu::ColorWrites::ALL,
    })];
    let pipeline = context
        .device
        .create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("fn64-hardware-msaa-non-exact"),
            layout: None,
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                buffers: &[],
            },
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState {
                count: HARDWARE_MSAA_SAMPLE_COUNT,
                ..wgpu::MultisampleState::default()
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                targets: &targets,
            }),
            multiview_mask: None,
            cache: None,
        });
    let extent = wgpu::Extent3d {
        width: 1,
        height: 1,
        depth_or_array_layers: 1,
    };
    let multisampled = context.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("fn64-hardware-msaa-target"),
        size: extent,
        mip_level_count: 1,
        sample_count: HARDWARE_MSAA_SAMPLE_COUNT,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        view_formats: &[],
    });
    let resolved = context.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("fn64-hardware-msaa-resolve"),
        size: extent,
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let multisampled_view = multisampled.create_view(&wgpu::TextureViewDescriptor::default());
    let resolved_view = resolved.create_view(&wgpu::TextureViewDescriptor::default());
    let readback = context.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("fn64-hardware-msaa-readback"),
        size: 256,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = context
        .device
        .create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("fn64-hardware-msaa-non-exact"),
        });
    {
        let attachments = [Some(wgpu::RenderPassColorAttachment {
            view: &multisampled_view,
            depth_slice: None,
            resolve_target: Some(&resolved_view),
            ops: wgpu::Operations {
                load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                store: wgpu::StoreOp::Discard,
            },
        })];
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("fn64-hardware-msaa-non-exact"),
            color_attachments: &attachments,
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        pass.set_pipeline(&pipeline);
        pass.draw(0..3, 0..1);
    }
    encoder.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture: &resolved,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyBufferInfo {
            buffer: &readback,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(256),
                rows_per_image: Some(1),
            },
        },
        extent,
    );
    support::submit_and_wait(&context.device, &context.queue, encoder)?;
    let mapped = support::map_buffer(&context.device, &readback)?;
    Ok(mapped[..4].to_vec())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn passed_coverage() -> CoverageArm {
        coverage_arm(Ok(expected_mask_bytes()))
    }

    fn executed_hardware_msaa() -> HardwareMsaaArm {
        HardwareMsaaArm::ExecutedNonExact {
            sample_count: HARDWARE_MSAA_SAMPLE_COUNT,
            qualification: CONFIGURATION.hardware_msaa_qualification,
            observed_sha256: support::digest_chunks([HARDWARE_MSAA_OUTPUT.as_slice()]),
            byte_count: HARDWARE_MSAA_OUTPUT.len(),
        }
    }

    fn evidence(exact_shader_coverage: CoverageArm, hardware_msaa: HardwareMsaaArm) -> Evidence {
        Evidence {
            adapter_backend: Some("metal"),
            configuration: CONFIGURATION,
            fixtures: EDGE_FIXTURES,
            exact_shader_coverage,
            hardware_msaa,
            unexpected_validation_or_uncaptured_error_count: 0,
        }
    }

    fn cpu_mask(fixture: EdgeFixture) -> u8 {
        SAMPLE_POSITIONS_EIGHTH
            .iter()
            .enumerate()
            .fold(0u8, |mask, (index, position)| {
                let value = fixture.a * position[0] + fixture.b * position[1] + fixture.c;
                if value >= 0 {
                    mask | (1 << index)
                } else {
                    mask
                }
            })
    }

    fn encoded_expected_masks() -> Vec<u8> {
        expected_mask_bytes()
    }

    #[test]
    fn fixed_masks_independently_match_documented_positions() {
        assert_eq!(EDGE_FIXTURES.len(), 12);
        for fixture in EDGE_FIXTURES {
            assert_eq!(cpu_mask(fixture), fixture.expected_mask, "{}", fixture.name);
        }
        assert_eq!(EDGE_FIXTURES[2].expected_mask, 0x55);
        assert_eq!(EDGE_FIXTURES[6].expected_mask, 0x01);
        assert_eq!(
            EDGE_FIXTURES[8].expected_mask | EDGE_FIXTURES[9].expected_mask,
            0xff
        );
        assert_eq!(
            EDGE_FIXTURES[10].expected_mask | EDGE_FIXTURES[11].expected_mask,
            0xff
        );
    }

    #[test]
    fn hostile_mask_and_geometry_mutations_cannot_pass() {
        assert!(matches!(
            coverage_arm(Ok(encoded_expected_masks())),
            CoverageArm::Passed { .. }
        ));
        let mut wrong_mask = encoded_expected_masks();
        wrong_mask[0] ^= 1;
        assert!(matches!(
            coverage_arm(Ok(wrong_mask)),
            CoverageArm::Mismatch { .. }
        ));
        assert!(matches!(
            coverage_arm(Ok(encoded_expected_masks()[..44].to_vec())),
            CoverageArm::Failed {
                stage: "coverage_readback_size"
            }
        ));
        let mut upper_bits = encoded_expected_masks();
        upper_bits[1] = 1;
        assert!(matches!(
            coverage_arm(Ok(upper_bits)),
            CoverageArm::Failed {
                stage: "coverage_mask_upper_bits"
            }
        ));
    }

    #[test]
    fn hardware_msaa_receipt_can_never_claim_exact_n64_coverage() {
        let executed = HardwareMsaaArm::ExecutedNonExact {
            sample_count: 4,
            qualification: CONFIGURATION.hardware_msaa_qualification,
            observed_sha256: "digest".into(),
            byte_count: 4,
        };
        let value = serde_json::to_value(executed).unwrap();
        assert_eq!(value["outcome"], "executed_non_exact");
        assert_eq!(
            value["qualification"],
            "execution_only_not_n64_coverage_exactness"
        );
        assert!(value.get("expected_masks").is_none());
    }

    #[test]
    fn coverage_and_hardware_outcome_table_rejects_every_nonpositive_state() {
        let unsupported = || HardwareMsaaArm::UnsupportedNonExactOptional {
            sample_count: HARDWARE_MSAA_SAMPLE_COUNT,
            qualification: CONFIGURATION.hardware_msaa_qualification,
        };
        assert_eq!(
            classify(&evidence(passed_coverage(), executed_hardware_msaa())),
            Status::Pass
        );
        assert_eq!(
            classify(&evidence(passed_coverage(), unsupported())),
            Status::Pass
        );

        let mut wrong_mask = expected_mask_bytes();
        wrong_mask[0] ^= 1;
        for coverage in [
            CoverageArm::NotRun,
            CoverageArm::Failed {
                stage: "coverage_execution",
            },
            coverage_arm(Ok(wrong_mask)),
        ] {
            assert_eq!(
                classify(&evidence(coverage, unsupported())),
                Status::SemanticOrValidationMismatch
            );
        }

        for hardware in [
            HardwareMsaaArm::NotRun,
            HardwareMsaaArm::Failed {
                stage: "hardware_msaa_execution",
            },
            HardwareMsaaArm::OutputMismatchNonExact {
                sample_count: HARDWARE_MSAA_SAMPLE_COUNT,
                qualification: CONFIGURATION.hardware_msaa_qualification,
                observed_sha256: "wrong".into(),
            },
        ] {
            assert_eq!(
                classify(&evidence(passed_coverage(), hardware)),
                Status::SemanticOrValidationMismatch
            );
        }
    }

    #[test]
    fn inexact_pass_fields_and_nonzero_errors_are_rejected() {
        let masks = EDGE_FIXTURES.map(|fixture| fixture.expected_mask);
        let mut wrong_coverage = evidence(
            CoverageArm::Passed {
                expected_masks: masks,
                observed_masks: masks,
                expected_sha256: "wrong".into(),
                observed_sha256: "wrong".into(),
            },
            executed_hardware_msaa(),
        );
        assert_eq!(
            classify(&wrong_coverage),
            Status::SemanticOrValidationMismatch
        );

        wrong_coverage = evidence(
            passed_coverage(),
            HardwareMsaaArm::ExecutedNonExact {
                sample_count: 8,
                qualification: CONFIGURATION.hardware_msaa_qualification,
                observed_sha256: support::digest_chunks([HARDWARE_MSAA_OUTPUT.as_slice()]),
                byte_count: HARDWARE_MSAA_OUTPUT.len(),
            },
        );
        assert_eq!(
            classify(&wrong_coverage),
            Status::SemanticOrValidationMismatch
        );

        let mut errors = evidence(passed_coverage(), executed_hardware_msaa());
        errors.unexpected_validation_or_uncaptured_error_count = 1;
        assert_eq!(classify(&errors), Status::SemanticOrValidationMismatch);
    }

    #[test]
    fn receipt_hash_binds_coverage_configuration() {
        let evidence = Evidence {
            adapter_backend: None,
            configuration: CONFIGURATION,
            fixtures: EDGE_FIXTURES,
            exact_shader_coverage: CoverageArm::NotRun,
            hardware_msaa: HardwareMsaaArm::NotRun,
            unexpected_validation_or_uncaptured_error_count: 0,
        };
        let first = support::finish(
            "fn64.test.coverage.v1",
            "metal_coverage_fallback",
            None,
            Status::NoMetalAdapter,
            &evidence,
            SOURCE_PARTS,
        );
        let mut mutated = evidence;
        mutated.configuration.sample_positions_eighth[0] = [3, 1];
        let second = support::finish(
            "fn64.test.coverage.v1",
            "metal_coverage_fallback",
            None,
            Status::NoMetalAdapter,
            &mutated,
            SOURCE_PARTS,
        );
        let first: serde_json::Value = serde_json::from_str(&first.json).unwrap();
        let second: serde_json::Value = serde_json::from_str(&second.json).unwrap();
        assert_ne!(first["canonical_sha256"], second["canonical_sha256"]);
    }
}
