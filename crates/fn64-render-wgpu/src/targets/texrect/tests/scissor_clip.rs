use fn64_render_ir::PhysicalMemoryLayout;

use crate::targets::ColorTargetExtent;

use super::*;

/// A 64x64 RGBA16 colour target, the extent every case below clips
/// against as its *second* bound.
const TARGET_WIDTH: u32 = 64;
const TARGET_HEIGHT: u32 = 64;

fn key() -> ColorTargetKey {
    let layout = PhysicalMemoryLayout::try_new(8 * 1024 * 1024).unwrap();
    ColorTargetKey::try_new(
        layout.address(0x400).unwrap(),
        ColorTargetExtent::try_new(TARGET_WIDTH, TARGET_HEIGHT).unwrap(),
        ColorTargetFormat::Rgba16,
    )
    .unwrap()
}

/// A texrect covering `[left, right) x [top, bottom)` with a texcoord
/// ramp whose endpoints are distinct, so a test can tell whether the
/// ramp moved when the rectangle was clipped.
///
/// The S10.5 endpoints are passed as `f32` pixels because that is the
/// domain `try_from_viewport_and_texcoords` recovers from; `s / 32.0`
/// inverts its own `* 32.0`.
fn draw(left: i32, top: i32, right: i32, bottom: i32, s_end: i16, t_end: i16) -> TexrectDraw {
    TexrectDraw::try_from_viewport_and_texcoords(
        RectViewportPixels {
            left,
            top,
            right,
            bottom,
        },
        [0.0, 0.0],
        [f32::from(s_end) / 32.0, f32::from(t_end) / 32.0],
    )
    .unwrap()
}

fn rectangle(draw: TexrectDraw) -> TargetRectangle {
    TargetRectangle::try_new(draw.left(), draw.top(), draw.width(), draw.height()).unwrap()
}

fn clip(
    draw: TexrectDraw,
    scissor: RdpScissorRect,
) -> Result<ClippedTexrectExtent, TexrectExecutionError> {
    clip_texrect_extent(
        draw,
        scissor,
        TARGET_WIDTH,
        TARGET_HEIGHT,
        key(),
        rectangle(draw),
    )
}

/// A scissor genuinely TIGHTER than the colour target on every edge.
///
/// Hand-derived from the wire layout, not from the code under test.
/// Public libultra's `gDPSetScissor` encodes each coordinate multiplied
/// by four into one of four twelve-bit fields
/// (`/Users/jer/Code/aki-recomp/refs/oot-decomp/include/ultra64/gbi.h:4794-4804`),
/// so a pixel bound is the quarter value divided by four:
///
/// - `ulx = 40` quarter-pixels -> first column `40 / 4 = 10`
/// - `lrx = 240` quarter-pixels -> column limit `240 / 4 = 60`
/// - `uly = 20` quarter-pixels -> first row `20 / 4 = 5`
/// - `lry = 200` quarter-pixels -> row limit `200 / 4 = 50`
///
/// Every bound is strictly inside `0..64`, so a result that matched the
/// target extent instead of this rect would be visibly wrong -- which is
/// the whole point of choosing it. A scissor equal to the target would
/// give the same answer under either precedence and prove nothing.
fn tight_scissor() -> RdpScissorRect {
    RdpScissorRect::from_wire_quarter_pixels(0, 40, 20, 240, 200)
}

/// A scissor genuinely LOOSER than the colour target: 0..512
/// quarter-pixels is 0..128 pixels, twice the target's 64. The target
/// extent must win here, and the tight case above must NOT win, so the
/// pair together pins the precedence rather than one bound happening to
/// be right.
fn loose_scissor() -> RdpScissorRect {
    RdpScissorRect::from_wire_quarter_pixels(0, 0, 0, 512, 512)
}

/// Wide-open, exactly the target: 64 pixels = 256 quarter-pixels.
fn exact_scissor() -> RdpScissorRect {
    RdpScissorRect::from_wire_quarter_pixels(0, 0, 0, 256, 256)
}

/// **The replacement for the old "outside the target is refused"
/// assertion.** A full-target rectangle under a tighter scissor is
/// CLIPPED to the scissor, not refused and not clipped to the target.
///
/// Pinned RT64 intersects the scissor and draw rectangles and retains a
/// non-empty intersection rather than rejecting the primitive
/// (`/Users/jer/Code/no-mercy-recompiled/third_party/rt64/src/hle/rt64_rdp.cpp:1214-1223`).
/// fn64's own reference renderer clamps identically at
/// `fn64-render-reference/src/raster/draw.rs:197-203`.
#[test]
fn a_rectangle_under_a_tighter_scissor_is_clipped_to_the_scissor_not_the_target() {
    let rect = draw(0, 0, 64, 64, 64, 64);
    let clipped = clip(rect, tight_scissor()).expect("a clipped rectangle still draws");
    assert_eq!(
        clipped.columns(),
        10..60,
        "hand-derived from ulx=40, lrx=240"
    );
    assert_eq!(clipped.rows(), 5..50, "hand-derived from uly=20, lry=200");
    // Distinguishes the scissor from the target: had the clip used the
    // 64x64 extent, the answer would have been the full 0..64 span.
    assert_ne!(clipped.columns(), 0..TARGET_WIDTH);
    assert_ne!(clipped.rows(), 0..TARGET_HEIGHT);
}

/// The precedence's other half: when the scissor is LOOSER than the
/// target, the target's extent is what bounds the write. Neither bound
/// substitutes for the other -- this case and the tight case above
/// disagree about the answer, so a clip that consulted only one of them
/// fails one of the two.
#[test]
fn a_rectangle_under_a_looser_scissor_is_clipped_to_the_target_extent() {
    // A rectangle overhanging the target on both axes -- exactly the
    // shape the old `OutsideTarget` refusal rejected outright.
    let rect = draw(32, 32, 96, 96, 64, 64);
    let clipped = clip(rect, loose_scissor()).expect("an overhanging rectangle still draws");
    // Offsets are relative to the rectangle's own origin at (32, 32):
    // screen span [32, min(96, 128, 64)) = [32, 64), so offsets 0..32.
    assert_eq!(clipped.columns(), 0..32);
    assert_eq!(clipped.rows(), 0..32);
    // Had the loose scissor won, the span would have run to offset 64.
    assert_ne!(clipped.columns(), 0..64);
}

/// A rectangle fully inside both bounds is untouched -- the clip must
/// not narrow content that nothing asked it to narrow.
#[test]
fn a_rectangle_inside_both_bounds_keeps_its_whole_span() {
    let rect = draw(16, 16, 48, 48, 32, 32);
    let clipped = clip(rect, exact_scissor()).expect("an interior rectangle draws whole");
    assert_eq!(clipped.columns(), 0..32);
    assert_eq!(clipped.rows(), 0..32);
}

/// The quarter-pixel bounds round UP, not down or to nearest.
///
/// `curover` fires on `>= clipxlshift` (angrylion `:2352`), making the
/// high edge exclusive, and the low edge is driven out to `clipxhshift`
/// (`:2351`), so both ends take the ceiling of `quarter / 4`. A scissor
/// at quarter-pixel 41 therefore first admits pixel 11, not 10: pixel 10
/// is only three-quarters covered on its right, and the RDP's clamp
/// pushes the span past it.
///
/// Truncation would give 10 and 60 here, so this case genuinely
/// distinguishes the two roundings; the exact multiples used elsewhere
/// would not.
#[test]
fn a_fractional_scissor_edge_rounds_up_on_both_ends() {
    let rect = draw(0, 0, 64, 64, 64, 64);
    // ulx = 41q -> ceil(41/4) = 11; lrx = 241q -> ceil(241/4) = 61.
    let scissor = RdpScissorRect::from_wire_quarter_pixels(0, 41, 41, 241, 241);
    let clipped = clip(rect, scissor).expect("a fractionally-scissored rectangle draws");
    assert_eq!(clipped.columns(), 11..61);
    assert_eq!(clipped.rows(), 11..61);
    // Truncating division would have produced these instead.
    assert_ne!(clipped.columns(), 10..60);
}

/// **Still refused, and this is the case kept.** A rectangle with no
/// pixel surviving the intersection is named rather than silently
/// written as zero pixels: it is either genuinely off-screen or the
/// scissor is degenerate, and both are worth surfacing.
#[test]
fn a_rectangle_entirely_outside_the_scissor_is_refused_by_name() {
    let rect = draw(0, 0, 8, 8, 8, 8);
    // Scissor starts at pixel 10; the rectangle ends at pixel 8.
    let error = clip(rect, tight_scissor()).expect_err("nothing survives the intersection");
    assert!(
        matches!(error, TexrectExecutionError::ScissoredAway { .. }),
        "expected ScissoredAway, got {error:?}"
    );
}

/// **The refusal fires on EITHER axis alone, not only on both.**
///
/// The two degenerate cases above are empty on X *and* Y, so a check
/// that consulted only one axis would pass them both -- exactly the
/// coincident-fixture trap. These two cases are empty on one axis while
/// the other still has a healthy span, so each one fails if its axis is
/// dropped from the emptiness test.
#[test]
fn an_extent_empty_on_only_the_x_axis_is_still_refused() {
    // X: rect 0..8 vs scissor first column 10 -> empty.
    // Y: rect 0..64 vs scissor rows 5..50 -> 45 rows survive.
    let rect = draw(0, 0, 8, 64, 8, 64);
    let error = clip(rect, tight_scissor()).expect_err("an empty X span admits nothing");
    assert!(
        matches!(error, TexrectExecutionError::ScissoredAway { .. }),
        "expected ScissoredAway, got {error:?}"
    );
}

#[test]
fn an_extent_empty_on_only_the_y_axis_is_still_refused() {
    // Y: rect 0..4 vs scissor first row 5 -> empty.
    // X: rect 0..64 vs scissor columns 10..60 -> 50 columns survive.
    let rect = draw(0, 0, 64, 4, 64, 4);
    let error = clip(rect, tight_scissor()).expect_err("an empty Y span admits nothing");
    assert!(
        matches!(error, TexrectExecutionError::ScissoredAway { .. }),
        "expected ScissoredAway, got {error:?}"
    );
}

/// A reversed scissor -- `lrx < ulx` -- is likewise refused rather than
/// producing a backwards span. The RDP latches whatever four values
/// arrive (`rdp_set_scissor` performs no ordering check at
/// `rasterizer.c:2779-2784`), so the degeneracy has to be caught at
/// clip time, which is where this catches it.
#[test]
fn a_reversed_scissor_is_refused_rather_than_producing_a_backwards_span() {
    let rect = draw(0, 0, 64, 64, 64, 64);
    let reversed = RdpScissorRect::from_wire_quarter_pixels(0, 200, 200, 40, 40);
    let error = clip(rect, reversed).expect_err("a reversed rect admits nothing");
    assert!(
        matches!(error, TexrectExecutionError::ScissoredAway { .. }),
        "expected ScissoredAway, got {error:?}"
    );
}

/// A rectangle entirely past the colour target's end is refused too --
/// the target bound is a real invariant even though the scissor is not
/// the thing rejecting it. fn64's target is a sized buffer, and a write
/// past it is a defect rather than content.
#[test]
fn a_rectangle_entirely_past_the_target_extent_is_refused_by_name() {
    let rect = draw(80, 80, 96, 96, 16, 16);
    let error = clip(rect, loose_scissor()).expect_err("nothing survives the target bound");
    assert!(
        matches!(error, TexrectExecutionError::ScissoredAway { .. }),
        "expected ScissoredAway, got {error:?}"
    );
}

/// **The texture ramp must NOT slide when the rectangle is clipped.**
///
/// fn64 evaluates S/T from offsets relative to the unclipped rectangle,
/// so clipping changes only the surviving screen extent and does not
/// rebase the texture ramp. This is fn64's own reading of the rule and
/// is not independently confirmed against an allowed hardware reference.
///
/// This is why the clip returns OFFSETS into the rectangle rather than
/// a narrowed `TexrectDraw`: rebasing the ramp onto the clipped left
/// edge would slide the texture sideways by the clipped amount. The
/// case is chosen so the two answers differ -- at clipped offset 10 the
/// correct S is 10, while a rebased ramp would sample 0 there.
#[test]
fn clipping_does_not_move_the_texture_coordinate_ramp() {
    // 64 pixels wide, S running 0..64 in S10.5 raw units: one raw unit
    // per pixel, so `s_at(n) == n` exactly.
    let rect = draw(0, 0, 64, 64, 64, 64);
    let clipped = clip(rect, tight_scissor()).unwrap();
    let first = clipped.columns().start;
    assert_eq!(first, 10, "the tight scissor's own first column");
    // Sampled at the clipped offset, which is the offset from the
    // UNCLIPPED origin -- so the first drawn pixel reads texel 10.
    assert_eq!(rect.s_at(first), 10);
    assert_eq!(rect.t_at(clipped.rows().start), 5);
    // A rebased ramp would have read texel 0 at the first drawn pixel.
    assert_ne!(rect.s_at(first), 0);
}

/// The mode field survives the latch and is not consulted by the clip.
/// Carried so a reader can see it was decoded rather than dropped; the
/// progressive full-frame path this executor serves draws every
/// scanline. Ignoring the mode during clipping is fn64's own policy and
/// is not independently confirmed against an allowed hardware reference.
#[test]
fn the_scissor_mode_field_round_trips_and_does_not_change_the_clip() {
    let rect = draw(0, 0, 64, 64, 64, 64);
    let plain = RdpScissorRect::from_wire_quarter_pixels(0, 40, 20, 240, 200);
    let interlaced = RdpScissorRect::from_wire_quarter_pixels(3, 40, 20, 240, 200);
    assert_eq!(plain.mode(), 0);
    assert_eq!(interlaced.mode(), 3);
    assert_eq!(clip(rect, plain).unwrap(), clip(rect, interlaced).unwrap());
}

/// Each of the four coordinates reaches its own axis and end. A clip
/// that transposed X and Y, or swapped the two ends of one axis, would
/// pass every symmetric fixture above; this one is deliberately
/// asymmetric in all four values so no pair coincides.
#[test]
fn each_scissor_coordinate_drives_its_own_axis_and_end() {
    let rect = draw(0, 0, 64, 64, 64, 64);
    // ulx=4q->1, uly=12q->3, lrx=180q->45, lry=100q->25. All distinct.
    let scissor = RdpScissorRect::from_wire_quarter_pixels(0, 4, 12, 180, 100);
    let clipped = clip(rect, scissor).expect("an asymmetric scissor still draws");
    assert_eq!(clipped.columns(), 1..45);
    assert_eq!(clipped.rows(), 3..25);
}
