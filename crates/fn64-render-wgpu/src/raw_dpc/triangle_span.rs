//! The covered-pixel geometry of one raw RDP triangle, derived from its own
//! edge coefficients.
//!
//! **One derivation, two consumers.** The decoder calls
//! [`covered_rows`] to declare per-row `ResourceAccess` runs into the
//! journal; the CPU rasterizer calls [`row_span`] and
//! [`pixel_coverage`] to produce the bytes those declarations promise. If
//! these were two derivations, `fill_completed_writes` -- which slices the
//! full-extent buffer for every declared range *without* checking the
//! raster touched it -- would happily digest stale bytes for any row the
//! decoder declared and the raster skipped. That passes `validate_effects`
//! and reaches guest RDRAM. Convincing garbage. So the decoder's rows and
//! the raster's rows are the same function, called twice.
//!
//! # The `lft` bit (wire bit 23) is LEFT-major, not right-major
//!
//! [`crate::raw_dpc::triangle::RawTriangle::right_major`] names wire bit 23
//! "right-major (flip)". `fn64-render-reference` reads the identical bit and
//! names it `left_major` (`gbi/entries.rs:596`). The two names are opposite
//! and only one can be right.
//!
//! The reference's reading is the one backed by evidence, and it is evidence
//! from this project's own target ROM. Its
//! `real_stream_left_major_rect_split_triangle_rasterizes_interior`
//! (`raster/tests/group2.rs:1336`) carries byte-exact coefficients captured
//! from WM2000's live title-scene XBUS stream: `lft`=1 with a *constant* XH
//! (`dxhdy == 0`, `xh == 770048` = 11.75px) and XM marching right
//! (`dxmdy == 272435` = +4.157px/line). For that triangle the major (H) edge
//! is unambiguously the LEFT side -- it is the smaller X, and the minor edge
//! grows away from it. So `lft == 1` means the H edge is on the left.
//!
//! RT64 is not an authority here: it is an HLE renderer that never decodes
//! raw RDP edge coefficients at all (its `drawRect` `flip` parameter is the
//! TEXRECT flip bit, a different field), so the pinned oracle checkout
//! contains no ground truth for this bit.
//!
//! This module therefore reads the accessor as left-major and says so at
//! every call site, rather than renaming the decoder's public accessor:
//! renaming a `pub` accessor is a separate, wider change, and
//! [`left_major`] here is the one place the polarity is decided.
//! `the_wm2000_title_triangle_spans_run_major_left_to_minor_right` pins it
//! against those same live-stream bytes, driven through the real decoder.

use super::triangle::RawTriangle;
use crate::CoverageMask;

/// One pixel in Q16.16, the fixed-point format every X edge coefficient
/// arrives in.
pub(crate) const Q16_ONE: i64 = 1 << 16;

/// The RDP's eight subpixel coverage samples, in eighth-pixel
/// `(x_eighth, y_eighth)` units.
///
/// **A checkerboard, not a 2x4 grid.** The X columns ALTERNATE by row:
/// (1,5) on Y rows 1 and 5, (3,7) on Y rows 3 and 7. This is
/// `crate::COVERAGE_SAMPLES` verbatim, which is itself
/// `fn64-render-reference::raster::coverage::COVERAGE_SAMPLES`
/// index-for-index.
///
/// Taking the columns as a constant (1,5) on every row -- which the first
/// draft of this module did -- shifts half of every edge pixel's coverage by
/// a quarter pixel, and shifts the attribute sample point with it.
pub(crate) const COVERAGE_SAMPLES: [(i32, i32); 8] = crate::COVERAGE_SAMPLES;

/// The four subpixel Y sample offsets, in eighths of a pixel, that the RDP
/// evaluates per scanline. Sample centers sit on odd eighths.
pub(crate) const SAMPLE_Y_EIGHTHS: [i32; 4] = [1, 3, 5, 7];

/// The two X sample columns the RDP checks on the scanline whose Y offset is
/// `y_eighth` -- (1, 5) on rows 1 and 5, (3, 7) on rows 3 and 7.
///
/// Derived from [`COVERAGE_SAMPLES`] rather than written out, so the two
/// cannot drift.
fn sample_x_eighths(y_eighth: i32) -> [i32; 2] {
    let mut columns = [0i32; 2];
    let mut found = 0;
    let mut index = 0;
    while index < COVERAGE_SAMPLES.len() {
        let (x, y) = COVERAGE_SAMPLES[index];
        if y == y_eighth && found < 2 {
            columns[found] = x;
            found += 1;
        }
        index += 1;
    }
    debug_assert!(found == 2, "every Y sample row has exactly two X columns");
    columns
}

/// `value * numerator / denominator`, evaluated in `i128` and floored, so a
/// full-range Q16.16 slope multiplied by a subpixel delta cannot overflow
/// on the way to its (small) result.
///
/// Floored rather than truncated: `div_euclid` rounds toward negative
/// infinity for negative numerators, which is what walking an edge downward
/// past X=0 requires. A truncating `/` would round those toward zero and
/// shift the left edge of every triangle straddling X=0 by one subpixel.
pub(crate) fn fixed_mul_ratio(value: i32, numerator: i64, denominator: i64) -> i64 {
    i64::try_from((i128::from(value) * i128::from(numerator)).div_euclid(i128::from(denominator)))
        .expect("a Q16.16 slope times a subpixel delta fits i64")
}

/// `ceil(numerator / denominator)` for a positive `denominator`, exact for
/// negative numerators (which `/`'s truncation gets wrong).
pub(crate) fn ceil_ratio(numerator: i64, denominator: i64) -> i64 {
    -(-numerator).div_euclid(denominator)
}

/// Whether wire bit 23 says the major (H) edge walks the triangle's LEFT
/// side.
///
/// The decoder's accessor is named `right_major`; see this module's own doc
/// for why that name is inverted and why the live-stream evidence names the
/// set state left-major. Reading it through this one function means the
/// polarity is decided in exactly one place.
pub(crate) const fn left_major(triangle: &RawTriangle) -> bool {
    triangle.right_major()
}

/// The two X edges, in Q16.16, at one subpixel Y sample line -- returned as
/// `(left, right)` in screen order.
///
/// Y arrives in S11.2 (quarter pixels); `sample_y_eighth` is in eighths, so
/// every S11.2 field is doubled to meet it. XH and XM are both evaluated
/// from the scanline preceding YH (`yh & !3`, the RDP's own truncation to a
/// whole scanline); XL is evaluated from YM.
pub(crate) fn row_span(triangle: &RawTriangle, sample_y_eighth: i32) -> (i64, i64) {
    let high_origin_eighth = i32::from(triangle.yh() & !3) * 2;
    let middle_eighth = i32::from(triangle.ym()) * 2;
    let major_x = i64::from(triangle.xh())
        + fixed_mul_ratio(
            triangle.dxhdy(),
            i64::from(sample_y_eighth - high_origin_eighth),
            8,
        );
    let minor_x = if sample_y_eighth < middle_eighth {
        i64::from(triangle.xm())
            + fixed_mul_ratio(
                triangle.dxmdy(),
                i64::from(sample_y_eighth - high_origin_eighth),
                8,
            )
    } else {
        i64::from(triangle.xl())
            + fixed_mul_ratio(
                triangle.dxldy(),
                i64::from(sample_y_eighth - middle_eighth),
                8,
            )
    };
    if left_major(triangle) {
        (major_x, minor_x)
    } else {
        (minor_x, major_x)
    }
}

/// One scanline's covered pixel range, `[x0, x1)`, clamped to
/// `[0, clamp_width)`; `None` when this scanline is entirely outside the
/// triangle.
///
/// The four subpixel sample lines are each tested for being inside
/// `[yh, yl)` before contributing, so a scanline the triangle only partly
/// covers vertically reports the union of the sample lines it actually
/// reaches -- never a range derived from a sample line above YH or at/below
/// YL.
pub(crate) fn row_pixel_range(
    triangle: &RawTriangle,
    y: i32,
    clamp_width: u32,
) -> Option<(u32, u32)> {
    let yh_eighth = i32::from(triangle.yh()) * 2;
    let yl_eighth = i32::from(triangle.yl()) * 2;
    let mut min_left = i64::MAX;
    let mut max_right = i64::MIN;
    for offset_y in SAMPLE_Y_EIGHTHS {
        let row_y_eighth = y * 8 + offset_y;
        if row_y_eighth < yh_eighth || row_y_eighth >= yl_eighth {
            continue;
        }
        let (left_x, right_x) = row_span(triangle, row_y_eighth);
        // `>=`, not `>`: a sample line whose left and right edges COINCIDE
        // still contributes.
        //
        // The argument here is internal consistency, not an external
        // rasterizer. This function answers "which pixels does the span
        // touch"; `row_span` returns a half-open `[left, right)` in Q16.16
        // subpixel units, and the caller converts that to whole pixels by
        // the subpixel-sample rule below (`- 7/8` and `- 1/8`). A span whose
        // Q16.16 width is zero can still round to a covered pixel under that
        // rule, so discarding it BEFORE the conversion decides coverage with
        // the wrong resolution -- it applies a whole-pixel test to a
        // subpixel quantity. The `x1 <= x0` check after the conversion is
        // where a genuinely empty range is rejected, and that check remains.
        //
        // Requiring strictly positive subpixel width dropped the WHOLE
        // triangle whenever every sample line produced a sliver, which is
        // what distant and edge-on geometry produces.
        //
        // **Not independently confirmed against an allowed hardware
        // reference.** RT64 rasterizes on the GPU and has no span-loop
        // equivalent to compare against; n64-systemtest coverage for
        // degenerate spans has not been checked. Treat the rule as fn64's
        // own reasoning until one of those settles it.
        if right_x >= left_x {
            min_left = min_left.min(left_x);
            max_right = max_right.max(right_x);
        }
    }
    if min_left == i64::MAX || max_right == i64::MIN {
        return None;
    }
    // The `- 7/8` and `- 1/8` are the leftmost and rightmost subpixel sample
    // columns: a pixel is entered as soon as its LAST sample column (x + 7/8)
    // reaches the left edge, and left as soon as its FIRST (x + 1/8) passes
    // the right edge.
    let x0 =
        ceil_ratio(min_left - 7 * Q16_ONE / 8, Q16_ONE).clamp(0, i64::from(clamp_width)) as u32;
    let x1 = ceil_ratio(max_right - Q16_ONE / 8, Q16_ONE).clamp(0, i64::from(clamp_width)) as u32;
    // A range that rounds to zero width still covers its pixel, for the same
    // reason as above -- but `[x0, x0)` is empty as a half-open range, so
    // widen it to one pixel rather than reporting nothing. Only a range
    // clamped entirely off the right edge (`x0 == clamp_width`) genuinely
    // covers nothing.
    if x1 <= x0 {
        if x0 >= clamp_width {
            return None;
        }
        return Some((x0, x0 + 1));
    }
    Some((x0, x1))
}

/// The scanline range `[y0, y1)` this triangle can cover, clamped to
/// `[0, clamp_height)`.
///
/// The `-7`/`-1` are the same first/last subpixel sample rows the X range
/// uses, transposed: a scanline is entered when its last sample row reaches
/// YH and left when its first sample row passes YL.
pub(crate) fn row_range(triangle: &RawTriangle, clamp_height: u32) -> (i32, i32) {
    let yh_eighth = i32::from(triangle.yh()) * 2;
    let yl_eighth = i32::from(triangle.yl()) * 2;
    let height = i64::from(clamp_height);
    let min_y = ceil_ratio(i64::from(yh_eighth - 7), 8).clamp(0, height) as i32;
    let max_y = ceil_ratio(i64::from(yl_eighth - 1), 8).clamp(0, height) as i32;
    (min_y, max_y.max(min_y))
}

/// One covered scanline: its Y, and its `[x0, x1)` pixel range.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct CoveredRow {
    pub(crate) y: u32,
    pub(crate) x0: u32,
    pub(crate) x1: u32,
}

/// Every scanline this triangle covers, in increasing Y, each with the
/// exact `[x0, x1)` the rasterizer will walk.
///
/// **This is the decoder's declaration and the raster's loop bounds, from
/// one call.** A row absent from this list is a row the journal must not
/// declare; a row present is a row the raster must visit.
pub(crate) fn covered_rows(triangle: &RawTriangle, width: u32, height: u32) -> Vec<CoveredRow> {
    let (min_y, max_y) = row_range(triangle, height);
    (min_y..max_y)
        .filter_map(|y| {
            let (x0, x1) = row_pixel_range(triangle, y, width)?;
            Some(CoveredRow {
                y: y as u32,
                x0,
                x1,
            })
        })
        .collect()
}

/// The subpixel coverage count (0..=8) of one pixel: how many of the RDP's
/// eight [`COVERAGE_SAMPLES`] fall inside the triangle. The X columns
/// alternate by Y row -- it is a checkerboard, not a grid.
///
/// Test oracle for the row-hoisted rasterizer coverage path.
#[cfg(test)]
pub(crate) fn pixel_coverage(triangle: &RawTriangle, x: i32, y: i32) -> u32 {
    let yh_eighth = i32::from(triangle.yh()) * 2;
    let yl_eighth = i32::from(triangle.yl()) * 2;
    let mut count = 0;
    for offset_y in SAMPLE_Y_EIGHTHS {
        let sample_y_eighth = y * 8 + offset_y;
        if sample_y_eighth < yh_eighth || sample_y_eighth >= yl_eighth {
            continue;
        }
        let (left_x, right_x) = row_span(triangle, sample_y_eighth);
        for offset_x in sample_x_eighths(offset_y) {
            let sample_x = (i64::from(x) * 8 + i64::from(offset_x)) * Q16_ONE / 8;
            if sample_x >= left_x && sample_x < right_x {
                count += 1;
            }
        }
    }
    count
}

// ---------------------------------------------------------------------------
// Attribute planes (shade, and later texture)
// ---------------------------------------------------------------------------

/// One attribute's four Q16.16 plane coefficients, in the RDP's own order:
/// the value at the triangle's origin, its X derivative, its along-edge
/// derivative, and its Y derivative.
///
/// `dcdy` is decoded and carried but not evaluated by [`attribute_plane`]:
/// the RDP's own plane evaluation is `base + de*dy + dx*dxpos`, walking the
/// major edge with `de` and then stepping across the span with `dx`. `dcdy`
/// is not a third term of THAT walk.
///
/// Whether it is truly unused on real hardware is an OPEN QUESTION. A
/// sub-pixel correction applied only to partially covered pixels would be
/// consistent with `dcdy` existing in the wire format while contributing
/// nothing to the interior walk -- but fn64 has no allowed reference that
/// settles it. RT64 does not model such a correction, and it is this
/// crate's render parity target.
///
/// **Deliberately not implemented here.** RT64 is this crate's render parity
/// target and does not model it either: it is a GPU rasterizer taking
/// coverage from hardware MSAA rather than from the RDP's coverage LUT, and
/// has no `offx`/`offy` equivalent. Adding the term would diverge from the
/// oracle we certify against in order to approach a third implementation.
/// Revisit if an allowed reference ever settles the partial-coverage rule.
///
/// Carried rather than dropped so the decode is complete and a future
/// consumer does not have to re-read the wire.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct AttributePlane {
    pub(crate) base: i32,
    pub(crate) dx: i32,
    pub(crate) de: i32,
    pub(crate) dy: i32,
}

/// The RDP-latched interpolation origin for one integer scanline.
///
/// Raw coefficients retain bits the span interpolator does not. Those bits
/// are discarded here, once, before either shade or texture walks the row.
/// Keeping the latch as a type prevents a caller from accidentally feeding
/// the decoded Q16.16 plane directly to a continuous geometric evaluator.
///
/// The masks and latch order follow paraLLEl-RDP's MIT-licensed
/// `span_setup.comp`/`interpolation.h`: row values clear bits 0..9 after the
/// X-fraction correction, and X steps clear bits 0..4. `do_offset` is derived
/// by `decode_triangle_setup` as `flip == sign(dxhdy)`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct AttributeSpanRow {
    base_x: i32,
    y_delta: i32,
    x_fraction: i32,
    do_offset: bool,
}

impl AttributeSpanRow {
    pub(crate) fn new(triangle: &RawTriangle, y: i32) -> Self {
        let y_delta = y.wrapping_sub(i32::from(triangle.yh()) >> 2);

        // paraLLEl stores XH at half the wire precision and dxhdy after its
        // three ignored low bits have been discarded. Staying in those half
        // units through the latch reproduces its signed shifts exactly.
        let mut xh_half =
            (triangle.xh() >> 1).wrapping_add(y_delta.wrapping_mul((triangle.dxhdy() >> 3) << 2));
        let do_offset = triangle.right_major() == (triangle.dxhdy() < 0);
        if do_offset {
            xh_half = xh_half.wrapping_add(3i32.wrapping_mul(triangle.dxhdy() >> 3));
        }

        Self {
            base_x: xh_half >> 15,
            y_delta,
            x_fraction: (xh_half >> 7) & 0xff,
            do_offset,
        }
    }

    pub(crate) fn interpolate(self, plane: AttributePlane, x: i32) -> i64 {
        let mut derivative_correction = 0i32;
        if self.do_offset {
            let de_high = plane.de & !0x1ff;
            let dy_high = plane.dy & !0x1ff;
            derivative_correction = de_high
                .wrapping_sub(de_high >> 2)
                .wrapping_sub(dy_high)
                .wrapping_add(dy_high >> 2);
        }

        let x_fraction_correction = self.x_fraction.wrapping_mul((plane.dx >> 8) & !1);
        let row_base = plane.base.wrapping_add(plane.de.wrapping_mul(self.y_delta));
        let latched = (row_base & !0x1ff)
            .wrapping_add(derivative_correction)
            .wrapping_sub(x_fraction_correction)
            & !0x3ff;
        let x_step = (plane.dx & !0x1f).wrapping_mul(x.wrapping_sub(self.base_x));
        i64::from(latched.wrapping_add(x_step))
    }

    #[inline]
    pub(crate) fn step(plane: AttributePlane, value: i64) -> i64 {
        i64::from((value as i32).wrapping_add(plane.dx & !0x1f))
    }

    /// Completes the RDP shade centroid operation from an integer-pixel span
    /// latch and the first set checkerboard coverage bit.
    pub(crate) fn shade_component(
        plane: AttributePlane,
        span_value: i64,
        first_coverage_bit: u32,
    ) -> u8 {
        assert!(first_coverage_bit < 8);
        let y_offset = (first_coverage_bit >> 1) as i16;
        let x_offset = (((first_coverage_bit & 1) << 1) + (u32::from(y_offset as u16) & 1)) as i16;
        let mut snapped = ((span_value as i32) >> 14) as i16;
        snapped = snapped.wrapping_shl(2);
        snapped = snapped.wrapping_add(x_offset.wrapping_mul((plane.dx >> 14) as i16));
        snapped = snapped.wrapping_add(y_offset.wrapping_mul((plane.dy >> 14) as i16));
        let value = i32::from(snapped >> 4);

        // paraLLEl-RDP's MIT-licensed clamp_9bit: sign-extend bits 0..8
        // around a 0x80 bias, then clamp into the combiner's u8 domain.
        let biased = value.wrapping_sub(0x80);
        let signed_nine = (biased << 23) >> 23;
        signed_nine.wrapping_add(0x80).clamp(0, 255) as u8
    }
}

/// Reassembles one Q16.16 coefficient from the split integer and fraction
/// half-words the RDP's coefficient blocks store them in.
///
/// The blocks are NOT eight consecutive Q16.16 values: the RDP stores every
/// coefficient's high 16 bits in the block's first half and its low 16 bits
/// 16 bytes later, so a naive `w0 as i32` read would produce a value with
/// the fraction of a completely different component.
const fn fixed_16_16(integer: u32, fraction: u32) -> i32 {
    ((integer as u16 as i16 as i32) << 16) | (fraction as u16 as i32)
}

/// Decodes the four RGBA shade planes from a triangle's eight shade
/// coefficient words.
///
/// Wire layout, from the RDP command summary's shade block and matched
/// against `fn64-render-reference`'s `decode_rdp_shade_coefficients`
/// (`gbi/entries.rs:623`): the block is 64 bytes = 8 wire words = 16 u32
/// halves, and each attribute occupies a (integer_offset, fraction_offset)
/// byte pair 16 bytes apart:
///   colour  (0, 16)   d/dx (8, 24)   d/de (32, 48)   d/dy (40, 56)
/// Within a pair, the first u32 holds R in its high half and G in its low
/// half; the second holds B and A the same way.
pub(crate) fn shade_planes(words: &super::triangle::CoefficientWords) -> [AttributePlane; 4] {
    let colour = coefficient_components(words, 0, 16);
    let dcdx = coefficient_components(words, 8, 24);
    let dcde = coefficient_components(words, 32, 48);
    let dcdy = coefficient_components(words, 40, 56);
    core::array::from_fn(|component| AttributePlane {
        base: colour[component],
        dx: dcdx[component],
        de: dcde[component],
        dy: dcdy[component],
    })
}

/// Evaluates one attribute plane at a sample point, in Q16.16.
///
/// `edge_delta_y_eighth` is the sample's Y distance below the triangle's own
/// high origin, in eighths of a scanline; `edge_delta_x_q16` is its X
/// distance from the MAJOR edge at that Y, in Q16.16.
///
/// The RDP walks the major edge with `de` and then steps across the span
/// with `dx`, so the X term is measured from the major edge rather than from
/// x=0. Measuring it from x=0 would add `dx * major_x`, which for a triangle
/// far from the left of the screen is an enormous colour offset.
pub(crate) fn attribute_plane(
    plane: AttributePlane,
    edge_delta_y_eighth: i32,
    edge_delta_x_q16: i64,
) -> i64 {
    let x_term = i64::try_from(
        (i128::from(plane.dx) * i128::from(edge_delta_x_q16)).div_euclid(i128::from(Q16_ONE)),
    )
    .expect("a Q16.16 attribute slope times a Q16.16 X delta fits i64");
    i64::from(plane.base)
        .checked_add(fixed_mul_ratio(plane.de, i64::from(edge_delta_y_eighth), 8))
        .and_then(|value| value.checked_add(x_term))
        .expect("attribute plane evaluation fits i64")
}

/// Advance an already-evaluated attribute plane by exactly one pixel in X.
///
/// **Bit-identical to [`attribute_plane`], but only while the selected
/// subsample is unchanged.** In that case `edge_delta_y_eighth` is unchanged
/// and `edge_delta_x_q16` grows by exactly [`Q16_ONE`], so for integer
/// `plane.dx` and positive `Q16_ONE`:
///
/// ```text
/// floor(dx * (delta_x + Q16_ONE) / Q16_ONE) == floor(dx * delta_x / Q16_ONE) + dx
/// ```
///
/// `attribute_plane` uses `div_euclid` with a positive divisor, so the
/// identity holds for negative slopes and negative deltas alike -- verified
/// over 200,000 random (dx, delta_x) pairs spanning both signs before this
/// was written.
///
/// **The caller MUST restart from [`attribute_plane`] whenever either sample
/// delta breaks that relationship.** A changed Y subsample moves both the
/// `de` term and the major-edge origin; a changed X subsample, or a skipped
/// pixel, advances `delta_x` by something other than `Q16_ONE`. Stepping
/// across either is not an approximation -- it is simply wrong.
#[inline]
pub(crate) fn attribute_plane_step(plane: AttributePlane, value: i64) -> i64 {
    value
        .checked_add(i64::from(plane.dx))
        .expect("attribute plane evaluation fits i64")
}

/// The X position of the MAJOR edge at one subpixel Y sample line, in
/// Q16.16 -- the origin every attribute plane's X term is measured from.
///
/// Always the H edge, regardless of which SIDE of the span it is on: `lft`
/// decides where the major edge sits on screen, not which edge the RDP
/// walks its attribute planes along.
pub(crate) fn major_edge_x(triangle: &RawTriangle, sample_y_eighth: i32) -> i64 {
    let high_origin_eighth = i32::from(triangle.yh() & !3) * 2;
    i64::from(triangle.xh())
        + fixed_mul_ratio(
            triangle.dxhdy(),
            i64::from(sample_y_eighth - high_origin_eighth),
            8,
        )
}

/// The sample point one covered pixel's attributes are evaluated at, as
/// `(edge_delta_y_eighth, edge_delta_x_q16)` -- ready for
/// [`attribute_plane`].
///
/// The RDP evaluates a pixel's attributes at one of its covered subsamples,
/// not at the pixel centre. This picks the first covered subsample in the
/// RDP's own scan order -- Y rows 1,3,5,7 eighths, and on each the two X
/// columns [`COVERAGE_SAMPLES`] gives that row -- and returns `None` when
/// the pixel has no covered subsample at all.
#[derive(Clone, Copy)]
struct AttributeSampleLine {
    delta_y_eighth: i32,
    left_x: i64,
    right_x: i64,
    major_x: i64,
    sample_x_eighths: [i32; 2],
    covered_x: [(i32, i32); 2],
}

fn ceil_q16(value: i64) -> i32 {
    let quotient = value.div_euclid(Q16_ONE);
    let rounded = quotient + i64::from(value.rem_euclid(Q16_ONE) != 0);
    i32::try_from(rounded).expect("a triangle sample x bound fits i32")
}

/// The four edge pairs and major-edge origins shared by every pixel in one
/// scanline. Edge positions vary with subpixel Y, but never with X; resolving
/// them once here avoids repeating the fixed-point edge divisions for every
/// pixel while preserving [`attribute_sample`]'s exact traversal order.
pub(crate) struct AttributeSampleRow {
    lines: [Option<AttributeSampleLine>; 4],
}

impl AttributeSampleRow {
    pub(crate) fn new(triangle: &RawTriangle, y: i32) -> Self {
        let yh_eighth = i32::from(triangle.yh()) * 2;
        let yl_eighth = i32::from(triangle.yl()) * 2;
        let high_origin_eighth = i32::from(triangle.yh() & !3) * 2;
        let lines = core::array::from_fn(|index| {
            let offset_y = SAMPLE_Y_EIGHTHS[index];
            let sample_y_eighth = y * 8 + offset_y;
            if sample_y_eighth < yh_eighth || sample_y_eighth >= yl_eighth {
                return None;
            }
            let (left_x, right_x) = row_span(triangle, sample_y_eighth);
            let sample_x_eighths = sample_x_eighths(offset_y);
            let covered_x = sample_x_eighths.map(|offset_x| {
                let offset_q16 = i64::from(offset_x) * Q16_ONE / 8;
                (
                    ceil_q16(left_x - offset_q16),
                    ceil_q16(right_x - offset_q16),
                )
            });
            Some(AttributeSampleLine {
                delta_y_eighth: sample_y_eighth - high_origin_eighth,
                left_x,
                right_x,
                major_x: major_edge_x(triangle, sample_y_eighth),
                sample_x_eighths,
                covered_x,
            })
        });
        Self { lines }
    }

    pub(crate) fn sample(&self, x: i32) -> Option<(i32, i64)> {
        self.coverage_and_sample(x).map(|(_, sample)| sample)
    }

    pub(crate) fn coverage_mask(&self, x: i32) -> Option<CoverageMask> {
        let mut mask = 0u8;
        for (line_index, line) in self.lines.iter().enumerate() {
            let Some(line) = line else { continue };
            for (column_index, (start, end)) in line.covered_x.into_iter().enumerate() {
                if x >= start && x < end {
                    mask |= 1 << (line_index * 2 + column_index);
                }
            }
        }
        (mask != 0).then(|| CoverageMask::from_bits(mask))
    }

    /// Counts all eight checkerboard samples while retaining the first
    /// covered sample used by shade interpolation. One row walk supplies
    /// both facts so exact primitive coverage does not double edge tests.
    pub(crate) fn coverage_and_sample(&self, x: i32) -> Option<(CoverageMask, (i32, i64))> {
        let mask = self.coverage_mask(x)?;
        let first_bit = mask.0.trailing_zeros() as usize;
        let line = self.lines[first_bit / 2]
            .as_ref()
            .expect("a covered bit names a present sample line");
        let offset_x = line.sample_x_eighths[first_bit & 1];
        let sample_x = i64::from(x) * Q16_ONE + i64::from(offset_x) * Q16_ONE / 8;
        Some((mask, (line.delta_y_eighth, sample_x - line.major_x)))
    }
}

#[cfg(test)]
pub(crate) fn attribute_sample(triangle: &RawTriangle, x: i32, y: i32) -> Option<(i32, i64)> {
    AttributeSampleRow::new(triangle, y).sample(x)
}

#[cfg(test)]
fn attribute_sample_unhoisted(triangle: &RawTriangle, x: i32, y: i32) -> Option<(i32, i64)> {
    let yh_eighth = i32::from(triangle.yh()) * 2;
    let yl_eighth = i32::from(triangle.yl()) * 2;
    let high_origin_eighth = i32::from(triangle.yh() & !3) * 2;
    for offset_y in SAMPLE_Y_EIGHTHS {
        let sample_y_eighth = y * 8 + offset_y;
        if sample_y_eighth < yh_eighth || sample_y_eighth >= yl_eighth {
            continue;
        }
        let (left_x, right_x) = row_span(triangle, sample_y_eighth);
        for offset_x in sample_x_eighths(offset_y) {
            let sample_x = (i64::from(x) * 8 + i64::from(offset_x)) * Q16_ONE / 8;
            if sample_x >= left_x && sample_x < right_x {
                return Some((
                    sample_y_eighth - high_origin_eighth,
                    sample_x - major_edge_x(triangle, sample_y_eighth),
                ));
            }
        }
    }
    None
}

/// Decodes the three S/T/W planes from a triangle's eight texture
/// coefficient words.
///
/// **The same wire layout as [`shade_planes`]**, and deliberately the same
/// code path: the RDP's shade and texture coefficient blocks are both 64
/// bytes with each attribute at a (integer, fraction) byte pair 16 bytes
/// apart --
///   value (0, 16)   d/dx (8, 24)   d/de (32, 48)   d/dy (40, 56)
/// -- and differ only in how many components they carry. Shade takes four
/// (R,G,B,A) from two u32s per pair; texture takes three (S,T,W), with S in
/// the first u32's high half, T in its low half, and W in the second u32's
/// high half. The second u32's low half is unused by the texture block.
///
/// Written as one shared `coefficient_components` rather than a near-copy,
/// because a second transcription of a split-fixed-point layout is exactly
/// where a fraction gets paired with the wrong component's integer.
pub(crate) fn texture_planes(words: &super::triangle::CoefficientWords) -> [AttributePlane; 3] {
    let value = coefficient_components(words, 0, 16);
    let dtdx = coefficient_components(words, 8, 24);
    let dtde = coefficient_components(words, 32, 48);
    let dtdy = coefficient_components(words, 40, 56);
    core::array::from_fn(|component| AttributePlane {
        base: value[component],
        dx: dtdx[component],
        de: dtde[component],
        dy: dtdy[component],
    })
}

/// The four Q16.16 components at one (integer byte, fraction byte) pair of a
/// coefficient block: the first u32's high and low halves, then the second
/// u32's high and low halves, each reassembled from its own integer and
/// fraction 16-bit field.
///
/// Shade reads all four as R, G, B, A. Texture reads the first three as
/// S, T, W and ignores the fourth.
fn coefficient_components(
    words: &super::triangle::CoefficientWords,
    integer_byte: usize,
    fraction_byte: usize,
) -> [i32; 4] {
    let half = |index: usize| -> u32 {
        let word = words[index / 2];
        if index % 2 == 0 {
            word.w0()
        } else {
            word.w1()
        }
    };
    let integer_first = half(integer_byte / 4);
    let integer_second = half(integer_byte / 4 + 1);
    let fraction_first = half(fraction_byte / 4);
    let fraction_second = half(fraction_byte / 4 + 1);
    [
        fixed_16_16(integer_first >> 16, fraction_first >> 16),
        fixed_16_16(integer_first, fraction_first),
        fixed_16_16(integer_second >> 16, fraction_second >> 16),
        fixed_16_16(integer_second, fraction_second),
    ]
}

/// The S10.5 texture coordinates one covered pixel samples at, from the
/// triangle's own S/T/W planes evaluated at the SAME attribute sample point
/// the shade planes use.
///
/// `stw` is the three planes' Q16.16 values at that point, already evaluated
/// by [`attribute_plane`]; `perspective` is `OtherMode::texture_perspective`.
///
/// # Both factors are grounded on pinned RT64 and the S10.5 wire format
///
/// Both come from `fn64-render-reference`'s `draw_raw_rdp_triangle_impl`
/// (`raster/draw.rs:898`), which derived them against WM2000's own title
/// screen and recorded what getting them wrong looks like.
///
/// 1. **Perspective.** Hardware `tcdiv` is not a bare `S/W` ratio. The
///    pipeline feeds tcdiv the HIGH bits of the s15.16 attribute planes and
///    multiplies by a 2^15-normalized reciprocal of W, so the output is
///    `(S/W) * 2^15` in S10.5 units = `(S/W) * 2^10` texels (pinned RT64's
///    `tcdiv` perspective path; RT64 divides `s.w` by `w` and scales
///    identically). So the RAW S10.5 value this function returns carries
///    `2^15`, and `2^10` is what that same value denotes in TEXELS -- the
///    two differ by the field's five fractional bits, and confusing them is
///    exactly the defect fixed here.
///
///    **This constant was `1024.0` until measured.** It was fitted
///    empirically against WM2000's title screen, where the reference had
///    recorded that without a scale at all "the whole title-screen quad
///    collapsed onto texel (0,0) -- every pixel sampled the image's corner
///    and the presented frame was a uniform field." Going from 1 to 1024
///    fixed that total collapse and left a 32x residue, which on one large
///    flat quad reads as mild mis-scaling. On in-match geometry -- 100%
///    perspective, small polygons -- the same residue collapsed each
///    triangle onto a single texel: 72% of textured triangles requested
///    zero whole texels of span and 87% sampled one distinct texel, while
///    the frame-wide texel histogram stayed broad and healthy-looking. See
///    `docs/RT64-WM2000-COMBINER-CENSUS.md`.
/// 2. **Non-perspective (`G_TP_NONE`).** The divide is skipped entirely and
///    the plane's own s15.16 value converts to S10.5 by dividing by `2^16`
///    -- the plane is s15.16, so `>> 16` yields S10.5, and the
///    non-perspective path applies no further scale of its own.
///    The remaining `>>5` to whole texels is the
///    SAMPLER's, not this function's, so the total is `2^21`. See
///    [`PLANE_TO_TEXEL`] for the measurement that corrected this.
///
/// # Perspective division preserves W's sign
///
/// Pinned RT64 reconstructs each W plane value as a signed Q16.16 float and
/// divides the corresponding S/T vertex values by that signed `w1`/`w2`/`w3`
/// (`src/gbi/rt64_gbi_rdp.cpp:512,523-525`). It never takes W's magnitude.
/// A negative W therefore flips both texture-coordinate signs; replacing it
/// with `abs(W)` selects the opposite clamped edge of a tile. A zero W still
/// does not fault in this float path: IEEE division produces an infinity or
/// NaN which the narrowing policy below handles deterministically.
///
/// # The result is currently saturated into S10.5's `i16`
///
/// `TextureCoordinateS10_5` is an `i16` -- ten integer bits and five
/// fractional ones -- which is the format the texrect path's
/// `TexrectDraw::s_at` also produces, so both primitives reach
/// [`crate::sample_point`] through one coordinate type.
///
/// A perspective divide by a near-zero W can produce a coordinate far outside
/// that range. Saturating gives the existing integer sampler a deterministic
/// input instead of wrapping across the tile, but it is not full RT64 parity:
/// RT64 retains floating texture coordinates, so exactly `S/W = 1` remains
/// 1024.0 texels while this representation clamps raw 32768 to 32767. Fixing
/// that boundary requires widening or separating the triangle-coordinate
/// representation from the shared texrect S10.5 input.
pub(crate) fn texture_coordinates_s10_5(stw: [i64; 3], perspective: bool) -> (i16, i16) {
    let (s, t) = if perspective {
        // RT64 divides by signed w1/w2/w3; negative W must flip S and T.
        let denominator = stw[2] as f32;
        (
            stw[0] as f32 / denominator * PERSPECTIVE_TEXEL_SCALE,
            stw[1] as f32 / denominator * PERSPECTIVE_TEXEL_SCALE,
        )
    } else {
        (
            stw[0] as f32 / PLANE_TO_TEXEL,
            stw[1] as f32 / PLANE_TO_TEXEL,
        )
    };
    // `as i16` on a float is NOT available; and `f32 as i32` saturates while
    // a subsequent `as i16` would WRAP. So the clamp is explicit, in float,
    // before any narrowing. Zero W can produce NaN for a zero numerator, so
    // map it to zero rather than letting a cast choose the result implicitly.
    (saturate_s10_5(s), saturate_s10_5(t))
}

/// One float S10.5 coordinate clamped into `i16` before narrowing.
///
/// Zero W with a zero numerator reaches the NaN arm; it maps to zero rather
/// than relying on a float-to-integer cast's implicit NaN policy.
fn saturate_s10_5(value: f32) -> i16 {
    if value.is_nan() {
        return 0;
    }
    value.clamp(f32::from(i16::MIN), f32::from(i16::MAX)) as i16
}

/// tcdiv's perspective output scale: `(S/W) * 2^15` in RAW S10.5 units,
/// which is `(S/W) * 2^10` texels once the field's five fractional bits are
/// divided out. This function returns the RAW value, so the constant is
/// `2^15`. See [`texture_coordinates_s10_5`].
///
/// **Was `1024.0` (`2^10`), and that was a crossed unit domain**: `2^10` is
/// the TEXEL-domain scale, written into the field that holds the RAW one, so
/// every perspective coordinate came out 32x too small. Measured on WM2000
/// with `FN64_COMBINER_CENSUS=1`: 72% of textured triangles requested a span
/// of zero whole texels and 87% sampled exactly one distinct texel across
/// their whole span -- flat-shaded models with a correctly varied frame-wide
/// texel histogram, which is why every aggregate instrument missed it. See
/// `docs/RT64-WM2000-COMBINER-CENSUS.md`.
///
/// **The value is independently derivable from pinned RT64**, so it is not a
/// refit and no longer rests on an excluded source. RT64's triangle path
/// (`src/gbi/rt64_gbi_rdp.cpp:523-530`) computes the perspective texture
/// coordinate as `(texcoord / w) * 1024.0f` -- 1024 whole TEXELS. This
/// function produces S10.5, which carries five fractional bits, so the
/// equivalent plane scale is `1024 * 2^5 = 2^15 = 32768`. That reconciles
/// exactly, and the `2^15` here is the S10.5 spelling of RT64's `* 1024`.
///
/// RT64's non-perspective arm in the same block is `(texcoord * 1024) /
/// 16384`, consistent with the `2^16` sibling constant below composing with
/// the sampler's own `>>5`.
const PERSPECTIVE_TEXEL_SCALE: f32 = 32768.0;

/// s15.16 plane value -> S10.5 texel coordinate, for the `G_TP_NONE` path.
///
/// **`2^16`, because this produces S10.5 and NOT texels.** Hardware reaches
/// the texel in two steps and fn64 does the same, in the same two places:
///
/// | step | hardware | fn64 |
/// |---|---|---|
/// | plane -> S10.5 | `>> 16`, the s15.16 wire scale | this constant |
/// | S10.5 -> texel | `*S = locs >> 5` (`tcoord.c:143`) | `sample.rs`'s `div_euclid(TEXEL_FRACTION_SCALE)` |
///
/// `tcdiv_nopersp` itself applies NO scale at all (`tcoord.c:1024`: it is
/// `SIGN16(ss) & 0x1ffff`), so the whole non-perspective conversion is
/// `>>16` composed with `>>5` = `2^21` from plane to whole texel.
///
/// **This was `2^21` and that was wrong**: the function's result is consumed
/// as S10.5 by `TextureCoordinateS10_5::from_raw`, so the sampler applied
/// its own `>>5` on top and the `2^5` was counted TWICE, making a plane
/// `2^26` per texel instead of `2^21`. Measured on the parity corpus's
/// `textured-triangle-point-sampled`: with `2^21` here, RT64 reproduced the
/// hand-derived key and wgpu read texel 0 everywhere; the two swapped at
/// `2^26`. RT64 agrees with hardware, and this constant is why fn64 did not.
const PLANE_TO_TEXEL: f32 = (1u32 << 16) as f32;

#[cfg(test)]
mod tests;
