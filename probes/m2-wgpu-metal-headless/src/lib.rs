use std::collections::BTreeMap;
use std::fmt;
use std::fs;
use std::path::Path;
use std::process::Command;
use std::sync::{Arc, Mutex, mpsc};

use serde::Serialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

pub const EXIT_PASS: i32 = 0;
pub const EXIT_SEMANTIC_FAILURE: i32 = 2;
pub const EXIT_NO_ADAPTER: i32 = 69;
pub const EXIT_UNSUPPORTED: i32 = 78;

const WGPU_VERSION: &str = "30.0.0";
const WGPU_FEATURES: &[&str] = &["metal", "wgsl"];
const SOURCE_PARTS: &[&str] = &[
    include_str!("../Cargo.toml"),
    include_str!("../Cargo.lock"),
    include_str!("../build.rs"),
    include_str!("lib.rs"),
    include_str!("main.rs"),
    include_str!("../README.md"),
];

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Cli {
    pub require: bool,
    pub iterations: u32,
}

impl Cli {
    pub fn parse<I, S>(args: I) -> Result<Self, String>
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        let mut require = false;
        let mut iterations = 1_u32;
        let mut args = args.into_iter().map(Into::into);
        while let Some(arg) = args.next() {
            match arg.as_str() {
                "--require" => require = true,
                "--iterations" => {
                    let value = args
                        .next()
                        .ok_or_else(|| "--iterations requires an integer".to_string())?;
                    iterations = value.parse().map_err(|_| {
                        "--iterations must be an integer from 1 through 100".to_string()
                    })?;
                    if !(1..=100).contains(&iterations) {
                        return Err("--iterations must be an integer from 1 through 100".into());
                    }
                }
                "-h" | "--help" => return Err(usage()),
                _ => return Err(format!("unknown argument {arg:?}\n{}", usage())),
            }
        }
        Ok(Self {
            require,
            iterations,
        })
    }

    pub fn canonical_command(&self) -> Vec<String> {
        let mut command = vec!["metal_caps".to_string()];
        if self.require {
            command.push("--require".to_string());
        }
        command.push("--iterations".to_string());
        command.push(self.iterations.to_string());
        command
    }
}

fn usage() -> String {
    "usage: metal_caps [--require] [--iterations 1..100]".into()
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Category {
    Pass,
    ObservedUnsupported,
    ExplicitlyUnsupported,
    NoAdapter,
    SemanticOrValidationFailure,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Assessment {
    pub category: Category,
    pub exit_code: i32,
    pub missing_required: Vec<String>,
}

pub fn assess(
    require: bool,
    adapter_found: bool,
    failures: Vec<String>,
    semantic_ok: bool,
) -> Assessment {
    if !adapter_found {
        return Assessment {
            category: Category::NoAdapter,
            exit_code: EXIT_NO_ADAPTER,
            missing_required: Vec::new(),
        };
    }
    if !semantic_ok {
        return Assessment {
            category: Category::SemanticOrValidationFailure,
            exit_code: EXIT_SEMANTIC_FAILURE,
            missing_required: failures,
        };
    }
    if failures.is_empty() {
        Assessment {
            category: Category::Pass,
            exit_code: EXIT_PASS,
            missing_required: failures,
        }
    } else if require {
        Assessment {
            category: Category::ExplicitlyUnsupported,
            exit_code: EXIT_UNSUPPORTED,
            missing_required: failures,
        }
    } else {
        Assessment {
            category: Category::ObservedUnsupported,
            exit_code: EXIT_PASS,
            missing_required: failures,
        }
    }
}

#[derive(Clone, Copy)]
struct FeatureRequirement {
    name: &'static str,
    flag: wgpu::Features,
    required: bool,
}

const FEATURE_REQUIREMENTS: &[FeatureRequirement] = &[
    FeatureRequirement {
        name: "dual_source_blending",
        flag: wgpu::Features::DUAL_SOURCE_BLENDING,
        required: true,
    },
    FeatureRequirement {
        name: "texture_binding_array",
        flag: wgpu::Features::TEXTURE_BINDING_ARRAY,
        required: true,
    },
    FeatureRequirement {
        name: "buffer_binding_array",
        flag: wgpu::Features::BUFFER_BINDING_ARRAY,
        required: true,
    },
    FeatureRequirement {
        name: "storage_resource_binding_array",
        flag: wgpu::Features::STORAGE_RESOURCE_BINDING_ARRAY,
        required: true,
    },
    FeatureRequirement {
        name: "sampled_texture_and_storage_buffer_array_non_uniform_indexing",
        flag: wgpu::Features::SAMPLED_TEXTURE_AND_STORAGE_BUFFER_ARRAY_NON_UNIFORM_INDEXING,
        required: true,
    },
    FeatureRequirement {
        name: "storage_texture_array_non_uniform_indexing",
        flag: wgpu::Features::STORAGE_TEXTURE_ARRAY_NON_UNIFORM_INDEXING,
        required: true,
    },
    FeatureRequirement {
        name: "partially_bound_binding_array",
        flag: wgpu::Features::PARTIALLY_BOUND_BINDING_ARRAY,
        required: true,
    },
    FeatureRequirement {
        name: "timestamp_query",
        flag: wgpu::Features::TIMESTAMP_QUERY,
        required: true,
    },
    FeatureRequirement {
        name: "timestamp_query_inside_encoders",
        flag: wgpu::Features::TIMESTAMP_QUERY_INSIDE_ENCODERS,
        required: false,
    },
    FeatureRequirement {
        name: "timestamp_query_inside_passes",
        flag: wgpu::Features::TIMESTAMP_QUERY_INSIDE_PASSES,
        required: false,
    },
    FeatureRequirement {
        name: "texture_adapter_specific_format_features",
        flag: wgpu::Features::TEXTURE_ADAPTER_SPECIFIC_FORMAT_FEATURES,
        required: true,
    },
];

#[derive(Clone, Copy)]
struct FormatRequirement {
    name: &'static str,
    format: wgpu::TextureFormat,
    usages: wgpu::TextureUsages,
    storage_read: bool,
    storage_write: bool,
    storage_read_write: bool,
}

const COPY_SAMPLE_STORAGE: wgpu::TextureUsages = wgpu::TextureUsages::COPY_SRC
    .union(wgpu::TextureUsages::COPY_DST)
    .union(wgpu::TextureUsages::TEXTURE_BINDING)
    .union(wgpu::TextureUsages::STORAGE_BINDING);
const COPY_SAMPLE_STORAGE_RENDER: wgpu::TextureUsages =
    COPY_SAMPLE_STORAGE.union(wgpu::TextureUsages::RENDER_ATTACHMENT);
const COPY_SAMPLE_RENDER: wgpu::TextureUsages = wgpu::TextureUsages::COPY_SRC
    .union(wgpu::TextureUsages::COPY_DST)
    .union(wgpu::TextureUsages::TEXTURE_BINDING)
    .union(wgpu::TextureUsages::RENDER_ATTACHMENT);

const FORMAT_REQUIREMENTS: &[FormatRequirement] = &[
    FormatRequirement {
        name: "r8uint",
        format: wgpu::TextureFormat::R8Uint,
        usages: COPY_SAMPLE_STORAGE_RENDER,
        storage_read: true,
        storage_write: true,
        storage_read_write: false,
    },
    FormatRequirement {
        name: "r16uint",
        format: wgpu::TextureFormat::R16Uint,
        usages: COPY_SAMPLE_STORAGE_RENDER,
        storage_read: true,
        storage_write: true,
        storage_read_write: false,
    },
    FormatRequirement {
        name: "r32uint",
        format: wgpu::TextureFormat::R32Uint,
        usages: COPY_SAMPLE_STORAGE_RENDER,
        storage_read: true,
        storage_write: true,
        storage_read_write: false,
    },
    FormatRequirement {
        name: "r32float",
        format: wgpu::TextureFormat::R32Float,
        usages: COPY_SAMPLE_STORAGE_RENDER,
        storage_read: true,
        storage_write: true,
        storage_read_write: false,
    },
    FormatRequirement {
        name: "rg8uint",
        format: wgpu::TextureFormat::Rg8Uint,
        usages: COPY_SAMPLE_STORAGE_RENDER,
        storage_read: true,
        storage_write: true,
        storage_read_write: false,
    },
    FormatRequirement {
        name: "rg16uint",
        format: wgpu::TextureFormat::Rg16Uint,
        usages: COPY_SAMPLE_STORAGE_RENDER,
        storage_read: true,
        storage_write: true,
        storage_read_write: false,
    },
    FormatRequirement {
        name: "rg32uint",
        format: wgpu::TextureFormat::Rg32Uint,
        usages: COPY_SAMPLE_STORAGE,
        storage_read: true,
        storage_write: true,
        storage_read_write: false,
    },
    FormatRequirement {
        name: "rgba8unorm",
        format: wgpu::TextureFormat::Rgba8Unorm,
        usages: COPY_SAMPLE_STORAGE_RENDER,
        storage_read: true,
        storage_write: true,
        storage_read_write: false,
    },
    FormatRequirement {
        name: "rgba8uint",
        format: wgpu::TextureFormat::Rgba8Uint,
        usages: COPY_SAMPLE_STORAGE_RENDER,
        storage_read: true,
        storage_write: true,
        storage_read_write: false,
    },
    FormatRequirement {
        name: "rgba16uint",
        format: wgpu::TextureFormat::Rgba16Uint,
        usages: COPY_SAMPLE_STORAGE_RENDER,
        storage_read: true,
        storage_write: true,
        storage_read_write: false,
    },
    FormatRequirement {
        name: "rgba16float",
        format: wgpu::TextureFormat::Rgba16Float,
        usages: COPY_SAMPLE_STORAGE_RENDER,
        storage_read: true,
        storage_write: true,
        storage_read_write: false,
    },
    FormatRequirement {
        name: "rgba32uint",
        format: wgpu::TextureFormat::Rgba32Uint,
        usages: COPY_SAMPLE_STORAGE,
        storage_read: true,
        storage_write: true,
        storage_read_write: false,
    },
    FormatRequirement {
        name: "depth32float",
        format: wgpu::TextureFormat::Depth32Float,
        usages: COPY_SAMPLE_RENDER,
        storage_read: false,
        storage_write: false,
        storage_read_write: false,
    },
];

#[derive(Serialize)]
struct FeatureRow {
    required: bool,
    advertised: bool,
}

#[derive(Serialize)]
struct LimitRow {
    observed: u64,
    required_minimum: u64,
    supported: bool,
}

#[derive(Serialize)]
struct FormatRow {
    required_usages: Vec<&'static str>,
    allowed_usages: Vec<&'static str>,
    required_storage_access: Vec<&'static str>,
    advertised_storage_access: Vec<&'static str>,
    supported: bool,
}

#[derive(Default, Serialize)]
struct SemanticEvidence {
    iterations_requested: u32,
    iterations_completed: u32,
    baseline_buffer_copy_and_map: &'static str,
    capability_specific_operations: &'static str,
    validation_errors: Vec<String>,
    uncaptured_errors: Vec<String>,
}

#[derive(Serialize)]
struct AdapterReceipt {
    name: String,
    vendor: u32,
    device: u32,
    device_type: String,
    driver: String,
    driver_info: String,
    backend: String,
    subgroup_min_size: u32,
    subgroup_max_size: u32,
    device_pci_bus_id: String,
    transient_saves_memory: Option<bool>,
    limit_bucket: Option<String>,
}

pub struct ProbeOutput {
    pub json: String,
    pub exit_code: i32,
}

pub fn run(cli: &Cli) -> ProbeOutput {
    let source_sha256 = digest_chunks(SOURCE_PARTS.iter().map(|part| part.as_bytes()));
    let binary_sha256 = std::env::current_exe()
        .ok()
        .and_then(|path| digest_file(&path).ok());

    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
        backends: wgpu::Backends::METAL,
        flags: wgpu::InstanceFlags::VALIDATION,
        ..wgpu::InstanceDescriptor::new_without_display_handle()
    });
    let adapter_result = block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::HighPerformance,
        force_fallback_adapter: false,
        compatible_surface: None,
        apply_limit_buckets: false,
    }));

    let adapter = match adapter_result {
        Ok(adapter) => adapter,
        Err(error) => {
            let assessment = assess(cli.require, false, Vec::new(), true);
            return finish_receipt(
                cli,
                assessment,
                source_sha256,
                binary_sha256,
                None,
                None,
                None,
                None,
                Some(error.to_string()),
            );
        }
    };

    let info = adapter.get_info();
    if info.backend != wgpu::Backend::Metal {
        let assessment = assess(cli.require, true, Vec::new(), false);
        let semantic = SemanticEvidence {
            iterations_requested: cli.iterations,
            baseline_buffer_copy_and_map: "failed",
            validation_errors: vec![format!(
                "requested METAL but adapter reported {:?}",
                info.backend
            )],
            capability_specific_operations: "not_exercised; this probe separates advertisements from executed semantics",
            ..SemanticEvidence::default()
        };
        return finish_receipt(
            cli,
            assessment,
            source_sha256,
            binary_sha256,
            Some(adapter_receipt(info)),
            None,
            None,
            Some(semantic),
            None,
        );
    }

    let supported_features = adapter.features();
    let mut missing = Vec::new();
    let feature_rows: BTreeMap<_, _> = FEATURE_REQUIREMENTS
        .iter()
        .map(|requirement| {
            let advertised = supported_features.contains(requirement.flag);
            if requirement.required && !advertised {
                missing.push(format!("feature:{}", requirement.name));
            }
            (
                requirement.name,
                FeatureRow {
                    required: requirement.required,
                    advertised,
                },
            )
        })
        .collect();

    let limits = adapter.limits();
    let limit_rows = limit_matrix(&limits);
    for (name, row) in &limit_rows {
        if !row.supported {
            missing.push(format!("limit:{name}"));
        }
    }
    let mut format_rows = BTreeMap::new();
    for requirement in FORMAT_REQUIREMENTS {
        let features = adapter.get_texture_format_features(requirement.format);
        let usages_ok = features.allowed_usages.contains(requirement.usages);
        let read_ok = !requirement.storage_read
            || features
                .flags
                .contains(wgpu::TextureFormatFeatureFlags::STORAGE_READ_ONLY);
        let write_ok = !requirement.storage_write
            || features
                .flags
                .contains(wgpu::TextureFormatFeatureFlags::STORAGE_WRITE_ONLY);
        let read_write_ok = !requirement.storage_read_write
            || features
                .flags
                .contains(wgpu::TextureFormatFeatureFlags::STORAGE_READ_WRITE);
        let supported = usages_ok && read_ok && write_ok && read_write_ok;
        if !supported {
            missing.push(format!("format:{}", requirement.name));
        }
        format_rows.insert(
            requirement.name,
            FormatRow {
                required_usages: usage_names(requirement.usages),
                allowed_usages: usage_names(features.allowed_usages),
                required_storage_access: storage_names(
                    requirement.storage_read,
                    requirement.storage_write,
                    requirement.storage_read_write,
                ),
                advertised_storage_access: storage_flag_names(features.flags),
                supported,
            },
        );
    }
    missing.sort();

    let semantic = execute_baseline(&adapter, supported_features, cli.iterations);
    let semantic_ok = semantic.iterations_completed == semantic.iterations_requested
        && semantic.validation_errors.is_empty()
        && semantic.uncaptured_errors.is_empty();
    let assessment = assess(cli.require, true, missing, semantic_ok);
    finish_receipt(
        cli,
        assessment,
        source_sha256,
        binary_sha256,
        Some(adapter_receipt(info)),
        Some(feature_rows),
        Some(limit_rows),
        Some(semantic),
        None,
    )
    .with_formats(format_rows)
}

trait WithFormats {
    fn with_formats(self, formats: BTreeMap<&'static str, FormatRow>) -> Self;
}

impl WithFormats for ProbeOutput {
    fn with_formats(mut self, formats: BTreeMap<&'static str, FormatRow>) -> Self {
        let mut value: Value =
            serde_json::from_str(&self.json).expect("internally generated receipt");
        value
            .as_object_mut()
            .unwrap()
            .insert("formats".into(), serde_json::to_value(formats).unwrap());
        value.as_object_mut().unwrap().remove("canonical_sha256");
        let body = serde_json::to_vec(&value).unwrap();
        value.as_object_mut().unwrap().insert(
            "canonical_sha256".into(),
            Value::String(digest_chunks([body.as_slice()])),
        );
        self.json = serde_json::to_string(&value).unwrap();
        self
    }
}

#[allow(clippy::too_many_arguments)]
fn finish_receipt(
    cli: &Cli,
    assessment: Assessment,
    source_sha256: String,
    binary_sha256: Option<String>,
    adapter: Option<AdapterReceipt>,
    features: Option<BTreeMap<&'static str, FeatureRow>>,
    limits: Option<BTreeMap<&'static str, LimitRow>>,
    semantics: Option<SemanticEvidence>,
    adapter_error: Option<String>,
) -> ProbeOutput {
    let mut value = json!({
        "adapter": adapter,
        "adapter_error": adapter_error,
        "binary_sha256": binary_sha256,
        "category": assessment.category,
        "command": cli.canonical_command(),
        "crate": {
            "name": env!("CARGO_PKG_NAME"),
            "version": env!("CARGO_PKG_VERSION"),
            "dependencies": {
                "serde": {"version": "1.0.228", "default_features": true, "features": ["derive"]},
                "serde_json": {"version": "1.0.145", "default_features": true, "features": []},
                "sha2": {"version": "0.10.9", "default_features": true, "features": []},
                "wgpu": {"version": WGPU_VERSION, "default_features": false, "features": WGPU_FEATURES},
            },
        },
        "features": features,
        "formats": Value::Null,
        "host": {
            "os": std::env::consts::OS,
            "arch": std::env::consts::ARCH,
            "os_build": os_build(),
            "rustc_release": env!("FN64_PROBE_RUSTC_RELEASE"),
            "rustc_commit_hash": env!("FN64_PROBE_RUSTC_COMMIT_HASH"),
            "target": env!("FN64_PROBE_TARGET"),
        },
        "iterations": cli.iterations,
        "limits": limits,
        "missing_required": assessment.missing_required,
        "require": cli.require,
        "schema": "fn64.m2-wgpu-metal-capability.v1",
        "semantics": semantics,
        "source_sha256": source_sha256,
    });
    let body = serde_json::to_vec(&value).expect("serialize receipt body");
    value.as_object_mut().unwrap().insert(
        "canonical_sha256".into(),
        Value::String(digest_chunks([body.as_slice()])),
    );
    ProbeOutput {
        json: serde_json::to_string(&value).expect("serialize receipt"),
        exit_code: assessment.exit_code,
    }
}

fn execute_baseline(
    adapter: &wgpu::Adapter,
    supported_features: wgpu::Features,
    iterations: u32,
) -> SemanticEvidence {
    let requested_features = FEATURE_REQUIREMENTS
        .iter()
        .fold(wgpu::Features::empty(), |all, item| all | item.flag)
        & supported_features;
    let request = block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        label: Some("fn64-m2-metal-caps"),
        required_features: requested_features,
        required_limits: wgpu::Limits::default(),
        experimental_features: wgpu::ExperimentalFeatures::disabled(),
        memory_hints: wgpu::MemoryHints::MemoryUsage,
        trace: wgpu::Trace::Off,
    }));
    let (device, queue) = match request {
        Ok(pair) => pair,
        Err(error) => {
            return SemanticEvidence {
                iterations_requested: iterations,
                baseline_buffer_copy_and_map: "failed",
                capability_specific_operations: "not_exercised; this probe separates advertisements from executed semantics",
                validation_errors: vec![format!("request_device: {error}")],
                ..SemanticEvidence::default()
            };
        }
    };

    let uncaptured = Arc::new(Mutex::new(Vec::new()));
    let uncaptured_for_handler = Arc::clone(&uncaptured);
    device.on_uncaptured_error(Arc::new(move |error| {
        uncaptured_for_handler
            .lock()
            .unwrap()
            .push(error.to_string());
    }));

    let mut evidence = SemanticEvidence {
        iterations_requested: iterations,
        baseline_buffer_copy_and_map: "passed",
        capability_specific_operations: "not_exercised; advertised capability rows are not semantic execution claims",
        ..SemanticEvidence::default()
    };
    for iteration in 0..iterations {
        let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
        let expected = [0x46_u8, 0x4e, 0x36, 0x34, iteration as u8, 0xa5, 0x5a, 0xff];
        let source = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("fn64-m2-source"),
            size: expected.len() as u64,
            usage: wgpu::BufferUsages::COPY_SRC | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let destination = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("fn64-m2-destination"),
            size: expected.len() as u64,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        queue.write_buffer(&source, 0, &expected);
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("fn64-m2-copy"),
        });
        encoder.copy_buffer_to_buffer(&source, 0, &destination, 0, expected.len() as u64);
        let submission = queue.submit([encoder.finish()]);
        if let Err(error) = device.poll(wgpu::PollType::Wait {
            submission_index: Some(submission),
            timeout: Some(std::time::Duration::from_secs(10)),
        }) {
            evidence
                .validation_errors
                .push(format!("iteration {iteration} poll: {error}"));
            break;
        }
        let (sender, receiver) = mpsc::sync_channel(1);
        destination
            .slice(..)
            .map_async(wgpu::MapMode::Read, move |result| {
                let _ = sender.send(result);
            });
        if let Err(error) = device.poll(wgpu::PollType::Wait {
            submission_index: None,
            timeout: Some(std::time::Duration::from_secs(10)),
        }) {
            evidence
                .validation_errors
                .push(format!("iteration {iteration} map poll: {error}"));
            break;
        }
        match receiver.recv_timeout(std::time::Duration::from_secs(1)) {
            Ok(Ok(())) => match destination.slice(..).get_mapped_range() {
                Ok(mapped) => {
                    if mapped.as_ref() != expected {
                        evidence
                            .validation_errors
                            .push(format!("iteration {iteration} copy result mismatch"));
                    }
                    drop(mapped);
                    destination.unmap();
                }
                Err(error) => evidence.validation_errors.push(format!(
                    "iteration {iteration} mapped-range acquisition: {error}"
                )),
            },
            Ok(Err(error)) => evidence
                .validation_errors
                .push(format!("iteration {iteration} map: {error}")),
            Err(error) => evidence
                .validation_errors
                .push(format!("iteration {iteration} map callback: {error}")),
        }
        if let Some(error) = block_on(scope.pop()) {
            evidence
                .validation_errors
                .push(format!("iteration {iteration} validation scope: {error}"));
        }
        if evidence.validation_errors.is_empty() {
            evidence.iterations_completed += 1;
        } else {
            break;
        }
    }
    evidence.uncaptured_errors = uncaptured.lock().unwrap().clone();
    if !evidence.validation_errors.is_empty() || !evidence.uncaptured_errors.is_empty() {
        evidence.baseline_buffer_copy_and_map = "failed";
    }
    evidence
}

fn adapter_receipt(info: wgpu::AdapterInfo) -> AdapterReceipt {
    AdapterReceipt {
        name: info.name,
        vendor: info.vendor,
        device: info.device,
        device_type: format!("{:?}", info.device_type),
        driver: info.driver,
        driver_info: info.driver_info,
        backend: format!("{:?}", info.backend),
        subgroup_min_size: info.subgroup_min_size,
        subgroup_max_size: info.subgroup_max_size,
        device_pci_bus_id: info.device_pci_bus_id,
        transient_saves_memory: info.transient_saves_memory,
        limit_bucket: info.limit_bucket.map(|bucket| bucket.name.into_owned()),
    }
}

fn limit_matrix(limits: &wgpu::Limits) -> BTreeMap<&'static str, LimitRow> {
    let required = wgpu::Limits::default();
    [
        (
            "max_bind_groups",
            limits.max_bind_groups as u64,
            required.max_bind_groups as u64,
        ),
        (
            "max_bindings_per_bind_group",
            limits.max_bindings_per_bind_group as u64,
            required.max_bindings_per_bind_group as u64,
        ),
        (
            "max_buffer_size",
            limits.max_buffer_size,
            required.max_buffer_size,
        ),
        (
            "max_compute_invocations_per_workgroup",
            limits.max_compute_invocations_per_workgroup as u64,
            required.max_compute_invocations_per_workgroup as u64,
        ),
        (
            "max_compute_workgroup_size_x",
            limits.max_compute_workgroup_size_x as u64,
            required.max_compute_workgroup_size_x as u64,
        ),
        (
            "max_compute_workgroups_per_dimension",
            limits.max_compute_workgroups_per_dimension as u64,
            required.max_compute_workgroups_per_dimension as u64,
        ),
        (
            "max_sampled_textures_per_shader_stage",
            limits.max_sampled_textures_per_shader_stage as u64,
            required.max_sampled_textures_per_shader_stage as u64,
        ),
        (
            "max_samplers_per_shader_stage",
            limits.max_samplers_per_shader_stage as u64,
            required.max_samplers_per_shader_stage as u64,
        ),
        (
            "max_storage_buffer_binding_size",
            limits.max_storage_buffer_binding_size,
            required.max_storage_buffer_binding_size,
        ),
        (
            "max_storage_buffers_per_shader_stage",
            limits.max_storage_buffers_per_shader_stage as u64,
            required.max_storage_buffers_per_shader_stage as u64,
        ),
        (
            "max_binding_array_elements_per_shader_stage",
            limits.max_binding_array_elements_per_shader_stage as u64,
            required.max_binding_array_elements_per_shader_stage as u64,
        ),
        (
            "max_binding_array_sampler_elements_per_shader_stage",
            limits.max_binding_array_sampler_elements_per_shader_stage as u64,
            required.max_binding_array_sampler_elements_per_shader_stage as u64,
        ),
        (
            "max_storage_textures_per_shader_stage",
            limits.max_storage_textures_per_shader_stage as u64,
            required.max_storage_textures_per_shader_stage as u64,
        ),
        (
            "max_texture_array_layers",
            limits.max_texture_array_layers as u64,
            required.max_texture_array_layers as u64,
        ),
        (
            "max_texture_dimension_2d",
            limits.max_texture_dimension_2d as u64,
            required.max_texture_dimension_2d as u64,
        ),
        (
            "max_uniform_buffer_binding_size",
            limits.max_uniform_buffer_binding_size,
            required.max_uniform_buffer_binding_size,
        ),
        (
            "max_uniform_buffers_per_shader_stage",
            limits.max_uniform_buffers_per_shader_stage as u64,
            required.max_uniform_buffers_per_shader_stage as u64,
        ),
    ]
    .into_iter()
    .map(|(name, observed, required_minimum)| {
        (
            name,
            LimitRow {
                observed,
                required_minimum,
                supported: observed >= required_minimum,
            },
        )
    })
    .collect()
}

fn usage_names(usages: wgpu::TextureUsages) -> Vec<&'static str> {
    [
        (wgpu::TextureUsages::COPY_SRC, "copy_src"),
        (wgpu::TextureUsages::COPY_DST, "copy_dst"),
        (wgpu::TextureUsages::TEXTURE_BINDING, "texture_binding"),
        (wgpu::TextureUsages::STORAGE_BINDING, "storage_binding"),
        (wgpu::TextureUsages::RENDER_ATTACHMENT, "render_attachment"),
    ]
    .into_iter()
    .filter_map(|(flag, name)| usages.contains(flag).then_some(name))
    .collect()
}

fn storage_names(read: bool, write: bool, read_write: bool) -> Vec<&'static str> {
    [
        (read, "read_only"),
        (write, "write_only"),
        (read_write, "read_write"),
    ]
    .into_iter()
    .filter_map(|(present, name)| present.then_some(name))
    .collect()
}

fn storage_flag_names(flags: wgpu::TextureFormatFeatureFlags) -> Vec<&'static str> {
    storage_names(
        flags.contains(wgpu::TextureFormatFeatureFlags::STORAGE_READ_ONLY),
        flags.contains(wgpu::TextureFormatFeatureFlags::STORAGE_WRITE_ONLY),
        flags.contains(wgpu::TextureFormatFeatureFlags::STORAGE_READ_WRITE),
    )
}

fn block_on<F: std::future::Future>(future: F) -> F::Output {
    use std::future::Future;
    use std::pin::pin;
    use std::task::{Context, Poll, Wake, Waker};
    struct ThreadWake(std::thread::Thread);
    impl Wake for ThreadWake {
        fn wake(self: Arc<Self>) {
            self.0.unpark();
        }
    }
    let waker = Waker::from(Arc::new(ThreadWake(std::thread::current())));
    let mut context = Context::from_waker(&waker);
    let mut future = pin!(future);
    loop {
        match Future::poll(future.as_mut(), &mut context) {
            Poll::Ready(output) => return output,
            Poll::Pending => std::thread::park(),
        }
    }
}

fn digest_file(path: &Path) -> std::io::Result<String> {
    let bytes = fs::read(path)?;
    Ok(digest_chunks([bytes.as_slice()]))
}

fn digest_chunks<'a>(chunks: impl IntoIterator<Item = &'a [u8]>) -> String {
    let mut digest = Sha256::new();
    for chunk in chunks {
        digest.update((chunk.len() as u64).to_be_bytes());
        digest.update(chunk);
    }
    digest
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn os_build() -> Option<String> {
    let output = Command::new("sw_vers").arg("-buildVersion").output().ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_string())
}

impl fmt::Display for Category {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{:?}", self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cli_defaults_are_observational_and_one_iteration() {
        assert_eq!(
            Cli::parse(Vec::<String>::new()).unwrap(),
            Cli {
                require: false,
                iterations: 1
            }
        );
    }

    #[test]
    fn cli_parses_require_and_bounded_iterations() {
        let cli = Cli::parse(["--iterations", "10", "--require"]).unwrap();
        assert_eq!(
            cli,
            Cli {
                require: true,
                iterations: 10
            }
        );
        assert!(Cli::parse(["--iterations", "0"]).is_err());
        assert!(Cli::parse(["--iterations", "101"]).is_err());
        assert!(Cli::parse(["--bogus"]).is_err());
    }

    #[test]
    fn unsupported_is_observational_without_require() {
        let assessment = assess(false, true, vec!["feature:x".into()], true);
        assert_eq!(assessment.category, Category::ObservedUnsupported);
        assert_eq!(assessment.exit_code, EXIT_PASS);
    }

    #[test]
    fn complete_required_matrix_is_a_pass() {
        let assessment = assess(true, true, Vec::new(), true);
        assert_eq!(assessment.category, Category::Pass);
        assert_eq!(assessment.exit_code, EXIT_PASS);
    }

    #[test]
    fn require_turns_missing_capability_into_explicit_unsupported() {
        let assessment = assess(true, true, vec!["feature:x".into()], true);
        assert_eq!(assessment.category, Category::ExplicitlyUnsupported);
        assert_eq!(assessment.exit_code, EXIT_UNSUPPORTED);
    }

    #[test]
    fn semantic_failure_precedes_unsupported_classification() {
        let assessment = assess(true, true, vec!["feature:x".into()], false);
        assert_eq!(assessment.category, Category::SemanticOrValidationFailure);
        assert_eq!(assessment.exit_code, EXIT_SEMANTIC_FAILURE);
    }

    #[test]
    fn no_adapter_has_stable_exit() {
        let assessment = assess(true, false, Vec::new(), true);
        assert_eq!(assessment.category, Category::NoAdapter);
        assert_eq!(assessment.exit_code, EXIT_NO_ADAPTER);
    }

    #[test]
    fn receipt_is_compact_canonical_json_without_paths() {
        let output = finish_receipt(
            &Cli {
                require: false,
                iterations: 1,
            },
            assess(false, false, Vec::new(), true),
            "source".into(),
            Some("binary".into()),
            None,
            None,
            None,
            None,
            Some("no adapter".into()),
        );
        assert!(!output.json.contains(std::path::MAIN_SEPARATOR));
        assert!(!output.json.contains('\n'));
        let value: Value = serde_json::from_str(&output.json).unwrap();
        assert_eq!(value["schema"], "fn64.m2-wgpu-metal-capability.v1");
        let recorded = value["canonical_sha256"].as_str().unwrap().to_string();
        assert_eq!(recorded.len(), 64);
        let mut body = value;
        body.as_object_mut().unwrap().remove("canonical_sha256");
        assert_eq!(
            recorded,
            digest_chunks([serde_json::to_vec(&body).unwrap().as_slice()])
        );
    }
}
