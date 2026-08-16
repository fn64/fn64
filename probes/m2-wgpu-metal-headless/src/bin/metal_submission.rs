// This shared module is compiled once per binary; the submission probe uses
// exact-index waits instead of its generic submit-and-wait helper.
#[allow(dead_code)]
mod support;

use std::process::ExitCode;
use std::sync::mpsc;
use std::time::Duration;

use serde::Serialize;
use support::{DeviceContext, MetalAdapter, Status};

const SOURCE_PARTS: &[&str] = &[
    include_str!("../../Cargo.toml"),
    include_str!("../../Cargo.lock"),
    include_str!("../../build.rs"),
    include_str!("../../README.md"),
    include_str!("support/mod.rs"),
    include_str!("metal_submission.rs"),
];
const OUTPUT_BYTES: [u8; 4] = [0x21, 0x3c, 0x4d, 0x59];
const PIPELINES_AT_ARM: u32 = 4;
const TIMESTAMP_COUNT: u32 = 4;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
struct Configuration {
    queue_count: u32,
    submission_count: u32,
    callback_count: u32,
    compute_pipeline_count: u32,
    prewarmed_ubershader_count: u32,
    prewarmed_finite_pso_count: u32,
    timestamp_query_count: u32,
    output_format: &'static str,
}

const CONFIGURATION: Configuration = Configuration {
    queue_count: 1,
    submission_count: 3,
    callback_count: 3,
    compute_pipeline_count: 1,
    prewarmed_ubershader_count: 1,
    prewarmed_finite_pso_count: 2,
    timestamp_query_count: TIMESTAMP_COUNT,
    output_format: "rgba8unorm",
};

#[derive(Serialize)]
struct Evidence {
    adapter_backend: Option<&'static str>,
    configuration: Configuration,
    advertised_timestamp_query: bool,
    pipeline_warmup: PipelineWarmupReceipt,
    submission_lifecycle: LifecycleArm,
    compute_render_copy: ByteArm,
    timestamps: TimestampArm,
    unexpected_validation_or_uncaptured_error_count: usize,
}

#[derive(Serialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
enum PipelineWarmupReceipt {
    Passed {
        creations_before_arm: u32,
        application_creations_after_arm: u32,
        enforcement: &'static str,
    },
    Failed {
        stage: &'static str,
    },
    NotRun,
}

impl PipelineWarmupReceipt {
    fn passes_exact(&self) -> bool {
        matches!(
            self,
            Self::Passed {
                creations_before_arm,
                application_creations_after_arm,
                enforcement,
            } if *creations_before_arm == PIPELINES_AT_ARM
                && *application_creations_after_arm == 0
                && *enforcement
                    == "consumed_pipeline_factory_and_execution_wrapper_without_pipeline_api"
        )
    }
}

#[derive(Serialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
enum LifecycleArm {
    Passed { events: Vec<LifecycleEvent> },
    Mismatch { events: Vec<LifecycleEvent> },
    Failed { stage: &'static str },
    NotRun,
}

impl LifecycleArm {
    fn passes_exact(&self) -> bool {
        matches!(self, Self::Passed { events } if events.as_slice() == EXPECTED_LIFECYCLE)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
enum LifecycleEvent {
    SubmittedOne,
    ExactWaitOneReturned,
    CallbackOneObservedReadyAfterExactWait,
    SubmittedTwo,
    ExactWaitTwoReturned,
    CallbackTwoObservedReadyAfterExactWait,
    SubmittedThree,
    ExactWaitThreeReturned,
    CallbackThreeObservedReadyAfterExactWait,
}

const EXPECTED_LIFECYCLE: [LifecycleEvent; 9] = [
    LifecycleEvent::SubmittedOne,
    LifecycleEvent::ExactWaitOneReturned,
    LifecycleEvent::CallbackOneObservedReadyAfterExactWait,
    LifecycleEvent::SubmittedTwo,
    LifecycleEvent::ExactWaitTwoReturned,
    LifecycleEvent::CallbackTwoObservedReadyAfterExactWait,
    LifecycleEvent::SubmittedThree,
    LifecycleEvent::ExactWaitThreeReturned,
    LifecycleEvent::CallbackThreeObservedReadyAfterExactWait,
];

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
    NotRun,
}

impl ByteArm {
    fn passes_exact(&self) -> bool {
        let expected = support::digest_chunks([OUTPUT_BYTES.as_slice()]);
        matches!(
            self,
            Self::Passed {
                expected_sha256,
                observed_sha256,
                byte_count,
            } if expected_sha256 == &expected
                && observed_sha256 == &expected
                && *byte_count == OUTPUT_BYTES.len()
        )
    }
}

#[derive(Serialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
enum TimestampArm {
    Passed {
        raw_ticks: [u64; 4],
        timestamp_period_ns: f32,
    },
    Mismatch {
        raw_ticks: [u64; 4],
        reason: &'static str,
    },
    Failed {
        stage: &'static str,
    },
    Unsupported {
        missing_advertisement: &'static str,
    },
    NotRun,
}

impl TimestampArm {
    fn passes_exact(&self) -> bool {
        matches!(
            self,
            Self::Passed {
                raw_ticks,
                timestamp_period_ns,
            } if !raw_ticks.contains(&0)
                && raw_ticks.windows(2).all(|pair| pair[0] <= pair[1])
                && *timestamp_period_ns > 0.0
                && timestamp_period_ns.is_finite()
        )
    }

    fn is_genuine_non_advertised_unsupported(&self) -> bool {
        matches!(
            self,
            Self::Unsupported {
                missing_advertisement: "feature:timestamp_query"
            }
        )
    }
}

fn main() -> ExitCode {
    if std::env::args_os().len() != 1 {
        eprintln!("metal_submission: this deterministic probe accepts no arguments");
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
            return finish_no_adapter();
        }
    };
    let timestamp_supported = adapter.features().contains(wgpu::Features::TIMESTAMP_QUERY);
    let required_features = if timestamp_supported {
        wgpu::Features::TIMESTAMP_QUERY
    } else {
        wgpu::Features::empty()
    };
    let context = match support::request_device(
        &adapter,
        required_features,
        wgpu::Limits::default(),
        "fn64-metal-submission",
    ) {
        Ok(context) => context,
        Err(stage) => {
            let evidence = Evidence {
                adapter_backend: Some("metal"),
                configuration: CONFIGURATION,
                advertised_timestamp_query: timestamp_supported,
                pipeline_warmup: PipelineWarmupReceipt::NotRun,
                submission_lifecycle: LifecycleArm::Failed { stage },
                compute_render_copy: ByteArm::NotRun,
                timestamps: TimestampArm::NotRun,
                unexpected_validation_or_uncaptured_error_count: 0,
            };
            return support::finish(
                "fn64.m2-metal-submission.v1",
                "metal_submission",
                Some(&adapter),
                Status::SemanticOrValidationMismatch,
                &evidence,
                SOURCE_PARTS,
            );
        }
    };

    let armed = match WarmupContext::new(context, timestamp_supported).arm() {
        Ok(armed) => armed,
        Err(stage) => {
            let evidence = Evidence {
                adapter_backend: Some("metal"),
                configuration: CONFIGURATION,
                advertised_timestamp_query: timestamp_supported,
                pipeline_warmup: PipelineWarmupReceipt::Failed { stage },
                submission_lifecycle: LifecycleArm::Failed { stage },
                compute_render_copy: ByteArm::NotRun,
                timestamps: TimestampArm::NotRun,
                unexpected_validation_or_uncaptured_error_count: 0,
            };
            return support::finish(
                "fn64.m2-metal-submission.v1",
                "metal_submission",
                Some(&adapter),
                Status::SemanticOrValidationMismatch,
                &evidence,
                SOURCE_PARTS,
            );
        }
    };
    let execution = armed.execute();
    let (pipeline_warmup, submission_lifecycle, compute_render_copy, timestamps, errors) =
        match execution {
            Ok(execution) => (
                execution.pipeline_warmup,
                execution.submission_lifecycle,
                execution.compute_render_copy,
                execution.timestamps,
                execution.uncaptured_errors,
            ),
            Err(stage) => (
                PipelineWarmupReceipt::Passed {
                    creations_before_arm: PIPELINES_AT_ARM,
                    application_creations_after_arm: 0,
                    enforcement: "consumed_pipeline_factory_and_execution_wrapper_without_pipeline_api",
                },
                LifecycleArm::Failed { stage },
                ByteArm::NotRun,
                TimestampArm::NotRun,
                0,
            ),
        };
    let evidence = Evidence {
        adapter_backend: Some("metal"),
        configuration: CONFIGURATION,
        advertised_timestamp_query: timestamp_supported,
        pipeline_warmup,
        submission_lifecycle,
        compute_render_copy,
        timestamps,
        unexpected_validation_or_uncaptured_error_count: errors,
    };
    let status = classify(&evidence);
    support::finish(
        "fn64.m2-metal-submission.v1",
        "metal_submission",
        Some(&adapter),
        status,
        &evidence,
        SOURCE_PARTS,
    )
}

fn finish_no_adapter() -> support::ProbeOutput {
    let evidence = Evidence {
        adapter_backend: None,
        configuration: CONFIGURATION,
        advertised_timestamp_query: false,
        pipeline_warmup: PipelineWarmupReceipt::NotRun,
        submission_lifecycle: LifecycleArm::NotRun,
        compute_render_copy: ByteArm::NotRun,
        timestamps: TimestampArm::NotRun,
        unexpected_validation_or_uncaptured_error_count: 0,
    };
    support::finish(
        "fn64.m2-metal-submission.v1",
        "metal_submission",
        None,
        Status::NoMetalAdapter,
        &evidence,
        SOURCE_PARTS,
    )
}

fn classify(evidence: &Evidence) -> Status {
    let core_passed = evidence.unexpected_validation_or_uncaptured_error_count == 0
        && evidence.pipeline_warmup.passes_exact()
        && evidence.submission_lifecycle.passes_exact()
        && evidence.compute_render_copy.passes_exact();
    if core_passed {
        match (evidence.advertised_timestamp_query, &evidence.timestamps) {
            (true, timestamps) if timestamps.passes_exact() => Status::Pass,
            (false, timestamps) if timestamps.is_genuine_non_advertised_unsupported() => {
                Status::ExplicitlyUnsupportedNativeSubtest
            }
            _ => Status::SemanticOrValidationMismatch,
        }
    } else {
        Status::SemanticOrValidationMismatch
    }
}

struct WarmupContext {
    context: DeviceContext,
    timestamp_supported: bool,
}

impl WarmupContext {
    fn new(context: DeviceContext, timestamp_supported: bool) -> Self {
        Self {
            context,
            timestamp_supported,
        }
    }

    fn arm(self) -> Result<ArmedContext, &'static str> {
        const SHADER: &str = r#"
struct Word {
    value: u32,
};

@group(0) @binding(0) var<storage, read_write> computed: Word;

@compute @workgroup_size(1)
fn compute_main() {
    computed.value = 0x594d3c21u;
}

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
fn fs_uber() -> @location(0) vec4<f32> {
    let word = computed.value;
    let rgba = vec4<u32>(
        word & 0xffu,
        (word >> 8u) & 0xffu,
        (word >> 16u) & 0xffu,
        (word >> 24u) & 0xffu,
    );
    return vec4<f32>(rgba) / 255.0;
}

@fragment
fn fs_finite() -> @location(0) vec4<f32> {
    return vec4<f32>(33.0 / 255.0, 60.0 / 255.0, 77.0 / 255.0, 89.0 / 255.0);
}
"#;
        let shader = self
            .context
            .device
            .create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("fn64-submission-prewarmed-shader"),
                source: wgpu::ShaderSource::Wgsl(SHADER.into()),
            });
        let mut pipeline_factory = PipelineFactory::new(&self.context.device);
        let compute = pipeline_factory.create_compute(&wgpu::ComputePipelineDescriptor {
            label: Some("fn64-submission-compute"),
            layout: None,
            module: &shader,
            entry_point: Some("compute_main"),
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            cache: None,
        });
        let ubershader =
            pipeline_factory.create_render(&shader, "fn64-submission-ubershader", "fs_uber", None);
        let finite_replace = pipeline_factory.create_render(
            &shader,
            "fn64-submission-finite-replace",
            "fs_finite",
            None,
        );
        let finite_add = pipeline_factory.create_render(
            &shader,
            "fn64-submission-finite-add",
            "fs_finite",
            Some(wgpu::BlendState {
                color: wgpu::BlendComponent {
                    src_factor: wgpu::BlendFactor::One,
                    dst_factor: wgpu::BlendFactor::One,
                    operation: wgpu::BlendOperation::Add,
                },
                alpha: wgpu::BlendComponent {
                    src_factor: wgpu::BlendFactor::One,
                    dst_factor: wgpu::BlendFactor::Zero,
                    operation: wgpu::BlendOperation::Add,
                },
            }),
        );

        let computed = self.context.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("fn64-submission-computed-word"),
            size: 4,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let compute_layout = compute.get_bind_group_layout(0);
        let compute_bind_group =
            self.context
                .device
                .create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("fn64-submission-compute-bind-group"),
                    layout: &compute_layout,
                    entries: &[wgpu::BindGroupEntry {
                        binding: 0,
                        resource: computed.as_entire_binding(),
                    }],
                });
        let render_layout = ubershader.get_bind_group_layout(0);
        let render_bind_group = self
            .context
            .device
            .create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("fn64-submission-render-bind-group"),
                layout: &render_layout,
                entries: &[wgpu::BindGroupEntry {
                    binding: 0,
                    resource: computed.as_entire_binding(),
                }],
            });
        let target = self
            .context
            .device
            .create_texture(&wgpu::TextureDescriptor {
                label: Some("fn64-submission-target"),
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
        let target_view = target.create_view(&wgpu::TextureViewDescriptor::default());
        let output_readback = self.context.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("fn64-submission-output-readback"),
            size: 256,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let timestamps = if self.timestamp_supported {
            let query_set = self
                .context
                .device
                .create_query_set(&wgpu::QuerySetDescriptor {
                    label: Some("fn64-submission-timestamps"),
                    ty: wgpu::QueryType::Timestamp,
                    count: TIMESTAMP_COUNT,
                });
            let resolved = self.context.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("fn64-submission-timestamp-resolve"),
                size: u64::from(TIMESTAMP_COUNT) * 8,
                usage: wgpu::BufferUsages::QUERY_RESOLVE | wgpu::BufferUsages::COPY_SRC,
                mapped_at_creation: false,
            });
            let readback = self.context.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("fn64-submission-timestamp-readback"),
                size: u64::from(TIMESTAMP_COUNT) * 8,
                usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                mapped_at_creation: false,
            });
            Some(TimestampResources {
                query_set,
                resolved,
                readback,
            })
        } else {
            None
        };
        let pipeline_witness = pipeline_factory.arm()?;

        Ok(ArmedContext {
            execution: ExecutionContext(self.context),
            pipeline_witness,
            compute,
            ubershader,
            _finite_replace: finite_replace,
            _finite_add: finite_add,
            computed,
            compute_bind_group,
            render_bind_group,
            target,
            target_view,
            output_readback,
            timestamps,
        })
    }
}

struct PipelineFactory<'a> {
    device: &'a wgpu::Device,
    creations: u32,
}

impl<'a> PipelineFactory<'a> {
    fn new(device: &'a wgpu::Device) -> Self {
        Self {
            device,
            creations: 0,
        }
    }

    fn create_compute(
        &mut self,
        descriptor: &wgpu::ComputePipelineDescriptor<'_>,
    ) -> wgpu::ComputePipeline {
        let pipeline = self.device.create_compute_pipeline(descriptor);
        self.creations += 1;
        pipeline
    }

    fn create_render(
        &mut self,
        shader: &wgpu::ShaderModule,
        label: &'static str,
        fragment_entry: &'static str,
        blend: Option<wgpu::BlendState>,
    ) -> wgpu::RenderPipeline {
        let pipeline = create_render_pipeline(self.device, shader, label, fragment_entry, blend);
        self.creations += 1;
        pipeline
    }

    fn arm(self) -> Result<ArmedPipelineWitness, &'static str> {
        if self.creations != PIPELINES_AT_ARM {
            return Err("pipeline_warmup_count");
        }
        Ok(ArmedPipelineWitness {
            creations_before_arm: self.creations,
        })
    }
}

struct ArmedPipelineWitness {
    creations_before_arm: u32,
}

impl ArmedPipelineWitness {
    fn into_receipt(self) -> PipelineWarmupReceipt {
        PipelineWarmupReceipt::Passed {
            creations_before_arm: self.creations_before_arm,
            application_creations_after_arm: 0,
            enforcement: "consumed_pipeline_factory_and_execution_wrapper_without_pipeline_api",
        }
    }
}

fn create_render_pipeline(
    device: &wgpu::Device,
    shader: &wgpu::ShaderModule,
    label: &'static str,
    fragment_entry: &'static str,
    blend: Option<wgpu::BlendState>,
) -> wgpu::RenderPipeline {
    let targets = [Some(wgpu::ColorTargetState {
        format: wgpu::TextureFormat::Rgba8Unorm,
        blend,
        write_mask: wgpu::ColorWrites::ALL,
    })];
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some(label),
        layout: None,
        vertex: wgpu::VertexState {
            module: shader,
            entry_point: Some("vs_main"),
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            buffers: &[],
        },
        primitive: wgpu::PrimitiveState::default(),
        depth_stencil: None,
        multisample: wgpu::MultisampleState::default(),
        fragment: Some(wgpu::FragmentState {
            module: shader,
            entry_point: Some(fragment_entry),
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            targets: &targets,
        }),
        multiview_mask: None,
        cache: None,
    })
}

struct ExecutionContext(DeviceContext);

impl ExecutionContext {
    fn encoder(&self, label: &'static str) -> wgpu::CommandEncoder {
        self.0
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some(label) })
    }

    fn submit(&self, encoder: wgpu::CommandEncoder) -> wgpu::SubmissionIndex {
        self.0.queue.submit([encoder.finish()])
    }

    fn wait(&self, index: wgpu::SubmissionIndex) -> Result<(), &'static str> {
        // wgpu-types 30.0.0 `src/lib.rs`, `PollType::Wait` (official API:
        // https://docs.rs/wgpu-types/30.0.0/wgpu_types/enum.PollType.html#variant.Wait)
        // guarantees on native wgpu-core backends that this returns only after
        // the selected submission completed and its callbacks were invoked.
        self.0
            .device
            .poll(wgpu::PollType::Wait {
                submission_index: Some(index),
                timeout: Some(Duration::from_secs(10)),
            })
            .map_err(|_| "exact_submission_wait")?;
        Ok(())
    }

    fn map(&self, buffer: &wgpu::Buffer) -> Result<Vec<u8>, &'static str> {
        support::map_buffer(&self.0.device, buffer)
    }

    fn timestamp_period(&self) -> f32 {
        self.0.queue.get_timestamp_period()
    }

    fn uncaptured_error_count(&self) -> usize {
        self.0.uncaptured_error_count()
    }
}

struct TimestampResources {
    query_set: wgpu::QuerySet,
    resolved: wgpu::Buffer,
    readback: wgpu::Buffer,
}

struct ArmedContext {
    execution: ExecutionContext,
    pipeline_witness: ArmedPipelineWitness,
    compute: wgpu::ComputePipeline,
    ubershader: wgpu::RenderPipeline,
    _finite_replace: wgpu::RenderPipeline,
    _finite_add: wgpu::RenderPipeline,
    computed: wgpu::Buffer,
    compute_bind_group: wgpu::BindGroup,
    render_bind_group: wgpu::BindGroup,
    target: wgpu::Texture,
    target_view: wgpu::TextureView,
    output_readback: wgpu::Buffer,
    timestamps: Option<TimestampResources>,
}

struct ExecutionEvidence {
    pipeline_warmup: PipelineWarmupReceipt,
    submission_lifecycle: LifecycleArm,
    compute_render_copy: ByteArm,
    timestamps: TimestampArm,
    uncaptured_errors: usize,
}

impl ArmedContext {
    fn execute(self) -> Result<ExecutionEvidence, &'static str> {
        let (callback_tx, callback_rx) = mpsc::channel();
        let mut events = Vec::with_capacity(EXPECTED_LIFECYCLE.len());

        let mut first = self.execution.encoder("fn64-submission-one");
        first.clear_buffer(&self.computed, 0, None);
        let first_callback = callback_tx.clone();
        first.on_submitted_work_done(move || {
            let _ = first_callback.send(1u8);
        });
        let first_index = self.execution.submit(first);
        events.push(LifecycleEvent::SubmittedOne);
        self.execution.wait(first_index)?;
        events.push(LifecycleEvent::ExactWaitOneReturned);
        // The nonblocking receive observes callback delivery after the exact
        // wait. Separately, the cited native PollType::Wait contract guarantees
        // callback invocation before return. Submission two does not exist yet,
        // closing the callback-two-before-callback-one interleaving.
        if callback_rx.try_recv().ok() != Some(1) {
            return Err("submission_one_callback");
        }
        events.push(LifecycleEvent::CallbackOneObservedReadyAfterExactWait);

        let mut second = self.execution.encoder("fn64-submission-two");
        {
            let timestamp_writes =
                self.timestamps
                    .as_ref()
                    .map(|timestamps| wgpu::ComputePassTimestampWrites {
                        query_set: &timestamps.query_set,
                        beginning_of_pass_write_index: Some(0),
                        end_of_pass_write_index: Some(1),
                    });
            let mut pass = second.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("fn64-submission-compute"),
                timestamp_writes,
            });
            pass.set_pipeline(&self.compute);
            pass.set_bind_group(0, &self.compute_bind_group, &[]);
            pass.dispatch_workgroups(1, 1, 1);
        }
        {
            let attachments = [Some(wgpu::RenderPassColorAttachment {
                view: &self.target_view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                    store: wgpu::StoreOp::Store,
                },
            })];
            let timestamp_writes =
                self.timestamps
                    .as_ref()
                    .map(|timestamps| wgpu::RenderPassTimestampWrites {
                        query_set: &timestamps.query_set,
                        beginning_of_pass_write_index: Some(2),
                        end_of_pass_write_index: Some(3),
                    });
            let mut pass = second.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("fn64-submission-render"),
                color_attachments: &attachments,
                depth_stencil_attachment: None,
                timestamp_writes,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_pipeline(&self.ubershader);
            pass.set_bind_group(0, &self.render_bind_group, &[]);
            pass.draw(0..3, 0..1);
        }
        second.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: &self.target,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &self.output_readback,
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
        let second_callback = callback_tx;
        let third_callback = second_callback.clone();
        second.on_submitted_work_done(move || {
            let _ = second_callback.send(2_u8);
        });
        let second_index = self.execution.submit(second);
        events.push(LifecycleEvent::SubmittedTwo);
        self.execution.wait(second_index)?;
        events.push(LifecycleEvent::ExactWaitTwoReturned);
        if callback_rx.try_recv().ok() != Some(2) {
            return Err("submission_two_callback");
        }
        events.push(LifecycleEvent::CallbackTwoObservedReadyAfterExactWait);

        let mut third = self.execution.encoder("fn64-submission-three-resolve");
        if let Some(timestamps) = &self.timestamps {
            // wgpu's documented portable query-resolve fallback is a distinct
            // submission after the timestamp-producing work has completed.
            // Keeping it explicit also prevents a zero/unavailable query from
            // being hidden behind the render submission's completion callback.
            third.resolve_query_set(
                &timestamps.query_set,
                0..TIMESTAMP_COUNT,
                &timestamps.resolved,
                0,
            );
            third.copy_buffer_to_buffer(
                &timestamps.resolved,
                0,
                &timestamps.readback,
                0,
                u64::from(TIMESTAMP_COUNT) * 8,
            );
        }
        third.on_submitted_work_done(move || {
            let _ = third_callback.send(3_u8);
        });
        let third_index = self.execution.submit(third);
        events.push(LifecycleEvent::SubmittedThree);
        self.execution.wait(third_index)?;
        events.push(LifecycleEvent::ExactWaitThreeReturned);
        if callback_rx.try_recv().ok() != Some(3) {
            return Err("submission_three_callback");
        }
        events.push(LifecycleEvent::CallbackThreeObservedReadyAfterExactWait);

        let submission_lifecycle = lifecycle_arm(events);
        let mapped_output = self.execution.map(&self.output_readback)?;
        let compute_render_copy = byte_arm(&OUTPUT_BYTES, Ok(mapped_output[..4].to_vec()));
        let timestamps = if let Some(resources) = &self.timestamps {
            let mapped = self.execution.map(&resources.readback)?;
            timestamp_arm(&mapped, self.execution.timestamp_period())
        } else {
            TimestampArm::Unsupported {
                missing_advertisement: "feature:timestamp_query",
            }
        };
        Ok(ExecutionEvidence {
            pipeline_warmup: self.pipeline_witness.into_receipt(),
            submission_lifecycle,
            compute_render_copy,
            timestamps,
            uncaptured_errors: self.execution.uncaptured_error_count(),
        })
    }
}

fn lifecycle_arm(events: Vec<LifecycleEvent>) -> LifecycleArm {
    if events == EXPECTED_LIFECYCLE {
        LifecycleArm::Passed { events }
    } else {
        LifecycleArm::Mismatch { events }
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

fn timestamp_arm(bytes: &[u8], timestamp_period_ns: f32) -> TimestampArm {
    if bytes.len() != (TIMESTAMP_COUNT * 8) as usize {
        return TimestampArm::Failed {
            stage: "timestamp_readback_size",
        };
    }
    let mut raw_ticks = [0u64; 4];
    for (index, chunk) in bytes.chunks_exact(8).enumerate() {
        raw_ticks[index] = u64::from_le_bytes(chunk.try_into().expect("eight-byte timestamp"));
    }
    if raw_ticks.contains(&0) {
        return TimestampArm::Mismatch {
            raw_ticks,
            reason: "zero_timestamp",
        };
    }
    if !raw_ticks.windows(2).all(|pair| pair[0] <= pair[1]) {
        return TimestampArm::Mismatch {
            raw_ticks,
            reason: "timestamps_not_ordered",
        };
    }
    if timestamp_period_ns <= 0.0 || !timestamp_period_ns.is_finite() {
        return TimestampArm::Mismatch {
            raw_ticks,
            reason: "invalid_timestamp_period",
        };
    }
    TimestampArm::Passed {
        raw_ticks,
        timestamp_period_ns,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn exact_evidence(advertised: bool, timestamps: TimestampArm) -> Evidence {
        Evidence {
            adapter_backend: Some("metal"),
            configuration: CONFIGURATION,
            advertised_timestamp_query: advertised,
            pipeline_warmup: PipelineWarmupReceipt::Passed {
                creations_before_arm: PIPELINES_AT_ARM,
                application_creations_after_arm: 0,
                enforcement: "consumed_pipeline_factory_and_execution_wrapper_without_pipeline_api",
            },
            submission_lifecycle: LifecycleArm::Passed {
                events: EXPECTED_LIFECYCLE.to_vec(),
            },
            compute_render_copy: byte_arm(&OUTPUT_BYTES, Ok(OUTPUT_BYTES.to_vec())),
            timestamps,
            unexpected_validation_or_uncaptured_error_count: 0,
        }
    }

    fn passed_timestamps() -> TimestampArm {
        TimestampArm::Passed {
            raw_ticks: [10, 20, 30, 40],
            timestamp_period_ns: 1.0,
        }
    }

    #[test]
    fn hostile_lifecycle_mutations_cannot_pass() {
        assert!(matches!(
            lifecycle_arm(EXPECTED_LIFECYCLE.to_vec()),
            LifecycleArm::Passed { .. }
        ));
        let mut swapped = EXPECTED_LIFECYCLE.to_vec();
        swapped.swap(1, 2);
        assert!(matches!(
            lifecycle_arm(swapped),
            LifecycleArm::Mismatch { .. }
        ));
        assert!(matches!(
            lifecycle_arm(EXPECTED_LIFECYCLE[..5].to_vec()),
            LifecycleArm::Mismatch { .. }
        ));
    }

    #[test]
    fn hostile_timestamp_mutations_cannot_pass() {
        let encode = |ticks: [u64; 4]| {
            ticks
                .into_iter()
                .flat_map(u64::to_le_bytes)
                .collect::<Vec<_>>()
        };
        assert!(matches!(
            timestamp_arm(&encode([10, 20, 30, 40]), 1.0),
            TimestampArm::Passed { .. }
        ));
        assert!(matches!(
            timestamp_arm(&encode([0, 20, 30, 40]), 1.0),
            TimestampArm::Mismatch {
                reason: "zero_timestamp",
                ..
            }
        ));
        assert!(matches!(
            timestamp_arm(&encode([10, 30, 20, 40]), 1.0),
            TimestampArm::Mismatch {
                reason: "timestamps_not_ordered",
                ..
            }
        ));
        assert!(matches!(
            timestamp_arm(&encode([10, 20, 30, 40]), 0.0),
            TimestampArm::Mismatch {
                reason: "invalid_timestamp_period",
                ..
            }
        ));
    }

    #[test]
    fn timestamp_advertisement_outcome_table_is_exhaustive() {
        let cases = [
            (true, passed_timestamps(), Status::Pass),
            (
                true,
                TimestampArm::Unsupported {
                    missing_advertisement: "feature:timestamp_query",
                },
                Status::SemanticOrValidationMismatch,
            ),
            (
                true,
                TimestampArm::Mismatch {
                    raw_ticks: [1, 3, 2, 4],
                    reason: "timestamps_not_ordered",
                },
                Status::SemanticOrValidationMismatch,
            ),
            (
                true,
                TimestampArm::Failed {
                    stage: "timestamp_readback",
                },
                Status::SemanticOrValidationMismatch,
            ),
            (
                true,
                TimestampArm::NotRun,
                Status::SemanticOrValidationMismatch,
            ),
            (
                false,
                passed_timestamps(),
                Status::SemanticOrValidationMismatch,
            ),
            (
                false,
                TimestampArm::Unsupported {
                    missing_advertisement: "feature:timestamp_query",
                },
                Status::ExplicitlyUnsupportedNativeSubtest,
            ),
            (
                false,
                TimestampArm::Mismatch {
                    raw_ticks: [1, 3, 2, 4],
                    reason: "timestamps_not_ordered",
                },
                Status::SemanticOrValidationMismatch,
            ),
            (
                false,
                TimestampArm::Failed {
                    stage: "timestamp_readback",
                },
                Status::SemanticOrValidationMismatch,
            ),
            (
                false,
                TimestampArm::NotRun,
                Status::SemanticOrValidationMismatch,
            ),
        ];
        for (advertised, timestamps, expected) in cases {
            assert_eq!(classify(&exact_evidence(advertised, timestamps)), expected);
        }
    }

    #[test]
    fn every_not_run_and_inexact_pass_receipt_is_rejected() {
        let mut warmup = exact_evidence(true, passed_timestamps());
        warmup.pipeline_warmup = PipelineWarmupReceipt::NotRun;
        assert_eq!(classify(&warmup), Status::SemanticOrValidationMismatch);

        let mut lifecycle = exact_evidence(true, passed_timestamps());
        lifecycle.submission_lifecycle = LifecycleArm::NotRun;
        assert_eq!(classify(&lifecycle), Status::SemanticOrValidationMismatch);

        let mut bytes = exact_evidence(true, passed_timestamps());
        bytes.compute_render_copy = ByteArm::NotRun;
        assert_eq!(classify(&bytes), Status::SemanticOrValidationMismatch);

        let timestamps = exact_evidence(true, TimestampArm::NotRun);
        assert_eq!(classify(&timestamps), Status::SemanticOrValidationMismatch);

        let mut wrong_count = exact_evidence(true, passed_timestamps());
        wrong_count.pipeline_warmup = PipelineWarmupReceipt::Passed {
            creations_before_arm: PIPELINES_AT_ARM - 1,
            application_creations_after_arm: 0,
            enforcement: "consumed_pipeline_factory_and_execution_wrapper_without_pipeline_api",
        };
        assert_eq!(classify(&wrong_count), Status::SemanticOrValidationMismatch);

        let mut wrong_events = exact_evidence(true, passed_timestamps());
        wrong_events.submission_lifecycle = LifecycleArm::Passed {
            events: EXPECTED_LIFECYCLE[..8].to_vec(),
        };
        assert_eq!(
            classify(&wrong_events),
            Status::SemanticOrValidationMismatch
        );

        let mut wrong_bytes = exact_evidence(true, passed_timestamps());
        wrong_bytes.compute_render_copy = ByteArm::Passed {
            expected_sha256: "wrong".into(),
            observed_sha256: "wrong".into(),
            byte_count: OUTPUT_BYTES.len(),
        };
        assert_eq!(classify(&wrong_bytes), Status::SemanticOrValidationMismatch);

        let wrong_ticks = exact_evidence(
            true,
            TimestampArm::Passed {
                raw_ticks: [10, 30, 20, 40],
                timestamp_period_ns: 1.0,
            },
        );
        assert_eq!(classify(&wrong_ticks), Status::SemanticOrValidationMismatch);

        let wrong_unsupported = exact_evidence(
            false,
            TimestampArm::Unsupported {
                missing_advertisement: "wrong",
            },
        );
        assert_eq!(
            classify(&wrong_unsupported),
            Status::SemanticOrValidationMismatch
        );
    }

    #[test]
    fn receipt_hash_binds_submission_configuration() {
        let evidence = Evidence {
            adapter_backend: None,
            configuration: CONFIGURATION,
            advertised_timestamp_query: false,
            pipeline_warmup: PipelineWarmupReceipt::NotRun,
            submission_lifecycle: LifecycleArm::NotRun,
            compute_render_copy: ByteArm::NotRun,
            timestamps: TimestampArm::NotRun,
            unexpected_validation_or_uncaptured_error_count: 0,
        };
        let first = support::finish(
            "fn64.test.submission.v1",
            "metal_submission",
            None,
            Status::NoMetalAdapter,
            &evidence,
            SOURCE_PARTS,
        );
        let mut mutated = evidence;
        mutated.configuration.submission_count = 4;
        let second = support::finish(
            "fn64.test.submission.v1",
            "metal_submission",
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
