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

/// The four subpixel Y sample offsets, in eighths of a pixel, that the RDP
/// evaluates per scanline. Sample centers sit on odd eighths.
pub(crate) const SAMPLE_Y_EIGHTHS: [i32; 4] = [1, 3, 5, 7];

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
/// eight subsamples -- two X columns at 1/8 and 5/8, four Y rows at 1/8,
/// 3/8, 5/8, 7/8 -- fall inside the triangle.
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
        for offset_x in [1, 5] {
            let sample_x = (i64::from(x) * 8 + i64::from(offset_x)) * Q16_ONE / 8;
            if sample_x >= left_x && sample_x < right_x {
                count += 1;
            }
        }
    }
    count
}

#[cfg(test)]
mod tests;
