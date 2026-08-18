use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::Duration;

use fn64_render_ir::{BackendCompletionAuthority, SubmittedTicket};

use crate::lifecycle::{
    decode_fill_fixture, finalize_completion, CompletionBinding, CompletionObservation,
    FillFixture, WgpuRenderError,
};
use crate::WgpuBackendCompletion;

const POLL_TIMEOUT: Duration = Duration::from_secs(10);
const CALLBACK_TIMEOUT: Duration = Duration::from_secs(1);
const OUTPUT_BYTES: u64 = crate::FILL_FIXTURE_BYTES as u64;
const SHADER: &str = include_str!("fill_fixture.wgsl");

pub(crate) mod adapter_selection;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum HeadlessBackend {
    #[default]
    AnyNative,
    Metal,
    Vulkan,
    Dx12,
}

impl HeadlessBackend {
    pub(crate) fn wgpu_backends(self) -> wgpu::Backends {
        adapter_selection::backends_for_request(self.native_wgpu_backends())
    }

    /// The mask this variant names on hardware, before
    /// `FN64_WGPU_SOFTWARE_ADAPTER` gets a say. Split out so the software
    /// rewrite has an un-rewritten input to be tested against.
    pub(crate) fn native_wgpu_backends(self) -> wgpu::Backends {
        match self {
            Self::AnyNative => {
                wgpu::Backends::METAL | wgpu::Backends::VULKAN | wgpu::Backends::DX12
            }
            Self::Metal => wgpu::Backends::METAL,
            Self::Vulkan => wgpu::Backends::VULKAN,
            Self::Dx12 => wgpu::Backends::DX12,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NoAdapter {
    requested: HeadlessBackend,
}

impl NoAdapter {
    pub(crate) const fn new(requested: HeadlessBackend) -> Self {
        Self { requested }
    }

    pub const fn requested(self) -> HeadlessBackend {
        self.requested
    }
}

pub enum HeadlessDeviceOutcome {
    Ready(Box<PrewarmedRenderer>),
    NoAdapter(NoAdapter),
}

pub struct UninitializedRenderer {
    backend: HeadlessBackend,
    completion_authority: BackendCompletionAuthority,
}

impl UninitializedRenderer {
    pub const fn new(
        backend: HeadlessBackend,
        completion_authority: BackendCompletionAuthority,
    ) -> Self {
        Self {
            backend,
            completion_authority,
        }
    }

    pub async fn request(self) -> Result<HeadlessDeviceOutcome, WgpuRenderError> {
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
                return Ok(HeadlessDeviceOutcome::NoAdapter(NoAdapter::new(
                    self.backend,
                )));
            }
            Err(error) => return Err(WgpuRenderError::RequestAdapter(error.to_string())),
        };
        adapter_selection::assert_expected_adapter(&adapter);
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                label: Some("fn64-render-wgpu-m3.1"),
                required_features: wgpu::Features::empty(),
                required_limits: wgpu::Limits::default(),
                experimental_features: wgpu::ExperimentalFeatures::disabled(),
                memory_hints: wgpu::MemoryHints::MemoryUsage,
                trace: wgpu::Trace::Off,
            })
            .await
            .map_err(|error| WgpuRenderError::RequestDevice(error.to_string()))?;

        let errors = Arc::new(BoundedErrorSink::default());
        let uncaptured = Arc::clone(&errors);
        device.on_uncaptured_error(Arc::new(move |error| {
            uncaptured.record(error.to_string());
        }));

        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("fn64-m3.1-fill-fixture"),
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });
        let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("fn64-m3.1-fill-fixture"),
            layout: None,
            module: &shader,
            entry_point: Some("fill"),
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            cache: None,
        });
        let parameters = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("fn64-m3.1-fill-parameters"),
            size: 16,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let output = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("fn64-m3.1-fill-output"),
            size: OUTPUT_BYTES,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        let readback = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("fn64-m3.1-fill-readback"),
            size: OUTPUT_BYTES,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let layout = pipeline.get_bind_group_layout(0);
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("fn64-m3.1-fill-bind-group"),
            layout: &layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: parameters.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: output.as_entire_binding(),
                },
            ],
        });
        let _ = device.poll(wgpu::PollType::Poll);
        if errors.count() != 0 {
            return Err(WgpuRenderError::PipelinePrewarm(
                errors
                    .first()
                    .unwrap_or_else(|| "uncaptured validation error".into()),
            ));
        }

        Ok(HeadlessDeviceOutcome::Ready(Box::new(PrewarmedRenderer {
            _instance: instance,
            adapter_info: adapter.get_info(),
            device,
            queue,
            pipeline,
            parameters,
            output,
            readback,
            bind_group,
            completion_authority: self.completion_authority,
            next_native_ordinal: 0,
            errors,
        })))
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

pub struct PrewarmedRenderer {
    _instance: wgpu::Instance,
    adapter_info: wgpu::AdapterInfo,
    device: wgpu::Device,
    queue: wgpu::Queue,
    pipeline: wgpu::ComputePipeline,
    parameters: wgpu::Buffer,
    output: wgpu::Buffer,
    readback: wgpu::Buffer,
    bind_group: wgpu::BindGroup,
    completion_authority: BackendCompletionAuthority,
    next_native_ordinal: u64,
    errors: Arc<BoundedErrorSink>,
}

impl PrewarmedRenderer {
    pub const fn adapter_info(&self) -> &wgpu::AdapterInfo {
        &self.adapter_info
    }

    pub fn submit_fill_full_sync(
        &mut self,
        submitted: SubmittedTicket,
    ) -> Result<InFlightFill<'_>, WgpuRenderError> {
        let error_count = self.errors.count();
        if error_count != 0 {
            return Err(WgpuRenderError::DevicePoisoned {
                count: error_count,
                first: self.errors.first(),
            });
        }
        let fixture = decode_fill_fixture(&submitted)?;
        let native_ordinal = self.next_native_ordinal;
        self.next_native_ordinal = self
            .next_native_ordinal
            .checked_add(1)
            .ok_or(WgpuRenderError::NativeSubmissionOrdinalExhausted)?;
        let binding = CompletionBinding::from_submitted(&submitted, native_ordinal);

        self.queue
            .write_buffer(&self.parameters, 0, &fixture.fill_rgba);
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("fn64-m3.1-fill-submit"),
            });
        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("fn64-m3.1-fill-pass"),
                timestamp_writes: None,
            });
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, &self.bind_group, &[]);
            pass.dispatch_workgroups(1, 1, 1);
        }
        encoder.copy_buffer_to_buffer(&self.output, 0, &self.readback, 0, OUTPUT_BYTES);
        let (callback_sender, callback_receiver) = mpsc::sync_channel(1);
        encoder.on_submitted_work_done(move || {
            let _ = callback_sender.try_send(());
        });
        let native_submission = self.queue.submit([encoder.finish()]);
        Ok(InFlightFill {
            renderer: self,
            submitted,
            fixture,
            binding,
            native_submission,
            callback_receiver,
        })
    }
}

pub struct InFlightFill<'renderer> {
    renderer: &'renderer mut PrewarmedRenderer,
    submitted: SubmittedTicket,
    fixture: FillFixture,
    binding: CompletionBinding,
    native_submission: wgpu::SubmissionIndex,
    callback_receiver: mpsc::Receiver<()>,
}

impl InFlightFill<'_> {
    pub fn complete(self) -> Result<WgpuBackendCompletion, WgpuRenderError> {
        self.renderer
            .device
            .poll(wgpu::PollType::Wait {
                submission_index: Some(self.native_submission),
                timeout: Some(POLL_TIMEOUT),
            })
            .map_err(|error| WgpuRenderError::ExactSubmissionWait(error.to_string()))?;
        self.callback_receiver
            .recv_timeout(CALLBACK_TIMEOUT)
            .map_err(|_| WgpuRenderError::CompletionCallbackNotObserved)?;

        let (sender, receiver) = mpsc::sync_channel(1);
        self.renderer
            .readback
            .slice(..)
            .map_async(wgpu::MapMode::Read, move |result| {
                let _ = sender.try_send(result);
            });
        self.renderer
            .device
            .poll(wgpu::PollType::Wait {
                submission_index: None,
                timeout: Some(POLL_TIMEOUT),
            })
            .map_err(|error| WgpuRenderError::Readback(error.to_string()))?;
        receiver
            .recv_timeout(CALLBACK_TIMEOUT)
            .map_err(|_| WgpuRenderError::Readback("map callback timeout".into()))?
            .map_err(|error| WgpuRenderError::Readback(error.to_string()))?;
        let mapped = self
            .renderer
            .readback
            .slice(..)
            .get_mapped_range()
            .map_err(|error| WgpuRenderError::Readback(error.to_string()))?;
        let output = mapped.to_vec();
        drop(mapped);
        self.renderer.readback.unmap();

        let error_count = self.renderer.errors.count();
        if error_count != 0 {
            return Err(WgpuRenderError::DevicePoisoned {
                count: error_count,
                first: self.renderer.errors.first(),
            });
        }
        let observation = CompletionObservation {
            binding: self.binding,
            exact_wait_complete: true,
            callback_observed: true,
            readback_complete: true,
        };
        finalize_completion(
            &mut self.renderer.completion_authority,
            self.submitted,
            self.fixture,
            self.binding,
            observation,
            output,
        )
    }
}

#[cfg(all(test, feature = "host-gpu-tests"))]
mod host_gpu_tests {
    use std::future::Future;
    use std::pin::pin;
    use std::sync::Arc;
    use std::task::{Context, Poll, Wake, Waker};

    use super::*;
    use crate::lifecycle::tests_support;

    #[test]
    fn required_host_executes_exact_fill_full_sync_to_receipted_gpu_completion() {
        let (submitted, backend_authority) =
            tests_support::submitted(crate::FILL_FIXTURE_TEST_COLOR);
        let requested = block_on(
            UninitializedRenderer::new(HeadlessBackend::AnyNative, backend_authority).request(),
        )
        .unwrap();
        let mut renderer = match requested {
            HeadlessDeviceOutcome::Ready(renderer) => renderer,
            HeadlessDeviceOutcome::NoAdapter(no_adapter) => panic!(
                "required host GPU evidence unavailable: typed no-adapter for {:?}",
                no_adapter.requested()
            ),
        };
        let completion = renderer
            .submit_fill_full_sync(submitted)
            .unwrap()
            .complete()
            .unwrap();
        assert_eq!(
            completion.staged_effects()[0].bytes(),
            crate::FILL_FIXTURE_TEST_OUTPUT
        );
        assert_eq!(completion.native_completion().native_ordinal(), 0);
    }

    fn block_on<F: Future>(future: F) -> F::Output {
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
}
