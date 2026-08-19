//! The harness's own validation.
//!
//! A wrong harness gives fast, confident, wrong answers -- strictly worse than
//! the slow loop it replaces. So the first thing it must do is reproduce a
//! result whose behaviour is already pinned elsewhere in this crate.

use super::*;

/// The primitive colour the pinned flat-triangle test uses, and its RGBA16
/// encoding, both derived BY HAND from the wire and from nothing else:
///
///   PRIM = 0x80FF4080 -> R 0x80, G 0xFF, B 0x40, A 0x80
///   RGBA16 5/5/5/1 = (0x80>>3 << 11) | (0xFF>>3 << 6) | (0x40>>3 << 1) | 1
///                  = 0x8000 | 0x07C0 | 0x0010 | 1 = 0x87D1
const PRIM_WIRE: u32 = 0x80FF_4080;
const PRIM_RGBA16: u16 = 0x87D1;

/// The pinned fixture, stated in the harness's own vocabulary.
///
/// `production::tests::a_flat_raw_triangles_pixels_reach_the_committed_guest_write_payload`
/// drives the identical packet by hand and asserts the identical bytes. This
/// builds it from `Rdp`/`Tri` instead.
fn pinned_flat_frame() -> Frame {
    Rdp::new(16, 8)
        .cycle(CycleType::One)
        .combine_prim_passthrough()
        .prim_color(PRIM_WIRE)
        .triangle(Tri::flat().left_major().edges(2.0, 6.0).rows(0..3))
        .run()
}

/// **The harness agrees with the pipeline the ROM path drives.**
///
/// The pinned test's hand-derived footprint, re-derived here from the wire
/// rather than copied from the code under test:
///   yh = 0, yl = 3<<2 = 12 (S11.2) -> rows 0, 1, 2
///   left edge  x = 2.0 -> x0 = ceil(2 - 7/8) = 2
///   right edge x = 6.0 -> x1 = ceil(6 - 1/8) = 6
/// So each row writes pixels 2..6 = 4 pixels = 8 bytes, at
/// 0x2000 + (16y + 2)*2 -> 0x2004, 0x2024, 0x2044.
///
/// If this disagrees with the pinned test, the harness is wrong.
#[test]
fn the_harness_reproduces_the_pinned_flat_triangles_guest_bytes() {
    let frame = pinned_flat_frame();

    assert_eq!(
        frame.write_ranges(),
        vec![(0x2004, 8), (0x2024, 8), (0x2044, 8)],
        "one 8-byte CompletedWrite per covered scanline, at the hand-derived addresses"
    );

    for y in 0..3 {
        for x in 2..6 {
            frame.assert_pixel(x, y, PRIM_RGBA16);
        }
    }
}

/// Each committed write's digest is the digest of four primitive-coloured
/// RGBA16 pixels -- the same derivation `copy_committed_guest_writes` re-runs
/// over the payload before writing a byte into guest RDRAM. A digest match is
/// therefore a statement about what lands in RDRAM, not about what the backend
/// recorded.
#[test]
fn the_harness_frames_writes_carry_the_digest_of_the_hand_derived_pixels() {
    let frame = pinned_flat_frame();
    let expected_row: Vec<u8> = PRIM_RGBA16.to_be_bytes().repeat(4);
    for (index, write) in frame.writes().iter().enumerate() {
        let expected = CompletedWrite::try_from_bytes(write.access(), &expected_row)
            .expect("eight bytes match the declared eight-byte access");
        assert_eq!(
            write.content(),
            expected.content(),
            "row {index}'s committed digest"
        );
    }
}

/// The triangle composes INTO the cleared buffer rather than replacing it.
///
/// The failure mode this catches is a triangle whose full-extent output is a
/// fresh buffer, which would blank every pixel the clear wrote.
#[test]
fn the_harness_leaves_every_pixel_outside_the_triangle_at_the_clear_colour() {
    pinned_flat_frame().assert_outside_untouched(2..6, 0..3);
}

// ---------------------------------------------------------------------------
// The Tri:: geometry builder states edge cases instead of searching for them
// ---------------------------------------------------------------------------

/// **A subpixel edge position is STATED, and it moves the covered span.**
///
/// The RDP's left-edge rule is `x0 = ceil(x - 7/8)`, derived from the wire,
/// not from the raster. So for a left edge swept across one pixel:
///   x = 2.000 -> ceil(1.125) = 2
///   x = 2.125 -> ceil(1.250) = 2
///   x = 2.875 -> ceil(2.000) = 2
///   x = 2.900 -> ceil(2.025) = 3
/// The first covered column therefore stays 2 until the edge passes 2.875.
///
/// Finding this boundary previously took a throwaway Python search, twice.
#[test]
fn a_stated_subpixel_left_edge_moves_the_first_covered_column_at_seven_eighths() {
    for (left_x, expected_first_column) in [(2.0, 2u32), (2.125, 2), (2.875, 2), (2.9, 3), (3.0, 3)]
    {
        let frame = Rdp::new(16, 8)
            .cycle(CycleType::One)
            .combine_prim_passthrough()
            .prim_color(PRIM_WIRE)
            .triangle(Tri::flat().left_major().edges(left_x, 6.0).rows(0..1))
            .run();

        let first_covered = (0..16)
            .find(|&x| frame.pixel(x, 0) == PRIM_RGBA16)
            .unwrap_or_else(|| panic!("left edge {left_x} covered no pixel at all"));
        assert_eq!(
            first_covered, expected_first_column,
            "left edge at {left_x} px must first cover column {expected_first_column}"
        );
    }
}

/// The same, for the right edge's own `ceil(x - 1/8)` rule:
///   x = 6.000 -> ceil(5.875) = 6 -> last covered column 5
///   x = 6.125 -> ceil(6.000) = 6 -> last covered column 5
///   x = 6.200 -> ceil(6.075) = 7 -> last covered column 6
#[test]
fn a_stated_subpixel_right_edge_moves_the_last_covered_column_at_one_eighth() {
    for (right_x, expected_last_column) in [(6.0, 5u32), (6.125, 5), (6.2, 6), (7.0, 6)] {
        let frame = Rdp::new(16, 8)
            .cycle(CycleType::One)
            .combine_prim_passthrough()
            .prim_color(PRIM_WIRE)
            .triangle(Tri::flat().left_major().edges(2.0, right_x).rows(0..1))
            .run();

        let last_covered = (0..16)
            .filter(|&x| frame.pixel(x, 0) == PRIM_RGBA16)
            .next_back()
            .unwrap_or_else(|| panic!("right edge {right_x} covered no pixel at all"));
        assert_eq!(
            last_covered, expected_last_column,
            "right edge at {right_x} px must last cover column {expected_last_column}"
        );
    }
}

/// `rows()` names the covered scanlines directly, and the raster's `y < yl`
/// bound means a `rows(1..3)` triangle covers rows 1 and 2 and leaves row 0
/// and row 3 at the clear colour.
#[test]
fn stated_rows_bound_the_covered_scanlines_at_both_ends() {
    let frame = Rdp::new(16, 8)
        .cycle(CycleType::One)
        .combine_prim_passthrough()
        .prim_color(PRIM_WIRE)
        .triangle(Tri::flat().left_major().edges(2.0, 6.0).rows(1..3))
        .run();

    assert_eq!(frame.write_ranges().len(), 2, "two covered scanlines");
    frame.assert_pixel(3, 0, CLEAR_COLOR_RGBA16);
    frame.assert_pixel(3, 1, PRIM_RGBA16);
    frame.assert_pixel(3, 2, PRIM_RGBA16);
    frame.assert_pixel(3, 3, CLEAR_COLOR_RGBA16);
}

/// A sloped edge is stated in pixels per scanline, and each row's covered span
/// shifts by that slope. Left edge 2.0 sliding right by 1 px per scanline:
///   row 0: ceil(2 - 7/8) = 2
///   row 1: ceil(3 - 7/8) = 3
///   row 2: ceil(4 - 7/8) = 4
#[test]
fn a_stated_edge_slope_shifts_each_rows_covered_span() {
    let frame = Rdp::new(16, 8)
        .cycle(CycleType::One)
        .combine_prim_passthrough()
        .prim_color(PRIM_WIRE)
        .triangle(
            Tri::flat()
                .left_major()
                .edges(2.0, 8.0)
                .slopes(1.0, 0.0)
                .rows(0..3),
        )
        .run();

    for (row, expected_first) in [(0u32, 2u32), (1, 3), (2, 4)] {
        let first_covered = (0..16)
            .find(|&x| frame.pixel(x, row) == PRIM_RGBA16)
            .unwrap();
        assert_eq!(
            first_covered, expected_first,
            "row {row}'s first covered column under a 1 px/scanline left slope"
        );
    }
}

/// `px_frac` is exact for the eighths the edge rules are defined on, and
/// rounds to nearest rather than truncating -- a truncating conversion would
/// place a stated 0.75 px edge one ULP low, which is precisely the kind of
/// off-by-one the builder exists to remove.
#[test]
fn stated_pixel_coordinates_convert_exactly_at_the_eighths() {
    assert_eq!(px_frac(0.0), 0);
    assert_eq!(px_frac(1.0), 65536);
    assert_eq!(px_frac(0.75), 49152);
    assert_eq!(px_frac(0.125), 8192);
    assert_eq!(px_frac(-1.5), -98304);
}
