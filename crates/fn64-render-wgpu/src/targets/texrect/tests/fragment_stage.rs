use super::fragment::{
    alpha_compare_texrect_fragment, blend_texrect_fragment, write_pixel, TexrectNoiseStage,
    NOISE_DITHER_THRESHOLD,
};
use super::*;
use crate::state::CoverageDestination;

const WM2000_HIGH: u32 = 0x0000_acef;
const WM2000_LOW: u32 = 0x0050_41c8;

fn mode(high: u32, low: u32) -> OtherMode {
    OtherMode::from_wire(high, low)
}

/// **Positive control for the whole module**: WM2000's captured words
/// decode to the stage modes every expectation below assumes, and each
/// field is reconciled against an independent derivation from the same
/// literal.
#[test]
fn wm2000_frame_zero_stage_modes_decode_as_derived() {
    let m = mode(WM2000_HIGH, WM2000_LOW);
    assert_eq!(m.alpha_compare(), AlphaCompare::None);
    assert_eq!(WM2000_LOW & 0x3, 0, "G_AC is other-mode low bits 0:1");
    assert_eq!(m.alpha_dither(), AlphaDither::Noise);
    assert_eq!((WM2000_HIGH >> 4) & 0x3, 2, "alpha dither is high bits 4:5");
    assert_eq!(m.rgb_dither(), RgbDither::Disabled);
    assert_eq!((WM2000_HIGH >> 6) & 0x3, 3, "RGB dither is high bits 6:7");
    assert!(!m.coverage_times_alpha());
    assert_eq!(WM2000_LOW & 0x1000, 0, "CVG_X_ALPHA is low bit 12");
    assert!(!m.alpha_coverage_select());
    assert_eq!(WM2000_LOW & 0x2000, 0, "ALPHA_CVG_SEL is low bit 13");
    assert_eq!(m.coverage_destination(), CoverageDestination::Wrap);
    assert_eq!((WM2000_LOW >> 8) & 0x3, 1, "cvg_dst is low bits 8:9");

    TexrectFragmentStages::try_new(m, Color4::from_wire(0))
        .expect("every WM2000 stage mode is admitted");
}

/// **The `blend_cycle_count` hazard, settled: the two counts are not
/// in conflict, they answer different questions.**
///
/// `rt64_blender_analysis::blend_cycle_count` returns
/// `combine_cycle_count - 1` without `FORCE_BL`, while
/// `BlendModeState::cycle_count` returns 1/2/0 straight from
/// `cycle_type()`. They disagree numerically for every
/// non-`force_blend` mode, which reads like a defect and is not one:
///
/// - `blend_cycle_count` counts the cycles that **actually blend**.
///   Its consumers are the `uses_*` predicates, which ask "does any
///   blending cycle read this input?" -- and a bypassed last cycle
///   reads only `P`, never `A`/`B`, so excluding it is correct.
/// - `cycle_count` counts the **loop iterations** `blend_fragment`
///   runs. That loop handles the bypass internally via
///   `is_last && !blend_enabled`, so it must still visit the cycle it
///   bypasses in order to resolve `P`.
///
/// Both are faithful ports of differently-purposed upstream
/// functions. This test pins the numeric disagreement **and** the
/// reconciliation, so neither can be "fixed" into the other.
#[test]
fn the_two_cycle_counts_disagree_by_design_and_the_reason_is_pinned() {
    use crate::rt64_blender_analysis::{blend_cycle_count, combine_cycle_count};

    // FORCE_BL clear: the two disagree, by exactly one.
    let no_force = mode(WM2000_HIGH, WM2000_LOW & !0x4000);
    assert!(!no_force.force_blend());
    let state =
        TexrectBlendRegisters::new(Color4::from_wire(0), Color4::from_wire(0)).mode_state(no_force);
    assert_eq!(combine_cycle_count(no_force), 1);
    assert_eq!(blend_cycle_count(no_force), 0, "no cycle actually blends");
    assert_eq!(state.cycle_count(), 1, "one loop iteration still runs");

    // FORCE_BL set -- WM2000's own mode -- and they agree, which is
    // why the disagreement is unreachable for this packet.
    let forced = mode(WM2000_HIGH, WM2000_LOW);
    assert!(forced.force_blend());
    let state =
        TexrectBlendRegisters::new(Color4::from_wire(0), Color4::from_wire(0)).mode_state(forced);
    assert_eq!(blend_cycle_count(forced), 1);
    assert_eq!(u32::from(state.cycle_count()), blend_cycle_count(forced));

    // The reconciliation, asserted rather than asserted-about: the
    // single iteration the loop runs under a cleared FORCE_BL is the
    // bypass, and it leaves the fragment's colour at P = Combined.
    let blended = blend_texrect_fragment(
        [200, 100, 50, 64],
        BlendFramebufferSample {
            rgba: [16, 200, 240, 255],
            coverage_count: 8,
        },
        TexrectBlendRegisters::new(Color4::from_wire(0), Color4::from_wire(0))
            .mode_state(mode(WM2000_HIGH, WM2000_LOW & !0x4000 & !0x0008)),
        0,
        0,
    )
    .unwrap();
    assert_eq!(
        blended[0..3],
        [200, 100, 50],
        "the zero blending cycles blend_cycle_count reports is what the bypass produces"
    );
}

/// **The endpoint proof `NOISE_DITHER_THRESHOLD` rests on.**
///
/// Exhaustive over all 256 alpha values and all 8 thresholds: the
/// dithered five-bit alpha takes exactly two values, `floor` and
/// `floor + 1`, and threshold 7 always selects `floor`. So the
/// executor's constant is a member of the mode's real output set, not
/// a third value between the two. Derived here from the arithmetic
/// rather than read off `apply_alpha_dither`, then reconciled against
/// it.
#[test]
fn the_noise_dither_threshold_is_an_endpoint_not_an_invention() {
    assert_eq!(
        NOISE_DITHER_THRESHOLD.dither(),
        7,
        "the maximum 3-bit threshold"
    );
    for alpha in 0u8..=255 {
        let floor = u16::from(alpha >> 3);
        let mut seen = std::collections::BTreeSet::new();
        for threshold in 0u8..8 {
            // Re-derived from `apply_alpha_dither`'s documented
            // arithmetic, independently of the function itself.
            let rounded = floor + u16::from((alpha & 7) > threshold);
            seen.insert(rounded.min(31));
        }
        assert!(
            seen.len() <= 2 && *seen.iter().next().unwrap() == floor.min(31),
            "alpha {alpha}: the mode's output set must be {{floor, floor+1}}, got {seen:?}"
        );
        assert!(
            seen.iter().all(|&v| v.abs_diff(floor.min(31)) <= 1),
            "alpha {alpha}: dither must never move the channel by more than one step"
        );

        // And the function itself, at threshold 7, must equal the
        // undithered floor re-expanded.
        let five = (floor.min(31)) as u8;
        assert_eq!(
            apply_alpha_dither(
                alpha,
                AlphaDither::Noise,
                RgbDither::Disabled,
                0,
                0,
                NOISE_DITHER_THRESHOLD
            ),
            (five << 3) | (five >> 2),
            "alpha {alpha} at the maximum threshold must be the undithered floor"
        );
    }
}

/// `wraps` does not need the two hidden coverage bits **for a
/// full-coverage fragment**, which is what a texrect always produces.
///
/// Derived two ways and reconciled: once by enumerating every value
/// the stored count can hold (`Coverage::from_stored` is
/// `(stored & 7) + 1`, so `1..=8`), and once from the inequality
/// `8 + memory > 8` being true for all `memory >= 1`.
#[test]
fn wraps_is_determined_for_a_full_coverage_fragment() {
    let bits = CoverageModeBits {
        image_read_enabled: true,
        force_blend: true,
        antialias_enabled: true,
        coverage_destination: CoverageDestination::Wrap,
    };
    // Enumeration.
    for stored in 0u8..8 {
        let memory = Coverage::from_stored(stored);
        assert!((1..=8).contains(&memory.count()));
        let result = coverage_result(Coverage::FULL, memory, bits);
        assert!(
            result.wraps,
            "stored {stored} (count {}) must still wrap under a full-coverage fragment",
            memory.count()
        );
        assert!(result.blend_enabled);
    }
    // The inequality, stated independently of the loop.
    assert!(Coverage::FULL.count() + 1 > Coverage::FULL.count());

    // And the executor's own accessor agrees, for the mode WM2000
    // latches.
    let stages =
        TexrectFragmentStages::try_new(mode(WM2000_HIGH, WM2000_LOW), Color4::from_wire(0))
            .unwrap();
    let result = stages.coverage_for(Coverage::FULL, Coverage::FULL).unwrap();
    assert!(result.wraps);
    assert!(result.blend_enabled);
}

/// Image-read Save still exposes the unavailable three-bit destination
/// coverage. A partial fragment with image read is likewise refused
/// because its wrap state is ambiguous.
#[test]
fn the_modes_that_expose_the_missing_coverage_bits_are_refused_by_name() {
    // cvg_dst = Save is low bits 8:9 == 3.
    let save = mode(WM2000_HIGH, (WM2000_LOW & !(0x3 << 8)) | (0x3 << 8));
    assert_eq!(save.coverage_destination(), CoverageDestination::Save);
    let stages = TexrectFragmentStages::try_new(save, Color4::from_wire(0)).unwrap();
    assert_eq!(
        stages.coverage_for(Coverage::FULL, Coverage::FULL),
        Err(TexrectExecutionError::DestinationCoverageUnavailable {
            consumer: "cvg_dst = Save"
        })
    );

    let stages =
        TexrectFragmentStages::try_new(mode(WM2000_HIGH, WM2000_LOW), Color4::from_wire(0))
            .unwrap();
    assert_eq!(
        stages.coverage_for(Coverage::new(4), Coverage::FULL),
        Err(TexrectExecutionError::DestinationCoverageUnavailable {
            consumer: "a partial-coverage fragment's cvg_dst accumulation"
        })
    );

    // With image read disabled the destination count is never read at
    // all, so even `Save` is admitted -- the refusal is about
    // observability, not about the mode's name.
    let no_read = mode(WM2000_HIGH, (WM2000_LOW & !0x40 & !(0x3 << 8)) | (0x3 << 8));
    assert!(!no_read.image_read_enabled());
    let stages = TexrectFragmentStages::try_new(no_read, Color4::from_wire(0)).unwrap();
    assert!(stages
        .coverage_for(Coverage::new(4), Coverage::FULL)
        .is_ok());
}

/// The visible RGBA16 coverage bit follows the post-accumulation coverage
/// destination, not fragment alpha. Programming Manual §§15.5.3, 15.5.6,
/// and 15.7 define the stored `count - 1` encoding; RT64's
/// `Float4ToRGBA16` independently extracts its bit 2.
#[test]
fn rgba16_bit_zero_follows_each_coverage_destination_mode() {
    let cases = [
        (CoverageDestination::Clamp, 1u16),
        (CoverageDestination::Wrap, 0),
        (CoverageDestination::Full, 1),
        (CoverageDestination::Save, 1),
    ];
    for (destination, expected_bit) in cases {
        let result = coverage_result(
            Coverage::new(4),
            Coverage::FULL,
            CoverageModeBits {
                image_read_enabled: true,
                force_blend: true,
                antialias_enabled: false,
                coverage_destination: destination,
            },
        );
        let mut packed = [0u8; 2];
        write_pixel(
            ColorTargetFormat::Rgba16,
            &mut packed,
            [0, 0, 0, 0],
            result.destination,
        );
        assert_eq!(u16::from_be_bytes(packed) & 1, expected_bit);
    }

    let (selected, coverage) = apply_coverage_alpha(false, true, [0, 0, 0, 0], Coverage::new(4));
    assert_eq!(selected[3], coverage.alpha());
    assert_eq!(coverage, Coverage::new(4));
    let mut packed = [0u8; 2];
    write_pixel(ColorTargetFormat::Rgba16, &mut packed, selected, coverage);
    assert_eq!(u16::from_be_bytes(packed) & 1, 0);

    let (unselected, coverage) = apply_coverage_alpha(false, false, [0, 0, 0, 0], Coverage::FULL);
    assert_eq!(unselected[3], 0);
    let full = coverage_result(
        coverage,
        Coverage::new(1),
        CoverageModeBits {
            image_read_enabled: false,
            force_blend: false,
            antialias_enabled: false,
            coverage_destination: CoverageDestination::Full,
        },
    );
    write_pixel(
        ColorTargetFormat::Rgba16,
        &mut packed,
        unselected,
        full.destination,
    );
    assert_eq!(u16::from_be_bytes(packed) & 1, 1);
}

/// The alpha-compare gate, hand-derived at the threshold boundary in
/// both directions.
///
/// `G_AC_THRESHOLD` passes iff `alpha >= G_SETBLENDCOLOR.a`, so `a-1`
/// rejects, `a` passes and `a+1` passes. `G_AC_NONE` passes
/// everything, including alpha 0.
#[test]
fn the_alpha_compare_gate_is_hand_derived_at_its_boundary() {
    // G_AC_NONE: WM2000's own mode.
    let stages =
        TexrectFragmentStages::try_new(mode(WM2000_HIGH, WM2000_LOW), Color4::from_wire(0))
            .unwrap();
    for alpha in [0u8, 1, 128, 255] {
        assert!(
            alpha_compare_texrect_fragment(stages, alpha).unwrap(),
            "G_AC_NONE must pass alpha {alpha}"
        );
    }

    // G_AC_THRESHOLD (low bits 0:1 == 1) against SetBlendColor alpha.
    const THRESHOLD: u8 = 0x80;
    let threshold_mode = mode(WM2000_HIGH, (WM2000_LOW & !0x3) | 0x1);
    assert_eq!(threshold_mode.alpha_compare(), AlphaCompare::Threshold);
    let blend_color = Color4::from_wire(0x0000_0000 | u32::from(THRESHOLD));
    assert_eq!(
        blend_color.rgba8()[3],
        THRESHOLD,
        "the wire's low byte is alpha"
    );
    let stages = TexrectFragmentStages::try_new(threshold_mode, blend_color).unwrap();
    assert!(!alpha_compare_texrect_fragment(stages, THRESHOLD - 1).unwrap());
    assert!(alpha_compare_texrect_fragment(stages, THRESHOLD).unwrap());
    assert!(alpha_compare_texrect_fragment(stages, THRESHOLD + 1).unwrap());

    // Threshold with no SetBlendColor staged compares against the
    // register's power-on zero. `alpha >= 0` holds for every alpha, so
    // every fragment passes -- derived by hand from the comparison
    // `alpha >= threshold_alpha`, and it is exactly what the reference
    // lane computes (`raster/blend.rs:113` against the zero-initialized
    // `other_mode.blend_color_alpha`).
    let unwritten = TexrectFragmentStages::try_new(threshold_mode, Color4::from_wire(0)).unwrap();
    for alpha in [0u8, 1, 0x7f, THRESHOLD - 1, THRESHOLD, 0xff] {
        assert!(
            alpha_compare_texrect_fragment(unwritten, alpha).unwrap(),
            "alpha {alpha:#04x} must pass a Threshold compare against the power-on zero"
        );
    }
    // ...and the written register must still reject below its own
    // threshold, or the sweep above could pass against a comparator
    // that ignores the register entirely.
    assert!(!alpha_compare_texrect_fragment(stages, THRESHOLD - 1).unwrap());
}

/// A rejected fragment writes **nothing** -- the destination keeps its
/// prior value rather than being overwritten with a blended one.
#[test]
fn an_alpha_compare_rejection_leaves_the_destination_untouched() {
    const THRESHOLD: u8 = 0xc0;
    let threshold_mode = mode(WM2000_HIGH, (WM2000_LOW & !0x3) | 0x1);
    let stages =
        TexrectFragmentStages::try_new(threshold_mode, Color4::from_wire(u32::from(THRESHOLD)))
            .unwrap();
    let blend_state = TexrectBlendRegisters::new(
        Color4::from_wire(u32::from(THRESHOLD)),
        Color4::from_wire(0),
    )
    .mode_state(threshold_mode);

    let mut stored = 0x0001u16.to_be_bytes();
    // Alpha below the threshold: rejected, nothing written.
    blend_and_write_pixel(
        ColorTargetFormat::Rgba16,
        &mut stored,
        [255, 255, 255, THRESHOLD - 1],
        blend_state,
        stages,
        0,
        0,
    )
    .unwrap();
    assert_eq!(
        u16::from_be_bytes(stored),
        0x0001,
        "a rejected fragment must not write"
    );

    // Alpha at the threshold: accepted, and the pixel changes.
    blend_and_write_pixel(
        ColorTargetFormat::Rgba16,
        &mut stored,
        [255, 255, 255, THRESHOLD],
        blend_state,
        stages,
        0,
        0,
    )
    .unwrap();
    assert_ne!(
        u16::from_be_bytes(stored),
        0x0001,
        "an accepted fragment must write"
    );
}

/// `CVG_X_ALPHA` and `ALPHA_CVG_SEL` are independent bits with
/// independent effects, hand-derived from `Coverage`'s own encodings.
///
/// With full coverage, `ALPHA_CVG_SEL` overwrites the fragment alpha
/// with `Coverage::FULL.alpha()` = `(8*255 + 4) / 8` = 255, and
/// `CVG_X_ALPHA` multiplies coverage by the fragment alpha first.
#[test]
fn the_two_coverage_alpha_bits_are_independent_and_hand_derived() {
    assert_eq!(Coverage::FULL.alpha(), 255);
    assert_eq!(
        (8u16 * 255 + 4) / 8,
        255,
        "derived independently of Coverage::alpha"
    );
    // Half alpha times full coverage: (8*128 + 127) / 255 = 4.
    assert_eq!(Coverage::FULL.times_alpha(128).count(), 4);
    assert_eq!((8u16 * 128 + 127) / 255, 4, "derived independently");

    let rgba = [10u8, 20, 30, 128];
    // Neither bit: pass-through.
    let (out, cvg) = apply_coverage_alpha(false, false, rgba, Coverage::FULL);
    assert_eq!(out, rgba);
    assert_eq!(cvg, Coverage::FULL);
    // ALPHA_CVG_SEL only: alpha becomes the coverage encoding.
    let (out, cvg) = apply_coverage_alpha(false, true, rgba, Coverage::FULL);
    assert_eq!(out[3], 255);
    assert_eq!(cvg, Coverage::FULL);
    // CVG_X_ALPHA only: coverage shrinks, alpha is untouched.
    let (out, cvg) = apply_coverage_alpha(true, false, rgba, Coverage::FULL);
    assert_eq!(out[3], 128);
    assert_eq!(cvg.count(), 4);
    // Both: coverage shrinks first, then alpha takes the shrunk value.
    let (out, cvg) = apply_coverage_alpha(true, true, rgba, Coverage::FULL);
    assert_eq!(cvg.count(), 4);
    assert_eq!(out[3], Coverage::new(4).alpha());
    assert_eq!(
        out[3],
        ((4u16 * 255 + 4) / 8) as u8,
        "derived independently"
    );
}

/// **The coverage-alpha stage runs inside the pixel loop**, asserted
/// through [`blend_and_write_pixel`] rather than through
/// `apply_coverage_alpha` alone.
///
/// **The first witness for this was degenerate and is recorded as a
/// correction.** It used `CVG_X_ALPHA` with a zero fragment alpha,
/// expecting no write; but a zero alpha makes the blend composite a
/// pure destination pass-through, so skipping the stage entirely
/// produced the *same* stored halfword and the mutant survived.
/// `ALPHA_CVG_SEL` separates them: with full coverage it *raises*
/// alpha to `Coverage::FULL.alpha()` = 255, so a fragment alpha of 64
/// blends to 5-bit 31 with the stage and 5-bit 8 without it.
#[test]
fn the_coverage_alpha_stage_runs_inside_the_pixel_loop() {
    // ALPHA_CVG_SEL is low bit 13.
    let cvg_sel = mode(WM2000_HIGH, WM2000_LOW | 0x2000);
    assert!(cvg_sel.alpha_coverage_select());
    let stages = TexrectFragmentStages::try_new(cvg_sel, Color4::from_wire(0)).unwrap();
    let blend_state =
        TexrectBlendRegisters::new(Color4::from_wire(0), Color4::from_wire(0)).mode_state(cvg_sel);

    let mut stored = 0x0001u16.to_be_bytes();
    blend_and_write_pixel(
        ColorTargetFormat::Rgba16,
        &mut stored,
        [255, 255, 255, 64],
        blend_state,
        stages,
        0,
        0,
    )
    .unwrap();

    // Hand-derived both ways. With the stage: alpha becomes
    // Coverage::FULL.alpha() = 255, so the composite is 255 * 1 + 0 = 255
    // -> 5-bit 31. Without it: 255 * 64/255 + 0 = 64 -> 5-bit 8.
    let with_stage = (255.0f32 * (255.0 / 255.0)).round() as u16 >> 3;
    let without_stage = (255.0f32 * (64.0 / 255.0)).round() as u16 >> 3;
    assert_eq!((with_stage, without_stage), (31, 8), "the two must differ");
    assert_eq!(
        u16::from_be_bytes(stored) >> 11,
        with_stage,
        "ALPHA_CVG_SEL must have raised the fragment alpha before the blend"
    );
}

/// Zero coverage writes nothing. Reachable only through
/// `CVG_X_ALPHA` with a zero fragment alpha, which is why the witness
/// sets that bit rather than asserting on an unreachable state.
///
/// **Necessary but not sufficient**, and deliberately kept alongside
/// the test above rather than replaced by it: a zero fragment alpha
/// makes the blend a destination pass-through, so this cannot on its
/// own distinguish "did not write" from "wrote the same value".
#[test]
fn a_zero_coverage_fragment_writes_nothing() {
    // CVG_X_ALPHA is low bit 12.
    let cvg_x_alpha = mode(WM2000_HIGH, WM2000_LOW | 0x1000);
    assert!(cvg_x_alpha.coverage_times_alpha());
    assert_eq!(Coverage::FULL.times_alpha(0).count(), 0);
    let stages = TexrectFragmentStages::try_new(cvg_x_alpha, Color4::from_wire(0)).unwrap();
    let blend_state = TexrectBlendRegisters::new(Color4::from_wire(0), Color4::from_wire(0))
        .mode_state(cvg_x_alpha);
    let mut stored = 0x0001u16.to_be_bytes();
    blend_and_write_pixel(
        ColorTargetFormat::Rgba16,
        &mut stored,
        [255, 255, 255, 0],
        blend_state,
        stages,
        0,
        0,
    )
    .unwrap();
    assert_eq!(u16::from_be_bytes(stored), 0x0001);
}

/// `CLR_ON_CVG` + `CVG_DST_WRAP` with `IM_RD`/`AA_EN`/`FORCE_BL` clear:
/// a FULL-coverage fragment MUST be written, matching angrylion + RT64
/// (the `gen-coverage-color-on-cvg-one-cycle` parity case). This pins the
/// write decision against the pre-fix `!coverage.wraps` gate, which
/// short-circuited `wraps` to `false` on clear `IM_RD` and dropped the
/// write -- reverting to that gate re-drops this pixel and fails here.
///
/// The hardware rule (angrylion `blender_1cycle`): `color_on_cvg` never
/// gates the color write itself; the write is gated by the coverage
/// carry-out (`prewrap = (memcvg + cvg) & 8`, `memcvg = 0` with no
/// `IM_RD` read), which a full-coverage fragment (`cvg = 8`) always sets.
#[test]
fn clr_on_cvg_with_wrap_writes_a_full_coverage_fragment_without_image_read() {
    // Only CLR_ON_CVG (bit 7) + CVG_DST_WRAP (bits 9:8 == 1): no IM_RD,
    // no AA_EN, no FORCE_BL, no alpha compare, no coverage-alpha bits.
    let low = 0x080 | 0x100;
    let m = mode(0, low);
    assert!(m.clear_on_coverage(), "CLR_ON_CVG must be set");
    assert_eq!(m.coverage_destination(), CoverageDestination::Wrap);
    assert!(!m.image_read_enabled(), "IM_RD must be clear for this case");
    assert!(!m.antialias_enabled(), "AA_EN must be clear for this case");
    assert!(!m.force_blend(), "FORCE_BL must be clear for this case");

    let stages = TexrectFragmentStages::try_new(m, Color4::from_wire(0)).unwrap();
    let blend_state =
        TexrectBlendRegisters::new(Color4::from_wire(0), Color4::from_wire(0)).mode_state(m);

    // Seed STALE (0xffff); a distinct opaque combined color must land.
    let mut stored = 0xffffu16.to_be_bytes();
    blend_and_write_pixel(
        ColorTargetFormat::Rgba16,
        &mut stored,
        [0x20, 0x40, 0x60, 0xff],
        blend_state,
        stages,
        0,
        0,
    )
    .unwrap();
    assert_ne!(
        u16::from_be_bytes(stored),
        0xffff,
        "CLR_ON_CVG + CVG_DST_WRAP must WRITE a full-coverage fragment \
         (angrylion + RT64 both do); the pixel stayed STALE"
    );
}

/// Every mode this card refuses, refused by name and distinguishable
/// from every other refusal.
#[test]
fn every_unevaluatable_stage_mode_is_refused_by_name() {
    // G_AC wire encoding 2 is NOT refused: pinned RT64 branches only for
    // `G_AC_DITHER` and `G_AC_THRESHOLD`, so encoding 2 performs no
    // compare
    // (`/Users/jer/Code/no-mercy-recompiled/third_party/rt64/src/shaders/RasterPS.hlsl:203-213`).
    // Retargeted from the assertion that
    // this encoding raised `ReservedAlphaCompare`; see
    // `docs/rt64/RT64-GUARD-AUDIT.md` finding A3.
    let dither_bit_without_enable = mode(WM2000_HIGH, (WM2000_LOW & !0x3) | 0x2);
    assert_eq!(
        dither_bit_without_enable.alpha_compare(),
        AlphaCompare::None,
        "wire 2 sets dither_alpha_en but clears alpha_compare_en"
    );
    assert!(
        TexrectFragmentStages::try_new(dither_bit_without_enable, Color4::from_wire(0)).is_ok(),
        "no compare is not a refusal"
    );
    // Distinguishing check: wire 3 (both bits set) IS still refused, so
    // the `is_ok` above cannot be produced by an executor that admits
    // every alpha-compare mode.
    assert!(TexrectFragmentStages::try_new(
        mode(WM2000_HIGH, (WM2000_LOW & !0x3) | 0x3),
        Color4::from_wire(0)
    )
    .is_err());

    // G_AC_DITHER (encoding 3) needs the per-pixel random value.
    let ac_dither = mode(WM2000_HIGH, (WM2000_LOW & !0x3) | 0x3);
    assert_eq!(ac_dither.alpha_compare(), AlphaCompare::Dither);
    assert_eq!(
        TexrectFragmentStages::try_new(ac_dither, Color4::from_wire(0)),
        Err(TexrectExecutionError::NoiseThresholdUnavailable {
            stage: TexrectNoiseStage::AlphaCompareDither
        })
    );

    // Alpha dither Pattern resolving to Bayer: RGB dither Disabled
    // (encoding 3) substitutes Bayer, whose tables the two ports
    // disagree about.
    let bayer = mode((WM2000_HIGH & !(0x3 << 4)) | (0x0 << 4), WM2000_LOW);
    assert_eq!(bayer.alpha_dither(), AlphaDither::Pattern);
    assert_eq!(bayer.rgb_dither(), RgbDither::Disabled);
    assert_eq!(
        TexrectFragmentStages::try_new(bayer, Color4::from_wire(0)),
        Err(TexrectExecutionError::OrderedDitherAuthorityUnsettled {
            stage: TexrectNoiseStage::AlphaDither,
            pattern: RgbDither::Bayer
        })
    );

    // The same Pattern resolving to MagicSquare instead IS admitted --
    // the two ports agree at all 16 of its cells.
    let magic = mode(
        (WM2000_HIGH & !(0x3 << 4) & !(0x3 << 6)) | (0x0 << 4) | (0x0 << 6),
        WM2000_LOW,
    );
    assert_eq!(magic.alpha_dither(), AlphaDither::Pattern);
    assert_eq!(magic.rgb_dither(), RgbDither::MagicSquare);
    assert!(TexrectFragmentStages::try_new(magic, Color4::from_wire(0)).is_ok());
}

/// **D7's premise, re-measured — and the refusal kept.**
///
/// `docs/rt64/RT64-LANE-DIVERGENCES.md` D7 scored
/// [`TexrectExecutionError::OrderedDitherAuthorityUnsettled`] a wgpu
/// defect on the ground that the Bayer dispute lives in the *RGB*
/// module while the alpha-dither stage read a separate,
/// reference-identical table in `alpha_compare.rs` — so the cited
/// authority conflict did not apply to the stage being refused.
///
/// That premise was true at the audit's pin and is false now.
/// `51b4e184` deleted the duplicate, because libultra defines
/// `G_AD_PATTERN`'s threshold as *the currently selected RGB dither
/// matrix* (`gbi.h:674-678`) and one hardware quantity must have one
/// table. The alpha path now reads the disputed tile by construction.
///
/// This test pins that, so the refusal cannot be re-litigated from the
/// stale premise: it asserts (a) the alpha-dither threshold IS
/// `rgb_dither`'s Bayer value, (b) that value differs from the
/// reference's at the documented cells, and (c) the difference is
/// observable in `apply_alpha_dither`'s own output — which is the only
/// thing that makes the refusal load-bearing rather than fussy.
///
/// Every expectation is hand-derived from the two tables and the
/// published rounding rule, never captured.
#[test]
fn the_alpha_dither_refusal_is_downstream_of_the_one_disputed_tile() {
    // `fn64-render-reference`'s BAYER (`raster/blend.rs:30`), as a
    // literal so this test needs no cross-crate dependency. Same
    // constant `rgb_dither.rs`'s own disagreement test uses.
    const REFERENCE_BAYER: [[u8; 4]; 4] = [[0, 4, 1, 5], [6, 2, 7, 3], [1, 5, 0, 4], [7, 3, 6, 2]];

    let mut disputed_cells = Vec::new();
    for y in 0..4i32 {
        for x in 0..4i32 {
            let ours = crate::alpha_compare::alpha_dither_pattern_threshold_for_tests(
                RgbDither::Bayer,
                x,
                y,
            );
            // (a) The alpha path reads `rgb_dither`'s tile, not a
            // private copy. If the duplicate ever returns, this fails.
            assert_eq!(
                ours,
                crate::rgb_dither::ordered_tile_value(RgbDither::Bayer, x, y),
                "the alpha-dither path must read rgb_dither's tile at ({x}, {y})"
            );
            if ours != REFERENCE_BAYER[y as usize][x as usize] {
                disputed_cells.push((x, y, ours, REFERENCE_BAYER[y as usize][x as usize]));
            }
        }
    }

    // (b) The dispute is real and reaches the alpha stage's own tile.
    assert!(
        !disputed_cells.is_empty(),
        "D7's refusal presumes a live disagreement; if the Bayer phase \
         has been settled, resolve the refusal rather than this test"
    );

    // (c) It is observable in alpha dither's output. The rounding rule
    // is `(alpha >> 3) + ((alpha & 7) > threshold)`, so for any two
    // thresholds t_ours != t_ref there is an alpha whose low three bits
    // fall strictly between them and which therefore rounds differently
    // under the two tables. Pick it, do not search for it: with
    // low = min(t_ours, t_ref), the alpha with low-three-bits
    // `low + 1` exceeds the smaller threshold and not the larger.
    let (x, y, ours, theirs) = disputed_cells[0];
    let low = ours.min(theirs);
    let alpha = (16u8 << 3) | (low + 1);
    assert!(
        (alpha & 7) > low && (alpha & 7) <= ours.max(theirs),
        "the probe alpha must separate the two thresholds"
    );
    let dithered_ours = crate::alpha_compare::apply_alpha_dither(
        alpha,
        AlphaDither::Pattern,
        RgbDither::Bayer,
        x,
        y,
        crate::alpha_compare::AlphaCompareNoise(0),
    );
    // Hand-derived expectation under each table, from the same rule.
    let expand = |five: u8| (five << 3) | (five >> 2);
    let round = |threshold: u8| expand(16 + u8::from((alpha & 7) > threshold));
    assert_eq!(
        dithered_ours,
        round(ours),
        "alpha dither follows this crate's tile"
    );
    assert_ne!(
        round(ours),
        round(theirs),
        "the two tables give different alpha at ({x}, {y}); refusing \
         Bayer is therefore a real choice, not a formality"
    );
}

/// The alpha-dither stage really runs, and its ordered arm really
/// perturbs -- so a mutation that drops the call is observable.
///
/// Uses the admitted `MagicSquare` tile at a cell whose threshold is
/// low enough to bump the chosen alpha, hand-picked from the table
/// rather than searched: `MAGIC_SQUARE[0][0] == 0`, so any alpha whose
/// low three bits exceed 0 rounds up.
#[test]
fn the_alpha_dither_stage_perturbs_where_the_mode_says_it_should() {
    let magic = mode(
        (WM2000_HIGH & !(0x3 << 4) & !(0x3 << 6)) | (0x0 << 4) | (0x0 << 6),
        WM2000_LOW,
    );
    let stages = TexrectFragmentStages::try_new(magic, Color4::from_wire(0)).unwrap();
    assert_eq!(stages.alpha_dither, AlphaDither::Pattern);

    // alpha 223: floor 27, low bits 7 > threshold 0 -> rounds to 28.
    let dithered = apply_alpha_dither(
        223,
        stages.alpha_dither,
        stages.rgb_dither,
        0,
        0,
        NOISE_DITHER_THRESHOLD,
    );
    assert_eq!(dithered, (28u8 << 3) | (28u8 >> 2), "231");
    assert_ne!(dithered, 223, "the ordered tile must actually perturb here");

    // **And it reaches the pixel loop**, not just the helper.
    // Measured, not assumed: while this test went only through
    // `apply_alpha_dither`, replacing the executor's call with an
    // identity function left the whole suite green. The stage is
    // observable here because `MagicSquare` at cell (0,0) has
    // threshold 0, which bumps alpha 223 to 231 and moves the blended
    // channel by a whole five-bit step.
    let blend_state =
        TexrectBlendRegisters::new(Color4::from_wire(0), Color4::from_wire(0)).mode_state(magic);
    let mut stored = 0x0001u16.to_be_bytes();
    blend_and_write_pixel(
        ColorTargetFormat::Rgba16,
        &mut stored,
        [255, 255, 255, 223],
        blend_state,
        stages,
        0,
        0,
    )
    .unwrap();
    // Hand-derived: dithered alpha 231 over a zero destination gives
    // 255 * 231/255 = 231 -> 5-bit 28. Undithered would be 27.
    let dithered_five = (255.0f32 * (231.0 / 255.0)).round() as u16 >> 3;
    let undithered_five = (255.0f32 * (223.0 / 255.0)).round() as u16 >> 3;
    assert_eq!(
        (dithered_five, undithered_five),
        (28, 27),
        "the two must differ"
    );
    assert_eq!(
        u16::from_be_bytes(stored) >> 11,
        dithered_five,
        "the executor must have applied the ordered dither before blending"
    );

    // And WM2000's own Noise mode at the endpoint does not.
    let wm = TexrectFragmentStages::try_new(mode(WM2000_HIGH, WM2000_LOW), Color4::from_wire(0))
        .unwrap();
    assert_eq!(
        apply_alpha_dither(
            223,
            wm.alpha_dither,
            wm.rgb_dither,
            0,
            0,
            NOISE_DITHER_THRESHOLD
        ),
        (27u8 << 3) | (27u8 >> 2),
        "222 -- the endpoint, which is the undithered floor"
    );
}
