//! Real `wgpu` vertex+fragment triangle pipeline (port card
//! `/private/tmp/fn64-rt64-gpu-pipeline-implementation-card.md`): the
//! smallest slice where the **host GPU** rasterizes a triangle, matching how
//! RT64's own D3D12/Vulkan/Metal backend works, rather than a CPU software
//! rasterizer (explicitly rejected, see the card's header).
//!
//! Modeled on `targets/raster.rs`'s `NativeRasterRenderer` shape (§2a):
//! adapter/device/queue request sequence, `BoundedErrorSink`
//! device-poisoning pattern, exact-submission-wait + callback-channel
//! completion protocol -- all reused verbatim in structure, not shared code
//! (this module owns its own device, a third device-owning path alongside
//! `device/mod.rs` and `targets/raster.rs`, matching that file's own
//! precedent of not sharing a device, port card §5).
//!
//! Restriction set (port card §3): one fixed triangle, hardcoded
//! vertex positions/colors/UVs in the test fixture (no `RawTriangle`
//! decode); opaque only (`blend: None`, no fixed-function blend state --
//! this is the actual "blend is a no-op" mechanism for this slice, not an
//! `OtherMode` bit combination. Independently verified against RT64's own
//! PSO construction (`rt64_raster_shader.cpp:311`, pinned commit
//! `5473732a822a4423b5696e7cb18fecc425a59875`): `renderTargetBlend[0]`
//! defaults to `RenderBlendDesc::Copy()` (fixed-function blend disabled),
//! only overridden to `blendEnabled = true` when the draw's `alphaBlend`
//! flag is set (line 334) -- this fixture never sets it, so `blend: None`
//! matches RT64's own real PSO exactly, not by analogy. Separately, this
//! fixture's `fixed_fixture_other_mode()` wire `(0, 0)` also happens to
//! decode to a `blend_color`/`blend.rs` selector combination
//! (`BlendColorInput::Combined`, cycle count 1) that reduces to `src`
//! unchanged if `blend.rs`'s runtime path were invoked -- but this slice
//! does not invoke it at all, so that coincidence is not the reason blend
//! is a no-op here); textureless (`renderFlagUsesTexture0/1` both false,
//! `tex_val0`/`tex_val1` zeroed in the fragment wrapper); no alpha compare,
//! no coverage-write variation, no decal, no backface culling, no MSAA, no
//! upscaling; smooth-shaded color only.
//!
//! Depth: a real `DepthStencilState` (`depth_write_enabled: true`,
//! `depth_compare: CompareFunction::Less`) -- fixed-function GPU depth-test
//! hardware state, not fragment-shader arithmetic (port card §2b; RT64's own
//! `RasterPS` contains no ordinary Z-compare/write). `Less` is confirmed
//! against RT64's own PSO construction, not by `depth_strict_less.rs`'s name
//! (that module's own doc states it cites only the public N64 Programming
//! Manual/libultra, not RT64 -- coincidental naming, not authority):
//! `rt64_raster_shader.cpp:317` sets `depthFunction = c.zCmp ?
//! RenderComparisonFunction::LESS : RenderComparisonFunction::ALWAYS`, and
//! `RenderComparisonFunction::LESS` maps to each native backend's
//! non-inclusive less-than compare op. Validated post-hoc against
//! `depth_strict_less.rs`'s oracle on the read-back depth buffer as a
//! differential check on wgpu's own depth-test hardware result, not as
//! fragment-shader logic.
//!
//! Nonclaims (port card §7): no RT64 parity claim, no performance claim, no
//! production/`decode_stream` wiring (fixture data only), no texture
//! sampling/alpha-compare/blend/coverage-write/decal/backface-cull/MSAA/
//! upscaling/`SetCombine` decode, no rasterization-algorithm claim of any
//! kind -- coverage determination routes entirely through wgpu's own
//! `TriangleList` primitive state and the host GPU's rasterizer.

use core::fmt;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::Duration;

use crate::device::{HeadlessBackend, NoAdapter};
use crate::shader_manifest::{
    triangle_pipeline_fragment_wgsl, TRIANGLE_PIPELINE_FRAGMENT_ENTRY_POINT,
    TRIANGLE_PIPELINE_VERTEX_ENTRY_POINT, TRIANGLE_PIPELINE_VERTEX_WGSL,
};
use crate::state::OtherMode;
use crate::{neutral_vertex_to_raster_vertex, CombineParams};
use fn64_render::NeutralTriangleVertex;

const POLL_TIMEOUT: Duration = Duration::from_secs(10);
const CALLBACK_TIMEOUT: Duration = Duration::from_secs(1);

const COLOR_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;
const DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;
const RASTER_PARAMS_BYTES: u64 = 32;
const COMBINE_PARAMS_BYTES: u64 = 16;

/// One `RasterVS`-shaped vertex: RDP screen-pixel position, UV (unused by
/// this slice's textureless fragment shader, but present in the layout to
/// keep it stable for a future textured slice -- port card §3 step 1), and
/// interpolated color.
#[derive(Clone, Copy, Debug, PartialEq)]
#[repr(C)]
pub struct RasterVertex {
    pub position: [f32; 4],
    pub uv: [f32; 2],
    pub color: [f32; 4],
}

impl RasterVertex {
    const ATTRIBUTES: [wgpu::VertexAttribute; 3] = wgpu::vertex_attr_array![
        0 => Float32x4,
        1 => Float32x2,
        2 => Float32x4,
    ];

    const fn layout() -> wgpu::VertexBufferLayout<'static> {
        wgpu::VertexBufferLayout {
            array_stride: core::mem::size_of::<RasterVertex>() as wgpu::BufferAddress,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &Self::ATTRIBUTES,
        }
    }

    fn to_bytes(self) -> [u8; 40] {
        let mut bytes = [0u8; 40];
        bytes[0..16].copy_from_slice(bytemuck_f32x4(self.position).as_slice());
        bytes[16..24].copy_from_slice(bytemuck_f32x2(self.uv).as_slice());
        bytes[24..40].copy_from_slice(bytemuck_f32x4(self.color).as_slice());
        bytes
    }
}

fn bytemuck_f32x4(values: [f32; 4]) -> [u8; 16] {
    let mut bytes = [0u8; 16];
    for (chunk, value) in bytes.chunks_exact_mut(4).zip(values) {
        chunk.copy_from_slice(&value.to_le_bytes());
    }
    bytes
}

fn bytemuck_f32x2(values: [f32; 2]) -> [u8; 8] {
    let mut bytes = [0u8; 8];
    for (chunk, value) in bytes.chunks_exact_mut(4).zip(values) {
        chunk.copy_from_slice(&value.to_le_bytes());
    }
    bytes
}

/// `RasterParams.resolution`/`screenScale`/`screenOffset`, matching the
/// vertex shader's `RasterParams` uniform (`shaders/triangle_pipeline_vertex.wgsl`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TriangleRasterParams {
    pub resolution: [f32; 2],
    pub screen_scale: [f32; 2],
    pub screen_offset: [f32; 2],
}

impl TriangleRasterParams {
    fn to_bytes(self) -> [u8; RASTER_PARAMS_BYTES as usize] {
        let mut bytes = [0u8; RASTER_PARAMS_BYTES as usize];
        bytes[0..8].copy_from_slice(&bytemuck_f32x2(self.resolution));
        bytes[8..16].copy_from_slice(&bytemuck_f32x2(self.screen_scale));
        bytes[16..24].copy_from_slice(&bytemuck_f32x2(self.screen_offset));
        // bytes[24..32] left zero: WGSL struct pads to a 16-byte multiple
        // (two trailing f32 reserved fields), matching `RasterParams`'s
        // `reserved_0`/`reserved_1`.
        bytes
    }
}

/// Small fixed render target extent (port card §3: "propose 8x8 or 16x16").
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TriangleTargetExtent {
    pub width: u32,
    pub height: u32,
}

/// One fixed-fixture triangle submission: three vertices, the raster
/// screen-transform uniform, and the fragment stage's caller-supplied
/// literal `CombineParams` (no `SetCombine` decode in this slice -- port
/// card §3 step 3).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TriangleFixture {
    pub vertices: [RasterVertex; 3],
    pub raster_params: TriangleRasterParams,
    pub combine_params: CombineParams,
    pub extent: TriangleTargetExtent,
}

pub enum TrianglePipelineDeviceOutcome {
    Ready(Box<TrianglePipelineRenderer>),
    NoAdapter(NoAdapter),
}

pub struct UninitializedTrianglePipeline {
    backend: HeadlessBackend,
}

impl UninitializedTrianglePipeline {
    pub const fn new(backend: HeadlessBackend) -> Self {
        Self { backend }
    }

    pub async fn request(self) -> Result<TrianglePipelineDeviceOutcome, TrianglePipelineError> {
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
                return Ok(TrianglePipelineDeviceOutcome::NoAdapter(NoAdapter::new(
                    self.backend,
                )));
            }
            Err(error) => return Err(TrianglePipelineError::RequestAdapter(error.to_string())),
        };
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                label: Some("fn64-triangle-pipeline"),
                required_features: wgpu::Features::empty(),
                required_limits: wgpu::Limits::default(),
                experimental_features: wgpu::ExperimentalFeatures::disabled(),
                memory_hints: wgpu::MemoryHints::MemoryUsage,
                trace: wgpu::Trace::Off,
            })
            .await
            .map_err(|error| TrianglePipelineError::RequestDevice(error.to_string()))?;

        let errors = Arc::new(BoundedErrorSink::default());
        let uncaptured = Arc::clone(&errors);
        device.on_uncaptured_error(Arc::new(move |error| {
            uncaptured.record(error.to_string());
        }));

        let vertex_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("fn64-triangle-pipeline-vertex"),
            source: wgpu::ShaderSource::Wgsl(TRIANGLE_PIPELINE_VERTEX_WGSL.into()),
        });
        let fragment_source = triangle_pipeline_fragment_wgsl();
        let fragment_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("fn64-triangle-pipeline-fragment"),
            source: wgpu::ShaderSource::Wgsl(fragment_source.into()),
        });

        let raster_params_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("fn64-triangle-pipeline-raster-params"),
            size: RASTER_PARAMS_BYTES,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let combine_params_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("fn64-triangle-pipeline-combine-params"),
            size: COMBINE_PARAMS_BYTES,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("fn64-triangle-pipeline-bind-group-layout"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ],
        });
        // Prewarm-only bind group: exercised once here so a bind-group-layout
        // mismatch surfaces at `request()` time (the `errors.count()` check
        // below), not on the first real `submit_triangles` call. Each real
        // draw builds its own bind group over its own per-draw uniform
        // buffers (see `submit_triangles`) -- this prewarm instance and its
        // buffers are not retained on the renderer.
        let prewarm_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("fn64-triangle-pipeline-prewarm-bind-group"),
            layout: &bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: raster_params_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: combine_params_buffer.as_entire_binding(),
                },
            ],
        });
        let _ = &prewarm_bind_group;
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("fn64-triangle-pipeline-layout"),
            bind_group_layouts: &[Some(&bind_group_layout)],
            immediate_size: 0,
        });

        let vertex_buffer_layout = RasterVertex::layout();
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("fn64-triangle-pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &vertex_shader,
                entry_point: Some(TRIANGLE_PIPELINE_VERTEX_ENTRY_POINT),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                buffers: &[Some(vertex_buffer_layout)],
            },
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                strip_index_format: None,
                front_face: wgpu::FrontFace::Ccw,
                cull_mode: None,
                unclipped_depth: false,
                polygon_mode: wgpu::PolygonMode::Fill,
                conservative: false,
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: DEPTH_FORMAT,
                depth_write_enabled: Some(true),
                depth_compare: Some(wgpu::CompareFunction::Less),
                stencil: wgpu::StencilState::default(),
                bias: wgpu::DepthBiasState::default(),
            }),
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: &fragment_shader,
                entry_point: Some(TRIANGLE_PIPELINE_FRAGMENT_ENTRY_POINT),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: COLOR_FORMAT,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        });

        let _ = device.poll(wgpu::PollType::Poll);
        if errors.count() != 0 {
            return Err(TrianglePipelineError::PipelinePrewarm(
                errors
                    .first()
                    .unwrap_or_else(|| "uncaptured validation error".into()),
            ));
        }

        Ok(TrianglePipelineDeviceOutcome::Ready(Box::new(
            TrianglePipelineRenderer {
                _instance: instance,
                adapter_info: adapter.get_info(),
                device,
                queue,
                pipeline,
                bind_group_layout,
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

/// Pure precondition check for [`TrianglePipelineRenderer::submit_triangle`]:
/// rejects a zero-width or zero-height target before any device work
/// (buffer/texture creation) happens. Factored out so this rejection is
/// independently testable without a device.
const fn validate_triangle_extent(
    extent: TriangleTargetExtent,
) -> Result<(), TrianglePipelineError> {
    if extent.width == 0 || extent.height == 0 {
        return Err(TrianglePipelineError::ZeroExtent {
            width: extent.width,
            height: extent.height,
        });
    }
    Ok(())
}

pub struct TrianglePipelineRenderer {
    _instance: wgpu::Instance,
    adapter_info: wgpu::AdapterInfo,
    device: wgpu::Device,
    queue: wgpu::Queue,
    pipeline: wgpu::RenderPipeline,
    bind_group_layout: wgpu::BindGroupLayout,
    errors: Arc<BoundedErrorSink>,
}

impl TrianglePipelineRenderer {
    pub const fn adapter_info(&self) -> &wgpu::AdapterInfo {
        &self.adapter_info
    }

    /// Submits one fixed-fixture triangle draw (`draw(0..3, 0..1)`) into a
    /// fresh per-submission color+depth texture pair (matching M3.3c's
    /// per-submission buffer pattern, not a persistent swapchain -- port
    /// card §3 step 6), then reads back both the color and depth targets.
    /// Thin single-draw wrapper over [`Self::submit_triangles`]; see that
    /// method for the multi-draw-same-target shape a real depth-reject
    /// differential needs.
    pub fn submit_triangle(
        &mut self,
        fixture: TriangleFixture,
    ) -> Result<InFlightTriangleDraw<'_>, TrianglePipelineError> {
        self.submit_triangles(&[fixture])
    }

    /// Submits one triangle sourced from a real admitted raw-DPC plan
    /// (`raw_dpc::triangle_draw_data`'s `RetrievedTriangleDraw`, snapshotted
    /// at that triangle's own stream position) rather than a hand-built
    /// fixture: `vertices` are the plan's own decoded `NeutralTriangleVertex`
    /// triple, adapted field-by-field via [`neutral_vertex_to_raster_vertex`]
    /// (no arithmetic), and `combine_params` is the plan's own real
    /// `SetCombine` value, not a caller-supplied literal.
    ///
    /// `other_mode` is accepted (the retrieval glue's other real admitted
    /// value) but not yet consumed by this fixed-fixture pipeline: this
    /// slice's vertex shader (`shaders/triangle_pipeline_vertex.wgsl`)
    /// hardcodes `is_rect = false`/no Z-override, the same restriction the
    /// GPU-pipeline card's own fixed fixture already carries (module doc,
    /// `fixed_fixture_other_mode`) -- a future slice that wires the
    /// Z-override branch into the vertex shader would read it here.
    /// `raster_params`/`extent` are caller-supplied because they describe
    /// the render target/viewport, not RDP command state this card's
    /// admission mechanism carries.
    ///
    /// `resolution`/`screen_scale`/`screen_offset`/depth conversion happen
    /// inside the vertex shader itself, not here -- `vertices`' `position`
    /// stays raw RDP screen-pixel `x`/`y`/`z`/`w`, matching
    /// `triangle_pipeline_vertex.wgsl`'s own module doc.
    pub fn submit_admitted_triangle(
        &mut self,
        vertices: [NeutralTriangleVertex; 3],
        other_mode: OtherMode,
        combine_params: CombineParams,
        raster_params: TriangleRasterParams,
        extent: TriangleTargetExtent,
    ) -> Result<InFlightTriangleDraw<'_>, TrianglePipelineError> {
        let _ = other_mode;
        let fixture = TriangleFixture {
            vertices: vertices.map(neutral_vertex_to_raster_vertex),
            raster_params,
            combine_params,
            extent,
        };
        self.submit_triangle(fixture)
    }

    /// Submits one or more fixed-fixture triangle draws, in order, into ONE
    /// shared per-submission color+depth texture pair -- the color
    /// attachment clears once (`LoadOp::Clear`) before the first draw, then
    /// every subsequent draw in the same render pass uses `LoadOp::Load` on
    /// both attachments, so later draws' real GPU depth test competes
    /// against earlier draws' committed depth/color, not a fresh buffer.
    /// This is the shape port card §6's depth differential needs ("a second
    /// triangle drawn behind/in-front of the first") -- `submit_triangle`'s
    /// single-fixture call is a degenerate one-draw case of this method, not
    /// a separately-implemented path.
    ///
    /// # Panics
    /// If `fixtures` is empty (a caller bug, not a runtime/device condition
    /// -- there is no meaningful "submit nothing" draw).
    pub fn submit_triangles(
        &mut self,
        fixtures: &[TriangleFixture],
    ) -> Result<InFlightTriangleDraw<'_>, TrianglePipelineError> {
        assert!(
            !fixtures.is_empty(),
            "submit_triangles requires at least one fixture"
        );
        let error_count = self.errors.count();
        if error_count != 0 {
            return Err(TrianglePipelineError::DevicePoisoned {
                count: error_count,
                first: self.errors.first(),
            });
        }

        let extent = fixtures[0].extent;
        for fixture in fixtures {
            validate_triangle_extent(fixture.extent)?;
            if fixture.extent != extent {
                return Err(TrianglePipelineError::MixedExtent {
                    first: extent,
                    other: fixture.extent,
                });
            }
        }
        // wgpu's `copy_texture_to_buffer` requires each row to start at a
        // `COPY_BYTES_PER_ROW_ALIGNMENT`-aligned offset; this slice's 8x8/
        // 16x16 fixed targets (port card §3) have an unaligned natural row
        // stride (e.g. 8 pixels * 4 bytes = 32), so the readback buffers use
        // a padded stride and `complete()` strips the padding back out.
        let unpadded_bytes_per_row = extent.width * 4;
        let padded_bytes_per_row = unpadded_bytes_per_row
            .div_ceil(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT)
            * wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
        let color_bytes = u64::from(padded_bytes_per_row) * u64::from(extent.height);
        let depth_bytes = u64::from(padded_bytes_per_row) * u64::from(extent.height);

        // Each draw gets its own vertex buffer and its own raster/combine
        // uniform buffer + bind group: mid-render-pass `queue.write_buffer`
        // calls are not safe against a buffer already bound by an
        // in-flight pass, so per-draw uniforms must be distinct resources
        // written before the pass opens, not one shared buffer rewritten
        // between draws.
        struct DrawResources {
            vertex_buffer: wgpu::Buffer,
            bind_group: wgpu::BindGroup,
            // Kept alive only because the bind group above borrows them
            // (`as_entire_binding`); never read after construction.
            _raster_params_buffer: wgpu::Buffer,
            _combine_params_buffer: wgpu::Buffer,
        }
        let mut draws = Vec::with_capacity(fixtures.len());
        for fixture in fixtures {
            let mut vertex_bytes = Vec::with_capacity(3 * 40);
            for vertex in fixture.vertices {
                vertex_bytes.extend_from_slice(&vertex.to_bytes());
            }
            let vertex_buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("fn64-triangle-pipeline-vertices"),
                size: vertex_bytes.len() as u64,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            self.queue.write_buffer(&vertex_buffer, 0, &vertex_bytes);

            let raster_params_buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("fn64-triangle-pipeline-raster-params"),
                size: RASTER_PARAMS_BYTES,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            self.queue
                .write_buffer(&raster_params_buffer, 0, &fixture.raster_params.to_bytes());
            let combine_params_buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("fn64-triangle-pipeline-combine-params"),
                size: COMBINE_PARAMS_BYTES,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            let mut combine_bytes = [0u8; COMBINE_PARAMS_BYTES as usize];
            combine_bytes[0..4].copy_from_slice(&fixture.combine_params.low().to_le_bytes());
            combine_bytes[4..8].copy_from_slice(&fixture.combine_params.high().to_le_bytes());
            self.queue
                .write_buffer(&combine_params_buffer, 0, &combine_bytes);

            let bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("fn64-triangle-pipeline-bind-group"),
                layout: &self.bind_group_layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: raster_params_buffer.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: combine_params_buffer.as_entire_binding(),
                    },
                ],
            });
            draws.push(DrawResources {
                vertex_buffer,
                bind_group,
                _raster_params_buffer: raster_params_buffer,
                _combine_params_buffer: combine_params_buffer,
            });
        }

        let color_texture = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("fn64-triangle-pipeline-color"),
            size: wgpu::Extent3d {
                width: extent.width,
                height: extent.height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: COLOR_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let color_view = color_texture.create_view(&wgpu::TextureViewDescriptor::default());
        let depth_texture = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("fn64-triangle-pipeline-depth"),
            size: wgpu::Extent3d {
                width: extent.width,
                height: extent.height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: DEPTH_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let depth_view = depth_texture.create_view(&wgpu::TextureViewDescriptor::default());

        let color_readback = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("fn64-triangle-pipeline-color-readback"),
            size: color_bytes,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let depth_readback = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("fn64-triangle-pipeline-depth-readback"),
            size: depth_bytes,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });

        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("fn64-triangle-pipeline-submit"),
            });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("fn64-triangle-pipeline-pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &color_view,
                    resolve_target: None,
                    depth_slice: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &depth_view,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.0),
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_pipeline(&self.pipeline);
            for resources in &draws {
                pass.set_bind_group(0, &resources.bind_group, &[]);
                pass.set_vertex_buffer(0, resources.vertex_buffer.slice(..));
                pass.draw(0..3, 0..1);
            }
        }
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: &color_texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &color_readback,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(padded_bytes_per_row),
                    rows_per_image: Some(extent.height),
                },
            },
            wgpu::Extent3d {
                width: extent.width,
                height: extent.height,
                depth_or_array_layers: 1,
            },
        );
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: &depth_texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &depth_readback,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(padded_bytes_per_row),
                    rows_per_image: Some(extent.height),
                },
            },
            wgpu::Extent3d {
                width: extent.width,
                height: extent.height,
                depth_or_array_layers: 1,
            },
        );
        let (callback_sender, callback_receiver) = mpsc::sync_channel(1);
        encoder.on_submitted_work_done(move || {
            let _ = callback_sender.try_send(());
        });
        let submission = self.queue.submit([encoder.finish()]);

        Ok(InFlightTriangleDraw {
            renderer: self,
            extent,
            padded_bytes_per_row,
            color_readback,
            depth_readback,
            submission,
            callback_receiver,
        })
    }
}

pub struct InFlightTriangleDraw<'renderer> {
    renderer: &'renderer mut TrianglePipelineRenderer,
    extent: TriangleTargetExtent,
    padded_bytes_per_row: u32,
    color_readback: wgpu::Buffer,
    depth_readback: wgpu::Buffer,
    submission: wgpu::SubmissionIndex,
    callback_receiver: mpsc::Receiver<()>,
}

/// Readback: RGBA8 color bytes (`Rgba8Unorm`) and per-pixel depth as `f32`
/// (`Depth32Float`), row-major, `extent.width * extent.height` pixels each.
pub struct TriangleDrawOutput {
    pub extent: TriangleTargetExtent,
    pub color_rgba8: Vec<u8>,
    pub depth_f32: Vec<f32>,
}

impl InFlightTriangleDraw<'_> {
    pub fn complete(self) -> Result<TriangleDrawOutput, TrianglePipelineError> {
        self.renderer
            .device
            .poll(wgpu::PollType::Wait {
                submission_index: Some(self.submission),
                timeout: Some(POLL_TIMEOUT),
            })
            .map_err(|error| TrianglePipelineError::ExactSubmissionWait(error.to_string()))?;
        self.callback_receiver
            .recv_timeout(CALLBACK_TIMEOUT)
            .map_err(|_| TrianglePipelineError::CompletionCallbackNotObserved)?;

        let color_padded = map_and_read(&self.renderer.device, &self.color_readback)?;
        let depth_padded = map_and_read(&self.renderer.device, &self.depth_readback)?;

        let error_count = self.renderer.errors.count();
        if error_count != 0 {
            return Err(TrianglePipelineError::DevicePoisoned {
                count: error_count,
                first: self.renderer.errors.first(),
            });
        }

        let unpadded_bytes_per_row = self.extent.width as usize * 4;
        let color_rgba8 = strip_row_padding(
            &color_padded,
            self.padded_bytes_per_row as usize,
            unpadded_bytes_per_row,
            self.extent.height as usize,
        );
        let depth_bytes = strip_row_padding(
            &depth_padded,
            self.padded_bytes_per_row as usize,
            unpadded_bytes_per_row,
            self.extent.height as usize,
        );

        let mut depth_f32 = Vec::with_capacity(depth_bytes.len() / 4);
        for chunk in depth_bytes.chunks_exact(4) {
            depth_f32.push(f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]));
        }

        Ok(TriangleDrawOutput {
            extent: self.extent,
            color_rgba8,
            depth_f32,
        })
    }
}

fn strip_row_padding(
    padded: &[u8],
    padded_bytes_per_row: usize,
    unpadded_bytes_per_row: usize,
    height: usize,
) -> Vec<u8> {
    let mut output = Vec::with_capacity(unpadded_bytes_per_row * height);
    for row in 0..height {
        let start = row * padded_bytes_per_row;
        output.extend_from_slice(&padded[start..start + unpadded_bytes_per_row]);
    }
    output
}

fn map_and_read(
    device: &wgpu::Device,
    buffer: &wgpu::Buffer,
) -> Result<Vec<u8>, TrianglePipelineError> {
    let (sender, receiver) = mpsc::sync_channel(1);
    buffer
        .slice(..)
        .map_async(wgpu::MapMode::Read, move |result| {
            let _ = sender.try_send(result);
        });
    device
        .poll(wgpu::PollType::Wait {
            submission_index: None,
            timeout: Some(POLL_TIMEOUT),
        })
        .map_err(|error| TrianglePipelineError::Readback(error.to_string()))?;
    receiver
        .recv_timeout(CALLBACK_TIMEOUT)
        .map_err(|_| TrianglePipelineError::Readback("map callback timeout".into()))?
        .map_err(|error| TrianglePipelineError::Readback(error.to_string()))?;
    let mapped = buffer
        .slice(..)
        .get_mapped_range()
        .map_err(|error| TrianglePipelineError::Readback(error.to_string()))?;
    let output = mapped.to_vec();
    drop(mapped);
    buffer.unmap();
    Ok(output)
}

/// `OtherMode` this slice's fixed fixture always uses: wire `(0, 0)` decodes
/// to `CycleType::OneCycle` (see `state.rs`'s wire-decode table) with
/// `primitive_depth_source()` and `force_blend()` both false -- no Z-override
/// branch, no blend-force -- matching the restriction set (§3) without
/// invoking `blend.rs`'s runtime OtherMode dispatch for this slice (see
/// module doc's blend-no-op note).
pub const fn fixed_fixture_other_mode() -> OtherMode {
    OtherMode::from_wire(0, 0)
}

#[derive(Debug, PartialEq, Eq)]
pub enum TrianglePipelineError {
    RequestAdapter(String),
    RequestDevice(String),
    PipelinePrewarm(String),
    DevicePoisoned {
        count: usize,
        first: Option<String>,
    },
    ZeroExtent {
        width: u32,
        height: u32,
    },
    MixedExtent {
        first: TriangleTargetExtent,
        other: TriangleTargetExtent,
    },
    ExactSubmissionWait(String),
    CompletionCallbackNotObserved,
    Readback(String),
}

impl fmt::Display for TrianglePipelineError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::RequestAdapter(reason) => {
                write!(formatter, "wgpu adapter request failed: {reason}")
            }
            Self::RequestDevice(reason) => {
                write!(formatter, "wgpu device request failed: {reason}")
            }
            Self::PipelinePrewarm(reason) => {
                write!(formatter, "triangle-pipeline prewarm failed: {reason}")
            }
            Self::DevicePoisoned { count, first } => write!(
                formatter,
                "triangle-pipeline device recorded {count} uncaptured errors; first={first:?}"
            ),
            Self::ZeroExtent { width, height } => {
                write!(formatter, "triangle-pipeline target extent is empty: {width}x{height}")
            }
            Self::MixedExtent { first, other } => write!(
                formatter,
                "triangle-pipeline batch has mixed target extents: {}x{} vs {}x{}",
                first.width, first.height, other.width, other.height
            ),
            Self::ExactSubmissionWait(reason) => {
                write!(formatter, "exact triangle-pipeline submission wait failed: {reason}")
            }
            Self::CompletionCallbackNotObserved => formatter.write_str(
                "triangle-pipeline completion callback was not observable after exact submission wait",
            ),
            Self::Readback(reason) => write!(formatter, "triangle-pipeline readback failed: {reason}"),
        }
    }
}

impl std::error::Error for TrianglePipelineError {}

#[cfg(test)]
mod tests;
