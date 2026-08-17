// The split module trees feed names through use-super glob chains; rustc
// accepts these imports at check time yet its fix pass calls them unused,
// and removing them breaks the build (pattern-bound constants, glob-fed
// children). Suppressed until the trees are normalized to single-source
// imports; see the file-split PR notes.
#![allow(unused_imports)]

use crate::gbi::{CullMode, CycleType, RdpDecodeState, RenderOp, TextureFilter, Triangle, Vertex};
use fn64_render::RenderError;
#[cfg(test)]
use fn64_render::UcodeId;

use super::*;
use super::object_mode::*;
use super::common::*;
use super::background::*;

pub(super) fn read_object_matrix_command(
    rdram: &[u8],
    w0: u32,
    address: u32,
    previous: Option<ObjectMatrix>,
    command_pc: usize,
) -> Result<(ObjectMatrix, bool), RenderError> {
    let parameter = ((w0 >> 16) & 0xff) as u8;
    let length = (w0 & 0xffff) as u16;
    match (parameter, length) {
        (0, 23) => {
            let address = require_object_range(rdram, address, 24, "G_OBJ_MOVEMEM ObjMatrix")?;
            let view = fn64_runtime::RdramView::from_storage(rdram);
            let base = fn64_runtime::RdramAddr::from_offset(address as u32);
            let word = |offset| {
                view.read_u32(base.checked_add(offset).expect("uObjMtx offset fits")) as i32
            };
            let half =
                |offset| view.read_u16(base.checked_add(offset).expect("uObjMtx offset fits"));
            Ok((
                ObjectMatrix {
                    a: word(0),
                    b: word(4),
                    c: word(8),
                    d: word(12),
                    x: half(16) as i16,
                    y: half(18) as i16,
                    base_scale_x: half(20),
                    base_scale_y: half(22),
                },
                true,
            ))
        }
        (2, 7) => {
            let address = require_object_range(rdram, address, 8, "G_OBJ_MOVEMEM ObjSubMatrix")?;
            let view = fn64_runtime::RdramView::from_storage(rdram);
            let base = fn64_runtime::RdramAddr::from_offset(address as u32);
            let half =
                |offset| view.read_u16(base.checked_add(offset).expect("uObjSubMtx offset fits"));
            Ok((
                ObjectMatrix {
                    x: half(0) as i16,
                    y: half(2) as i16,
                    base_scale_x: half(4),
                    base_scale_y: half(6),
                    ..previous.unwrap_or_default()
                },
                false,
            ))
        }
        _ => Err(reject(format!(
            "G_OBJ_MOVEMEM at RDRAM {command_pc:#010x} has parameter={parameter} length={length}; public S2DEX admits ObjMatrix (0,23) or ObjSubMatrix (2,7)"
        ))),
    }
}

pub(super) fn require_rotation_matrix(
    matrix: Option<ObjectMatrix>,
    rotation_loaded: bool,
    command: &str,
    command_pc: usize,
    compound: bool,
) -> Result<ObjectMatrix, RenderError> {
    if !rotation_loaded {
        let suffix = if compound {
            "; texture load was not applied"
        } else {
            ""
        };
        return Err(reject(format!(
            "{command} at RDRAM {command_pc:#010x} requires a preceding full G_OBJ_MOVEMEM ObjMatrix for A/B/C/D{suffix}"
        )));
    }
    matrix.ok_or_else(|| reject(format!("{command} rotation matrix state is missing")))
}

pub(super) fn read_object_render_mode(mode: u32, command_pc: usize) -> Result<ObjectRenderMode, RenderError> {
    if mode & !G_OBJRM_ALL != 0 {
        return Err(reject(format!(
            "G_OBJ_RENDERMODE at RDRAM {command_pc:#010x} has unknown flags {:#010x}",
            mode & !G_OBJRM_ALL
        )));
    }
    if mode & G_OBJRM_SHRINKSIZE_1 != 0 && mode & G_OBJRM_SHRINKSIZE_2 != 0 {
        return Err(reject(format!(
            "G_OBJ_RENDERMODE at RDRAM {command_pc:#010x} combines mutually exclusive G_OBJRM_SHRINKSIZE_1 and G_OBJRM_SHRINKSIZE_2"
        )));
    }
    Ok(ObjectRenderMode {
        texture_clamp: if mode & G_OBJRM_NOTXCLAMP != 0 {
            ObjectTextureClamp::Disabled
        } else {
            ObjectTextureClamp::Perimeter
        },
        filter_correction: if mode & G_OBJRM_BILERP != 0 {
            ObjectFilterCorrection::Bilinear
        } else {
            ObjectFilterCorrection::PointOrAverage
        },
        perimeter: ObjectPerimeter {
            shrink_half_texels: if mode & G_OBJRM_SHRINKSIZE_2 != 0 {
                2
            } else if mode & G_OBJRM_SHRINKSIZE_1 != 0 {
                1
            } else {
                0
            },
            widen_three_eighths_texel: mode & G_OBJRM_WIDEN != 0,
        },
        ignored_edge_flags: IgnoredObjectEdgeFlags {
            xlu: mode & G_OBJRM_XLU != 0,
            antialias: mode & G_OBJRM_ANTIALIAS != 0,
        },
    })
}

pub(super) fn object_rectangle_op(
    rdp: &mut RdpDecodeState,
    mut sprite: ObjectSprite,
    object_mode: ObjectRenderMode,
    command: &str,
) -> Result<RenderOp, RenderError> {
    if sprite.image_flags & !(G_OBJ_FLAG_FLIPS | G_OBJ_FLAG_FLIPT) != 0 {
        return Err(reject(format!(
            "{command} imageFlags={:#04x} contains bits outside G_OBJ_FLAG_FLIPS|G_OBJ_FLAG_FLIPT",
            sprite.image_flags
        )));
    }
    let image_flags = sprite.image_flags;
    sprite.image_flags = 0;
    let average_shrink =
        ObjectAverageShrinkFootprint::from_mode(object_mode, rdp.texture_filter(), command)?;
    let unclamped_point =
        ObjectUnclampedPointFootprint::from_mode(object_mode, rdp.texture_filter(), command)?;
    let unclamped_average =
        ObjectUnclampedAverageFootprint::from_mode(object_mode, rdp.texture_filter(), command)?;
    let mut filter_validation_mode = average_shrink
        .map(|footprint| footprint.filter_validation_mode(object_mode))
        .unwrap_or(object_mode);
    if unclamped_average.is_some() {
        // Lowering retains a clamped tile, but the typed proof below makes
        // that state unobservable for every addressed Average neighbour.
        filter_validation_mode.texture_clamp = ObjectTextureClamp::Perimeter;
    }
    let mut operation = rdp.object_rectangle_with_mode(sprite, filter_validation_mode)?;
    let RenderOp::TextureRectangle(rectangle) = &mut operation else {
        unreachable!("object rectangle lowering has one typed result")
    };
    // The current public gs2dex.h marks both legacy edge flags Ignored. Keep
    // them typed so an older revision can never acquire guessed behavior.
    let _ignored_by_current_public_header = object_mode.ignored_edge_flags;
    debug_assert_eq!(
        unclamped_point.is_some() || unclamped_average.is_some(),
        object_mode.texture_clamp == ObjectTextureClamp::Disabled
    );
    if object_mode.widens() && rectangle.other_mode.texture_filter() != TextureFilter::Point {
        return Err(unsupported(
            "render.s2dex.widen-filter-footprint",
            format!(
                "{command} G_OBJRM_WIDEN with filtered sampling requires unpublished perimeter filter arithmetic"
            ),
        ));
    }
    let source_width = f32::from(sprite.image_w / 32);
    let source_height = f32::from(sprite.image_h / 32);
    let shrink_half_texels = object_mode.shrink_half_texels();
    let shrink = f32::from(shrink_half_texels) * 0.5;
    if (shrink_half_texels != 0 || object_mode.widens())
        && rectangle.other_mode.cycle_type() == CycleType::Copy
    {
        return Err(unsupported(
            "render.s2dex.copy-perimeter",
            format!(
                "{command} Copy cycle does not support G_OBJRM_SHRINKSIZE/G_OBJRM_WIDEN subpixel perimeter processing"
            ),
        ));
    }
    if shrink * 2.0 >= source_width || shrink * 2.0 >= source_height {
        return Err(reject(format!(
            "{command} shrink perimeter {shrink} texels leaves no positive image area in {source_width}x{source_height}"
        )));
    }
    if object_mode.widens() && image_flags != 0 {
        return Err(unsupported(
            "render.s2dex.widen-flip-edge",
            format!(
                "{command} G_OBJRM_WIDEN with flipped S/T requires unpublished positive-edge selection"
            ),
        ));
    }
    let (shrink_x, widen_x) =
        object_mode
            .perimeter
            .exact_screen_adjustments(sprite.scale_w, "X", command)?;
    let (shrink_y, widen_y) =
        object_mode
            .perimeter
            .exact_screen_adjustments(sprite.scale_h, "Y", command)?;
    rectangle.lrx += widen_x - shrink_x;
    rectangle.lry += widen_y - shrink_y;
    if image_flags & G_OBJ_FLAG_FLIPS != 0 {
        rectangle.s = average_shrink.map_or(source_width - 1.0 - shrink, |footprint| {
            footprint.rectangle_start(sprite.image_w, true)
        });
        rectangle.dsdx = -rectangle.dsdx;
    } else {
        rectangle.s = average_shrink
            .map(|footprint| footprint.rectangle_start(sprite.image_w, false))
            .unwrap_or(shrink);
    }
    if image_flags & G_OBJ_FLAG_FLIPT != 0 {
        rectangle.t = average_shrink.map_or(source_height - 1.0 - shrink, |footprint| {
            footprint.rectangle_start(sprite.image_h, true)
        });
        rectangle.dtdy = -rectangle.dtdy;
    } else {
        rectangle.t = average_shrink
            .map(|footprint| footprint.rectangle_start(sprite.image_h, false))
            .unwrap_or(shrink);
    }
    if let Some(footprint) = average_shrink {
        let _average_axis_footprints = (
            footprint.validate_axis(
                rectangle.s,
                f32::from(rectangle.dsdx) / 1024.0,
                rectangle.ulx,
                rectangle.lrx,
                sprite.image_w / 32,
                "S",
                command,
            )?,
            footprint.validate_axis(
                rectangle.t,
                f32::from(rectangle.dtdy) / 1024.0,
                rectangle.uly,
                rectangle.lry,
                sprite.image_h / 32,
                "T",
                command,
            )?,
        );
    }
    if let Some(footprint) = unclamped_point {
        // Copy has its own inclusive raster command and cannot combine with
        // subpixel perimeter processing. Preserve the already-admitted
        // no-perimeter Copy case; this proof owns one/two-cycle point samples.
        if rectangle.other_mode.cycle_type() != CycleType::Copy {
            let _unclamped_axis_footprints = (
                footprint.validate_axis(
                    rectangle.s,
                    f32::from(rectangle.dsdx) / 1024.0,
                    rectangle.ulx,
                    rectangle.lrx,
                    sprite.image_w / 32,
                    "S",
                    command,
                )?,
                footprint.validate_axis(
                    rectangle.t,
                    f32::from(rectangle.dtdy) / 1024.0,
                    rectangle.uly,
                    rectangle.lry,
                    sprite.image_h / 32,
                    "T",
                    command,
                )?,
            );
        }
    }
    if let Some(footprint) = unclamped_average {
        let _unclamped_axis_footprints = (
            footprint.validate_axis(
                rectangle.s,
                f32::from(rectangle.dsdx) / 1024.0,
                rectangle.ulx,
                rectangle.lrx,
                sprite.image_w / 32,
                "S",
                command,
            )?,
            footprint.validate_axis(
                rectangle.t,
                f32::from(rectangle.dtdy) / 1024.0,
                rectangle.uly,
                rectangle.lry,
                sprite.image_h / 32,
                "T",
                command,
            )?,
        );
    }
    Ok(operation)
}

pub(super) fn object_sprite_ops(
    rdp: &mut RdpDecodeState,
    sprite: ObjectSprite,
    matrix: ObjectMatrix,
    object_mode: ObjectRenderMode,
    command: &str,
) -> Result<[RenderOp; 2], RenderError> {
    if rdp.texture_filter() == TextureFilter::Average && object_mode.shrink_half_texels() != 0 {
        return Err(unsupported(
            "render.s2dex.sprite-precision",
            format!(
                "{command} Average plus G_OBJRM_SHRINKSIZE on a rotating polygon requires a separately evidenced pixel-center coordinate correction"
            ),
        ));
    }
    let RenderOp::TextureRectangle(snapshot) =
        object_rectangle_op(rdp, sprite, object_mode, command)?
    else {
        unreachable!("object rectangle lowering has one typed result")
    };
    let cycle_type = snapshot.other_mode.cycle_type();
    if object_mode.texture_clamp == ObjectTextureClamp::Disabled {
        return Err(unsupported(
            "render.s2dex.sprite-tmem-addressing",
            format!(
                "{command} G_OBJRM_NOTXCLAMP on a polygon requires unpublished out-of-domain TMEM addressing"
            ),
        ));
    }
    if !matches!(cycle_type, CycleType::OneCycle | CycleType::TwoCycle) {
        return Err(unsupported(
            "render.s2dex.sprite-cycle",
            format!(
                "{command} polygon lowering supports one-cycle or two-cycle mode, got {cycle_type:?}"
            ),
        ));
    }
    if snapshot.other_mode.depth_compare_enabled()
        || snapshot.other_mode.depth_update_enabled()
        || snapshot.other_mode.primitive_depth_source()
    {
        return Err(unsupported(
            "render.s2dex.sprite-depth",
            format!("{command} depth state requires an evidenced S2DEX sprite Z policy"),
        ));
    }
    let texture0 = snapshot.texture.ok_or_else(|| {
        reject(format!(
            "{command} requires loaded TMEM for its documented textured quad"
        ))
    })?;
    let requires_texel1 = snapshot.combiner.mode.uses_texel1(cycle_type);
    if requires_texel1 && snapshot.texture1.is_none() {
        return Err(unsupported(
            "render.s2dex.sprite-combiner",
            format!("{command} combiner selects TEXEL1 without an initialized tile 1 image"),
        ));
    }
    // Section 4.2.5 defines the rotating object as two ordinary textured
    // polygons and assigns it the same texture settings as G_OBJ_RECTANGLE.
    // Preserve both no-LOD tiles in the shared immutable triangle snapshot:
    // the public RDP combiner defines TEXEL1 as the tile after TEXEL0.
    let mut tiles = std::array::from_fn(|_| None);
    tiles[0] = Some(texture0.clone());
    tiles[1] = snapshot.texture1;
    let texture = texture0.with_lod_snapshot(tiles, 0, 0);

    let exact_extent = |image: u16, scale: u16, axis: &str| -> Result<i64, RenderError> {
        let numerator = i64::from(image) * 128;
        if numerator % i64::from(scale) != 0 {
            return Err(unsupported(
                "render.s2dex.sprite-precision",
                format!(
                    "{command} {axis} extent requires unimplemented sub-quarter-pixel division: image={image} scale={scale}"
                ),
            ));
        }
        Ok(numerator / i64::from(scale))
    };
    let width = exact_extent(
        object_mode
            .perimeter
            .corrected_image_5(sprite.image_w, "width", command)?,
        sprite.scale_w,
        "X",
    )?;
    let height = exact_extent(
        object_mode
            .perimeter
            .corrected_image_5(sprite.image_h, "height", command)?,
        sprite.scale_h,
        "Y",
    )?;
    let x0 = i64::from(sprite.obj_x);
    let y0 = i64::from(sprite.obj_y);
    let x1 = x0 + width;
    let y1 = y0 + height;
    let transform = |x: i64, y: i64, axis: &str| -> Result<i16, RenderError> {
        let (first, second, origin) = if axis == "X" {
            (matrix.a, matrix.b, matrix.x)
        } else {
            (matrix.c, matrix.d, matrix.y)
        };
        let numerator = i64::from(first) * x + i64::from(second) * y;
        if numerator % (1 << 16) != 0 {
            return Err(unsupported(
                "render.s2dex.sprite-precision",
                format!(
                    "{command} transformed {axis} requires unimplemented sub-quarter-pixel matrix rounding: numerator={numerator}"
                ),
            ));
        }
        i16::try_from(i64::from(origin) + numerator / (1 << 16)).map_err(|_| {
            reject(format!(
                "{command} transformed {axis} coordinate exceeds s10.2"
            ))
        })
    };
    let corner = |x: i64, y: i64, s: f32, t: f32| -> Result<Vertex, RenderError> {
        Ok(Vertex {
            x: f32::from(transform(x, y, "X")?) / 4.0,
            y: f32::from(transform(x, y, "Y")?) / 4.0,
            r: 255,
            g: 255,
            b: 255,
            a: 255,
            s,
            t,
            w: 1.0,
            ..Vertex::default()
        })
    };
    // Rectangle rasterization evaluates S/T at its integer upper-left, while
    // triangle interpolation evaluates attributes at pixel centers. Apply the
    // public bilerp half-texel correction in the latter coordinate domain so
    // both object primitives address the same texel centers.
    let filter_correction = if object_mode.bilerp() { -0.5 } else { 0.0 };
    let (source_s_start, source_s_end) = object_mode.perimeter.source_bounds(sprite.image_w);
    let (source_t_start, source_t_end) = object_mode.perimeter.source_bounds(sprite.image_h);
    let (left_s, right_s) = if sprite.image_flags & G_OBJ_FLAG_FLIPS != 0 {
        (
            source_s_end + filter_correction,
            source_s_start + filter_correction,
        )
    } else {
        (
            source_s_start + filter_correction,
            source_s_end + filter_correction,
        )
    };
    let (top_t, bottom_t) = if sprite.image_flags & G_OBJ_FLAG_FLIPT != 0 {
        (
            source_t_end + filter_correction,
            source_t_start + filter_correction,
        )
    } else {
        (
            source_t_start + filter_correction,
            source_t_end + filter_correction,
        )
    };
    let corners = [
        corner(x0, y0, left_s, top_t)?,
        corner(x1, y0, right_s, top_t)?,
        corner(x1, y1, right_s, bottom_t)?,
        corner(x0, y1, left_s, bottom_t)?,
    ];
    let triangle = |indices: [usize; 3]| {
        RenderOp::Triangle(Triangle {
            v: [
                corners[indices[0]],
                corners[indices[1]],
                corners[indices[2]],
            ],
            scissor: snapshot.scissor,
            cull: CullMode::None,
            texture: Some(texture.clone()),
            other_mode: snapshot.other_mode,
            combiner: snapshot.combiner,
            blender: snapshot.blender,
        })
    };
    Ok([triangle([0, 1, 2]), triangle([0, 2, 3])])
}

pub(super) fn matrix_relative_sprite(
    mut sprite: ObjectSprite,
    matrix: ObjectMatrix,
) -> Result<ObjectSprite, RenderError> {
    // RectangleR deliberately ignores rotation, but retaining these fields
    // makes a SubMatrix update preserve the complete public matrix for the
    // later rotating-sprite slice.
    let _rotation_terms_reserved_for_sprite = (matrix.a, matrix.b, matrix.c, matrix.d);
    let position = |object: i16, origin: i16, base_scale: u16, axis: &str| {
        if base_scale == 0 {
            return Err(reject(format!(
                "G_OBJ_RECTANGLE_R BaseScale{axis} must be nonzero"
            )));
        }
        let numerator = i64::from(object) * (1 << 10);
        if numerator % i64::from(base_scale) != 0 {
            return Err(unsupported(
                "render.s2dex.rectangle-r-precision",
                format!(
                    "G_OBJ_RECTANGLE_R {axis} position requires unimplemented sub-fixed-point division: object={object} BaseScale{axis}={base_scale}"
                ),
            ));
        }
        i16::try_from(i64::from(origin) + numerator / i64::from(base_scale)).map_err(|_| {
            reject(format!(
                "G_OBJ_RECTANGLE_R transformed {axis} position exceeds s10.2"
            ))
        })
    };
    let scale = |object_scale: u16, base_scale: u16, axis: &str| {
        let product = u32::from(object_scale) * u32::from(base_scale);
        if product % (1 << 10) != 0 {
            return Err(unsupported(
                "render.s2dex.rectangle-r-precision",
                format!(
                    "G_OBJ_RECTANGLE_R {axis} scale requires unimplemented sub-fixed-point multiplication: scale={object_scale} BaseScale{axis}={base_scale}"
                ),
            ));
        }
        u16::try_from(product >> 10).map_err(|_| {
            reject(format!(
                "G_OBJ_RECTANGLE_R transformed {axis} scale exceeds u5.10"
            ))
        })
    };
    sprite.obj_x = position(sprite.obj_x, matrix.x, matrix.base_scale_x, "X")?;
    sprite.obj_y = position(sprite.obj_y, matrix.y, matrix.base_scale_y, "Y")?;
    sprite.scale_w = scale(sprite.scale_w, matrix.base_scale_x, "X")?;
    sprite.scale_h = scale(sprite.scale_h, matrix.base_scale_y, "Y")?;
    Ok(sprite)
}

pub(super) fn require_dma_length(
    w0: u32,
    expected: u32,
    command: &str,
    command_pc: usize,
) -> Result<(), RenderError> {
    let length = w0 & 0x00ff_ffff;
    if length != expected {
        return Err(reject(format!(
            "{command} at {command_pc:#010x} has DMA length {length}, expected {expected} from public gs2dex.h"
        )));
    }
    Ok(())
}

pub(super) fn require_object_range(
    rdram: &[u8],
    address: u32,
    bytes: usize,
    command: &str,
) -> Result<usize, RenderError> {
    let address_class = address >> 24;
    if !matches!(address_class, 0x00 | 0x80 | 0xa0) {
        return Err(reject(format!(
            "{command} object address {address:#010x} was not resolved to the physical 24-bit domain"
        )));
    }
    let address = (address & 0x00ff_ffff) as usize;
    let end = address
        .checked_add(bytes)
        .ok_or_else(|| reject(format!("{command} object address overflow")))?;
    if !address.is_multiple_of(8) {
        return Err(reject(format!(
            "{command} object address {address:#010x} is not 8-byte aligned"
        )));
    }
    if end > PHYSICAL_RDRAM_BYTES {
        return Err(reject(format!(
            "{command} object range [{address:#010x}, {end:#010x}) exceeds physical 8 MiB RDRAM"
        )));
    }
    if end > rdram.len() {
        return Err(reject(format!(
            "{command} object range [{address:#010x}, {end:#010x}) exceeds RDRAM length {}",
            rdram.len()
        )));
    }
    Ok(address)
}

pub(super) fn read_background(
    rdram: &[u8],
    address: u32,
    segments: &[u32; 16],
    background_command: S2dexCommand,
) -> Result<Background, RenderError> {
    let command = background_command.name();
    let address = require_object_range(rdram, address, OBJ_BG_BYTES, command)?;
    let view = fn64_runtime::RdramView::from_storage(rdram);
    let base = fn64_runtime::RdramAddr::from_offset(address as u32);
    let word = |offset| view.read_u32(base.checked_add(offset).expect("uObjBg offset fits"));
    let half = |offset| view.read_u16(base.checked_add(offset).expect("uObjBg offset fits"));
    let byte = |offset| view.read_u8(base.checked_add(offset).expect("uObjBg offset fits"));
    let common = BackgroundCommon {
        image_x: half(0),
        image_w: half(2),
        frame_x: half(4) as i16,
        frame_w: half(6),
        image_y: half(8),
        image_h: half(10),
        frame_y: half(12) as i16,
        frame_h: half(14),
        image: resolve_s2dex_pointer(segments, word(16), command, "background image")?,
        image_load: half(20),
        image_format: byte(22),
        image_size: byte(23),
        image_palette: half(24),
        image_flip: half(26),
    };
    if background_command == S2dexCommand::BgCopy {
        Ok(Background::Copy {
            common,
            tmem_w: half(28),
            tmem_h: half(30),
            tmem_load_sh: half(32),
            tmem_load_th: half(34),
            tmem_size_w: half(36),
            tmem_size: half(38),
        })
    } else {
        if (36..40).any(|offset| byte(offset) != 0) {
            return Err(reject(format!(
                "{command} uObjScaleBg padding[4] must be zero"
            )));
        }
        Ok(Background::Scale {
            common,
            scale_w: half(28),
            scale_h: half(30),
            image_y_origin: word(32) as i32,
        })
    }
}

pub(super) fn background_ops(
    rdram: &[u8],
    background: Background,
    rdp: &mut RdpDecodeState,
    command: &str,
) -> Result<Vec<RenderOp>, RenderError> {
    let (common, scale_w, scale_h, copy_tmem_rows, image_y_origin) = match background {
        Background::Copy {
            common,
            tmem_w,
            tmem_h,
            tmem_load_sh,
            tmem_load_th,
            tmem_size_w,
            tmem_size,
        } => {
            validate_copy_background_init(
                common,
                [
                    tmem_w,
                    tmem_h,
                    tmem_load_sh,
                    tmem_load_th,
                    tmem_size_w,
                    tmem_size,
                ],
                command,
            )?;
            (common, 1 << 10, 1 << 10, Some(tmem_h / 4), None)
        }
        Background::Scale {
            common,
            scale_w,
            scale_h,
            image_y_origin,
        } => (common, scale_w, scale_h, None, Some(image_y_origin)),
    };
    if !matches!(common.image_load, G_BGLT_LOADBLOCK | G_BGLT_LOADTILE) {
        return Err(reject(format!(
            "{command} imageLoad={:#06x} is not G_BGLT_LOADBLOCK or G_BGLT_LOADTILE",
            common.image_load
        )));
    }
    if common.image_format > 4 || common.image_size > 3 {
        return Err(reject(format!(
            "{command} image format={} size={} is outside public G_IM_FMT/G_IM_SIZ encodings",
            common.image_format, common.image_size
        )));
    }
    if common.image_palette > 7 {
        return Err(reject(format!(
            "{command} imagePal={} is outside the public S2DEX range 0..=7",
            common.image_palette
        )));
    }
    if !matches!(common.image_flip, 0 | G_BG_FLAG_FLIPS) {
        return Err(unsupported(
            "render.s2dex.background-flags",
            format!(
                "{command} imageFlip={:#06x} requests unsupported vertical/reserved flags",
                common.image_flip
            ),
        ));
    }
    if common.image_w == 0
        || common.image_h == 0
        || common.frame_w == 0
        || common.frame_h == 0
        || common.image_w > 0x0fff
        || common.image_h > 0x0fff
        || common.frame_w > 0x0fff
        || common.frame_h > 0x0fff
        || !common.image_w.is_multiple_of(4)
        || !common.image_h.is_multiple_of(4)
        || !common.frame_w.is_multiple_of(4)
        || !common.frame_h.is_multiple_of(4)
    {
        return Err(reject(format!(
            "{command} requires positive whole-pixel u10.2 image/frame dimensions within 0x0fff"
        )));
    }
    if common.frame_y & 3 != 0 {
        return Err(unsupported(
            "render.s2dex.background-subpixel",
            format!(
                "{command} frameY={} requests unsupported vertical subpixel movement",
                common.frame_y
            ),
        ));
    }
    if command == "G_BG_COPY"
        && (common.image_x & 31 != 0 || common.image_y & 31 != 0 || common.frame_x & 3 != 0)
    {
        return Err(reject(format!(
            "{command} requires integer image/frame origins"
        )));
    }
    if scale_w == 0 || scale_h == 0 || scale_w > i16::MAX as u16 || scale_h > i16::MAX as u16 {
        return Err(reject(format!(
            "{command} scaleW={scale_w} scaleH={scale_h} is outside the nonzero RDP S5.10 gradient range"
        )));
    }

    let image_width = u32::from(common.image_w / 4);
    let image_height = u32::from(common.image_h / 4);
    let frame_width = u32::from(common.frame_w / 4);
    let frame_height = u32::from(common.frame_h / 4);
    let bits_per_texel = 4u32 << common.image_size;
    let row_bits = image_width
        .checked_mul(bits_per_texel)
        .ok_or_else(|| reject(format!("{command} image row size overflow")))?;
    if row_bits % 64 != 0 {
        return Err(reject(format!(
            "{command} imageW={} pixels does not satisfy the public 8-byte row alignment for size {}",
            image_width, common.image_size
        )));
    }
    let row_bytes = row_bits / 8;
    let image_bytes = row_bytes
        .checked_mul(image_height)
        .ok_or_else(|| reject(format!("{command} image byte size overflow")))?;
    let image = physical_pointer(common.image, command, "background image")?;
    if command == "G_BG_1CYC" && image < 0x1000 {
        return Err(reject(format!(
            "{command} background image {image:#010x} violates the public >=0x1000 physical-address restriction"
        )));
    }
    let image_end = image
        .checked_add(image_bytes)
        .ok_or_else(|| reject(format!("{command} image range overflow")))?;
    if image_end as usize > PHYSICAL_RDRAM_BYTES || image_end as usize > rdram.len() {
        return Err(reject(format!(
            "{command} image range [{image:#010x}, {image_end:#010x}) exceeds physical/backed RDRAM"
        )));
    }

    if command == "G_BG_COPY" {
        let copy_tmem_rows =
            copy_tmem_rows.expect("validated G_BG_COPY carries initialized TMEM row capacity");
        let window = BackgroundCopyWindow::new(
            image_width,
            image_height,
            frame_width,
            frame_height,
            u32::from(common.image_x / 32),
            u32::from(common.image_y / 32),
            common.image_flip == G_BG_FLAG_FLIPS,
            u32::from(copy_tmem_rows),
            command,
        )?;
        return copy_background_ops(rdram, common, image, rdp, window, command);
    }
    let window = ScaledBackgroundWindow::new(
        image_width,
        image_height,
        frame_width,
        frame_height,
        common.image_x,
        common.image_y,
        scale_w,
        scale_h,
        common.image_flip == G_BG_FLAG_FLIPS,
        image_y_origin.expect("validated G_BG_1CYC carries imageYorig"),
        command,
    )?;
    scaled_background_ops(rdram, common, image, rdp, window, command)
}

pub(super) fn copy_background_ops(
    rdram: &[u8],
    common: BackgroundCommon,
    image: u32,
    rdp: &mut RdpDecodeState,
    window: BackgroundCopyWindow,
    command: &str,
) -> Result<Vec<RenderOp>, RenderError> {
    let mut operations = Vec::new();
    let mut scratch = BackgroundScratch::new();
    for slice in window.slices() {
        let (load_source_x_start, load_source_x_end) = if common.image_load == G_BGLT_LOADBLOCK {
            (0, window.image_width)
        } else {
            (slice.source_x_start, slice.source_x_end)
        };
        let mut rectangle = load_background_tile(
            rdram,
            &mut scratch,
            common,
            image,
            window.image_width,
            load_source_x_start,
            slice.source_y_start,
            load_source_x_end,
            slice.source_y_end,
            rdp,
            command,
        )?;
        if rectangle.other_mode.cycle_type() != CycleType::Copy {
            return Err(reject(format!(
                "{command} requires Copy cycle, got {:?}",
                rectangle.other_mode.cycle_type()
            )));
        }
        if common.image_format == 2 && rectangle.other_mode.texture_lut() == 0 {
            return Err(reject(format!(
                "{command} CI background requires an active RGBA16 or IA16 texture-LUT mode"
            )));
        }

        let frame_x = f32::from(common.frame_x) / 4.0;
        let frame_y = f32::from(common.frame_y) / 4.0;
        rectangle.ulx = frame_x + slice.output_x_start as f32;
        rectangle.uly = frame_y + slice.output_y_start as f32;
        rectangle.lrx = frame_x + slice.output_x_end as f32 - 1.0;
        rectangle.lry = frame_y + slice.output_y_end as f32 - 1.0;
        rectangle.s = if slice.reverse_s {
            (slice.source_x_end - 1 - load_source_x_start) as f32
        } else {
            (slice.source_x_start - load_source_x_start) as f32
        };
        rectangle.t = 0.0;
        rectangle.dsdx = if slice.reverse_s { -(4 << 10) } else { 4 << 10 };
        rectangle.dtdy = 1 << 10;
        operations.push(RenderOp::TextureRectangle(rectangle));
    }
    Ok(operations)
}

pub(super) fn scaled_background_ops(
    rdram: &[u8],
    common: BackgroundCommon,
    image: u32,
    rdp: &mut RdpDecodeState,
    window: ScaledBackgroundWindow,
    command: &str,
) -> Result<Vec<RenderOp>, RenderError> {
    let footprint = BackgroundFilterFootprint::from_rdp(rdp.texture_filter(), command)?;
    let slices = window.slices(footprint, command)?;
    let bits_per_texel = 4u32 << common.image_size;
    let tmem_capacity = if common.image_format == 2 { 256 } else { 512 };
    let mut operations = Vec::with_capacity(slices.len());
    let mut scratch = BackgroundScratch::new();
    for slice in slices {
        let (load_source_x_start, load_source_x_end) = if common.image_load == G_BGLT_LOADBLOCK {
            (0, window.image_width)
        } else {
            (slice.source_x_start, slice.source_x_end)
        };
        let source_width = load_source_x_end - load_source_x_start;
        let line_words = source_width
            .checked_mul(bits_per_texel)
            .and_then(|bits| bits.checked_add(63))
            .map(|bits| bits / 64)
            .ok_or_else(|| reject(format!("{command} TMEM line size overflow")))?;
        if line_words == 0 || line_words > tmem_capacity || line_words > 511 || source_width > 1024
        {
            return Err(reject(format!(
                "{command} source span width={source_width} line_words={line_words} exceeds one TMEM row"
            )));
        }

        let mut rectangle = load_background_tile(
            rdram,
            &mut scratch,
            common,
            image,
            window.image_width,
            load_source_x_start,
            slice.source_y,
            load_source_x_end,
            slice.source_y + 1,
            rdp,
            command,
        )?;
        if rectangle.other_mode.cycle_type() != CycleType::OneCycle {
            return Err(reject(format!(
                "{command} requires OneCycle mode, got {:?}",
                rectangle.other_mode.cycle_type()
            )));
        }
        debug_assert_eq!(rectangle.other_mode.texture_filter(), TextureFilter::Point);
        if common.image_format == 2 && rectangle.other_mode.texture_lut() == 0 {
            return Err(reject(format!(
                "{command} CI background requires an active RGBA16 or IA16 texture-LUT mode"
            )));
        }

        let frame_x = f32::from(common.frame_x) / 4.0;
        let frame_y = f32::from(common.frame_y) / 4.0;
        rectangle.ulx = frame_x + slice.output_x_start as f32;
        rectangle.uly = frame_y + slice.output_y as f32;
        rectangle.lrx = frame_x + slice.output_x_end as f32;
        rectangle.lry = frame_y + slice.output_y as f32 + 1.0;
        rectangle.s =
            (slice.source_x_start - load_source_x_start) as f32 + slice.s_start_10 as f32 / 1024.0;
        rectangle.t = slice.t_start_10 as f32 / 1024.0;
        rectangle.dsdx =
            i16::try_from(slice.dsdx_10).expect("validated scaled-background S gradient fits i16");
        rectangle.dtdy = window.scale_h_10 as i16;
        operations.push(RenderOp::TextureRectangle(rectangle));
    }
    Ok(operations)
}

pub(super) fn physical_pointer(pointer: u32, command: &str, field: &str) -> Result<u32, RenderError> {
    let class = pointer >> 24;
    if !matches!(class, 0x00 | 0x80 | 0xa0) {
        return Err(reject(format!(
            "{command} {field} {pointer:#010x} was not resolved to the physical 24-bit domain"
        )));
    }
    let pointer = pointer & 0x00ff_ffff;
    if !pointer.is_multiple_of(8) {
        return Err(reject(format!(
            "{command} {field} {pointer:#010x} is not 8-byte aligned"
        )));
    }
    Ok(pointer)
}

pub(super) fn resolve_s2dex_pointer(
    segments: &[u32; 16],
    pointer: u32,
    command: &str,
    field: &str,
) -> Result<u32, RenderError> {
    let class = pointer >> 24;
    if matches!(class, 0x80 | 0xa0) {
        return Ok(pointer & 0x00ff_ffff);
    }
    if class > 0x0f {
        return Err(reject(format!(
            "{command} {field} pointer {pointer:#010x} has non-public segment byte {class:#04x}"
        )));
    }
    segments[class as usize]
        .checked_add(pointer & 0x00ff_ffff)
        .filter(|resolved| *resolved < 0x0100_0000)
        .ok_or_else(|| {
            reject(format!(
                "{command} {field} pointer {pointer:#010x} overflows the 24-bit segmented address domain"
            ))
        })
}

pub(super) fn validate_copy_background_init(
    common: BackgroundCommon,
    observed: [u16; 6],
    command: &str,
) -> Result<(), RenderError> {
    if common.image_size > 3 || common.image_w == 0 || common.frame_w == 0 {
        return Err(reject(format!(
            "{command} cannot validate guS2DInitBg fields for imageSiz={} imageW={} frameW={}",
            common.image_size, common.image_w, common.frame_w
        )));
    }
    let image_width = u32::from(common.image_w / 4);
    let frame_width = u32::from(common.frame_w / 4);
    let shift = 4 - u32::from(common.image_size);
    let image_words = image_width >> shift;
    let frame_words = frame_width >> shift;
    let tmem_w = match common.image_load {
        G_BGLT_LOADBLOCK => image_words,
        G_BGLT_LOADTILE => frame_words + 1,
        _ => {
            return Err(reject(format!(
                "{command} imageLoad={:#06x} is not public",
                common.image_load
            )));
        }
    };
    if tmem_w == 0 {
        return Err(reject(format!("{command} guS2DInitBg computed zero tmemW")));
    }
    let capacity = if common.image_format == 2 { 256 } else { 512 };
    let tmem_h = (capacity / tmem_w) * 4;
    if tmem_h == 0 {
        return Err(reject(format!(
            "{command} guS2DInitBg geometry cannot fit one image row in TMEM"
        )));
    }
    let tmem_size_w = match common.image_load {
        G_BGLT_LOADBLOCK => tmem_w * 2,
        G_BGLT_LOADTILE => image_words * 2,
        _ => unreachable!(),
    };
    let tmem_size = tmem_size_w
        .checked_mul(tmem_h)
        .ok_or_else(|| reject(format!("{command} guS2DInitBg tmemSize overflow")))?;
    let tmem_load_sh = match common.image_load {
        G_BGLT_LOADBLOCK => tmem_size / 2 - 1,
        G_BGLT_LOADTILE => tmem_w * 16 - 1,
        _ => unreachable!(),
    };
    let tmem_load_th = match common.image_load {
        G_BGLT_LOADBLOCK => 2047 / tmem_w + 1,
        G_BGLT_LOADTILE => tmem_h - 1,
        _ => unreachable!(),
    };
    let expected_u32 = [
        tmem_w,
        tmem_h,
        tmem_load_sh,
        tmem_load_th,
        tmem_size_w,
        tmem_size,
    ];
    let mut expected = [0u16; 6];
    for (index, value) in expected_u32.into_iter().enumerate() {
        expected[index] = u16::try_from(value).map_err(|_| {
            reject(format!(
                "{command} guS2DInitBg derived field {index}={value} exceeds u16"
            ))
        })?;
    }
    if observed != expected {
        return Err(reject(format!(
            "{command} uObjBg guS2DInitBg fields are stale/uninitialized: observed={observed:?} expected={expected:?}"
        )));
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub(super) fn load_background_tile(
    rdram: &[u8],
    scratch: &mut BackgroundScratch,
    common: BackgroundCommon,
    image: u32,
    image_width: u32,
    source_x_start: u32,
    source_y_start: u32,
    source_x_end: u32,
    source_y_end: u32,
    rdp: &mut RdpDecodeState,
    command: &str,
) -> Result<crate::gbi::TextureRectangle, RenderError> {
    if source_x_end > 1024 || source_y_end > 1024 {
        return Err(reject(format!(
            "{command} source tile ({source_x_start},{source_y_start})..({source_x_end},{source_y_end}) exceeds public RDP tile coordinates"
        )));
    }
    let width = source_x_end - source_x_start;
    let height = source_y_end - source_y_start;
    let bits_per_texel = 4u32 << common.image_size;
    let line_words = (width * bits_per_texel).div_ceil(64);
    // Every independently loaded strip is rebased to staging T=0. LoadBlock
    // is a one-dimensional transfer: its low command coordinate is S, so
    // encoding source-row parity there would skip texels instead of selecting
    // an odd source row. Multi-row parity is retained naturally within the
    // rebased strip by DXT.
    let staged_y = 0;
    let staged_rows = staged_y + height;
    let staged_bytes = usize::try_from(
        width
            .checked_mul(staged_rows)
            .and_then(|texels| texels.checked_mul(bits_per_texel))
            .ok_or_else(|| reject(format!("{command} staged image size overflow")))?
            .div_ceil(8),
    )
    .expect("bounded background staging size fits usize");
    let command_start = (staged_bytes + 7) & !7;
    let command_end = command_start + 5 * 8;
    if command_end > scratch.bytes.len() {
        return Err(reject(format!(
            "{command} staged strip requires {command_end} bytes, exceeding the bounded {}-byte background scratch",
            scratch.bytes.len()
        )));
    }
    scratch.bytes[..command_end].fill(0);
    copy_background_texels(
        rdram,
        &mut scratch.bytes,
        common.image_size,
        image,
        image_width,
        source_x_start,
        source_y_start,
        width,
        height,
        staged_y,
    );
    let settimg = (u32::from(RDP_SETTIMG) << 24)
        | (u32::from(common.image_format) << 21)
        | (u32::from(common.image_size) << 19)
        | (width - 1);
    let load_line = if common.image_load == G_BGLT_LOADBLOCK {
        0
    } else {
        line_words
    };
    let settile = (u32::from(RDP_SETTILE) << 24)
        | (u32::from(common.image_format) << 21)
        | (u32::from(common.image_size) << 19)
        | (load_line << 9);
    let load_tile = 7 << 24;
    let load_command = if common.image_load == G_BGLT_LOADBLOCK {
        if source_x_start != 0 {
            return Err(reject(format!(
                "{command} LoadBlock lowering requires a full source row"
            )));
        }
        let count = width
            .checked_mul(height)
            .ok_or_else(|| reject(format!("{command} LoadBlock texel count overflow")))?;
        if count == 0 || count > 4096 {
            return Err(reject(format!(
                "{command} LoadBlock count={count} exceeds the public 12-bit span"
            )));
        }
        let dxt = 2047 / line_words + 1;
        (
            (u32::from(RDP_LOADBLOCK) << 24) | staged_y,
            load_tile | ((count - 1) << 12) | dxt,
        )
    } else {
        (
            (u32::from(RDP_LOADTILE) << 24) | (staged_y << 2),
            load_tile | ((width - 1) << 14) | ((staged_y + height - 1) << 2),
        )
    };
    let commands = [
        (settimg, 0),
        (settile, load_tile),
        (u32::from(RDP_LOADSYNC) << 24, 0),
        load_command,
        (u32::from(G_ENDDL) << 24, 0),
    ];
    for (index, (w0, w1)) in commands.into_iter().enumerate() {
        let offset = command_start + index * 8;
        scratch.bytes[offset..offset + 4].copy_from_slice(&w0.to_ne_bytes());
        scratch.bytes[offset + 4..offset + 8].copy_from_slice(&w1.to_ne_bytes());
    }
    let load_ops = crate::gbi::decode_raw_rdp_ops_with_state(
        &mut scratch.bytes[..command_end],
        command_start as u32,
        rdp,
    )?;
    if !load_ops.is_empty() {
        return Err(reject(format!(
            "{command} background load unexpectedly emitted {} operations",
            load_ops.len()
        )));
    }
    let sprite = ObjectSprite {
        obj_x: 0,
        scale_w: 1 << 10,
        image_w: u16::try_from(width * 32)
            .map_err(|_| reject(format!("{command} loaded tile width exceeds u10.5")))?,
        padding_x: 0,
        obj_y: 0,
        scale_h: 1 << 10,
        image_h: u16::try_from(height * 32)
            .map_err(|_| reject(format!("{command} loaded tile height exceeds u10.5")))?,
        padding_y: 0,
        image_stride: u16::try_from(line_words)
            .map_err(|_| reject(format!("{command} TMEM line exceeds u16")))?,
        image_address: 0,
        image_format: common.image_format,
        image_size: common.image_size,
        image_palette: common.image_palette as u8,
        image_flags: 0,
    };
    let RenderOp::TextureRectangle(rectangle) = rdp.object_rectangle(sprite).map_err(|error| {
        reject(format!(
            "{command} could not snapshot its loaded background tile: {error}"
        ))
    })?
    else {
        unreachable!("object rectangle lowering has one typed result")
    };
    Ok(rectangle)
}

#[allow(clippy::too_many_arguments)]
pub(super) fn copy_background_texels(
    rdram: &[u8],
    scratch: &mut [u8],
    image_size: u8,
    image: u32,
    image_width: u32,
    source_x: u32,
    source_y: u32,
    width: u32,
    height: u32,
    staged_y: u32,
) {
    let source = fn64_runtime::RdramView::from_storage(rdram);
    let mut staged = fn64_runtime::RdramViewMut::from_storage(scratch);
    for y in 0..height {
        for x in 0..width {
            let source_texel = (source_y + y) * image_width + source_x + x;
            let staged_texel = (staged_y + y) * width + x;
            match image_size {
                0 => {
                    let source_byte = source.read_u8(fn64_runtime::RdramAddr::from_offset(
                        image + source_texel / 2,
                    ));
                    let shift = if source_texel & 1 == 0 { 4 } else { 0 };
                    let texel = (source_byte >> shift) & 0x0f;
                    let staged_address = fn64_runtime::RdramAddr::from_offset(staged_texel / 2);
                    let old = staged.as_view().read_u8(staged_address);
                    let packed = if staged_texel & 1 == 0 {
                        (old & 0x0f) | (texel << 4)
                    } else {
                        (old & 0xf0) | texel
                    };
                    staged.write_u8(staged_address, packed);
                }
                1 => staged.write_u8(
                    fn64_runtime::RdramAddr::from_offset(staged_texel),
                    source.read_u8(fn64_runtime::RdramAddr::from_offset(image + source_texel)),
                ),
                2 => staged.write_u16(
                    fn64_runtime::RdramAddr::from_offset(staged_texel * 2),
                    source.read_u16(fn64_runtime::RdramAddr::from_offset(
                        image + source_texel * 2,
                    )),
                ),
                3 => staged.write_u32(
                    fn64_runtime::RdramAddr::from_offset(staged_texel * 4),
                    source.read_u32(fn64_runtime::RdramAddr::from_offset(
                        image + source_texel * 4,
                    )),
                ),
                _ => unreachable!("background image size was validated"),
            }
        }
    }
}

pub(super) fn read_object_sprite(
    rdram: &[u8],
    address: u32,
    command: &str,
) -> Result<ObjectSprite, RenderError> {
    let address = require_object_range(rdram, address, OBJ_SPRITE_BYTES, command)?;
    let view = fn64_runtime::RdramView::from_storage(rdram);
    let base = fn64_runtime::RdramAddr::from_offset(address as u32);
    let half = |offset| view.read_u16(base.checked_add(offset).expect("uObjSprite offset fits"));
    let byte = |offset| view.read_u8(base.checked_add(offset).expect("uObjSprite offset fits"));
    Ok(ObjectSprite {
        obj_x: half(0) as i16,
        scale_w: half(2),
        image_w: half(4),
        padding_x: half(6),
        obj_y: half(8) as i16,
        scale_h: half(10),
        image_h: half(12),
        padding_y: half(14),
        image_stride: half(16),
        image_address: half(18),
        image_format: byte(20),
        image_size: byte(21),
        image_palette: byte(22),
        image_flags: byte(23),
    })
}

pub(super) fn read_object_texture(
    rdram: &[u8],
    address: u32,
    segments: &[u32; 16],
    command: &str,
) -> Result<ObjectTexture, RenderError> {
    let address = require_object_range(rdram, address, OBJ_TEXTURE_BYTES, command)?;
    let view = fn64_runtime::RdramView::from_storage(rdram);
    let base = fn64_runtime::RdramAddr::from_offset(address as u32);
    let word = |offset| view.read_u32(base.checked_add(offset).expect("uObjTxtr offset fits"));
    let half = |offset| view.read_u16(base.checked_add(offset).expect("uObjTxtr offset fits"));
    let common = ObjectTextureCommon {
        image: resolve_s2dex_pointer(segments, word(4), command, "texture image")?,
        sid: half(14),
        flag: word(16),
        mask: word(20),
    };
    if !matches!(common.sid, 0 | 4 | 8 | 12) {
        return Err(reject(format!(
            "{command} uObjTxtr sid={} is outside the public status IDs 0,4,8,12",
            common.sid
        )));
    }
    match word(0) {
        G_OBJLT_TXTRBLOCK => Ok(ObjectTexture::Block {
            common,
            tmem: half(8),
            tsize: half(10),
            tline: half(12),
        }),
        G_OBJLT_TXTRTILE => Ok(ObjectTexture::Tile {
            common,
            tmem: half(8),
            twidth: half(10),
            theight: half(12),
        }),
        G_OBJLT_TLUT => {
            let zero = half(12);
            if zero != 0 {
                return Err(reject(format!(
                    "{command} uObjTxtrTLUT zero field must be 0, got {zero}"
                )));
            }
            Ok(ObjectTexture::Tlut {
                common,
                phead: half(8),
                pnum: half(10),
            })
        }
        kind => Err(unsupported(
            "render.s2dex.object-texture-type",
            format!(
                "unsupported S2DEX command {command}: uObjTxtr type {kind:#010x} is not G_OBJLT_TXTRBLOCK, G_OBJLT_TXTRTILE, or G_OBJLT_TLUT"
            ),
        )),
    }
}

pub(super) fn apply_object_texture(
    rdram: &[u8],
    texture: ObjectTexture,
    status: &mut [u32; 4],
    scratch: &mut ObjectTextureScratch,
    rdp: &mut RdpDecodeState,
    command: &str,
) -> Result<(), RenderError> {
    let common = texture.common();
    let slot = usize::from(common.sid / 4);
    if status[slot] & common.mask == common.flag {
        return Ok(());
    }
    let ObjectTextureRdpLoad {
        commands,
        image,
        image_bytes,
    } = object_texture_rdp_commands(rdram, texture, command)?;
    let command_start = (image_bytes + 7) & !7;
    let command_bytes = commands
        .len()
        .checked_mul(8)
        .ok_or_else(|| reject(format!("{command} synthesized RDP command length overflow")))?;
    let command_end = command_start
        .checked_add(command_bytes)
        .ok_or_else(|| reject(format!("{command} synthesized RDP range overflow")))?;
    if command_end > scratch.bytes.len() {
        return Err(reject(format!(
            "{command} bounded object-texture staging requires {command_end} bytes, exceeding its {}-byte scratch",
            scratch.bytes.len()
        )));
    }
    scratch.bytes[..command_end].fill(0);
    let source = fn64_runtime::RdramView::from_storage(rdram);
    let mut staged = fn64_runtime::RdramViewMut::from_storage(&mut scratch.bytes);
    for offset in 0..image_bytes {
        let offset = u32::try_from(offset).expect("bounded object texture size fits u32");
        staged.write_u8(
            fn64_runtime::RdramAddr::from_offset(offset),
            source.read_u8(fn64_runtime::RdramAddr::from_offset(image + offset)),
        );
    }
    for (index, (w0, w1)) in commands.into_iter().enumerate() {
        let offset = command_start + index * 8;
        scratch.bytes[offset..offset + 4].copy_from_slice(&w0.to_ne_bytes());
        scratch.bytes[offset + 4..offset + 8].copy_from_slice(&w1.to_ne_bytes());
    }
    let operations = crate::gbi::decode_raw_rdp_ops_with_state(
        &mut scratch.bytes[..command_end],
        command_start as u32,
        rdp,
    )?;
    if !operations.is_empty() {
        return Err(reject(format!(
            "{command} texture-only lowering unexpectedly emitted {} render operations",
            operations.len()
        )));
    }
    status[slot] = (status[slot] & !common.mask) | (common.flag & common.mask);
    Ok(())
}

pub(super) fn object_texture_rdp_commands(
    rdram: &[u8],
    texture: ObjectTexture,
    command: &str,
) -> Result<ObjectTextureRdpLoad, RenderError> {
    let common = texture.common();
    let (image, image_bytes) = require_image_range(rdram, common.image, texture, command)?;
    let settimg = (u32::from(RDP_SETTIMG) << 24) | (2 << 19);
    let settile = |line: u16, tmem: u16| {
        (u32::from(RDP_SETTILE) << 24) | (2 << 19) | (u32::from(line) << 9) | u32::from(tmem)
    };
    let load_tile = 7 << 24;
    let mut commands = match texture {
        ObjectTexture::Block {
            tmem, tsize, tline, ..
        } => {
            let high_s = (u32::from(tsize) + 1) * 4 - 1;
            vec![
                (settimg, 0),
                (settile(0, tmem), load_tile),
                (u32::from(RDP_LOADSYNC) << 24, 0),
                (
                    u32::from(RDP_LOADBLOCK) << 24,
                    load_tile | (high_s << 12) | u32::from(tline),
                ),
            ]
        }
        ObjectTexture::Tile {
            tmem,
            twidth,
            theight,
            ..
        } => {
            let width_16 = u32::from(twidth) + 1;
            let line = u16::try_from(width_16 / 4)
                .map_err(|_| reject(format!("{command} tile line exceeds u16")))?;
            vec![
                (settimg | (width_16 - 1), 0),
                (settile(line, tmem), load_tile),
                (u32::from(RDP_LOADSYNC) << 24, 0),
                (
                    u32::from(RDP_LOADTILE) << 24,
                    load_tile | ((u32::from(twidth) * 4) << 12) | u32::from(theight),
                ),
            ]
        }
        ObjectTexture::Tlut { phead, pnum, .. } => vec![
            (settimg, 0),
            (settile(0, phead), load_tile),
            (u32::from(RDP_LOADSYNC) << 24, 0),
            (
                u32::from(RDP_LOADTLUT) << 24,
                load_tile | (u32::from(pnum) << 14),
            ),
        ],
    };
    commands.push((u32::from(G_ENDDL) << 24, 0));
    Ok(ObjectTextureRdpLoad {
        commands,
        image,
        image_bytes,
    })
}
