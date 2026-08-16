mod support;

use std::num::NonZeroU32;
use std::process::ExitCode;

use serde::Serialize;
use support::{DeviceContext, MetalAdapter, Status};

const SOURCE_PARTS: &[&str] = &[
    include_str!("../../Cargo.toml"),
    include_str!("../../Cargo.lock"),
    include_str!("../../build.rs"),
    include_str!("../../README.md"),
    include_str!("support/mod.rs"),
    include_str!("metal_semantics.rs"),
];
const OUTPUT_WORDS: u32 = 8;
const TMEM_BYTES: usize = 4096;
const EXPECTED_BLEND_BYTES: [u8; 4] = [64, 60, 89, 94];
const BINDING_ARRAY_FEATURES: wgpu::Features = wgpu::Features::BUFFER_BINDING_ARRAY
    .union(wgpu::Features::STORAGE_RESOURCE_BINDING_ARRAY)
    .union(wgpu::Features::SAMPLED_TEXTURE_AND_STORAGE_BUFFER_ARRAY_NON_UNIFORM_INDEXING);

#[derive(Serialize)]
struct Evidence {
    adapter_backend: Option<&'static str>,
    advertised: Advertised,
    exact_u32_and_tmem: ArmReceipt,
    binding_array_native: ArmReceipt,
    binding_array_packed_fallback: ArmReceipt,
    dual_source_native: ArmReceipt,
    manual_blend_fallback: ArmReceipt,
    unexpected_validation_or_uncaptured_error_count: usize,
}

#[derive(Default, Serialize)]
struct Advertised {
    buffer_binding_array: bool,
    storage_resource_binding_array: bool,
    storage_buffer_array_non_uniform_indexing: bool,
    binding_array_elements_per_stage_at_least_4: bool,
    dual_source_blending: bool,
    rgba8unorm_render_and_copy: bool,
}

#[derive(Serialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
enum ArmReceipt {
    Passed {
        expected_sha256: String,
        observed_sha256: String,
        byte_count: usize,
    },
    Mismatch {
        expected_sha256: String,
        observed_sha256: String,
        byte_count: usize,
    },
    Failed {
        stage: &'static str,
    },
    Unsupported {
        missing_advertisements: Vec<&'static str>,
    },
    NotRun,
}

impl ArmReceipt {
    fn is_failure(&self) -> bool {
        matches!(self, Self::Mismatch { .. } | Self::Failed { .. })
    }

    fn is_unsupported(&self) -> bool {
        matches!(self, Self::Unsupported { .. })
    }
}

fn main() -> ExitCode {
    if std::env::args_os().len() != 1 {
        eprintln!("metal_semantics: this deterministic probe accepts no arguments");
        return ExitCode::from(support::EXIT_SEMANTIC_FAILURE as u8);
    }
    let output = run();
    println!("{}", output.json);
    ExitCode::from(output.exit_code as u8)
}

fn run() -> support::ProbeOutput {
    let (_instance, adapter) = match support::request_metal_adapter() {
        MetalAdapter::Available(instance, adapter) => (instance, adapter),
        MetalAdapter::Unavailable => {
            let evidence = Evidence {
                adapter_backend: None,
                advertised: Advertised::default(),
                exact_u32_and_tmem: ArmReceipt::NotRun,
                binding_array_native: ArmReceipt::NotRun,
                binding_array_packed_fallback: ArmReceipt::NotRun,
                dual_source_native: ArmReceipt::NotRun,
                manual_blend_fallback: ArmReceipt::NotRun,
                unexpected_validation_or_uncaptured_error_count: 0,
            };
            return support::finish(
                "fn64.m2-metal-semantics.v1",
                "metal_semantics",
                None,
                Status::NoMetalAdapter,
                &evidence,
                SOURCE_PARTS,
            );
        }
    };

    let features = adapter.features();
    let limits = adapter.limits();
    let rgba_features = adapter.get_texture_format_features(wgpu::TextureFormat::Rgba8Unorm);
    let advertised = Advertised {
        buffer_binding_array: features.contains(wgpu::Features::BUFFER_BINDING_ARRAY),
        storage_resource_binding_array: features
            .contains(wgpu::Features::STORAGE_RESOURCE_BINDING_ARRAY),
        storage_buffer_array_non_uniform_indexing: features.contains(
            wgpu::Features::SAMPLED_TEXTURE_AND_STORAGE_BUFFER_ARRAY_NON_UNIFORM_INDEXING,
        ),
        binding_array_elements_per_stage_at_least_4: limits
            .max_binding_array_elements_per_shader_stage
            >= 4,
        dual_source_blending: features.contains(wgpu::Features::DUAL_SOURCE_BLENDING),
        rgba8unorm_render_and_copy: rgba_features
            .allowed_usages
            .contains(wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC),
    };
    let binding_native_supported = features.contains(BINDING_ARRAY_FEATURES)
        && advertised.binding_array_elements_per_stage_at_least_4;
    let dual_native_supported =
        advertised.dual_source_blending && advertised.rgba8unorm_render_and_copy;
    let mut requested = wgpu::Features::empty();
    if binding_native_supported {
        requested |= BINDING_ARRAY_FEATURES;
    }
    if dual_native_supported {
        requested |= wgpu::Features::DUAL_SOURCE_BLENDING;
    }
    let mut required_limits = wgpu::Limits::default();
    if binding_native_supported {
        required_limits.max_binding_array_elements_per_shader_stage = 4;
    }
    let context =
        match support::request_device(&adapter, requested, required_limits, "fn64-metal-semantics")
        {
            Ok(context) => context,
            Err(stage) => {
                let evidence = Evidence {
                    adapter_backend: Some("metal"),
                    advertised,
                    exact_u32_and_tmem: ArmReceipt::Failed { stage },
                    binding_array_native: ArmReceipt::NotRun,
                    binding_array_packed_fallback: ArmReceipt::NotRun,
                    dual_source_native: ArmReceipt::NotRun,
                    manual_blend_fallback: ArmReceipt::NotRun,
                    unexpected_validation_or_uncaptured_error_count: 0,
                };
                return support::finish(
                    "fn64.m2-metal-semantics.v1",
                    "metal_semantics",
                    Some(&adapter),
                    Status::SemanticOrValidationMismatch,
                    &evidence,
                    SOURCE_PARTS,
                );
            }
        };

    let integer_expected = expected_integer_tmem_bytes();
    let exact_u32_and_tmem = arm_receipt(
        &integer_expected,
        run_scoped(&context, || execute_integer_tmem(&context)),
    );
    let binding_expected = expected_binding_bytes();
    let binding_array_packed_fallback = arm_receipt(
        &binding_expected,
        run_scoped(&context, || execute_packed_binding_fallback(&context)),
    );
    let binding_array_native = if binding_native_supported {
        arm_receipt(
            &binding_expected,
            run_scoped(&context, || execute_binding_array(&context)),
        )
    } else {
        ArmReceipt::Unsupported {
            missing_advertisements: binding_missing(&advertised),
        }
    };
    let blend_expected = EXPECTED_BLEND_BYTES;
    let manual_blend_fallback = arm_receipt(
        &blend_expected,
        run_scoped(&context, || execute_manual_blend(&context)),
    );
    let dual_source_native = if dual_native_supported {
        arm_receipt(
            &blend_expected,
            run_scoped(&context, || execute_dual_source(&context)),
        )
    } else {
        let mut missing = Vec::new();
        if !advertised.dual_source_blending {
            missing.push("feature:dual_source_blending");
        }
        if !advertised.rgba8unorm_render_and_copy {
            missing.push("format:rgba8unorm:render_attachment+copy_src");
        }
        ArmReceipt::Unsupported {
            missing_advertisements: missing,
        }
    };

    let runtime_errors = context.uncaptured_error_count();
    let evidence = Evidence {
        adapter_backend: Some("metal"),
        advertised,
        exact_u32_and_tmem,
        binding_array_native,
        binding_array_packed_fallback,
        dual_source_native,
        manual_blend_fallback,
        unexpected_validation_or_uncaptured_error_count: runtime_errors,
    };
    let arms = [
        &evidence.exact_u32_and_tmem,
        &evidence.binding_array_native,
        &evidence.binding_array_packed_fallback,
        &evidence.dual_source_native,
        &evidence.manual_blend_fallback,
    ];
    let status = classify(runtime_errors, &arms);
    support::finish(
        "fn64.m2-metal-semantics.v1",
        "metal_semantics",
        Some(&adapter),
        status,
        &evidence,
        SOURCE_PARTS,
    )
}

fn classify(runtime_errors: usize, arms: &[&ArmReceipt]) -> Status {
    if runtime_errors != 0 || arms.iter().any(|arm| arm.is_failure()) {
        Status::SemanticOrValidationMismatch
    } else if arms.iter().any(|arm| arm.is_unsupported()) {
        Status::ExplicitlyUnsupportedNativeSubtest
    } else {
        Status::Pass
    }
}

fn binding_missing(advertised: &Advertised) -> Vec<&'static str> {
    let mut missing = Vec::new();
    if !advertised.buffer_binding_array {
        missing.push("feature:buffer_binding_array");
    }
    if !advertised.storage_resource_binding_array {
        missing.push("feature:storage_resource_binding_array");
    }
    if !advertised.storage_buffer_array_non_uniform_indexing {
        missing.push("feature:storage_buffer_array_non_uniform_indexing");
    }
    if !advertised.binding_array_elements_per_stage_at_least_4 {
        missing.push("limit:max_binding_array_elements_per_shader_stage<4");
    }
    missing
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

fn arm_receipt(expected: &[u8], result: Result<Vec<u8>, &'static str>) -> ArmReceipt {
    match result {
        Ok(observed) => {
            let expected_sha256 = support::digest_chunks([expected]);
            let observed_sha256 = support::digest_chunks([observed.as_slice()]);
            if observed == expected {
                ArmReceipt::Passed {
                    expected_sha256,
                    observed_sha256,
                    byte_count: expected.len(),
                }
            } else {
                ArmReceipt::Mismatch {
                    expected_sha256,
                    observed_sha256,
                    byte_count: expected.len(),
                }
            }
        }
        Err(stage) => ArmReceipt::Failed { stage },
    }
}

fn execute_integer_tmem(context: &DeviceContext) -> Result<Vec<u8>, &'static str> {
    const SHADER: &str = r#"
@group(0) @binding(0) var<storage, read> tmem: array<u32>;
@group(0) @binding(1) var<storage, read_write> output: array<u32>;

@compute @workgroup_size(8)
fn main(@builtin(global_invocation_id) gid: vec3<u32>) {
    let lane = gid.x;
    let word = 0x81234567u ^ (lane * 0x10204081u);
    let shift = ((lane * 5u) + 3u) & 31u;
    let rotated = (word >> shift) | (word << (32u - shift));
    let wrapped = (word * 0x9e3779b9u) + 0xfedcba98u;
    let masked = (wrapped & 0x0ff00ff0u) ^ rotated;

    let x = ((lane * 13u) + 5u) & 63u;
    let y = ((lane * 7u) + 3u) & 31u;
    let logical_address = (0x3c0u + (y * 16u) + (x >> 1u)) & 0xfffu;
    let odd_row_swap = select(0u, 4u, (y & 1u) != 0u);
    let physical_address = logical_address ^ odd_row_swap;
    let packed_byte = (tmem[physical_address >> 2u] >> ((physical_address & 3u) * 8u)) & 0xffu;
    let texel = select(packed_byte >> 4u, packed_byte & 0x0fu, (x & 1u) != 0u);
    output[lane] = masked ^ (physical_address << 16u) ^ (texel << 28u);
}
"#;
    let tmem = tmem_bytes();
    execute_regular_compute(
        context,
        "fn64-u32-tmem",
        SHADER,
        &tmem,
        (OUTPUT_WORDS * 4) as u64,
        OUTPUT_WORDS,
    )
}

fn execute_packed_binding_fallback(context: &DeviceContext) -> Result<Vec<u8>, &'static str> {
    const SHADER: &str = r#"
@group(0) @binding(0) var<storage, read> packed: array<u32>;
@group(0) @binding(1) var<storage, read_write> output: array<u32>;

@compute @workgroup_size(8)
fn main(@builtin(global_invocation_id) gid: vec3<u32>) {
    let lane = gid.x;
    let selector = ((lane * 3u) + 1u) & 3u;
    output[lane] = packed[selector] ^ (lane * 0x01010101u);
}
"#;
    let mut input = Vec::new();
    for word in binding_words() {
        input.extend_from_slice(&word.to_le_bytes());
    }
    execute_regular_compute(
        context,
        "fn64-packed-binding-fallback",
        SHADER,
        &input,
        (OUTPUT_WORDS * 4) as u64,
        OUTPUT_WORDS,
    )
}

fn execute_regular_compute(
    context: &DeviceContext,
    label: &'static str,
    shader_source: &'static str,
    input_bytes: &[u8],
    output_size: u64,
    invocation_count: u32,
) -> Result<Vec<u8>, &'static str> {
    let shader = context
        .device
        .create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some(label),
            source: wgpu::ShaderSource::Wgsl(shader_source.into()),
        });
    let pipeline = context
        .device
        .create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some(label),
            layout: None,
            module: &shader,
            entry_point: Some("main"),
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            cache: None,
        });
    let input = context.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("fn64-compute-input"),
        size: input_bytes.len() as u64,
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let output = context.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("fn64-compute-output"),
        size: output_size,
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
        mapped_at_creation: false,
    });
    let readback = context.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("fn64-compute-readback"),
        size: output_size,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    context.queue.write_buffer(&input, 0, input_bytes);
    let layout = pipeline.get_bind_group_layout(0);
    let bind_group = context
        .device
        .create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("fn64-compute-bind-group"),
            layout: &layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: input.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: output.as_entire_binding(),
                },
            ],
        });
    let mut encoder = context
        .device
        .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some(label) });
    {
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some(label),
            timestamp_writes: None,
        });
        pass.set_pipeline(&pipeline);
        pass.set_bind_group(0, &bind_group, &[]);
        pass.dispatch_workgroups(invocation_count.div_ceil(8), 1, 1);
    }
    encoder.copy_buffer_to_buffer(&output, 0, &readback, 0, output_size);
    support::submit_and_wait(&context.device, &context.queue, encoder)?;
    support::map_buffer(&context.device, &readback)
}

fn execute_binding_array(context: &DeviceContext) -> Result<Vec<u8>, &'static str> {
    const SHADER: &str = r#"
enable wgpu_binding_array;

struct Input {
    value: u32,
};

@group(0) @binding(0) var<storage, read> inputs: binding_array<Input, 4>;
@group(0) @binding(1) var<storage, read_write> output: array<u32>;

@compute @workgroup_size(8)
fn main(@builtin(global_invocation_id) gid: vec3<u32>) {
    let lane = gid.x;
    let selector = ((lane * 3u) + 1u) & 3u;
    output[lane] = inputs[selector].value ^ (lane * 0x01010101u);
}
"#;
    let shader = context
        .device
        .create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("fn64-binding-array"),
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });
    let layout = context
        .device
        .create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("fn64-binding-array-layout"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: true },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: NonZeroU32::new(4),
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: false },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ],
        });
    let pipeline_layout = context
        .device
        .create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("fn64-binding-array-pipeline-layout"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
    let pipeline = context
        .device
        .create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("fn64-binding-array"),
            layout: Some(&pipeline_layout),
            module: &shader,
            entry_point: Some("main"),
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            cache: None,
        });
    let words = binding_words();
    let buffers: Vec<_> = words
        .iter()
        .map(|word| {
            let buffer = context.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("fn64-binding-array-element"),
                size: 4,
                usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            context.queue.write_buffer(&buffer, 0, &word.to_le_bytes());
            buffer
        })
        .collect();
    let bindings: Vec<_> = buffers
        .iter()
        .map(|buffer| wgpu::BufferBinding {
            buffer,
            offset: 0,
            size: None,
        })
        .collect();
    let output = context.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("fn64-binding-array-output"),
        size: (OUTPUT_WORDS * 4) as u64,
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
        mapped_at_creation: false,
    });
    let readback = context.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("fn64-binding-array-readback"),
        size: (OUTPUT_WORDS * 4) as u64,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let bind_group = context
        .device
        .create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("fn64-binding-array-bind-group"),
            layout: &layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::BufferArray(&bindings),
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
            label: Some("fn64-binding-array"),
        });
    {
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("fn64-binding-array"),
            timestamp_writes: None,
        });
        pass.set_pipeline(&pipeline);
        pass.set_bind_group(0, &bind_group, &[]);
        pass.dispatch_workgroups(1, 1, 1);
    }
    encoder.copy_buffer_to_buffer(&output, 0, &readback, 0, (OUTPUT_WORDS * 4) as u64);
    support::submit_and_wait(&context.device, &context.queue, encoder)?;
    support::map_buffer(&context.device, &readback)
}

fn execute_manual_blend(context: &DeviceContext) -> Result<Vec<u8>, &'static str> {
    const SHADER: &str = r#"
@group(0) @binding(0) var<storage, read> input: array<u32>;
@group(0) @binding(1) var<storage, read_write> output: array<u32>;

fn channel(value: u32, shift: u32) -> u32 {
    return (value >> shift) & 0xffu;
}

@compute @workgroup_size(1)
fn main() {
    let source = input[0];
    let source1 = input[1];
    let destination = input[2];
    var packed = 0u;
    for (var component = 0u; component < 4u; component++) {
        let shift = component * 8u;
        let factor = channel(source1, shift);
        let numerator = channel(source, shift) * factor
            + channel(destination, shift) * (255u - factor) + 127u;
        packed = packed | ((numerator / 255u) << shift);
    }
    output[0] = packed;
}
"#;
    let mut input = Vec::new();
    for word in blend_words() {
        input.extend_from_slice(&word.to_le_bytes());
    }
    execute_regular_compute(context, "fn64-manual-blend", SHADER, &input, 4, 1)
}

fn execute_dual_source(context: &DeviceContext) -> Result<Vec<u8>, &'static str> {
    const SHADER: &str = r#"
enable dual_source_blending;

@vertex
fn vs_main(@builtin(vertex_index) vertex_index: u32) -> @builtin(position) vec4<f32> {
    let positions = array<vec2<f32>, 3>(
        vec2<f32>(-1.0, -1.0),
        vec2<f32>(3.0, -1.0),
        vec2<f32>(-1.0, 3.0),
    );
    return vec4<f32>(positions[vertex_index], 0.0, 1.0);
}

struct FragmentOutput {
    @location(0) @blend_src(0) source: vec4<f32>,
    @location(0) @blend_src(1) factor: vec4<f32>,
};

@fragment
fn fs_main() -> FragmentOutput {
    return FragmentOutput(
        vec4<f32>(204.0 / 255.0, 85.0 / 255.0, 102.0 / 255.0, 136.0 / 255.0),
        vec4<f32>(64.0 / 255.0, 128.0 / 255.0, 192.0 / 255.0, 96.0 / 255.0),
    );
}
"#;
    let shader = context
        .device
        .create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("fn64-dual-source"),
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });
    let targets = [Some(wgpu::ColorTargetState {
        format: wgpu::TextureFormat::Rgba8Unorm,
        blend: Some(wgpu::BlendState {
            color: wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::Src1,
                dst_factor: wgpu::BlendFactor::OneMinusSrc1,
                operation: wgpu::BlendOperation::Add,
            },
            alpha: wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::Src1Alpha,
                dst_factor: wgpu::BlendFactor::OneMinusSrc1Alpha,
                operation: wgpu::BlendOperation::Add,
            },
        }),
        write_mask: wgpu::ColorWrites::ALL,
    })];
    let pipeline = context
        .device
        .create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("fn64-dual-source"),
            layout: None,
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                buffers: &[],
            },
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                targets: &targets,
            }),
            multiview_mask: None,
            cache: None,
        });
    let texture = context.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("fn64-dual-source-target"),
        size: wgpu::Extent3d {
            width: 1,
            height: 1,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
    let readback = context.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("fn64-dual-source-readback"),
        size: 256,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = context
        .device
        .create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("fn64-dual-source"),
        });
    {
        let color_attachments = [Some(wgpu::RenderPassColorAttachment {
            view: &view,
            depth_slice: None,
            resolve_target: None,
            ops: wgpu::Operations {
                load: wgpu::LoadOp::Clear(wgpu::Color {
                    r: 17.0 / 255.0,
                    g: 34.0 / 255.0,
                    b: 51.0 / 255.0,
                    a: 68.0 / 255.0,
                }),
                store: wgpu::StoreOp::Store,
            },
        })];
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("fn64-dual-source"),
            color_attachments: &color_attachments,
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
            texture: &texture,
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
        wgpu::Extent3d {
            width: 1,
            height: 1,
            depth_or_array_layers: 1,
        },
    );
    support::submit_and_wait(&context.device, &context.queue, encoder)?;
    let mapped = support::map_buffer(&context.device, &readback)?;
    Ok(mapped[..4].to_vec())
}

fn tmem_bytes() -> Vec<u8> {
    (0..TMEM_BYTES)
        .map(|address| (((address as u32 * 37 + 11) ^ 0xa5) & 0xff) as u8)
        .collect()
}

fn expected_integer_tmem_bytes() -> Vec<u8> {
    let tmem = tmem_bytes();
    let mut expected = Vec::new();
    for lane in 0..OUTPUT_WORDS {
        let word = 0x8123_4567_u32 ^ lane.wrapping_mul(0x1020_4081);
        let shift = (lane.wrapping_mul(5).wrapping_add(3)) & 31;
        let rotated = word.rotate_right(shift);
        let wrapped = word.wrapping_mul(0x9e37_79b9).wrapping_add(0xfedc_ba98);
        let masked = (wrapped & 0x0ff0_0ff0) ^ rotated;
        let x = (lane.wrapping_mul(13).wrapping_add(5)) & 63;
        let y = (lane.wrapping_mul(7).wrapping_add(3)) & 31;
        let logical_address = (0x3c0 + y * 16 + (x >> 1)) & 0xfff;
        let odd_row_swap = if y & 1 != 0 { 4 } else { 0 };
        let physical_address = logical_address ^ odd_row_swap;
        let packed_byte = tmem[physical_address as usize];
        let texel = if x & 1 != 0 {
            packed_byte & 0x0f
        } else {
            packed_byte >> 4
        } as u32;
        let result = masked ^ (physical_address << 16) ^ (texel << 28);
        expected.extend_from_slice(&result.to_le_bytes());
    }
    expected
}

const fn binding_words() -> [u32; 4] {
    [0x0123_4567, 0x89ab_cdef, 0x55aa_33cc, 0xf00d_cafe]
}

fn expected_binding_bytes() -> Vec<u8> {
    let words = binding_words();
    let mut expected = Vec::new();
    for lane in 0..OUTPUT_WORDS {
        let selector = ((lane * 3 + 1) & 3) as usize;
        let value = words[selector] ^ lane.wrapping_mul(0x0101_0101);
        expected.extend_from_slice(&value.to_le_bytes());
    }
    expected
}

const fn pack_rgba(bytes: [u8; 4]) -> u32 {
    u32::from_le_bytes(bytes)
}

const fn blend_words() -> [u32; 3] {
    [
        pack_rgba([204, 85, 102, 136]),
        pack_rgba([64, 128, 192, 96]),
        pack_rgba([17, 34, 51, 68]),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_expected_vectors_are_stable() {
        assert_eq!(expected_integer_tmem_bytes().len(), 32);
        assert_eq!(
            support::digest_chunks([expected_integer_tmem_bytes().as_slice()]),
            "51f8f52940eed2dc84c4f43dae61dd605850cfff4ec624c3716f8f31dac23910"
        );
        assert_eq!(expected_binding_bytes().len(), 32);
        assert_eq!(EXPECTED_BLEND_BYTES, [64, 60, 89, 94]);
        assert!(
            blend_words()[1]
                .to_le_bytes()
                .into_iter()
                .all(|factor| factor != 0 && factor != 255)
        );
    }

    #[test]
    fn fallback_failure_is_never_unsupported() {
        let unsupported = ArmReceipt::Unsupported {
            missing_advertisements: vec!["feature:native"],
        };
        let failed_fallback = ArmReceipt::Failed { stage: "fallback" };
        assert_eq!(
            classify(0, &[&unsupported]),
            Status::ExplicitlyUnsupportedNativeSubtest
        );
        assert_eq!(
            classify(0, &[&unsupported, &failed_fallback]),
            Status::SemanticOrValidationMismatch
        );
        assert_eq!(
            classify(1, &[&unsupported]),
            Status::SemanticOrValidationMismatch
        );
    }
}
