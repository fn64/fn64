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
pub(crate) fn row_pixel_range(triangle: &RawTriangle, y: i32, clamp_width: u32) -> Option<(u32, u32)> {
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
        if right_x > left_x {
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
    let x0 = ceil_ratio(min_left - 7 * Q16_ONE / 8, Q16_ONE).clamp(0, i64::from(clamp_width)) as u32;
    let x1 = ceil_ratio(max_right - Q16_ONE / 8, Q16_ONE).clamp(0, i64::from(clamp_width)) as u32;
    if x1 <= x0 {
        return None;
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
/// Used by the rasterizer, not the decoder: a declared row is declared as a
/// whole `[x0, x1)` range because that is what `plan_render_target_rows`
/// can express, while a pixel at the very edge of that range may still have
/// zero coverage. That asymmetry is safe in exactly one direction -- the
/// declaration is a superset of what is drawn -- and the rasterizer closes
/// it by writing *every* pixel in the declared range, using the resident's
/// own prior byte where coverage is zero. See `raster_flat_triangle`.
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
/// exists for the hardware's own span-walker and is not a third term.
/// Carried rather than dropped so the decode is complete and a future
/// consumer does not have to re-read the wire.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct AttributePlane {
    pub(crate) base: i32,
    pub(crate) dx: i32,
    pub(crate) de: i32,
    pub(crate) dy: i32,
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
pub(crate) fn attribute_sample(
    triangle: &RawTriangle,
    x: i32,
    y: i32,
) -> Option<(i32, i64)> {
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

#[cfg(test)]
mod tests;
