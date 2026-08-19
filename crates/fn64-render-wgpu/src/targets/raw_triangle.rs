//! The CPU rasterizer for one opaque, untextured raw RDP triangle -- shaded
//! or flat -- producing the same [`CompletedColorTargetWrite`] the fill and
//! texrect executors produce.
//!
//! # Why a CPU rasterizer and not the GPU one
//!
//! `docs/RT64-TRIANGLE-WRITEBACK.md` records the fork and the disproofs. In
//! short: the guest-visible path has no GPU in it. `ColorTargetRegistry`'s
//! `device_bytes` is a CPU `Vec<u8>`, `draw_admitted_triangles` refuses
//! without an adapter, and `TriangleDrawOutput` is `Rgba8Unorm` at a
//! `RenderConfig` extent while the guest framebuffer is RGBA16 at a
//! `SetColorImage` one. Routing the GPU raster back would requantize
//! invented content across a stride nothing checks, and would make every
//! guest-visible claim adapter-conditional. So a raw triangle gets the same
//! CPU seam the texrect already has.
//!
//! # What this executor is, and is not
//!
//! It executes opcodes 0x08 (flat) and 0x0c (shaded) -- no texture plane,
//! no depth plane on the wire -- in non-Fill cycle, drawn through the
//! latched combiner and blender. Shade planes ARE interpolated per pixel.
//!
//! It is **not** a general triangle rasterizer. There is no s/t/w
//! perspective divide, no LOD, and no depth test -- which is why it draws
//! nothing in WM2000, whose 826,056 attract-loop triangles are all 0x0e
//! (shaded AND textured). The texture rung is the one that remains; see
//! `docs/RT64-TRIANGLE-WRITEBACK.md`.
//! `crate::raw_dpc::raw_triangle_is_executable` is the decoder-side twin of
//! that admission, and the two must widen together in that order: the
//! executor first, the declaration second. Declaring a row this executor
//! cannot fill is worse than declaring none, because `fill_completed_writes`
//! slices the full-extent buffer for every declared range without checking
//! the raster touched it.
//!
//! # Coverage is a superset in exactly one direction
//!
//! The journal declares whole `[x0, x1)` runs per scanline, because a
//! `ResourceAccess` is a byte range and cannot express "these pixels within
//! the range". A pixel at either end of that run may have zero subpixel
//! coverage. That is safe because this executor LEAVES such a pixel holding
//! the resident's own current byte -- it is skipped, not painted -- so the
//! declared range's content is always real, current content rather than
//! stale bytes from a generation the accumulator moved past, and the
//! triangle's colour never lands outside the triangle. See
//! [`raster_triangle`]'s loop and
//! `a_declared_pixel_with_no_subpixel_coverage_is_not_painted`.

use fn64_render_ir::ResourceAccess;

use super::texrect::{
    admitted_cycle_evaluation, blend_and_write_pixel, combine_one_texel, require_blendable_mode,
    TexrectBlendRegisters, TexrectCombinerEvaluation, TexrectExecutionError,
    TexrectFragmentStages, TexrectShading,
};
use super::{
    CandidateColorTarget, ColorTargetFormat, CompletedColorTargetWrite, DeviceColorBytes,
    TargetError, TargetRectangle,
};
use crate::raw_dpc::{triangle_span, RawTriangle};
use crate::{CycleType, OtherMode};

/// The subpixel coverage a fully-covered pixel has: two X sample columns
/// times four Y sample rows.
const FULL_COVERAGE: u32 = 8;

/// Rasterizes one flat raw triangle into `resident_bytes`, returning the
/// full-extent result.
///
/// `declared` is the DECODER's own `ResourceAccess` run for this triangle.
/// This executor recomputes the row list from `triangle_span::covered_rows`
/// -- the same function the decoder called -- and refuses by name unless
/// every declared range is exactly the byte range of the row at the same
/// position.
///
/// Comparing the RANGES, not merely the count, is deliberate. Equal counts
/// would leave the equality an inference ("the two walks differ only in
/// their height bound, and the widths are always the same value, so equal
/// lengths implies equal rows"). Comparing the ranges makes it a check.
///
/// That check is not belt-and-braces; it is the whole safety argument. The
/// decoder bounds its row walk by installed RDRAM and a fixed cap, because
/// `SetColorImage` carries no height and the target extent does not exist at
/// decode time. This executor bounds the same walk by the real extent. The
/// two bounds are different, so the two lists CAN differ -- and if they do,
/// `fill_completed_writes` would slice and digest bytes for every declared
/// row including ones this raster never visited, putting stale content into
/// guest RDRAM under a valid digest. Refusing here is the only place that
/// divergence is visible.
///
/// `resident_bytes` is required and must be the target's full extent: a
/// triangle covers a sub-region, so every pixel outside it must come from
/// real prior content, which in the composed schedule is the previous
/// command's own output.
pub fn execute_raw_triangle(
    candidate: &CandidateColorTarget,
    other_mode: OtherMode,
    triangle: &RawTriangle,
    shading: TexrectShading,
    blend_registers: TexrectBlendRegisters,
    resident_bytes: &[u8],
    declared: &[fn64_render_ir::ResourceAccess],
) -> Result<CompletedColorTargetWrite, TexrectExecutionError> {
    // Cycle, combiner-program, blender and fragment-stage admission all run
    // BEFORE any pixel is produced, so a mode this executor cannot evaluate
    // exactly refuses with an untouched target rather than a half-drawn one.
    // Same order and same reason as `execute_texture_rectangle`.
    let evaluation = admitted_cycle_evaluation(other_mode.cycle_type())?;
    // Copy cycle blits a texel. An untextured triangle has no texel to blit,
    // so unlike a texrect this executor cannot pass it through -- refused by
    // the same named error the cycle admission uses, rather than combining
    // against a fabricated zero texel.
    if matches!(evaluation, TexrectCombinerEvaluation::BlitsTheTexel) {
        return Err(TexrectExecutionError::UnsupportedCycleType {
            cycle_type: CycleType::Copy,
        });
    }
    let cycles = evaluation
        .validated_cycles()
        .expect("Copy cycle was refused above, and Fill by admitted_cycle_evaluation");
    // A shaded triangle may read `Shade`; an unshaded one may not, because
    // there is nothing to read and `base_inputs`' zeroed field would be a
    // silent substitution. The flag is the wire opcode's own shade bit, not
    // a policy.
    let base_inputs = shading
        .validate_combiner_program_with_shade(cycles, triangle.flags().shaded())?
        .base_inputs();
    let blend_state = blend_registers.mode_state(other_mode);
    require_blendable_mode(blend_state)?;
    let stages = TexrectFragmentStages::try_new(other_mode, blend_registers.blend_color())?;

    let key = candidate.key();
    let format = key.format();
    let extent = key.extent();
    let bytes_per_pixel = format.bytes_per_pixel() as usize;
    let full_len = (extent.pixels() as usize)
        .checked_mul(bytes_per_pixel)
        .ok_or(TargetError::PixelBufferLengthOverflow {
            pixels: extent.pixels() as usize,
            bytes_per_pixel: format.bytes_per_pixel(),
        })?;
    if resident_bytes.len() != full_len {
        return Err(TargetError::CompletedByteLengthMismatch {
            key,
            generation: candidate.generation(),
            expected: full_len,
            actual: resident_bytes.len(),
        }
        .into());
    }

    // **The rows the DECODER declared, recomputed here from the same
    // function with the same inputs.** Not a second derivation: identical
    // call, identical arguments, so it cannot disagree. The alternative --
    // threading the decoder's `Vec` through the executor signature -- was
    // rejected because the executor also needs the target extent to bound
    // the walk, and the decoder has no extent to bound it with. Instead the
    // extent is checked against the rows below, and a row outside it is a
    // named refusal rather than a silent clip.
    let rows = triangle_span::covered_rows(triangle, extent.width(), extent.height());
    if rows.len() != declared.len() {
        return Err(TexrectExecutionError::TriangleRowCountDisagreesWithJournal {
            declared: declared.len(),
            rasterized: rows.len(),
        });
    }
    // Range for range, in order. Each row's bytes start at the target's own
    // base plus `(y * width + x0) * bpp` and run `(x1 - x0) * bpp` -- which
    // is the identical arithmetic `plan_raw_triangle` used, over the
    // identical row, so a mismatch means the two walks genuinely disagreed.
    let base = key.address().get();
    let bpp = format.bytes_per_pixel();
    for (position, (row, access)) in rows.iter().zip(declared.iter()).enumerate() {
        let start = base + (row.y * extent.width() + row.x0) * bpp;
        let len = (row.x1 - row.x0) * bpp;
        // A non-RDRAM region is refused outright rather than mapped to a
        // sentinel: `(0, 0)` would be indistinguishable from a legitimate
        // zero-length range at address zero, and "this access names the
        // wrong RESOURCE" is a different fault from "it names the wrong
        // bytes". `verify_accesses_inside` already proved the resource
        // upstream, which is exactly why this arm must not quietly agree.
        let fn64_render_ir::ResourceRegion::Rdram { range, .. } = access.region() else {
            return Err(TexrectExecutionError::TriangleRowRangeDisagreesWithJournal {
                position,
                declared: (0, 0),
                rasterized: (start, len),
            });
        };
        let declared_range = (range.start().get(), range.len());
        if declared_range != (start, len) {
            return Err(TexrectExecutionError::TriangleRowRangeDisagreesWithJournal {
                position,
                declared: declared_range,
                rasterized: (start, len),
            });
        }
    }
    if rows.is_empty() {
        // A triangle whose declared rows are all empty at this extent has
        // nothing to draw. The caller only reaches this executor for a
        // triangle the decoder DID declare rows for, so an empty list here
        // means the executor's extent and the decoder's RDRAM bound
        // disagree -- a named refusal, never a silent no-op that would
        // leave declared ranges holding stale bytes.
        return Err(TexrectExecutionError::Target(TargetError::ZeroRectangle {
            x: 0,
            y: 0,
            width: 0,
            height: 0,
        }));
    }
    // The declared rows and the drawn rows must be the SAME rows. The
    // decoder bounded its walk by installed RDRAM and a fixed row cap, not
    // by this extent, so a triangle taller or wider than the target would
    // make the two lists differ -- which is the stale-digest hazard. Refused
    // by name here rather than clipped.
    let last = rows.last().expect("rows is non-empty");
    let rectangle = TargetRectangle::try_new(
        rows.iter().map(|row| row.x0).min().expect("non-empty"),
        rows[0].y,
        {
            let left = rows.iter().map(|row| row.x0).min().expect("non-empty");
            let right = rows.iter().map(|row| row.x1).max().expect("non-empty");
            right - left
        },
        last.y - rows[0].y + 1,
    )?;
    if rectangle.x() + rectangle.width() > extent.width()
        || rectangle.y() + rectangle.height() > extent.height()
    {
        return Err(TexrectExecutionError::OutsideTarget { key, rectangle });
    }

    // The four RGBA shade planes, decoded from this triangle's own shade
    // coefficient block. `None` for an unshaded triangle, which is a fact
    // about the wire opcode rather than a missing value.
    let shade = triangle.shade().map(triangle_span::shade_planes);

    let mut bytes = resident_bytes.to_vec();
    raster_triangle(
        &mut bytes,
        format,
        extent.width(),
        triangle,
        &rows,
        shading,
        base_inputs,
        shade,
        blend_state,
        stages,
        evaluation,
    )?;

    let device_bytes = DeviceColorBytes::new_for_fill(key, candidate.generation(), format, bytes)?;
    Ok(CompletedColorTargetWrite::new_for_fill(
        key,
        candidate.generation(),
        key.range(),
        rectangle,
        device_bytes,
    ))
}

/// The per-pixel loop, over exactly the declared rows.
///
/// **Every pixel in a declared run is visited**, including the ones whose
/// subpixel coverage is zero. A zero-coverage pixel is not skipped and not
/// left untouched: it is written back with the byte it already held. That is
/// what makes a declared byte range's digest describe real, current content
/// even though the range is a superset of the covered pixels -- and it costs
/// nothing, because the source of that byte is the resident copy this
/// function is mutating in place.
///
/// The colour of a covered pixel comes from the latched combiner program
/// evaluated through [`combine_one_texel`], the texrect path's own
/// evaluator.
///
/// `Texel0` is passed as zero and it is never read: this executor admits
/// only UNTEXTURED triangles, so there is no texel, and a program selecting
/// `Texel0` would be combining against a value nothing produced. That is
/// the one selector the admission cannot currently refuse, because
/// `ADMITTED_COLOR_INPUTS` admits it for the texrect path that shares the
/// table -- see this module's `Nonclaim` below.
///
/// A SHADED triangle's `Shade` comes from real per-pixel interpolation of
/// its own coefficient planes; an unshaded one's stays zero and is refused
/// by `validate_combiner_program_with_shade` before any pixel is produced.
#[allow(clippy::too_many_arguments)]
fn raster_triangle(
    bytes: &mut [u8],
    format: ColorTargetFormat,
    width: u32,
    triangle: &RawTriangle,
    rows: &[triangle_span::CoveredRow],
    shading: TexrectShading,
    base_inputs: crate::CombinerInputs,
    shade: Option<[triangle_span::AttributePlane; 4]>,
    blend_state: crate::BlendModeState,
    stages: TexrectFragmentStages,
    evaluation: TexrectCombinerEvaluation,
) -> Result<(), TexrectExecutionError> {
    let bytes_per_pixel = format.bytes_per_pixel() as usize;
    for row in rows {
        for x in row.x0..row.x1 {
            // The coverage this pixel actually has, from the SAME span
            // module the row list came from. Zero here is a real answer, not
            // a skip: see this function's own doc.
            let coverage = triangle_span::pixel_coverage(triangle, x as i32, row.y as i32);
            if coverage == 0 {
                continue;
            }
            debug_assert!(coverage <= FULL_COVERAGE);
            // **The shade colour is interpolated per pixel, at the pixel's
            // own covered subsample -- not at its centre.** The RDP
            // evaluates a fragment's attributes at a subsample it actually
            // covers, so an edge pixel's colour comes from inside the
            // triangle rather than from a point the triangle misses.
            //
            // `attribute_sample` returning `None` cannot happen here: the
            // `coverage == 0` guard above already proved this pixel has a
            // covered subsample, and both functions scan the same samples in
            // the same order. Handled rather than `expect`ed so a future
            // divergence between them refuses instead of aborting.
            let inputs = match (shade, triangle_span::attribute_sample(triangle, x as i32, row.y as i32)) {
                (Some(planes), Some((delta_y_eighth, delta_x))) => {
                    let shade_color = std::array::from_fn(|component| {
                        // Q16.16 -> [0.0, 1.0]: the plane's integer part is
                        // the 0..=255 channel value, clamped the way the
                        // RDP's own colour combiner input stage clamps it.
                        let value = triangle_span::attribute_plane(
                            planes[component],
                            delta_y_eighth,
                            delta_x,
                        )
                        .div_euclid(triangle_span::Q16_ONE)
                        .clamp(0, 255);
                        value as f32 / 255.0
                    });
                    crate::CombinerInputs {
                        shade_color,
                        ..base_inputs
                    }
                }
                // An unshaded triangle keeps `base_inputs`' zeroed shade.
                // That is not a substitution: `validate_combiner_program`
                // refuses every Shade-reading selector for an unshaded
                // triangle, so nothing reads it.
                (None, _) => base_inputs,
                (Some(_), None) => {
                    return Err(TexrectExecutionError::TriangleAttributeSampleMissing {
                        column: x,
                        row: row.y,
                    })
                }
            };
            let combined = combine_one_texel(shading.combine(), inputs, [0; 4], evaluation);
            let offset = (row.y as usize * width as usize + x as usize) * bytes_per_pixel;
            blend_and_write_pixel(
                format,
                &mut bytes[offset..offset + bytes_per_pixel],
                combined,
                blend_state,
                stages,
                x,
                row.y,
            )?;
        }
    }
    Ok(())
}

/// The exact ordered accesses one rasterized triangle's rows correspond to,
/// for the caller to hand back to the journal.
///
/// Deliberately absent: this module never derives accesses. The decoder's
/// own `plan_raw_triangle` pushed them and `bind_texture_rectangle` returns
/// them; a second derivation here is exactly the drift
/// `ExactRawDpcPlanWriter::finish` exists to catch.
pub type DeclaredAccesses<'a> = &'a [ResourceAccess];

#[cfg(test)]
mod tests;
