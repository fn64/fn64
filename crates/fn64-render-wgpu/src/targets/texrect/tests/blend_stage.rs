use super::fragment::{blend_texrect_fragment, read_pixel, write_pixel};
use super::*;
use crate::blend::{BlendAlphaInput, BlendBInput, BlendColorInput};

/// WM2000 frame 0's latched other-mode words, read off the captured
/// packet (`docs/rt64/RT64-WM2000-VALIDATION.md` §3): high `0x0000acef`
/// (one-cycle, RGB dither Disabled, alpha dither Noise), low
/// `0x005041c8` (`AA_EN`, `IM_RD`, `CLR_ON_CVG`, `cvg_dst = Wrap`,
/// `FORCE_BL`, `CVG_X_ALPHA` and `ALPHA_CVG_SEL` clear).
const WM2000_OTHER_MODE_HIGH: u32 = 0x0000_acef;
const WM2000_OTHER_MODE_LOW: u32 = 0x0050_41c8;

fn wm2000_other_mode() -> OtherMode {
    OtherMode::from_wire(WM2000_OTHER_MODE_HIGH, WM2000_OTHER_MODE_LOW)
}

fn wm2000_blend_state() -> BlendModeState {
    TexrectBlendRegisters::new(Color4::from_wire(0), Color4::from_wire(0))
        .mode_state(wm2000_other_mode())
}

fn wm2000_stages() -> TexrectFragmentStages {
    TexrectFragmentStages::try_new(wm2000_other_mode(), Color4::from_wire(0))
        .expect("WM2000's frame-0 mode is admitted by every stage")
}

/// **Positive control for every expectation below.** The two wire words
/// really do decode to the mode the derivation assumes.
///
/// Each field is asserted from the accessor AND reconciled against an
/// independent bit derivation of the same literal, so an off-by-one in
/// either the mask or the transcription contradicts itself rather than
/// agreeing by construction.
#[test]
fn wm2000_frame_zero_other_mode_decodes_to_the_derived_blender_state() {
    let mode = wm2000_other_mode();
    assert_eq!(mode.cycle_type(), CycleType::OneCycle);
    assert_eq!(
        (WM2000_OTHER_MODE_HIGH >> 20) & 0x3,
        0,
        "one-cycle, derived from the literal independently of the accessor"
    );
    assert!(mode.force_blend());
    assert_eq!(WM2000_OTHER_MODE_LOW & 0x4000, 0x4000, "FORCE_BL is bit 14");
    assert!(mode.image_read_enabled());
    assert_eq!(WM2000_OTHER_MODE_LOW & 0x0040, 0x0040, "IM_RD is bit 6");

    let cycle = ResolvedBlendCycleUnderTest::of(mode);
    assert_eq!(cycle.p, BlendColorInput::Combined);
    assert_eq!(cycle.a, BlendAlphaInput::Combined);
    assert_eq!(cycle.m, BlendColorInput::Framebuffer);
    assert_eq!(cycle.b, BlendBInput::OneMinusA);
    // The same four selectors, re-derived from the literal's own bit
    // fields rather than from `blender_cycle_1`.
    assert_eq!((WM2000_OTHER_MODE_LOW >> 30) & 0x3, 0, "P = Combined");
    assert_eq!((WM2000_OTHER_MODE_LOW >> 26) & 0x3, 0, "A = Combined");
    assert_eq!((WM2000_OTHER_MODE_LOW >> 22) & 0x3, 1, "M = Framebuffer");
    assert_eq!((WM2000_OTHER_MODE_LOW >> 18) & 0x3, 0, "B = OneMinusA");
}

struct ResolvedBlendCycleUnderTest;
impl ResolvedBlendCycleUnderTest {
    fn of(mode: OtherMode) -> crate::blend::ResolvedBlendCycle {
        crate::blend::ResolvedBlendCycle::from_wire(mode.blender_cycle_1())
    }
}

/// **The hand-derivation the whole card rests on.**
///
/// WM2000's texrect combiner is `(Zero - Zero) * Zero + Primitive` with
/// `SetPrimColor 0xffffffdf`, so the combined fragment is RGB 255 at
/// alpha 223. The cycle above is `P = Combined, A = Combined,
/// M = Framebuffer, B = 1 - A`, and `blend_fragment`'s
/// `M == Framebuffer` arm makes the composite
/// `combined * (223/255) + destination * (1 - 223/255)`.
///
/// Derived here in the test, twice and by different routes: once as the
/// closed form, once by stepping the selector arms the same way the
/// blender does. The two must agree, so a transcription slip in either
/// contradicts itself rather than being confirmed by the implementation
/// it is supposed to check.
#[test]
fn the_wm2000_composite_is_hand_derived_over_a_zero_destination() {
    const COMBINED: [u8; 4] = [255, 255, 255, 223];
    let destination = BlendFramebufferSample {
        rgba: [0, 0, 0, 255],
        coverage_count: 8,
    };

    let a = f32::from(COMBINED[3]) / 255.0;
    let closed_form = (f32::from(COMBINED[0]) * a + 0.0 * (1.0 - a)).round() as u8;

    // The selector walk: P resolves to the combined color (cycle 0's
    // `Combined`), M to the framebuffer; the `M == Framebuffer` arm
    // keeps P as the blender color and makes A the composite factor.
    let p = f32::from(COMBINED[0]);
    let final_alpha = a;
    let stepped = (p * final_alpha + 0.0 * (1.0 - final_alpha)).round() as u8;
    assert_eq!(
        closed_form, stepped,
        "the two derivations of the same composite must agree"
    );
    assert_eq!(closed_form, 223, "255 * 223/255 over a zero destination");

    let blended = blend_texrect_fragment(COMBINED, destination, wm2000_blend_state(), 0, 0)
        .expect("WM2000's mode is admitted");
    assert_eq!(blended[0..3], [223, 223, 223]);
    // **A corrected derivation, kept as a correction.** The first draft
    // expected 223 here by assuming the alpha channel composites the
    // same way RGB does. It does not: `blend_fragment` composites alpha
    // as `255 * final_alpha + memory_alpha * (1 - final_alpha)`
    // (`crates/fn64-render-reference/src/raster/blend.rs:232-236`) --
    // the *source* alpha term is the constant 255, not the fragment's
    // own 223. With the destination's alpha byte also 255 the result is
    // 255 for every `final_alpha`. The test caught the wrong
    // expectation; the implementation was right.
    let memory_alpha = f32::from(destination.rgba[3]);
    let derived_alpha = (255.0 * a + memory_alpha * (1.0 - a)).round() as u8;
    assert_eq!(derived_alpha, 255);
    assert_eq!(blended[3], derived_alpha);

    // Full destination coverage supplies RGBA16 bit 0 independently of
    // the blender's alpha result.
    let mut packed = [0u8; 2];
    write_pixel(
        ColorTargetFormat::Rgba16,
        &mut packed,
        blended,
        Coverage::FULL,
    );
    let five = 223u16 >> 3;
    let expected = (five << 11) | (five << 6) | (five << 1) | 1;
    assert_eq!(u16::from_be_bytes(packed), expected);
    assert_eq!(
        expected, 0xdef7,
        "27 in all three channels, coverage bit set"
    );
    // Changing blended alpha cannot move bit 0; only destination coverage
    // can do that.
}

/// The unblended value the executor produced before this stage existed,
/// asserted as a **contrast**, so a regression that silently drops the
/// blender is a failing assertion rather than a quiet return to the
/// old output.
///
/// Asserted through [`blend_and_write_pixel`] -- **the executor's own
/// per-pixel composition**, the exact function the sampling loop calls
/// -- not through `blend_texrect_fragment` alone. Measured, not
/// stylistic: while this test went through the lower helper, deleting
/// the blender call from the pixel loop left this crate's entire suite
/// green and was caught only by `fn64-abi`'s whole-image comparison.
/// A mutant that survives is first a question about the test's reach.
#[test]
fn skipping_the_blender_would_produce_a_different_pixel() {
    const COMBINED: [u8; 4] = [255, 255, 255, 223];
    let mut unblended = [0u8; 2];
    write_pixel(
        ColorTargetFormat::Rgba16,
        &mut unblended,
        COMBINED,
        Coverage::FULL,
    );
    assert_eq!(
        u16::from_be_bytes(unblended),
        0xffff,
        "the pre-blend combiner output, which the port used to publish"
    );

    // A zero destination, which is what WM2000's Fill-cycle
    // `0x00010001` decodes to: RGB 0 with the coverage bit set.
    let mut stored = 0x0001u16.to_be_bytes();
    blend_and_write_pixel(
        ColorTargetFormat::Rgba16,
        &mut stored,
        COMBINED,
        wm2000_blend_state(),
        wm2000_stages(),
        0,
        0,
    )
    .expect("WM2000's mode is admitted");
    assert_ne!(
        u16::from_be_bytes(stored),
        u16::from_be_bytes(unblended),
        "running the blender must change this pixel; if it does not, the stage is not running"
    );
    assert_eq!(
        u16::from_be_bytes(stored),
        0xdef7,
        "the blended value derived in \
         `the_wm2000_composite_is_hand_derived_over_a_zero_destination`"
    );
}

/// The destination a pixel blends against is the **buffer being
/// written**, not the caller's incoming resident bytes, so two writes
/// to the same pixel compose serially the way the RDP's per-pixel
/// pipeline does.
///
/// Without this, reading `resident_bytes` instead would pass every
/// other test in this module -- every one of them writes each pixel
/// once.
#[test]
fn a_second_write_to_one_pixel_blends_against_the_first() {
    const COMBINED: [u8; 4] = [255, 255, 255, 223];
    let state = wm2000_blend_state();
    let mut stored = 0x0001u16.to_be_bytes();
    blend_and_write_pixel(
        ColorTargetFormat::Rgba16,
        &mut stored,
        COMBINED,
        state,
        wm2000_stages(),
        0,
        0,
    )
    .unwrap();
    let after_first = u16::from_be_bytes(stored);
    blend_and_write_pixel(
        ColorTargetFormat::Rgba16,
        &mut stored,
        COMBINED,
        state,
        wm2000_stages(),
        0,
        0,
    )
    .unwrap();
    let after_second = u16::from_be_bytes(stored);
    assert_ne!(
        after_first, after_second,
        "the second write must see the first's result as its destination"
    );

    // Hand-derived: the first write leaves 5-bit 27, which `read_pixel`
    // expands to `(27 << 3) | (27 >> 2)` = 222. Blending white at
    // 223/255 over 222 gives 222 + 33/255*... -- computed here rather
    // than quoted.
    let a = 223.0f32 / 255.0;
    let first_channel = ((27u8 << 3) | (27u8 >> 2)) as f32;
    let second = (255.0 * a + first_channel * (1.0 - a)).round() as u16;
    let five = second >> 3;
    assert_eq!(after_second >> 11, five);
}

/// Source and destination are **not** interchangeable in this
/// composite. Swapping them is a standard mutation, and without this
/// it survives on WM2000's own fixture only because its destination is
/// zero -- so the witness deliberately uses a non-zero destination.
///
/// **The first witness was wrong and is recorded as a correction.**
/// Alpha 128 was chosen by hand; at `128/255 = 0.50196` the composite is
/// symmetric to within a byte and the "swapped" result was *identical*
/// (`[108, 150, 145]` both ways), so the mutation it was written to
/// catch survived. Alpha 64 separates them (`[62, 175, 192]` vs
/// `[154, 125, 98]`). A hand-picked witness near `a = 1/2` proves
/// nothing here.
#[test]
fn the_blend_source_and_destination_are_not_interchangeable() {
    const COMBINED: [u8; 4] = [200, 100, 50, 64];
    let destination = BlendFramebufferSample {
        rgba: [16, 200, 240, 255],
        coverage_count: 8,
    };
    let state = wm2000_blend_state();
    let forward = blend_texrect_fragment(COMBINED, destination, state, 0, 0).unwrap();

    let swapped_combined = [
        destination.rgba[0],
        destination.rgba[1],
        destination.rgba[2],
        COMBINED[3],
    ];
    let swapped_destination = BlendFramebufferSample {
        rgba: [COMBINED[0], COMBINED[1], COMBINED[2], destination.rgba[3]],
        coverage_count: destination.coverage_count,
    };
    let reversed =
        blend_texrect_fragment(swapped_combined, swapped_destination, state, 0, 0).unwrap();
    assert_ne!(
        forward[0..3],
        reversed[0..3],
        "P and M are asymmetric: A weights the source and (1 - A) the destination"
    );

    // Hand-derived, both directions, at alpha 64/255.
    let a = f32::from(COMBINED[3]) / 255.0;
    let expect = |src: u8, dst: u8| (f32::from(src) * a + f32::from(dst) * (1.0 - a)).round() as u8;
    assert_eq!(forward[0], expect(200, 16));
    assert_eq!(forward[1], expect(100, 200));
    assert_eq!(forward[2], expect(50, 240));
}

/// Rounding, not truncation, and it is observable. The witness is
/// chosen by exhaustive search below rather than guessed -- most
/// (source, destination, alpha) triples round and truncate to the same
/// byte, which is exactly why a spot check would have let the mutation
/// live.
#[test]
fn the_blend_composite_rounds_rather_than_truncating() {
    let state = wm2000_blend_state();
    let mut witnesses = 0usize;
    for alpha in [1u8, 64, 128, 200, 223, 254] {
        for source in [0u8, 1, 7, 100, 200, 255] {
            for destination in [0u8, 3, 9, 128, 255] {
                let a = f32::from(alpha) / 255.0;
                let exact = f32::from(source) * a + f32::from(destination) * (1.0 - a);
                let rounded = exact.round() as u8;
                let truncated = exact as u8;
                if rounded == truncated {
                    continue;
                }
                witnesses += 1;
                let blended = blend_texrect_fragment(
                    [source, source, source, alpha],
                    BlendFramebufferSample {
                        rgba: [destination, destination, destination, 255],
                        coverage_count: 8,
                    },
                    state,
                    0,
                    0,
                )
                .unwrap();
                assert_eq!(
                    blended[0], rounded,
                    "source {source} over destination {destination} at alpha {alpha}: \
                     the composite must round ({rounded}), not truncate ({truncated})"
                );
            }
        }
    }
    assert!(
        witnesses > 0,
        "the sweep must contain at least one triple where rounding and truncation differ, \
         or it proves nothing about which one runs"
    );
}

/// `read_pixel`'s 5-bit expansion is the crate's existing one, asserted
/// against **the fill executor's own decode** rather than against a
/// literal.
///
/// The round-trip test below cannot catch this on its own, and that is
/// measured, not assumed: dropping the `>> 2` low-bit replication
/// leaves `write_pixel`'s `>> 3` recovering the same five bits, so the
/// round trip is preserved while every non-zero destination changes
/// (5-bit 27 expands to 222 with the replication and 216 without). The
/// mutant survived until this test existed.
///
/// The authority is [`crate::targets::decode_fill_cycle_pixel`], which
/// applies the identical `(value << 3) | (value >> 2)` to a fill colour
/// (`targets/fill.rs`'s `expand_five`) and is itself the port of the
/// oracle's `decode_16` (`fn64-render-reference/src/raster/draw.rs:130-142`).
/// A destination the fill executor wrote must decode back to the colour
/// the fill executor meant, or the two halves of a composed image
/// disagree about their shared format.
#[test]
fn read_pixel_expands_five_bit_channels_the_way_the_fill_decode_does() {
    use crate::state::FillColor;
    use crate::targets::decode_fill_cycle_pixel;
    for five in 0u16..32 {
        // A fill colour whose even-column halfword carries `five` in
        // all three channels with the coverage bit set.
        let halfword = (five << 11) | (five << 6) | (five << 1) | 1;
        let fill_word = (u32::from(halfword) << 16) | u32::from(halfword);
        let from_fill = decode_fill_cycle_pixel(
            FillColor::from_wire(fill_word),
            ColorTargetFormat::Rgba16,
            0,
        );
        let from_read = read_pixel(ColorTargetFormat::Rgba16, &halfword.to_be_bytes());
        assert_eq!(
            [from_read.rgba[0], from_read.rgba[1], from_read.rgba[2]],
            [from_fill.red, from_fill.green, from_fill.blue],
            "read_pixel and the fill decode must expand 5-bit {five} identically"
        );
        // And reconciled against an independent derivation of the same
        // expansion, so a shared slip in both would still contradict.
        let value = five as u8;
        assert_eq!(from_read.rgba[0], (value << 3) | (value >> 2));
    }
    // The witness that separates the two expansions, named so a
    // "simplification" to `<< 3` fails loudly rather than silently.
    assert_eq!(
        read_pixel(ColorTargetFormat::Rgba16, &(27u16 << 11).to_be_bytes()).rgba[0],
        222,
        "5-bit 27 expands to 222, not 216: the low-bit replication is load bearing"
    );
}

/// `read_pixel` is the exact inverse of `write_pixel` on every value
/// RGBA16 can hold, so a destination the executor wrote decodes back to
/// the color it meant. Exhaustive over all 65,536 halfwords, not
/// sampled.
///
/// **Necessary but not sufficient** -- see
/// `read_pixel_expands_five_bit_channels_the_way_the_fill_decode_does`
/// for the expansion this round trip cannot see.
#[test]
fn read_pixel_inverts_write_pixel_over_every_rgba16_halfword() {
    for raw in 0u16..=u16::MAX {
        let stored = raw.to_be_bytes();
        let sample = read_pixel(ColorTargetFormat::Rgba16, &stored);
        let mut round_tripped = [0u8; 2];
        let coverage = if raw & 1 == 0 {
            Coverage::new(1)
        } else {
            Coverage::FULL
        };
        write_pixel(
            ColorTargetFormat::Rgba16,
            &mut round_tripped,
            sample.rgba,
            coverage,
        );
        assert_eq!(
            u16::from_be_bytes(round_tripped),
            raw,
            "read_pixel then write_pixel must be the identity on {raw:#06x}"
        );
    }
}

/// Copy cycle runs no blender at all -- `cycle_count() == 0` is the
/// RDP's own bypass, not this executor declining to implement one. A
/// mutation that blends in Copy cycle changes the pixel; this catches
/// it.
#[test]
fn copy_cycle_passes_the_fragment_through_unblended() {
    let copy_mode = OtherMode::from_wire(2 << 20, WM2000_OTHER_MODE_LOW);
    assert_eq!(copy_mode.cycle_type(), CycleType::Copy);
    let state = TexrectBlendRegisters::new(Color4::from_wire(0), Color4::from_wire(0))
        .mode_state(copy_mode);
    assert_eq!(state.cycle_count(), 0);
    const TEXEL: [u8; 4] = [200, 100, 50, 128];
    let blended = blend_texrect_fragment(
        TEXEL,
        BlendFramebufferSample {
            rgba: [16, 200, 240, 255],
            coverage_count: 8,
        },
        state,
        0,
        0,
    )
    .unwrap();
    assert_eq!(blended, TEXEL, "Copy cycle blits the texel unchanged");
}

/// Each of the three admission refusals fires by name, and none of
/// them fires on WM2000's own mode.
#[test]
fn every_unevaluatable_blender_mode_is_refused_by_name() {
    assert_eq!(require_blendable_mode(wm2000_blend_state()), Ok(()));

    // FORCE_BL clear (bit 14) with AA_EN set (bit 3) AND IM_RD set
    // (bit 6): the one case where `blend_enabled` rests on the coverage
    // count. All three conjuncts are required; see
    // `a_clear_image_read_settles_blend_enabled_without_any_coverage_count`
    // for the IM_RD-clear case this refusal must NOT claim.
    let no_force = OtherMode::from_wire(WM2000_OTHER_MODE_HIGH, WM2000_OTHER_MODE_LOW & !0x4000);
    assert!(!no_force.force_blend());
    assert!(no_force.antialias_enabled(), "WM2000's mode sets AA_EN");
    assert!(no_force.image_read_enabled(), "WM2000's mode sets IM_RD");
    let state =
        TexrectBlendRegisters::new(Color4::from_wire(0), Color4::from_wire(0)).mode_state(no_force);
    assert_eq!(
        require_blendable_mode(state),
        Err(TexrectExecutionError::BlendEnabledNotDerivable)
    );

    // **The narrowing, pinned.** FORCE_BL clear AND AA_EN clear is
    // admitted, because `force_blend() || (antialias_enabled() &&
    // !wraps)` is then `false` outright with no `wraps` consulted.
    // Refusing this case too was measured wrong: three composed
    // one-cycle fixtures in `production.rs` latch other-mode low `0`
    // and had executed correctly for the life of the texrect path.
    let no_force_no_aa = OtherMode::from_wire(
        WM2000_OTHER_MODE_HIGH,
        WM2000_OTHER_MODE_LOW & !0x4000 & !0x0008,
    );
    assert!(!no_force_no_aa.force_blend());
    assert!(!no_force_no_aa.antialias_enabled());
    let state = TexrectBlendRegisters::new(Color4::from_wire(0), Color4::from_wire(0))
        .mode_state(no_force_no_aa);
    assert_eq!(require_blendable_mode(state), Ok(()));
    // And that admitted mode bypasses the blender: `is_last &&
    // !blend_enabled` selects P, which is `Combined`, leaving the
    // fragment unchanged. Derived from the selector, not from a run.
    const FRAGMENT: [u8; 4] = [200, 100, 50, 64];
    assert_eq!(
        crate::blend::ResolvedBlendCycle::from_wire(no_force_no_aa.blender_cycle_1()).p,
        BlendColorInput::Combined
    );
    assert_eq!(
        blend_texrect_fragment(
            FRAGMENT,
            BlendFramebufferSample {
                rgba: [16, 200, 240, 255],
                coverage_count: 8,
            },
            state,
            0,
            0,
        )
        .unwrap()[0..3],
        FRAGMENT[0..3],
        "the no-FORCE_BL last-cycle bypass selects P = Combined unchanged"
    );

    // A = Shade: cycle 1's alpha_a is bits 26:27, encoding 2.
    let shade_low = (WM2000_OTHER_MODE_LOW & !(0x3 << 26)) | (0x2 << 26);
    let shade = OtherMode::from_wire(WM2000_OTHER_MODE_HIGH, shade_low);
    assert_eq!(
        crate::blend::ResolvedBlendCycle::from_wire(shade.blender_cycle_1()).a,
        BlendAlphaInput::Shade
    );
    let state =
        TexrectBlendRegisters::new(Color4::from_wire(0), Color4::from_wire(0)).mode_state(shade);
    assert_eq!(
        require_blendable_mode(state),
        Err(TexrectExecutionError::UnsupportedBlendShadeAlpha)
    );

    // B = FramebufferAlpha: cycle 1's alpha_b is bits 18:19, encoding 1.
    let fba_low = (WM2000_OTHER_MODE_LOW & !(0x3 << 18)) | (0x1 << 18);
    let fba = OtherMode::from_wire(WM2000_OTHER_MODE_HIGH, fba_low);
    assert_eq!(
        crate::blend::ResolvedBlendCycle::from_wire(fba.blender_cycle_1()).b,
        BlendBInput::FramebufferAlpha
    );
    let state =
        TexrectBlendRegisters::new(Color4::from_wire(0), Color4::from_wire(0)).mode_state(fba);
    assert_eq!(
        require_blendable_mode(state),
        Err(TexrectExecutionError::UnsupportedBlendFramebufferAlpha)
    );
}

/// **D5, the second narrowing.** `BlendEnabledNotDerivable` claimed that
/// `FORCE_BL` clear with `AA_EN` set always rests on a coverage count.
/// It does not. The reference's own definition
/// (`fn64-render-reference/src/raster/coverage.rs:68-69`) is
///
/// ```text
/// wraps         = image_read_enabled && sum > 8
/// blend_enabled = force_blend || (antialias_enabled && !wraps)
/// ```
///
/// and `wraps` is a **conjunction whose first term is `image_read`**. A
/// clear `IM_RD` therefore pins `wraps` to `false` without the sum being
/// evaluated at all, and `blend_enabled` collapses to
/// `antialias_enabled()`, which this branch already knows is `true`. No
/// coverage count on either side is read, so the stated reason for the
/// refusal — "needs the destination coverage count this executor does
/// not maintain" — is simply not true of this case.
///
/// Hand-derived, not captured: the expectation below is read off the
/// two-line formula above, and the blended pixel off the resolved
/// selectors, never off a recorded run.
#[test]
fn a_clear_image_read_settles_blend_enabled_without_any_coverage_count() {
    // FORCE_BL clear (bit 14), AA_EN set (bit 3), IM_RD clear (bit 6).
    let no_force_no_read = OtherMode::from_wire(
        WM2000_OTHER_MODE_HIGH,
        (WM2000_OTHER_MODE_LOW & !0x4000) & !0x0040,
    );
    assert!(!no_force_no_read.force_blend());
    assert!(no_force_no_read.antialias_enabled());
    assert!(!no_force_no_read.image_read_enabled());
    let state = TexrectBlendRegisters::new(Color4::from_wire(0), Color4::from_wire(0))
        .mode_state(no_force_no_read);

    // Before this narrowing this returned `BlendEnabledNotDerivable`.
    assert_eq!(
        require_blendable_mode(state),
        Ok(()),
        "IM_RD clear pins `wraps` false, so `blend_enabled` is exactly \
         `antialias_enabled()` with no coverage count consulted"
    );

    // **The KEPT arm, pinned in the same test.** Setting IM_RD back —
    // and changing nothing else — must still refuse, because then and
    // only then does `wraps` depend on `pixel + memory > 8`. Without
    // this assertion, deleting the whole condition would pass.
    let read_enabled = OtherMode::from_wire(
        WM2000_OTHER_MODE_HIGH,
        (WM2000_OTHER_MODE_LOW & !0x4000) | 0x0040,
    );
    assert!(read_enabled.image_read_enabled());
    let refused = TexrectBlendRegisters::new(Color4::from_wire(0), Color4::from_wire(0))
        .mode_state(read_enabled);
    assert_eq!(
        require_blendable_mode(refused),
        Err(TexrectExecutionError::BlendEnabledNotDerivable),
        "with IM_RD set, `wraps` rests on a sum this executor cannot form"
    );

    // **`IM_RD` clear also removes the destination itself.** WM2000's
    // own cycle 1 selects `M = Framebuffer`, and with no image read
    // there is legally no destination sample, so `blend_fragment`
    // refuses by name rather than substituting one. That is the RDP's
    // rule, not a gap: the widening admits the *mode*, and this
    // orthogonal refusal still fires on the *program*.
    let cycle = crate::blend::ResolvedBlendCycle::from_wire(no_force_no_read.blender_cycle_1());
    assert_eq!(cycle.m, BlendColorInput::Framebuffer);
    const FRAGMENT: [u8; 4] = [200, 100, 50, 64];
    const DESTINATION: [u8; 4] = [16, 200, 240, 255];
    let sample = BlendFramebufferSample {
        rgba: DESTINATION,
        coverage_count: 8,
    };
    assert!(
        matches!(
            blend_texrect_fragment(FRAGMENT, sample, state, 0, 0),
            Err(TexrectExecutionError::Blend { .. })
        ),
        "a framebuffer term with IM_RD clear is refused by the blender, \
         not answered with an invented destination"
    );

    // **Positive control: the widened mode actually runs the mux.**
    // Admitting it would be worthless if every admitted fragment then
    // bypassed the blender. Swap `M` (cycle 1's color_b, bits 22:23) to
    // `Blend` (encoding 2) so the second term is a register rather than
    // the absent framebuffer, and supply that register.
    let mixing_low = ((WM2000_OTHER_MODE_LOW & !0x4000) & !0x0040 & !(0x3 << 22)) | (0x2 << 22);
    let mixing = OtherMode::from_wire(WM2000_OTHER_MODE_HIGH, mixing_low);
    assert!(!mixing.force_blend());
    assert!(mixing.antialias_enabled());
    assert!(!mixing.image_read_enabled());
    let resolved = crate::blend::ResolvedBlendCycle::from_wire(mixing.blender_cycle_1());
    assert_eq!(resolved.p, BlendColorInput::Combined);
    assert_eq!(resolved.m, BlendColorInput::Blend);
    const BLEND_REGISTER: [u8; 4] = [16, 200, 240, 255];
    let blend_color = Color4::from_wire(u32::from_be_bytes(BLEND_REGISTER));
    assert_eq!(blend_color.rgba8(), BLEND_REGISTER);
    let mixing_state =
        TexrectBlendRegisters::new(blend_color, Color4::from_wire(0)).mode_state(mixing);
    assert_eq!(require_blendable_mode(mixing_state), Ok(()));

    let blended = blend_texrect_fragment(FRAGMENT, sample, mixing_state, 0, 0)
        .expect("the widened mode evaluates");
    assert_ne!(
        blended[0..3],
        FRAGMENT[0..3],
        "an admitted-but-inert mode would prove nothing; with \
         `blend_enabled` true the last cycle must NOT take the \
         `is_last && !blend_enabled` P-passthrough"
    );
    // And it mixed the blend register in, not an invented constant:
    // every channel lands between the two operands.
    for channel in 0..3 {
        let low = FRAGMENT[channel].min(BLEND_REGISTER[channel]);
        let high = FRAGMENT[channel].max(BLEND_REGISTER[channel]);
        assert!(
            (low..=high).contains(&blended[channel]),
            "channel {channel}: {} is outside [{low}, {high}]",
            blended[channel]
        );
    }

    // **The kept arm's other half, pinned.** Reverting `blend_enabled`
    // to the old `force_blend()` alone would make this same program
    // bypass the mux and return the fragment unchanged. Deriving the
    // expected P-passthrough from the selector proves the two answers
    // are actually distinguishable, so the assertion above is not
    // vacuous.
    assert_eq!(
        crate::blend::ResolvedBlendCycle::from_wire(mixing.blender_cycle_1()).p,
        BlendColorInput::Combined,
        "the bypass this fix avoids would have returned P = Combined, \
         i.e. the fragment unchanged"
    );
}

/// A blender cycle reading a never-written `SetBlendColor`/
/// `SetFogColor` gets the register's power-on zero, not a refusal.
///
/// This replaces a test that asserted the refusal. The registers always
/// hold a value: `fn64-render-reference` zero-initializes both
/// (`gbi/state.rs:227-228`, `:387-388`) and RT64's C++ does the same at
/// `src/hle/rt64_state.cpp:130-131`.
#[test]
fn a_never_written_blender_register_reads_as_zero_instead_of_refusing() {
    // P = Blend is cycle 1's color_a (bits 30:31) encoding 2.
    let blend_low = (WM2000_OTHER_MODE_LOW & !(0x3u32 << 30)) | (0x2u32 << 30);
    let mode = OtherMode::from_wire(WM2000_OTHER_MODE_HIGH, blend_low);
    // Derived by hand: an unwritten register holds four zero bytes.
    let unwritten =
        TexrectBlendRegisters::new(Color4::from_wire(0), Color4::from_wire(0)).mode_state(mode);
    assert_eq!(unwritten.blend_color_register, [0, 0, 0, 0]);

    // A written register must reach `BlendModeState` unchanged, or the
    // assertion above could hold against a hardcoded zero. `rgba8`
    // unpacks the wire word big-endian, so 0x1122_3344 is [0x11, 0x22,
    // 0x33, 0x44] -- derived from the wire layout, not from the code
    // under test.
    let written = TexrectBlendRegisters::new(Color4::from_wire(0x1122_3344), Color4::from_wire(0))
        .mode_state(mode);
    assert_eq!(written.blend_color_register, [0x11, 0x22, 0x33, 0x44]);

    // A = Fog is cycle 1's alpha_a (bits 26:27) encoding 1.
    let fog_low = (WM2000_OTHER_MODE_LOW & !(0x3 << 26)) | (0x1 << 26);
    let mode = OtherMode::from_wire(WM2000_OTHER_MODE_HIGH, fog_low);
    let unwritten_fog =
        TexrectBlendRegisters::new(Color4::from_wire(0), Color4::from_wire(0)).mode_state(mode);
    assert_eq!(unwritten_fog.fog_color, [0, 0, 0, 0]);
    let written_fog =
        TexrectBlendRegisters::new(Color4::from_wire(0), Color4::from_wire(0x5566_7788))
            .mode_state(mode);
    assert_eq!(written_fog.fog_color, [0x55, 0x66, 0x77, 0x88]);

    // WM2000's own cycle reads neither register; both still carry their
    // real (zero) contents.
    let wm2000 = TexrectBlendRegisters::new(Color4::from_wire(0), Color4::from_wire(0))
        .mode_state(wm2000_other_mode());
    assert_eq!(wm2000.blend_color_register, [0, 0, 0, 0]);
    assert_eq!(wm2000.fog_color, [0, 0, 0, 0]);
}

/// `IM_RD` disabled with a `Framebuffer` selector is propagated as a
/// named error, never substituted with a zero destination.
#[test]
fn a_framebuffer_selector_without_image_read_is_refused_by_name() {
    let no_read = OtherMode::from_wire(WM2000_OTHER_MODE_HIGH, WM2000_OTHER_MODE_LOW & !0x0040);
    assert!(!no_read.image_read_enabled());
    let state =
        TexrectBlendRegisters::new(Color4::from_wire(0), Color4::from_wire(0)).mode_state(no_read);
    let error = blend_texrect_fragment(
        [255, 255, 255, 223],
        BlendFramebufferSample {
            rgba: [0, 0, 0, 255],
            coverage_count: 8,
        },
        state,
        7,
        9,
    )
    .expect_err("a Framebuffer selector with IM_RD clear has no legal destination");
    let TexrectExecutionError::Blend {
        column,
        row,
        source,
    } = error
    else {
        panic!("expected the blender's own refusal, got {error:?}");
    };
    assert_eq!((column, row), (7, 9), "the refusal names the pixel");
    assert_eq!(source.selector, "framebuffer color");
}
