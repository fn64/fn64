use super::shading::{ADMITTED_ALPHA_INPUTS, ADMITTED_COLOR_INPUTS};
use super::*;

/// The two combiner programs `docs/rt64/RT64-WM2000-CYCLE-MODES.md` §2
/// measured across all 2,520 of WM2000's texrects, packed into their
/// `SetCombine` wire words.
///
/// The packing is hand-derived from `CombineParams`' own
/// `parse_color_*`/`parse_alpha_*` **second-cycle** bit positions
/// (`combiner.rs:189-250`), which is the slice one-cycle mode reads:
/// color A `low >> 5 & 0xF`, B `high >> 24 & 0xF`, C `low & 0x1F`,
/// D `high >> 6 & 0x7`; alpha A `high >> 21 & 0x7`, B `high >> 3 & 0x7`,
/// C `high >> 18 & 0x7`, D `high & 0x7`. Every field occupies disjoint
/// bits in its word, which `wire_programs_decode_to_the_measured_selectors`
/// below asserts by decoding rather than by inspection.
fn pack_second_cycle(color: [u32; 4], alpha: [u32; 4]) -> CombineParams {
    let [ca, cb, cc, cd] = color;
    let [aa, ab, ac, ad] = alpha;
    let low = (ca << 5) | cc;
    let high = (cb << 24) | (cd << 6) | (aa << 21) | (ab << 3) | (ac << 18) | ad;
    CombineParams::from_wire(low, high)
}

/// Program 1 (2,100 of 2,520 texrects): RGB
/// `(Environment - Texel0) * Primitive + Texel0`, Alpha
/// `(Texel0 - Zero) * Primitive + Zero`.
///
/// Indices: `Environment = 5` and `Texel0 = 1` from the shared
/// `colorInputCommon` table, `Primitive = 3` likewise; alpha `Zero` is
/// index 7 in both `alphaInputABD` and `alphaInputC`.
fn env_lerp_program() -> CombineParams {
    pack_second_cycle([5, 1, 3, 1], [1, 7, 3, 7])
}

/// [`pack_second_cycle`]'s first-cycle twin, hand-derived from the same
/// `parse_color_*`/`parse_alpha_*` functions at `second_cycle = false`
/// (`combiner.rs:189-250`): color A `low >> 20 & 0xF`,
/// B `high >> 28 & 0xF`, C `low >> 15 & 0x1F`, D `high >> 15 & 0x7`;
/// alpha A `low >> 12 & 0x7`, B `high >> 12 & 0x7`, C `low >> 9 & 0x7`,
/// D `high >> 9 & 0x7`.
///
/// Every one of those fields is disjoint from every field
/// [`pack_second_cycle`] writes, which is what lets the two be OR'd into
/// one `CombineParams` to build a genuine two-cycle program.
/// `two_cycle_wire_program_decodes_to_both_slices` asserts that by
/// decoding rather than by inspection.
fn pack_first_cycle(color: [u32; 4], alpha: [u32; 4]) -> CombineParams {
    let [ca, cb, cc, cd] = color;
    let [aa, ab, ac, ad] = alpha;
    let low = (ca << 20) | (cc << 15) | (aa << 12) | (ac << 9);
    let high = (cb << 28) | (cd << 15) | (ab << 12) | (ad << 9);
    CombineParams::from_wire(low, high)
}

/// Merges a first-cycle and a second-cycle packing into one program.
fn merge_cycles(first: CombineParams, second: CombineParams) -> CombineParams {
    CombineParams::from_wire(first.low() | second.low(), first.high() | second.high())
}

/// A two-cycle program whose two cycles cannot be collapsed into one.
///
/// **Cycle 0** (RGB and alpha alike): `(Zero - Zero) * Zero + Primitive`
/// -- the accumulator becomes the primitive colour. Slot indices are
/// each slot's own out-of-table `Zero` (`A = 8`, `B = 8`, `C = 16`,
/// alpha `= 7`) with `D = 3` (`Primitive` in `colorInputD` and
/// `alphaInputABD` alike), exactly as [`flat_primitive_program`] does
/// for the second slice.
///
/// **Cycle 1** (RGB and alpha alike): `(Zero - Zero) * Zero + Combined`
/// -- `D = 0`, which is `Combined` in `colorInputD` and
/// `alphaInputABD`. So cycle 1 emits, verbatim, whatever cycle 0 put in
/// the accumulator.
///
/// Under two-cycle evaluation the result is the primitive colour.
/// Under one-cycle evaluation **only the second slice runs**, against
/// the zero-initialized accumulator, so `D = Combined` resolves to zero
/// and the result is transparent black. The two answers differ in all
/// four channels, which is the point: no reading of the second slice
/// alone can produce the two-cycle answer.
///
/// Deliberately `Texel0`-free in both slices. This executor binds one
/// tile, so `Texel1` is refused (the reference lane refuses it for the
/// same reason), and a program needing a second texel would prove
/// nothing about the carry.
fn carry_program() -> CombineParams {
    merge_cycles(
        pack_first_cycle([8, 8, 16, 3], [7, 7, 7, 3]),
        pack_second_cycle([8, 8, 16, 0], [7, 7, 7, 0]),
    )
}

/// Program 2 (420 of 2,520): RGB and Alpha both
/// `(Zero - Zero) * Zero + Primitive`.
///
/// Each slot's `Zero` index is that slot's OWN out-of-table value, not
/// a shared constant -- `IDX_COLOR_ZERO_A = 8`, `_B = 8`, `_C = 16`
/// (its field is 5 bits wide), alpha `Zero = 7`. Using one index for
/// all four would decode to `NOISE`/`K4`/`K5` in the slots whose
/// tables define index 7.
fn flat_primitive_program() -> CombineParams {
    pack_second_cycle([8, 8, 16, 3], [7, 7, 7, 3])
}

const ENV_WIRE: u32 = 0xFF00_80FF;
const PRIM_WIRE: u32 = 0x80FF_4080;
/// `SetPrimColor`'s `w0`: `lod_frac` in bits 0:7, `lod_min` in 8:12.
/// Neither program reads either, so the value is deliberately non-zero
/// -- if `prim_lod_frac` ever leaked into a channel, this catches it.
const PRIM_LOD_W0: u32 = 0x0540;

fn measured_shading(combine: CombineParams) -> TexrectShading {
    TexrectShading::new(
        combine,
        Color4::from_wire(ENV_WIRE),
        PrimColor::from_wire(PRIM_LOD_W0, PRIM_WIRE),
    )
    .validate_one_cycle()
    .expect("both measured programs read only admitted selectors")
}

/// Calls [`combine_one_texel`] -- **the executor's own** per-texel
/// function, not a copy of it.
///
/// This was a duplicate in the first draft, on the reasoning that a
/// shared helper makes agreement structural rather than tested. That
/// reasoning was wrong in the direction that matters: the duplicate put
/// the executor's real arithmetic out of every unit test's reach, and a
/// truncation mutant inside it survived the whole suite. Sharing the
/// function is what makes these tests able to kill it. What proves the
/// executor actually *calls* it is the composed and end-to-end tests,
/// which is the right place for that claim.
fn combine_texel(shading: TexrectShading, texel: [u8; 4]) -> [u8; 4] {
    combine_one_texel(
        shading.combine(),
        shading.base_inputs(),
        texel,
        TexrectCombinerEvaluation::OneCycle,
    )
}

/// **Positive control for every assertion below**: the two wire words
/// really do decode to the measured programs.
///
/// Without this, a packing slip would silently substitute a different
/// program and every hand-derived expectation below would be checking
/// arithmetic nobody measured. Asserted through
/// `CombineParams::decode_color`/`decode_alpha` at `second_cycle =
/// true`, the exact call `TexrectShading::try_new` and `run_one_cycle`
/// both make.
#[test]
fn wire_programs_decode_to_the_measured_selectors() {
    let lerp = env_lerp_program();
    assert_eq!(
        [
            lerp.decode_color(ColorInputSlot::A, true),
            lerp.decode_color(ColorInputSlot::B, true),
            lerp.decode_color(ColorInputSlot::C, true),
            lerp.decode_color(ColorInputSlot::D, true),
        ],
        [
            ColorInput::Environment,
            ColorInput::Texel0,
            ColorInput::Primitive,
            ColorInput::Texel0,
        ],
        "program 1's RGB must be (Environment - Texel0) * Primitive + Texel0"
    );
    assert_eq!(
        [
            lerp.decode_alpha(AlphaInputSlot::A, true),
            lerp.decode_alpha(AlphaInputSlot::B, true),
            lerp.decode_alpha(AlphaInputSlot::C, true),
            lerp.decode_alpha(AlphaInputSlot::D, true),
        ],
        [
            AlphaInput::Texel0,
            AlphaInput::Zero,
            AlphaInput::Primitive,
            AlphaInput::Zero,
        ],
        "program 1's alpha must be (Texel0 - Zero) * Primitive + Zero"
    );

    let flat = flat_primitive_program();
    assert_eq!(
        [
            flat.decode_color(ColorInputSlot::A, true),
            flat.decode_color(ColorInputSlot::B, true),
            flat.decode_color(ColorInputSlot::C, true),
            flat.decode_color(ColorInputSlot::D, true),
        ],
        [
            ColorInput::Zero,
            ColorInput::Zero,
            ColorInput::Zero,
            ColorInput::Primitive,
        ],
        "program 2's RGB must be (Zero - Zero) * Zero + Primitive"
    );
    assert_eq!(
        [
            flat.decode_alpha(AlphaInputSlot::A, true),
            flat.decode_alpha(AlphaInputSlot::B, true),
            flat.decode_alpha(AlphaInputSlot::C, true),
            flat.decode_alpha(AlphaInputSlot::D, true),
        ],
        [
            AlphaInput::Zero,
            AlphaInput::Zero,
            AlphaInput::Zero,
            AlphaInput::Primitive,
        ],
        "program 2's alpha must be (Zero - Zero) * Zero + Primitive"
    );
    // The two programs must not be the same wire value, or every
    // "different program, different pixel" assertion is vacuous.
    assert_ne!(
        (lerp.low(), lerp.high()),
        (flat.low(), flat.high()),
        "the two measured programs must be distinct wire words"
    );
}

/// **Program 1's arithmetic, hand-derived per channel and reconciled
/// against a second derivation of the same value.**
///
/// Inputs: texel `(0x18, 0x40, 0xC8, 0xFF)`, env `0xFF0080FF` ->
/// `(255, 0, 128, 255)`, prim `0x80FF4080` -> `(128, 255, 64, 128)`.
///
/// Derivation 1, per channel, in the `(A - B) * C + D` order RT64
/// evaluates (`run_one_cycle`'s own expression, not an algebraically
/// rearranged one -- `A*C - B*C + D` is equal in exact arithmetic and
/// NOT bit-identical in f32):
///
/// ```text
/// R: (255/255 - 24/255) * (128/255) + 24/255
/// G: (  0/255 - 64/255) * (255/255) + 64/255
/// B: (128/255 - 200/255) * ( 64/255) + 200/255
/// A: (255/255 -       0) * (128/255) +       0
/// ```
///
/// Derivation 2, independent of the first: G's `C` is exactly `1.0`, so
/// G reduces algebraically to `A - B + B = A = 0`. B's operand
/// `(128 - 200)/255` is negative, so B must fall BELOW its `D` addend
/// of `200/255 ~ 0.784` -- `0.713` does. A's `B` and `D` are both
/// `Zero`, so alpha reduces to `texel.a * prim.a = 1.0 * 128/255 =
/// 0.502`, and `0.502 * 255` rounds to exactly `128` -- the primitive
/// alpha byte returned unchanged, which is the sharpest possible check
/// that the `* 255.0` quantization is not off by one.
///
/// The green channel is the load-bearing one: it is `0` only because
/// the `+ Texel0` addend cancels the `- Texel0` subtrahend at `C = 1`.
/// Dropping the `D` addend gives `-64/255`, which `wrap_clamp` pins to
/// `0` -- so green ALONE cannot catch a dropped addend, and red and
/// blue are what do.
#[test]
fn program_one_env_lerp_produces_hand_derived_bytes() {
    let shading = measured_shading(env_lerp_program());
    let observed = combine_texel(shading, [0x18, 0x40, 0xC8, 0xFF]);
    assert_eq!(
        observed,
        [140, 0, 182, 128],
        "program 1 must produce the hand-derived RGBA8888"
    );

    // Derivation 2, recomputed here in the target precision (f32, not
    // f64) so a Python-style f64 model cannot hide a rounding lane.
    let n = |byte: u8| f32::from(byte) / 255.0;
    let red = (n(0xFF) - n(0x18)) * n(0x80) + n(0x18);
    let green = (n(0x00) - n(0x40)) * n(0xFF) + n(0x40);
    let blue = (n(0x80) - n(0xC8)) * n(0x40) + n(0xC8);
    let alpha = (n(0xFF) - 0.0) * n(0x80) + 0.0;
    assert_eq!(
        [red, green, blue, alpha].map(|c| (c.clamp(0.0, 1.0) * 255.0).round() as u8),
        observed,
        "the second, independently written derivation must reconcile with the first"
    );
    // Green really is exactly zero, not merely small -- the `C = 1`
    // cancellation, pinned.
    assert_eq!(
        green, 0.0,
        "green's C is exactly ONE, so A - B + B cancels to A = 0"
    );
    // And alpha really is the primitive alpha byte round-tripped.
    assert_eq!(
        observed[3], 0x80,
        "alpha is texel.a * prim.a with texel.a = 1.0"
    );
}

/// **Program 2's arithmetic: `(Zero - Zero) * Zero + Primitive` is
/// exactly the primitive color, every channel, texel-independent.**
///
/// Hand-derived twice. Derivation 1: `(0 - 0) * 0 = 0`, so the result
/// is the `D` addend, which is `Primitive` in all four channels ->
/// `0x80FF4080` -> `(128, 255, 64, 128)`. Derivation 2, independent:
/// the byte values must be `Color4::normalized`'s `/ 255.0` followed by
/// `* 255.0` and `round`, which is the identity on every byte because
/// `f32` represents `b / 255.0 * 255.0` exactly enough that no byte
/// moves -- asserted for the full `0..=255` sweep below rather than for
/// these four values alone.
///
/// The texel-independence is asserted, not assumed: the same program
/// against three unrelated texels must give one answer. That is what
/// distinguishes "the combiner ran program 2" from "the combiner was
/// bypassed and wrote the texel", which is mutant (a).
#[test]
fn program_two_flat_primitive_ignores_the_texel_entirely() {
    let shading = measured_shading(flat_primitive_program());
    let expected = [0x80, 0xFF, 0x40, 0x80];
    for texel in [
        [0x00, 0x00, 0x00, 0x00],
        [0x18, 0x40, 0xC8, 0xFF],
        [0xFF, 0xFF, 0xFF, 0xFF],
    ] {
        assert_eq!(
            combine_texel(shading, texel),
            expected,
            "program 2 must be the primitive color regardless of texel {texel:?}"
        );
    }
    // Derivation 2's round-trip claim, swept exhaustively rather than
    // spot-checked: `b / 255.0 * 255.0` rounds back to `b` for every
    // byte, so "the primitive color unchanged" is a real claim about
    // the quantization and not an accident of these four values.
    for byte in 0u8..=255 {
        let round_tripped = ((f32::from(byte) / 255.0) * 255.0).round() as u8;
        assert_eq!(
            round_tripped, byte,
            "byte {byte} must survive the normalize/quantize pair"
        );
    }
}

/// **The two programs disagree on the same texel** -- so a test that
/// applied the wrong program to the wrong entries (mutant (e)) cannot
/// pass, and neither can one that ignores the program entirely.
#[test]
fn the_two_measured_programs_disagree_on_one_texel() {
    let texel = [0x18, 0x40, 0xC8, 0xFF];
    assert_ne!(
        combine_texel(measured_shading(env_lerp_program()), texel),
        combine_texel(measured_shading(flat_primitive_program()), texel),
        "the env-lerp and flat-primitive programs must produce different pixels for the \
         same texel, or applying one where the other belongs is undetectable"
    );
    // And neither equals the raw texel, so bypassing the combiner
    // (mutant (a)) is detectable by either program.
    for (label, shading) in [
        ("env-lerp", measured_shading(env_lerp_program())),
        ("flat-primitive", measured_shading(flat_primitive_program())),
    ] {
        assert_ne!(
            combine_texel(shading, texel),
            texel,
            "{label} must not reproduce the raw texel, or bypassing the combiner is \
             indistinguishable from running it"
        );
    }
}

/// **Primitive and Environment are not interchangeable** -- mutant (b).
///
/// Swapping the two registers must change program 1's output. Asserted
/// by evaluating the same program with the two wire words exchanged,
/// which is exactly what a swapped plumbing would do at the call site.
#[test]
fn swapping_primitive_and_environment_changes_the_pixel() {
    let texel = [0x18, 0x40, 0xC8, 0xFF];
    let straight = combine_texel(measured_shading(env_lerp_program()), texel);
    let swapped = TexrectShading::new(
        env_lerp_program(),
        Color4::from_wire(PRIM_WIRE),
        PrimColor::from_wire(PRIM_LOD_W0, ENV_WIRE),
    )
    .validate_one_cycle()
    .expect("the swapped registers are still admitted selectors");
    assert_ne!(
        combine_texel(swapped, texel),
        straight,
        "exchanging the Primitive and Environment wire words must change program 1's \
         output, or the two are plumbed interchangeably"
    );
}

/// **Dropping the `+ Texel0` addend changes the pixel** -- mutant (c),
/// expressed as the program that differs by exactly that term.
///
/// `(Environment - Texel0) * Primitive + Zero` is program 1 with `D`
/// changed from `Texel0` to `Zero`; its output must differ, and on the
/// red and blue channels specifically (green's `C = 1` makes the
/// clamped result `0` either way -- documented in
/// `program_one_env_lerp_produces_hand_derived_bytes`).
#[test]
fn dropping_the_texel_addend_changes_the_pixel() {
    let texel = [0x18, 0x40, 0xC8, 0xFF];
    let with_addend = combine_texel(measured_shading(env_lerp_program()), texel);
    // `colorInputD`'s ZERO index is 7 -- its 3-bit table's only
    // out-of-range value.
    let without = pack_second_cycle([5, 1, 3, 7], [1, 7, 3, 7]);
    let observed = combine_texel(measured_shading(without), texel);
    assert_ne!(
        observed, with_addend,
        "the `+ Texel0` addend must be load bearing"
    );
    assert_ne!(
        observed[0], with_addend[0],
        "red must differ without the addend"
    );
    assert_ne!(
        observed[2], with_addend[2],
        "blue must differ without the addend"
    );
}

/// **Clamping happens in float, before quantization, and the wrap step
/// runs before the clamp** -- mutant (d).
///
/// Color slot C's table has no `ONE` entry at all (`colorInputC` maps
/// index 6 to `KEY_SCALE`), so an over-range color result is reached
/// through a `PRIMITIVE` register set to `0xFFFFFFFF` instead -- a real
/// register at exactly `1.0`, not a synthetic constant.
///
/// **The over-range case.** `(One - Zero) * Primitive(1.0) + One`
/// evaluates to `2.0`. `wrap_clamp` sees `2.0 >= 1.5 + 1/255`, subtracts
/// the `2.0 + 2/255` range to get `~-0.008`, and the final
/// `clamp(0, 1)` pins that to **`0.0` -> byte 0**. A naive
/// clamp-without-wrap would give `1.0` -> byte 255, and a
/// quantize-then-clamp order would compute `2.0 * 255 = 510` and
/// saturate to 255 as well. So byte `0` separates RT64's actual
/// wrap-then-clamp-then-quantize order from BOTH of the plausible
/// wrong orders, by the full channel range.
///
/// Hand-derived twice: (1) `2.0 - (1.5 + 1/255 - (-0.5 - 1/255)) =
/// 2.0 - 2.00784 = -0.00784`, clamped to `0.0`; (2) the wrap range is
/// exactly `2 + 2/255`, and `2.0` is `2/255` below it, so the wrapped
/// value is `-2/255 ~ -0.00784`. Same. Both are computed in `f32`
/// below, not in `f64`.
///
/// **The negative case.** `(Zero - One) * Primitive(1.0) + Zero` is
/// `-1.0`; the wrap step fires (`-1.0 <= -0.5 - 1/255`), adding the
/// range to give `~1.008`, clamped to `1.0` -> byte **255**. A
/// quantize-first order would saturate `-255.0` to byte `0`. Again the
/// two orders disagree by the full range.
#[test]
fn wrap_clamp_runs_before_quantization() {
    let one_register = PrimColor::from_wire(0, 0xFFFF_FFFF);
    // Color: A = ONE (index 6 in `colorInputA`), B = ZERO (8),
    // C = PRIMITIVE (3), D = ONE (6 in `colorInputD`).
    // Alpha: A = ONE (6), B = ZERO (7), C = PRIMITIVE (3), D = ONE (6).
    let over = pack_second_cycle([6, 8, 3, 6], [6, 7, 3, 6]);
    let shading = TexrectShading::new(over, Color4::from_wire(0), one_register)
        .validate_one_cycle()
        .expect("ONE/ZERO/PRIMITIVE are all admitted selectors");
    assert_eq!(
        combine_texel(shading, [0x18, 0x40, 0xC8, 0xFF]),
        [0, 0, 0, 0],
        "(One - Zero) * Primitive(1.0) + One is 2.0; wrap_clamp wraps it to ~-0.008 and              clamps to 0.0 -> byte 0. A clamp-only or a quantize-first order gives 255."
    );

    // Derivation 2, in f32: the wrap range and the wrapped value.
    let rounding = 1.0f32 / 255.0;
    let low = -0.5f32 - rounding;
    let high = 1.5f32 + rounding;
    let wrapped = 2.0f32 - (high - low);
    assert!(
        wrapped < 0.0,
        "2.0 must wrap BELOW zero, which is what makes the clamped answer 0 and not 1:              got {wrapped}"
    );
    assert_eq!(
        (wrapped.clamp(0.0, 1.0) * 255.0).round() as u8,
        0,
        "the independently computed wrap must reconcile with the observed byte"
    );

    // The negative case, the other direction.
    // Color: A = ZERO (8), B = ONE (6 in `colorInputB`? no -- B's 6 is
    // KEY_CENTER), so the subtrahend ONE comes from B index... none.
    // Reached instead through B = PRIMITIVE (3) at 1.0.
    let negative = pack_second_cycle([8, 3, 3, 7], [7, 3, 3, 7]);
    let shading = TexrectShading::new(negative, Color4::from_wire(0), one_register)
        .validate_one_cycle()
        .expect("ZERO/PRIMITIVE are admitted selectors");
    assert_eq!(
        combine_texel(shading, [0x18, 0x40, 0xC8, 0xFF]),
        [255, 255, 255, 255],
        "(Zero - Primitive(1.0)) * Primitive(1.0) + Zero is -1.0; wrap_clamp wraps it to              ~1.008 and clamps to 1.0 -> byte 255. A quantize-first order saturates to 0."
    );
    let wrapped_negative = -1.0f32 + (high - low);
    assert!(
        wrapped_negative > 1.0,
        "-1.0 must wrap ABOVE one, which is what makes the clamped answer 255 and not 0:              got {wrapped_negative}"
    );
}

/// **The executor evaluates the LATCHED program, not a fixed one** --
/// mutant (e), and this test exists because that mutant SURVIVED its
/// first draft.
///
/// Replacing `shading.combine()` inside the pixel loop with a hardcoded
/// flat-primitive program left the whole suite green. The reason is a
/// reach gap, not an equivalence: the only *executed* one-cycle fixture
/// runs the flat-primitive program itself (the env-lerp one is blocked
/// by the GPU-path defect
/// `a_texel_referencing_combine_is_blocked_by_the_gpu_paths_committed_tmem_projection`
/// pins), so substituting that exact program for the latched one is a
/// no-op there, and every other assertion reached the arithmetic
/// through the test helper.
///
/// This test closes it at the executor's own function: the same texel
/// and the same registers, through [`combine_one_texel`], must give
/// different bytes for the two measured programs. A hardcoded program
/// makes them equal.
#[test]
fn combine_one_texel_consults_the_program_it_is_given() {
    let texel = [0x18, 0x40, 0xC8, 0xFF];
    let base = measured_shading(env_lerp_program()).base_inputs();
    let lerp = combine_one_texel(
        env_lerp_program(),
        base,
        texel,
        TexrectCombinerEvaluation::OneCycle,
    );
    let flat = combine_one_texel(
        flat_primitive_program(),
        base,
        texel,
        TexrectCombinerEvaluation::OneCycle,
    );
    assert_ne!(
        lerp, flat,
        "the same texel and the same registers must combine differently under the two \
         measured programs, or the executor is not consulting the program it is handed"
    );
    // And each is the value its own program's hand derivation gives, so
    // "they differ" is not satisfied by two equally wrong answers.
    assert_eq!(
        lerp,
        [140, 0, 182, 128],
        "the env-lerp program's hand-derived bytes"
    );
    assert_eq!(
        flat,
        [0x80, 0xFF, 0x40, 0x80],
        "the flat program's primitive colour"
    );
}

/// **The quantization is round-half-away-from-zero, not truncation** --
/// mutant (d), and this test exists because the first draft's mutant
/// SURVIVED.
///
/// # Why it survived, and what that revealed
///
/// Replacing `(channel * 255.0).round() as u8` with a truncating
/// `(channel * 255.0) as u8` left the whole suite green. Two reasons,
/// both real:
///
/// 1. Every other assertion in this module reached the arithmetic
///    through this module's own `combine_texel` helper, which duplicates
///    the quantization rather than calling the executor -- so a mutation
///    inside the executor's pixel loop was out of the helper's reach.
/// 2. The executed fixtures write an **RGBA16** target, whose
///    `write_pixel` truncates each colour channel by `>> 3`. That
///    absorbs a one-count difference in the 8-bit intermediate unless
///    the two values straddle a multiple of 8. For the env-lerp
///    program's own bytes they do not: `139.95` truncates to `139` and
///    rounds to `140`, and `139 >> 3 == 140 >> 3 == 17`.
///
/// # The witness, found by search rather than guessed
///
/// `(Environment(0) - Texel0(16)) * Primitive(128) + Texel0(16)`
/// evaluates to `7.96862745...` in f32 after `* 255.0`. Truncation
/// gives `7`; round-half-away-from-zero gives `8`. `7 >> 3 == 0` and
/// `8 >> 3 == 1`, so the two **do** straddle a multiple of eight and
/// the difference survives the RGBA16 pack.
///
/// A spot-check on the env-lerp program's own bytes would have
/// supported the truncating form. This is the same lesson
/// `RT64-PORT-CARD-BRIEF.md` §3.4 records: the witness had to be
/// searched for, not assumed.
///
/// Hand-derived twice: (1) `(0 - 16/255) * (128/255) + 16/255 =
/// (16/255)(1 - 128/255) = (16 * 127) / 255^2 = 2032/65025 =
/// 0.031249...`, times 255 is `7.9686...`; (2) computed in `f32` below
/// and asserted to land strictly between 7 and 8, which is what makes
/// the two roundings differ at all.
#[test]
fn the_quantization_rounds_rather_than_truncating() {
    let program = pack_second_cycle([5, 1, 3, 1], [1, 7, 3, 7]);
    let shading = TexrectShading::new(
        program,
        Color4::from_wire(0x0000_0000),
        PrimColor::from_wire(0, 0x8080_8080),
    )
    .validate_one_cycle()
    .expect("the env-lerp program reads only admitted selectors");
    let combined = combine_texel(shading, [0x10, 0x10, 0x10, 0x10]);
    assert_eq!(
        combined[0], 8,
        "7.9686 must round to 8, not truncate to 7 -- RT64 clamps in float and the byte is \
         the rounded value"
    );

    // Derivation 2, in the target precision: the pre-quantization value
    // really does lie strictly between 7 and 8, which is the only
    // condition under which the two roundings can disagree.
    let n = |byte: u8| f32::from(byte) / 255.0;
    let raw = (n(0x00) - n(0x10)) * n(0x80) + n(0x10);
    let scaled = raw * 255.0;
    assert!(
        scaled > 7.0 && scaled < 8.0,
        "the witness must straddle the two roundings: got {scaled}"
    );
    assert_eq!(scaled.round() as u8, 8);
    assert_eq!(
        scaled as u8, 7,
        "truncation gives 7, which is the mutant's answer"
    );

    // **And the difference survives the RGBA16 pack**, which is what
    // makes it observable in a composed image rather than only in the
    // 8-bit intermediate. This is the half the first draft missed.
    assert_ne!(
        8u16 >> 3,
        7u16 >> 3,
        "the two roundings must straddle a multiple of eight, or the RGBA16 target's `>> 3` \
         absorbs the difference and no composed test can ever see it"
    );
}

/// **A texrect's `Shade`/`ShadeAlpha` is admitted and evaluates to
/// fn64's zero**, rather than being refused by name.
///
/// A `G_TEXRECT` command carries no shade coefficient words; fn64 reads
/// that wire layout as making `shade_color` `[0, 0, 0, 0]` for every
/// pixel of every rectangle. This rule is not independently confirmed
/// against an allowed hardware reference. See
/// [`TexrectShading::base_inputs`].
#[test]
fn a_texrect_shade_reading_program_is_admitted_and_reads_zero() {
    // Color A index 4 is SHADE in the shared common table.
    let shade_in_color = pack_second_cycle([4, 8, 16, 7], [7, 7, 7, 7]);
    assert!(
        TexrectShading::new(
            shade_in_color,
            Color4::from_wire(0),
            PrimColor::from_wire(0, 0)
        )
        .validate_one_cycle()
        .is_ok(),
        "a texrect program reading SHADE in a color slot must be admitted: its shade is the \
         hardware's zero, not an absent value"
    );
    // Color C index 11 is SHADE_ALPHA -- the exact selector WM2000
    // stages, and the one this executor used to abort on.
    let shade_alpha_in_c = pack_second_cycle([1, 8, 11, 7], [7, 7, 7, 7]);
    assert!(
        TexrectShading::new(
            shade_alpha_in_c,
            Color4::from_wire(0),
            PrimColor::from_wire(0, 0)
        )
        .validate_one_cycle()
        .is_ok(),
        "slot C selecting SHADE_ALPHA must be admitted"
    );
    // And the alpha side, which has its own table.
    let shade_in_alpha = pack_second_cycle([8, 8, 16, 7], [4, 7, 7, 7]);
    assert!(
        TexrectShading::new(
            shade_in_alpha,
            Color4::from_wire(0),
            PrimColor::from_wire(0, 0)
        )
        .validate_one_cycle()
        .is_ok(),
        "a program reading SHADE in an alpha slot must be admitted too"
    );
}

/// **The value a texrect's admitted `Shade` reads is zero, and the test
/// can tell zero from every other candidate.**
///
/// This is the mutation-resistant half. The trap this guards is a
/// fixture where the correct answer and a wrong one coincide: if the
/// executor were changed to feed `Shade` the primitive colour, the
/// environment colour, or one, a program that merely *succeeds* would
/// not notice. So both constant registers are staged to distinctive
/// NON-ZERO values and the program is `(SHADE - ZERO) * ONE + ZERO`,
/// whose output is the shade itself. Expected `[0, 0, 0, 0]` follows
/// fn64's reading of the `G_TEXRECT` wire layout, which has no shade
/// coefficient words. The zero-shade rule is not independently confirmed
/// against an allowed hardware reference.
#[test]
fn a_texrects_shade_evaluates_to_the_hardwares_zero_not_a_neighbouring_register() {
    // Distinctive non-zero registers: any executor that substituted one
    // of these for the shade fails this test rather than passing it.
    let env = Color4::from_wire(0x1122_3344);
    let prim = PrimColor::from_wire(0, 0x5566_7788);
    // Derived by hand from the four per-slot decode tables, which are
    // NOT one shared table: slot A's index 6 is ONE, but slot C's is
    // KEY_SCALE and slot C has no ONE entry at all. So the passthrough
    // is built on the multiply-by-zero form instead of multiply-by-one.
    //
    // Color slots: A = SHADE(4), B = ZERO(8), C = ZERO(16), D = SHADE(4)
    // => (shade - 0) * 0 + shade = shade.
    // Alpha slots: A = SHADE(4), B = ZERO(7), C = ZERO(7), D = SHADE(4)
    // (alpha A/B/D share `alpha_input_abd`, where 4 is SHADE).
    let shade_passthrough = pack_second_cycle([4, 8, 16, 4], [4, 7, 7, 4]);
    let shading = TexrectShading::new(shade_passthrough, env, prim)
        .validate_one_cycle()
        .expect("a texrect reading SHADE is admitted");
    let inputs = shading.base_inputs();
    assert_eq!(
        inputs.shade_color, [0.0; 4],
        "a texture rectangle's shade is zero on hardware: the command carries no shade \
         coefficient words and the rasterizer zeroes the block"
    );
    // The registers really are distinct from the shade, so the equality
    // above is a measurement rather than a coincidence.
    assert_ne!(
        inputs.env_color, inputs.shade_color,
        "the fixture must stage an environment colour that differs from the shade, or a \
         mutant substituting ENV for SHADE survives"
    );
    assert_ne!(
        inputs.prim_color, inputs.shade_color,
        "the fixture must stage a primitive colour that differs from the shade, or a mutant \
         substituting PRIM for SHADE survives"
    );
    // And zero is distinguishable from the other obvious wrong answer.
    assert_ne!(
        inputs.shade_color, [1.0; 4],
        "zero must be distinguishable from ONE, or a mutant feeding a full-scale shade \
         survives"
    );
}

/// **An UNSHADED raw triangle keeps its `Shade` refusal.** The texrect
/// admission above must not leak into the triangle path, where the
/// hardware interpolates a real non-zero shade this executor cannot
/// reconstruct. Kills the mutant that widens `shade_available` to `true`
/// everywhere instead of only for rectangles.
#[test]
fn an_unshaded_raw_triangle_still_refuses_shade() {
    let shade_in_color = pack_second_cycle([4, 8, 16, 7], [7, 7, 7, 7]);
    assert_eq!(
        TexrectShading::new(
            shade_in_color,
            Color4::from_wire(0),
            PrimColor::from_wire(0, 0)
        )
        .validate_combiner_program_with_shade(CombinerProgramCycles::OnlySecondSlice, false,),
        Err(TexrectExecutionError::UnsupportedColorInput {
            slot: ColorInputSlot::A,
            input: ColorInput::Shade,
        }),
        "an unshaded triangle has a real interpolated shade this executor cannot supply, so \
         it must still refuse"
    );
    // The message still names the selector, so a future title's log says
    // what is missing rather than only that something is.
    let message = TexrectShading::new(
        shade_in_color,
        Color4::from_wire(0),
        PrimColor::from_wire(0, 0),
    )
    .validate_combiner_program_with_shade(CombinerProgramCycles::OnlySecondSlice, false)
    .unwrap_err()
    .to_string();
    assert!(
        message.contains("Shade"),
        "the refusal must name the selector: {message}"
    );
}

/// **The admitted-selector tables are pinned by value, not merely
/// consulted.**
///
/// [`every_unmeasured_selector_is_refused`] derives its expectation
/// FROM [`ADMITTED_COLOR_INPUTS`]/[`ADMITTED_ALPHA_INPUTS`], so it
/// cannot notice a selector being added to those tables -- the
/// expectation moves with the mutation. Measured: a mutant inserting
/// `ColorInput::Shade` into `ADMITTED_COLOR_INPUTS` SURVIVED that sweep
/// once the sweep was taught the second (slice-scoped) admission rule.
///
/// This test is the fixed point. The contents are the measured WM2000
/// window's selector set plus the register-backed widening, transcribed
/// here by hand so that widening the executor's admitted set requires
/// editing an explicit list in a test that says why.
#[test]
fn the_admitted_selector_tables_are_exactly_these() {
    assert_eq!(
        ADMITTED_COLOR_INPUTS,
        [
            ColorInput::Texel0,
            ColorInput::Primitive,
            ColorInput::Environment,
            ColorInput::One,
            ColorInput::Zero,
            ColorInput::Texel0Alpha,
            ColorInput::PrimitiveAlpha,
            ColorInput::EnvAlpha,
            ColorInput::PrimLodFrac,
        ],
        "ADMITTED_COLOR_INPUTS changed; a selector added here is a claim this executor \
         evaluates it, which needs its own measurement and citation"
    );
    assert_eq!(
        ADMITTED_ALPHA_INPUTS,
        [
            AlphaInput::Texel0,
            AlphaInput::Primitive,
            AlphaInput::Environment,
            AlphaInput::One,
            AlphaInput::Zero,
            AlphaInput::PrimLodFrac,
        ],
        "ADMITTED_ALPHA_INPUTS changed; same rule as the color table"
    );
    // `Combined`/`CombinedAlpha` are deliberately NOT in either table:
    // their admissibility is slice-scoped, not selector-scoped, and
    // lives in `resolves_the_combined_selector`.
    assert!(
        !ADMITTED_COLOR_INPUTS.contains(&ColorInput::Combined)
            && !ADMITTED_COLOR_INPUTS.contains(&ColorInput::CombinedAlpha)
            && !ADMITTED_ALPHA_INPUTS.contains(&AlphaInput::Combined),
        "the COMBINED selectors must stay out of the flat tables, or the slice rule is \
         bypassed for a two-cycle program's first cycle"
    );
}

/// The other unmeasured selectors are refused too, each by name -- not
/// only `Shade`. Swept over every selector the wire can express in
/// color slot A and alpha slot A, so a selector added to `ColorInput`
/// later cannot be silently admitted.
///
/// **Admission has two independent rules, and the sweep must model
/// both.** [`ADMITTED_COLOR_INPUTS`]/[`ADMITTED_ALPHA_INPUTS`] are the
/// register-and-texel table, and
/// [`CombinerProgramSlice::resolves_the_combined_selector`] is a
/// separate slice-scoped rule for `Combined`/`CombinedAlpha`, which are
/// deliberately absent from those tables because their admissibility
/// depends on the cycle mode rather than on the selector alone. This
/// sweep runs `validate_one_cycle`, so for it the second rule says
/// `Combined` IS admitted -- it reads RT64's zero-initialized
/// accumulator (`rt64_color_combiner.h:470-471`, `611-620`). Deriving
/// the expectation from the table alone would assert the opposite and
/// contradict the gate this crate actually ships.
///
/// There is a **third** rule, also deliberately outside the tables:
/// `Shade`/`ShadeAlpha` is primitive-scoped. `validate_one_cycle` is
/// the texrect entry point, and a rectangle's shade is the hardware's
/// zero (derived in [`TexrectShading::base_inputs`]), so this sweep
/// expects it admitted. The same selector on an UNSHADED raw triangle
/// is still refused -- see
/// [`an_unshaded_raw_triangle_still_refuses_shade`].
#[test]
fn every_unmeasured_selector_is_refused() {
    for index in 0u32..16 {
        let params = pack_second_cycle([index, 8, 16, 7], [7, 7, 7, 7]);
        let input = params.decode_color(ColorInputSlot::A, true);
        // All three admission rules, exactly as `admits_color`
        // composes them -- see this test's own doc.
        let admitted = matches!(input, ColorInput::Combined | ColorInput::CombinedAlpha)
            || matches!(input, ColorInput::Shade | ColorInput::ShadeAlpha)
            || ADMITTED_COLOR_INPUTS
                .iter()
                .any(|a| core::mem::discriminant(a) == core::mem::discriminant(&input));
        let result = TexrectShading::new(
            params,
            Color4::from_wire(ENV_WIRE),
            PrimColor::from_wire(PRIM_LOD_W0, PRIM_WIRE),
        )
        .validate_one_cycle();
        if admitted {
            assert!(
                result.is_ok(),
                "color index {index} ({input:?}) must be admitted"
            );
        } else {
            assert_eq!(
                result,
                Err(TexrectExecutionError::UnsupportedColorInput {
                    slot: ColorInputSlot::A,
                    input,
                }),
                "color index {index} decodes to {input:?}, which must be refused by name"
            );
        }
    }
    for index in 0u32..8 {
        let params = pack_second_cycle([8, 8, 16, 7], [index, 7, 7, 7]);
        let input = params.decode_alpha(AlphaInputSlot::A, true);
        let admitted = matches!(input, AlphaInput::Combined)
            || matches!(input, AlphaInput::Shade)
            || ADMITTED_ALPHA_INPUTS
                .iter()
                .any(|a| core::mem::discriminant(a) == core::mem::discriminant(&input));
        let result = TexrectShading::new(
            params,
            Color4::from_wire(ENV_WIRE),
            PrimColor::from_wire(PRIM_LOD_W0, PRIM_WIRE),
        )
        .validate_one_cycle();
        if admitted {
            assert!(
                result.is_ok(),
                "alpha index {index} ({input:?}) must be admitted"
            );
        } else {
            assert_eq!(
                result,
                Err(TexrectExecutionError::UnsupportedAlphaInput {
                    slot: AlphaInputSlot::A,
                    input,
                }),
                "alpha index {index} decodes to {input:?}, which must be refused by name"
            );
        }
    }
}

/// **D4, the register-backed widening.** Four color selectors and one
/// alpha selector this executor refused resolve to components of values
/// it already sources from real wire registers, so evaluating them
/// invents nothing.
///
/// `docs/rt64/RT64-LANE-DIVERGENCES.md` D4 scores twelve refused selectors
/// reference-correct on the ground that `crate::combiner` implements
/// every one of them. That is true and it is not sufficient: the
/// combiner implementing a selector means it can *read* a
/// `CombinerInputs` field, not that this executor can *fill* it. This
/// test separates the two, admitting exactly the register-backed subset
/// and pinning the rest as still-refused.
///
/// Expectations are hand-derived from `combiner.rs`'s
/// `resolve_color_input`/`resolve_alpha_input` and from
/// `combiner_inputs_from_fragment_registers`, and the wire indices are
/// found by asking the decoder rather than transcribed -- so a decode
/// table change moves the probe instead of silently testing the wrong
/// selector. Deliberately does NOT consult `ADMITTED_COLOR_INPUTS`, the
/// way the exhaustive sweep above does: a test that derives its
/// expectation from the constant under test cannot fail when that
/// constant changes, which is why the sweep stayed green across this
/// widening.
#[test]
fn register_backed_selectors_are_admitted_and_invented_ones_are_not() {
    // Find a (slot, wire index) pair decoding to `target`. Slot C is
    // the five-bit slot reaching most extended selectors, but not all:
    // `Noise` is slot-A only, and slot A is four bits. Ask the decoder
    // which slot can express the selector rather than assuming one.
    let color_probe_for = |target: ColorInput| -> (ColorInputSlot, CombineParams) {
        for index in 0u32..32 {
            let params = pack_second_cycle([1, 1, index, 1], [7, 7, 7, 7]);
            if core::mem::discriminant(&params.decode_color(ColorInputSlot::C, true))
                == core::mem::discriminant(&target)
            {
                return (ColorInputSlot::C, params);
            }
        }
        // Slots A, B and D are four-bit and reach different subsets:
        // `Noise` is slot-A only and `K4` is slot-B only. Try each.
        for index in 0u32..16 {
            for (slot, params) in [
                (
                    ColorInputSlot::A,
                    pack_second_cycle([index, 1, 16, 1], [7, 7, 7, 7]),
                ),
                (
                    ColorInputSlot::B,
                    pack_second_cycle([1, index, 16, 1], [7, 7, 7, 7]),
                ),
                (
                    ColorInputSlot::D,
                    pack_second_cycle([1, 1, 16, index], [7, 7, 7, 7]),
                ),
            ] {
                if core::mem::discriminant(&params.decode_color(slot, true))
                    == core::mem::discriminant(&target)
                {
                    return (slot, params);
                }
            }
        }
        panic!("no color wire index decodes to {target:?}")
    };
    let alpha_index_for = |target: AlphaInput| -> u32 {
        (0u32..8)
            .find(|index| {
                let params = pack_second_cycle([1, 1, 16, 7], [7, 7, *index, 7]);
                core::mem::discriminant(&params.decode_alpha(AlphaInputSlot::C, true))
                    == core::mem::discriminant(&target)
            })
            .unwrap_or_else(|| panic!("no slot-C alpha wire index decodes to {target:?}"))
    };

    let shading = |params| {
        TexrectShading::new(
            params,
            Color4::from_wire(ENV_WIRE),
            PrimColor::from_wire(PRIM_LOD_W0, PRIM_WIRE),
        )
        .validate_one_cycle()
    };

    // ADMITTED: each reads a component of a register this executor
    // already sources. Before this widening every one of these was
    // `UnsupportedColorInput`.
    for target in [
        ColorInput::Texel0Alpha,
        ColorInput::PrimitiveAlpha,
        ColorInput::EnvAlpha,
        ColorInput::PrimLodFrac,
    ] {
        let (slot, params) = color_probe_for(target);
        assert!(
            shading(params).is_ok(),
            "{target:?} (via {slot:?}) reads a register this executor \
             already supplies and must be admitted"
        );
    }
    let index = alpha_index_for(AlphaInput::PrimLodFrac);
    let params = pack_second_cycle([1, 1, 16, 7], [7, 7, index, 7]);
    assert!(
        shading(params).is_ok(),
        "AlphaInput::PrimLodFrac (slot-C index {index}) comes from the \
         same SetPrimColor word as Primitive and must be admitted"
    );

    // STILL REFUSED: each reads a `base_inputs` field this executor
    // leaves at zero *with no authority saying zero is what the
    // hardware produces*. There is no `SetConvert`/`SetKey` plumbing,
    // no LOD stage, no noise seed, and no decoded tile+1 -- so
    // admitting one would combine against an invented value.
    //
    // `Shade`/`ShadeAlpha` is deliberately NOT in this list any more,
    // and the distinction is the whole point of the list. Its zero is
    // not an accidental unset field: a texture rectangle carries no
    // shade words, and fn64 reads that wire layout as requiring zero for
    // the synthesized primitive. This zero-shade rule is fn64's own
    // reading and is not independently confirmed against an allowed
    // hardware reference. See
    // [`a_texrects_shade_evaluates_to_the_hardwares_zero_not_a_neighbouring_register`]
    // and [`TexrectShading::base_inputs`]. The unshaded-*triangle*
    // refusal, where the hardware really does interpolate a value this
    // executor cannot reconstruct, is pinned by
    // [`an_unshaded_raw_triangle_still_refuses_shade`].
    for target in [
        ColorInput::LodFraction,
        ColorInput::Noise,
        ColorInput::K4,
        ColorInput::K5,
        ColorInput::KeyCenter,
        ColorInput::KeyScale,
        ColorInput::Texel1,
        ColorInput::Texel1Alpha,
    ] {
        let (slot, params) = color_probe_for(target);
        assert_eq!(
            shading(params),
            Err(TexrectExecutionError::UnsupportedColorInput {
                slot,
                input: target,
            }),
            "{target:?} (via {slot:?}) reads a zeroed field and must \
             stay refused by name"
        );
    }

    // **The register-backed selectors are admitted even against a
    // never-written register.** `SetEnvColor`/`SetPrimColor` name RDP
    // registers, which hold their power-on zero until the guest writes
    // them, so reading one before any wire command is a legal read of a
    // real value -- not a substitution. This is the opposite assertion
    // to the loop above, and the difference is the point: `LodFraction`
    // has no authority behind its zero at all, while `EnvAlpha` reads a
    // real RDP register and `Shade` reads a value the rasterizer
    // demonstrably clears.
    for target in [
        ColorInput::EnvAlpha,
        ColorInput::PrimitiveAlpha,
        ColorInput::PrimLodFrac,
    ] {
        let (_, params) = color_probe_for(target);
        assert!(
            TexrectShading::new(params, Color4::from_wire(0), PrimColor::from_wire(0, 0))
                .validate_one_cycle()
                .is_ok(),
            "{target:?} reads a register that always holds a value, so a never-written \
             register must not refuse the rectangle"
        );
    }
    let index = alpha_index_for(AlphaInput::PrimLodFrac);
    let params = pack_second_cycle([1, 1, 16, 7], [7, 7, index, 7]);
    assert!(
        TexrectShading::new(params, Color4::from_wire(0), PrimColor::from_wire(0, 0))
            .validate_one_cycle()
            .is_ok()
    );
}

/// **A never-written constant register reads as its power-on zero
/// rather than refusing the rectangle**, and the value it supplies is
/// really the register's -- a written register still wins.
///
/// This replaces a test that asserted the opposite. The refusal it
/// pinned invented an "unset" state the RDP has no way to be in:
/// `fn64-render-reference` models the constant color registers as
/// zero-initialized `[u8; 4]` (`gbi/state.rs:227`, `:387`) and RT64's
/// C++ zero-initializes `primColor`/`envColor` at
/// `src/hle/rt64_state.cpp:126-129`.
#[test]
fn a_never_written_constant_register_reads_as_zero_instead_of_refusing() {
    let unwritten = TexrectShading::new(
        env_lerp_program(),
        Color4::from_wire(0),
        PrimColor::from_wire(0, 0),
    )
    .validate_one_cycle()
    .expect("a program reading ENVIRONMENT/PRIMITIVE before any wire command is legal");
    // Derived by hand: an RDP color register powers up holding four
    // zero bytes, and `Color4::normalized` is `byte / 255.0`, so every
    // channel is exactly 0.0.
    let inputs = unwritten.base_inputs();
    assert_eq!(inputs.env_color, [0.0; 4]);
    assert_eq!(inputs.prim_color, [0.0; 4]);
    assert_eq!(inputs.prim_lod_frac, 0.0);

    // A written register must actually reach the combiner inputs, or
    // the assertions above could pass against a hardcoded zero that
    // ignores every SetEnvColor/SetPrimColor.
    let written = TexrectShading::new(
        env_lerp_program(),
        Color4::from_wire(ENV_WIRE),
        PrimColor::from_wire(PRIM_LOD_W0, PRIM_WIRE),
    )
    .validate_one_cycle()
    .expect("a program reading written registers is legal");
    let written_inputs = written.base_inputs();
    assert_ne!(
        written_inputs.env_color, inputs.env_color,
        "a written SetEnvColor must differ from the power-on zero, or this test cannot \
         distinguish a real register read from a hardcoded zero"
    );
    assert_ne!(
        written_inputs.prim_color, inputs.prim_color,
        "a written SetPrimColor must differ from the power-on zero"
    );

    // A ZERO-only program reads neither register and is legal either
    // way -- unchanged by this fix.
    let neither = pack_second_cycle([8, 8, 16, 7], [7, 7, 7, 7]);
    assert!(
        TexrectShading::new(neither, Color4::from_wire(0), PrimColor::from_wire(0, 0))
            .validate_one_cycle()
            .is_ok(),
        "a program reading neither constant register stays admitted"
    );
}

/// **Which evaluation each cycle type selects at the EXECUTOR, not only
/// in the error type's prose** -- mutant (f), and this test exists
/// because that mutant SURVIVED its first draft.
///
/// Reaching `execute_texture_rectangle` needs a live pending TMEM
/// transaction, which no unit test can build. What this test pins
/// instead is the decision the executor makes, extracted as
/// [`admitted_cycle_evaluation`] -- the same match, called by the
/// executor. Exhaustive over all four `CycleType` variants, so a fifth
/// added later cannot be silently admitted either.
///
/// Note what this test does **not** prove and never did: that the
/// evaluation it names is the arithmetic that runs. Naming
/// `TexrectCombinerEvaluation::TwoCycle` here would still pass if
/// `combine_one_texel` ignored it and called `run_one_cycle` anyway.
/// [`two_cycle_carries_the_accumulator_one_cycle_cannot`] is the test
/// that closes that, and it is the reason the widening was allowed to
/// land at all.
#[test]
fn the_executor_admits_copy_one_cycle_and_two_cycle() {
    assert_eq!(
        admitted_cycle_evaluation(CycleType::Copy),
        Ok(TexrectCombinerEvaluation::BlitsTheTexel),
        "Copy is admitted and evaluates NO combiner"
    );
    assert_eq!(
        admitted_cycle_evaluation(CycleType::OneCycle),
        Ok(TexrectCombinerEvaluation::OneCycle),
        "OneCycle is admitted and evaluates ONE combiner pass"
    );
    assert_eq!(
        admitted_cycle_evaluation(CycleType::TwoCycle),
        Ok(TexrectCombinerEvaluation::TwoCycle),
        "TwoCycle is admitted and evaluates BOTH combiner passes"
    );
    assert_eq!(
        admitted_cycle_evaluation(CycleType::Fill),
        Err(TexrectExecutionError::UnsupportedCycleType {
            cycle_type: CycleType::Fill
        }),
        "Fill samples no texture and must still be refused by name"
    );
}

/// **Positive control for [`carry_program`]**: the merged wire words
/// really do decode to two *different* programs in the two slices.
///
/// Without this, a packing slip could put the same selectors in both
/// slices, and the witness test below would compare one-cycle against a
/// two-cycle run of the same formula -- which could pass for the wrong
/// reason.
#[test]
fn two_cycle_wire_program_decodes_to_both_slices() {
    let program = carry_program();
    assert_eq!(
        [
            program.decode_color(ColorInputSlot::A, false),
            program.decode_color(ColorInputSlot::B, false),
            program.decode_color(ColorInputSlot::C, false),
            program.decode_color(ColorInputSlot::D, false),
        ],
        [
            ColorInput::Zero,
            ColorInput::Zero,
            ColorInput::Zero,
            ColorInput::Primitive,
        ],
        "cycle 0's RGB must be (Zero - Zero) * Zero + Primitive"
    );
    assert_eq!(
        [
            program.decode_color(ColorInputSlot::A, true),
            program.decode_color(ColorInputSlot::B, true),
            program.decode_color(ColorInputSlot::C, true),
            program.decode_color(ColorInputSlot::D, true),
        ],
        [
            ColorInput::Zero,
            ColorInput::Zero,
            ColorInput::Zero,
            ColorInput::Combined,
        ],
        "cycle 1's RGB must be (Zero - Zero) * Zero + Combined"
    );
    assert_eq!(
        [
            program.decode_alpha(AlphaInputSlot::A, false),
            program.decode_alpha(AlphaInputSlot::B, false),
            program.decode_alpha(AlphaInputSlot::C, false),
            program.decode_alpha(AlphaInputSlot::D, false),
        ],
        [
            AlphaInput::Zero,
            AlphaInput::Zero,
            AlphaInput::Zero,
            AlphaInput::Primitive,
        ],
        "cycle 0's alpha must be (Zero - Zero) * Zero + Primitive"
    );
    assert_eq!(
        [
            program.decode_alpha(AlphaInputSlot::A, true),
            program.decode_alpha(AlphaInputSlot::B, true),
            program.decode_alpha(AlphaInputSlot::C, true),
            program.decode_alpha(AlphaInputSlot::D, true),
        ],
        [
            AlphaInput::Zero,
            AlphaInput::Zero,
            AlphaInput::Zero,
            AlphaInput::Combined,
        ],
        "cycle 1's alpha must be (Zero - Zero) * Zero + Combined"
    );
}

/// **Two-cycle evaluation carries cycle 0's result into cycle 1, and
/// one-cycle evaluation of the same program cannot** -- the observation
/// the previous draft of this module was missing.
///
/// The refusal that used to sit at [`admitted_cycle_evaluation`] recorded
/// its own blind spot: "while this match was inline, widening it to
/// admit two-cycle left the entire suite green." A green suite proved
/// nothing was broken, not that anything was evaluated. Nothing in the
/// suite ever ran two-cycle *arithmetic*, so the widened arm was
/// unobserved either way.
///
/// This test observes it. [`carry_program`]'s two slices are different
/// formulas by construction (asserted above), and the hand derivation
/// is:
///
/// - cycle 0: `(0 - 0) * 0 + Primitive` = the primitive colour,
///   `0x80/0xFF/0x40/0x80` normalized, written into the accumulator;
/// - cycle 1: `(0 - 0) * 0 + Combined` = that accumulator verbatim.
///
/// So two-cycle must give back the primitive colour's own bytes. The
/// same program run as one-cycle evaluates **only** the second slice
/// against the zero-initialized accumulator, where `Combined` is `0.0`,
/// so it must give transparent black. Both are asserted, and asserted to
/// differ -- the inequality alone would be satisfied by two equally
/// wrong answers.
///
/// `wrap_clamp` is the identity on both: every channel of both answers
/// is already inside `[0, 1]`.
#[test]
fn two_cycle_carries_the_accumulator_one_cycle_cannot() {
    let program = carry_program();
    let base = TexrectShading::new(
        program,
        Color4::from_wire(ENV_WIRE),
        PrimColor::from_wire(PRIM_LOD_W0, PRIM_WIRE),
    )
    .base_inputs();
    // The texel is deliberately non-zero and unlike the primitive
    // colour: neither slice reads TEXEL0, so a two-cycle answer that
    // leaked the texel would be caught here rather than mistaken for
    // the carry.
    let texel = [0x18, 0x40, 0xC8, 0xFF];

    let two_cycle = combine_one_texel(program, base, texel, TexrectCombinerEvaluation::TwoCycle);
    let one_cycle = combine_one_texel(program, base, texel, TexrectCombinerEvaluation::OneCycle);

    assert_eq!(
        two_cycle,
        [0x80, 0xFF, 0x40, 0x80],
        "cycle 0 writes the primitive colour into the accumulator and cycle 1 emits it \
         verbatim through D = COMBINED"
    );
    assert_eq!(
        one_cycle,
        [0x00, 0x00, 0x00, 0x00],
        "one-cycle mode runs ONLY the second slice, whose D = COMBINED reads the \
         zero-initialized accumulator"
    );
    assert_ne!(
        two_cycle, one_cycle,
        "if these agree, the two-cycle arm is not running two cycles"
    );
}

/// **`Combined` is admitted everywhere except a two-cycle program's
/// FIRST slice**, and in one-cycle mode it resolves to RT64's
/// zero-initialized accumulator rather than to a value this executor
/// invents.
///
/// See [`CombinerProgramSlice::resolves_the_combined_selector`] for the
/// RT64 citation (`rt64_color_combiner.h:470-471`, `611-620`, `577`)
/// and the ROM measurement.
///
/// This asserts the ADMISSION rule and the ARITHMETIC together, because
/// admitting the selector is only correct if the value behind it is the
/// hardware's. Hand-derived from [`carry_program`]'s second slice,
/// `(Zero - Zero) * Zero + Combined` over a zero accumulator, which is
/// transparent black.
#[test]
fn combined_is_admitted_outside_the_first_slice_of_two_cycles() {
    let program = carry_program();
    let shading = TexrectShading::new(
        program,
        Color4::from_wire(ENV_WIRE),
        PrimColor::from_wire(PRIM_LOD_W0, PRIM_WIRE),
    );
    shading
        .validate_combiner_program(CombinerProgramCycles::BothSlices)
        .expect("cycle 1 has a first-cycle result for COMBINED to read");
    shading
        .validate_combiner_program(CombinerProgramCycles::OnlySecondSlice)
        .expect(
            "one-cycle COMBINED reads RT64's zero-initialized accumulator, which is \
             defined behaviour",
        );
    assert_eq!(
        shading.validate_one_cycle(),
        shading.validate_combiner_program(CombinerProgramCycles::OnlySecondSlice),
        "validate_one_cycle must stay an exact alias for the one-slice admission"
    );

    // The admitted program must also EVALUATE to the hardware's answer,
    // not merely get past the gate.
    let base = shading.base_inputs();
    assert_eq!(
        combine_one_texel(
            program,
            base,
            [0x18, 0x40, 0xC8, 0xFF],
            TexrectCombinerEvaluation::OneCycle
        ),
        [0x00, 0x00, 0x00, 0x00],
        "one-cycle D = COMBINED reads the zero-initialized accumulator"
    );
}

/// **The alpha `Combined` and the colour `CombinedAlpha` selectors go
/// through the same slice gate as the plain colour `Combined`.**
///
/// Both are distinct decode paths --
/// `alphaInputABD` index `0` is `AlphaInput::Combined`
/// (`combiner.rs`'s `alpha_input_abd`), and `colorInputC` index `7` is
/// `ColorInput::CombinedAlpha` (`color_input_c`) -- and RT64 resolves
/// both from the same accumulator (`rt64_color_combiner.h:486-487`
/// `C_COMBINED_ALPHA -> combinerColor.a`, `517-518` `A_COMBINED ->
/// combinerAlpha`). A gate that admitted the plain colour selector but
/// left either of these unguarded would let a two-cycle cycle-0
/// program through the one door this repair deliberately keeps shut.
///
/// Written because mutants that bypassed `admits_alpha`'s gate and that
/// dropped `CombinedAlpha` from `admits_color`'s guarded set both
/// SURVIVED the admission tests above.
#[test]
fn the_alpha_and_combined_alpha_selectors_share_the_slice_gate() {
    // Alpha slot A = COMBINED (index 0) in cycle 0 of a two-cycle
    // program; every colour slot and the rest of alpha are Zero.
    let alpha_combined_first = merge_cycles(
        pack_first_cycle([8, 8, 16, 7], [0, 7, 7, 7]),
        pack_second_cycle([8, 8, 16, 7], [7, 7, 7, 7]),
    );
    let shading = TexrectShading::new(
        alpha_combined_first,
        Color4::from_wire(ENV_WIRE),
        PrimColor::from_wire(PRIM_LOD_W0, PRIM_WIRE),
    );
    assert_eq!(
        shading.validate_combiner_program(CombinerProgramCycles::BothSlices),
        Err(TexrectExecutionError::UnsupportedAlphaInput {
            slot: AlphaInputSlot::A,
            input: AlphaInput::Combined,
        }),
        "alpha COMBINED in cycle 0 of a two-cycle program must be refused"
    );
    shading
        .validate_combiner_program(CombinerProgramCycles::OnlySecondSlice)
        .expect("the same word read as one-cycle names no COMBINED at all");

    // Colour slot C = COMBINED_ALPHA (index 7) in cycle 0.
    let combined_alpha_first = merge_cycles(
        pack_first_cycle([8, 8, 7, 7], [7, 7, 7, 7]),
        pack_second_cycle([8, 8, 16, 7], [7, 7, 7, 7]),
    );
    let shading = TexrectShading::new(
        combined_alpha_first,
        Color4::from_wire(ENV_WIRE),
        PrimColor::from_wire(PRIM_LOD_W0, PRIM_WIRE),
    );
    assert_eq!(
        shading.validate_combiner_program(CombinerProgramCycles::BothSlices),
        Err(TexrectExecutionError::UnsupportedColorInput {
            slot: ColorInputSlot::C,
            input: ColorInput::CombinedAlpha,
        }),
        "COMBINED_ALPHA in cycle 0 of a two-cycle program must be refused"
    );

    // ...and both are ADMITTED in one-cycle mode, where the zero
    // accumulator is the hardware's own answer.
    let alpha_combined_one_cycle = pack_second_cycle([8, 8, 16, 7], [0, 7, 7, 7]);
    TexrectShading::new(
        alpha_combined_one_cycle,
        Color4::from_wire(ENV_WIRE),
        PrimColor::from_wire(PRIM_LOD_W0, PRIM_WIRE),
    )
    .validate_combiner_program(CombinerProgramCycles::OnlySecondSlice)
    .expect("one-cycle alpha COMBINED reads the zero-initialized accumulator");

    let combined_alpha_one_cycle = pack_second_cycle([8, 8, 7, 7], [7, 7, 7, 7]);
    TexrectShading::new(
        combined_alpha_one_cycle,
        Color4::from_wire(ENV_WIRE),
        PrimColor::from_wire(PRIM_LOD_W0, PRIM_WIRE),
    )
    .validate_combiner_program(CombinerProgramCycles::OnlySecondSlice)
    .expect("one-cycle COMBINED_ALPHA reads the zero-initialized accumulator");
}

/// **A two-cycle program's FIRST slice still refuses `Combined`.**
///
/// The widening above is not "admit COMBINED everywhere". This pins the
/// arm the repair KEEPS: no measurement in this repo covers a
/// `COMBINED` read in cycle 0 of a two-cycle program.
#[test]
fn combined_in_the_first_slice_of_two_cycles_is_still_refused() {
    // Cycle 0 slot A = COMBINED (index 0); cycle 1 is an
    // all-Zero/Primitive program that admits cleanly on its own.
    let program = merge_cycles(
        pack_first_cycle([0, 8, 16, 3], [7, 7, 7, 3]),
        pack_second_cycle([8, 8, 16, 3], [7, 7, 7, 3]),
    );
    let shading = TexrectShading::new(
        program,
        Color4::from_wire(ENV_WIRE),
        PrimColor::from_wire(PRIM_LOD_W0, PRIM_WIRE),
    );
    assert_eq!(
        shading.validate_combiner_program(CombinerProgramCycles::BothSlices),
        Err(TexrectExecutionError::UnsupportedColorInput {
            slot: ColorInputSlot::A,
            input: ColorInput::Combined,
        }),
        "cycle 0 of a two-cycle program has no first-cycle result behind COMBINED"
    );
    shading
        .validate_combiner_program(CombinerProgramCycles::OnlySecondSlice)
        .expect("the same wire word read as one-cycle names no COMBINED at all");
}

/// **`Texel1` stays refused in BOTH slices of a two-cycle program.**
///
/// A rectangle binds one tile ([`TexrectTileBinding`] carries a single
/// descriptor), so there is no decoded tile+1 to sample. The reference
/// lane refuses `Texel1` for a rectangle for exactly that reason
/// (`fn64-render-reference/src/backend/validate.rs:479-483`). Widening
/// the cycle admission must not widen this one.
#[test]
fn texel1_is_refused_in_both_slices_of_a_two_cycle_program() {
    // Color slot A index 2 is TEXEL0's neighbour TEXEL1 in
    // `colorInputCommon`; placed in cycle 0 first, then in cycle 1.
    let in_first = merge_cycles(
        pack_first_cycle([2, 8, 16, 3], [7, 7, 7, 3]),
        pack_second_cycle([8, 8, 16, 0], [7, 7, 7, 0]),
    );
    let in_second = merge_cycles(
        pack_first_cycle([8, 8, 16, 3], [7, 7, 7, 3]),
        pack_second_cycle([2, 8, 16, 0], [7, 7, 7, 0]),
    );
    for (program, slice) in [(in_first, "cycle 0"), (in_second, "cycle 1")] {
        let shading = TexrectShading::new(
            program,
            Color4::from_wire(ENV_WIRE),
            PrimColor::from_wire(PRIM_LOD_W0, PRIM_WIRE),
        );
        assert_eq!(
            shading.validate_combiner_program(CombinerProgramCycles::BothSlices),
            Err(TexrectExecutionError::UnsupportedColorInput {
                slot: ColorInputSlot::A,
                input: ColorInput::Texel1,
            }),
            "TEXEL1 in {slice} has no decoded tile+1 behind it and must be refused"
        );
    }
}

/// **The reason Fill cannot be admitted by widening the match above,
/// as an executable fact rather than a comment.**
///
/// A texrect reaches [`execute_texture_rectangle`] as an
/// already-resolved [`RectViewportPixels`], built by
/// `raw_dpc/texture_rectangle.rs`'s port of RT64's `FixedRect`:
/// `(coord + 3) >> 2` at both ends, half-open. A fill rectangle's rule
/// is `targets/fill.rs`'s [`super::resolve_fill_pixel_rectangle`]:
/// `coord >> 2` at both ends, inclusive.
///
/// This asserts the two disagree on wire coordinates that are legal for
/// both -- every edge a multiple of four, so the fill rule's
/// `FractionalEdge` refusal does not fire and the disagreement is
/// purely the inclusive/half-open split. If a future change made them
/// agree, admitting Fill above would become a one-line fix and this
/// test would say so by failing.
///
/// Both rules are re-derived in this test body from their published
/// expressions on the texrect side, so this pins the disagreement
/// itself rather than one implementation against the other. The fill
/// side calls the real `resolve_fill_pixel_rectangle`, since that is
/// the function a fix would have to route to.
#[test]
fn the_texrect_and_fill_rectangle_rules_disagree_by_a_pixel_on_every_axis() {
    // Wire 2.2 fixed-point coordinates, every edge a whole pixel.
    for (ulx, uly, lrx, lry) in [
        (0u16, 0u16, 16u16, 16u16),
        (8, 8, 40, 24),
        (0, 0, 1276, 956),
    ] {
        // The texrect side, re-derived: fill mode rounds the upper-left
        // down (`ulx &= !3`, a no-op on these whole-pixel edges), then
        // `FixedRect::left/top/right/bottom` with `ceil = true`.
        let left = ((i32::from(ulx) & !3) + 3) >> 2;
        let top = ((i32::from(uly) & !3) + 3) >> 2;
        let right = (i32::from(lrx) + 3) >> 2;
        let bottom = (i32::from(lry) + 3) >> 2;
        let texrect_extent = (right - left, bottom - top);

        // The fill side, through the executor a fix would route to.
        let fill = super::super::resolve_fill_pixel_rectangle(ulx, uly, lrx, lry)
            .expect("every edge here is a whole pixel");
        let fill_extent = (fill.width() as i32, fill.height() as i32);

        assert_eq!(
            fill_extent,
            (texrect_extent.0 + 1, texrect_extent.1 + 1),
            "wire ({ulx}, {uly}, {lrx}, {lry}): the fill rule is inclusive and the texrect \
             rule is half-open, so the fill rectangle is exactly one pixel larger on each \
             axis"
        );
        assert_ne!(
            fill_extent, texrect_extent,
            "if these ever agree, admitting Fill at admitted_cycle_evaluation becomes a \
             one-line fix and this test must be re-justified"
        );
    }
}

/// **The fill rule refuses a fractional edge the texrect rule silently
/// rounds** -- the second half of why the two are not interchangeable.
#[test]
fn the_fill_rule_refuses_a_fractional_edge_the_texrect_rule_rounds() {
    // `ulx = 2` is half a pixel.
    let texrect_left = ((2i32 & !3) + 3) >> 2;
    assert_eq!(
        texrect_left, 0,
        "the texrect rule rounds a half-pixel upper-left down to pixel 0"
    );
    assert!(
        super::super::resolve_fill_pixel_rectangle(2, 2, 18, 18).is_err(),
        "the fill rule refuses a fractional edge by name rather than rounding it"
    );
}

/// **Fill remains refused by name** -- the admission widened by exactly
/// one mode, not into a blanket acceptance.
///
/// Checked at the enum rather than through the executor because
/// reaching the executor needs a live pending TMEM transaction, which
/// the end-to-end tests supply; what is pinned here is that the mode
/// set this module claims is `{Copy, OneCycle, TwoCycle}` and its
/// complement is named.
#[test]
fn the_admitted_cycle_set_is_copy_one_cycle_and_two_cycle() {
    let cycle_type = CycleType::Fill;
    let error = TexrectExecutionError::UnsupportedCycleType { cycle_type };
    let message = error.to_string();
    assert!(
        message.contains(&format!("{cycle_type:?}")),
        "the refusal must name the mode it refused: {message}"
    );
    assert!(
        message.contains("Copy") && message.contains("OneCycle") && message.contains("TwoCycle"),
        "the refusal must state which modes ARE admitted: {message}"
    );
}
