use fn64_render_ir::PhysicalMemoryLayout;

use super::*;
use crate::state::OtherMode;
use crate::targets::{ColorTargetExtent, ColorTargetKey, ColorTargetRegistry, TargetGeneration};

const RDRAM_BYTES: u32 = 8 * 1024 * 1024;
const FIXTURE_START: u32 = 0x400;

fn layout() -> PhysicalMemoryLayout {
    PhysicalMemoryLayout::try_new(RDRAM_BYTES).unwrap()
}

fn key_at(start: u32, width: u32, height: u32, format: ColorTargetFormat) -> ColorTargetKey {
    let layout = layout();
    ColorTargetKey::try_new(
        layout.address(start).unwrap(),
        ColorTargetExtent::try_new(width, height).unwrap(),
        format,
    )
    .unwrap()
}

/// `high` selects cycle type via bits 20:21 -- `3` (`0x300000`) is Fill,
/// matching `state.rs`'s `cycle_type()` `_ => CycleType::Fill` arm. `low = 0`
/// leaves every bypass-hazard bit (0x10/0x20/0x40) clear.
fn fill_cycle_other_mode() -> OtherMode {
    OtherMode::from_wire(0x0030_0000, 0)
}

fn one_cycle_other_mode() -> OtherMode {
    OtherMode::from_wire(0, 0)
}

fn rect(ulx: u16, uly: u16, lrx: u16, lry: u16) -> crate::FillRectangle {
    crate::FillRectangle::from_wire_fields(ulx, uly, lrx, lry)
}

// --- Tier 3: hand-computed unit fixtures for the 5-bit expansion + coverage ---

#[test]
fn rgba16_pixel_decode_matches_hand_worked_expansion() {
    // fill_color's high halfword covers even x, low halfword covers odd x
    // (draw.rs:152-158: rect.fill_color >> 16 for indices 0/2, as-is for
    // 1/3, i.e. even/odd by x parity).
    // Halfword 0xF801: R=11111 G=00000 B=00000 A=1
    //   R: (0b11111<<3)|(0b11111>>2) = 0xF8|0x07 = 0xFF
    //   G/B: 0
    //   A: bit0 set -> 255
    let fill_color = FillColor::from_wire(0xF801_0000 | 0xF801);
    let even = decode_fill_cycle_pixel(fill_color, ColorTargetFormat::Rgba16, 0);
    assert_eq!(even, Rgba8::new(0xFF, 0x00, 0x00, 255));
    let odd = decode_fill_cycle_pixel(fill_color, ColorTargetFormat::Rgba16, 1);
    assert_eq!(odd, Rgba8::new(0xFF, 0x00, 0x00, 255));
}

#[test]
fn rgba16_pixel_decode_alpha_bit_clear_is_zero_alpha() {
    // Halfword 0x0000: all zero, alpha bit clear -> alpha 0, not 255.
    let fill_color = FillColor::from_wire(0);
    let pixel = decode_fill_cycle_pixel(fill_color, ColorTargetFormat::Rgba16, 0);
    assert_eq!(pixel, Rgba8::new(0, 0, 0, 0));
}

#[test]
fn rgba16_pixel_decode_even_and_odd_use_distinct_halfwords() {
    // High halfword all-1s green, low halfword all-1s blue -- prove x parity
    // selects the correct 16 bits, matching draw.rs:152-158 exactly (not
    // just "some halfword").
    // Halfword G: 0b0_00000_11111_0_0000 = 0x03E0 -> green channel bits 6..10 set
    //   G expand: 0b11111 -> 0xFF; R/B: 0; A: bit0=0 -> 0
    // Halfword B: 0b0_00000_00000_11111_0 -> wait, blue is bits 1..5.
    let green_halfword: u16 = 0x07C0; // R=0 G=0x1f B=0 A=0 (G is bits 6..10)
    let blue_halfword: u16 = 0x003E; // R=0 G=0 B=0x1f A=0 (B is bits 1..5)
    let fill_color =
        FillColor::from_wire((u32::from(green_halfword) << 16) | u32::from(blue_halfword));
    let even = decode_fill_cycle_pixel(fill_color, ColorTargetFormat::Rgba16, 0);
    assert_eq!(even, Rgba8::new(0, 0xFF, 0, 0));
    let odd = decode_fill_cycle_pixel(fill_color, ColorTargetFormat::Rgba16, 1);
    assert_eq!(odd, Rgba8::new(0, 0, 0xFF, 0));
    // x parity, not x itself, selects the halfword.
    let far_even = decode_fill_cycle_pixel(fill_color, ColorTargetFormat::Rgba16, 40);
    assert_eq!(far_even, even);
    let far_odd = decode_fill_cycle_pixel(fill_color, ColorTargetFormat::Rgba16, 41);
    assert_eq!(far_odd, odd);
}

#[test]
fn rgba32_pixel_decode_matches_hand_worked_coverage_unpack() {
    // draw.rs:184-188: [R,G,B,alpha_coverage] = fill_color.to_be_bytes();
    // alpha = expand_five(alpha_coverage & 0x1f) -- coverage bits (>>5)
    // are not part of the decoded pixel color at all (period 1, no
    // coverage-byte tracking in this executor's scope).
    // alpha_coverage = 0b111_11111 = 0xFF -> low5 = 0x1F -> expand -> 0xFF
    let fill_color = FillColor::from_wire(u32::from_be_bytes([0x11, 0x22, 0x33, 0xFF]));
    let pixel = decode_fill_cycle_pixel(fill_color, ColorTargetFormat::Rgba32, 0);
    assert_eq!(pixel, Rgba8::new(0x11, 0x22, 0x33, 0xFF));

    // alpha_coverage = 0b010_00001 = 0x21 -> low5 = 0x01 -> expand ->
    // (1<<3)|(1>>2) = 8|0 = 8
    let fill_color2 = FillColor::from_wire(u32::from_be_bytes([0x00, 0x00, 0x00, 0x21]));
    let pixel2 = decode_fill_cycle_pixel(fill_color2, ColorTargetFormat::Rgba32, 0);
    assert_eq!(pixel2, Rgba8::new(0, 0, 0, 8));

    // period 1: x has no effect on RGBA32.
    let pixel_far = decode_fill_cycle_pixel(fill_color, ColorTargetFormat::Rgba32, 99);
    assert_eq!(pixel_far, pixel);
}

// --- Tier 1: exhaustive differential against an inline-duplicated oracle ---

/// Independently re-derived oracle for `draw_fill_rectangle`'s fill-cycle
/// branch (`fn64-render-reference/src/raster/draw.rs:131-190`), written
/// without calling `decode_fill_cycle_pixel` -- a genuinely separate
/// computation path so this differential can catch a bug in either.
fn oracle_decode_16(halfword: u16) -> Rgba8 {
    let expand = |value: u16| -> u8 {
        let value = value as u8;
        (value << 3) | (value >> 2)
    };
    Rgba8::new(
        expand((halfword >> 11) & 0x1f),
        expand((halfword >> 6) & 0x1f),
        expand((halfword >> 1) & 0x1f),
        if halfword & 1 != 0 { 255 } else { 0 },
    )
}

fn oracle_decode_32(fill_color: u32) -> Rgba8 {
    let [red, green, blue, alpha_coverage] = fill_color.to_be_bytes();
    let alpha = (alpha_coverage & 0x1f) << 3 | (alpha_coverage & 0x1f) >> 2;
    Rgba8::new(red, green, blue, alpha)
}

#[test]
fn rgba16_pixel_decode_matches_oracle_exhaustively() {
    for high in 0..=u16::MAX {
        for low in [0u16, 1, 0x8000, 0xFFFF, high.wrapping_add(1)] {
            let fill_color = FillColor::from_wire((u32::from(high) << 16) | u32::from(low));
            for x in 0..4u32 {
                let expected = oracle_decode_16(if x % 2 == 0 { high } else { low });
                let actual = decode_fill_cycle_pixel(fill_color, ColorTargetFormat::Rgba16, x);
                assert_eq!(actual, expected, "high={high:#06x} low={low:#06x} x={x}");
            }
        }
    }
}

#[test]
fn rgba32_pixel_decode_matches_oracle_exhaustively_over_alpha_coverage_byte() {
    for alpha_coverage in 0..=u8::MAX {
        let fill_color_raw = u32::from_be_bytes([0x12, 0x34, 0x56, alpha_coverage]);
        let fill_color = FillColor::from_wire(fill_color_raw);
        let expected = oracle_decode_32(fill_color_raw);
        let actual = decode_fill_cycle_pixel(fill_color, ColorTargetFormat::Rgba32, 0);
        assert_eq!(actual, expected, "alpha_coverage={alpha_coverage:#04x}");
    }
}

// --- Coordinate resolution ---

#[test]
fn whole_pixel_coordinates_resolve_by_dividing_by_four() {
    // (w1>>12)&0xfff style raw fields, already whole-pixel (low 2 bits 0):
    // 4 raw = 1 pixel; 4*10=40 raw = 10 pixels.
    let rect = resolve_fill_pixel_rectangle(0, 0, 40, 8).unwrap();
    assert_eq!((rect.x0(), rect.y0(), rect.x1(), rect.y1()), (0, 0, 10, 2));
    assert_eq!(rect.width(), 11);
    assert_eq!(rect.height(), 3);
}

#[test]
fn single_pixel_rectangle_is_the_literal_edge_case() {
    let rect = resolve_fill_pixel_rectangle(0, 0, 0, 0).unwrap();
    assert_eq!(rect.width(), 1);
    assert_eq!(rect.height(), 1);
}

#[test]
fn fractional_edge_is_rejected_loudly_not_rounded() {
    let error = resolve_fill_pixel_rectangle(1, 0, 40, 8).unwrap_err();
    assert!(matches!(
        error,
        FillCoordinateError::FractionalEdge {
            field: "upper_left_x",
            raw: 1
        }
    ));
}

#[test]
fn inverted_rectangle_is_rejected_loudly() {
    // lower_right_x (4) < upper_left_x (40): x0=10 > x1=1.
    let error = resolve_fill_pixel_rectangle(40, 0, 4, 8).unwrap_err();
    assert!(matches!(
        error,
        FillCoordinateError::ReversedRectangle {
            x0: 10,
            y0: 0,
            x1: 1,
            y1: 2
        }
    ));
}

#[test]
fn zero_height_degenerate_rectangle_via_equal_y_is_one_row_inclusive() {
    // RDP FillRectangle has no "zero-size" wire encoding for width/height
    // directly -- ulx==lrx and uly==lry is the degenerate/minimal case, and
    // per draw.rs's inclusive lower/right edge it is one pixel, not zero.
    let rect = resolve_fill_pixel_rectangle(40, 40, 40, 40).unwrap();
    assert_eq!(rect.width(), 1);
    assert_eq!(rect.height(), 1);
}

// --- Tier 2 + characterization: real executor against a real target ---

fn other_mode_with_hazard(bit: u32) -> OtherMode {
    OtherMode::from_wire(0x0030_0000, bit)
}

#[test]
fn non_fill_cycle_is_rejected_before_touching_the_target() {
    let registry = ColorTargetRegistry::try_new(layout(), 1).unwrap();
    let key = key_at(FIXTURE_START, 4, 2, ColorTargetFormat::Rgba16);
    let candidate = registry.begin_candidate(key).unwrap();
    let error = execute_fill_rectangle(
        &candidate,
        one_cycle_other_mode(),
        FillColor::from_wire(0xF801_F801),
        rect(0, 0, 12, 4),
        crate::targets::RdpScissorRect::from_wire_quarter_pixels(0, 0, 0, 0xffff, 0xffff),
        None,
    )
    .unwrap_err();
    assert!(matches!(error, FillExecutionError::NotFillCycle));
}

#[test]
fn z_cmp_bypass_hazard_is_rejected_before_touching_the_target() {
    let registry = ColorTargetRegistry::try_new(layout(), 1).unwrap();
    let key = key_at(FIXTURE_START, 4, 2, ColorTargetFormat::Rgba16);
    let candidate = registry.begin_candidate(key).unwrap();
    let error = execute_fill_rectangle(
        &candidate,
        other_mode_with_hazard(0x0010),
        FillColor::from_wire(0xF801_F801),
        rect(0, 0, 12, 4),
        crate::targets::RdpScissorRect::from_wire_quarter_pixels(0, 0, 0, 0xffff, 0xffff),
        None,
    )
    .unwrap_err();
    assert!(matches!(
        error,
        FillExecutionError::UnsafeFillCycleBypass {
            hazards: FillCycleBypassHazards {
                depth_compare: true,
                ..
            }
        }
    ));
}

#[test]
fn z_upd_and_im_rd_bypass_hazards_are_each_rejected() {
    let registry = ColorTargetRegistry::try_new(layout(), 1).unwrap();
    let key = key_at(FIXTURE_START, 4, 2, ColorTargetFormat::Rgba16);
    let candidate = registry.begin_candidate(key).unwrap();
    for (bit, expect_update, expect_read) in [(0x0020, true, false), (0x0040, false, true)] {
        let error = execute_fill_rectangle(
            &candidate,
            other_mode_with_hazard(bit),
            FillColor::from_wire(0xF801_F801),
            rect(0, 0, 12, 4),
            crate::targets::RdpScissorRect::from_wire_quarter_pixels(0, 0, 0, 0xffff, 0xffff),
            None,
        )
        .unwrap_err();
        assert!(matches!(
            error,
            FillExecutionError::UnsafeFillCycleBypass {
                hazards: FillCycleBypassHazards { depth_update, image_read, .. }
            } if depth_update == expect_update && image_read == expect_read
        ));
    }
}

#[test]
fn full_extent_new_target_writes_exact_bytes() {
    let registry = ColorTargetRegistry::try_new(layout(), 1).unwrap();
    let key = key_at(FIXTURE_START, 4, 2, ColorTargetFormat::Rgba16);
    let candidate = registry.begin_candidate(key).unwrap();
    // Fill cycle bypasses the pixel pipeline: the memory interface expands
    // each 16-bit fill-register halfword to an 18-bit framebuffer pixel by
    // replicating its LSB into the hidden coverage bits (Programming Manual
    // §12.8.2). Bit 0 clear therefore remains clear in visible RGBA16, so
    // this deliberately differs from the one-/two-cycle coverage packers.
    let fill_color = FillColor::from_wire(0xF800_F800);
    let completed = execute_fill_rectangle(
        &candidate,
        fill_cycle_other_mode(),
        fill_color,
        rect(0, 0, 12, 4), // (12>>2)=3 -> x1=3, width 4; (4>>2)=1 -> y1=1, height 2
        crate::targets::RdpScissorRect::from_wire_quarter_pixels(0, 0, 0, 0xffff, 0xffff),
        None,
    )
    .unwrap();
    assert_eq!(completed.rectangle().width(), 4);
    assert_eq!(completed.rectangle().height(), 2);
    let bytes = completed.device_bytes().device_bytes();
    assert_eq!(bytes, [0xF8, 0x00].repeat(8));

    let initialized = candidate.admit_completed_initialization(completed).unwrap();
    assert_eq!(initialized.initialized_region().rows(), 2);
}

#[test]
fn resident_sub_rectangle_write_patches_only_the_claimed_rows_exact_bytes() {
    let mut registry = ColorTargetRegistry::try_new(layout(), 1).unwrap();
    let key = key_at(FIXTURE_START, 4, 2, ColorTargetFormat::Rgba16);

    // Seed a resident 4x2 target, all red (0xF801 per pixel).
    let candidate = registry.begin_candidate(key).unwrap();
    let seed = execute_fill_rectangle(
        &candidate,
        fill_cycle_other_mode(),
        FillColor::from_wire(0xF801_F801),
        rect(0, 0, 12, 4),
        crate::targets::RdpScissorRect::from_wire_quarter_pixels(0, 0, 0, 0xffff, 0xffff),
        None,
    )
    .unwrap();
    let initialized = candidate.admit_completed_initialization(seed).unwrap();
    registry.commit_initialized(initialized).unwrap();
    assert_eq!(
        registry.residents()[0].generation(),
        TargetGeneration::FIRST
    );
    let resident_bytes_before = registry.residents()[0]
        .device_bytes()
        .device_bytes()
        .to_vec();
    assert_eq!(resident_bytes_before, [0xF8, 0x01].repeat(8));

    // Sub-rectangle write: only row 1 (y in [1,1]), all green
    // (halfword 0x07C1: G=31, A=1 -> expand G=0xFF, repack -> 0x07C1).
    let candidate2 = registry.begin_candidate(key).unwrap();
    assert_eq!(candidate2.predecessor(), Some(TargetGeneration::FIRST));
    let completed = execute_fill_rectangle(
        &candidate2,
        fill_cycle_other_mode(),
        FillColor::from_wire(0x07C1_07C1),
        rect(0, 4, 12, 4),
        crate::targets::RdpScissorRect::from_wire_quarter_pixels(0, 0, 0, 0xffff, 0xffff), // uly raw 4 -> y0=1; lry raw 4 -> y1=1 (single row)
        Some(&resident_bytes_before),
    )
    .unwrap();
    assert_eq!(completed.rectangle().y(), 1);
    assert_eq!(completed.rectangle().height(), 1);
    let patched = completed.device_bytes().device_bytes();
    // Row 0 unchanged (still red), row 1 patched to green.
    assert_eq!(&patched[0..8], [0xF8, 0x01].repeat(4).as_slice());
    assert_eq!(&patched[8..16], [0x07, 0xC1].repeat(4).as_slice());

    let initialized2 = candidate2
        .admit_completed_initialization(completed)
        .unwrap();
    let resident = registry.commit_initialized(initialized2).unwrap();
    assert_eq!(resident.generation(), TargetGeneration(2));
    assert_eq!(
        resident.device_bytes().device_bytes(),
        [
            [0xF8, 0x01].repeat(4).as_slice(),
            [0x07, 0xC1].repeat(4).as_slice(),
        ]
        .concat()
    );
}

#[test]
fn resident_candidate_without_resident_bytes_is_rejected_not_zero_filled() {
    // A resident candidate (predecessor.is_some()) with no resident_bytes
    // supplied must be rejected, not silently treated as a fresh
    // (zero-filled) target -- that would discard every row outside the
    // claimed rectangle, a silent data-loss shrug rather than a loud trap.
    let mut registry = ColorTargetRegistry::try_new(layout(), 1).unwrap();
    let key = key_at(FIXTURE_START, 4, 2, ColorTargetFormat::Rgba16);
    let candidate = registry.begin_candidate(key).unwrap();
    let seed = execute_fill_rectangle(
        &candidate,
        fill_cycle_other_mode(),
        FillColor::from_wire(0xF801_F801),
        rect(0, 0, 12, 4),
        crate::targets::RdpScissorRect::from_wire_quarter_pixels(0, 0, 0, 0xffff, 0xffff),
        None,
    )
    .unwrap();
    let initialized = candidate.admit_completed_initialization(seed).unwrap();
    registry.commit_initialized(initialized).unwrap();
    let resident_bytes_before = registry.residents()[0]
        .device_bytes()
        .device_bytes()
        .to_vec();

    let candidate2 = registry.begin_candidate(key).unwrap();
    assert!(candidate2.predecessor().is_some());
    let error = execute_fill_rectangle(
        &candidate2,
        fill_cycle_other_mode(),
        FillColor::from_wire(0x07C1_07C1),
        rect(0, 4, 12, 4),
        crate::targets::RdpScissorRect::from_wire_quarter_pixels(0, 0, 0, 0xffff, 0xffff),
        None,
    )
    .unwrap_err();
    assert!(matches!(
        error,
        FillExecutionError::MissingResidentBytes { key: rejected_key } if rejected_key == key
    ));
    // Nonmutation: the resident target is untouched.
    assert_eq!(
        registry.residents()[0].generation(),
        TargetGeneration::FIRST
    );
    assert_eq!(
        registry.residents()[0].device_bytes().device_bytes(),
        resident_bytes_before.as_slice()
    );
}

#[test]
fn out_of_bounds_rectangle_is_rejected_without_touching_the_target() {
    let mut registry = ColorTargetRegistry::try_new(layout(), 1).unwrap();
    let key = key_at(FIXTURE_START, 4, 2, ColorTargetFormat::Rgba16);
    let candidate = registry.begin_candidate(key).unwrap();
    let seed = execute_fill_rectangle(
        &candidate,
        fill_cycle_other_mode(),
        FillColor::from_wire(0xF801_F801),
        rect(0, 0, 12, 4),
        crate::targets::RdpScissorRect::from_wire_quarter_pixels(0, 0, 0, 0xffff, 0xffff),
        None,
    )
    .unwrap();
    let initialized = candidate.admit_completed_initialization(seed).unwrap();
    registry.commit_initialized(initialized).unwrap();
    let before = registry.residents()[0]
        .device_bytes()
        .device_bytes()
        .to_vec();

    let candidate2 = registry.begin_candidate(key).unwrap();
    // upper_left_x raw 0 -> x0 = 0 (in bounds); lower_right_x raw 20 -> x1 =
    // 5, but target width is only 4 (x in 0..=3) -- partially, not fully,
    // out of bounds: proves plan_rows rejects a rectangle that starts inside
    // the target and only later crosses the boundary, not just a rectangle
    // entirely outside it.
    let error = execute_fill_rectangle(
        &candidate2,
        fill_cycle_other_mode(),
        FillColor::from_wire(0x0000_0000),
        rect(0, 0, 20, 4),
        crate::targets::RdpScissorRect::from_wire_quarter_pixels(0, 0, 0, 0xffff, 0xffff),
        Some(&before),
    )
    .unwrap_err();
    assert!(matches!(
        error,
        FillExecutionError::Target(TargetError::RectangleOutOfBounds { .. })
    ));
    // Nonmutation: resident is untouched (still red, still generation 1).
    assert_eq!(
        registry.residents()[0].generation(),
        TargetGeneration::FIRST
    );
    assert_eq!(
        registry.residents()[0].device_bytes().device_bytes(),
        before.as_slice()
    );
}

#[test]
fn resident_byte_length_mismatch_is_rejected_without_touching_the_target() {
    let registry = ColorTargetRegistry::try_new(layout(), 1).unwrap();
    let key = key_at(FIXTURE_START, 4, 2, ColorTargetFormat::Rgba16);
    let candidate = registry.begin_candidate(key).unwrap();
    let wrong_length_bytes = vec![0u8; 4]; // target needs 8*2=16 bytes
    let error = execute_fill_rectangle(
        &candidate,
        fill_cycle_other_mode(),
        FillColor::from_wire(0xF801_F801),
        rect(0, 0, 12, 4),
        crate::targets::RdpScissorRect::from_wire_quarter_pixels(0, 0, 0, 0xffff, 0xffff),
        Some(&wrong_length_bytes),
    )
    .unwrap_err();
    assert!(matches!(
        error,
        FillExecutionError::Target(TargetError::CompletedByteLengthMismatch {
            expected: 16,
            actual: 4,
            ..
        })
    ));
}

#[test]
fn rgba32_target_format_stride_is_four_bytes_per_pixel() {
    let registry = ColorTargetRegistry::try_new(layout(), 1).unwrap();
    let key = key_at(FIXTURE_START, 4, 2, ColorTargetFormat::Rgba32);
    let candidate = registry.begin_candidate(key).unwrap();
    let fill_color = FillColor::from_wire(u32::from_be_bytes([0x10, 0x20, 0x30, 0xFF]));
    let completed = execute_fill_rectangle(
        &candidate,
        fill_cycle_other_mode(),
        fill_color,
        rect(0, 0, 12, 4),
        crate::targets::RdpScissorRect::from_wire_quarter_pixels(0, 0, 0, 0xffff, 0xffff),
        None,
    )
    .unwrap();
    let bytes = completed.device_bytes().device_bytes();
    assert_eq!(bytes.len(), 32); // 8 pixels * 4 bytes
    assert_eq!(&bytes[0..4], [0x10, 0x20, 0x30, 0xFF]);
    assert_eq!(&bytes[28..32], [0x10, 0x20, 0x30, 0xFF]);
}

#[test]
fn a_partial_fill_of_a_brand_new_target_refuses_without_seed_bytes() {
    // **Retargeted, not deleted.** This used to assert
    // `PartialNewTargetInitialization`: that a brand-new target could not
    // become resident from a partial rectangle at all. That refusal was
    // wrong -- hardware's untouched pixels are simply the RDRAM bytes that
    // were already there, and refusing swallowed every partial-rect fill,
    // which is ordinary content.
    //
    // What was RIGHT about it, and what this now pins, is the narrower
    // fact: the untouched pixels must not be fabricated. A partial fill
    // with no seed still refuses, by name, and still publishes nothing.
    let registry = ColorTargetRegistry::try_new(layout(), 1).unwrap();
    let key = key_at(FIXTURE_START, 4, 2, ColorTargetFormat::Rgba16);
    let candidate = registry.begin_candidate(key).unwrap();
    assert_eq!(candidate.predecessor(), None);
    let error = execute_fill_rectangle(
        &candidate,
        fill_cycle_other_mode(),
        FillColor::from_wire(0xF801_F801),
        rect(0, 4, 12, 4),
        crate::targets::RdpScissorRect::from_wire_quarter_pixels(0, 0, 0, 0xffff, 0xffff), // partial: only row 1
        None,
    )
    .unwrap_err();
    assert!(
        matches!(error, FillExecutionError::MissingSeedBytes { .. }),
        "expected MissingSeedBytes, got {error:?}"
    );
    assert!(registry.residents().is_empty());
}

#[test]
fn a_seeded_partial_fill_of_a_brand_new_target_keeps_the_seed_outside_the_rectangle() {
    // The positive half of the test above, and the one that would have
    // caught the fabricated zeros: the SAME partial rectangle, now given a
    // seed, must become resident with the seed's own bytes everywhere it
    // did not paint.
    //
    // Expectation derived by hand from the wire, not from the executor.
    // Target is 4x2 RGBA16. `rect(0, 4, 12, 4)` is quarter-pixel
    // (ulx=0, uly=4, lrx=12, lry=4) -> x 0..=3, y 1..=1: the whole of row 1
    // and none of row 0. Fill colour halfword 0xF801 is the memory-interface
    // source itself, so its RGB5 and visible coverage bit round-trip together.
    //
    // The seed is 0x1234 everywhere -- deliberately NOT the fill colour and
    // deliberately not zero, so "kept the seed", "painted the fill" and
    // "fabricated a zero" are three distinguishable outcomes. A seed equal
    // to the fill colour would have passed under the very bug this pins.
    let registry = ColorTargetRegistry::try_new(layout(), 1).unwrap();
    let key = key_at(FIXTURE_START, 4, 2, ColorTargetFormat::Rgba16);
    let candidate = registry.begin_candidate(key).unwrap();
    assert_eq!(candidate.predecessor(), None);
    let seed = [0x12u8, 0x34].repeat(8);
    let completed = execute_fill_rectangle(
        &candidate,
        fill_cycle_other_mode(),
        FillColor::from_wire(0xF801_F801),
        rect(0, 4, 12, 4),
        crate::targets::RdpScissorRect::from_wire_quarter_pixels(0, 0, 0, 0xffff, 0xffff),
        Some(&seed),
    )
    .unwrap();
    assert_eq!(
        completed.rectangle(),
        TargetRectangle::try_new(0, 1, 4, 1).unwrap()
    );
    let bytes = completed.device_bytes().device_bytes();
    // Row 0: untouched, so still the seed.
    assert_eq!(
        &bytes[0..8],
        &[0x12, 0x34].repeat(4)[..],
        "row 0 must keep the seed"
    );
    // Row 1: painted with the fill colour.
    assert_eq!(
        &bytes[8..16],
        &[0xF8, 0x01].repeat(4)[..],
        "row 1 must carry the fill"
    );
    // And it publishes, which the old refusal prevented.
    let initialized = candidate.admit_completed_initialization(completed).unwrap();
    assert_eq!(
        initialized.initialized_region().covered(),
        TargetRectangle::try_new(0, 1, 4, 1).unwrap(),
        "the proof must name the rectangle actually covered, not the whole target"
    );
}

/// **The scissor must clip on BOTH axes, and each axis is pinned alone.**
///
/// A parallel three-way differential (wgpu vs RT64 vs reference) measured
/// this backend honouring the scissor horizontally and IGNORING it
/// vertically before the fill clip landed: its `scissor-top-rows-only` case
/// differed by exactly 320x120 pixels -- precisely the scissored-out region
/// -- with RT64, the reference backend and an independent hand-derived key
/// all agreeing against wgpu, while the X-axis counterpart case was
/// byte-identical.
///
/// That asymmetry is the reason these are two tests over one helper rather
/// than one test with a scissor narrowed on both axes. A rectangle clipped
/// correctly in X and not at all in Y still LOOKS clipped, so a fixture that
/// narrows both would pass while Y silently regressed -- the coincidence
/// trap `docs/RT64-WM2000-HARNESS-TRAPS.md` names.
///
/// Each expectation is derived from the wire, not the executor: the scissor
/// is latched in quarter-pixels and fn64 resolves its pixel bounds as
/// `ceil(q / 4)` on every edge
/// (`RdpScissorRect::quarter_to_pixel_ceil`). That rounding rule is fn64's
/// own reading and is not independently confirmed against an allowed
/// hardware reference.
fn fill_clipped_by(scissor: RdpScissorRect) -> (TargetRectangle, Vec<u8>) {
    // A 4x4 RGBA16 target, seeded 0x1234 everywhere -- neither the fill
    // colour nor zero, so "kept the seed", "painted the fill" and
    // "fabricated a zero" stay three distinguishable outcomes.
    let registry = ColorTargetRegistry::try_new(layout(), 1).unwrap();
    let key = key_at(FIXTURE_START, 4, 4, ColorTargetFormat::Rgba16);
    let candidate = registry.begin_candidate(key).unwrap();
    let seed = [0x12u8, 0x34].repeat(16);
    let completed = execute_fill_rectangle(
        &candidate,
        fill_cycle_other_mode(),
        FillColor::from_wire(0xF801_F801),
        // The whole 4x4 target, in quarter-pixels: 0..=12 on both axes.
        rect(0, 0, 12, 12),
        scissor,
        Some(&seed),
    )
    .unwrap();
    let bytes = completed.device_bytes().device_bytes().to_vec();
    (completed.rectangle(), bytes)
}

#[test]
fn the_scissor_clips_columns_leaving_every_row_present() {
    // Scissor admits columns 0..2 (lrx = 8 quarter-pixels -> ceil(8/4) = 2)
    // and every row. The fill asks for all four columns.
    let (rectangle, bytes) =
        fill_clipped_by(RdpScissorRect::from_wire_quarter_pixels(0, 0, 0, 8, 16));
    assert_eq!(
        rectangle,
        TargetRectangle::try_new(0, 0, 2, 4).unwrap(),
        "two columns, all four rows"
    );
    for row in 0..4 {
        let base = row * 8;
        assert_eq!(
            &bytes[base..base + 4],
            &[0xF8, 0x01].repeat(2)[..],
            "row {row} columns 0..2 must be painted"
        );
        assert_eq!(
            &bytes[base + 4..base + 8],
            &[0x12, 0x34].repeat(2)[..],
            "row {row} columns 2..4 are outside the scissor and must keep the seed"
        );
    }
}

#[test]
fn the_scissor_clips_rows_leaving_every_column_present() {
    // The Y counterpart, and the axis the differential measured as broken.
    // Scissor admits rows 0..2 (lry = 8 -> ceil(8/4) = 2) and every column.
    //
    // A backend that clipped X and ignored Y would return a 4x4 rectangle
    // here and paint rows 2 and 3, which the seed assertion below catches.
    let (rectangle, bytes) =
        fill_clipped_by(RdpScissorRect::from_wire_quarter_pixels(0, 0, 0, 16, 8));
    assert_eq!(
        rectangle,
        TargetRectangle::try_new(0, 0, 4, 2).unwrap(),
        "all four columns, two rows"
    );
    assert_eq!(
        &bytes[0..16],
        &[0xF8, 0x01].repeat(8)[..],
        "rows 0..2 must be painted across their full width"
    );
    assert_eq!(
        &bytes[16..32],
        &[0x12, 0x34].repeat(8)[..],
        "rows 2..4 are outside the scissor and must keep the seed"
    );
}

#[test]
fn the_scissor_clips_the_low_edge_on_both_axes() {
    // The high edges alone would pass a backend that clamped only `lrx`/
    // `lry`. This narrows `ulx`/`uly` instead: columns 1..4 and rows 1..4
    // (ceil(4/4) = 1 on each low edge).
    let (rectangle, bytes) =
        fill_clipped_by(RdpScissorRect::from_wire_quarter_pixels(0, 4, 4, 16, 16));
    assert_eq!(
        rectangle,
        TargetRectangle::try_new(1, 1, 3, 3).unwrap(),
        "origin moves to (1, 1) and the extent shrinks to 3x3"
    );
    assert_eq!(
        &bytes[0..8],
        &[0x12, 0x34].repeat(4)[..],
        "row 0 is above the scissor and must keep the seed"
    );
    assert_eq!(
        &bytes[8..10],
        &[0x12, 0x34],
        "row 1 column 0 is left of the scissor and must keep the seed"
    );
    assert_eq!(
        &bytes[10..16],
        &[0xF8, 0x01].repeat(3)[..],
        "row 1 columns 1..4 must be painted"
    );
}

#[test]
fn a_rectangle_entirely_outside_the_scissor_refuses_rather_than_drawing_nothing() {
    // **Mutation-driven.** Replacing the `ScissoredAway` return with
    // `Ok(rectangle)` -- a silent no-op -- survived every other test in this
    // file and the whole differential sweep, because no fixture placed a
    // rectangle wholly outside its scissor.
    //
    // The distinction is not cosmetic. A silent `Ok` here returns the
    // UNCLIPPED rectangle, so the executor would then paint the full span
    // the scissor was supposed to suppress, and `plan_rows` would declare
    // writes for it. The refusal is what stops that.
    //
    // Scissor admits columns 0..1 only (lrx = 4 quarter-pixels -> 1); the
    // rectangle starts at column 2, so nothing survives.
    let registry = ColorTargetRegistry::try_new(layout(), 1).unwrap();
    let key = key_at(FIXTURE_START, 4, 4, ColorTargetFormat::Rgba16);
    let candidate = registry.begin_candidate(key).unwrap();
    let seed = [0x12u8, 0x34].repeat(16);
    let error = execute_fill_rectangle(
        &candidate,
        fill_cycle_other_mode(),
        FillColor::from_wire(0xF801_F801),
        rect(8, 0, 12, 12), // columns 2..=3
        RdpScissorRect::from_wire_quarter_pixels(0, 0, 0, 4, 16),
        Some(&seed),
    )
    .unwrap_err();
    assert!(
        matches!(error, FillExecutionError::ScissoredAway { .. }),
        "expected ScissoredAway, got {error:?}"
    );
    assert!(registry.residents().is_empty(), "nothing may be published");
}
