//! Unit evidence for the guest-RDRAM VI scanout.
//!
//! Every expectation here is hand-derived from the VI register semantics and
//! the RGBA5551 packing, never captured from a run of the code under test.
//! Where a quantity has two independent derivations (the U2.10 coordinate,
//! the five-bit expansion) both are written out and reconciled, per the
//! standing brief's §3.2/§3.3.

use super::*;
use crate::rt64_vi_registers::{status, x_transform, y_transform};

/// Build a live fourteen-word VI image. Register indices are named against
/// the public VI interface order so a transposed word is visible here rather
/// than only in a failing pixel.
fn live_registers(
    st: u32,
    origin: u32,
    width: u32,
    h_start: u32,
    h_end: u32,
    v_start_half: u32,
    v_end_half: u32,
    x_scale: u32,
    y_scale: u32,
) -> ViScanoutRegisters {
    let mut words = [0u32; ViScanoutRegisters::WORD_COUNT];
    words[0] = st;
    words[1] = origin;
    words[2] = width;
    words[9] = (h_start << 16) | h_end;
    words[10] = (v_start_half << 16) | v_end_half;
    words[12] = x_scale;
    words[13] = y_scale;
    ViScanoutRegisters::from_words(words)
}

fn presentation(registers: ViScanoutRegisters) -> ViPresentation {
    ViPresentation {
        blanked: false,
        fade: None,
        repeat_line: false,
        scanout: fn64_render::ViScanoutState::Registers(registers),
        noise_seed: 0,
    }
}

/// VI STATUS for RGBA16 (`pixel type 2`) with AA mode 3 (replicate) and
/// every optional filter off. Built from the *ported RT64 field extents*
/// rather than a hand-typed literal, and reconciled against the literal --
/// two independent derivations of the same word (§3.2).
fn rgba16_replicate_status() -> u32 {
    let derived = (2 << status::TYPE.offset) | (3 << status::AA_MODE.offset);
    assert_eq!(
        derived, 0x0302,
        "the RGBA16+replicate STATUS word must agree between the ported rt64_vi.h field \
         offsets and its literal spelling"
    );
    derived
}

fn rgba32_replicate_status() -> u32 {
    let derived = (3 << status::TYPE.offset) | (3 << status::AA_MODE.offset);
    assert_eq!(derived, 0x0303);
    derived
}

/// Write one RGBA16 pixel into guest RDRAM through the *same* lane-mapped
/// authority the scanout reads through, so this fixture cannot encode a
/// second byte-order convention.
fn write_rgba16(rdram: &mut [u8], address: u32, pixel: u16) {
    fn64_runtime::RdramViewMut::from_storage(rdram)
        .write_u16(fn64_runtime::RdramAddr::from_offset(address), pixel);
}

fn fresh_rdram() -> Vec<u8> {
    vec![0u8; fn64_runtime::rdram::DEFAULT_RDRAM_SIZE]
}

/// Pack five-bit channels into an RGBA5551 halfword, the inverse of the
/// scanout's expansion.
const fn pack_rgba5551(r5: u8, g5: u8, b5: u8, a1: u8) -> u16 {
    ((r5 as u16) << 11) | ((g5 as u16) << 6) | ((b5 as u16) << 1) | (a1 as u16)
}

// ---------------------------------------------------------------------
// The coordinate convention, asserted as fn64's and NOT as RT64's.
// ---------------------------------------------------------------------

/// fn64 multiplies the raw U2.10 step; RT64 divides by
/// `xScaleFloat() = 1024.0f / xScale`. This test pins fn64's form.
///
/// **CORRECTION, found by a surviving mutant.** An earlier version of this
/// test asserted only `step = 3 << 9` at indices 0..3 and reasoned in a doc
/// comment that "fn64 never materializes a float, so no rounding can perturb
/// it". That reasoning is true and the test was still worthless: substituting
/// RT64's `(index / (1024.0 / step)) as u64` for the port's body left the
/// whole suite green. A convention claim needs an input where the two
/// conventions **disagree**, not an argument that they might.
///
/// The discriminator, found by sweeping every 12-bit step against indices
/// 0..64: at `step = 896, index = 8` the exact product is
/// `8 * 896 = 7168`, and `7168 >> 10` is exactly `7`. RT64's path computes
/// `1024.0f / 896.0f = 1.1428571f`, then `8.0f / 1.1428571f = 6.9999995f`,
/// which truncates to **6**. Same quantity, one whole sample apart -- the
/// exact "RT64 always rounding down at ties" shape the port brief warns
/// about. `step = 448, index = 16` and `step = 224, index = 32` reach the
/// same 7168 by different routes and separate identically.
#[test]
fn source_index_multiplies_the_raw_u2_10_step_with_no_float_reciprocal() {
    let axis = ViScaleAxis::from_register(3 << 9);
    // Hand-derived, entirely in integers:
    //   index 0 -> (0 + 0*1536) >> 10 = 0
    //   index 1 -> (0 + 1*1536) >> 10 = 1536 >> 10 = 1
    //   index 2 -> (0 + 2*1536) >> 10 = 3072 >> 10 = 3
    //   index 3 -> (0 + 3*1536) >> 10 = 4608 >> 10 = 4
    for (output, expected) in [(0u32, 0u64), (1, 1), (2, 3), (3, 4)] {
        assert_eq!(
            source_index(output, axis, 64),
            expected,
            "output index {output} must sample source row/column {expected} under fn64's \
             multiply-the-raw-step convention"
        );
    }

    // Second, independent derivation of the same four answers, written as
    // the explicit accumulator the register field describes rather than as
    // the port's expression (§3.3).
    let mut accumulator: u64 = u64::from(axis.offset_u2_10());
    for output in 0..4u32 {
        assert_eq!(
            accumulator >> 10,
            source_index(output, axis, 64),
            "the running U2.10 accumulator and the port's closed form must agree at {output}"
        );
        accumulator += u64::from(axis.step_u2_10());
    }

    // The discriminating cases. Each asserts fn64's answer AND that RT64's
    // reciprocal form would give a different one, so importing RT64's
    // convention breaks this test rather than sliding through it.
    for (step, index) in [(896u32, 8u32), (448, 16), (224, 32)] {
        let axis = ViScaleAxis::from_register(step);
        let fn64_answer = source_index(index, axis, 64);
        // Integer derivation, independent of the port: the exact product.
        assert_eq!(u64::from(index) * u64::from(step), 7168);
        assert_eq!(
            fn64_answer, 7,
            "fn64 multiplies the raw step: (0 + {index} * {step}) >> 10 = 7"
        );

        // RT64's form, written out literally in f32 as `rt64_vi.cpp` spells
        // it, and asserted to disagree. This is not the port's code path --
        // it exists only to prove the two conventions are separable here.
        let rt64_scale_float = 1024.0f32 / (step as f32);
        let rt64_answer = ((index as f32) / rt64_scale_float) as u64;
        assert_eq!(
            rt64_answer, 6,
            "RT64's 1024.0/{step} reciprocal divide floors this exact tie down to 6"
        );
        assert_ne!(
            fn64_answer, rt64_answer,
            "step {step} at index {index} must separate the two conventions; if it stops \
             doing so, this test has stopped proving anything and needs a new discriminator"
        );
    }
}

/// The offset field participates: a nonzero `X_OFFSET`/`Y_OFFSET` shifts
/// every sample. A port that read only the scale field would pass every
/// unit-scale test and fail this one.
#[test]
fn source_index_adds_the_programmed_u2_10_offset() {
    // Offset 2.0 (2 << 10) with unit step: output 0 samples source 2.
    let axis = ViScaleAxis::from_register((2 << 10) << 16 | u32::from(ViScaleAxis::ONE));
    assert_eq!(axis.offset_u2_10(), 2 << 10);
    assert_eq!(axis.step_u2_10(), ViScaleAxis::ONE);
    assert_eq!(source_index(0, axis, 64), 2);
    assert_eq!(source_index(1, axis, 64), 3);
    assert_eq!(source_index(5, axis, 64), 7);
}

/// The high edge is held rather than read out of bounds.
#[test]
fn source_index_holds_the_last_sample_at_the_high_edge() {
    let axis = ViScaleAxis::from_register(u32::from(ViScaleAxis::ONE));
    assert_eq!(source_index(3, axis, 4), 3);
    assert_eq!(source_index(4, axis, 4), 3);
    assert_eq!(source_index(4000, axis, 4), 3);
}

/// The ported RT64 `XTransform`/`YTransform` field extents and fn64's
/// `ViScaleAxis` decode must name the same twelve-bit scale and offset
/// halves. Two independent representations of one register layout,
/// reconciled (§3.2).
#[test]
fn rt64_transform_field_extents_agree_with_fn64_scale_axis_decode() {
    for register in [
        0x0000_0400u32,
        0x0800_0200,
        0x0fff_0fff,
        0x0123_0456,
        0xffff_ffff,
    ] {
        let axis = ViScaleAxis::from_register(register);
        assert_eq!(
            u32::from(axis.step_u2_10()),
            x_transform::X_SCALE.get(register),
            "the scale half of {register:#010x} must agree between rt64_vi.h's XTransform \
             field and ViScaleAxis"
        );
        assert_eq!(
            u32::from(axis.offset_u2_10()),
            x_transform::X_OFFSET.get(register),
            "the offset half of {register:#010x} must agree"
        );
        // YTransform is declared with the identical shape; assert it rather
        // than assume it.
        assert_eq!(
            x_transform::X_SCALE.get(register),
            y_transform::Y_SCALE.get(register)
        );
        assert_eq!(
            x_transform::X_OFFSET.get(register),
            y_transform::Y_OFFSET.get(register)
        );
    }
}

// ---------------------------------------------------------------------
// The five-bit expansion, proven exhaustively rather than spot-checked.
// ---------------------------------------------------------------------

/// TWO CORRECTIONS, both found by this test failing rather than by reading
/// the code -- §3.3's exact failure mode, twice in a row on the same
/// quantity.
///
/// 1. The independent derivation was first written as `value * 255 / 31`
///    **truncated**, on the reasoning that bit-replication is that rescale.
///    It is not: at `value = 4`, replication gives `0b100_00100 = 33` while
///    truncation gives `floor(32.90) = 32`.
/// 2. It was then rewritten as **round-to-nearest** (`+15`). That is also
///    wrong: at `value = 3`, replication gives `0b011_00011 = 24` while
///    round-to-nearest gives `round(24.68) = 25`.
///
/// Replication is neither. It is the leading eight bits of the *infinite
/// repetition* of the five-bit pattern -- `0.vvvvvvvvvv...` in binary,
/// truncated to eight fractional bits. The derivation below writes that out
/// literally (four repetitions is more than enough to fix the top eight
/// bits) and is genuinely independent of the port's two-shift form. The port
/// was right on both occasions and the expectation was wrong.
///
/// Disagreement counts, measured over all 32 inputs: truncation differs on
/// 15 values, round-to-nearest on 4. Neither is a spot-check away from
/// looking correct, which is why an exhaustive sweep is the assertion.
#[test]
fn five_bit_expansion_is_the_truncated_infinite_bit_repetition() {
    let mut truncation_disagreements = 0u32;
    let mut rounding_disagreements = 0u32;
    for value in 0..32u8 {
        let expanded = expand_five_bit(value);
        // Independent derivation: repeat the five-bit pattern four times
        // into a twenty-bit word and keep its top eight bits.
        let repeated = (u32::from(value) << 15)
            | (u32::from(value) << 10)
            | (u32::from(value) << 5)
            | u32::from(value);
        let independent = (repeated >> 12) as u8;
        assert_eq!(
            expanded, independent,
            "the replicate expansion of {value} must equal the top eight bits of its own \
             infinite bit repetition"
        );
        assert_eq!(expanded >> 3, value, "the expansion must be recoverable");

        if expanded != ((u32::from(value) * 255) / 31) as u8 {
            truncation_disagreements += 1;
        }
        if expanded != ((u32::from(value) * 255 + 15) / 31) as u8 {
            rounding_disagreements += 1;
        }
    }
    // The two endpoints, which every candidate rescale agrees on -- which is
    // exactly why checking only them would have missed both corrections.
    assert_eq!(expand_five_bit(0), 0);
    assert_eq!(expand_five_bit(31), 255);
    // The two witnesses, pinned so neither disproved form can return.
    assert_eq!(
        expand_five_bit(4),
        33,
        "separates replication from truncation"
    );
    assert_eq!((4u32 * 255) / 31, 32);
    assert_eq!(
        expand_five_bit(3),
        24,
        "separates replication from rounding"
    );
    assert_eq!((3u32 * 255 + 15) / 31, 25);
    assert_eq!(truncation_disagreements, 15);
    assert_eq!(rounding_disagreements, 4);
}

// ---------------------------------------------------------------------
// Scanout behavior.
// ---------------------------------------------------------------------

/// A 4x2 output over a 4-pixel-wide RGBA16 source at unit scale is the
/// identity: output pixel (x, y) is source pixel (x, y), expanded.
#[test]
fn unit_scale_rgba16_scanout_is_the_identity_over_the_source_rectangle() {
    const ORIGIN: u32 = 0x400;
    let mut rdram = fresh_rdram();
    // Hand-chosen five-bit channels, distinct per pixel so a transposed
    // row/column or a wrong stride is visible.
    let source: [(u8, u8, u8, u8); 8] = [
        (31, 0, 0, 1),
        (0, 31, 0, 1),
        (0, 0, 31, 1),
        (31, 31, 31, 1),
        (1, 2, 3, 0),
        (4, 5, 6, 0),
        (7, 8, 9, 1),
        (10, 11, 12, 1),
    ];
    for (index, &(r, g, b, a)) in source.iter().enumerate() {
        write_rgba16(
            &mut rdram,
            ORIGIN + index as u32 * 2,
            pack_rgba5551(r, g, b, a),
        );
    }

    let registers = live_registers(
        rgba16_replicate_status(),
        ORIGIN,
        4,
        100,
        104,
        20,
        24,
        u32::from(ViScaleAxis::ONE),
        u32::from(ViScaleAxis::ONE),
    );
    let memory = fn64_runtime::PhysicalRdramRead::from_storage(&rdram);
    let field = scan_out_guest_rdram(presentation(registers), &memory).unwrap();

    assert_eq!((field.width, field.height), (4, 2));
    for y in 0..2u32 {
        for x in 0..4u32 {
            let (r5, g5, b5, _) = source[(y * 4 + x) as usize];
            assert_eq!(
                field.pixel(x, y).unwrap(),
                [
                    expand_five_bit(r5),
                    expand_five_bit(g5),
                    expand_five_bit(b5),
                    255
                ],
                "output ({x}, {y}) must be source pixel {} expanded",
                y * 4 + x
            );
        }
    }
}

/// **VI STATUS bit 2 runs the gamma dither instead of refusing it** -- D17
/// of `docs/RT64-LANE-DIVERGENCES.md`.
///
/// The refusal this replaces said gamma dither "needs a retrace-seeded noise
/// generator this module does not own." It was already public in
/// `fn64_render::vi_public_filters`, which `vi_scanout.rs` imports
/// `restore_rgba16_component_bounded_v1` from one line away.
///
/// The expectation is hand-derived, not captured. For each output pixel and
/// each RGB channel the quantizer is
/// `q = (channel + bit) >> 1; (q << 1) | (q >> 6)`, with `bit` the
/// SplitMix64-derived `reference_noise_bit_v1(seed, pixel_index,
/// channel_index)`. Both are re-implemented in this test body from the
/// published expressions, independently of the module under test, so a
/// mutation to either half is caught here rather than compared against
/// itself.
///
/// The source channels are deliberately chosen so that the eight-bit
/// expansions are ODD: `expand_five_bit` is `(v << 3) | (v >> 2)`, whose low
/// bit is set exactly when the five-bit value's bit 2 is set. An odd channel
/// is the only kind the quantizer can move, so an even-only fixture would
/// pass against a no-op filter. `the_gamma_dither_moves_at_least_one_channel`
/// below asserts the fixture actually moves something rather than trusting
/// that reasoning.
///
/// Alpha is asserted untouched at 255: gamma dither is an RGB filter in both
/// this module and the reference.
#[test]
fn gamma_dither_runs_and_quantizes_each_rgb_channel_to_seven_bits() {
    const ORIGIN: u32 = 0x400;
    const SEED: u64 = 0x0123_4567_89ab_cdef;
    let mut rdram = fresh_rdram();
    // Every five-bit value here has bit 2 set, so every eight-bit expansion
    // is odd and therefore movable by the quantizer.
    let source: [(u8, u8, u8, u8); 8] = [
        (5, 7, 13, 1),
        (21, 29, 31, 1),
        (4 | 1, 4 | 2, 4 | 3, 1),
        (12, 20, 28, 1),
        (7, 5, 6, 1),
        (13, 15, 23, 1),
        (31, 21, 5, 1),
        (6, 14, 30, 1),
    ];
    for (index, &(r, g, b, a)) in source.iter().enumerate() {
        write_rgba16(
            &mut rdram,
            ORIGIN + index as u32 * 2,
            pack_rgba5551(r, g, b, a),
        );
    }

    // `rgba16_replicate_status()` plus VI STATUS bit 2 (GAMMA_DITHER_EN).
    let status_word = rgba16_replicate_status() | (1 << 2);
    let registers = live_registers(
        status_word,
        ORIGIN,
        4,
        100,
        104,
        20,
        24,
        u32::from(ViScaleAxis::ONE),
        u32::from(ViScaleAxis::ONE),
    );
    assert!(
        registers.filters().gamma_dither,
        "the fixture's STATUS word must actually select gamma dither"
    );
    let mut vi = presentation(registers);
    vi.noise_seed = SEED;
    let memory = fn64_runtime::PhysicalRdramRead::from_storage(&rdram);
    let field = scan_out_guest_rdram(vi, &memory).unwrap();

    assert_eq!((field.width, field.height), (4, 2));
    for y in 0..2u32 {
        for x in 0..4u32 {
            let pixel_index = (y * 4 + x) as u64;
            let (r5, g5, b5, _) = source[pixel_index as usize];
            let expected: Vec<u8> = [r5, g5, b5]
                .iter()
                .enumerate()
                .map(|(channel_index, &five)| {
                    hand_gamma_dither(
                        expand_five_bit(five),
                        SEED,
                        pixel_index,
                        channel_index as u8,
                    )
                })
                .collect();
            let actual = field.pixel(x, y).unwrap();
            assert_eq!(
                &actual[..3],
                expected.as_slice(),
                "output ({x}, {y}) RGB must be the seven-bit stochastic rounding of the \
                 expanded source"
            );
            assert_eq!(actual[3], 255, "gamma dither must not touch alpha");
        }
    }
}

/// **Positive control for the fixture above**: the filter is not the
/// identity on it.
///
/// Without this, `gamma_dither_runs_and_quantizes_each_rgb_channel_to_seven_bits`
/// could pass against a gamma dither that did nothing, since the quantizer
/// is the identity on every even channel. This asserts the same source,
/// scanned out with bit 2 clear, differs from the bit-2-set field -- and
/// names how many channels moved, so a fixture that degrades to one lucky
/// cell is visible.
#[test]
fn the_gamma_dither_moves_at_least_one_channel() {
    const ORIGIN: u32 = 0x400;
    const SEED: u64 = 0x0123_4567_89ab_cdef;
    let mut rdram = fresh_rdram();
    let source: [(u8, u8, u8, u8); 8] = [
        (5, 7, 13, 1),
        (21, 29, 31, 1),
        (5, 6, 7, 1),
        (12, 20, 28, 1),
        (7, 5, 6, 1),
        (13, 15, 23, 1),
        (31, 21, 5, 1),
        (6, 14, 30, 1),
    ];
    for (index, &(r, g, b, a)) in source.iter().enumerate() {
        write_rgba16(
            &mut rdram,
            ORIGIN + index as u32 * 2,
            pack_rgba5551(r, g, b, a),
        );
    }
    let memory = fn64_runtime::PhysicalRdramRead::from_storage(&rdram);

    let scan = |status_word: u32| {
        let registers = live_registers(
            status_word,
            ORIGIN,
            4,
            100,
            104,
            20,
            24,
            u32::from(ViScaleAxis::ONE),
            u32::from(ViScaleAxis::ONE),
        );
        let mut vi = presentation(registers);
        vi.noise_seed = SEED;
        scan_out_guest_rdram(vi, &memory).unwrap()
    };

    let plain = scan(rgba16_replicate_status());
    let dithered = scan(rgba16_replicate_status() | (1 << 2));
    let moved = plain
        .rgba8
        .iter()
        .zip(dithered.rgba8.iter())
        .filter(|(a, b)| a != b)
        .count();
    assert!(
        moved >= 4,
        "the gamma-dither fixture must exercise the quantizer on several channels, not one \
         lucky cell; only {moved} of {} bytes moved",
        plain.rgba8.len()
    );
}

/// **The seed reaches the filter.** Two different `noise_seed` values over
/// the same source must produce different fields, or the noise bit is being
/// derived from something the caller does not control.
#[test]
fn the_gamma_dither_noise_stream_is_keyed_by_the_retrace_seed() {
    const ORIGIN: u32 = 0x400;
    let mut rdram = fresh_rdram();
    for index in 0..8u32 {
        write_rgba16(&mut rdram, ORIGIN + index * 2, pack_rgba5551(5, 13, 21, 1));
    }
    let memory = fn64_runtime::PhysicalRdramRead::from_storage(&rdram);

    let scan = |seed: u64| {
        let registers = live_registers(
            rgba16_replicate_status() | (1 << 2),
            ORIGIN,
            4,
            100,
            104,
            20,
            24,
            u32::from(ViScaleAxis::ONE),
            u32::from(ViScaleAxis::ONE),
        );
        let mut vi = presentation(registers);
        vi.noise_seed = seed;
        scan_out_guest_rdram(vi, &memory).unwrap().rgba8
    };

    assert_ne!(
        scan(0),
        scan(0x0123_4567_89ab_cdef),
        "two retrace seeds over one source must give different dithered fields"
    );
}

/// **Gamma dither leaves alpha alone, proven on an alpha the quantizer
/// COULD move.**
///
/// The RGBA16 fixture above asserts alpha stays 255, which is worthless as
/// evidence: 255 is a fixed point of the quantizer
/// (`(255 + bit) >> 1 = 127`, `(127 << 1) | (127 >> 6) = 255`), so an
/// implementation that dithered all four channels would pass it. That
/// mutant survived until this test existed.
///
/// RGBA32 carries a rescaled coverage alpha that is not pinned to 255. The
/// source alpha byte `0x05` decodes to five bits 5 and expands to
/// `(5 << 3) | (5 >> 2) = 41`, which is odd and therefore movable: the
/// quantizer would take it to 40 or 42. Asserting it stays exactly 41 is
/// what makes "alpha is untouched" a real claim.
#[test]
fn gamma_dither_leaves_a_movable_alpha_untouched() {
    const ORIGIN: u32 = 0x2000;
    const SEED: u64 = 0x0123_4567_89ab_cdef;
    // Every RGB byte is odd so the RGB half of this fixture is movable too;
    // alpha 0x05 expands to 41, likewise odd.
    const PIXELS: [[u8; 4]; 2] = [[0x11, 0x23, 0x35, 0x05], [0xa9, 0xbb, 0xcd, 0x05]];
    let mut rdram = fresh_rdram();
    {
        let mut view = fn64_runtime::RdramViewMut::from_storage(&mut rdram);
        for (index, bytes) in PIXELS.into_iter().enumerate() {
            for (byte_index, byte) in bytes.into_iter().enumerate() {
                view.write_u8(
                    fn64_runtime::RdramAddr::from_offset(
                        ORIGIN + index as u32 * 4 + byte_index as u32,
                    ),
                    byte,
                );
            }
        }
    }

    let registers = live_registers(
        rgba32_replicate_status() | (1 << 2),
        ORIGIN,
        2,
        0,
        2,
        0,
        2,
        u32::from(ViScaleAxis::ONE),
        u32::from(ViScaleAxis::ONE),
    );
    let mut vi = presentation(registers);
    vi.noise_seed = SEED;
    let memory = fn64_runtime::PhysicalRdramRead::from_storage(&rdram);
    let field = scan_out_guest_rdram(vi, &memory).unwrap();

    // Independent derivation of the expanded alpha, not a call into the
    // module: five bits 5, expanded (5 << 3) | (5 >> 2).
    const EXPANDED_ALPHA: u8 = (5 << 3) | (5 >> 2);
    assert_eq!(EXPANDED_ALPHA, 41);
    let moved_alpha = hand_gamma_dither(EXPANDED_ALPHA, SEED, 0, 3);
    assert_ne!(
        moved_alpha, EXPANDED_ALPHA,
        "this fixture is only evidence if the quantizer WOULD move this alpha; it takes 41 to \
         {moved_alpha}"
    );

    for (index, source) in PIXELS.iter().enumerate() {
        let actual = field.pixel(index as u32, 0).unwrap();
        assert_eq!(
            actual[3], EXPANDED_ALPHA,
            "pixel {index}: gamma dither must not touch alpha, even one it could move"
        );
        for (channel_index, &byte) in source[..3].iter().enumerate() {
            assert_eq!(
                actual[channel_index],
                hand_gamma_dither(byte, SEED, index as u64, channel_index as u8),
                "pixel {index} channel {channel_index}: RGBA32 RGB is dithered from the raw \
                 eight-bit source byte"
            );
        }
    }
}

/// `gamma_dither_quantize_bounded_v1` composed with `reference_noise_bit_v1`,
/// re-derived here from their published expressions rather than called.
///
/// The point of re-deriving is that a mutation inside either shared function
/// must fail this file's assertions. Calling them would make the test a
/// tautology over whatever they currently do.
fn hand_gamma_dither(channel: u8, seed: u64, pixel_index: u64, channel_index: u8) -> u8 {
    // `reference_noise_bit_v1` (`fn64-render/src/vi_public_filters.rs:63-76`).
    let key = seed
        ^ pixel_index.wrapping_mul(0x9e37_79b9_7f4a_7c15)
        ^ (channel_index as u64).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    let mut mixed = key.wrapping_add(0x9e37_79b9_7f4a_7c15);
    mixed = (mixed ^ (mixed >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    mixed = (mixed ^ (mixed >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    let bit = (mixed ^ (mixed >> 31)) as u8 & 1;
    // `gamma_dither_quantize_bounded_v1` (`:56-59`).
    let quantized = channel.saturating_add(bit) >> 1;
    (quantized << 1) | (quantized >> 6)
}

/// The source stride, not the output width, selects the next row. A 4-wide
/// output over an 8-wide source must skip four pixels per row.
#[test]
fn the_source_stride_advances_rows_not_the_output_width() {
    const ORIGIN: u32 = 0x800;
    let mut rdram = fresh_rdram();
    // 8-pixel-wide source, 2 rows. Row 0 is red, row 1 is blue; the four
    // pixels past the output width are green so a stride-as-output-width
    // bug lands on them.
    for x in 0..8u32 {
        let row0 = if x < 4 {
            pack_rgba5551(31, 0, 0, 1)
        } else {
            pack_rgba5551(0, 31, 0, 1)
        };
        let row1 = if x < 4 {
            pack_rgba5551(0, 0, 31, 1)
        } else {
            pack_rgba5551(0, 31, 0, 1)
        };
        write_rgba16(&mut rdram, ORIGIN + x * 2, row0);
        write_rgba16(&mut rdram, ORIGIN + 16 + x * 2, row1);
    }

    let registers = live_registers(
        rgba16_replicate_status(),
        ORIGIN,
        8,
        0,
        4,
        0,
        4,
        u32::from(ViScaleAxis::ONE),
        u32::from(ViScaleAxis::ONE),
    );
    let memory = fn64_runtime::PhysicalRdramRead::from_storage(&rdram);
    let field = scan_out_guest_rdram(presentation(registers), &memory).unwrap();

    assert_eq!((field.width, field.height), (4, 2));
    for x in 0..4u32 {
        assert_eq!(
            field.pixel(x, 0).unwrap(),
            [255, 0, 0, 255],
            "row 0 column {x} must be red"
        );
        assert_eq!(
            field.pixel(x, 1).unwrap(),
            [0, 0, 255, 255],
            "row 1 column {x} must be blue -- green here means the row advanced by the \
             output width (4) instead of the source stride (8)"
        );
    }
}

/// The programmed origin selects the source, not address zero.
#[test]
fn the_programmed_origin_selects_the_source_rectangle() {
    let mut rdram = fresh_rdram();
    // Decoys at two other plausible origins.
    for address in [0u32, 0x400] {
        for x in 0..2u32 {
            write_rgba16(&mut rdram, address + x * 2, pack_rgba5551(31, 31, 0, 1));
        }
    }
    const ORIGIN: u32 = 0x1_0000;
    write_rgba16(&mut rdram, ORIGIN, pack_rgba5551(0, 31, 31, 1));
    write_rgba16(&mut rdram, ORIGIN + 2, pack_rgba5551(31, 0, 31, 1));

    let registers = live_registers(
        rgba16_replicate_status(),
        ORIGIN,
        2,
        0,
        2,
        0,
        2,
        u32::from(ViScaleAxis::ONE),
        u32::from(ViScaleAxis::ONE),
    );
    let memory = fn64_runtime::PhysicalRdramRead::from_storage(&rdram);
    let field = scan_out_guest_rdram(presentation(registers), &memory).unwrap();
    assert_eq!(field.pixel(0, 0).unwrap(), [0, 255, 255, 255]);
    assert_eq!(field.pixel(1, 0).unwrap(), [255, 0, 255, 255]);
}

/// RGBA32 reads four bytes per pixel through the lane-mapped byte accessor
/// and rescales the five-bit alpha/coverage byte.
#[test]
fn rgba32_scanout_reads_four_byte_pixels_and_rescales_coverage_alpha() {
    const ORIGIN: u32 = 0x2000;
    let mut rdram = fresh_rdram();
    {
        let mut view = fn64_runtime::RdramViewMut::from_storage(&mut rdram);
        for (index, bytes) in [[0x11u8, 0x22, 0x33, 0x1f], [0xaa, 0xbb, 0xcc, 0x00]]
            .into_iter()
            .enumerate()
        {
            for (byte_index, byte) in bytes.into_iter().enumerate() {
                view.write_u8(
                    fn64_runtime::RdramAddr::from_offset(
                        ORIGIN + index as u32 * 4 + byte_index as u32,
                    ),
                    byte,
                );
            }
        }
    }

    let registers = live_registers(
        rgba32_replicate_status(),
        ORIGIN,
        2,
        0,
        2,
        0,
        2,
        u32::from(ViScaleAxis::ONE),
        u32::from(ViScaleAxis::ONE),
    );
    let memory = fn64_runtime::PhysicalRdramRead::from_storage(&rdram);
    let field = scan_out_guest_rdram(presentation(registers), &memory).unwrap();
    // alpha 0x1f -> five bits 31 -> expand to 255; alpha 0x00 -> 0.
    assert_eq!(field.pixel(0, 0).unwrap(), [0x11, 0x22, 0x33, 255]);
    assert_eq!(field.pixel(1, 0).unwrap(), [0xaa, 0xbb, 0xcc, 0]);
}

/// A blanked field is black at the programmed rectangle -- the VI's own
/// behavior, and distinguishable from a refusal.
#[test]
fn a_blanked_field_is_black_at_the_programmed_output_rectangle() {
    let rdram = fresh_rdram();
    let registers = live_registers(
        rgba16_replicate_status(),
        0x400,
        4,
        0,
        4,
        0,
        4,
        u32::from(ViScaleAxis::ONE),
        u32::from(ViScaleAxis::ONE),
    );
    let mut vi = presentation(registers);
    vi.blanked = true;
    let memory = fn64_runtime::PhysicalRdramRead::from_storage(&rdram);
    let field = scan_out_guest_rdram(vi, &memory).unwrap();
    assert_eq!((field.width, field.height), (4, 2));
    assert!(field
        .rgba8
        .chunks_exact(4)
        .all(|pixel| pixel == [0, 0, 0, 255]));
}

/// A source rectangle running past physical RDRAM is a typed bounds
/// rejection, not a panic and not a clamped read.
#[test]
fn an_out_of_bounds_source_rectangle_is_a_typed_bounds_error() {
    let rdram = fresh_rdram();
    let origin = (fn64_runtime::rdram::DEFAULT_RDRAM_SIZE as u32) - 16;
    let registers = live_registers(
        rgba16_replicate_status(),
        origin,
        320,
        0,
        320,
        0,
        480,
        u32::from(ViScaleAxis::ONE),
        u32::from(ViScaleAxis::ONE),
    );
    let memory = fn64_runtime::PhysicalRdramRead::from_storage(&rdram);
    match scan_out_guest_rdram(presentation(registers), &memory) {
        Err(RenderError::InvalidViSourceBounds { origin: o, .. }) => assert_eq!(o, origin),
        other => panic!("expected a typed VI bounds rejection, got {other:?}"),
    }
}

/// An odd origin cannot address whole RGBA16 pixels.
#[test]
fn an_unaligned_rgba16_origin_is_a_typed_alignment_error() {
    let rdram = fresh_rdram();
    let registers = live_registers(
        rgba16_replicate_status(),
        0x401,
        4,
        0,
        4,
        0,
        4,
        u32::from(ViScaleAxis::ONE),
        u32::from(ViScaleAxis::ONE),
    );
    let memory = fn64_runtime::PhysicalRdramRead::from_storage(&rdram);
    match scan_out_guest_rdram(presentation(registers), &memory) {
        Err(RenderError::InvalidViSourceAlignment {
            origin,
            bytes_per_pixel,
        }) => {
            assert_eq!(origin, 0x401);
            assert_eq!(bytes_per_pixel, 2);
        }
        other => panic!("expected a typed VI alignment rejection, got {other:?}"),
    }
}

/// Every unimplemented filter is refused BY NAME. A generic "out of scope"
/// would pass a weaker version of this test; naming each one is what makes
/// the refusal actionable.
#[test]
fn every_unimplemented_vi_filter_is_refused_by_its_own_name() {
    let rdram = fresh_rdram();
    let memory = fn64_runtime::PhysicalRdramRead::from_storage(&rdram);
    let base = rgba16_replicate_status();

    let cases: [(u32, bool, bool, ViScanoutRefusal); 6] = [
        // AA mode 0: silhouette AA (and resampling; AA is checked first).
        (
            base & !(3 << 8),
            false,
            false,
            ViScanoutRefusal::SilhouetteAntialias,
        ),
        // AA mode 1: the same.
        (
            (base & !(3 << 8)) | (1 << 8),
            false,
            false,
            ViScanoutRefusal::SilhouetteAntialias,
        ),
        // AA mode 2 (resample only) and dither restoration over RGBA16 are
        // WM2000's measured pair and are both implemented, so neither
        // appears here. Bit 16 over RGBA32 has no five-bit dither to
        // restore and stays refused.
        (
            (base & !3) | 3 | (1 << 16),
            false,
            false,
            ViScanoutRefusal::DitherRestorationNonRgba16,
        ),
        (base | (1 << 4), false, false, ViScanoutRefusal::Divot),
        (base | (1 << 3), false, false, ViScanoutRefusal::Gamma),
        (base, true, false, ViScanoutRefusal::Fade),
    ];

    for (status_word, fade, repeat, expected) in cases {
        let registers = live_registers(
            status_word,
            0x400,
            4,
            0,
            4,
            0,
            4,
            u32::from(ViScaleAxis::ONE),
            u32::from(ViScaleAxis::ONE),
        );
        let mut vi = presentation(registers);
        vi.fade = fade.then_some(0);
        vi.repeat_line = repeat;
        let error = scan_out_guest_rdram(vi, &memory)
            .expect_err("an unimplemented VI filter must be refused, never silently ignored");
        let RenderError::Backend { backend, reason } = &error else {
            panic!("expected a named backend refusal, got {error:?}");
        };
        assert_eq!(*backend, "render-wgpu-vi-scanout");
        assert_eq!(
            reason,
            expected.reason(),
            "STATUS {status_word:#010x} (fade={fade}, repeat={repeat}) must be refused as \
             {expected:?}"
        );
        assert!(
            !reason.contains("out of scope"),
            "a refusal must name the filter, not say 'out of scope'"
        );
    }

    // repeat_line, checked separately so its own row cannot be masked by a
    // status bit.
    let registers = live_registers(
        base,
        0x400,
        4,
        0,
        4,
        0,
        4,
        u32::from(ViScaleAxis::ONE),
        u32::from(ViScaleAxis::ONE),
    );
    let mut vi = presentation(registers);
    vi.repeat_line = true;
    let error = scan_out_guest_rdram(vi, &memory).unwrap_err();
    assert_eq!(
        error.to_string(),
        RenderError::Backend {
            backend: "render-wgpu-vi-scanout",
            reason: ViScanoutRefusal::RepeatLine.reason().to_string(),
        }
        .to_string()
    );
}

/// Every refusal reason is distinct and none of them is the generic text the
/// old `present` returned. A copy-paste that gave two filters the same
/// message would make a rejection unactionable.
#[test]
fn refusal_reasons_are_distinct_and_never_generic() {
    let all = [
        ViScanoutRefusal::SilhouetteAntialias,
        ViScanoutRefusal::DitherRestorationNonRgba16,
        ViScanoutRefusal::Divot,
        ViScanoutRefusal::Gamma,
        ViScanoutRefusal::Fade,
        ViScanoutRefusal::RepeatLine,
        ViScanoutRefusal::ReservedPixelType,
    ];
    let mut seen = std::collections::BTreeSet::new();
    for refusal in all {
        let reason = refusal.reason();
        assert!(
            seen.insert(reason),
            "{refusal:?} reuses another refusal's reason text"
        );
        assert!(!reason.contains("out of scope"), "{refusal:?} is generic");
        assert!(!reason.is_empty());
    }
    assert_eq!(seen.len(), all.len());
}

/// A backend-only presentation has no register image to read guest memory
/// through, and says so by name.
#[test]
fn a_backend_only_presentation_names_the_missing_register_image() {
    let rdram = fresh_rdram();
    let memory = fn64_runtime::PhysicalRdramRead::from_storage(&rdram);
    let vi = ViPresentation::default();
    let error = scan_out_guest_rdram(vi, &memory).unwrap_err();
    let RenderError::Backend { reason, .. } = &error else {
        panic!("expected a named refusal, got {error:?}");
    };
    assert!(
        reason.contains("live fourteen-word register image"),
        "got: {reason}"
    );
}

/// The exact STATUS word measured on the real WM2000 ROM's first present:
/// pixel type 0 (`Blank`) with AA mode 0 (`AaResampleAlways`).
///
/// Hand-derived from `ViFilterControl::from_status`' documented bit layout,
/// not captured from this module's output: type occupies bits 0..=1 and the
/// AA selector bits 8..=9, so a blanked field requesting AA-always is
/// `(0 << 0) | (0 << 8) == 0x0000`.
fn wm2000_blank_aa_always_status() -> u32 {
    let derived = (0 << status::TYPE.offset) | (0 << status::AA_MODE.offset);
    assert_eq!(derived, 0x0000);
    let filters = fn64_render::ViFilterControl::from_status(derived);
    assert_eq!(filters.pixel_type, ViPixelType::Blank);
    assert_eq!(
        filters.antialias_mode,
        fn64_render::ViAaMode::AaResampleAlways
    );
    assert!(
        filters.antialias_mode.silhouette_aa_enabled(),
        "positive control: this fixture must genuinely latch a silhouette AA mode, \
         otherwise the blanked-field test below would pass vacuously"
    );
    derived
}

/// A blanked field that *also* selects silhouette AA is black, not a
/// refusal.
///
/// This is the WM2000 case. A blanked field scans out no source pixels, so
/// the coverage filter has nothing to transform and cannot make the output
/// wrong. `fn64-render-reference`'s `scanout` returns the cleared field on
/// `pixel_type == Blank` before it consults `silhouette_aa_enabled`; this
/// asserts the wgpu backend agrees rather than refusing a filter that
/// cannot apply.
#[test]
fn a_blanked_field_selecting_silhouette_aa_is_black_not_a_refusal() {
    let rdram = fresh_rdram();
    let registers = live_registers(
        wm2000_blank_aa_always_status(),
        0x400,
        4,
        0,
        4,
        0,
        4,
        u32::from(ViScaleAxis::ONE),
        u32::from(ViScaleAxis::ONE),
    );
    let memory = fn64_runtime::PhysicalRdramRead::from_storage(&rdram);
    let field = scan_out_guest_rdram(presentation(registers), &memory)
        .expect("a blanked field must present black, not refuse an inapplicable filter");
    assert_eq!((field.width, field.height), (4, 2));
    assert!(
        field
            .rgba8
            .chunks_exact(4)
            .all(|pixel| pixel == [0, 0, 0, 255]),
        "a blanked field is opaque black at the programmed rectangle"
    );
}

/// The blanked admission is scoped to blanked fields only: an *unblanked*
/// field selecting silhouette AA still refuses by name.
///
/// This is the guard that the ordering fix did not weaken the refusal. The
/// only difference from the test above is the pixel type.
#[test]
fn an_unblanked_field_selecting_silhouette_aa_still_refuses_by_name() {
    let rdram = fresh_rdram();
    let status = (2 << status::TYPE.offset) | (0 << status::AA_MODE.offset);
    let filters = fn64_render::ViFilterControl::from_status(status);
    assert_eq!(filters.pixel_type, ViPixelType::Rgba16);
    assert!(filters.antialias_mode.silhouette_aa_enabled());
    let registers = live_registers(
        status,
        0x400,
        4,
        0,
        4,
        0,
        4,
        u32::from(ViScaleAxis::ONE),
        u32::from(ViScaleAxis::ONE),
    );
    let memory = fn64_runtime::PhysicalRdramRead::from_storage(&rdram);
    let error = scan_out_guest_rdram(presentation(registers), &memory)
        .expect_err("an RGBA16 field selecting silhouette AA must still refuse");
    match error {
        RenderError::Backend { backend, reason } => {
            assert_eq!(backend, "render-wgpu-vi-scanout");
            assert_eq!(reason, ViScanoutRefusal::SilhouetteAntialias.reason());
        }
        other => panic!("expected the named silhouette refusal, got {other:?}"),
    }
}

/// `vi.blanked` (osViBlack) reaches the same admission as STATUS type 0,
/// even when a filter this module does not implement is also selected.
#[test]
fn os_vi_black_with_an_unimplemented_filter_is_black_not_a_refusal() {
    let rdram = fresh_rdram();
    let status = (2 << status::TYPE.offset) | (0 << status::AA_MODE.offset);
    let registers = live_registers(
        status,
        0x400,
        4,
        0,
        4,
        0,
        4,
        u32::from(ViScaleAxis::ONE),
        u32::from(ViScaleAxis::ONE),
    );
    let mut vi = presentation(registers);
    vi.blanked = true;
    let memory = fn64_runtime::PhysicalRdramRead::from_storage(&rdram);
    let field = scan_out_guest_rdram(vi, &memory)
        .expect("osViBlack must present black regardless of the latched filter selection");
    assert!(field
        .rgba8
        .chunks_exact(4)
        .all(|pixel| pixel == [0, 0, 0, 255]));
}

/// A blanked field still refuses the *reserved* pixel type, which is a
/// malformed register image rather than an inapplicable filter.
#[test]
fn a_reserved_pixel_type_still_refuses_ahead_of_the_blank_admission() {
    let rdram = fresh_rdram();
    let status = 1 << status::TYPE.offset;
    assert_eq!(
        fn64_render::ViFilterControl::from_status(status).pixel_type,
        ViPixelType::Reserved
    );
    let registers = live_registers(
        status,
        0x400,
        4,
        0,
        4,
        0,
        4,
        u32::from(ViScaleAxis::ONE),
        u32::from(ViScaleAxis::ONE),
    );
    let mut vi = presentation(registers);
    vi.blanked = true;
    let memory = fn64_runtime::PhysicalRdramRead::from_storage(&rdram);
    let error = scan_out_guest_rdram(vi, &memory)
        .expect_err("reserved pixel type 1 is malformed and refuses even when blanked");
    match error {
        RenderError::Backend { reason, .. } => {
            assert_eq!(reason, ViScanoutRefusal::ReservedPixelType.reason());
        }
        other => panic!("expected the reserved-pixel-type refusal, got {other:?}"),
    }
}

// ---------------------------------------------------------------------
// VI dither restoration (STATUS bit 16) and resampling (AA modes 0/1/2).
//
// Measured on the real WM2000 ROM under `FN64_RENDER=wgpu`: fields 0-19 are
// blanked, and field 20 -- the first with content -- latches
// `status = 0x00013202`. Decoded that is RGBA16 (bits 0-1 = 2), AA mode 2
// (`ResampleOnly`), `dither_filter` set (bit 16), and `gamma`,
// `gamma_dither`, `divot`, `fade`, `repeat_line` all clear. Those two
// filters are the whole of what this ROM asks for, and the whole of what
// the section below implements. The other six stay refusing by name.
// ---------------------------------------------------------------------

/// WM2000's measured STATUS word, rebuilt from the ported `rt64_vi.h` field
/// extents and reconciled against the literal the live run printed. Two
/// independent derivations of one quantity (§3.2): if a field offset were
/// wrong, the two spellings would disagree here rather than in a pixel.
fn wm2000_measured_status() -> u32 {
    let derived = (2 << status::TYPE.offset)
        | (2 << status::AA_MODE.offset)
        | (3 << status::PIXEL_ADVANCE.offset)
        | (1 << status::DITHER_FILTER.offset);
    // Rebuilt from the ported `rt64_vi.h` field extents, reconciled against
    // the literal the live WM2000 run printed. `PIXEL_ADVANCE = 3` is part
    // of the measured word and is included so this is the real register
    // image, not a filter-only subset -- it selects no scanout filter and
    // this module does not consume it.
    assert_eq!(
        derived, 0x0001_3202,
        "the rebuilt WM2000 STATUS word must equal the one the live run printed"
    );
    assert_eq!(
        fn64_render::ViFilterControl::from_status(derived),
        fn64_render::ViFilterControl {
            pixel_type: ViPixelType::Rgba16,
            antialias_mode: fn64_render::ViAaMode::ResampleOnly,
            gamma: false,
            gamma_dither: false,
            divot: false,
            dither_filter: true,
        },
        "WM2000's measured field latches exactly dither restoration and resampling"
    );
    derived
}

/// The measured word is the one the running ROM actually programmed. Pinned
/// as a literal so a later refactor of the field extents cannot quietly
/// redefine what "WM2000 latches" means.
#[test]
fn the_wm2000_measured_status_word_selects_exactly_two_filters() {
    assert_eq!(wm2000_measured_status(), 0x0001_3202);
    let filters = fn64_render::ViFilterControl::from_status(0x0001_3202);
    assert!(filters.dither_filter, "bit 16 is set in the measured word");
    assert!(
        filters.antialias_mode.resampling_enabled(),
        "AA mode 2 resamples"
    );
    assert!(
        !filters.antialias_mode.silhouette_aa_enabled(),
        "AA mode 2 is NOT a silhouette-AA mode; mode 0/1 would be"
    );
    assert!(!filters.gamma && !filters.gamma_dither && !filters.divot);
}

/// Coverage is the RGBA16 low bit, exactly as `fn64-render-reference`'s
/// `load_vi_source` derives it on a hidden-sidecar miss. Hand-derived from
/// that composition, not captured:
///
/// ```text
/// bits   = if pixel & 1 == 0 { 0 } else { 3 }
/// stored = ((pixel & 1) << 2) | bits      -> 0b000 or 0b111
/// count  = (stored & 7) + 1               -> 1 or 8
/// ```
#[test]
fn rgba16_coverage_is_the_low_bit_expanded_the_reference_way() {
    let mut rdram = fresh_rdram();
    write_rgba16(&mut rdram, 0x400, pack_rgba5551(1, 2, 3, 0));
    write_rgba16(&mut rdram, 0x402, pack_rgba5551(1, 2, 3, 1));
    let memory = fn64_runtime::PhysicalRdramRead::from_storage(&rdram);
    let geometry = SourceGeometry {
        origin: 0x400,
        stride_pixels: 2,
        rows: 1,
        bytes_per_pixel: 2,
        pixel_type: ViPixelType::Rgba16,
    };
    assert_eq!(
        geometry.coverage(&memory, 0, 0),
        1,
        "a clear low bit is minimum coverage"
    );
    assert_eq!(
        geometry.coverage(&memory, 1, 0),
        8,
        "a set low bit is full coverage, which is what dither restoration filters"
    );
}

/// Hand-derived restoration of one interior pixel.
///
/// A 3x3 RGBA16 plane, every alpha bit set so every pixel is full-coverage.
/// The red five-bit values are
///
/// ```text
///   4  9  4
///   9  8  9      center = 8, neighbors = [4,9,4,9,9,4,9,4]
///   4  9  4
/// ```
///
/// US 5,699,079's restoration is `(center << 3)` plus one per greater
/// neighbor and minus one per lesser: `64 + 4 - 4 = 64`. Derived a second
/// way, the four 9s contribute `+4` and the four 4s `-4`, which cancel, so
/// the center is unchanged at `8 << 3 = 64`. Both spellings agree (§3.3).
///
/// The blue channel uses `2 2 2 / 2 5 2 / 2 2 2`: all eight neighbors are
/// lesser, so `40 - 8 = 32`.
#[test]
fn dither_restoration_matches_the_hand_derived_signed_neighbor_sum() {
    let mut rdram = fresh_rdram();
    let red = [[4u8, 9, 4], [9, 8, 9], [4, 9, 4]];
    let blue = [[2u8, 2, 2], [2, 5, 2], [2, 2, 2]];
    for row in 0..3usize {
        for column in 0..3usize {
            write_rgba16(
                &mut rdram,
                0x400 + ((row * 3 + column) as u32) * 2,
                pack_rgba5551(red[row][column], 0, blue[row][column], 1),
            );
        }
    }
    let memory = fn64_runtime::PhysicalRdramRead::from_storage(&rdram);
    let geometry = SourceGeometry {
        origin: 0x400,
        stride_pixels: 3,
        rows: 3,
        bytes_per_pixel: 2,
        pixel_type: ViPixelType::Rgba16,
    };
    let mut plane = SourcePlane::load(geometry, &memory);
    assert_eq!(
        plane.component(1, 1, 0),
        expand_five_bit(8),
        "before restoration the centre is the plain five-bit expansion"
    );
    plane.restore_dither();
    assert_eq!(
        plane.component(1, 1, 0),
        64,
        "four greater and four lesser neighbours cancel: (8 << 3) + 4 - 4"
    );
    assert_eq!(
        plane.component(1, 1, 2),
        32,
        "eight lesser neighbours: (5 << 3) - 8"
    );
}

/// Restoration reads a pre-filter snapshot. Filtering in place would feed an
/// already-restored neighbour back in, making the result depend on scan
/// order; this fixture is asymmetric left-to-right so that would show.
#[test]
fn dither_restoration_reads_unrestored_neighbors_only() {
    let mut rdram = fresh_rdram();
    let red = [0u8, 31, 0, 31, 0];
    for (index, value) in red.iter().enumerate() {
        write_rgba16(
            &mut rdram,
            0x400 + (index as u32) * 2,
            pack_rgba5551(*value, 0, 0, 1),
        );
    }
    let memory = fn64_runtime::PhysicalRdramRead::from_storage(&rdram);
    let geometry = SourceGeometry {
        origin: 0x400,
        stride_pixels: 5,
        rows: 1,
        bytes_per_pixel: 2,
        pixel_type: ViPixelType::Rgba16,
    };
    let mut plane = SourcePlane::load(geometry, &memory);
    plane.restore_dither();
    // Pixel 1 (value 31) has neighbours 0 and 0 in the source: both lesser,
    // so (31 << 3) - 2 = 246. Had pixel 0 been restored first (0 with a
    // greater neighbour -> 1), pixel 1 would still see 0 only if the
    // snapshot is honoured.
    assert_eq!(plane.component(1, 0, 0), 246);
    assert_eq!(
        plane.component(0, 0, 0),
        1,
        "(0 << 3) + 1 for its single greater neighbour"
    );
}

/// Only full-coverage pixels are restored. A clear low bit means coverage 1,
/// which this backend passes through untouched -- the same `continue` the
/// reference takes when silhouette AA is off.
#[test]
fn dither_restoration_skips_partial_coverage_pixels() {
    let mut rdram = fresh_rdram();
    // Centre has a CLEAR low bit; its neighbours are all set and all lesser.
    for index in 0..3u32 {
        write_rgba16(&mut rdram, 0x400 + index * 2, pack_rgba5551(2, 0, 0, 1));
    }
    write_rgba16(&mut rdram, 0x402, pack_rgba5551(20, 0, 0, 0));
    let memory = fn64_runtime::PhysicalRdramRead::from_storage(&rdram);
    let geometry = SourceGeometry {
        origin: 0x400,
        stride_pixels: 3,
        rows: 1,
        bytes_per_pixel: 2,
        pixel_type: ViPixelType::Rgba16,
    };
    let mut plane = SourcePlane::load(geometry, &memory);
    plane.restore_dither();
    assert_eq!(
        plane.component(1, 0, 0),
        expand_five_bit(20),
        "a partial-coverage pixel keeps its plain expansion, unrestored"
    );
}

/// Bilinear interpolation at the exact half-step, hand-derived.
///
/// Two source columns with red 0 and 31 -> expanded 0 and 255. A U0.10
/// fraction of 512 weights them 512/1024 each:
/// `(0*512 + 255*512 + 512) / 1024 = (130560 + 512) / 1024 = 128`.
/// Derived a second way: the midpoint of 0 and 255 is 127.5, and the
/// `+ ONE/2` bias rounds half up to 128. Both agree (§3.3).
#[test]
fn bilinear_interpolation_at_the_half_step_is_the_hand_derived_midpoint() {
    assert_eq!(interpolate_u2_10(0, 255, 512), 128);
    assert_eq!((0 * 512 + 255 * 512 + 512) / 1024, 128);
    // Endpoints are exact: weight 0 is pure lower, and the generator never
    // emits weight 1024 (that is the next integer position).
    assert_eq!(interpolate_u2_10(17, 200, 0), 17);
    assert_eq!(interpolate_u2_10(17, 200, 1023), 200);
}

/// `AxisSample::from_output(..).lower` must equal `source_index` for every
/// output position, on both the interpolating and the held-edge side. The
/// two are independent derivations of the same nearest index (§3.2); this is
/// what lets `replicate` supersede the original sampling without changing a
/// single pixel.
#[test]
fn replication_agrees_with_source_index_on_every_output_column() {
    for step in [256u16, 512, 1024, 1536, 2048, 3000] {
        for offset in [0u16, 1, 511, 1023] {
            let axis = ViScaleAxis::from_register((u32::from(offset) << 16) | u32::from(step));
            for extent in [1usize, 2, 5, 64] {
                for output in 0..40u32 {
                    assert_eq!(
                        AxisSample::from_output(output, axis, extent).lower as u64,
                        source_index(output, axis, extent as u64),
                        "step={step} offset={offset} extent={extent} output={output}"
                    );
                }
            }
        }
    }
}

/// The high edge holds the last sample and forces the weight to zero, so a
/// clamped position repeats rather than extrapolating past the source.
#[test]
fn the_resampling_high_edge_holds_the_last_sample_with_zero_weight() {
    let axis = ViScaleAxis::from_register(u32::from(ViScaleAxis::ONE));
    let sample = AxisSample::from_output(9, axis, 4);
    assert_eq!(sample.lower, 3);
    assert_eq!(sample.upper, 3);
    assert_eq!(sample.fraction_u0_10, 0);
}

/// A unit-scale resample is the identity: every position lands exactly on a
/// source sample with weight zero, so interpolation cannot perturb it. This
/// is the positive control proving the resampling path RUNS -- it is on the
/// AA-mode-2 branch, not the replicate one.
#[test]
fn unit_scale_resampling_is_the_identity_and_still_takes_the_resample_branch() {
    let mut rdram = fresh_rdram();
    let values = [1u8, 7, 19, 31];
    for (index, value) in values.iter().enumerate() {
        write_rgba16(
            &mut rdram,
            0x400 + (index as u32) * 2,
            pack_rgba5551(*value, 0, 0, 0),
        );
    }
    let memory = fn64_runtime::PhysicalRdramRead::from_storage(&rdram);
    // AA mode 2 with dither restoration OFF, so only resampling runs.
    let status_word = (2 << status::TYPE.offset) | (2 << status::AA_MODE.offset);
    let registers = live_registers(
        status_word,
        0x400,
        4,
        0,
        4,
        0,
        2,
        u32::from(ViScaleAxis::ONE),
        u32::from(ViScaleAxis::ONE),
    );
    let field = scan_out_guest_rdram(presentation(registers), &memory)
        .expect("AA mode 2 must scan out, not refuse");
    assert!(
        fn64_render::ViFilterControl::from_status(status_word)
            .antialias_mode
            .resampling_enabled(),
        "positive control: this fixture really is on the resampling branch"
    );
    for (index, value) in values.iter().enumerate() {
        assert_eq!(
            field.pixel(index as u32, 0).unwrap()[0],
            expand_five_bit(*value),
            "unit scale must reproduce source column {index} exactly"
        );
    }
}

/// A half-step horizontal resample interpolates between neighbours. Source
/// red values 0 and 31 expand to 0 and 255; output column 1 sits at fraction
/// 512 and must be the hand-derived 128, which nearest-neighbour replication
/// could never produce. This is the mutation-style positive control for
/// bilinear: replace `resample_bilinear` with `replicate` and this fails.
#[test]
fn a_half_step_resample_interpolates_where_replication_would_not() {
    let mut rdram = fresh_rdram();
    write_rgba16(&mut rdram, 0x400, pack_rgba5551(0, 0, 0, 0));
    write_rgba16(&mut rdram, 0x402, pack_rgba5551(31, 0, 0, 0));
    let memory = fn64_runtime::PhysicalRdramRead::from_storage(&rdram);
    let geometry = SourceGeometry {
        origin: 0x400,
        stride_pixels: 2,
        rows: 1,
        bytes_per_pixel: 2,
        pixel_type: ViPixelType::Rgba16,
    };
    let plane = SourcePlane::load(geometry, &memory);
    let half = ViScaleAxis::from_register(512);
    let unit = ViScaleAxis::from_register(u32::from(ViScaleAxis::ONE));
    let resampled = resample_bilinear(&plane, half, unit, 3, 1);
    assert_eq!(resampled[0], 0, "output 0 is source 0");
    assert_eq!(
        resampled[4], 128,
        "output 1 sits at fraction 512 between 0 and 255"
    );
    let replicated = replicate(&plane, half, unit, 3, 1);
    assert_eq!(
        replicated[4], 0,
        "replication holds the lower sample -- the two branches genuinely differ"
    );
}

// ---------------------------------------------------------------------
// The reference differential.
//
// `fn64-render-reference` implements both filters and is this project's
// established comparison oracle. These tests run BOTH backends over one
// RDRAM image and one register word and require the presented pixels to
// agree, which is far stronger evidence than either backend's own unit
// expectations.
//
// The oracle is driven through its public `RenderBackend` impl with a
// freshly `create`d instance, whose `rdram_hidden_bits` map `create` clears
// (`crates/fn64-render-reference/src/backend/render_backend.rs`). That cold
// map is exactly the state this backend is permanently in, so the two are
// being compared on equal footing rather than one being handed state the
// other cannot have. See `SourcePlane::coverage` for the deviation that
// remains when the reference's map is warm.
// ---------------------------------------------------------------------

/// Present one field through the reference backend over the same physical
/// RDRAM and the same register image.
fn reference_field(
    rdram: &[u8],
    vi: ViPresentation,
    width: u32,
    height: u32,
) -> fn64_render_reference::raster::Framebuffer {
    use fn64_render::RenderBackend;
    let mut backend = fn64_render_reference::ReferenceBackend::new();
    backend
        .create(&fn64_render::RenderConfig {
            width,
            height,
            tv_type: fn64_runtime::TvType::Ntsc,
        })
        .expect("reference backend create");
    backend
        .present(fn64_render::PresentRequest::live(
            vi,
            fn64_runtime::PhysicalRdramRead::from_storage(rdram),
        ))
        .expect("reference backend present");
    backend
        .presented_framebuffer()
        .expect("reference backend presented no field")
        .clone()
}

/// Assert the two backends produce the same RGB for every output pixel.
fn assert_agrees_with_reference(rdram: &[u8], registers: ViScanoutRegisters) {
    let vi = presentation(registers);
    let window = registers
        .active_window()
        .expect("fixture must program an active window");
    let (width, height) = (window.output_width(), window.output_height());
    let memory = fn64_runtime::PhysicalRdramRead::from_storage(rdram);
    let ours = scan_out_guest_rdram(vi, &memory).expect("wgpu scanout must not refuse");
    let theirs = reference_field(rdram, vi, width, height);
    assert_eq!(
        (ours.width, ours.height),
        (theirs.width, theirs.height),
        "the two backends disagree on the presented geometry"
    );
    let mut mismatches = Vec::new();
    for y in 0..height {
        for x in 0..width {
            let ours_px = ours.pixel(x, y).unwrap();
            let base = ((y * width + x) * 4) as usize;
            let theirs_px = &theirs.pixels[base..base + 3];
            if ours_px[..3] != *theirs_px {
                mismatches.push((
                    x,
                    y,
                    [ours_px[0], ours_px[1], ours_px[2]],
                    theirs_px.to_vec(),
                ));
            }
        }
    }
    assert!(
        mismatches.is_empty(),
        "wgpu and the reference oracle disagree on {} of {} pixels; first few: {:?}",
        mismatches.len(),
        width * height,
        &mismatches[..mismatches.len().min(6)]
    );
}

/// Build an RGBA16 source with a deterministic but non-uniform pattern, so a
/// filter that no-ops and a filter that runs cannot look alike.
fn patterned_rgba16(rdram: &mut [u8], origin: u32, stride: u32, rows: u32) {
    for row in 0..rows {
        for column in 0..stride {
            let key = row * 7 + column * 3;
            let r5 = (key % 32) as u8;
            let g5 = ((key * 5 + 11) % 32) as u8;
            let b5 = ((key * 11 + 3) % 32) as u8;
            // Alternate coverage so both the restored and the pass-through
            // path are exercised in one image.
            let a1 = u8::from((row + column) % 3 != 0);
            write_rgba16(
                rdram,
                origin + (row * stride + column) * 2,
                pack_rgba5551(r5, g5, b5, a1),
            );
        }
    }
}

/// **The headline differential.** WM2000's measured STATUS word, both of its
/// filters live at once, compared pixel-for-pixel against the reference
/// oracle over the same RDRAM.
#[test]
fn the_wm2000_filter_pair_agrees_with_the_reference_oracle() {
    let mut rdram = fresh_rdram();
    patterned_rgba16(&mut rdram, 0x1000, 12, 8);
    let registers = live_registers(
        wm2000_measured_status(),
        0x1000,
        12,
        0,
        12,
        0,
        16,
        u32::from(ViScaleAxis::ONE),
        u32::from(ViScaleAxis::ONE),
    );
    assert_agrees_with_reference(&rdram, registers);
}

/// The same pair under a fractional *horizontal* scale, so the resampler is
/// genuinely interpolating rather than landing on integer positions. The
/// vertical step stays integral here; a fractional vertical step composed
/// with restoration is the one measured disagreement, pinned separately in
/// `the_restoration_bottom_halo_disagreement_is_confined_to_the_last_output_row`.
#[test]
fn the_wm2000_filter_pair_agrees_with_the_reference_oracle_under_fractional_scale() {
    let mut rdram = fresh_rdram();
    patterned_rgba16(&mut rdram, 0x1000, 16, 10);
    for x_scale in [512u32, 683, 1536] {
        let registers = live_registers(
            wm2000_measured_status(),
            0x1000,
            16,
            0,
            10,
            0,
            12,
            x_scale,
            u32::from(ViScaleAxis::ONE),
        );
        assert_agrees_with_reference(&rdram, registers);
    }
}

/// **A measured, unresolved disagreement, pinned rather than tuned away.**
///
/// Under a fractional *vertical* scale with dither restoration enabled, the
/// two backends differ on the final output row only, by one or two units per
/// channel. Isolated below: restoration alone agrees everywhere, and
/// resampling alone agrees everywhere; only the two composed under a
/// fractional vertical step disagree, and only on the last row.
///
/// The cause is a genuine semantic fork this card is not authorised to
/// settle. Both backends load `last_center + 1` as the bottom halo row, but
/// that row is the *last* row of each plane, so its own 3x3 restoration
/// window is missing the row below it that an unbounded source would have
/// supplied. Each backend therefore restores its halo row against a
/// truncated neighbourhood, and the vertical interpolator then blends that
/// differently-truncated value into the final output row. Neither result is
/// derivable from public VI documentation, which does not specify the
/// filter's behaviour at the bottom of the scanned region.
///
/// This test asserts the disagreement *exists and is confined*, so that a
/// later change that silently widens it fails here. It deliberately does not
/// assert either backend is correct. Per this card's brief, the reference is
/// corroboration and not validation -- it shares fn64's lineage -- so a
/// disagreement is recorded, not resolved by tuning one side to the other.
#[test]
fn the_restoration_bottom_halo_disagreement_is_confined_to_the_last_output_row() {
    let mut rdram = fresh_rdram();
    patterned_rgba16(&mut rdram, 0x1000, 16, 10);
    let registers = live_registers(
        wm2000_measured_status(),
        0x1000,
        16,
        0,
        10,
        0,
        12,
        683,
        1365,
    );
    let vi = presentation(registers);
    let window = registers.active_window().unwrap();
    let (width, height) = (window.output_width(), window.output_height());
    let memory = fn64_runtime::PhysicalRdramRead::from_storage(&rdram);
    let ours = scan_out_guest_rdram(vi, &memory).unwrap();
    let theirs = reference_field(&rdram, vi, width, height);
    for y in 0..height {
        for x in 0..width {
            let ours_px = ours.pixel(x, y).unwrap();
            let base = ((y * width + x) * 4) as usize;
            let theirs_px = &theirs.pixels[base..base + 3];
            if y + 1 < height {
                assert_eq!(
                    ours_px[..3],
                    *theirs_px,
                    "rows above the last must agree exactly, at ({x}, {y})"
                );
            } else {
                for channel in 0..3 {
                    let delta = i16::from(ours_px[channel]) - i16::from(theirs_px[channel]);
                    assert!(
                        delta.abs() <= 2,
                        "the last-row disagreement must stay within two units, got {delta} at \
                         ({x}, {y}) channel {channel}"
                    );
                }
            }
        }
    }
}

/// Restoration on its own agrees with the oracle everywhere, including the
/// last row. This is what confines the disagreement above to the composition
/// rather than to the restoration filter itself.
#[test]
fn restoration_without_resampling_agrees_with_the_reference_oracle() {
    let mut rdram = fresh_rdram();
    patterned_rgba16(&mut rdram, 0x1000, 16, 10);
    // RGBA16 + AA mode 3 (replicate, no resampling) + dither restoration.
    let status_word = (2 << status::TYPE.offset)
        | (3 << status::AA_MODE.offset)
        | (1 << status::DITHER_FILTER.offset);
    let registers = live_registers(status_word, 0x1000, 16, 0, 10, 0, 12, 683, 1365);
    assert_agrees_with_reference(&rdram, registers);
}

/// Resampling alone (AA mode 2, bit 16 clear) also agrees, isolating the
/// resampler from the restoration filter.
#[test]
fn resampling_without_restoration_agrees_with_the_reference_oracle() {
    let mut rdram = fresh_rdram();
    patterned_rgba16(&mut rdram, 0x1000, 14, 9);
    let status_word = (2 << status::TYPE.offset) | (2 << status::AA_MODE.offset);
    let registers = live_registers(status_word, 0x1000, 14, 0, 11, 0, 14, 700, 900);
    assert_agrees_with_reference(&rdram, registers);
}

/// Replication (AA mode 3) still agrees, proving the refactor from the
/// original single-pass sampler onto `AxisSample` changed no pixel.
#[test]
fn replication_still_agrees_with_the_reference_oracle() {
    let mut rdram = fresh_rdram();
    patterned_rgba16(&mut rdram, 0x1000, 14, 9);
    let registers = live_registers(
        rgba16_replicate_status(),
        0x1000,
        14,
        0,
        11,
        0,
        14,
        700,
        900,
    );
    assert_agrees_with_reference(&rdram, registers);
}

/// The bottom halo, hand-derived per reader and asserted exactly.
///
/// Three cases, each a separate derivation rather than a table of captured
/// numbers. With `y_offset = 0` and `y_step = ONE`, output row `n` maps to
/// source centre `n`, so `last_center = output_height - 1`:
///
/// | filters | rows the readers touch | expected |
/// |---|---|---|
/// | neither (AA 3, bit 16 clear) | centres `0..=last_center` | `last_center + 1` |
/// | resampling only | plus `upper = last_center + 1` | `last_center + 2` |
/// | restoration only | plus the 3x3 row below | `last_center + 2` |
/// | both | the *same* row below, not two | `last_center + 2` |
///
/// The last row is the one that matters: summing the halos would load
/// `last_center + 3` and give restoration's bottom row a neighbour it should
/// not have. Both halos name the single row immediately below the last
/// centre, so they compose by `max`, never by `+`.
#[test]
fn the_bottom_halo_is_derived_per_reader_and_composes_by_max() {
    let output_height = 6u32;
    let last_center = u64::from(output_height - 1);
    let unit = u32::from(ViScaleAxis::ONE);
    let rgba16 = 2 << status::TYPE.offset;
    let replicate = 3 << status::AA_MODE.offset;
    let resample = 2 << status::AA_MODE.offset;
    let dither = 1 << status::DITHER_FILTER.offset;

    let rows_for = |status_word: u32| {
        let registers = live_registers(status_word, 0x1000, 8, 0, 8, 0, 12, unit, unit);
        assert_eq!(
            registers.active_window().unwrap().output_height(),
            output_height,
            "fixture must program the output height this derivation assumes"
        );
        source_rows(
            registers,
            output_height,
            fn64_render::ViFilterControl::from_status(status_word),
        )
    };

    assert_eq!(
        rows_for(rgba16 | replicate),
        last_center + 1,
        "replication reads only the centres"
    );
    assert_eq!(
        rows_for(rgba16 | resample),
        last_center + 2,
        "bilinear additionally reads `upper = last_center + 1`"
    );
    assert_eq!(
        rows_for(rgba16 | replicate | dither),
        last_center + 2,
        "restoration additionally reads the 3x3 row below the last centre"
    );
    assert_eq!(
        rows_for(rgba16 | resample | dither),
        last_center + 2,
        "both halos name the SAME row below the last centre, so they max, not sum"
    );
    // Stated as the inequality the `max` exists to enforce, so a `+` cannot
    // pass by coincidence on some other fixture.
    assert_ne!(
        rows_for(rgba16 | resample | dither),
        last_center + 3,
        "summing the halos would hand restoration's bottom row a neighbour it must not have"
    );
}

/// End-to-end hand-derived proof that restoration RUNS through
/// `scan_out_guest_rdram`, not merely through `SourcePlane` directly.
///
/// A 3x3 full-coverage RGBA16 image presented 1:1 with AA mode 3, so no
/// resampling can perturb the result. Red five-bit values:
///
/// ```text
///   0  0  0
///   0 16  0      centre = 16, all eight neighbours lesser
///   0  0  0
/// ```
///
/// `(16 << 3) - 8 = 120`. Second derivation: the unrestored expansion is
/// `expand_five_bit(16) = 132`, and restoration replaces that with
/// `128 - 8 = 120`, so the two differ by 12 -- a difference no pass-through
/// could produce. Positive control: `assert_ne!` against the unrestored
/// value proves the filter is latched rather than the test passing
/// vacuously.
#[test]
fn restoration_runs_end_to_end_through_the_public_scanout() {
    let mut rdram = fresh_rdram();
    for row in 0..3u32 {
        for column in 0..3u32 {
            let r5 = if row == 1 && column == 1 { 16 } else { 0 };
            write_rgba16(
                &mut rdram,
                0x1000 + (row * 3 + column) * 2,
                pack_rgba5551(r5, 0, 0, 1),
            );
        }
    }
    let memory = fn64_runtime::PhysicalRdramRead::from_storage(&rdram);
    let status_word = (2 << status::TYPE.offset)
        | (3 << status::AA_MODE.offset)
        | (1 << status::DITHER_FILTER.offset);
    let unit = u32::from(ViScaleAxis::ONE);
    let registers = live_registers(status_word, 0x1000, 3, 0, 3, 0, 4, unit, unit);
    let field = scan_out_guest_rdram(presentation(registers), &memory)
        .expect("dither restoration over RGBA16 must scan out, not refuse");
    assert!(
        fn64_render::ViFilterControl::from_status(status_word).dither_filter,
        "positive control: this fixture really does latch bit 16"
    );
    assert_eq!(
        field.pixel(1, 1).unwrap()[0],
        120,
        "(16 << 3) - 8 for eight lesser neighbours"
    );
    assert_ne!(
        field.pixel(1, 1).unwrap()[0],
        expand_five_bit(16),
        "a pass-through would emit the plain expansion; restoration must change it"
    );
}

/// End-to-end hand-derived proof that bilinear resampling RUNS through
/// `scan_out_guest_rdram`.
///
/// Two source columns, red 0 and 31, presented to three output columns with
/// a half-step horizontal scale and restoration off. Output column 1 sits at
/// fraction 512 between expanded 0 and 255, so it must be the hand-derived
/// 128. Positive control: `assert_ne!` against both endpoints proves an
/// interpolated value, which nearest-neighbour replication cannot produce.
#[test]
fn resampling_runs_end_to_end_through_the_public_scanout() {
    let mut rdram = fresh_rdram();
    write_rgba16(&mut rdram, 0x1000, pack_rgba5551(0, 0, 0, 0));
    write_rgba16(&mut rdram, 0x1002, pack_rgba5551(31, 0, 0, 0));
    let memory = fn64_runtime::PhysicalRdramRead::from_storage(&rdram);
    let status_word = (2 << status::TYPE.offset) | (2 << status::AA_MODE.offset);
    let registers = live_registers(
        status_word,
        0x1000,
        2,
        0,
        3,
        0,
        2,
        512,
        u32::from(ViScaleAxis::ONE),
    );
    let field = scan_out_guest_rdram(presentation(registers), &memory)
        .expect("AA mode 2 must scan out, not refuse");
    assert!(
        fn64_render::ViFilterControl::from_status(status_word)
            .antialias_mode
            .resampling_enabled(),
        "positive control: this fixture really is on the resampling branch"
    );
    assert_eq!(field.pixel(0, 0).unwrap()[0], 0, "output 0 is source 0");
    assert_eq!(
        field.pixel(1, 0).unwrap()[0],
        128,
        "output 1 is the fraction-512 midpoint of 0 and 255"
    );
    assert_ne!(
        field.pixel(1, 0).unwrap()[0],
        0,
        "replication would hold the lower sample; interpolation must not"
    );
    assert_ne!(field.pixel(1, 0).unwrap()[0], 255);
}

/// The coverage reconstruction's limitation, pinned so it cannot be quietly
/// forgotten or "improved" into a fabricated intermediate value.
///
/// Real hardware stores two hidden bits per RGBA16 halfword which, with the
/// visible low bit, give coverage `1..=8`. This backend cannot read them and
/// does not invent them: it reconstructs from the visible low bit alone,
/// which reaches only the two saturated ends. Every one of the 65,536
/// possible halfwords maps to 8 or 1 and never to anything between.
///
/// This is exactly why `SilhouetteAntialias` still refuses -- it weights a
/// blend by the coverage magnitude -- while dither restoration is admitted,
/// since restoration asks only the boolean `== 8`.
#[test]
fn reconstructed_coverage_reaches_only_the_saturated_ends_never_a_middle_value() {
    let mut rdram = fresh_rdram();
    let memory_len = rdram.len();
    let geometry = SourceGeometry {
        origin: 0,
        stride_pixels: 1,
        rows: 1,
        bytes_per_pixel: 2,
        pixel_type: ViPixelType::Rgba16,
    };
    let mut seen = std::collections::BTreeSet::new();
    for pixel in 0..=u16::MAX {
        write_rgba16(&mut rdram, 0, pixel);
        let memory = fn64_runtime::PhysicalRdramRead::from_storage(&rdram);
        let coverage = geometry.coverage(&memory, 0, 0);
        assert_eq!(
            coverage,
            if pixel & 1 == 0 { 1 } else { 8 },
            "coverage must come from the visible low bit of {pixel:#06x} and nothing else"
        );
        seen.insert(coverage);
    }
    assert_eq!(
        seen,
        std::collections::BTreeSet::from([1, 8]),
        "the reconstruction must NOT produce an intermediate coverage it cannot know"
    );
    for middle in 2u8..=7 {
        assert!(
            !seen.contains(&middle),
            "coverage {middle} would be fabricated: the hidden bits carrying it are unreadable"
        );
    }
    assert_eq!(memory_len, rdram.len());
}

/// Silhouette AA still refuses, and its reason still names the coverage
/// dependency the reconstruction above cannot satisfy. Guards against a
/// later change admitting it on the strength of the coverage this module
/// now computes -- which would be a blend weighted by a value that is only
/// ever 1 or 8.
#[test]
fn silhouette_aa_still_refuses_because_the_coverage_magnitude_is_unavailable() {
    let rdram = fresh_rdram();
    let memory = fn64_runtime::PhysicalRdramRead::from_storage(&rdram);
    for aa_mode in [0u32, 1] {
        let status_word = (2 << status::TYPE.offset) | (aa_mode << status::AA_MODE.offset);
        let registers = live_registers(
            status_word,
            0x400,
            4,
            0,
            4,
            0,
            4,
            u32::from(ViScaleAxis::ONE),
            u32::from(ViScaleAxis::ONE),
        );
        let error = scan_out_guest_rdram(presentation(registers), &memory)
            .expect_err("AA modes 0 and 1 must still refuse");
        let RenderError::Backend { reason, .. } = &error else {
            panic!("expected a named backend refusal, got {error:?}");
        };
        assert_eq!(reason, ViScanoutRefusal::SilhouetteAntialias.reason());
        assert!(
            reason.contains("coverage"),
            "the refusal must still name the coverage dependency"
        );
    }
}
