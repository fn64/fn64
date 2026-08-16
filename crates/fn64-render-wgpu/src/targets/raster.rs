//! Exact native GPU execution for the first 4x2 RGBA16 fill.
//!
//! The module owns one prewarmed raster pipeline and composes it with M3.3d's
//! separately typed VI mechanism pipeline. It admits no other command stream,
//! target, format, geometry, or VI configuration.

use core::fmt;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::Duration;

use fn64_render_ir::{BackendCompletionAuthority, PhysicalMemoryLayout};

use crate::device::{HeadlessBackend, NoAdapter};
use crate::native_contract::{
    DeviceRgba16Bytes, InFlightNativeFill as ContractInFlightNativeFill,
    N64RecompRdramStorageBytes, NativeContractError, NativeFrameBinding, NativeGpuOutput,
    NativeGuestCommitError, PendingNativeCommit, PreparedNativeFill, NATIVE_FILL_DEVICE_RGBA16,
    NATIVE_FILL_NATIVE_RGBA8, NATIVE_FILL_POST_VI_BGRA8, NATIVE_FILL_RDRAM_BYTES,
};
use crate::vi::{
    execute_cpu_oracle, validate_exact_presentation, M3dPresentationSpec, ValidatedM3dPresentation,
    ViExecutionError, ViValidationError, REPLICATE_RGBA16_WGSL,
};

use super::{
    unpack_device_pixels, CandidateColorTarget, ColorTargetExtent, ColorTargetFormat,
    ColorTargetKey, ColorTargetRegistry, CompletedColorTargetWrite, DeviceColorBytes, ExactRowPlan,
    InitializedCandidateColorTarget, ResidentColorTarget, ResidentPublication, TargetError,
    TargetGeneration, TargetRectangle,
};

const POLL_TIMEOUT: Duration = Duration::from_secs(10);
const CALLBACK_TIMEOUT: Duration = Duration::from_secs(1);
const RASTER_SHADER: &str = include_str!("native_fill_rgba16.wgsl");
const RASTER_PARAMETER_BYTES: u64 = 16;
const VI_PARAMETER_BYTES: u64 = 16;
const DEVICE_RGBA16_BYTES: u64 = 16;
const POST_VI_BGRA8_BYTES: u64 = 32;
const READBACK_BYTES: u64 = DEVICE_RGBA16_BYTES + POST_VI_BGRA8_BYTES;

pub enum NativeRasterDeviceOutcome {
    Ready(Box<NativeRasterRenderer>),
    NoAdapter(NoAdapter),
}

pub struct UninitializedNativeRaster {
    backend: HeadlessBackend,
    completion_authority: BackendCompletionAuthority,
}

impl UninitializedNativeRaster {
    pub const fn new(
        backend: HeadlessBackend,
        completion_authority: BackendCompletionAuthority,
    ) -> Self {
        Self {
            backend,
            completion_authority,
        }
    }

    pub async fn request(self) -> Result<NativeRasterDeviceOutcome, NativeRasterError> {
        let presentation = validate_exact_presentation(M3dPresentationSpec::exact_fixture())?;
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            backends: self.backend.wgpu_backends(),
            flags: wgpu::InstanceFlags::VALIDATION,
            ..wgpu::InstanceDescriptor::new_without_display_handle()
        });
        let adapter = match instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                force_fallback_adapter: false,
                compatible_surface: None,
                apply_limit_buckets: false,
            })
            .await
        {
            Ok(adapter) => adapter,
            Err(wgpu::RequestAdapterError::NotFound { .. }) => {
                return Ok(NativeRasterDeviceOutcome::NoAdapter(NoAdapter::new(
                    self.backend,
                )));
            }
            Err(error) => return Err(NativeRasterError::RequestAdapter(error.to_string())),
        };
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                label: Some("fn64-render-wgpu-m3.3c"),
                required_features: wgpu::Features::empty(),
                required_limits: wgpu::Limits::default(),
                experimental_features: wgpu::ExperimentalFeatures::disabled(),
                memory_hints: wgpu::MemoryHints::MemoryUsage,
                trace: wgpu::Trace::Off,
            })
            .await
            .map_err(|error| NativeRasterError::RequestDevice(error.to_string()))?;

        let errors = Arc::new(BoundedErrorSink::default());
        let uncaptured = Arc::clone(&errors);
        device.on_uncaptured_error(Arc::new(move |error| {
            uncaptured.record(error.to_string());
        }));

        let raster_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("fn64-m3.3c-native-fill-rgba16"),
            source: wgpu::ShaderSource::Wgsl(RASTER_SHADER.into()),
        });
        let raster_pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("fn64-m3.3c-native-fill-rgba16"),
            layout: None,
            module: &raster_shader,
            entry_point: Some("fill_rgba16"),
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            cache: None,
        });
        let vi_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("fn64-m3.3c-bounded-vi-replicate"),
            source: wgpu::ShaderSource::Wgsl(REPLICATE_RGBA16_WGSL.into()),
        });
        let vi_pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("fn64-m3.3c-bounded-vi-replicate"),
            layout: None,
            module: &vi_shader,
            entry_point: Some("replicate_rgba16"),
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            cache: None,
        });
        let raster_parameters = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("fn64-m3.3c-raster-parameters"),
            size: RASTER_PARAMETER_BYTES,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let vi_parameters = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("fn64-m3.3c-vi-parameters"),
            size: VI_PARAMETER_BYTES,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let target_rgba16 = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("fn64-m3.3c-device-rgba16"),
            size: DEVICE_RGBA16_BYTES,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        let post_vi_bgra8 = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("fn64-m3.3c-post-vi-bgra8"),
            size: POST_VI_BGRA8_BYTES,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        let readback = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("fn64-m3.3c-bounded-readback"),
            size: READBACK_BYTES,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let raster_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("fn64-m3.3c-raster-bind-group"),
            layout: &raster_pipeline.get_bind_group_layout(0),
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: raster_parameters.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: target_rgba16.as_entire_binding(),
                },
            ],
        });
        let vi_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("fn64-m3.3c-vi-bind-group"),
            layout: &vi_pipeline.get_bind_group_layout(0),
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: vi_parameters.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: target_rgba16.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: post_vi_bgra8.as_entire_binding(),
                },
            ],
        });
        let _ = device.poll(wgpu::PollType::Poll);
        if errors.count() != 0 {
            return Err(NativeRasterError::PipelinePrewarm(
                errors
                    .first()
                    .unwrap_or_else(|| "uncaptured validation error".into()),
            ));
        }

        let layout = PhysicalMemoryLayout::try_new(NATIVE_FILL_RDRAM_BYTES)
            .map_err(NativeRasterError::Ir)?;
        let targets = ColorTargetRegistry::try_new(layout, 1)?;
        Ok(NativeRasterDeviceOutcome::Ready(Box::new(
            NativeRasterRenderer {
                _instance: instance,
                adapter_info: adapter.get_info(),
                device,
                queue,
                raster_pipeline,
                vi_pipeline,
                raster_parameters,
                vi_parameters,
                target_rgba16,
                post_vi_bgra8,
                readback,
                raster_bind_group,
                vi_bind_group,
                completion_authority: self.completion_authority,
                targets,
                presentation,
                next_native_ordinal: 0,
                errors,
            },
        )))
    }
}

#[derive(Default)]
struct BoundedErrorSink {
    count: AtomicUsize,
    first: Mutex<Option<String>>,
}

impl BoundedErrorSink {
    fn record(&self, error: String) {
        let _ = self
            .count
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |count| {
                count.checked_add(1)
            });
        let mut first = self.first.lock().expect("wgpu error sink poisoned");
        if first.is_none() {
            *first = Some(error);
        }
    }

    fn count(&self) -> usize {
        self.count.load(Ordering::Relaxed)
    }

    fn first(&self) -> Option<String> {
        self.first.lock().expect("wgpu error sink poisoned").clone()
    }
}

pub struct NativeRasterRenderer {
    _instance: wgpu::Instance,
    adapter_info: wgpu::AdapterInfo,
    device: wgpu::Device,
    queue: wgpu::Queue,
    raster_pipeline: wgpu::ComputePipeline,
    vi_pipeline: wgpu::ComputePipeline,
    raster_parameters: wgpu::Buffer,
    vi_parameters: wgpu::Buffer,
    target_rgba16: wgpu::Buffer,
    post_vi_bgra8: wgpu::Buffer,
    readback: wgpu::Buffer,
    raster_bind_group: wgpu::BindGroup,
    vi_bind_group: wgpu::BindGroup,
    completion_authority: BackendCompletionAuthority,
    targets: ColorTargetRegistry,
    presentation: ValidatedM3dPresentation,
    next_native_ordinal: u64,
    errors: Arc<BoundedErrorSink>,
}

impl NativeRasterRenderer {
    pub const fn adapter_info(&self) -> &wgpu::AdapterInfo {
        &self.adapter_info
    }

    pub fn resident_targets(&self) -> &[ResidentColorTarget] {
        self.targets.residents()
    }

    pub fn submit_native_fill<'renderer, 'state>(
        &'renderer mut self,
        prepared: PreparedNativeFill<'state>,
    ) -> Result<InFlightNativeRasterFill<'renderer, 'state>, NativeRasterError> {
        let error_count = self.errors.count();
        if error_count != 0 {
            return Err(NativeRasterError::DevicePoisoned {
                count: error_count,
                first: self.errors.first(),
            });
        }

        let target = prepared.target();
        let extent = ColorTargetExtent::try_new(target.width(), target.height())?;
        let format = ColorTargetFormat::try_from_rdp(target.format(), target.size())?;
        let key = ColorTargetKey::try_new(target.range().start(), extent, format)?;
        if key.range() != target.range() {
            return Err(NativeRasterError::TargetBindingMismatch {
                field: "physical target range",
            });
        }
        let candidate = self.targets.begin_candidate(key)?;
        let rectangle = TargetRectangle::try_new(0, 0, extent.width(), extent.height())?;
        let plan = candidate.plan_rows(rectangle)?;
        let fill = prepared
            .state_delta()
            .fill_color()
            .ok_or(NativeRasterError::TargetBindingMismatch {
                field: "fill-color state",
            })?
            .value();
        let upper = (fill >> 16) as u16;
        let lower = fill as u16;
        if upper != lower {
            return Err(NativeRasterError::TargetBindingMismatch {
                field: "RGBA16 fill halves",
            });
        }

        let native_ordinal = self.next_native_ordinal;
        self.next_native_ordinal = self
            .next_native_ordinal
            .checked_add(1)
            .ok_or(NativeRasterError::NativeSubmissionOrdinalExhausted)?;
        let binding = RasterCompletionBinding {
            frame: prepared.binding(),
            key,
            generation: candidate.generation(),
            range: key.range(),
            native_ordinal,
        };

        let mut raster_parameters = Vec::with_capacity(RASTER_PARAMETER_BYTES as usize);
        raster_parameters.extend_from_slice(&u32::from(upper).to_le_bytes());
        raster_parameters.extend_from_slice(&extent.width().to_le_bytes());
        raster_parameters.extend_from_slice(&extent.height().to_le_bytes());
        raster_parameters.extend_from_slice(&0_u32.to_le_bytes());
        self.queue
            .write_buffer(&self.raster_parameters, 0, &raster_parameters);

        let vi_plan = self.presentation.plans().vi();
        let mut vi_parameters = Vec::with_capacity(VI_PARAMETER_BYTES as usize);
        vi_parameters.extend_from_slice(&vi_plan.output().extent().width.to_le_bytes());
        vi_parameters.extend_from_slice(&vi_plan.output().extent().height.to_le_bytes());
        vi_parameters.extend_from_slice(&vi_plan.source().stride_pixels().to_le_bytes());
        vi_parameters.extend_from_slice(&0_u32.to_le_bytes());
        self.queue
            .write_buffer(&self.vi_parameters, 0, &vi_parameters);

        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("fn64-m3.3c-native-fill-submit"),
            });
        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("fn64-m3.3c-native-fill-pass"),
                timestamp_writes: None,
            });
            pass.set_pipeline(&self.raster_pipeline);
            pass.set_bind_group(0, &self.raster_bind_group, &[]);
            pass.dispatch_workgroups(1, 1, 1);
            pass.set_pipeline(&self.vi_pipeline);
            pass.set_bind_group(0, &self.vi_bind_group, &[]);
            pass.dispatch_workgroups(1, 1, 1);
        }
        encoder.copy_buffer_to_buffer(
            &self.target_rgba16,
            0,
            &self.readback,
            0,
            DEVICE_RGBA16_BYTES,
        );
        encoder.copy_buffer_to_buffer(
            &self.post_vi_bgra8,
            0,
            &self.readback,
            DEVICE_RGBA16_BYTES,
            POST_VI_BGRA8_BYTES,
        );
        let (callback_sender, callback_receiver) = mpsc::sync_channel(1);
        encoder.on_submitted_work_done(move || {
            let _ = callback_sender.try_send(());
        });
        let native_submission = self.queue.submit([encoder.finish()]);
        let contract = prepared.begin();

        Ok(InFlightNativeRasterFill {
            renderer: self,
            contract,
            candidate,
            plan,
            binding,
            native_submission,
            callback_receiver,
        })
    }
}

pub struct InFlightNativeRasterFill<'renderer, 'state> {
    renderer: &'renderer mut NativeRasterRenderer,
    contract: ContractInFlightNativeFill<'state>,
    candidate: CandidateColorTarget,
    plan: ExactRowPlan,
    binding: RasterCompletionBinding,
    native_submission: wgpu::SubmissionIndex,
    callback_receiver: mpsc::Receiver<()>,
}

impl<'renderer, 'state> InFlightNativeRasterFill<'renderer, 'state> {
    pub const fn binding(&self) -> NativeFrameBinding {
        self.binding.frame
    }

    pub fn complete(
        self,
    ) -> Result<PendingNativeRasterCommit<'renderer, 'state>, NativeRasterError> {
        // Interleaving closed: a later queue submission may complete first, but
        // it cannot authorize this candidate. This owner waits the exact
        // SubmissionIndex stored beside the semantic/target binding, then
        // observes the callback registered on that same command encoder before
        // mapping the only bounded readback buffer.
        self.renderer
            .device
            .poll(wgpu::PollType::Wait {
                submission_index: Some(self.native_submission),
                timeout: Some(POLL_TIMEOUT),
            })
            .map_err(|error| NativeRasterError::ExactSubmissionWait(error.to_string()))?;
        self.callback_receiver
            .recv_timeout(CALLBACK_TIMEOUT)
            .map_err(|_| NativeRasterError::CompletionCallbackNotObserved)?;

        let (sender, receiver) = mpsc::sync_channel(1);
        self.renderer
            .readback
            .slice(..)
            .map_async(wgpu::MapMode::Read, move |result| {
                let _ = sender.try_send(result);
            });
        let readback = WgpuMappedReadback {
            device: &self.renderer.device,
            buffer: &self.renderer.readback,
            receiver,
        };
        let output = finish_mapped_readback(&readback)?;

        let error_count = self.renderer.errors.count();
        if error_count != 0 {
            return Err(NativeRasterError::DevicePoisoned {
                count: error_count,
                first: self.renderer.errors.first(),
            });
        }
        let completion = finish_exact_completion(
            self.binding,
            RasterCompletionObservation {
                binding: self.binding,
                exact_wait_complete: true,
                callback_observed: true,
                readback_complete: true,
            },
            output,
        )?;
        let (initialized, native_output) =
            completion.into_outputs(self.candidate, self.plan, self.renderer.presentation)?;

        let publication = self.renderer.targets.prepare_publication(initialized)?;
        let pending = self
            .contract
            .complete(&mut self.renderer.completion_authority, native_output)?;
        Ok(PendingNativeRasterCommit {
            native: pending,
            publication,
        })
    }
}

trait MappedReadback {
    type Range: core::ops::Deref<Target = [u8]>;

    fn wait_for_mapping(&self) -> Result<(), NativeRasterError>;
    fn observe_mapping(&self) -> Result<(), NativeRasterError>;
    fn mapped_range(&self) -> Result<Self::Range, NativeRasterError>;
    fn unmap(&self);
}

struct WgpuMappedReadback<'resource> {
    device: &'resource wgpu::Device,
    buffer: &'resource wgpu::Buffer,
    receiver: mpsc::Receiver<Result<(), wgpu::BufferAsyncError>>,
}

impl MappedReadback for WgpuMappedReadback<'_> {
    type Range = wgpu::BufferView;

    fn wait_for_mapping(&self) -> Result<(), NativeRasterError> {
        self.device
            .poll(wgpu::PollType::Wait {
                submission_index: None,
                timeout: Some(POLL_TIMEOUT),
            })
            .map_err(|error| NativeRasterError::Readback(error.to_string()))?;
        Ok(())
    }

    fn observe_mapping(&self) -> Result<(), NativeRasterError> {
        self.receiver
            .recv_timeout(CALLBACK_TIMEOUT)
            .map_err(|_| NativeRasterError::Readback("map callback timeout".into()))?
            .map_err(|error| NativeRasterError::Readback(error.to_string()))
    }

    fn mapped_range(&self) -> Result<Self::Range, NativeRasterError> {
        self.buffer
            .slice(..)
            .get_mapped_range()
            .map_err(|error| NativeRasterError::Readback(error.to_string()))
    }

    fn unmap(&self) {
        self.buffer.unmap();
    }
}

struct ReadbackUnmapGuard<'readback, R: MappedReadback + ?Sized>(&'readback R);

impl<R: MappedReadback + ?Sized> Drop for ReadbackUnmapGuard<'_, R> {
    fn drop(&mut self) {
        self.0.unmap();
    }
}

fn finish_mapped_readback(
    readback: &(impl MappedReadback + ?Sized),
) -> Result<Vec<u8>, NativeRasterError> {
    // Interleaving closed: if submission N fails after map_async, its mapping
    // must be cancelled or unmapped before the exclusive renderer borrow is
    // released and submission N+1 reuses this one buffer. Every return below,
    // including poll/timeout/map-result/range failure, crosses this guard.
    let unmap = ReadbackUnmapGuard(readback);
    readback.wait_for_mapping()?;
    readback.observe_mapping()?;
    let mapped = readback.mapped_range()?;
    let output = mapped.to_vec();
    drop(mapped);
    drop(unmap);
    Ok(output)
}

pub struct PendingNativeRasterCommit<'renderer, 'state> {
    native: PendingNativeCommit<'state>,
    publication: ResidentPublication<'renderer>,
}

impl<'renderer, 'state> PendingNativeRasterCommit<'renderer, 'state> {
    pub const fn binding(&self) -> NativeFrameBinding {
        self.native.binding()
    }

    pub const fn guest_writeback_storage(&self) -> &N64RecompRdramStorageBytes {
        self.native.guest_writeback_storage()
    }

    pub fn commit_guest<E>(
        self,
        commit: impl FnOnce(
            fn64_render_ir::GpuCompleteTicket,
            &N64RecompRdramStorageBytes,
        ) -> Result<fn64_render_ir::GuestCommittedTicket, E>,
    ) -> Result<CommittedNativeRasterFrame<'renderer, 'state>, NativeGuestCommitError<E>> {
        let native = self.native.commit_guest(commit)?;
        let resident = self.publication.publish();
        Ok(CommittedNativeRasterFrame { native, resident })
    }
}

pub struct CommittedNativeRasterFrame<'renderer, 'state> {
    native: crate::CommittedNativeFrame<'state>,
    resident: &'renderer ResidentColorTarget,
}

impl<'renderer, 'state> CommittedNativeRasterFrame<'renderer, 'state> {
    pub const fn native_frame(&self) -> &crate::CommittedNativeFrame<'state> {
        &self.native
    }

    pub const fn resident_target(&self) -> &'renderer ResidentColorTarget {
        self.resident
    }

    pub fn into_guest_ticket(self) -> fn64_render_ir::GuestCommittedTicket {
        self.native.into_guest_ticket()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct RasterCompletionBinding {
    frame: NativeFrameBinding,
    key: ColorTargetKey,
    generation: TargetGeneration,
    range: fn64_render_ir::PhysicalRange,
    native_ordinal: u64,
}

#[derive(Clone, Copy)]
struct RasterCompletionObservation {
    binding: RasterCompletionBinding,
    exact_wait_complete: bool,
    callback_observed: bool,
    readback_complete: bool,
}

#[derive(Debug)]
struct ExactRasterGpuCompletion {
    binding: RasterCompletionBinding,
    device_rgba16: Box<[u8]>,
    post_vi_bgra8: Box<[u8]>,
}

fn finish_exact_completion(
    expected: RasterCompletionBinding,
    observed: RasterCompletionObservation,
    bytes: Vec<u8>,
) -> Result<ExactRasterGpuCompletion, NativeRasterError> {
    for (matches, field) in [
        (expected.frame == observed.binding.frame, "semantic frame"),
        (expected.key == observed.binding.key, "target key"),
        (
            expected.generation == observed.binding.generation,
            "target generation",
        ),
        (expected.range == observed.binding.range, "target range"),
        (
            expected.native_ordinal == observed.binding.native_ordinal,
            "native submission ordinal",
        ),
    ] {
        if !matches {
            return Err(NativeRasterError::CompletionBindingMismatch { field });
        }
    }
    for (complete, missing) in [
        (observed.exact_wait_complete, "the exact indexed GPU wait"),
        (
            observed.callback_observed,
            "the exact submission completion callback",
        ),
        (observed.readback_complete, "the bounded GPU readback"),
    ] {
        if !complete {
            return Err(NativeRasterError::EarlyCompletion { missing });
        }
    }
    if bytes.len() != READBACK_BYTES as usize {
        return Err(NativeRasterError::OutputLength {
            expected: READBACK_BYTES as usize,
            actual: bytes.len(),
        });
    }
    let (device, post_vi) = bytes.split_at(DEVICE_RGBA16_BYTES as usize);
    Ok(ExactRasterGpuCompletion {
        binding: expected,
        device_rgba16: device.into(),
        post_vi_bgra8: post_vi.into(),
    })
}

impl ExactRasterGpuCompletion {
    fn into_outputs(
        self,
        candidate: CandidateColorTarget,
        plan: ExactRowPlan,
        presentation: ValidatedM3dPresentation,
    ) -> Result<(InitializedCandidateColorTarget, NativeGpuOutput), NativeRasterError> {
        if plan.key() != self.binding.key
            || plan.generation() != self.binding.generation
            || plan.key().range() != self.binding.range
        {
            return Err(NativeRasterError::CompletionBindingMismatch {
                field: "exact row plan",
            });
        }
        compare_output(
            "logical/device RGBA16 bytes",
            &self.device_rgba16,
            &NATIVE_FILL_DEVICE_RGBA16,
        )?;

        let device_color = DeviceColorBytes {
            key: self.binding.key,
            generation: self.binding.generation,
            format: ColorTargetFormat::Rgba16,
            bytes: self.device_rgba16.clone(),
        };
        let completed = CompletedColorTargetWrite {
            key: self.binding.key,
            generation: self.binding.generation,
            range: self.binding.range,
            rectangle: plan.rectangle(),
            device_bytes: device_color,
        };
        let initialized = candidate.admit_completed_initialization(completed)?;

        let device = DeviceRgba16Bytes::from_device_bytes(self.device_rgba16.into_vec());
        let mut storage = device.device_bytes().to_vec();
        for pair in storage.chunks_exact_mut(2) {
            pair.swap(0, 1);
        }
        let pixels = unpack_device_pixels(ColorTargetFormat::Rgba16, device.device_bytes())?;
        let mut native_rgba8 = Vec::with_capacity(NATIVE_FILL_NATIVE_RGBA8.len());
        for pixel in &pixels {
            native_rgba8.extend_from_slice(&[pixel.red, pixel.green, pixel.blue, pixel.alpha]);
        }
        compare_output(
            "native RGBA8 target bytes",
            &native_rgba8,
            &NATIVE_FILL_NATIVE_RGBA8,
        )?;

        let oracle = execute_cpu_oracle(presentation, &device)?;
        compare_output(
            "bounded M3.3d post-VI BGRA8 bytes",
            &self.post_vi_bgra8,
            oracle.bgra8(),
        )?;
        compare_output(
            "frozen post-VI BGRA8 bytes",
            &self.post_vi_bgra8,
            &NATIVE_FILL_POST_VI_BGRA8,
        )?;

        let output = NativeGpuOutput::from_typed_domains(
            device,
            N64RecompRdramStorageBytes::from_n64recomp_storage_bytes(storage),
            native_rgba8,
            self.post_vi_bgra8.into_vec(),
        );
        Ok((initialized, output))
    }
}

fn compare_output(
    field: &'static str,
    actual: &[u8],
    expected: &[u8],
) -> Result<(), NativeRasterError> {
    if actual != expected {
        return Err(NativeRasterError::OutputMismatch {
            field,
            expected: expected.into(),
            actual: actual.into(),
        });
    }
    Ok(())
}

#[derive(Debug)]
pub enum NativeRasterError {
    RequestAdapter(String),
    RequestDevice(String),
    PipelinePrewarm(String),
    DevicePoisoned {
        count: usize,
        first: Option<String>,
    },
    NativeSubmissionOrdinalExhausted,
    TargetBindingMismatch {
        field: &'static str,
    },
    ExactSubmissionWait(String),
    CompletionCallbackNotObserved,
    Readback(String),
    CompletionBindingMismatch {
        field: &'static str,
    },
    EarlyCompletion {
        missing: &'static str,
    },
    OutputLength {
        expected: usize,
        actual: usize,
    },
    OutputMismatch {
        field: &'static str,
        expected: Box<[u8]>,
        actual: Box<[u8]>,
    },
    Target(TargetError),
    Contract(NativeContractError),
    ViValidation(ViValidationError),
    ViExecution(ViExecutionError),
    Ir(fn64_render_ir::ValidationError),
}

impl fmt::Display for NativeRasterError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::RequestAdapter(reason) => {
                write!(formatter, "wgpu adapter request failed: {reason}")
            }
            Self::RequestDevice(reason) => {
                write!(formatter, "wgpu device request failed: {reason}")
            }
            Self::PipelinePrewarm(reason) => {
                write!(formatter, "M3.3c pipeline prewarm failed: {reason}")
            }
            Self::DevicePoisoned { count, first } => write!(
                formatter,
                "M3.3c device recorded {count} uncaptured errors; first={first:?}"
            ),
            Self::NativeSubmissionOrdinalExhausted => {
                formatter.write_str("M3.3c native submission ordinal exhausted")
            }
            Self::TargetBindingMismatch { field } => {
                write!(formatter, "M3.3c prepared target differs at {field}")
            }
            Self::ExactSubmissionWait(reason) => {
                write!(
                    formatter,
                    "exact M3.3c wgpu submission wait failed: {reason}"
                )
            }
            Self::CompletionCallbackNotObserved => formatter.write_str(
                "M3.3c completion callback was not observable after exact submission wait",
            ),
            Self::Readback(reason) => write!(formatter, "M3.3c readback failed: {reason}"),
            Self::CompletionBindingMismatch { field } => {
                write!(formatter, "M3.3c completion belongs to a different {field}")
            }
            Self::EarlyCompletion { missing } => {
                write!(formatter, "M3.3c completion attempted before {missing}")
            }
            Self::OutputLength { expected, actual } => write!(
                formatter,
                "M3.3c bounded readback has {actual} bytes; expected {expected}"
            ),
            Self::OutputMismatch {
                field,
                expected,
                actual,
            } => write!(
                formatter,
                "M3.3c {field} mismatch: expected {} bytes, observed {} bytes",
                expected.len(),
                actual.len()
            ),
            Self::Target(error) => error.fmt(formatter),
            Self::Contract(error) => error.fmt(formatter),
            Self::ViValidation(error) => error.fmt(formatter),
            Self::ViExecution(error) => error.fmt(formatter),
            Self::Ir(error) => error.fmt(formatter),
        }
    }
}

impl std::error::Error for NativeRasterError {}

impl From<TargetError> for NativeRasterError {
    fn from(error: TargetError) -> Self {
        Self::Target(error)
    }
}

impl From<NativeContractError> for NativeRasterError {
    fn from(error: NativeContractError) -> Self {
        Self::Contract(error)
    }
}

impl From<ViValidationError> for NativeRasterError {
    fn from(error: ViValidationError) -> Self {
        Self::ViValidation(error)
    }
}

impl From<ViExecutionError> for NativeRasterError {
    fn from(error: ViExecutionError) -> Self {
        Self::ViExecution(error)
    }
}

#[cfg(test)]
mod tests;
