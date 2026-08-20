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
    const DECLARED_VARIANTS: usize = 23;

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
/// Derived from angrylion, not from fn64's code: `G_TP_NONE` converts an
/// s15.16 plane value to S10.5 with `ss = s >> 16` (`rasterizer.c:479`) --
/// `tcdiv_nopersp` itself applies no scale -- and one whole texel is 32 in
/// S10.5 (`*S = locs >> 5`, `tcoord.c:143`). So one texel is
/// `32 * 2^16 = 2^21` plane units.
///
/// **This was `2^26`, derived from the premise that `2^21` is the
/// plane->S10.5 divisor.** That premise was the `PLANE_TO_TEXEL` defect: the
/// `2^5` from S10.5 to texels was counted twice, once there and once in the
/// sampler. Both are corrected together; see `PLANE_TO_TEXEL`'s own doc for
/// the parity measurement that caught it.
const PLANE_PER_TEXEL: i32 = 1 << 21;

/// Half a texel in plane units, the offset every fixture's base carries.
///
/// **This is the anti-coincidence offset, and it is deliberate.** A sample
/// landing exactly on a texel boundary needs a FULL texel of error before the
/// sampled texel changes, so a boundary fixture cannot see a half-texel bug.
/// Sampling at the texel's midpoint means an error of half a texel in either
/// direction is visible.
const PLANE_HALF_TEXEL: i32 = PLANE_PER_TEXEL / 2;

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

    let expected_row: Vec<u8> = TEXELS
        .iter()
        .flat_map(|texel| texel.to_be_bytes())
        .collect();
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

/// A constant W for the perspective fixtures. `2^20`, chosen so that one
/// texel of S is `32 * W / 32768 = W / 1024 = 1024` -- a round number in
/// plane units that leaves every derived S comfortably inside `i32`.
const PERSPECTIVE_W: i32 = 1 << 20;

/// The S plane value that samples S10.5 coordinate `s10_5` at [`PERSPECTIVE_W`].
///
/// Inverts the CITED perspective rule -- `s10_5 = (S / |W|) * 2^15` -- rather
/// than reading anything back from the implementation.
///
/// **The scale here was `1024` and that made this fixture circular.** It was
/// derived by inverting fn64's own constant, so it asserted the
/// implementation against itself and passed under a scale that is 32x short
/// of the hardware's. The value is now angrylion's: `tcdiv_persp`
/// (`src/core/n64video/rdp/tcoord.c:1027`) returns `(ss/sw) * 2^15` into a
/// field whose five fractional bits `texture_pipeline_cycle` reads as
/// `sfrac = sss1 & 0x1f` (`tex.c:182`). See
/// `docs/RT64-WM2000-COMBINER-CENSUS.md` and
/// `the_perspective_scale_matches_angrylions_tcdiv_persp`.
///
/// Everything these fixtures actually CLAIM -- that the divide happens, that
/// the two paths differ, that W's magnitude is used -- is unchanged and
/// still asserted; only the constant the expectation is built from moved.
const fn perspective_s_for(s10_5: i32) -> i32 {
    s10_5 * (PERSPECTIVE_W / 32768)
}

/// The perspective twin of [`non_perspective_texture_planes`]: the same four
/// columns sampling the same four texels, through the `* 32768.0` path.
///
/// W is held CONSTANT across the triangle, so the divide is exact and the
/// expected texels are hand-computable. A varying W is a different claim (the
/// divide is per pixel) and belongs in its own fixture.
///
/// One texel of S is `32 * W / 32768 = W / 1024`, so `dx = W / 1024` advances
/// one texel per pixel of X, and the base cancels the 1/8-pixel
/// first-subsample offset and adds the half-texel anti-coincidence offset.
fn perspective_texture_planes() -> ([i32; 4], [i32; 4], [i32; 4], [i32; 4]) {
    let per_texel = PERSPECTIVE_W / 1024;
    let s_base = perspective_s_for(16) - per_texel / 8;
    let t_base = perspective_s_for(16);
    (
        [s_base, t_base, PERSPECTIVE_W, 0],
        [per_texel, 0, 0, 0],
        [0, 0, 0, 0],
        [0, 0, 0, 0],
    )
}

/// **The perspective path -- the one WM2000's own triangles take.**
///
/// FAILS BEFORE this lane (no texture rung at all). It also fails if the
/// `* 32768.0` factor is dropped or shrunk: without it every S10.5
/// coordinate here collapses to well under one texel and all four columns
/// sample texel 0, which is precisely the "uniform field" the reference
/// recorded on the real title screen -- and, at the 1024 this constant used
/// to hold, precisely the one-texel-per-triangle flatness measured in
/// WM2000 gameplay (`docs/RT64-WM2000-COMBINER-CENSUS.md`).
#[test]
fn a_perspective_textured_triangle_divides_by_w_and_scales_by_2_pow_15() {
    let (value, dx, de, dy) = perspective_texture_planes();
    let frame = Rdp::new(16, 8)
        .cycle(CycleType::One)
        .texture_perspective()
        .combine_texel_passthrough()
        .texture(0, 4, 1, TEXELS.to_vec())
        .triangle(
            Tri::flat()
                .left_major()
                .edges(2.0, 6.0)
                .rows(0..3)
                .texture_planes(value, dx, de, dy),
        )
        .run();

    for y in 0..3 {
        for (index, expected) in TEXELS.iter().enumerate() {
            frame.assert_pixel(2 + index as u32, y, *expected);
        }
    }
}

/// **The two paths are genuinely different, measured rather than assumed.**
///
/// The identical S/T/W planes, drawn once with `G_TP_PERSP` set and once
/// clear, must sample DIFFERENT texels. If they agreed, one of the two scale
/// factors would be unreachable and a mutant swapping them could survive
/// every other test in this file.
///
/// The planes are the perspective fixture's, whose W is `2^20`: through the
/// perspective path column 2 reads texel 0 (by construction), while through
/// `G_TP_NONE` the same S divides by `2^21` to a coordinate well inside
/// texel 0. So the DISTINGUISHING column is the last one, where perspective
/// reads texel 3 and non-perspective still reads texel 0.
#[test]
fn the_perspective_and_non_perspective_paths_sample_different_texels() {
    let (value, dx, de, dy) = perspective_texture_planes();
    let triangle = Tri::flat()
        .left_major()
        .edges(2.0, 6.0)
        .rows(0..3)
        .texture_planes(value, dx, de, dy);
    let frame_for = |perspective: bool| {
        let staged = Rdp::new(16, 8)
            .cycle(CycleType::One)
            .combine_texel_passthrough()
            .texture(0, 4, 1, TEXELS.to_vec())
            .triangle(triangle);
        if perspective {
            staged.texture_perspective()
        } else {
            staged
        }
        .run()
    };

    let perspective = frame_for(true);
    let none = frame_for(false);

    assert_eq!(
        perspective.pixel(5, 0),
        TEXELS[3],
        "the perspective path scales by 2^15, putting column 5 on texel 3"
    );
    assert_eq!(
        none.pixel(5, 0),
        TEXELS[0],
        "G_TP_NONE divides the same plane by 2^21, leaving column 5 inside texel 0"
    );
    assert_ne!(
        perspective.pixel(5, 0),
        none.pixel(5, 0),
        "the two texture paths must not agree, or one scale factor is untested"
    );
}

/// **The `w <= 0` divide uses W's MAGNITUDE, not its signed value.**
///
/// The killing case for the difference between `w.unsigned_abs().max(1)` and
/// a bare `w.max(1)`: a NEGATIVE W of the same magnitude as a positive one
/// must sample the SAME texels, because the sign is discarded before the
/// divide. A `max(1)` would floor the negative denominator at 1, multiplying
/// every coordinate by 2^20 and sending all four columns off the tile.
///
/// This is a mutation-driven test, and it is stated as an equality against
/// the positive-W fixture's own already-pinned texels rather than against a
/// literal, so it cannot drift away from the claim it exists to make.
///
/// Found because the non-fault test below SURVIVED that mutant: asserting a
/// packet completes says nothing about which texel it read. That is this
/// area's recorded failure mode -- a fixture reading the arm at a point where
/// correct and incorrect answers coincide -- and it recurred here.
#[test]
fn a_negative_w_samples_the_same_texels_its_positive_magnitude_does() {
    let (mut value, dx, de, dy) = perspective_texture_planes();
    value[2] = -PERSPECTIVE_W;
    let frame = Rdp::new(16, 8)
        .cycle(CycleType::One)
        .texture_perspective()
        .combine_texel_passthrough()
        .texture(0, 4, 1, TEXELS.to_vec())
        .triangle(
            Tri::flat()
                .left_major()
                .edges(2.0, 6.0)
                .rows(0..3)
                .texture_planes(value, dx, de, dy),
        )
        .run();

    for (index, expected) in TEXELS.iter().enumerate() {
        assert_eq!(
            frame.pixel(2 + index as u32, 0),
            *expected,
            "column {} under W = -2^20 must read the same texel it reads under W = +2^20; \
             the divide takes |W|",
            2 + index
        );
    }
}

/// **`w <= 0` must not fault** -- the rule earned from real WM2000 content
/// (gfx task ~#27), not a hypothetical.
///
/// A perspective triangle crossing the near plane legitimately presents a
/// non-positive W. Real hardware's tcdiv derives 1/w from the operand's top
/// bits with no sign trap: the pixel samples garbage texels and the chip
/// keeps rasterizing. So this triangle must COMPLETE -- writing whatever the
/// magnitude divide produces -- rather than panicking, refusing, or aborting
/// the packet.
///
/// Asserted as "the packet completed and wrote its declared rows", not as a
/// specific texel: the sampled texel IS defined garbage, and pinning it would
/// pin an accident rather than the rule.
#[test]
fn a_non_positive_w_samples_garbage_without_faulting() {
    for w in [0i32, -1, -PERSPECTIVE_W, i32::MIN] {
        let (mut value, dx, de, dy) = perspective_texture_planes();
        value[2] = w;
        let frame = Rdp::new(16, 8)
            .cycle(CycleType::One)
            .texture_perspective()
            .combine_texel_passthrough()
            .texture(0, 4, 1, TEXELS.to_vec())
            .triangle(
                Tri::flat()
                    .left_major()
                    .edges(2.0, 6.0)
                    .rows(0..3)
                    .texture_planes(value, dx, de, dy),
            )
            .try_run()
            .unwrap_or_else(|refusal| {
                panic!("W = {w} must rasterize as defined garbage, not refuse: {refusal:?}")
            });

        assert_eq!(
            frame.write_ranges(),
            vec![(0x2004, 8), (0x2024, 8), (0x2044, 8)],
            "W = {w} must still declare and fill its three covered scanlines"
        );
    }
}

/// A second four-texel palette, disjoint from [`TEXELS`] in every entry, so a
/// pixel that read the wrong LOAD is as visible as one that read the wrong
/// texel within a load.
const SECOND_TEXELS: [u16; 4] = [0x8421, 0x1085 ^ 0xFFFE, 0x7BDF, 0x0421];

/// **The TMEM prefix rule, at the seam the brief names as the hard part.**
///
/// Two triangles in ONE packet with a `LoadBlock` BETWEEN them. The first
/// must sample the first load's texels; the second must sample the second's.
///
/// This is the distinction a per-packet TMEM image cannot make. WM2000's own
/// triangle packets carry NINE loads each, and the identical defect has
/// already shipped once in this crate on the GPU side -- a single projection
/// sealed per `draw_admitted_triangles` call meant WM2000's measured sixth
/// packet drew the seventh sprite seven times. A test that staged both loads
/// BEFORE both triangles would pass with either implementation, because both
/// draws would legitimately observe the last load.
///
/// It also pins the direction: swapping the two assertions fails, so this
/// cannot pass by the two draws merely DIFFERING.
#[test]
fn two_triangles_straddling_a_load_sample_the_texture_each_one_saw() {
    let (value, dx, de, dy) = non_perspective_texture_planes();
    let textured = |rows: std::ops::Range<i16>| {
        Tri::flat()
            .left_major()
            .edges(2.0, 6.0)
            .rows(rows)
            .texture_planes(value, dx, de, dy)
    };
    let frame = Rdp::new(16, 8)
        .cycle(CycleType::One)
        .combine_texel_passthrough()
        // Load #1, then the triangle on rows 0..2, then load #2 into the SAME
        // tile, then the triangle on rows 4..6. Same tile deliberately: a
        // fixture using two different tiles would pass even if the prefix
        // were ignored, because the tile descriptors alone would separate
        // them.
        .texture(0, 4, 1, TEXELS.to_vec())
        .triangle(textured(0..2))
        .texture(0, 4, 1, SECOND_TEXELS.to_vec())
        .triangle(textured(4..6))
        .run();

    for (index, expected) in TEXELS.iter().enumerate() {
        assert_eq!(
            frame.pixel(2 + index as u32, 0),
            *expected,
            "the FIRST triangle precedes the second load and must sample the first"
        );
    }
    for (index, expected) in SECOND_TEXELS.iter().enumerate() {
        assert_eq!(
            frame.pixel(2 + index as u32, 4),
            *expected,
            "the SECOND triangle follows the second load and must sample it"
        );
    }
    // The two palettes are disjoint, so "each saw its own" is a real claim
    // rather than one satisfiable by both draws reading the same thing.
    assert_ne!(
        frame.pixel(2, 0),
        frame.pixel(2, 4),
        "the two loads must be distinguishable at the pixel this test reads"
    );
}

/// **A raw triangle's tile comes from its OWN wire field, not a frozen 0.**
///
/// `RawTriangle::tile()` is wire word 0 bits 18:16 -- a real field, read by
/// the CPU executor this harness drives. `PlanCollector`'s
/// `bound_tile_index` once froze it to 0 for the GPU uniform path, with a
/// comment claiming "it carries no tile field of its own to read"; that
/// claim was wrong and the GPU arm now reads the same field (see
/// `production.rs`'s own
/// `plan_collector_binds_the_tile_a_raw_triangle_s_own_wire_word_names`).
///
/// The fixture puts DIFFERENT texels in tile 0 and tile 5 and points the
/// triangle at tile 5, so an implementation reading tile 0 samples the wrong
/// sprite -- the defect class this crate has already shipped once. Every other
/// texture test here uses tile 0, where the correct and incorrect answers
/// coincide, so this mutant survived until this test existed.
///
/// The two tiles are loaded at DISJOINT TMEM addresses, so the second load
/// cannot overwrite the first and "wrong tile" is distinguishable from
/// "clobbered TMEM".
#[test]
fn a_textured_triangle_samples_the_tile_its_own_wire_word_names() {
    let (value, dx, de, dy) = non_perspective_texture_planes();
    let frame = Rdp::new(16, 8)
        .cycle(CycleType::One)
        .combine_texel_passthrough()
        .texture(0, 4, 1, TEXELS.to_vec())
        .texture_at(5, 4, 1, SECOND_TEXELS.to_vec(), 0x100)
        .triangle(
            Tri::flat()
                .left_major()
                .tile(5)
                .edges(2.0, 6.0)
                .rows(0..3)
                .texture_planes(value, dx, de, dy),
        )
        .run();

    for (index, expected) in SECOND_TEXELS.iter().enumerate() {
        assert_eq!(
            frame.pixel(2 + index as u32, 0),
            *expected,
            "the triangle names tile 5, so column {} must read tile 5's texel, \
             not tile 0's {:#06x}",
            2 + index,
            TEXELS[index]
        );
    }
    assert_ne!(
        SECOND_TEXELS[0], TEXELS[0],
        "the two tiles must differ at the column this test reads"
    );
}

/// **A tile with an ODD T origin still reads back exactly what its load
/// wrote.**
///
/// TMEM's odd rows carry a 4-byte bank exchange, and the reader must land on
/// the same bank the writer used. What decides that bank is the
/// TILE-RELATIVE row and nothing else -- angrylion takes `dswap = sst & 1`
/// on the write side after `TRELATIVE` has made the row tile-relative
/// (`tex.c:583`, `tcoord.c:998-999`) and `(t & 1)` on the read side over the
/// equally tile-relative row, with `fetch_texel` never reading `tile->tl` at
/// all (`tmem.c:63`). See `tmem/read.rs::odd_row_exchange`.
///
/// The T ORIGIN is therefore not part of the rule, and this test's job is to
/// prove that an odd origin does not perturb the round trip. It is the
/// regression guard for the defect fixed in this lane: the LoadBlock writer
/// used to add a `source_t` term the reader answered with a `low_t` term, so
/// the two disagreed and every texel on a disagreeing row came back from the
/// wrong 4-byte half. See `docs/RT64-WM2000-TEXEL-LOCALISATION.md`.
///
/// **What this harness can and cannot stage.** The harness loads every
/// texture with a `line = 1` LoadBlock at `DXT = 0`, and a LoadBlock word
/// lands on TMEM row `(word * dxt) >> 11`, so every word here is written on
/// tile-relative row 0 and none of them takes the write-side exchange. That
/// is a real, physical load shape -- DXT 0 genuinely means "no row advance"
/// -- but it means this fixture cannot exhibit an exchanged row, and a
/// version of it that sampled tile row 1 was asserting something the RDP
/// would not produce either: hardware would read row 1 through the exchange
/// and find bytes the DXT-0 load had written unexchanged.
///
/// So the tile origin here is EVEN and both sampled texels sit on
/// tile-relative row 0. What the test still pins -- and the reason it is
/// worth keeping -- is that a nonzero T origin does not perturb the round
/// trip at all, which is exactly what breaks if an origin term is
/// reintroduced on either side of the exchange. The word-level fixture for a
/// genuinely exchanged row is
/// `tmem::execute::load_block::linear_odd_row_full_word_exchanges_lane_halves`,
/// which reaches row 1 through a nonzero DXT.
///
/// Every other texture fixture in this file uses `low_t = 0`, where a
/// nonzero origin and a zero one coincide, so this is the only place an
/// origin term would show up at the guest-bytes seam.
#[test]
fn a_tile_with_an_odd_t_origin_reads_the_xor4_bank_its_load_wrote() {
    // `low_t = 2` puts the tile's two rows at T = 2 and T = 3, so sampling
    // T = 2 reads the tile's FIRST row -- tile-relative row 0, which is where
    // the DXT-0 load actually wrote every word. The origin is nonzero, so a
    // reader that folded `low_t` into the exchange would flip the bank and
    // return a texel four bytes away; the origin is EVEN, so it also does not
    // accidentally cancel against a writer-side `source_t` term.
    const LOW_T: u32 = 2;
    let t_texel = LOW_T;
    let (mut value, dx, de, dy) = non_perspective_texture_planes();
    value[1] = PLANE_HALF_TEXEL + (t_texel as i32) * PLANE_PER_TEXEL;

    let rows: Vec<u16> = TEXELS
        .iter()
        .copied()
        .chain(SECOND_TEXELS.iter().copied())
        .collect();
    let frame = Rdp::new(16, 8)
        .cycle(CycleType::One)
        .combine_texel_passthrough()
        .texture(0, 4, 2, rows)
        .with_low_t(LOW_T)
        .triangle(
            Tri::flat()
                .left_major()
                .edges(2.0, 6.0)
                .rows(0..3)
                .texture_planes(value, dx, de, dy),
        )
        .run();

    // The first row of the staged image is TEXELS, written unexchanged, so it
    // must read back unexchanged -- asserted per column so a single
    // coincidental match cannot carry the test.
    for (index, expected) in TEXELS.iter().enumerate() {
        assert_eq!(
            frame.pixel(2 + index as u32, 0),
            *expected,
            "column {} must read the tile's first row exactly as its load wrote \
             it; a nonzero T origin is not part of the XOR4 bank rule",
            2 + index
        );
    }
    // The exchange this test guards against would return a texel four bytes
    // away, which within this row is the texel two columns over. Asserting
    // the two rows differ keeps that a real discrimination rather than a
    // coincidence.
    assert_ne!(
        TEXELS[0], SECOND_TEXELS[0],
        "the two staged rows must differ for a wrong-bank read to be visible"
    );
}

/// **An S10.5 coordinate that overflows `i16` SATURATES; it does not wrap.**
///
/// The `w <= 0` rule guarantees the perspective divide never faults, which
/// means it can and does produce coordinates far outside S10.5's range -- so
/// the narrowing to `i16` must have a defined, correct answer for one. The
/// difference is directly visible: a saturated coordinate clamps to the
/// tile's LAST texel, while a wrapped one (`as i32 as i16`) folds back to the
/// FIRST. On a real frame that is the difference between a stretched edge and
/// a tear.
///
/// The fixture drives it there with a tiny positive W (1), the shape a
/// near-plane crossing actually produces. Hand-derived from the cited rule
/// alone: S is 16384 at column 2, so `(S / 1) * 1024 = 16,777,216` in S10.5 --
/// four orders of magnitude past `i16::MAX`. Saturating gives 32767, which
/// the tile's clamp addressing (mask 0) folds to texel 3; wrapping gives
/// `16777216 & 0xffff = 0`, i.e. texel 0.
///
/// Every fixture above keeps its coordinates comfortably in range, which is
/// why a wrapping mutant survived all of them.
#[test]
fn an_overflowing_texture_coordinate_saturates_to_the_last_texel_not_the_first() {
    let (mut value, dx, de, dy) = perspective_texture_planes();
    value[2] = 1;
    let frame = Rdp::new(16, 8)
        .cycle(CycleType::One)
        .texture_perspective()
        .combine_texel_passthrough()
        .texture(0, 4, 1, TEXELS.to_vec())
        .triangle(
            Tri::flat()
                .left_major()
                .edges(2.0, 6.0)
                .rows(0..3)
                .texture_planes(value, dx, de, dy),
        )
        .run();

    for column in 2..6 {
        assert_eq!(
            frame.pixel(column, 0),
            TEXELS[3],
            "column {column}'s coordinate overflows S10.5 and must clamp to the tile's \
             LAST texel {:#06x}, not wrap to its first {:#06x}",
            TEXELS[3],
            TEXELS[0]
        );
    }
}

/// **A triangle taller than its colour target must not declare rows past the
/// target's end.**
///
/// `plan_raw_triangle` bounds its row walk by INSTALLED RDRAM and a 4096-row
/// cap, because `SetColorImage` carries no height and the target extent does
/// not exist at decode time. On a 4MB RDRAM that bound is far looser than the
/// real target, so a triangle whose YL reaches past the target's last row
/// declares byte ranges beyond the target -- and `verify_accesses_inside`
/// refuses the whole PACKET by name before the executor's own row-count guard
/// can be consulted.
///
/// Measured on the real ROM: WM2000 aborted at 280 VI swaps with
/// "FillRectangle access #59 names a range outside its own color target's
/// full extent". The defect is NOT specific to textured triangles -- it was
/// simply unreachable while the decoder refused every triangle WM2000
/// emits.
#[test]
fn a_triangle_taller_than_its_target_does_not_declare_rows_past_the_targets_end() {
    // A target 4 rows tall, and a triangle spanning 8 -- so rows 4..8 exist
    // in the decoder's walk and not in the target.
    let frame = Rdp::new(16, 4)
        .cycle(CycleType::One)
        .combine_prim_passthrough()
        .prim_color(PRIM_WIRE)
        .triangle(Tri::flat().left_major().edges(2.0, 6.0).rows(0..8))
        .run();

    for (start, _len) in frame.write_ranges() {
        let offset = start - TARGET_ADDRESS;
        assert!(
            offset < 16 * 4 * 2,
            "declared write at {start:#x} (offset {offset}) lies past the \
             16x4 RGBA16 target's own 128 bytes"
        );
    }
    // The rows that DO exist are still drawn, so this is not satisfied by
    // declaring nothing at all.
    for y in 0..4 {
        for x in 2..6 {
            frame.assert_pixel(x, y, PRIM_RGBA16);
        }
    }
}
