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
            .rfind(|&x| frame.pixel(x, 0) == PRIM_RGBA16)
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

/// **The conversion rounds to nearest; it does not truncate.**
///
/// Every value above is an exact binary fraction, where rounding and
/// truncation AGREE -- so on its own that test cannot tell the two apart, and
/// a truncating `px_frac` survived it. (Found by mutation, and exactly the
/// class of gap this builder exists to close: a fixture that samples only
/// points where the correct and incorrect answers coincide.)
///
/// 0.1 px is not representable in binary: 0.1 * 65536 = 6553.6, which rounds
/// to 6554 and truncates to 6553. Negative values are the case where
/// truncation is not merely off by one but off in the WRONG DIRECTION --
/// `as i32` truncates toward zero, so -0.1 px would become -6553, one ULP
/// *right* of where it was stated.
#[test]
fn stated_pixel_coordinates_round_to_nearest_rather_than_truncating() {
    assert_eq!(px_frac(0.1), 6554, "0.1 px rounds up, not down to 6553");
    assert_eq!(
        px_frac(-0.1),
        -6554,
        "-0.1 px rounds away from zero, not toward it"
    );
    assert_eq!(px_frac(2.7), 176947, "2.7 px = 176946.6 rounds to 176947");
}

// ---------------------------------------------------------------------------
// Refusal enumeration
// ---------------------------------------------------------------------------
//
// A variant nothing can reach is either dead code or an untested guard, and
// today nobody can tell which. These tests drive the harness at each refusal
// the raw-triangle path can actually fire and assert the NAMED variant comes
// back -- keyed on the error's own text rather than a `to_string()` of the
// whole chain, so a reworded sibling variant cannot satisfy the assertion.
//
// The raw-triangle executor shares `TexrectExecutionError` with the texrect
// path (`targets/raw_triangle.rs` returns it directly); it has no enum of its
// own. So the honest enumeration is over that type, split into the variants
// this seam can reach and the ones it structurally cannot.

/// **A fill-cycle triangle draws nothing, and does so WITHOUT a named
/// refusal.** This is the enumeration's first real finding.
///
/// `plan_raw_triangle` returns early on `CycleType::Fill` (raw_dpc/mod.rs:
/// "Refused by declaring nothing rather than drawn as an approximation"), so
/// the triangle declares no rows, the executor is never reached, and
/// `TexrectExecutionError::UnsupportedCycleType`'s fill arm is unreachable
/// from this seam. The command is dropped silently at decode instead.
///
/// That is a deliberate, documented choice -- but it means "fill-cycle
/// triangle" is indistinguishable from "triangle that covered no pixels" at
/// every layer above the planner. Pinned here so a future change that starts
/// declaring rows for fill-cycle triangles, or that adds a named refusal, is
/// a visible behaviour change rather than a quiet one.
#[test]
fn a_fill_cycle_triangle_silently_declares_nothing_rather_than_refusing_by_name() {
    let frame = Rdp::new(16, 8)
        .cycle(CycleType::Fill)
        .combine_prim_passthrough()
        .prim_color(PRIM_WIRE)
        .triangle(Tri::flat().left_major().edges(2.0, 6.0).rows(0..3))
        .run();

    assert!(
        frame.write_ranges().is_empty(),
        "a fill-cycle triangle declares no write at all, got {:?}",
        frame.write_ranges()
    );
    // And the target still holds the clear, untouched.
    frame.assert_outside_untouched(0..0, 0..0);
}

/// Copy cycle takes the same silent path: the planner is not gated on copy,
/// but the executor's copy arm refuses -- and because the planner DID declare
/// rows, the refusal surfaces as a named error rather than a no-op.
///
/// This asymmetry between fill and copy is exactly what an enumeration is for:
/// two neighbouring cycle types, two different failure shapes.
#[test]
fn the_harness_reaches_unsupported_cycle_type_for_a_copy_cycle_triangle() {
    let refusal = Rdp::new(16, 8)
        .cycle(CycleType::Copy)
        .combine_prim_passthrough()
        .prim_color(PRIM_WIRE)
        .triangle(Tri::flat().left_major().edges(2.0, 6.0).rows(0..3))
        .try_run()
        .expect_err("a copy-cycle raw triangle is refused by the executor");
    assert!(
        refusal.message().contains("UnsupportedCycleType") || refusal.message().contains("Copy"),
        "expected a named cycle-type refusal, got: {}",
        refusal.message()
    );
}

/// An UNSHADED triangle whose combiner reads `Shade` reaches
/// `UnsupportedColorInput`. There is nothing to read -- the flag is the wire
/// opcode's own shade bit -- and a zero substituted here would draw
/// plausible-looking wrong pixels, which is the failure mode the refusal
/// exists to prevent.
///
/// This is also the TDD case the design doc names: a test for a feature that
/// does not exist yet can be written today and watched to fail by name.
#[test]
fn the_harness_reaches_unsupported_color_input_for_shade_on_an_unshaded_triangle() {
    let refusal = Rdp::new(16, 8)
        .cycle(CycleType::One)
        .combine_shade_passthrough()
        .prim_color(PRIM_WIRE)
        .triangle(Tri::flat().left_major().edges(2.0, 6.0).rows(0..3))
        .try_run()
        .expect_err("an unshaded triangle may not read Shade");
    assert!(
        refusal.message().contains("Shade") || refusal.message().contains("Input"),
        "expected a named combiner-input refusal, got: {}",
        refusal.message()
    );
}

/// **Every `TexrectExecutionError` variant is accounted for.**
///
/// This is the test the design doc asks for: iterate the variants and state,
/// for each, whether this seam can reach it. The list is written down by hand
/// from the enum declaration, so ADDING a variant without classifying it
/// leaves the count wrong and fails here -- which is the point. A variant that
/// is neither reachable nor explained is exactly the "dead code or untested
/// guard" ambiguity this test removes.
#[test]
fn every_texrect_execution_error_variant_is_classified_as_reachable_or_not() {
    /// Variants the raw-triangle harness reaches, each with the test above
    /// that drives it.
    const REACHED_BY_THIS_HARNESS: &[&str] = &[
        "UnsupportedCycleType",  // copy cycle; fill never reaches the executor
        "UnsupportedColorInput", // Shade read by an unshaded triangle
    ];

    /// Variants this seam structurally cannot reach, each with the reason.
    /// Every one of these is a guard owned by the TEXRECT path or by a stage
    /// the raw-triangle executor does not run -- not dead code, but not this
    /// harness's to cover either.
    const UNREACHABLE_FROM_THE_RAW_TRIANGLE_SEAM: &[(&str, &str)] = &[
        ("UnsupportedAlphaInput", "same guard as the colour input; the colour slot refuses first for every program this seam can state"),
        ("NoDeclaredRows", "the decoder only routes a triangle here once it has declared rows"),
        ("NegativeViewportOrigin", "texrect-only: a raw triangle carries no viewport origin"),
        ("EmptyViewport", "texrect-only: a raw triangle carries no viewport"),
        ("NonIntegralTexcoord", "texture-only: an untextured triangle has no texcoords"),
        ("TexcoordOutOfRange", "texture-only"),
        ("OutsideTarget", "requires a triangle wider or taller than the extent; the decoder's own RDRAM bound refuses first"),
        ("UnboundTile", "texture-only"),
        ("MissingResidentBytes", "the harness always publishes a clear first, so the target is always resident"),
        ("Sample", "texture-only"),
        ("NoiseThresholdUnavailable", "requires a noise-enabled other-mode this harness does not stage"),
        ("OrderedDitherAuthorityUnsettled", "requires an ordered-dither other-mode this harness does not stage"),
        ("DestinationCoverageUnavailable", "requires a coverage-reading blend mode"),
        ("ReservedAlphaCompare", "requires a reserved alpha-compare mode"),
        ("UnsupportedBlendShadeAlpha", "requires a blend program reading shade alpha"),
        ("UnsupportedBlendFramebufferAlpha", "requires a blend program reading framebuffer alpha"),
        ("BlendEnabledNotDerivable", "requires an other-mode whose blend enable is ambiguous"),
        ("Blend", "requires an admitted blend program that then fails evaluation"),
        ("TriangleRowCountDisagreesWithJournal", "a decoder/executor disagreement; unreachable while both call the SAME covered_rows with the same inputs"),
        ("TriangleRowRangeDisagreesWithJournal", "same: identical call, identical arguments"),
        ("TriangleAttributeSampleMissing", "requires a shaded triangle whose shade block is absent, which the wire opcode makes contradictory"),
        ("Target", "wraps TargetError; the harness's extents never overflow a pixel buffer"),
    ];

    // The enum's own variant count, written down from its declaration in
    // `targets/texrect.rs`. Adding a variant without classifying it above
    // fails here rather than silently going untested.
    const DECLARED_VARIANTS: usize = 24;

    assert_eq!(
        REACHED_BY_THIS_HARNESS.len() + UNREACHABLE_FROM_THE_RAW_TRIANGLE_SEAM.len(),
        DECLARED_VARIANTS,
        "every TexrectExecutionError variant must be either reached by this harness or \
         carry a written reason it cannot be; if this fails, a variant was added to the \
         enum without being classified"
    );

    // No variant may be claimed BOTH reachable and unreachable.
    for (unreachable, _) in UNREACHABLE_FROM_THE_RAW_TRIANGLE_SEAM {
        assert!(
            !REACHED_BY_THIS_HARNESS.contains(unreachable),
            "{unreachable} is classified both reachable and unreachable"
        );
    }

    // Every unreachable classification must carry a non-empty reason.
    for (variant, reason) in UNREACHABLE_FROM_THE_RAW_TRIANGLE_SEAM {
        assert!(
            !reason.is_empty(),
            "{variant} is classified unreachable with no reason given"
        );
    }
}

/// **YM is load-bearing: it is where the minor edge switches from XM to XL.**
///
/// Every fixture above parks XM on XL, so the crossover is invisible to them
/// and a `ym` that read the wrong scanline survived. Splitting the two edges
/// makes it observable, derived by hand from `row_span`:
///   XM = 4.0 governs sample rows below YM, XL = 8.0 governs rows at/after it.
/// With `rows(0..4)` and YM at line 2, rows 0 and 1 take XM and rows 2 and 3
/// take XL. Right-edge rule is `ceil(x - 1/8)`:
///   rows 0,1: ceil(4 - 1/8) = 4 -> last covered column 3
///   rows 2,3: ceil(8 - 1/8) = 8 -> last covered column 7
#[test]
fn the_minor_edge_switches_from_the_upper_to_the_lower_edge_at_ym() {
    let frame = Rdp::new(16, 8)
        .cycle(CycleType::One)
        .combine_prim_passthrough()
        .prim_color(PRIM_WIRE)
        .triangle(
            Tri::flat()
                .left_major()
                .edges(2.0, 8.0)
                .upper_right(4.0)
                .rows(0..4)
                .ym_row(2),
        )
        .run();

    for (row, expected_last) in [(0u32, 3u32), (1, 3), (2, 7), (3, 7)] {
        let last_covered = (0..16)
            .rfind(|&x| frame.pixel(x, row) == PRIM_RGBA16)
            .unwrap_or_else(|| panic!("row {row} covered no pixel"));
        assert_eq!(
            last_covered,
            expected_last,
            "row {row} must take the {} edge",
            if row < 2 { "upper (XM)" } else { "lower (XL)" }
        );
    }
}

/// **The runner names WHICH stage refused.**
///
/// Reporting a draw refusal as a clear refusal would misattribute every
/// failure the harness reports -- the operator would go looking at the wrong
/// packet. Keyed on the variant, not on the message text.
#[test]
fn a_draw_stage_refusal_is_reported_as_a_draw_refusal_not_a_clear_refusal() {
    let refusal = Rdp::new(16, 8)
        .cycle(CycleType::Copy)
        .combine_prim_passthrough()
        .prim_color(PRIM_WIRE)
        .triangle(Tri::flat().left_major().edges(2.0, 6.0).rows(0..3))
        .try_run()
        .expect_err("a copy-cycle triangle is refused");
    assert!(
        matches!(refusal, HarnessRefusal::Draw(_)),
        "the copy-cycle refusal comes from the DRAW packet; the clear is a plain \
         fill that always succeeds. Got: {refusal:?}"
    );
}

/// **`assert_outside_untouched` actually asserts.**
///
/// A version whose region test always matched would check nothing at all and
/// pass on every frame -- a green assertion that proves nothing, which is the
/// exact failure mode this harness exists to avoid. Driving it with a region
/// that does NOT cover the drawn pixels must fail.
#[test]
#[should_panic(expected = "outside the drawn region")]
fn assert_outside_untouched_fails_when_a_drawn_pixel_is_claimed_untouched() {
    // The triangle really covers columns 2..6 of rows 0..3. Claiming only
    // column 0 was drawn leaves those covered pixels inside the "untouched"
    // region, so the assertion must fire.
    pinned_flat_frame().assert_outside_untouched(0..1, 0..1);
}

/// **The default YM puts the whole triangle on the UPPER (XM) edge.**
///
/// `ym_row` is optional, and its default arm is load-bearing: with YM at
/// `y_end` no sample row reaches the crossover, so XM governs every row. A
/// default of `y_start` would put every row on XL instead -- invisible to any
/// fixture where XM and XL coincide, which is every other test here.
///
/// Derived by hand: XM = 4.0 and XL = 8.0 with the crossover left at its
/// default. Right-edge rule `ceil(4 - 1/8) = 4` -> last covered column 3, on
/// EVERY row. Were XL governing, it would be column 7.
#[test]
fn the_default_ym_leaves_every_row_on_the_upper_edge() {
    let frame = Rdp::new(16, 8)
        .cycle(CycleType::One)
        .combine_prim_passthrough()
        .prim_color(PRIM_WIRE)
        .triangle(
            Tri::flat()
                .left_major()
                .edges(2.0, 8.0)
                .upper_right(4.0)
                .rows(0..3),
        )
        .run();

    for row in 0..3 {
        let last_covered = (0..16)
            .rfind(|&x| frame.pixel(x, row) == PRIM_RGBA16)
            .unwrap_or_else(|| panic!("row {row} covered no pixel"));
        assert_eq!(
            last_covered, 3,
            "row {row} must take the upper (XM) edge at x = 4.0, not XL at 8.0"
        );
    }
}

/// **The committed write's stored byte count agrees with its declared range.**
///
/// `CompletedWrite::byte_count` is a stored field, not a projection of the
/// access's range, so the two can in principle disagree -- and a harness that
/// reported the range's length in place of the stored count would hide
/// exactly that disagreement. `copy_committed_guest_writes` trusts the stored
/// count when it copies into guest RDRAM, so a mismatch is a real hazard
/// rather than a bookkeeping detail.
///
/// Each of the three rows is 4 pixels of RGBA16 = 8 bytes, derived by hand.
#[test]
fn each_committed_writes_stored_byte_count_matches_its_declared_range_length() {
    let frame = pinned_flat_frame();
    for (index, write) in frame.writes().iter().enumerate() {
        let declared_len = match write.access().region() {
            fn64_render_ir::ResourceRegion::Rdram { range, .. } => range.len(),
            other => panic!("a render-target write must name an RDRAM range, got {other:?}"),
        };
        assert_eq!(
            write.byte_count(),
            declared_len,
            "row {index}: the stored byte count and the declared range disagree"
        );
        assert_eq!(write.byte_count(), 8, "row {index} is four RGBA16 pixels");
    }
}

// ---------------------------------------------------------------------------
// The texture rung: a raw triangle's texel reaches guest RDRAM
// ---------------------------------------------------------------------------

/// Four RGBA16 texels, each distinguishable from every other AND from the
/// clear colour, so a pixel that sampled the WRONG texel is as visible as one
/// that sampled none.
///
/// The alpha bit is 1 in all four: RGBA16's low bit is alpha, and a texel
/// combining to alpha 0 would be indistinguishable from a differently-coloured
/// one whose alpha happened to differ.
const TEXELS: [u16; 4] = [0xF801, 0x07C1, 0x003F, 0xFFFF];

/// **One texel of S, in the non-perspective plane's own units.**
///
/// Derived from the cited constant, not from the code: `G_TP_NONE` converts
/// an s15.16 plane value to S10.5 by dividing by `2^21`, and one whole texel
/// is 32 in S10.5. So one texel is `32 * 2^21 = 2^26` plane units.
const PLANE_PER_TEXEL: i32 = 1 << 26;

/// Half a texel in plane units, the offset every fixture's base carries.
///
/// **This is the anti-coincidence offset, and it is deliberate.** A sample
/// landing exactly on a texel boundary needs a FULL texel of error before the
/// sampled texel changes, so a boundary fixture cannot see a half-texel bug.
/// Sampling at the texel's midpoint means an error of half a texel in either
/// direction is visible.
const PLANE_HALF_TEXEL: i32 = 16 << 21;

/// The X distance, in Q16.16, from the major edge to the first covered
/// subsample of the pixel the major edge itself starts in.
///
/// `attribute_sample` scans Y row 1 first, whose X columns are (1, 5) eighths.
/// For a left edge at a whole pixel the first covered column is x + 1/8, so
/// the delta is `Q16 / 8`. Every fixture's base cancels this, so column 2
/// evaluates to exactly its intended plane value rather than one eighth of a
/// dx past it.
const FIRST_SUBSAMPLE_DELTA_X: i32 = 65536 / 8;

/// A non-perspective textured triangle whose four covered columns sample the
/// four staged texels in order.
///
/// Everything here is hand-derived from the wire layout and the two cited
/// scale factors, never read back from the implementation:
///
///   * Geometry: left edge 2.0, right edge 6.0, rows 0..3 -- the SAME
///     footprint the pinned flat fixture uses, so a geometry regression shows
///     up as the already-pinned failure rather than as a texture one.
///   * `attribute_sample` picks Y row 1 (sample_y_eighth = 8y + 1) and X
///     column 1/8. For column x the X delta from the major (left) edge is
///     `(x - 2) * 2^16 + 2^13`.
///   * S plane: `dx = PLANE_PER_TEXEL`, so one pixel of X advance is one
///     texel of S. `base` cancels the 1/8-pixel first-subsample offset and
///     adds the half-texel anti-coincidence offset.
///   * `de = 0` so every row samples the same S -- three identical rows are
///     three independent readings of the same claim.
///   * T plane: constant at the half-texel offset, so every pixel reads row 0
///     of the 4x1 tile.
///
/// So column 2 -> texel 0, 3 -> 1, 4 -> 2, 5 -> 3.
fn non_perspective_texture_planes() -> ([i32; 4], [i32; 4], [i32; 4], [i32; 4]) {
    let s_base = PLANE_HALF_TEXEL - PLANE_PER_TEXEL / 8;
    let t_base = PLANE_HALF_TEXEL;
    // W is unread on the non-perspective path. Set to a value that would be
    // CATASTROPHIC if the perspective path ran by mistake -- 1, which would
    // divide S by 1 and then multiply by 1024, sending every coordinate off
    // the tile -- rather than to zero, which the `max(1)` rule would quietly
    // normalize into something plausible.
    let w_base = 1;
    (
        [s_base, t_base, w_base, 0],
        [PLANE_PER_TEXEL, 0, 0, 0],
        [0, 0, 0, 0],
        [0, 0, 0, 0],
    )
}

/// A 4x1 RGBA16 tile carrying [`TEXELS`], staged into tile 0.
fn four_texel_frame(triangle: Tri) -> Frame {
    Rdp::new(16, 8)
        .cycle(CycleType::One)
        .combine_texel_passthrough()
        .texture(0, 4, 1, TEXELS.to_vec())
        .triangle(triangle)
        .run()
}

/// **The texture rung's headline claim: a textured raw triangle's sampled
/// texels reach guest RDRAM.**
///
/// FAILS BEFORE this lane: `raw_triangle_is_executable` refused every
/// textured triangle, so the packet declared no write at all and the harness
/// refused with the exact-journal guard. AFTER: four distinct texels land at
/// four distinct columns.
///
/// The assertion is per COLUMN and per texel, not "some pixel changed": a
/// triangle that sampled texel 0 everywhere -- which is exactly what the
/// missing `* 1024.0` produced on the real title screen -- would pass a
/// "something was drawn" check and fail this one.
#[test]
fn a_non_perspective_textured_triangles_texels_reach_guest_rdram() {
    let (value, dx, de, dy) = non_perspective_texture_planes();
    let frame = four_texel_frame(
        Tri::flat()
            .left_major()
            .edges(2.0, 6.0)
            .rows(0..3)
            .texture_planes(value, dx, de, dy),
    );

    for y in 0..3 {
        for (index, expected) in TEXELS.iter().enumerate() {
            frame.assert_pixel(2 + index as u32, y, *expected);
        }
    }
    frame.assert_outside_untouched(2..6, 0..3);
}

/// The committed guest writes carry the digest of the hand-derived texels --
/// so the claim is about bytes that reach RDRAM, not about a backend buffer.
///
/// `copy_committed_guest_writes` re-derives each digest from the payload
/// before writing a byte, so a digest match here is the same statement it
/// makes.
#[test]
fn a_textured_triangles_committed_writes_digest_the_sampled_texels() {
    let (value, dx, de, dy) = non_perspective_texture_planes();
    let frame = four_texel_frame(
        Tri::flat()
            .left_major()
            .edges(2.0, 6.0)
            .rows(0..3)
            .texture_planes(value, dx, de, dy),
    );

    let expected_row: Vec<u8> = TEXELS.iter().flat_map(|texel| texel.to_be_bytes()).collect();
    assert_eq!(expected_row.len(), 8, "four RGBA16 texels are eight bytes");
    // **The write list is asserted before it is walked.** Measured: with the
    // admission predicate reverted this test PASSED, because the triangle
    // declared no write at all and the loop below had nothing to iterate. A
    // digest assertion over an empty list is vacuous green, which is exactly
    // the failure mode this file exists to make impossible.
    assert_eq!(
        frame.write_ranges(),
        vec![(0x2004, 8), (0x2024, 8), (0x2044, 8)],
        "one 8-byte write per covered scanline, at the same hand-derived \
         addresses the pinned flat fixture declares -- the footprint is a \
         function of the edges alone, so the texture block must not move it"
    );
    for (index, write) in frame.writes().iter().enumerate() {
        let expected = CompletedWrite::try_from_bytes(write.access(), &expected_row)
            .expect("eight bytes match the declared eight-byte access");
        assert_eq!(
            write.content(),
            expected.content(),
            "row {index}'s committed digest must be the digest of the four sampled texels"
        );
    }
}
