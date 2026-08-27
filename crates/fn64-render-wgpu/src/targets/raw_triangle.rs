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
//! # Parallel scanlines
//!
//! A depth-free triangle with at least 4,096 pixels across its declared row
//! runs uses a persistent work-stealing pool over exclusive whole scanlines.
//! The scalar pixel body stays the only implementation: each worker calls it
//! with a local row and guest-row base, and completing the parallel iterator
//! is the draw-order barrier before the next command can observe the target.
//! Depth-bearing draws and combiner census runs remain scalar because they
//! carry cross-row mutable state. `FN64_PARALLEL_RASTER=0` selects the scalar
//! A/B lane; absent selects parallelism, and every other value traps rather
//! than silently changing the lane.
//!
//! # Draw census
//!
//! `FN64_DRAW_CENSUS=1` times the raster call and aggregates successful draws
//! by complete resolved combiner/other-mode state and structural flags. It
//! reports a bounded top twelve every 25,000 draws. The switch is diagnostic
//! only and absent by default; its disabled hot-path cost is one cached branch
//! per draw, never a clock read, allocation, lock, or per-pixel operation.
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

use rayon::prelude::*;
use std::borrow::Cow;
use std::sync::OnceLock;

use super::texrect::{
    admitted_cycle_evaluation, blend_and_write_pixel, blend_and_write_pixel_with_coverage,
    combine_one_texel, combine_one_texel_prepared_two_cycle, require_blendable_mode,
    TexrectBlendRegisters, TexrectCombinerEvaluation, TexrectExecutionError, TexrectFragmentStages,
    TexrectShading, TexrectTileBinding,
};
use super::{
    CandidateColorTarget, ColorTargetFormat, CompletedColorTargetWrite, DeviceColorBytes,
    TargetError, TargetRectangle,
};
use crate::combiner::PreparedTwoCycleCombiner;
use crate::raw_dpc::{triangle_span, RawTriangle};
use crate::state::{DepthMode, PrimDepth};
use crate::tmem::{
    sample_point, PointSampleCoordinates, PointSampleRequest, PreparedPointSampler,
    TextureCoordinateS10_5, TmemFirstRowParity,
};
use crate::{CombineParams, CycleType, OtherMode, TextureLutMode, TmemByteSource};

mod prepared;
pub(crate) use prepared::{execute_prepared_raw_triangle_row_bins, PreparedRawTriangleRaster};

/// One RDP depth-memory cell of the CPU raster path's depth accumulator: the
/// 18-bit working Z and the stored four-bit DeltaZ exponent, exactly the pair
/// [`crate::depth_mode::relations`] compares against. Seeded to `(0, 0)` --
/// the value a zeroed guest z-image decodes to -- for every pixel of the
/// target when the packet's first z-compared or z-updated draw appears.
pub type DepthCell = (u32, u8);

/// The z-buffer wiring one raw-triangle draw carries into the raster loop.
///
/// This is the guest-visible seam's answer to the depth test the RDP does in
/// hardware and that RT64/angrylion both model: overlapping triangles at
/// different depths resolve per `SetOtherMode`'s `z_compare_en` /
/// `z_update_en` / `z_source_sel` bits and the staged `SetPrimDepth`. The
/// depth cells persist across every draw in one packet's schedule (owned by
/// `stage_color_commands`), so a later draw's fragment sees the depth an
/// earlier draw committed -- which is the whole point of the buffer.
///
/// **Compare is a strict less-than**, matching fn64's own documented RDP
/// convention on the GPU pipeline path (`targets::triangle_pipeline`: "the
/// RDP's z-buffer is a non-inclusive less-than compare op", `Less`), and the
/// zeroed-z-image seed makes that observable: with memory Z at 0 (the nearest
/// representable), no `z_compare_en` fragment is strictly nearer, so a
/// z-compared draw over a freshly-bound (unfilled) z-image draws nothing --
/// exactly what angrylion produces for the five `gen-zbuffer-*` compare cases.
pub struct RawTriangleDepth<'a> {
    /// The per-pixel depth accumulator, `target.extent().pixels()` long and
    /// indexed by `row.y * width + x` -- the same index the colour write
    /// uses.
    pub cells: &'a mut [DepthCell],
    /// `OtherMode`'s `Z_CMP`: gate the colour write on the depth compare.
    pub compare: bool,
    /// `OtherMode`'s `Z_UPD`: commit the passing fragment's Z to the cell.
    pub update: bool,
    /// `OtherMode`'s `ZMODE_*` (bits 10:11): selects the compare relation.
    pub mode: DepthMode,
    /// `OtherMode`'s `G_MDSFT_ZSRCSEL`: `true` = `G_ZS_PRIM` (fragment Z is
    /// the staged `SetPrimDepth`), `false` = `G_ZS_PIXEL` (fragment Z is the
    /// triangle's own depth coefficient block).
    pub source_is_primitive: bool,
    /// The staged `SetPrimDepth`, consulted only under `G_ZS_PRIM`.
    pub prim_depth: Option<PrimDepth>,
}

impl RawTriangleDepth<'_> {
    /// The fragment's 18-bit working Z for this draw.
    ///
    /// Under `G_ZS_PRIM` the RDP uses the primitive depth register: RT64 and
    /// the fn64 reference both take `(z & 0x7fff) << 3` (15-bit primitive z
    /// widened to the 18-bit working range). Under `G_ZS_PIXEL` the depth
    /// comes from the triangle's own coefficient block; the admitted subset's
    /// only `G_ZS_PIXEL` case carries a *flat* Z (all deltas zero), so the
    /// integer part of the coefficient's base Z is the whole-triangle
    /// fragment Z -- a deliberately narrow reading, documented as such,
    /// rather than a full per-pixel plane interpolation this corpus never
    /// exercises.
    fn fragment_z(&self, triangle: &RawTriangle) -> Option<(u32, u16)> {
        if self.source_is_primitive {
            let prim = self.prim_depth?;
            Some((u32::from(prim.z() & 0x7fff) << 3, prim.dz()))
        } else {
            let words = triangle.depth()?;
            // The depth block's first wire word carries the base Z in s15.16
            // (`w0` = z: high 16 integer, low 16 fraction); its integer part
            // is the working Z. The remaining fields (`w1` = dzdx, and the
            // second `RawWord`'s dzde/dzdy) are the per-pixel gradient,
            // unused for the flat-Z admitted case (DeltaZ 0).
            Some((words[0].w0() >> 16, 0))
        }
    }
}

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
pub fn execute_raw_triangle<'a, S: TmemByteSource + ?Sized>(
    candidate: &CandidateColorTarget,
    other_mode: OtherMode,
    triangle: &RawTriangle,
    shading: TexrectShading,
    blend_registers: TexrectBlendRegisters,
    resident_bytes: impl Into<Cow<'a, [u8]>>,
    declared: &[fn64_render_ir::ResourceAccess],
    texture: Option<RawTriangleTexture<'_, S>>,
    depth: Option<RawTriangleDepth<'_>>,
) -> Result<CompletedColorTargetWrite, TexrectExecutionError> {
    execute_raw_triangle_selected(
        candidate,
        other_mode,
        triangle,
        shading,
        blend_registers,
        resident_bytes,
        declared,
        texture,
        depth,
        FragmentProgramSelection::AdmitExact,
    )
}

#[derive(Clone, Copy)]
enum FragmentProgramSelection {
    AdmitExact,
    #[cfg(test)]
    GenericOracle,
    #[cfg(test)]
    FogNoiseGenericTerminalOracle,
}

#[allow(clippy::too_many_arguments)]
fn execute_raw_triangle_selected<'a, S: TmemByteSource + ?Sized>(
    candidate: &CandidateColorTarget,
    other_mode: OtherMode,
    triangle: &RawTriangle,
    shading: TexrectShading,
    blend_registers: TexrectBlendRegisters,
    resident_bytes: impl Into<Cow<'a, [u8]>>,
    declared: &[fn64_render_ir::ResourceAccess],
    texture: Option<RawTriangleTexture<'_, S>>,
    depth: Option<RawTriangleDepth<'_>>,
    fragment_selection: FragmentProgramSelection,
) -> Result<CompletedColorTargetWrite, TexrectExecutionError> {
    let mut bytes = resident_bytes.into().into_owned();
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
    if bytes.len() != full_len {
        return Err(TargetError::CompletedByteLengthMismatch {
            key,
            generation: candidate.generation(),
            expected: full_len,
            actual: bytes.len(),
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

    let draw_census_start = draw_census::enabled().then(std::time::Instant::now);
    let draw_census_key = draw_census_start.map(|_| draw_census::Key {
        combine_low: shading.combine().low(),
        combine_high: shading.combine().high(),
        other_mode_high: other_mode.high(),
        other_mode_low: other_mode.low(),
        evaluation: match evaluation {
            TexrectCombinerEvaluation::OneCycle => 1,
            TexrectCombinerEvaluation::TwoCycle => 2,
            TexrectCombinerEvaluation::BlitsTheTexel => 0,
        },
        format: match format {
            ColorTargetFormat::Rgba16 => 16,
            ColorTargetFormat::Rgba32 => 32,
        },
        textured,
        perspective: other_mode.texture_perspective(),
        depth: depth.is_some(),
    });
    let rasterized = raster_triangle(
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
        depth,
        fragment_selection,
    );
    rasterized?;
    if let (Some(start), Some(census_key)) = (draw_census_start, draw_census_key) {
        draw_census::note(
            census_key,
            rows.iter().map(|row| u64::from(row.x1 - row.x0)).sum(),
            start.elapsed(),
        );
    }

    let device_bytes = DeviceColorBytes::new_for_fill(key, candidate.generation(), format, bytes)?;
    Ok(CompletedColorTargetWrite::new_for_fill(
        key,
        candidate.generation(),
        key.range(),
        rectangle,
        device_bytes,
    ))
}

#[cfg(test)]
#[allow(clippy::too_many_arguments)]
fn execute_raw_triangle_generic_oracle<'a, S: TmemByteSource + ?Sized>(
    candidate: &CandidateColorTarget,
    other_mode: OtherMode,
    triangle: &RawTriangle,
    shading: TexrectShading,
    blend_registers: TexrectBlendRegisters,
    resident_bytes: impl Into<Cow<'a, [u8]>>,
    declared: &[fn64_render_ir::ResourceAccess],
    texture: Option<RawTriangleTexture<'_, S>>,
    depth: Option<RawTriangleDepth<'_>>,
) -> Result<CompletedColorTargetWrite, TexrectExecutionError> {
    execute_raw_triangle_selected(
        candidate,
        other_mode,
        triangle,
        shading,
        blend_registers,
        resident_bytes,
        declared,
        texture,
        depth,
        FragmentProgramSelection::GenericOracle,
    )
}

#[cfg(test)]
#[allow(clippy::too_many_arguments)]
fn execute_raw_triangle_fog_noise_generic_terminal_oracle<'a, S: TmemByteSource + ?Sized>(
    candidate: &CandidateColorTarget,
    other_mode: OtherMode,
    triangle: &RawTriangle,
    shading: TexrectShading,
    blend_registers: TexrectBlendRegisters,
    resident_bytes: impl Into<Cow<'a, [u8]>>,
    declared: &[fn64_render_ir::ResourceAccess],
    texture: Option<RawTriangleTexture<'_, S>>,
    depth: Option<RawTriangleDepth<'_>>,
) -> Result<CompletedColorTargetWrite, TexrectExecutionError> {
    execute_raw_triangle_selected(
        candidate,
        other_mode,
        triangle,
        shading,
        blend_registers,
        resident_bytes,
        declared,
        texture,
        depth,
        FragmentProgramSelection::FogNoiseGenericTerminalOracle,
    )
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
fn prepared_combiner_enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| match std::env::var("FN64_PREPARED_TRIANGLE_COMBINER") {
        Ok(value) if value == "0" => false,
        Ok(value) if value == "1" => true,
        Ok(value) => {
            panic!("FN64_PREPARED_TRIANGLE_COMBINER must be exactly 0 or 1, got {value:?}")
        }
        Err(std::env::VarError::NotPresent) => true,
        Err(error) => {
            panic!("FN64_PREPARED_TRIANGLE_COMBINER is not valid Unicode: {error}")
        }
    })
}

fn incremental_texture_planes_enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| match std::env::var("FN64_INCREMENTAL_TEXTURE_PLANES") {
        Ok(value) if value == "0" => false,
        Ok(value) if value == "1" => true,
        Ok(value) => {
            panic!("FN64_INCREMENTAL_TEXTURE_PLANES must be exactly 0 or 1, got {value:?}")
        }
        Err(std::env::VarError::NotPresent) => true,
        Err(error) => {
            panic!("FN64_INCREMENTAL_TEXTURE_PLANES is not valid Unicode: {error}")
        }
    })
}

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
    depth: Option<RawTriangleDepth<'_>>,
    fragment_selection: FragmentProgramSelection,
) -> Result<(), TexrectExecutionError> {
    const MIN_PARALLEL_PIXELS: u64 = 4_096;

    fn parallel_enabled() -> bool {
        static ENABLED: OnceLock<bool> = OnceLock::new();
        *ENABLED.get_or_init(|| {
            let enabled = match std::env::var("FN64_PARALLEL_RASTER") {
                Ok(value) if value == "0" => false,
                Ok(value) if value == "1" => true,
                Ok(value) => panic!(
                    "FN64_PARALLEL_RASTER must be exactly 0 or 1, got {value:?}"
                ),
                Err(std::env::VarError::NotPresent) => true,
                Err(error) => panic!("FN64_PARALLEL_RASTER is not valid Unicode: {error}"),
            };
            eprintln!(
                "[fn64-render-wgpu] FN64_PARALLEL_RASTER={} (scanline threshold {} covered-range pixels)",
                u8::from(enabled),
                MIN_PARALLEL_PIXELS,
            );
            enabled
        })
    }

    let fragment_program = select_fragment_program(
        format,
        triangle,
        shading,
        blend_state,
        evaluation,
        fragment_selection,
    );
    let incremental_texture_planes = incremental_texture_planes_enabled();

    let declared_pixels: u64 = rows.iter().map(|row| u64::from(row.x1 - row.x0)).sum();
    let contiguous_rows = rows
        .windows(2)
        .all(|pair| pair[0].y.checked_add(1) == Some(pair[1].y));
    let census_pixels = crate::combiner::census::enabled();
    if depth.is_none()
        && !census_pixels
        && contiguous_rows
        && declared_pixels >= MIN_PARALLEL_PIXELS
        && parallel_enabled()
    {
        let row_stride = width as usize * format.bytes_per_pixel() as usize;

        // `par_chunks_mut` is the ownership proof: each job exclusively owns
        // one whole color row, so no two jobs can address the same byte. The
        // explicit contiguity check above is what makes zipping the target
        // slice to `rows` exact even for degenerate triangles whose declared
        // row walk may contain holes; those stay scalar. The parallel
        // iterator's completion is the draw-order barrier before the next
        // triangle can observe or modify the target.
        let first_y = rows[0].y as usize;
        return bytes
            .par_chunks_mut(row_stride)
            .skip(first_y)
            .take(rows.len())
            .zip(rows.par_iter())
            .try_for_each(|(row_bytes, covered_row)| {
                let y = covered_row.y;
                let texture = texture.as_ref().map(|binding| RawTriangleTexture {
                    tile: binding.tile,
                    tmem: binding.tmem,
                    lut_mode: binding.lut_mode,
                });
                raster_triangle_scalar(
                    row_bytes,
                    format,
                    width,
                    triangle,
                    std::slice::from_ref(covered_row),
                    shading,
                    base_inputs,
                    shade,
                    texture_planes,
                    texture,
                    perspective,
                    blend_state,
                    stages,
                    evaluation,
                    fragment_program,
                    incremental_texture_planes,
                    None,
                    None,
                    None,
                    y,
                )
            });
    }

    raster_triangle_scalar(
        bytes,
        format,
        width,
        triangle,
        rows,
        shading,
        base_inputs,
        shade,
        texture_planes,
        texture,
        perspective,
        blend_state,
        stages,
        evaluation,
        fragment_program,
        incremental_texture_planes,
        None,
        None,
        depth,
        0,
    )
}

#[derive(Clone, Copy)]
enum RawTriangleFragmentProgram {
    Generic(Option<PreparedTwoCycleCombiner>),
    CoverageFogRgba16(CoverageFogRgba16Program),
    FogNoiseRgba16(FogNoiseRgba16Program),
    #[cfg(test)]
    FogNoiseRgba16GenericTerminal(FogNoiseRgba16Program),
}

fn select_fragment_program(
    format: ColorTargetFormat,
    triangle: &RawTriangle,
    shading: TexrectShading,
    blend_state: crate::BlendModeState,
    evaluation: TexrectCombinerEvaluation,
    selection: FragmentProgramSelection,
) -> RawTriangleFragmentProgram {
    let coverage_fog = CoverageFogRgba16Program::try_admit(
        format,
        shading.combine(),
        blend_state.other_mode,
        evaluation,
        triangle.flags().shaded(),
        triangle.flags().textured(),
    );
    let fog_noise = FogNoiseRgba16Program::try_admit(
        format,
        shading.combine(),
        blend_state.other_mode,
        evaluation,
        triangle.flags().shaded(),
        triangle.flags().textured(),
    );
    let generic = || {
        RawTriangleFragmentProgram::Generic(match evaluation {
            TexrectCombinerEvaluation::TwoCycle if prepared_combiner_enabled() => {
                Some(PreparedTwoCycleCombiner::new(shading.combine()))
            }
            _ => None,
        })
    };
    match selection {
        FragmentProgramSelection::AdmitExact => coverage_fog
            .map(RawTriangleFragmentProgram::CoverageFogRgba16)
            .or_else(|| fog_noise.map(RawTriangleFragmentProgram::FogNoiseRgba16))
            .unwrap_or_else(generic),
        #[cfg(test)]
        FragmentProgramSelection::GenericOracle => generic(),
        #[cfg(test)]
        FragmentProgramSelection::FogNoiseGenericTerminalOracle => fog_noise
            .map(RawTriangleFragmentProgram::FogNoiseRgba16GenericTerminal)
            .unwrap_or_else(generic),
    }
}

#[derive(Clone, Copy)]
struct CoverageFogRgba16Program;

impl CoverageFogRgba16Program {
    fn try_admit(
        format: ColorTargetFormat,
        combine: CombineParams,
        other_mode: OtherMode,
        evaluation: TexrectCombinerEvaluation,
        shaded: bool,
        textured: bool,
    ) -> Option<Self> {
        (format == ColorTargetFormat::Rgba16
            && combine.low() == 0xfc15_fea3
            && combine.high() == 0xf00f_f23f
            && matches!(other_mode.high(), 0x0018_ac8f | 0x0018_acff)
            && other_mode.low() == 0x0f0a_7008
            && evaluation == TexrectCombinerEvaluation::TwoCycle
            && shaded
            && textured)
            .then_some(Self)
    }
}

#[derive(Clone, Copy)]
struct FogNoiseRgba16Program;

impl FogNoiseRgba16Program {
    fn try_admit(
        format: ColorTargetFormat,
        combine: CombineParams,
        other_mode: OtherMode,
        evaluation: TexrectCombinerEvaluation,
        shaded: bool,
        textured: bool,
    ) -> Option<Self> {
        (format == ColorTargetFormat::Rgba16
            && combine.low() == 0xfc15_96a3
            && combine.high() == 0xf0ff_fe38
            && other_mode.high() == 0x0018_acef
            && other_mode.low() == 0x0050_4240
            && evaluation == TexrectCombinerEvaluation::TwoCycle
            && shaded
            && textured)
            .then_some(Self)
    }

    /// Closed terminal for this exact program. The admitted mode supplies a
    /// full fragment, performs no coverage-to-alpha or alpha compare, stores
    /// `CVG_DST_FULL`, and has disabled RGB dither. Its threshold-seven Noise
    /// alpha dither is exactly RGBA5551 quantization/expansion. Both blender
    /// cycles select `(Combined * CombinedAlpha) + (Framebuffer * (1-A))`;
    /// cycle two's Combined is cycle one's unchanged source, so the terminal
    /// is one source-over composite against the expanded resident RGB5.
    fn write_rgba16(self, dest: &mut [u8], combined: [u8; 4]) {
        let resident = u16::from_be_bytes([dest[0], dest[1]]);
        let expand_five = |five: u16| -> u32 {
            let five = u32::from(five);
            (five << 3) | (five >> 2)
        };
        let alpha_five = combined[3] >> 3;
        let alpha = u32::from((alpha_five << 3) | (alpha_five >> 2));
        let blend = |source: u8, destination_five: u16| -> u16 {
            let source = u32::from(source);
            let destination = expand_five(destination_five);
            ((source * alpha + destination * (255 - alpha) + 127) / 255) as u16
        };
        let red = blend(combined[0], (resident >> 11) & 0x1f);
        let green = blend(combined[1], (resident >> 6) & 0x1f);
        let blue = blend(combined[2], (resident >> 1) & 0x1f);
        let packed = ((red >> 3) << 11) | ((green >> 3) << 6) | ((blue >> 3) << 1) | 1;
        dest.copy_from_slice(&packed.to_be_bytes());
    }
}

/// Exact shared algebra of the closed `fc15fea3/f00ff23f` and
/// `fc1596a3/f0fffe38` programs. Both decode to `Texel0 * ShadeAlpha` for
/// cycle-zero RGB and carry `Texel0Alpha`; cycle one lerps that RGB toward
/// Environment by Primitive and multiplies alpha by PrimitiveAlpha. Every
/// operand is in `[0, 1]`, so the generic combiner's cross-cycle wrap and
/// final wrap-clamp stages are identities for both closed programs.
fn combine_fog_lerp(inputs: crate::CombinerInputs, texel: [u8; 4]) -> [u8; 4] {
    let texel = texel.map(|channel| f32::from(channel) / 255.0);
    let first_rgb = [
        texel[0] * inputs.shade_color[3],
        texel[1] * inputs.shade_color[3],
        texel[2] * inputs.shade_color[3],
    ];
    let combined = [
        (inputs.env_color[0] - first_rgb[0]) * inputs.prim_color[0] + first_rgb[0],
        (inputs.env_color[1] - first_rgb[1]) * inputs.prim_color[1] + first_rgb[1],
        (inputs.env_color[2] - first_rgb[2]) * inputs.prim_color[2] + first_rgb[2],
        texel[3] * inputs.prim_color[3],
    ];
    combined.map(|channel| (channel * 255.0).round() as u8)
}

/// Exact terminal stages for the same closed program on RGBA16.
/// `CVG_X_ALPHA` reduces full coverage from the quantized combined alpha;
/// zero coverage discards, while `CVG_DST_CLAMP` with image-read disabled
/// stores that fragment coverage directly. `ALPHA_CVG_SEL` and alpha dither
/// feed only alpha, which this program's blender does not select. Its RGB
/// output is Combined, so neither destination color nor blender arithmetic
/// is observable.
fn write_coverage_fog_rgba16(dest: &mut [u8], combined: [u8; 4]) {
    let coverage = ((8 * u16::from(combined[3]) + 127) / 255) as u8;
    if coverage == 0 {
        return;
    }
    let packed = (u16::from(combined[0] >> 3) << 11)
        | (u16::from(combined[1] >> 3) << 6)
        | (u16::from(combined[2] >> 3) << 1)
        | u16::from(((coverage - 1) >> 2) & 1);
    dest.copy_from_slice(&packed.to_be_bytes());
}

#[cfg(test)]
fn generic_coverage_fog_rgba16_oracle(
    other_mode: OtherMode,
    inputs: crate::CombinerInputs,
    texel: [u8; 4],
    registers: TexrectBlendRegisters,
    resident: [u8; 2],
    column: u32,
    row: u32,
) -> [u8; 2] {
    let combined = combine_one_texel_prepared_two_cycle(
        PreparedTwoCycleCombiner::new(CombineParams::from_wire(0xfc15_fea3, 0xf00f_f23f)),
        inputs,
        texel,
    );
    let state = registers.mode_state(other_mode);
    require_blendable_mode(state).unwrap();
    let stages = TexrectFragmentStages::try_new(other_mode, registers.blend_color()).unwrap();
    let mut output = resident;
    blend_and_write_pixel(
        ColorTargetFormat::Rgba16,
        &mut output,
        combined,
        state,
        stages,
        column,
        row,
    )
    .unwrap();
    output
}

#[allow(clippy::too_many_arguments)]
fn raster_triangle_scalar<S: TmemByteSource + ?Sized>(
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
    fragment_program: RawTriangleFragmentProgram,
    incremental_texture_planes: bool,
    prepared_sampler: Option<PreparedPointSampler>,
    mut exact_coverage: Option<&mut [u8]>,
    mut depth: Option<RawTriangleDepth<'_>>,
    base_y: u32,
) -> Result<(), TexrectExecutionError> {
    let mut bound_sampler = match (prepared_sampler, texture.as_ref()) {
        (Some(sampler), Some(binding)) => Some(sampler.bind(binding.tmem)),
        (None, _) => None,
        (Some(_), None) => {
            unreachable!("a prepared sampler is admitted only with a texture binding")
        }
    };
    // The fragment's whole-triangle Z and DeltaZ, resolved once per draw:
    // under `G_ZS_PRIM` from the staged `SetPrimDepth`, under `G_ZS_PIXEL`
    // from this triangle's own (flat, in the admitted subset) depth block.
    // `None` when no depth wiring is present at all, in which case the loop
    // below writes every covered pixel unconditionally -- the pre-z-buffer
    // painter's-order behaviour, unchanged for non-z draws.
    //
    // Resolved outside the loop deliberately: it is constant across the
    // whole draw here (flat prim/pixel Z), and a live lane measures frame
    // rate, so it stays out of the per-pixel path. A future per-pixel Z
    // plane would move this inside; the admitted subset has none.
    let fragment_depth: Option<(u32, u16)> = depth.as_ref().and_then(|d| d.fragment_z(triangle));
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
        let attribute_samples = triangle_span::AttributeSampleRow::new(triangle, row.y as i32);
        // **Incremental attribute run state.** Attribute planes are exactly
        // linear in x while the selected subsample holds, so a run can be
        // stepped by `plane.dx` instead of re-evaluating the full formula
        // (an i128 multiply and divide per plane, up to seven planes per
        // pixel). Reset per row: the first covered pixel of every row must
        // restart from the exact formula.
        let mut previous_sample: Option<(i32, i64)> = None;
        let mut shade_values: Option<[i64; 4]> = None;
        let mut texture_values: Option<[i64; 3]> = None;
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
            let sample = attribute_samples.sample(x as i32);
            let Some((delta_y_eighth, delta_x)) = sample else {
                // A hole in a declared row breaks the run: the next covered
                // pixel must restart from the exact formula.
                previous_sample = None;
                shade_values = None;
                texture_values = None;
                continue;
            };
            let primitive_coverage = exact_coverage.as_ref().map(|_| {
                attribute_samples
                    .coverage_mask(x as i32)
                    .expect("the selected attribute sample proves nonzero coverage")
                    .coverage()
            });
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
            texture_values = if incremental_texture_planes {
                match (texture_planes, texture_values, continues_run) {
                    (Some(planes), Some(values), true) => Some(std::array::from_fn(|component| {
                        triangle_span::attribute_plane_step(planes[component], values[component])
                    })),
                    (Some(planes), _, _) => Some(std::array::from_fn(|component| {
                        triangle_span::attribute_plane(planes[component], delta_y_eighth, delta_x)
                    })),
                    (None, _, _) => None,
                }
            } else {
                None
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
                    // Texture planes obey the same exact linear run as shade:
                    // restart from the full formula when the covered subsample
                    // changes, then advance by `dx` while it remains fixed.
                    // Keeping S/T/W beside `shade_values` avoids three wide
                    // multiply/divide evaluations per continuing pixel.
                    let stw = if incremental_texture_planes {
                        texture_values
                            .expect("a textured triangle carries incremental S/T/W values")
                    } else {
                        core::array::from_fn(|component| {
                            triangle_span::attribute_plane(
                                planes[component],
                                delta_y_eighth,
                                delta_x,
                            )
                        })
                    };
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
                    match bound_sampler.as_mut() {
                        Some(sampler) => sampler.sample(request),
                        None => sample_point(
                            binding.tmem,
                            binding.tile.descriptor(),
                            binding.tile.size(),
                            request,
                            binding.lut_mode,
                        ),
                    }
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
            // **The z-buffer decision, for a z-wired draw only.** For a
            // non-z draw (`depth` is `None`) this is skipped entirely and
            // every covered pixel writes, exactly as before. When wired, the
            // fragment's Z is compared against this pixel's depth cell under
            // the draw's `ZMODE`/`Z_CMP`, and on a pass the cell is updated
            // under `Z_UPD`. `fragment_depth` is `None` when the draw wants a
            // z source it has no value for (e.g. `G_ZS_PRIM` with no staged
            // `SetPrimDepth`); in that case the compare cannot be evaluated
            // and the draw falls back to painter's order rather than
            // fabricating a depth -- a documented, loud-by-absence fallback.
            let pixel = (row.y - base_y) as usize * width as usize + x as usize;
            let passes_depth = match (depth.as_ref(), fragment_depth) {
                (Some(d), Some((frag_z, frag_dz))) if d.compare => {
                    let (mem_z, mem_delta) = d.cells[pixel];
                    let relations = crate::depth_mode::relations(
                        frag_z.min(0x3ffff),
                        frag_dz,
                        mem_z,
                        mem_delta,
                    );
                    // **Strict less-than (`in_front`).** This matches fn64's
                    // own documented RDP convention on the GPU pipeline path
                    // (`targets::triangle_pipeline`: the depth test is a
                    // "non-inclusive less-than compare op", `Less`), and it
                    // is the relation angrylion produces on this corpus: with
                    // the zeroed-z-image seed the memory Z is 0 (the nearest
                    // representable), so no `Z_CMP` fragment is strictly
                    // nearer and a z-compared draw over a freshly-bound,
                    // unfilled z-image draws nothing -- exactly angrylion's
                    // output for the five compare cases. The four `ZMODE`
                    // relations (`mode_passes`) are deliberately NOT
                    // dispatched here: the admitted subset carries only
                    // `ZMODE_OPAQUE`, and picking a per-mode relation this
                    // corpus cannot exercise would be an unverified guess.
                    // `d.mode` is retained so a future ZMODE-bearing case
                    // widens this by name rather than silently.
                    let _ = d.mode;
                    relations.in_front
                }
                // No compare requested (or no fragment Z): the fragment is
                // admitted by depth and resolves by draw order.
                _ => true,
            };
            if !passes_depth {
                continue;
            }
            let offset =
                ((row.y - base_y) as usize * width as usize + x as usize) * bytes_per_pixel;
            if let Some(coverage) = exact_coverage.as_deref_mut() {
                let combined = match fragment_program {
                    RawTriangleFragmentProgram::CoverageFogRgba16(_)
                    | RawTriangleFragmentProgram::FogNoiseRgba16(_) => {
                        combine_fog_lerp(inputs, texel)
                    }
                    #[cfg(test)]
                    RawTriangleFragmentProgram::FogNoiseRgba16GenericTerminal(_) => {
                        combine_fog_lerp(inputs, texel)
                    }
                    RawTriangleFragmentProgram::Generic(prepared_two_cycle) => {
                        match prepared_two_cycle {
                            Some(prepared) => {
                                combine_one_texel_prepared_two_cycle(prepared, inputs, texel)
                            }
                            None => combine_one_texel(shading.combine(), inputs, texel, evaluation),
                        }
                    }
                };
                let destination = blend_and_write_pixel_with_coverage(
                    format,
                    &mut bytes[offset..offset + bytes_per_pixel],
                    combined,
                    blend_state,
                    stages,
                    x,
                    row.y,
                    primitive_coverage.expect("exact coverage destination resolved a mask"),
                    crate::Coverage::new(coverage[pixel]),
                    true,
                )?;
                coverage[pixel] = destination.count();
            } else {
                match fragment_program {
                    RawTriangleFragmentProgram::CoverageFogRgba16(_) => {
                        let combined = combine_fog_lerp(inputs, texel);
                        write_coverage_fog_rgba16(
                            &mut bytes[offset..offset + bytes_per_pixel],
                            combined,
                        );
                    }
                    RawTriangleFragmentProgram::FogNoiseRgba16(_) => {
                        let combined = combine_fog_lerp(inputs, texel);
                        FogNoiseRgba16Program
                            .write_rgba16(&mut bytes[offset..offset + bytes_per_pixel], combined);
                    }
                    #[cfg(test)]
                    RawTriangleFragmentProgram::FogNoiseRgba16GenericTerminal(_) => {
                        let combined = combine_fog_lerp(inputs, texel);
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
                    RawTriangleFragmentProgram::Generic(prepared_two_cycle) => {
                        let combined = match prepared_two_cycle {
                            Some(prepared) => {
                                combine_one_texel_prepared_two_cycle(prepared, inputs, texel)
                            }
                            None => combine_one_texel(shading.combine(), inputs, texel, evaluation),
                        };
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
            }
            // Commit depth AFTER the colour write, and only when `Z_UPD` is
            // set and this draw carries a fragment Z. The stored value is
            // quantized through the RDP's exponent/mantissa codec so a later
            // fragment compares against the same value the hardware would
            // have kept.
            if let (Some(d), Some((frag_z, frag_dz))) = (depth.as_mut(), fragment_depth) {
                if d.update {
                    let quantized = crate::depth_mode::decode_z(crate::depth_mode::encode_z(
                        frag_z.min(0x3ffff),
                    ));
                    d.cells[pixel] = (quantized, crate::depth_mode::encode_delta_z(frag_dz));
                }
            }
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

mod draw_census {
    use std::collections::BTreeMap;
    use std::sync::Mutex;
    use std::time::Duration;

    #[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
    pub(super) struct Key {
        pub combine_low: u32,
        pub combine_high: u32,
        pub other_mode_high: u32,
        pub other_mode_low: u32,
        pub evaluation: u8,
        pub format: u8,
        pub textured: bool,
        pub perspective: bool,
        pub depth: bool,
    }

    #[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
    struct Stats {
        draws: u64,
        pixels: u64,
        elapsed_ns: u128,
        max_draw_ns: u128,
    }

    impl Stats {
        fn note(&mut self, pixels: u64, elapsed: Duration) {
            self.draws += 1;
            self.pixels += pixels;
            self.elapsed_ns += elapsed.as_nanos();
            self.max_draw_ns = self.max_draw_ns.max(elapsed.as_nanos());
        }
    }

    static CENSUS: Mutex<Option<BTreeMap<Key, Stats>>> = Mutex::new(None);

    pub(super) fn enabled() -> bool {
        static ENABLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
        *ENABLED.get_or_init(|| std::env::var_os("FN64_DRAW_CENSUS").is_some())
    }

    pub(super) fn note(key: Key, pixels: u64, elapsed: Duration) {
        let mut guard = CENSUS.lock().expect("draw census mutex poisoned");
        let census = guard.get_or_insert_with(BTreeMap::new);
        census.entry(key).or_default().note(pixels, elapsed);
        let draws = census.values().map(|stats| stats.draws).sum::<u64>();
        if draws % 25_000 == 0 {
            report(draws, census);
        }
    }

    fn report(draws: u64, census: &BTreeMap<Key, Stats>) {
        let mut ranked = census.iter().collect::<Vec<_>>();
        ranked.sort_by(|(key_a, stats_a), (key_b, stats_b)| {
            stats_b
                .elapsed_ns
                .cmp(&stats_a.elapsed_ns)
                .then_with(|| key_a.cmp(key_b))
        });
        let total_ns = census.values().map(|stats| stats.elapsed_ns).sum::<u128>();
        let total_pixels = census.values().map(|stats| stats.pixels).sum::<u64>();
        eprintln!(
            "[fn64-draw-census] draws={draws} keys={} pixels={total_pixels} elapsed_ms={:.3}",
            census.len(),
            total_ns as f64 / 1_000_000.0,
        );
        for (rank, (key, stats)) in ranked.into_iter().take(12).enumerate() {
            let ns_per_pixel = if stats.pixels == 0 {
                0.0
            } else {
                stats.elapsed_ns as f64 / stats.pixels as f64
            };
            eprintln!(
                "[fn64-draw-census] rank={} combine={:#010x}/{:#010x} other={:#010x}/{:#010x} eval={} fmt={} textured={} perspective={} depth={} draws={} pixels={} elapsed_ms={:.3} max_draw_ms={:.3} ns_per_pixel={:.2}",
                rank + 1,
                key.combine_low,
                key.combine_high,
                key.other_mode_high,
                key.other_mode_low,
                key.evaluation,
                key.format,
                u8::from(key.textured),
                u8::from(key.perspective),
                u8::from(key.depth),
                stats.draws,
                stats.pixels,
                stats.elapsed_ns as f64 / 1_000_000.0,
                stats.max_draw_ns as f64 / 1_000_000.0,
                ns_per_pixel,
            );
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn aggregation_tracks_elapsed_pixels_draws_and_maximum() {
            let slow = Key {
                combine_low: 1,
                combine_high: 2,
                other_mode_high: 3,
                other_mode_low: 4,
                evaluation: 2,
                format: 16,
                textured: true,
                perspective: true,
                depth: false,
            };
            let mut stats = Stats::default();
            stats.note(20, Duration::from_micros(10));
            stats.note(30, Duration::from_micros(40));
            assert_eq!(stats.draws, 2);
            assert_eq!(stats.pixels, 50);
            assert_eq!(stats.elapsed_ns, 50_000);
            assert_eq!(stats.max_draw_ns, 40_000);
            assert_eq!(slow.evaluation, 2);
        }
    }
}

#[cfg(test)]
mod tests;
