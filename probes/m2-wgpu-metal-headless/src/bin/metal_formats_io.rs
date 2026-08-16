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
    include_str!("metal_formats_io.rs"),
];
const INPUT_WIDTH: u32 = 8;
const OUTPUT_WIDTH: u32 = 4;
const HEIGHT: u32 = 2;
const STAGING_ROW_BYTES: u32 = 256;
const GUEST_RGBA5551: [u8; 16] = [
    0xf8, 0x01, 0x07, 0xc1, 0x00, 0x3f, 0x84, 0x20, 0xff, 0xff, 0x00, 0x00, 0x42, 0x11, 0x7b, 0xdf,
];

#[derive(Serialize)]
struct Evidence {
    adapter_backend: Option<&'static str>,
    formats: FormatAdvertisements,
    guest_packed_buffer_fallback: ByteArm,
    r8uint_to_rgba8unorm_conversion: ByteArm,
    staging_readback: StagingEvidence,
    incompatible_view_reinterpretation: RejectionArm,
    unexpected_validation_or_uncaptured_error_count: usize,
}

#[derive(Default, Serialize)]
struct FormatAdvertisements {
    r8uint_copy_dst_and_texture_binding: bool,
    rgba8unorm_storage_binding_and_copy_src: bool,
    rgba8unorm_storage_write: bool,
    rgba8unorm_copy_src_for_view_validation: bool,
}

#[derive(Serialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
enum ByteArm {
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

impl ByteArm {
    fn is_failure(&self) -> bool {
        matches!(self, Self::Mismatch { .. } | Self::Failed { .. })
    }

    fn is_unsupported(&self) -> bool {
        matches!(self, Self::Unsupported { .. })
    }
}

#[derive(Serialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
enum StagingEvidence {
    NotRun,
    Failed {
        stage: &'static str,
    },
    Passed {
        bytes_per_row: u32,
        rows: u32,
        allocated_bytes: u32,
        logical_bytes_compared: u32,
    },
}

#[derive(Serialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
enum RejectionArm {
    RejectedUnderValidationScope {
        base_format: &'static str,
        requested_view_format: &'static str,
    },
    AcceptedUnexpectedly,
    Unsupported {
        missing_advertisement: &'static str,
    },
    NotRun,
}

impl RejectionArm {
    fn is_failure(&self) -> bool {
        matches!(self, Self::AcceptedUnexpectedly)
    }

    fn is_unsupported(&self) -> bool {
        matches!(self, Self::Unsupported { .. })
    }
}

fn main() -> ExitCode {
    if std::env::args_os().len() != 1 {
        eprintln!("metal_formats_io: this deterministic probe accepts no arguments");
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
                formats: FormatAdvertisements::default(),
                guest_packed_buffer_fallback: ByteArm::NotRun,
                r8uint_to_rgba8unorm_conversion: ByteArm::NotRun,
                staging_readback: StagingEvidence::NotRun,
                incompatible_view_reinterpretation: RejectionArm::NotRun,
                unexpected_validation_or_uncaptured_error_count: 0,
            };
            return support::finish(
                "fn64.m2-metal-formats-io.v1",
                "metal_formats_io",
                None,
                Status::NoMetalAdapter,
                &evidence,
                SOURCE_PARTS,
            );
        }
    };

    let r8 = adapter.get_texture_format_features(wgpu::TextureFormat::R8Uint);
    let rgba8 = adapter.get_texture_format_features(wgpu::TextureFormat::Rgba8Unorm);
    let formats = FormatAdvertisements {
        r8uint_copy_dst_and_texture_binding: r8
            .allowed_usages
            .contains(wgpu::TextureUsages::COPY_DST | wgpu::TextureUsages::TEXTURE_BINDING),
        rgba8unorm_storage_binding_and_copy_src: rgba8
            .allowed_usages
            .contains(wgpu::TextureUsages::STORAGE_BINDING | wgpu::TextureUsages::COPY_SRC),
        rgba8unorm_storage_write: rgba8
            .flags
            .contains(wgpu::TextureFormatFeatureFlags::STORAGE_WRITE_ONLY),
        rgba8unorm_copy_src_for_view_validation: rgba8
            .allowed_usages
            .contains(wgpu::TextureUsages::COPY_SRC),
    };
    let conversion_supported = formats.r8uint_copy_dst_and_texture_binding
        && formats.rgba8unorm_storage_binding_and_copy_src
        && formats.rgba8unorm_storage_write;
    let requested_features = if adapter
        .features()
        .contains(wgpu::Features::TEXTURE_ADAPTER_SPECIFIC_FORMAT_FEATURES)
    {
        wgpu::Features::TEXTURE_ADAPTER_SPECIFIC_FORMAT_FEATURES
    } else {
        wgpu::Features::empty()
    };
    let context = match support::request_device(
        &adapter,
        requested_features,
        wgpu::Limits::default(),
        "fn64-metal-formats-io",
    ) {
        Ok(context) => context,
        Err(stage) => {
            let evidence = Evidence {
                adapter_backend: Some("metal"),
                formats,
                guest_packed_buffer_fallback: ByteArm::Failed { stage },
                r8uint_to_rgba8unorm_conversion: ByteArm::NotRun,
                staging_readback: StagingEvidence::NotRun,
                incompatible_view_reinterpretation: RejectionArm::NotRun,
                unexpected_validation_or_uncaptured_error_count: 0,
            };
            return support::finish(
                "fn64.m2-metal-formats-io.v1",
                "metal_formats_io",
                Some(&adapter),
                Status::SemanticOrValidationMismatch,
                &evidence,
                SOURCE_PARTS,
            );
        }
    };

    let expected = expected_rgba8();
    let guest_packed_buffer_fallback = byte_arm(
        &expected,
        run_scoped(&context, || execute_buffer_fallback(&context)),
    );
    let (r8uint_to_rgba8unorm_conversion, staging_readback) = if conversion_supported {
        let conversion = run_scoped(&context, || execute_texture_conversion(&context));
        let staging_readback = staging_evidence(&conversion);
        (byte_arm(&expected, conversion), staging_readback)
    } else {
        (
            ByteArm::Unsupported {
                missing_advertisements: missing_conversion_advertisements(&formats),
            },
            StagingEvidence::NotRun,
        )
    };
    let incompatible_view_reinterpretation = if formats.rgba8unorm_copy_src_for_view_validation {
        execute_incompatible_view_rejection(&context)
    } else {
        RejectionArm::Unsupported {
            missing_advertisement: "format:rgba8unorm:copy_src",
        }
    };
    let runtime_errors = context.uncaptured_error_count();
    let evidence = Evidence {
        adapter_backend: Some("metal"),
        formats,
        guest_packed_buffer_fallback,
        r8uint_to_rgba8unorm_conversion,
        staging_readback,
        incompatible_view_reinterpretation,
        unexpected_validation_or_uncaptured_error_count: runtime_errors,
    };
    let byte_failure = evidence.guest_packed_buffer_fallback.is_failure()
        || evidence.r8uint_to_rgba8unorm_conversion.is_failure();
    let unsupported = evidence.r8uint_to_rgba8unorm_conversion.is_unsupported()
        || evidence.incompatible_view_reinterpretation.is_unsupported();
    let status = classify(
        runtime_errors,
        byte_failure,
        unsupported,
        evidence.incompatible_view_reinterpretation.is_failure(),
    );
    support::finish(
        "fn64.m2-metal-formats-io.v1",
        "metal_formats_io",
        Some(&adapter),
        status,
        &evidence,
        SOURCE_PARTS,
    )
}

fn classify(
    runtime_errors: usize,
    byte_failure: bool,
    unsupported: bool,
    rejection_failure: bool,
) -> Status {
    if runtime_errors != 0 || byte_failure || rejection_failure {
        Status::SemanticOrValidationMismatch
    } else if unsupported {
        Status::ExplicitlyUnsupportedNativeSubtest
    } else {
        Status::Pass
    }
}

fn missing_conversion_advertisements(formats: &FormatAdvertisements) -> Vec<&'static str> {
    let mut missing = Vec::new();
    if !formats.r8uint_copy_dst_and_texture_binding {
        missing.push("format:r8uint:copy_dst+texture_binding");
    }
    if !formats.rgba8unorm_storage_binding_and_copy_src {
        missing.push("format:rgba8unorm:storage_binding+copy_src");
    }
    if !formats.rgba8unorm_storage_write {
        missing.push("format:rgba8unorm:storage_write");
    }
    missing
}

fn staging_evidence(result: &Result<Vec<u8>, &'static str>) -> StagingEvidence {
    match result {
        Ok(bytes) if bytes.len() == (OUTPUT_WIDTH * HEIGHT * 4) as usize => {
            StagingEvidence::Passed {
                bytes_per_row: STAGING_ROW_BYTES,
                rows: HEIGHT,
                allocated_bytes: STAGING_ROW_BYTES * HEIGHT,
                logical_bytes_compared: OUTPUT_WIDTH * HEIGHT * 4,
            }
        }
        Ok(_) => StagingEvidence::Failed {
            stage: "logical_row_size",
        },
        Err(stage) => StagingEvidence::Failed { stage },
    }
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

fn byte_arm(expected: &[u8], result: Result<Vec<u8>, &'static str>) -> ByteArm {
    match result {
        Ok(observed) => {
            let expected_sha256 = support::digest_chunks([expected]);
            let observed_sha256 = support::digest_chunks([observed.as_slice()]);
            if observed == expected {
                ByteArm::Passed {
                    expected_sha256,
                    observed_sha256,
                    byte_count: expected.len(),
                }
            } else {
                ByteArm::Mismatch {
                    expected_sha256,
                    observed_sha256,
                    byte_count: expected.len(),
                }
            }
        }
        Err(stage) => ByteArm::Failed { stage },
    }
}

fn execute_buffer_fallback(context: &DeviceContext) -> Result<Vec<u8>, &'static str> {
    const SHADER: &str = r#"
@group(0) @binding(0) var<storage, read> guest_words: array<u32>;
@group(0) @binding(1) var<storage, read_write> output: array<u32>;

fn guest_byte(address: u32) -> u32 {
    return (guest_words[address >> 2u] >> ((address & 3u) * 8u)) & 0xffu;
}

fn expand5(value: u32) -> u32 {
    return (value << 3u) | (value >> 2u);
}

@compute @workgroup_size(4, 2)
fn main(@builtin(global_invocation_id) gid: vec3<u32>) {
    let pixel = gid.y * 4u + gid.x;
    let address = pixel * 2u;
    let packed = (guest_byte(address) << 8u) | guest_byte(address + 1u);
    let r = expand5((packed >> 11u) & 31u);
    let g = expand5((packed >> 6u) & 31u);
    let b = expand5((packed >> 1u) & 31u);
    let a = (packed & 1u) * 255u;
    output[pixel] = r | (g << 8u) | (b << 16u) | (a << 24u);
}
"#;
    let shader = context
        .device
        .create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("fn64-formats-buffer-fallback"),
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });
    let pipeline = context
        .device
        .create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("fn64-formats-buffer-fallback"),
            layout: None,
            module: &shader,
            entry_point: Some("main"),
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            cache: None,
        });
    let input = context.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("fn64-guest-packed-rgba5551"),
        size: GUEST_RGBA5551.len() as u64,
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let output_size = u64::from(OUTPUT_WIDTH * HEIGHT * 4);
    let output = context.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("fn64-formats-buffer-output"),
        size: output_size,
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
        mapped_at_creation: false,
    });
    let readback = context.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("fn64-formats-buffer-readback"),
        size: output_size,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    context.queue.write_buffer(&input, 0, &GUEST_RGBA5551);
    let layout = pipeline.get_bind_group_layout(0);
    let bind_group = context
        .device
        .create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("fn64-formats-buffer-bind-group"),
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
        .create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("fn64-formats-buffer-fallback"),
        });
    {
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("fn64-formats-buffer-fallback"),
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

fn execute_texture_conversion(context: &DeviceContext) -> Result<Vec<u8>, &'static str> {
    const SHADER: &str = r#"
@group(0) @binding(0) var guest_bytes: texture_2d<u32>;
@group(0) @binding(1) var converted: texture_storage_2d<rgba8unorm, write>;

fn expand5(value: u32) -> u32 {
    return (value << 3u) | (value >> 2u);
}

@compute @workgroup_size(4, 2)
fn main(@builtin(global_invocation_id) gid: vec3<u32>) {
    let byte_x = gid.x * 2u;
    let high = textureLoad(guest_bytes, vec2<u32>(byte_x, gid.y), 0).r;
    let low = textureLoad(guest_bytes, vec2<u32>(byte_x + 1u, gid.y), 0).r;
    let packed = (high << 8u) | low;
    let rgba = vec4<u32>(
        expand5((packed >> 11u) & 31u),
        expand5((packed >> 6u) & 31u),
        expand5((packed >> 1u) & 31u),
        (packed & 1u) * 255u,
    );
    textureStore(converted, vec2<u32>(gid.xy), vec4<f32>(rgba) / 255.0);
}
"#;
    let input_extent = wgpu::Extent3d {
        width: INPUT_WIDTH,
        height: HEIGHT,
        depth_or_array_layers: 1,
    };
    let output_extent = wgpu::Extent3d {
        width: OUTPUT_WIDTH,
        height: HEIGHT,
        depth_or_array_layers: 1,
    };
    let input = context.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("fn64-guest-rgba5551-byte-texture"),
        size: input_extent,
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::R8Uint,
        usage: wgpu::TextureUsages::COPY_DST | wgpu::TextureUsages::TEXTURE_BINDING,
        view_formats: &[],
    });
    let output = context.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("fn64-converted-rgba8unorm"),
        size: output_extent,
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::STORAGE_BINDING | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    context.queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture: &input,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        &GUEST_RGBA5551,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(INPUT_WIDTH),
            rows_per_image: Some(HEIGHT),
        },
        input_extent,
    );
    let shader = context
        .device
        .create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("fn64-format-conversion"),
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });
    let pipeline = context
        .device
        .create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("fn64-format-conversion"),
            layout: None,
            module: &shader,
            entry_point: Some("main"),
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            cache: None,
        });
    let input_view = input.create_view(&wgpu::TextureViewDescriptor::default());
    let output_view = output.create_view(&wgpu::TextureViewDescriptor::default());
    let layout = pipeline.get_bind_group_layout(0);
    let bind_group = context
        .device
        .create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("fn64-format-conversion-bind-group"),
            layout: &layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&input_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&output_view),
                },
            ],
        });
    let readback_size = u64::from(STAGING_ROW_BYTES * HEIGHT);
    let readback = context.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("fn64-format-padded-readback"),
        size: readback_size,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = context
        .device
        .create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("fn64-format-conversion"),
        });
    {
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("fn64-format-conversion"),
            timestamp_writes: None,
        });
        pass.set_pipeline(&pipeline);
        pass.set_bind_group(0, &bind_group, &[]);
        pass.dispatch_workgroups(1, 1, 1);
    }
    encoder.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture: &output,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyBufferInfo {
            buffer: &readback,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(STAGING_ROW_BYTES),
                rows_per_image: Some(HEIGHT),
            },
        },
        output_extent,
    );
    support::submit_and_wait(&context.device, &context.queue, encoder)?;
    let padded = support::map_buffer(&context.device, &readback)?;
    extract_logical_rows(&padded)
}

fn execute_incompatible_view_rejection(context: &DeviceContext) -> RejectionArm {
    let texture = context.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("fn64-view-rejection-base"),
        size: wgpu::Extent3d {
            width: 1,
            height: 1,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let scope = context
        .device
        .push_error_scope(wgpu::ErrorFilter::Validation);
    let _invalid = texture.create_view(&wgpu::TextureViewDescriptor {
        label: Some("fn64-incompatible-r32uint-view"),
        format: Some(wgpu::TextureFormat::R32Uint),
        ..wgpu::TextureViewDescriptor::default()
    });
    if support::pop_validation_scope(scope) {
        RejectionArm::RejectedUnderValidationScope {
            base_format: "rgba8unorm",
            requested_view_format: "r32uint",
        }
    } else {
        RejectionArm::AcceptedUnexpectedly
    }
}

fn extract_logical_rows(padded: &[u8]) -> Result<Vec<u8>, &'static str> {
    let expected_size = (STAGING_ROW_BYTES * HEIGHT) as usize;
    if padded.len() != expected_size {
        return Err("staging_size");
    }
    let logical_row_bytes = (OUTPUT_WIDTH * 4) as usize;
    let mut logical = Vec::with_capacity(logical_row_bytes * HEIGHT as usize);
    for row in 0..HEIGHT as usize {
        let start = row * STAGING_ROW_BYTES as usize;
        logical.extend_from_slice(&padded[start..start + logical_row_bytes]);
    }
    Ok(logical)
}

fn expected_rgba8() -> Vec<u8> {
    GUEST_RGBA5551
        .chunks_exact(2)
        .flat_map(|bytes| decode_rgba5551(u16::from_be_bytes([bytes[0], bytes[1]])))
        .collect()
}

fn decode_rgba5551(packed: u16) -> [u8; 4] {
    fn expand5(value: u16) -> u8 {
        ((value << 3) | (value >> 2)) as u8
    }
    [
        expand5((packed >> 11) & 31),
        expand5((packed >> 6) & 31),
        expand5((packed >> 1) & 31),
        if packed & 1 != 0 { 255 } else { 0 },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn guest_big_endian_rgba5551_sentinels_decode_exactly() {
        let expected = expected_rgba8();
        assert_eq!(expected.len(), 32);
        assert_eq!(&expected[0..4], &[255, 0, 0, 255]);
        assert_eq!(&expected[4..8], &[0, 255, 0, 255]);
        assert_eq!(&expected[8..12], &[0, 0, 255, 255]);
        assert_eq!(&expected[16..20], &[255, 255, 255, 255]);
        assert_eq!(
            support::digest_chunks([expected.as_slice()]),
            "eb3b30dd83699a3a5165418198896b1a7e7462f00a37050bdf1252c71ef626b9"
        );
    }

    #[test]
    fn padded_rows_extract_only_logical_texture_bytes() {
        let mut padded = vec![0xa5; (STAGING_ROW_BYTES * HEIGHT) as usize];
        padded[..16].copy_from_slice(&[1; 16]);
        padded[STAGING_ROW_BYTES as usize..STAGING_ROW_BYTES as usize + 16]
            .copy_from_slice(&[2; 16]);
        assert_eq!(
            extract_logical_rows(&padded).unwrap(),
            [[1; 16], [2; 16]].concat()
        );
        assert!(extract_logical_rows(&padded[..511]).is_err());
    }

    #[test]
    fn fallback_failure_has_semantic_failure_precedence() {
        assert_eq!(
            classify(0, false, true, false),
            Status::ExplicitlyUnsupportedNativeSubtest
        );
        assert_eq!(
            classify(0, true, true, false),
            Status::SemanticOrValidationMismatch
        );
        assert_eq!(
            classify(0, false, true, true),
            Status::SemanticOrValidationMismatch
        );
        assert_eq!(
            classify(1, false, true, false),
            Status::SemanticOrValidationMismatch
        );
    }

    #[test]
    fn staging_geometry_exists_only_after_successful_readback() {
        let not_run = serde_json::to_value(StagingEvidence::NotRun).unwrap();
        assert_eq!(not_run["outcome"], "not_run");
        assert!(not_run.get("bytes_per_row").is_none());

        let map_failure = staging_evidence(&Err("map_poll"));
        let failed = serde_json::to_value(map_failure).unwrap();
        assert_eq!(failed["outcome"], "failed");
        assert_eq!(failed["stage"], "map_poll");
        assert!(failed.get("bytes_per_row").is_none());

        let wrong_size = staging_evidence(&Ok(vec![0; 31]));
        let wrong_size = serde_json::to_value(wrong_size).unwrap();
        assert_eq!(wrong_size["outcome"], "failed");
        assert_eq!(wrong_size["stage"], "logical_row_size");
        assert!(wrong_size.get("allocated_bytes").is_none());

        let passed = serde_json::to_value(staging_evidence(&Ok(vec![0; 32]))).unwrap();
        assert_eq!(passed["outcome"], "passed");
        assert_eq!(passed["bytes_per_row"], STAGING_ROW_BYTES);
        assert_eq!(passed["allocated_bytes"], STAGING_ROW_BYTES * HEIGHT);
    }
}
