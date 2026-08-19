use super::*;

/// The non-shaded, non-textured, non-depth triangle opcode: four base-edge
/// words, 32 bytes.
const BASE: u8 = 0x08;

/// Builds one triangle's four base-edge wire words, big-endian, exactly as
/// the RDP command stream carries them.
///
/// Layout taken from the wire, not from the decoder: word 0 high half holds
/// `lft` at bit 23, `level` at 21..19, `tile` at 18..16; word 0 low half is
/// YL; word 1 high half is YM, low half is YH; then XL/dXLdy, XH/dXHdy,
/// XM/dXMdy as full 32-bit Q16.16 pairs.
#[allow(clippy::too_many_arguments)]
fn wire(
    lft: bool,
    yl: i16,
    ym: i16,
    yh: i16,
    xl: i32,
    dxldy: i32,
    xh: i32,
    dxhdy: i32,
    xm: i32,
    dxmdy: i32,
) -> Vec<u8> {
    let w0 = (u32::from(lft) << 23) | (yl as u16 as u32);
    let w1 = ((ym as u16 as u32) << 16) | (yh as u16 as u32);
    let mut bytes = Vec::with_capacity(32);
    for word in [
        w0,
        w1,
        xl as u32,
        dxldy as u32,
        xh as u32,
        dxhdy as u32,
        xm as u32,
        dxmdy as u32,
    ] {
        bytes.extend_from_slice(&word.to_be_bytes());
    }
    bytes
}

fn decode(bytes: &[u8]) -> RawTriangle {
    RawTriangle::decode(BASE, bytes).expect("hand-built base triangle is 32 bytes")
}

/// Q16.16 for a whole number of pixels.
const fn px(pixels: i32) -> i32 {
    pixels << 16
}

/// S11.2 for a whole number of scanlines.
const fn line(scanlines: i16) -> i16 {
    scanlines << 2
}

// ---------------------------------------------------------------------------
// The polarity of wire bit 23
// ---------------------------------------------------------------------------

#[test]
fn the_wm2000_title_triangle_spans_run_major_left_to_minor_right() {
    // Byte-exact coefficients from WM2000's live title-scene XBUS stream,
    // as pinned by `fn64-render-reference`'s
    // `real_stream_left_major_rect_split_triangle_rasterizes_interior`
    // (`raster/tests/group2.rs:1336`). This is the ONLY ground truth in the
    // repo for wire bit 23's polarity: RT64 never decodes raw RDP edge
    // coefficients at all.
    //
    // Read the geometry independently of any span code, from the numbers:
    //   xh    =   770048 / 65536 = 11.75 px, and dxhdy == 0 -- a VERTICAL
    //                                        edge parked at x = 11.75.
    //   xm    =   701940 / 65536 = 10.71 px, dxmdy = 272435 / 65536
    //                                      = +4.157 px per scanline.
    // The H (major) edge is the constant one at 11.75; the M (minor) edge
    // starts just LEFT of it and marches RIGHT, so by scanline 15 it is far
    // to the right of the major edge. The only non-empty reading is
    // major-on-the-left. And this triangle carries bit 23 SET.
    //
    // Therefore bit 23 set == LEFT-major. The decoder's accessor
    // `RawTriangle::right_major()` names the same bit with the opposite
    // sense; `left_major()` here is where that is corrected, once.
    let triangle = decode(&wire(
        true, 106, 106, 17, 6832128, -16842729, 770048, 0, 701940, 272435,
    ));
    assert!(triangle.right_major(), "the live-stream tri carries bit 23 set");
    assert!(left_major(&triangle), "bit 23 set means the H edge is on the LEFT");

    // Scanline 15, sample row y*8+7 = 127 eighths.
    //   yh = 17 (S11.2) -> yh & !3 = 16 -> high_origin_eighth = 32.
    //   major_x = xh + 0 * (127-32)/8      = 770048          = 11.75 px
    //   ym = 106 -> middle_eighth = 212 > 127, so the minor edge is XM:
    //   minor_x = xm + 272435 * (127-32)/8
    //           = 701940 + floor(272435 * 95 / 8)
    //           = 701940 + floor(25881325 / 8) = 701940 + 3235165 = 3937105
    //           = 60.07 px
    let (left, right) = row_span(&triangle, 15 * 8 + 7);
    assert_eq!(left, 770048, "the major edge is the LEFT one");
    assert_eq!(right, 3_937_105, "the minor edge is the RIGHT one");
    assert!(right > left, "a left-major span is non-empty");
}

#[test]
fn inverting_bit_twenty_three_makes_the_wm2000_title_triangle_empty() {
    // The failure mode the reference recorded and this lane must never
    // reintroduce: with the polarity flipped, the SAME live-stream triangle
    // computes right < left on every scanline and rasterizes zero pixels --
    // "raw RDP geometry decoded but never rasterized a single pixel".
    //
    // Driven by clearing bit 23 on the wire rather than by calling an
    // internal with a flag, so this exercises the real decode path.
    let flipped = decode(&wire(
        false, 106, 106, 17, 6832128, -16842729, 770048, 0, 701940, 272435,
    ));
    let (left, right) = row_span(&flipped, 15 * 8 + 7);
    assert!(right < left, "the inverted reading yields an empty span");
    assert_eq!(row_pixel_range(&flipped, 15, 320), None);
    assert!(covered_rows(&flipped, 320, 240).is_empty());
}

#[test]
fn the_wm2000_title_triangle_covers_its_hand_derived_rows() {
    let triangle = decode(&wire(
        true, 106, 106, 17, 6832128, -16842729, 770048, 0, 701940, 272435,
    ));
    // Vertical extent, by hand:
    //   yh_eighth = 17*2 = 34, yl_eighth = 106*2 = 212.
    //   min_y = ceil((34-7)/8) = ceil(27/8) = 4
    //   max_y = ceil((212-1)/8) = ceil(211/8) = 27
    assert_eq!(row_range(&triangle, 240), (4, 27));

    let rows = covered_rows(&triangle, 320, 240);
    assert_eq!(rows.first().map(|row| row.y), Some(4));
    assert_eq!(rows.last().map(|row| row.y), Some(26));
    assert_eq!(rows.len(), 23);

    // Scanline 15's own range, from the two edge values derived by hand in
    // `the_wm2000_title_triangle_spans_run_major_left_to_minor_right` --
    // except that the range is the UNION over the scanline's four sample
    // rows, and the minor edge marches right, so the widest sample row (7/8)
    // sets the right end while the narrowest (1/8) sets the left.
    //   sample row 1/8: minor_x = 701940 + floor(272435*(121-32)/8)
    //                           = 701940 + floor(24246715/8)
    //                           = 701940 + 3030839 = 3732779
    //   min_left over the four rows is the constant major edge, 770048.
    //   max_right is the 7/8 row's 3937105.
    //   x0 = ceil((770048 - 7*65536/8) / 65536) = ceil((770048-57344)/65536)
    //      = ceil(712704/65536) = ceil(10.875) = 11
    //   x1 = ceil((3937105 - 65536/8) / 65536) = ceil((3937105-8192)/65536)
    //      = ceil(3928913/65536) = ceil(59.95) = 60
    assert_eq!(row_pixel_range(&triangle, 15, 320), Some((11, 60)));

    // The reference's own oracle asserts an interior pixel at (30,15) is
    // covered and (5,15) is not. Both agree with the range above, and this
    // module reproduces them through its own per-pixel coverage.
    assert_eq!(pixel_coverage(&triangle, 30, 15), 8);
    assert_eq!(pixel_coverage(&triangle, 5, 15), 0);
}

// ---------------------------------------------------------------------------
// Geometry, on hand-built triangles whose answer is arithmetic
// ---------------------------------------------------------------------------

#[test]
fn an_axis_aligned_left_major_box_covers_exactly_its_own_rectangle() {
    // A degenerate "triangle" whose two edges are both vertical, at x=4 and
    // x=12, spanning scanlines 2..=5. Every scanline must report [4, 12).
    //   yh = line(2) = 8, yl = line(6) = 24, ym = yl so the XL edge never
    //   takes over within the covered range... except that sample rows at or
    //   past ym use XL, and ym == yl == 24 means no sample row reaches it.
    let triangle = decode(&wire(
        true,
        line(6),
        line(6),
        line(2),
        px(12),
        0,
        px(4),
        0,
        px(12),
        0,
    ));
    // min_y = ceil((16-7)/8) = ceil(9/8) = 2; max_y = ceil((48-1)/8) = 6.
    assert_eq!(row_range(&triangle, 240), (2, 6));
    let rows = covered_rows(&triangle, 320, 240);
    assert_eq!(rows.len(), 4);
    for (index, row) in rows.iter().enumerate() {
        assert_eq!(row.y, 2 + index as u32);
        // x0 = ceil((4*65536 - 57344)/65536) = ceil(3.125) = 4
        // x1 = ceil((12*65536 - 8192)/65536) = ceil(11.875) = 12
        assert_eq!((row.x0, row.x1), (4, 12));
    }
    // Interior fully covered; the pixel just outside each end is not.
    assert_eq!(pixel_coverage(&triangle, 8, 3), 8);
    assert_eq!(pixel_coverage(&triangle, 3, 3), 0);
    assert_eq!(pixel_coverage(&triangle, 12, 3), 0);
    assert_eq!(pixel_coverage(&triangle, 8, 1), 0, "above YH");
    assert_eq!(pixel_coverage(&triangle, 8, 6), 0, "at or below YL");
}

#[test]
fn a_right_major_triangle_mirrors_the_span_sides() {
    // Same edges, bit 23 CLEAR: the H edge is now the RIGHT side, so the
    // span runs minor(12) -> major(4) and is empty. Pins that `left_major`
    // is actually consulted rather than the sides being fixed.
    let triangle = decode(&wire(
        false,
        line(6),
        line(6),
        line(2),
        px(12),
        0,
        px(4),
        0,
        px(12),
        0,
    ));
    let (left, right) = row_span(&triangle, 3 * 8 + 1);
    assert_eq!((left, right), (px(12) as i64, px(4) as i64));
    assert!(covered_rows(&triangle, 320, 240).is_empty());
}

#[test]
fn a_sloped_minor_edge_widens_each_declared_row_by_its_own_slope() {
    // Major edge parked at x=4; minor edge starts at x=4 and marches right
    // one whole pixel per scanline. Scanline n's right end must therefore
    // grow by exactly one pixel per scanline -- the property a single
    // collapsed span would destroy.
    let triangle = decode(&wire(
        true,
        line(10),
        line(10),
        line(0),
        px(4),
        0,
        px(4),
        0,
        px(4),
        px(1),
    ));
    let rows = covered_rows(&triangle, 320, 240);
    assert_eq!(rows.len(), 10);
    for row in &rows {
        assert_eq!(row.x0, 4);
    }
    // Row y's widest sample row is y*8+7 eighths; high_origin is 0.
    //   minor_x = 4*65536 + 65536 * (8y+7) / 8 = 65536 * (4 + y + 7/8)
    //   x1 = ceil((minor_x - 8192)/65536) = ceil(4 + y + 0.875 - 0.125)
    //      = ceil(4 + y + 0.75) = 5 + y
    for (index, row) in rows.iter().enumerate() {
        assert_eq!(row.x1, 5 + index as u32, "row {} right end", row.y);
    }
}

#[test]
fn the_minor_edge_switches_from_xm_to_xl_at_ym() {
    // XM runs from x=4 rightward; at YM=scanline 4 the minor edge jumps to
    // XL at x=20 and stays. A sample row below YM must read XL, not XM.
    let triangle = decode(&wire(
        true,
        line(8),
        line(4),
        line(0),
        px(20),
        0,
        px(0),
        0,
        px(4),
        0,
    ));
    // Sample row in scanline 1 (below YM=32 eighths): minor is XM = 4 px.
    assert_eq!(row_span(&triangle, 1 * 8 + 1).1, i64::from(px(4)));
    // Sample row in scanline 5 (at/past YM): minor is XL = 20 px.
    assert_eq!(row_span(&triangle, 5 * 8 + 1).1, i64::from(px(20)));
    let rows = covered_rows(&triangle, 320, 240);
    assert_eq!(rows[0].x1, 4);
    assert_eq!(rows[5].x1, 20);
}

#[test]
fn covered_rows_clamps_to_the_target_and_drops_rows_it_leaves_empty() {
    // A triangle whose vertical extent runs off the bottom of a 4-row
    // target and whose horizontal extent runs off the right of a 6-column
    // one. Nothing outside the target may be declared.
    let triangle = decode(&wire(
        true,
        line(100),
        line(100),
        line(0),
        px(50),
        0,
        px(2),
        0,
        px(50),
        0,
    ));
    let rows = covered_rows(&triangle, 6, 4);
    assert_eq!(rows.len(), 4);
    for row in &rows {
        assert!(row.y < 4);
        assert_eq!((row.x0, row.x1), (2, 6));
    }
    // A triangle entirely to the right of a narrow target declares nothing
    // rather than an empty or clamped-to-zero-width row.
    let rows = covered_rows(&triangle, 2, 4);
    assert!(rows.is_empty());
}

#[test]
fn a_zero_height_triangle_declares_no_rows() {
    let triangle = decode(&wire(
        true,
        line(3),
        line(3),
        line(3),
        px(12),
        0,
        px(4),
        0,
        px(12),
        0,
    ));
    assert!(covered_rows(&triangle, 320, 240).is_empty());
}

#[test]
fn a_partially_covered_edge_pixel_reports_partial_coverage() {
    // Left edge parked at x = 4.5 (halfway through pixel 4): of pixel 4's
    // two sample columns (x+1/8 = 4.125 and x+5/8 = 4.625) only the second
    // is inside, so four of eight subsamples are covered. Pixel 5 is fully
    // inside. This is the fact that makes a declared row a SUPERSET of the
    // drawn pixels, which `raster_flat_triangle` must handle.
    let triangle = decode(&wire(
        true,
        line(4),
        line(4),
        line(0),
        px(12),
        0,
        px(4) + (1 << 15),
        0,
        px(12),
        0,
    ));
    assert_eq!(pixel_coverage(&triangle, 4, 1), 4);
    assert_eq!(pixel_coverage(&triangle, 5, 1), 8);
    // The declared range still starts at pixel 4, because the pixel is
    // partly covered: x0 = ceil((4.5*65536 - 57344)/65536) = ceil(3.625) = 4.
    assert_eq!(row_pixel_range(&triangle, 1, 320), Some((4, 12)));
}

// ---------------------------------------------------------------------------
// The fixed-point helpers, on the cases `/` gets wrong
// ---------------------------------------------------------------------------

#[test]
fn fixed_mul_ratio_floors_toward_negative_infinity() {
    // A negative slope walked one subpixel: truncation would give -1, but
    // the RDP's edge walk floors, so this must be -2.
    assert_eq!(fixed_mul_ratio(-9, 1, 8), -2);
    assert_eq!(fixed_mul_ratio(9, 1, 8), 1);
    // The full i32 range times a large subpixel delta stays in i64.
    assert_eq!(fixed_mul_ratio(i32::MIN, 8, 8), i64::from(i32::MIN));
}

#[test]
fn ceil_ratio_rounds_up_on_both_sides_of_zero() {
    assert_eq!(ceil_ratio(9, 8), 2);
    assert_eq!(ceil_ratio(8, 8), 1);
    assert_eq!(ceil_ratio(-9, 8), -1);
    assert_eq!(ceil_ratio(-8, 8), -1);
    assert_eq!(ceil_ratio(-1, 8), 0);
}


// ---------------------------------------------------------------------------
// The coverage samples are a CHECKERBOARD, not a 2x4 grid
// ---------------------------------------------------------------------------

/// **The X sample columns alternate by Y row, and the difference decides
/// whether a pixel is painted at all.**
///
/// This test exists because the mutant that freezes the columns at (1, 5) on
/// every row SURVIVED the whole suite. The gradient tests above sample at Y
/// row 1, where both readings agree, so nothing reached the difference.
///
/// Hand-derived. Left edge parked at x = 0.75 px (49152 in Q16.16), right
/// edge far away. Pixel 0's eight subsamples, by
/// `crate::COVERAGE_SAMPLES`:
///   Y row 1 -> X columns 1/8 = 0.125, 5/8 = 0.625  -- both < 0.75, OUT
///   Y row 3 -> X columns 3/8 = 0.375, 7/8 = 0.875  -- 0.875 >= 0.75, IN
///   Y row 5 -> X columns 1/8, 5/8                  -- both OUT
///   Y row 7 -> X columns 3/8, 7/8                  -- 0.875 IN
/// So the checkerboard covers exactly 2 of 8.
///
/// Frozen at (1, 5) on every row, all eight samples are 0.125 or 0.625 and
/// the pixel covers ZERO -- so the pixel would not be painted at all, not
/// merely painted with a different weight.
#[test]
fn the_x_sample_columns_alternate_by_row_and_change_whether_a_pixel_is_covered() {
    // yh = 0, yl = 4 scanlines, left edge x = 0.75, right edge x = 6.
    let triangle = decode(&wire(
        true,
        line(4),
        line(4),
        0,
        px(6),
        0,
        49152,
        0,
        px(6),
        0,
    ));
    assert_eq!(
        pixel_coverage(&triangle, 0, 0),
        2,
        "pixel 0 is covered only on the Y rows whose X columns are (3, 7)"
    );
    // Pixel 1 is entirely right of the edge and fully covered either way,
    // so this test cannot pass by the coverage function returning 0.
    assert_eq!(pixel_coverage(&triangle, 1, 0), 8);

    // And the columns themselves, read off `crate::COVERAGE_SAMPLES`.
    assert_eq!(sample_x_eighths(1), [1, 5]);
    assert_eq!(sample_x_eighths(3), [3, 7]);
    assert_eq!(sample_x_eighths(5), [1, 5]);
    assert_eq!(sample_x_eighths(7), [3, 7]);
}

/// The attribute sample point follows the same checkerboard.
///
/// For the triangle above, pixel 0's FIRST covered subsample in the RDP's
/// scan order is Y row 3, X column 7/8 -- not Y row 1. So the attribute
/// plane is evaluated at `edge_delta_y_eighth = 3` and
/// `edge_delta_x = 0.875 - 0.75 = 0.125 px`, not at row 1 at all.
///
/// With the columns frozen at (1, 5) there is no covered subsample and the
/// function returns `None`, which the executor turns into a named refusal.
#[test]
fn the_attribute_sample_point_follows_the_checkerboard_too() {
    let triangle = decode(&wire(
        true,
        line(4),
        line(4),
        0,
        px(6),
        0,
        49152,
        0,
        px(6),
        0,
    ));
    let (delta_y_eighth, delta_x) =
        attribute_sample(&triangle, 0, 0).expect("pixel 0 has two covered subsamples");
    assert_eq!(delta_y_eighth, 3, "the first covered Y row is 3/8, not 1/8");
    // 7/8 px = 57344; major edge = 49152. 57344 - 49152 = 8192 = 0.125 px.
    assert_eq!(delta_x, 8192);
}
