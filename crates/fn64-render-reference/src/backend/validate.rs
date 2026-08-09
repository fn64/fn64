use crate::{
    gbi, render_unsupported_error,
};
use fn64_render::RenderError;

use super::*;

pub(super) fn validate_reference_color_image(
    rdram: &[u8],
    height: u32,
    target: gbi::ColorImage,
) -> Result<(), RenderError> {
    let Some(layout) = target.layout() else {
        return Err(render_unsupported_error(
            "reference",
            "render.rdp.color-image-layout",
            format!(
                "G_SETCIMG format={} size={} is unsupported; reference execution requires 8-bit intensity, RGBA16, or RGBA32",
                target.format, target.size
            ),
        ));
    };
    let bytes_per_pixel = layout.bytes_per_pixel();
    if target.width == 0 {
        return Err(RenderError::Backend {
            backend: "reference",
            reason: "G_SETCIMG decoded a zero-width color image".to_string(),
        });
    }
    if !target.address.is_multiple_of(8) {
        return Err(RenderError::Backend {
            backend: "reference",
            reason: format!(
                "G_SETCIMG {} base {:#010x} is not 64-bit aligned",
                layout.name(),
                target.address,
            ),
        });
    }
    let byte_len = usize::from(target.width)
        .checked_mul(height as usize)
        .and_then(|pixels| pixels.checked_mul(bytes_per_pixel))
        .ok_or_else(|| RenderError::Backend {
            backend: "reference",
            reason: "G_SETCIMG dimensions overflow host address space".to_string(),
        })?;
    let end = (target.address as usize)
        .checked_add(byte_len)
        .ok_or_else(|| RenderError::Backend {
            backend: "reference",
            reason: "G_SETCIMG address range overflows host address space".to_string(),
        })?;
    if end > rdram.len() {
        return Err(RenderError::Backend {
            backend: "reference",
            reason: format!(
                "G_SETCIMG {} target [{:#010x}, {end:#010x}) exceeds RDRAM length {}",
                layout.name(),
                target.address,
                rdram.len()
            ),
        });
    }
    Ok(())
}

pub(super) fn require_reference_color_target(
    decode_mode: DecodeMode,
    target: Option<gbi::ColorImage>,
    operation: &str,
) -> Result<(), RenderError> {
    if decode_mode != DecodeMode::Simple && target.is_none() {
        return Err(render_unsupported_error(
            "reference",
            "render.rdp.color-target-state",
            format!(
                "{operation} has no persistent G_SETCIMG color target; VI/output_addr state is not an RDP color-image substitute"
            ),
        ));
    }
    Ok(())
}

pub(super) fn validate_texture_rectangle(
    rectangle: &gbi::TextureRectangle,
    target: Option<gbi::ColorImage>,
) -> Result<(), RenderError> {
    match rectangle.other_mode.cycle_type() {
        gbi::CycleType::Copy => validate_copy_texture_rectangle(rectangle, target),
        gbi::CycleType::OneCycle | gbi::CycleType::TwoCycle => {
            validate_combined_texture_rectangle(rectangle)
        }
        // NOT invalid. The N64brew RDP command table states, in the Texture
        // Rectangle section itself: "In FILL mode this behaves identically to
        // Fill Rectangle, the texturing properties are ignored." Sampling is
        // bypassed, but the rectangle is still rasterized -- from the fill
        // color register, which Fill Rectangle documents as the sole colour
        // input in that mode. Rejecting it aborted a WCW/nWo Revenge frame
        // over a combination the hardware defines.
        //
        // The fill-cycle blender hazard is a property of the CYCLE, not the
        // command, so it applies here exactly as it does to G_FILLRECT: a
        // retained depth consumer in fill cycle can hang the RDP. Checked
        // here rather than inside the shared rasterizer so the diagnostic
        // names the command the guest actually submitted.
        gbi::CycleType::Fill => {
            if let Err(hazards) = rectangle.other_mode.validate_fill_cycle_bypass() {
                return Err(render_unsupported_error(
                    "reference",
                    "render.rdp.fill-cycle-hazard-state",
                    format!(
                        "{} in Fill cycle retains unsafe {hazards} state; the public fill \
                         contract requires G_RM_NOOP/G_RM_NOOP2, and retaining Z/framebuffer \
                         consumers is outside that safe contract (a depth read can hang the RDP)",
                        texture_rectangle_name(rectangle)
                    ),
                ));
            }
            Ok(())
        }
    }
}

pub(super) fn texture_rectangle_name(rectangle: &gbi::TextureRectangle) -> &'static str {
    if rectangle.flip {
        "G_TEXRECTFLIP"
    } else {
        "G_TEXRECT"
    }
}

pub(super) fn validate_alpha_compare(mode: gbi::AlphaCompare, primitive: &str) -> Result<(), RenderError> {
    match mode {
        gbi::AlphaCompare::None | gbi::AlphaCompare::Threshold | gbi::AlphaCompare::Dither => {
            Ok(())
        }
        gbi::AlphaCompare::Reserved => Err(render_unsupported_error(
            "reference",
            "render.rdp.alpha-compare",
            format!("{primitive} uses reserved alpha-compare mode 2"),
        )),
    }
}

pub(super) fn validate_copy_texture_rectangle(
    rectangle: &gbi::TextureRectangle,
    target: Option<gbi::ColorImage>,
) -> Result<(), RenderError> {
    let reject = |reason: String| RenderError::Backend {
        backend: "reference",
        reason,
    };
    debug_assert_eq!(rectangle.other_mode.cycle_type(), gbi::CycleType::Copy);
    if rectangle.other_mode.depth_compare_enabled() || rectangle.other_mode.depth_update_enabled() {
        return Err(reject(format!(
            "{} enables depth in Copy cycle, which bypasses the blender",
            texture_rectangle_name(rectangle)
        )));
    }
    if rectangle.dsdx != 4 << 10 {
        return Err(reject(format!(
            "{} copy dsdx={} violates the public copy-mode 4<<10 step",
            texture_rectangle_name(rectangle),
            rectangle.dsdx
        )));
    }
    validate_alpha_compare(
        rectangle.other_mode.alpha_compare(),
        texture_rectangle_name(rectangle),
    )?;
    let texture = rectangle.texture.as_ref().ok_or_else(|| {
        reject(format!(
            "{} references tile {} without a decoded G_LOADBLOCK/G_LOADTILE image",
            texture_rectangle_name(rectangle),
            rectangle.tile
        ))
    })?;
    let rgba16 =
        texture.format == gbi::ColorImage::RGBA_FORMAT && texture.size == gbi::ColorImage::BITS_16;
    let direct_8bit = texture.size == gbi::ColorImage::BITS_8
        && match texture.format {
            gbi::ColorImage::I_FORMAT | gbi::ColorImage::IA_FORMAT => true,
            gbi::ColorImage::CI_FORMAT => rectangle.other_mode.texture_lut() == 0,
            _ => false,
        };
    if !rgba16 && !direct_8bit {
        return Err(render_unsupported_error(
            "reference",
            "render.rdp.copy-source-layout",
            format!(
                "{} copy source format={} size={} LUT={} is unsupported; expected RGBA16, I8, IA8, or non-dereferenced CI8",
                texture_rectangle_name(rectangle),
                texture.format,
                texture.size,
                rectangle.other_mode.texture_lut()
            ),
        ));
    }
    if let Some(target) = target {
        let matching_target = matches!(
            (rgba16, direct_8bit, target.layout()),
            (true, false, Some(gbi::ColorImageLayout::Rgba16))
                | (false, true, Some(gbi::ColorImageLayout::Index8))
        );
        if !matching_target {
            return Err(reject(format!(
                "{} copy source format={} size={} does not match color target format={} size={}",
                texture_rectangle_name(rectangle),
                texture.format,
                texture.size,
                target.format,
                target.size
            )));
        }
    }
    if let Some(scissor) = rectangle.scissor {
        let multiple_of_four = |edge: f32| edge.fract() == 0.0 && (edge as i32).rem_euclid(4) == 0;
        if ![scissor.ulx, scissor.uly, scissor.lrx, scissor.lry]
            .into_iter()
            .all(multiple_of_four)
        {
            return Err(reject(format!(
                "{} copy scissor ({}, {})..({}, {}) is not aligned to the documented four-pixel boundary",
                texture_rectangle_name(rectangle),
                scissor.ulx,
                scissor.uly,
                scissor.lrx,
                scissor.lry
            )));
        }
    }
    Ok(())
}

pub(super) fn validate_combined_texture_rectangle(
    rectangle: &gbi::TextureRectangle,
) -> Result<(), RenderError> {
    let reject = |reason: String| RenderError::Backend {
        backend: "reference",
        reason,
    };
    let name = texture_rectangle_name(rectangle);
    let mode = rectangle.other_mode;
    validate_alpha_compare(mode.alpha_compare(), name)?;
    if mode.texture_filter() == gbi::TextureFilter::Reserved {
        return Err(reject(format!(
            "{name} uses reserved texture-filter mode 1"
        )));
    }
    if (mode.depth_compare_enabled() || mode.depth_update_enabled())
        && !mode.primitive_depth_source()
    {
        return Err(reject(format!(
            "{name} requests depth compare/update with pixel Z, but rectangles require G_ZS_PRIM"
        )));
    }
    if !matches!(mode.texture_convert(), 0 | 5 | 6) {
        return Err(reject(format!(
            "{name} uses reserved texture-convert mode {}",
            mode.texture_convert()
        )));
    }
    if mode.texture_detail() == 3 {
        return Err(reject(format!(
            "{name} selects reserved texture-detail mode 3"
        )));
    }
    rectangle.texture.as_ref().ok_or_else(|| {
        reject(format!(
            "{name} references tile {} without a decoded G_LOADBLOCK/G_LOADTILE image",
            rectangle.tile
        ))
    })?;

    let cycle_count = match mode.cycle_type() {
        gbi::CycleType::OneCycle => 1,
        gbi::CycleType::TwoCycle => 2,
        _ => unreachable!("combined rectangle validator called for bypass cycle"),
    };
    for (cycle_index, cycle) in rectangle
        .combiner
        .mode
        .cycles
        .iter()
        .take(cycle_count)
        .enumerate()
    {
        for source in cycle.rgb {
            validate_rectangle_color_source(rectangle, cycle_index, source)?;
        }
        for source in cycle.alpha {
            validate_rectangle_alpha_source(rectangle, cycle_index, source)?;
        }
    }
    if rectangle
        .blender
        .cycles
        .iter()
        .take(usize::from(rectangle.blender.cycle_count))
        .any(|cycle| cycle.a == gbi::BlendAlphaInput::Shade)
    {
        return Err(reject(format!(
            "{name} blender selects SHADE alpha, but rectangle commands carry no shade attributes"
        )));
    }
    Ok(())
}

pub(super) fn validate_fill_rectangle(rectangle: &gbi::FillRectangle) -> Result<(), RenderError> {
    use gbi::{AlphaSource, ColorSource, CycleType};
    let reject = |reason: String| RenderError::Backend {
        backend: "reference",
        reason,
    };
    match rectangle.cycle_type {
        CycleType::Fill => {
            if let Err(hazards) = rectangle.other_mode.validate_fill_cycle_bypass() {
                return Err(render_unsupported_error(
                    "reference",
                    "render.rdp.fill-cycle-hazard-state",
                    format!(
                        "G_FILLRECT in Fill cycle retains unsafe {hazards} state; the public fill contract requires G_RM_NOOP/G_RM_NOOP2, and retaining Z/framebuffer consumers is outside that safe contract (a depth read can hang the RDP)"
                    ),
                ));
            }
            return Ok(());
        }
        CycleType::Copy => {
            return Err(render_unsupported_error(
                "reference",
                "render.rdp.fill-rectangle-cycle",
                "G_FILLRECT in copy cycle has no guaranteed public result; use G_TEXRECT",
            ));
        }
        CycleType::OneCycle | CycleType::TwoCycle => {}
    }
    validate_alpha_compare(rectangle.other_mode.alpha_compare(), "combined G_FILLRECT")?;
    if (rectangle.other_mode.depth_compare_enabled() || rectangle.other_mode.depth_update_enabled())
        && !rectangle.other_mode.primitive_depth_source()
    {
        return Err(reject(
            "combined G_FILLRECT requests depth compare/update with pixel Z, but rectangles require G_ZS_PRIM"
                .into(),
        ));
    }

    let cycle_count = match rectangle.cycle_type {
        CycleType::OneCycle => 1,
        CycleType::TwoCycle => 2,
        _ => unreachable!(),
    };
    for (cycle_index, cycle) in rectangle
        .combiner
        .mode
        .cycles
        .iter()
        .take(cycle_count)
        .enumerate()
    {
        for source in cycle.rgb {
            let reason = match source {
                ColorSource::Combined | ColorSource::CombinedAlpha if cycle_index == 0 => {
                    Some("selects COMBINED before a first-cycle result exists")
                }
                ColorSource::Texel0
                | ColorSource::Texel1
                | ColorSource::Texel0Alpha
                | ColorSource::Texel1Alpha
                | ColorSource::LodFraction => {
                    Some("selects texture state, but G_FILLRECT carries no texture coordinates")
                }
                ColorSource::Shade | ColorSource::ShadeAlpha => {
                    Some("selects SHADE, but G_FILLRECT carries no shade attributes")
                }
                _ => None,
            };
            if let Some(reason) = reason {
                return Err(reject(format!(
                    "combined G_FILLRECT combiner cycle {} {reason}",
                    cycle_index + 1
                )));
            }
        }
        for source in cycle.alpha {
            let reason = match source {
                AlphaSource::Combined if cycle_index == 0 => {
                    Some("selects COMBINED before a first-cycle result exists")
                }
                AlphaSource::Texel0 | AlphaSource::Texel1 | AlphaSource::LodFraction => {
                    Some("selects texture state, but G_FILLRECT carries no texture coordinates")
                }
                AlphaSource::Shade => {
                    Some("selects SHADE, but G_FILLRECT carries no shade attributes")
                }
                _ => None,
            };
            if let Some(reason) = reason {
                return Err(reject(format!(
                    "combined G_FILLRECT alpha combiner cycle {} {reason}",
                    cycle_index + 1
                )));
            }
        }
    }
    if rectangle
        .blender
        .cycles
        .iter()
        .take(usize::from(rectangle.blender.cycle_count))
        .any(|cycle| cycle.a == gbi::BlendAlphaInput::Shade)
    {
        return Err(reject(
            "combined G_FILLRECT blender selects SHADE alpha, but the command carries no shade attributes"
                .into(),
        ));
    }
    Ok(())
}

pub(super) fn validate_rectangle_color_source(
    rectangle: &gbi::TextureRectangle,
    cycle_index: usize,
    source: gbi::ColorSource,
) -> Result<(), RenderError> {
    use gbi::ColorSource;
    let name = texture_rectangle_name(rectangle);
    let unsupported = |reason: &str| {
        render_unsupported_error(
            "reference",
            "render.rdp.rectangle-color-source",
            format!("{name} combiner cycle {} {reason}", cycle_index + 1),
        )
    };
    match source {
        ColorSource::Combined | ColorSource::CombinedAlpha if cycle_index == 0 => Err(unsupported(
            "selects COMBINED before a first-cycle result exists",
        )),
        ColorSource::Texel1 | ColorSource::Texel1Alpha
            if rectangle.texture1.is_none() && !rectangle.other_mode.texture_lod() =>
        {
            Err(unsupported("selects TEXEL1 without a decoded tile+1 image"))
        }
        ColorSource::Shade | ColorSource::ShadeAlpha => Err(unsupported(
            "selects SHADE, but rectangle commands carry no shade attributes",
        )),
        _ => Ok(()),
    }
}

pub(super) fn validate_rectangle_alpha_source(
    rectangle: &gbi::TextureRectangle,
    cycle_index: usize,
    source: gbi::AlphaSource,
) -> Result<(), RenderError> {
    use gbi::AlphaSource;
    let name = texture_rectangle_name(rectangle);
    let unsupported = |reason: &str| {
        render_unsupported_error(
            "reference",
            "render.rdp.rectangle-alpha-source",
            format!("{name} alpha combiner cycle {} {reason}", cycle_index + 1),
        )
    };
    match source {
        AlphaSource::Combined if cycle_index == 0 => Err(unsupported(
            "selects COMBINED before a first-cycle result exists",
        )),
        AlphaSource::Texel1
            if rectangle.texture1.is_none() && !rectangle.other_mode.texture_lod() =>
        {
            Err(unsupported("selects TEXEL1 without a decoded tile+1 image"))
        }
        AlphaSource::Shade => Err(unsupported(
            "selects SHADE, but rectangle commands carry no shade attributes",
        )),
        _ => Ok(()),
    }
}
