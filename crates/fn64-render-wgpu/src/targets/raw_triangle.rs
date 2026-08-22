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
    TexrectBlendRegisters, TexrectCombinerEvaluation, TexrectExecutionError, TexrectFragmentStages,
    TexrectShading, TexrectTileBinding,
};
use super::{
    CandidateColorTarget, ColorTargetFormat, CompletedColorTargetWrite, DeviceColorBytes,
    TargetError, TargetRectangle,
};
use crate::raw_dpc::{triangle_span, RawTriangle};
use crate::tmem::{
    sample_point, PointSampleCoordinates, PointSampleRequest, TextureCoordinateS10_5,
    TmemFirstRowParity,
};
use crate::{CycleType, OtherMode, TextureLutMode, TmemByteSource};

/// The TMEM binding one TEXTURED raw triangle samples through: the tile pair
/// current at its own stream position, the TMEM image that position observes,
/// and the TLUT mode its `SetOtherMode` selected.
///
/// **The image is selected by the caller, never by this module.** It is the
/// SAME `TexrectTmemSource::Pending` / `prefix_before` selection
/// `execute_scheduled_texrect` makes -- see `production.rs`'s schedule loop
/// -- passed in already resolved. Reimplementing that rule here would give a
/// packet's triangles and its texrects two answers to "which load did this
/// draw see", and within one packet TMEM is not one image: WM2000's measured
/// sixth packet interleaves seven `LoadTile`s with seven draws all loading
/// from TMEM word zero, so a shared image holds only the seventh load's
/// texels and every draw samples the seventh sprite.
pub struct RawTriangleTexture<'a, S: TmemByteSource + ?Sized> {
    /// The `SetTile`/`SetTileSize` pair for the tile index the TRIANGLE's own
    /// wire word 0 names -- not a frozen tile 0.
    pub tile: TexrectTileBinding,
    /// TMEM as this triangle's stream position observes it.
    pub tmem: &'a S,
    pub lut_mode: TextureLutMode,
}

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
#[allow(clippy::too_many_arguments)]
pub fn execute_raw_triangle<S: TmemByteSource + ?Sized>(
    candidate: &CandidateColorTarget,
    other_mode: OtherMode,
    triangle: &RawTriangle,
    shading: TexrectShading,
    blend_registers: TexrectBlendRegisters,
    resident_bytes: &[u8],
    declared: &[fn64_render_ir::ResourceAccess],
    texture: Option<RawTriangleTexture<'_, S>>,
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
    // **`texel_available` is the wire opcode's own texture bit AND a
    // resolved binding**, never one without the other. A textured triangle
    // whose caller passed no `RawTriangleTexture` would combine against a
    // fabricated zero texel; a caller that passed one for an UNtextured
    // triangle has no S/T/W planes to evaluate. Both are refused by this
    // one equality rather than tolerated in either direction.
    let textured = triangle.flags().textured();
    if textured != texture.is_some() {
        return Err(
            TexrectExecutionError::TriangleTextureBindingDisagreesWithOpcode {
                opcode_textured: textured,
                binding_present: texture.is_some(),
            },
        );
    }
    let base_inputs = shading
        .validate_combiner_program_for(cycles, triangle.flags().shaded(), textured)?
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
        return Err(
            TexrectExecutionError::TriangleRowCountDisagreesWithJournal {
                declared: declared.len(),
                rasterized: rows.len(),
            },
        );
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
            return Err(
                TexrectExecutionError::TriangleRowRangeDisagreesWithJournal {
                    position,
                    declared: (0, 0),
                    rasterized: (start, len),
                },
            );
        };
        let declared_range = (range.start().get(), range.len());
        if declared_range != (start, len) {
            return Err(
                TexrectExecutionError::TriangleRowRangeDisagreesWithJournal {
                    position,
                    declared: declared_range,
                    rasterized: (start, len),
                },
            );
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

    // The three S/T/W planes, decoded from this triangle's OWN texture
    // coefficient block by the same `coefficient_components` the shade
    // block goes through -- not a second transcription of the split
    // fixed-point layout. `None` exactly when the wire opcode has no
    // texture bit, which the equality above already proved matches
    // `texture`.
    let texture_planes = triangle.texture().map(triangle_span::texture_planes);

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
        texture_planes,
        texture,
        other_mode.texture_perspective(),
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
fn raster_triangle<S: TmemByteSource + ?Sized>(
    bytes: &mut [u8],
    format: ColorTargetFormat,
    width: u32,
    triangle: &RawTriangle,
    rows: &[triangle_span::CoveredRow],
    shading: TexrectShading,
    base_inputs: crate::CombinerInputs,
    shade: Option<[triangle_span::AttributePlane; 4]>,
    texture_planes: Option<[triangle_span::AttributePlane; 3]>,
    texture: Option<RawTriangleTexture<'_, S>>,
    perspective: bool,
    blend_state: crate::BlendModeState,
    stages: TexrectFragmentStages,
    evaluation: TexrectCombinerEvaluation,
) -> Result<(), TexrectExecutionError> {
    // Hoisted out of the pixel loop: this is the per-PIXEL path, and a live
    // lane is measuring frame rate. `enabled()` is itself a `OnceLock` read,
    // but binding it here keeps even that out of the inner loop.
    let census_pixels = crate::combiner::census::enabled();
    // Distinct texels THIS triangle samples, for the spatial-variation
    // question the whole-frame luma histogram cannot answer. Only allocated
    // when the census is on.
    // Only for a TEXTURED triangle: an untextured one samples nothing, and
    // recording its empty set as "1 distinct texel" would put correct
    // untextured geometry in the bucket that means "flat despite a
    // texture" -- the exact bucket the measurement turns on.
    let mut distinct_texels = if census_pixels && texture.is_some() {
        Some(std::collections::HashSet::new())
    } else {
        None
    };
    // The S/T range this triangle actually requested, in S10.5 raw units,
    // tracked alongside the distinct-texel count so a one-texel triangle can
    // be attributed to constant COORDINATES rather than to collapsing
    // ADDRESSING without a further run.
    let mut coordinate_range: Option<(i16, i16, i16, i16)> = None;
    let bytes_per_pixel = format.bytes_per_pixel() as usize;
    // **First-row parity comes from the tile's own T origin, not a
    // constant** -- `execute_texture_rectangle`'s own rule, applied here for
    // the identical reason: the reader owes the WRITER the same parity, or
    // the two disagree about which TMEM rows carry the XOR4 bank exchange.
    // A frozen `Even` is correct only for an even T origin, and WM2000's
    // measured sprite-strip tile has `low_t.integer() == 47`, an odd one.
    let first_row_parity = texture.as_ref().map(|texture| {
        if texture.tile.size().low_t().integer() & 1 == 1 {
            TmemFirstRowParity::Odd
        } else {
            TmemFirstRowParity::Even
        }
    });
    for row in rows {
        // **Incremental attribute run state.** Attribute planes are exactly
        // linear in x while the selected subsample holds, so a run can be
        // stepped by `plane.dx` instead of re-evaluating the full formula
        // (an i128 multiply and divide per plane, up to seven planes per
        // pixel). Reset per row: the first covered pixel of every row must
        // restart from the exact formula.
        let mut previous_sample: Option<(i32, i64)> = None;
        let mut shade_values: Option<[i64; 4]> = None;
        for x in row.x0..row.x1 {
            // **One subsample scan, not two.**
            //
            // This used to call `pixel_coverage` (which counts ALL eight
            // subsamples) and then `attribute_sample` (which rescans the
            // same eight, in the same order, and returns the FIRST covered
            // one). Both walk identical rows, identical checkerboard
            // columns, and the identical `sample_x >= left_x && < right_x`
            // predicate -- so the pair did up to 16 sample tests where one
            // scan stopping at the first hit answers both questions.
            //
            // The count itself was never consumed: the only uses were
            // `coverage == 0` and a `debug_assert!(coverage <= 8)`. The
            // blender supplies `Coverage::FULL` independently
            // (`texrect.rs`'s `blend_and_write_pixel` call below), so no
            // downstream stage reads the number.
            //
            // Bit-exact by construction: `attribute_sample` returns `Some`
            // exactly when at least one subsample is inside, which is
            // exactly `pixel_coverage(..) > 0`. Same predicate, same
            // traversal order, same first hit.
            let sample = triangle_span::attribute_sample(triangle, x as i32, row.y as i32);
            let Some((delta_y_eighth, delta_x)) = sample else {
                // A hole in a declared row breaks the run: the next covered
                // pixel must restart from the exact formula.
                previous_sample = None;
                shade_values = None;
                continue;
            };
            // Step only when the SAME subsample advanced exactly one pixel.
            // Checking the returned deltas (not merely adjacent x) is what
            // makes this exact: it restarts across an X- or Y-subsample
            // change and across a zero-coverage hole alike.
            let continues_run = previous_sample.is_some_and(|(prev_y, prev_x)| {
                prev_y == delta_y_eighth
                    && prev_x.checked_add(triangle_span::Q16_ONE) == Some(delta_x)
            });
            shade_values = match (shade, shade_values, continues_run) {
                (Some(planes), Some(values), true) => Some(std::array::from_fn(|c| {
                    triangle_span::attribute_plane_step(planes[c], values[c])
                })),
                (Some(planes), _, _) => Some(std::array::from_fn(|c| {
                    triangle_span::attribute_plane(planes[c], delta_y_eighth, delta_x)
                })),
                (None, _, _) => None,
            };
            previous_sample = Some((delta_y_eighth, delta_x));
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
            // The attribute sample point, computed ONCE per pixel and shared
            // by the shade and texture planes. The RDP evaluates every one of
            // a fragment's attributes at the SAME covered subsample, so
            // sampling the two at different points would put a pixel's colour
            // and its texel a quarter pixel apart.
            let inputs = match (shade, sample) {
                (Some(_), Some(_)) => {
                    let values = shade_values.expect("a shaded triangle carries run values");
                    let shade_color = std::array::from_fn(|component| {
                        // Q16.16 -> [0.0, 1.0]: the plane's integer part is
                        // the 0..=255 channel value, clamped the way the
                        // RDP's own colour combiner input stage clamps it.
                        let value = values[component]
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
            // **The texel, sampled through the texrect path's own one
            // sampler.** `sample_point` is `tmem/sample.rs`'s existing
            // reader, monomorphized over whichever TMEM image the CALLER
            // selected -- the same shift/mask/mirror/clamp addressing, the
            // same validity-gated physical read, the same format and TLUT
            // decode. There is no second sampler and no second tile binding.
            //
            // `[0; 4]` for an untextured triangle is not a substitution: the
            // admission above refused every `Texel0`-reading selector for
            // one, so nothing reads it.
            let texel = match (&texture, texture_planes, sample) {
                (Some(binding), Some(planes), Some((delta_y_eighth, delta_x))) => {
                    // The three planes at the pixel's own covered subsample,
                    // in Q16.16 -- the identical `attribute_plane` call the
                    // shade components go through.
                    let stw: [i64; 3] = core::array::from_fn(|component| {
                        triangle_span::attribute_plane(planes[component], delta_y_eighth, delta_x)
                    });
                    let (s, t) = triangle_span::texture_coordinates_s10_5(stw, perspective);
                    if census_pixels {
                        coordinate_range = Some(match coordinate_range {
                            None => (s, s, t, t),
                            Some((s_low, s_high, t_low, t_high)) => {
                                (s_low.min(s), s_high.max(s), t_low.min(t), t_high.max(t))
                            }
                        });
                    }
                    let request = PointSampleRequest::new(
                        PointSampleCoordinates::new(
                            TextureCoordinateS10_5::from_raw(s),
                            TextureCoordinateS10_5::from_raw(t),
                        ),
                        first_row_parity.expect("a bound texture resolved its parity above"),
                    );
                    sample_point(
                        binding.tmem,
                        binding.tile.descriptor(),
                        binding.tile.size(),
                        request,
                        binding.lut_mode,
                    )
                    .map_err(|source| TexrectExecutionError::Sample {
                        column: x,
                        row: row.y,
                        source,
                    })?
                    .texel()
                    .rgba8888()
                }
                // A textured triangle whose pixel has no covered subsample
                // cannot reach here: the `coverage == 0` guard above already
                // proved one exists, and `attribute_sample` scans the same
                // samples in the same order. Refused rather than `expect`ed,
                // for the same reason the shade arm is.
                (Some(_), _, None) | (Some(_), None, _) => {
                    return Err(TexrectExecutionError::TriangleAttributeSampleMissing {
                        column: x,
                        row: row.y,
                    })
                }
                (None, _, _) => [0; 4],
            };
            // **Diagnostic-only, and gated on the same flag as the program
            // census.** Records the two values the dominant measured program
            // multiplies together, so "the texel is wrong" and "the shade
            // alpha is flat" can be told apart from one run. Both are read
            // here, at the one place both are final: `texel` is the
            // sampler's own output and `inputs.shade_color[3]` is the
            // interpolated plane after the same clamp the combiner sees.
            if census_pixels {
                crate::combiner::census::note_pixel(
                    texel,
                    (inputs.shade_color[3] * 255.0).round() as u8,
                );
                if let Some(seen) = distinct_texels.as_mut() {
                    // Capped: the question is "is this triangle flat", and a
                    // triangle past 512 distinct texels has answered it. The
                    // cap also bounds the set on a full-screen quad.
                    if seen.len() <= 512 {
                        seen.insert(texel);
                    }
                }
            }
            let combined = combine_one_texel(shading.combine(), inputs, texel, evaluation);
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
    if let Some(seen) = distinct_texels {
        crate::combiner::census::note_triangle_distinct_texels(seen.len());
        // S10.5 raw units carry five fractional bits, so `>> 5` is the whole
        // texel count the span covered. A triangle spanning less than one
        // texel is genuinely entitled to one distinct texel; one spanning
        // many and still reading one is the defect.
        if let Some((s_low, s_high, t_low, t_high)) = coordinate_range {
            crate::combiner::census::note_triangle_spread(
                (i32::from(s_high) - i32::from(s_low)) >> 5,
                (i32::from(t_high) - i32::from(t_low)) >> 5,
                perspective,
            );
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
