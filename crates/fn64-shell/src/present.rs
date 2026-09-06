//! The window presentation surface: one `wgpu::Surface`, one staging texture
//! the guest field is uploaded into, one fullscreen blit per presented field.
//!
//! This replaces the `pixels` crate, which provided exactly this shape but
//! pinned wgpu 0.19 while `fn64-render-wgpu` builds on wgpu 30 -- so the
//! workspace compiled two wgpu, two naga, and two `objc2` trees, and the old
//! tree dragged in `block 0.1.6`, which rustc flags as future-incompatible.
//! Nothing here is new capability: the fullscreen triangle, the 4:3 viewport
//! math, and the nearest sampler all already lived in `zoom_fill.rs`, which
//! reached through `pixels` for the device and the texture. This module owns
//! those objects directly instead.
//!
//! **The upload path is deliberately the same shape `pixels` used**: the
//! caller writes tightly-packed RGBA8888 into a CPU-side staging buffer
//! ([`Presenter::frame_mut`]) and one `write_texture` per presented field
//! moves it to the GPU. The guest field's bytes are therefore unchanged by
//! this port -- the frame tripwire hashes that CPU buffer, upstream of every
//! GPU object here.
//!
//! The texture format is `Rgba8UnormSrgb`, matching what `pixels` created,
//! so the sampled values and the sRGB encoding the blit performs are the
//! same. `wgpu::TextureFormat::Rgba8UnormSrgb` is not necessarily the
//! surface's own preferred format; the surface is configured with whatever
//! the adapter reports, and the blit converts.

/// Why a presentation could not be formed or submitted. Presentation is a
/// loud failure path in this shell: `main.rs` prints the reason and, for
/// setup, exits rather than running a window that shows nothing.
#[derive(Debug)]
pub enum PresentError {
    /// No adapter, device, or surface could be created for this window.
    Setup(String),
    /// The swapchain could not hand out a texture this frame, for a reason
    /// that is not the ordinary reconfigure-and-retry case.
    Surface(&'static str),
}

impl std::fmt::Display for PresentError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Setup(message) => write!(f, "presentation setup failed: {message}"),
            Self::Surface(status) => write!(f, "surface acquisition failed: {status}"),
        }
    }
}

impl std::error::Error for PresentError {}

/// The window's presentation resources.
///
/// Field order matters for drop: wgpu requires the surface to be dropped
/// before the device it was created from, and Rust drops struct fields in
/// declaration order.
pub struct Presenter {
    surface: wgpu::Surface<'static>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    surface_format: wgpu::TextureFormat,
    surface_size: (u32, u32),
    /// The staging texture the guest field is uploaded into. Its extent is
    /// the VI field's sample dimensions, NOT the window's.
    texture: wgpu::Texture,
    texture_size: (u32, u32),
    /// CPU-side tightly-packed RGBA8888 of the pending field. Owned here so
    /// `frame_mut`/`render` keep the previous presenter's call shape.
    frame: Vec<u8>,
    /// Set when the swapchain reported Suboptimal. Acted on AFTER the frame
    /// is presented: reconfiguring while a texture from the old
    /// configuration is still outstanding is what this defers.
    needs_reconfigure: bool,
}

impl Presenter {
    /// Create the surface for `window` with a `width`x`height` guest field.
    ///
    /// Blocks on adapter and device request. This runs once, during
    /// `resumed`, before any guest time has been established.
    pub fn new(
        window: std::sync::Arc<winit::window::Window>,
        width: u32,
        height: u32,
    ) -> Result<Self, PresentError> {
        let (surface_width, surface_height) = {
            let size = window.inner_size();
            (size.width.max(1), size.height.max(1))
        };
        // The same backend set `fn64-render-wgpu` selects, so the window's
        // surface and the renderer's device come from one instance family.
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            backends: wgpu::Backends::METAL | wgpu::Backends::VULKAN | wgpu::Backends::DX12,
            ..wgpu::InstanceDescriptor::new_without_display_handle()
        });
        let surface = instance
            .create_surface(window)
            .map_err(|e| PresentError::Setup(format!("create_surface: {e}")))?;
        let adapter = pollster_block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::default(),
            force_fallback_adapter: false,
            compatible_surface: Some(&surface),
            apply_limit_buckets: false,
        }))
        .map_err(|e| PresentError::Setup(format!("request_adapter: {e}")))?;
        let (device, queue) = pollster_block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("fn64 presentation device"),
            required_features: wgpu::Features::empty(),
            required_limits: wgpu::Limits::downlevel_defaults().using_resolution(adapter.limits()),
            experimental_features: wgpu::ExperimentalFeatures::disabled(),
            memory_hints: wgpu::MemoryHints::default(),
            trace: wgpu::Trace::Off,
        }))
        .map_err(|e| PresentError::Setup(format!("request_device: {e}")))?;

        let capabilities = surface.get_capabilities(&adapter);
        let surface_format = *capabilities
            .formats
            .first()
            .ok_or_else(|| PresentError::Setup("surface reports no supported format".into()))?;

        let (width, height) = (width.max(1), height.max(1));
        let texture = Self::make_texture(&device, width, height);
        let mut presenter = Self {
            surface,
            device,
            queue,
            surface_format,
            surface_size: (surface_width, surface_height),
            texture,
            texture_size: (width, height),
            frame: vec![0u8; (width as usize) * (height as usize) * 4],
            needs_reconfigure: false,
        };
        presenter.configure_surface();
        Ok(presenter)
    }

    fn make_texture(device: &wgpu::Device, width: u32, height: u32) -> wgpu::Texture {
        device.create_texture(&wgpu::TextureDescriptor {
            label: Some("fn64 presented guest field"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8UnormSrgb,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        })
    }

    fn configure_surface(&mut self) {
        self.surface.configure(
            &self.device,
            &wgpu::SurfaceConfiguration {
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                format: self.surface_format,
                width: self.surface_size.0,
                height: self.surface_size.1,
                present_mode: wgpu::PresentMode::Fifo,
                color_space: wgpu::SurfaceColorSpace::Srgb,
                desired_maximum_frame_latency: 2,
                alpha_mode: wgpu::CompositeAlphaMode::Auto,
                view_formats: vec![],
            },
        );
    }

    pub fn device(&self) -> &wgpu::Device {
        &self.device
    }

    pub fn queue(&self) -> &wgpu::Queue {
        &self.queue
    }

    pub fn texture(&self) -> &wgpu::Texture {
        &self.texture
    }

    /// The format the swapchain hands out, which is what any pipeline
    /// rendering into it must declare as its colour target.
    pub fn surface_texture_format(&self) -> wgpu::TextureFormat {
        self.surface_format
    }

    /// The CPU-side RGBA8888 staging buffer for the next presented field.
    pub fn frame_mut(&mut self) -> &mut [u8] {
        &mut self.frame
    }

    /// Resize the guest field. Replaces the staging texture (so any bind
    /// group sampling it must be rebuilt -- `FramePresenter` detects this by
    /// comparing texture extents) and the CPU buffer.
    pub fn resize_buffer(&mut self, width: u32, height: u32) {
        let (width, height) = (width.max(1), height.max(1));
        if (width, height) == self.texture_size {
            return;
        }
        self.texture = Self::make_texture(&self.device, width, height);
        self.texture_size = (width, height);
        self.frame = vec![0u8; (width as usize) * (height as usize) * 4];
    }

    /// Resize the window surface. Reconfigures the swapchain.
    pub fn resize_surface(&mut self, width: u32, height: u32) {
        let size = (width.max(1), height.max(1));
        if size == self.surface_size {
            return;
        }
        self.surface_size = size;
        self.configure_surface();
    }

    /// Upload the staging buffer, run `encode` against the acquired
    /// swapchain view, submit, and present.
    ///
    /// The upload happens unconditionally, before `encode`: this is one
    /// texture upload per presented field, the contract the module doc
    /// states. `encode` sees a texture already holding this field's bytes.
    pub fn render_with<F>(&mut self, encode: F) -> Result<(), PresentError>
    where
        F: FnOnce(&mut wgpu::CommandEncoder, &wgpu::TextureView, &wgpu::Device, &wgpu::Queue),
    {
        let (width, height) = self.texture_size;
        self.queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &self.texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &self.frame,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(width * 4),
                rows_per_image: Some(height),
            },
            wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
        );

        let surface_texture = match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(texture) => texture,
            // Suboptimal still hands out a usable texture. Reconfiguring is
            // recommended for performance, so do it, and present this frame
            // rather than dropping it -- a dropped frame here would be a
            // silent hole in the tripwire's sequence.
            wgpu::CurrentSurfaceTexture::Suboptimal(texture) => {
                self.needs_reconfigure = true;
                texture
            }
            // Lost, Outdated, Timeout, and Occluded are the ordinary
            // consequences of a resize, a display change, or the window
            // being hidden -- not shell failures. Reconfigure where that is
            // the documented remedy and let the next redraw present.
            // Reporting these as errors would make every monitor change and
            // every minimize print a scary line.
            wgpu::CurrentSurfaceTexture::Lost | wgpu::CurrentSurfaceTexture::Outdated => {
                self.configure_surface();
                return Ok(());
            }
            wgpu::CurrentSurfaceTexture::Timeout | wgpu::CurrentSurfaceTexture::Occluded => {
                return Ok(());
            }
            // A validation error is a real defect in this module, not a
            // transient window state: report it rather than swallowing it.
            wgpu::CurrentSurfaceTexture::Validation => {
                return Err(PresentError::Surface("validation"));
            }
        };
        let view = surface_texture
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("fn64 presentation encoder"),
            });
        encode(&mut encoder, &view, &self.device, &self.queue);
        self.queue.submit(std::iter::once(encoder.finish()));
        self.queue.present(surface_texture);
        if std::mem::take(&mut self.needs_reconfigure) {
            self.configure_surface();
        }
        Ok(())
    }
}

/// Minimal executor for wgpu's futures, which complete without ever yielding
/// to a reactor on the native backends this shell builds for. Avoids taking
/// `pollster` as a dependency for three `poll` calls.
fn pollster_block_on<F: std::future::Future>(mut future: F) -> F::Output {
    use std::task::{Context, Poll, RawWaker, RawWakerVTable, Waker};

    const VTABLE: RawWakerVTable = RawWakerVTable::new(
        |_| RawWaker::new(std::ptr::null(), &VTABLE),
        |_| {},
        |_| {},
        |_| {},
    );
    // Safety: the vtable's operations are all no-ops on a null pointer and
    // never dereference it, which is the documented contract for a waker
    // that is only ever cloned/woken by a completed-immediately future.
    let waker = unsafe { Waker::from_raw(RawWaker::new(std::ptr::null(), &VTABLE)) };
    let mut context = Context::from_waker(&waker);
    // Safety: `future` is owned by this frame and never moved after this
    // pin -- the shadowing binding makes the original inaccessible.
    let mut future = unsafe { std::pin::Pin::new_unchecked(&mut future) };
    loop {
        if let Poll::Ready(output) = future.as_mut().poll(&mut context) {
            return output;
        }
        std::thread::yield_now();
    }
}
