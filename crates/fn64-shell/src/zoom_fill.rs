//! Cached presenter for the shell's original-aspect and zoom-to-fill policies.
//!
//! `pixels` derives its built-in fit from the uploaded texture extent. That is
//! a square-pixel assumption, but an N64 VI field's sample dimensions do not
//! define its display aspect: for example, a 639x237 field is still presented
//! as the original 4:3 picture. This presenter samples the unchanged uploaded
//! field into either a centered 4:3 viewport or the complete surface when the
//! player explicitly enables zoom-to-fill. GPU objects are retained across
//! frames; only a Pixels buffer resize, which replaces the sampled texture,
//! rebuilds the bind group.

use pixels::wgpu;

const SHADER: &str = r#"
struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
}

@vertex
fn vs_main(@builtin(vertex_index) index: u32) -> VertexOutput {
    // Naga 0.19 rejects runtime indexing into a function-local array. Keep
    // the three fullscreen-triangle vertices explicit so the shader is valid
    // on the wgpu version pinned by pixels 0.15.
    var position: vec2<f32>;
    switch index {
        case 0u: { position = vec2<f32>(-1.0, -1.0); }
        case 1u: { position = vec2<f32>( 3.0, -1.0); }
        default: { position = vec2<f32>(-1.0,  3.0); }
    }
    var output: VertexOutput;
    output.position = vec4<f32>(position, 0.0, 1.0);
    output.uv = position * vec2<f32>(0.5, -0.5) + vec2<f32>(0.5, 0.5);
    return output;
}

@group(0) @binding(0) var frame: texture_2d<f32>;
@group(0) @binding(1) var frame_sampler: sampler;

@fragment
fn fs_main(input: VertexOutput) -> @location(0) vec4<f32> {
    return textureSample(frame, frame_sampler, input.uv);
}
"#;

#[cfg(test)]
mod tests {
    use super::{presentation_viewport, PresentationViewport, SHADER};

    #[test]
    fn presentation_shader_validates_with_pixels_naga_line() {
        let module = naga::front::wgsl::parse_str(SHADER).expect("presentation WGSL parses");
        naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::all(),
        )
        .validate(&module)
        .expect("presentation WGSL validates under Naga 0.19");
    }

    #[test]
    fn original_policy_ignores_wide_vi_sample_geometry() {
        // WM2000's observed source field is 639x237 at this seam. Source
        // dimensions deliberately are not an input: they describe samples,
        // while this function owns host display geometry.
        assert_eq!(
            presentation_viewport((1280, 720), false),
            PresentationViewport {
                x: 160,
                y: 0,
                width: 960,
                height: 720,
            }
        );
    }

    #[test]
    fn original_policy_letterboxes_portrait_surfaces_at_four_by_three() {
        assert_eq!(
            presentation_viewport((600, 800), false),
            PresentationViewport {
                x: 0,
                y: 175,
                width: 600,
                height: 450,
            }
        );
    }

    #[test]
    fn zoom_fill_still_uses_the_complete_surface() {
        assert_eq!(
            presentation_viewport((1280, 720), true),
            PresentationViewport {
                x: 0,
                y: 0,
                width: 1280,
                height: 720,
            }
        );
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PresentationViewport {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

/// Derive host composition bounds without consulting the VI sample extent.
///
/// The default is the original N64 4:3 display aspect. Zoom-fill deliberately
/// occupies the complete surface and therefore retains its existing stretch
/// semantics. Inputs are clamped because winit can transiently report a zero
/// dimension while a window is minimized.
pub fn presentation_viewport(surface_size: (u32, u32), zoom_fill: bool) -> PresentationViewport {
    let (surface_width, surface_height) = (surface_size.0.max(1), surface_size.1.max(1));
    if zoom_fill {
        return PresentationViewport {
            x: 0,
            y: 0,
            width: surface_width,
            height: surface_height,
        };
    }

    let surface_is_at_least_four_by_three =
        u64::from(surface_width) * 3 >= u64::from(surface_height) * 4;
    let (width, height) = if surface_is_at_least_four_by_three {
        (((u64::from(surface_height) * 4) / 3) as u32, surface_height)
    } else {
        (surface_width, ((u64::from(surface_width) * 3) / 4) as u32)
    };
    let height = height.max(1);
    PresentationViewport {
        x: (surface_width - width) / 2,
        y: (surface_height - height) / 2,
        width,
        height,
    }
}

pub struct FramePresenter {
    pipeline: wgpu::RenderPipeline,
    bind_group_layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    bind_group: wgpu::BindGroup,
    texture_extent: wgpu::Extent3d,
}

impl FramePresenter {
    pub fn new(pixels: &pixels::Pixels<'static>) -> Self {
        let device = pixels.device();
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("fn64 frame presentation shader"),
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });
        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("fn64 frame presentation bind group layout"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("fn64 frame presentation nearest sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Nearest,
            min_filter: wgpu::FilterMode::Nearest,
            mipmap_filter: wgpu::FilterMode::Nearest,
            ..Default::default()
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("fn64 frame presentation pipeline layout"),
            bind_group_layouts: &[&bind_group_layout],
            push_constant_ranges: &[],
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("fn64 frame presentation pipeline"),
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: "vs_main",
                buffers: &[],
            },
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: "fs_main",
                targets: &[Some(wgpu::ColorTargetState {
                    format: pixels.render_texture_format(),
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview: None,
        });
        let texture_extent = pixels.texture().size();
        let bind_group =
            Self::make_bind_group(device, &bind_group_layout, &sampler, pixels.texture());
        Self {
            pipeline,
            bind_group_layout,
            sampler,
            bind_group,
            texture_extent,
        }
    }

    fn make_bind_group(
        device: &wgpu::Device,
        layout: &wgpu::BindGroupLayout,
        sampler: &wgpu::Sampler,
        texture: &wgpu::Texture,
    ) -> wgpu::BindGroup {
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("fn64 frame presentation bind group"),
            layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(sampler),
                },
            ],
        })
    }

    fn refresh_texture(&mut self, pixels: &pixels::Pixels<'static>) {
        let texture_extent = pixels.texture().size();
        if texture_extent != self.texture_extent {
            self.bind_group = Self::make_bind_group(
                pixels.device(),
                &self.bind_group_layout,
                &self.sampler,
                pixels.texture(),
            );
            self.texture_extent = texture_extent;
        }
    }

    pub fn encode(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
        target: &wgpu::TextureView,
        viewport: PresentationViewport,
    ) {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("fn64 frame presentation pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: target,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
        });
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &self.bind_group, &[]);
        pass.set_viewport(
            viewport.x as f32,
            viewport.y as f32,
            viewport.width as f32,
            viewport.height as f32,
            0.0,
            1.0,
        );
        pass.set_scissor_rect(viewport.x, viewport.y, viewport.width, viewport.height);
        pass.draw(0..3, 0..1);
    }

    pub fn render(
        &mut self,
        pixels: &pixels::Pixels<'static>,
        surface_size: (u32, u32),
        zoom_fill: bool,
    ) -> Result<(), pixels::Error> {
        self.refresh_texture(pixels);
        let viewport = presentation_viewport(surface_size, zoom_fill);
        pixels.render_with(|encoder, target, _context| {
            self.encode(encoder, target, viewport);
            Ok(())
        })
    }

    pub fn prepare(&mut self, pixels: &pixels::Pixels<'static>) {
        self.refresh_texture(pixels);
    }
}
