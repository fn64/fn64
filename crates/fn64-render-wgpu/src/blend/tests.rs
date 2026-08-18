#![allow(clippy::too_many_arguments)]

use super::*;
use crate::state::OtherMode;

fn other_mode(
    cycle_type_bits: u32,
    low_extra: u32,
    force_blend: bool,
    image_read: bool,
) -> OtherMode {
    let mut low = low_extra;
    if force_blend {
        low |= 0x4000;
    }
    if image_read {
        low |= 0x0040;
    }
    OtherMode::from_wire(cycle_type_bits << 20, low)
}

fn one_cycle_mode_state(
    color_a: u8,
    alpha_a: u8,
    color_b: u8,
    alpha_b: u8,
    force_blend: bool,
    image_read: bool,
    blend_color_register: [u8; 4],
    fog_color: [u8; 4],
) -> BlendModeState {
    // blender_cycle_1 reads low bits 30:31 (color_a), 26:27 (alpha_a),
    // 22:23 (color_b), 18:19 (alpha_b).
    let low_extra = ((color_a as u32 & 0x3) << 30)
        | ((alpha_a as u32 & 0x3) << 26)
        | ((color_b as u32 & 0x3) << 22)
        | ((alpha_b as u32 & 0x3) << 18);
    BlendModeState {
        other_mode: other_mode(0, low_extra, force_blend, image_read),
        blend_color_register,
        fog_color,
    }
}

fn two_cycle_mode_state(
    cycle1: (u8, u8, u8, u8),
    cycle2: (u8, u8, u8, u8),
    force_blend: bool,
    image_read: bool,
    blend_color_register: [u8; 4],
    fog_color: [u8; 4],
) -> BlendModeState {
    let low_extra = ((cycle1.0 as u32 & 0x3) << 30)
        | ((cycle1.1 as u32 & 0x3) << 26)
        | ((cycle1.2 as u32 & 0x3) << 22)
        | ((cycle1.3 as u32 & 0x3) << 18)
        | ((cycle2.0 as u32 & 0x3) << 28)
        | ((cycle2.1 as u32 & 0x3) << 24)
        | ((cycle2.2 as u32 & 0x3) << 20)
        | ((cycle2.3 as u32 & 0x3) << 16);
    BlendModeState {
        other_mode: other_mode(1, low_extra, force_blend, image_read),
        blend_color_register,
        fog_color,
    }
}

fn copy_mode_state() -> BlendModeState {
    BlendModeState {
        other_mode: other_mode(2, 0, false, false),
        blend_color_register: [0; 4],
        fog_color: [0; 4],
    }
}

fn fill_mode_state() -> BlendModeState {
    BlendModeState {
        other_mode: other_mode(3, 0, false, false),
        blend_color_register: [0; 4],
        fog_color: [0; 4],
    }
}

fn fb_sample(rgba: [u8; 4], coverage_count: u8) -> BlendFramebufferSample {
    BlendFramebufferSample {
        rgba,
        coverage_count,
    }
}

// --- Selector decode totality ------------------------------------------

#[test]
fn every_two_bit_color_selector_decodes_to_a_distinct_variant() {
    assert_eq!(BlendColorInput::from_wire(0), BlendColorInput::Combined);
    assert_eq!(BlendColorInput::from_wire(1), BlendColorInput::Framebuffer);
    assert_eq!(BlendColorInput::from_wire(2), BlendColorInput::Blend);
    assert_eq!(BlendColorInput::from_wire(3), BlendColorInput::Fog);
}

#[test]
fn every_two_bit_alpha_selector_decodes_to_a_distinct_variant() {
    assert_eq!(BlendAlphaInput::from_wire(0), BlendAlphaInput::Combined);
    assert_eq!(BlendAlphaInput::from_wire(1), BlendAlphaInput::Fog);
    assert_eq!(BlendAlphaInput::from_wire(2), BlendAlphaInput::Shade);
    assert_eq!(BlendAlphaInput::from_wire(3), BlendAlphaInput::Zero);
}

#[test]
fn every_two_bit_b_selector_decodes_to_a_distinct_variant() {
    assert_eq!(BlendBInput::from_wire(0), BlendBInput::OneMinusA);
    assert_eq!(BlendBInput::from_wire(1), BlendBInput::FramebufferAlpha);
    assert_eq!(BlendBInput::from_wire(2), BlendBInput::One);
    assert_eq!(BlendBInput::from_wire(3), BlendBInput::Zero);
}

#[test]
fn selector_decode_masks_out_of_range_bits_matching_two_bit_wire_width() {
    // The wire field is always exactly 2 bits in practice (BlenderCycle's
    // fields are already masked to 0x3 by OtherMode::blender_cycle_1/_2),
    // but from_wire itself masks defensively -- confirm that masking is a
    // no-op for in-range values and wraps out-of-range ones identically to
    // `& 0x3`, so a caller cannot observe different behavior for a
    // hypothetically wider input.
    for raw in 0u8..=255 {
        assert_eq!(
            BlendColorInput::from_wire(raw),
            BlendColorInput::from_wire(raw & 0x3)
        );
        assert_eq!(
            BlendAlphaInput::from_wire(raw),
            BlendAlphaInput::from_wire(raw & 0x3)
        );
        assert_eq!(
            BlendBInput::from_wire(raw),
            BlendBInput::from_wire(raw & 0x3)
        );
    }
}

// --- Copy/Fill full bypass ------------------------------------------------

#[test]
fn copy_cycle_bypasses_blender_entirely() {
    let state = copy_mode_state();
    assert_eq!(state.cycle_count(), 0);
    let src = [10, 20, 30, 40];
    let result = blend_fragment(src, None, 0, state, false).unwrap();
    assert_eq!(result.rgba, src);
}

#[test]
fn fill_cycle_bypasses_blender_entirely() {
    let state = fill_mode_state();
    assert_eq!(state.cycle_count(), 0);
    let src = [1, 2, 3, 4];
    let result = blend_fragment(src, None, 0, state, true).unwrap();
    assert_eq!(result.rgba, src);
}

// --- Exhaustive one-cycle 4x4x4x4 = 256 partition -------------------------
//
// Port card §1 "Characterization fixture partitions" axis 3: every wire-legal
// P x M x A x B combination for a single cycle, cross-referenced against an
// independent re-derivation of blend_fragment's own documented arithmetic
// (not calling blend_fragment to "test itself" blindly -- this expected()
// function is written directly from the port card's cited arithmetic and
// the reference's blend.rs doc comments, independent of blend_fragment's
// control flow).
fn expected_one_cycle(
    p: BlendColorInput,
    m: BlendColorInput,
    a_sel: BlendAlphaInput,
    b_sel: BlendBInput,
    src: [u8; 4],
    shade_alpha: u8,
    fog: [u8; 4],
    blend_color_register: [u8; 4],
    memory: Option<BlendFramebufferSample>,
    force_blend: bool,
) -> Result<[u8; 4], &'static str> {
    let src_rgb = [src[0] as f32, src[1] as f32, src[2] as f32];
    let color_of = |input: BlendColorInput| -> Result<[f32; 3], &'static str> {
        match input {
            BlendColorInput::Combined => Ok(src_rgb),
            BlendColorInput::Framebuffer => memory
                .map(|s| [s.rgba[0] as f32, s.rgba[1] as f32, s.rgba[2] as f32])
                .ok_or("framebuffer color"),
            BlendColorInput::Blend => Ok([
                blend_color_register[0] as f32,
                blend_color_register[1] as f32,
                blend_color_register[2] as f32,
            ]),
            BlendColorInput::Fog => Ok([fog[0] as f32, fog[1] as f32, fog[2] as f32]),
        }
    };
    let a_value = match a_sel {
        BlendAlphaInput::Combined => src[3],
        BlendAlphaInput::Fog => fog[3],
        BlendAlphaInput::Shade => shade_alpha,
        BlendAlphaInput::Zero => 0,
    };
    let a = a_value as f32 / 255.0;

    // One-cycle mode: the single cycle is always both first and last, so
    // whether FORCE_BL bypass applies depends only on `force_blend` (no
    // separate coverage-driven blend_enabled input in this helper -- callers
    // pass that combined decision as `force_blend` directly, matching how
    // this test drives blend_fragment with the same value as its own
    // `blend_enabled` argument).
    if !force_blend {
        let p_val = color_of(p)?;
        let alpha = if p == BlendColorInput::Framebuffer {
            0.0
        } else {
            1.0
        };
        return composite(p_val, alpha, memory);
    }

    // blend_fragment evaluates blend_color(cycle.p, ...) and
    // blend_color(cycle.m, ...) unconditionally, before branching on
    // whether either selector is Framebuffer -- so a Framebuffer selector
    // on *either* P or M requires a memory sample even though only one of
    // the two resolved values is actually used by the taken branch. This
    // mirrors the reference's own `blend.rs:191-192` unconditional
    // evaluation exactly (not a simplification this port introduced).
    let p_val = color_of(p);
    let m_val = color_of(m);
    if p == BlendColorInput::Framebuffer {
        let m_val = m_val?;
        p_val?;
        return composite(m_val, 1.0 - a, memory);
    }
    if m == BlendColorInput::Framebuffer {
        let p_val = p_val?;
        m_val?;
        return composite(p_val, a, memory);
    }
    let p_val = p_val?;
    let m_val = m_val?;
    let b = match b_sel {
        BlendBInput::OneMinusA => 1.0 - a,
        BlendBInput::FramebufferAlpha => memory
            .map(|s| s.coverage_count as f32 / 8.0)
            .ok_or("framebuffer coverage alpha")?,
        BlendBInput::One => 1.0,
        BlendBInput::Zero => 0.0,
    };
    let blended = if a == 0.0 {
        m_val
    } else if b == 0.0 {
        p_val
    } else {
        let divisor = a + b;
        let mut out = [0.0f32; 3];
        for c in 0..3 {
            out[c] = ((p_val[c] * a + m_val[c] * b) / divisor).clamp(0.0, 255.0);
        }
        out
    };
    composite(blended, 1.0, memory)
}

fn composite(
    blender_rgb: [f32; 3],
    final_alpha: f32,
    memory: Option<BlendFramebufferSample>,
) -> Result<[u8; 4], &'static str> {
    let dst = memory.map(|s| s.rgba);
    if dst.is_none() && final_alpha != 1.0 {
        return Err("framebuffer color");
    }
    let mut out = [0u8; 4];
    for c in 0..3 {
        let mem_c = dst.map_or(0.0, |rgba| rgba[c] as f32);
        out[c] = (blender_rgb[c] * final_alpha + mem_c * (1.0 - final_alpha))
            .round()
            .clamp(0.0, 255.0) as u8;
    }
    let mem_a = dst.map_or(0.0, |rgba| rgba[3] as f32);
    out[3] = (255.0 * final_alpha + mem_a * (1.0 - final_alpha))
        .round()
        .clamp(0.0, 255.0) as u8;
    Ok(out)
}

const ALL_COLOR: [BlendColorInput; 4] = [
    BlendColorInput::Combined,
    BlendColorInput::Framebuffer,
    BlendColorInput::Blend,
    BlendColorInput::Fog,
];
const ALL_ALPHA: [BlendAlphaInput; 4] = [
    BlendAlphaInput::Combined,
    BlendAlphaInput::Fog,
    BlendAlphaInput::Shade,
    BlendAlphaInput::Zero,
];
const ALL_B: [BlendBInput; 4] = [
    BlendBInput::OneMinusA,
    BlendBInput::FramebufferAlpha,
    BlendBInput::One,
    BlendBInput::Zero,
];

fn selector_wire(input: BlendColorInput) -> u8 {
    match input {
        BlendColorInput::Combined => 0,
        BlendColorInput::Framebuffer => 1,
        BlendColorInput::Blend => 2,
        BlendColorInput::Fog => 3,
    }
}
fn alpha_wire(input: BlendAlphaInput) -> u8 {
    match input {
        BlendAlphaInput::Combined => 0,
        BlendAlphaInput::Fog => 1,
        BlendAlphaInput::Shade => 2,
        BlendAlphaInput::Zero => 3,
    }
}
fn b_wire(input: BlendBInput) -> u8 {
    match input {
        BlendBInput::OneMinusA => 0,
        BlendBInput::FramebufferAlpha => 1,
        BlendBInput::One => 2,
        BlendBInput::Zero => 3,
    }
}

#[test]
fn exhaustive_one_cycle_256_selector_combinations_with_image_read_and_force_blend() {
    let src = [64, 128, 200, 90];
    let shade_alpha = 77;
    let fog = [10, 20, 30, 40];
    let blend_color_register = [200, 150, 100, 255];
    let memory = fb_sample([5, 6, 7, 8], 4);

    let mut cases = 0usize;
    for &p in &ALL_COLOR {
        for &m in &ALL_COLOR {
            for &a_sel in &ALL_ALPHA {
                for &b_sel in &ALL_B {
                    for force_blend in [false, true] {
                        for image_read in [false, true] {
                            cases += 1;
                            let mem_arg = if image_read { Some(memory) } else { None };
                            let state = one_cycle_mode_state(
                                selector_wire(p),
                                alpha_wire(a_sel),
                                selector_wire(m),
                                b_wire(b_sel),
                                force_blend,
                                image_read,
                                blend_color_register,
                                fog,
                            );
                            let expected = expected_one_cycle(
                                p,
                                m,
                                a_sel,
                                b_sel,
                                src,
                                shade_alpha,
                                fog,
                                blend_color_register,
                                mem_arg,
                                force_blend,
                            );
                            let actual =
                                blend_fragment(src, mem_arg, shade_alpha, state, force_blend);
                            match expected {
                                Ok(expected_rgba) => {
                                    assert_eq!(
                                        actual.unwrap().rgba,
                                        expected_rgba,
                                        "p={p:?} m={m:?} a={a_sel:?} b={b_sel:?} force_blend={force_blend} image_read={image_read}"
                                    );
                                }
                                Err(_) => {
                                    assert!(
                                        actual.is_err(),
                                        "p={p:?} m={m:?} a={a_sel:?} b={b_sel:?} force_blend={force_blend} image_read={image_read} expected an image-read error"
                                    );
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    // 4*4*4*4 = 256 selector combinations * 2 (force_blend) * 2 (image_read)
    // = 1024: full one-cycle partition including both axes, per the port
    // card's "manageable exhaustive enumeration" note (~1024 cases).
    assert_eq!(cases, 1024);
}

// --- Boundary alpha/divisor-collapse values -------------------------------

#[test]
fn zero_alpha_collapses_to_m_without_dividing() {
    // A selects Zero -> a=0.0 regardless of src alpha; P/M both Combined/
    // Blend so no Framebuffer branch triggers; result must equal M exactly
    // (not (P*0+M*b)/(0+b), which would be arithmetically identical but
    // this pins the *branch*, not just the coincidental numeric equality).
    let state = one_cycle_mode_state(
        selector_wire(BlendColorInput::Combined),
        alpha_wire(BlendAlphaInput::Zero),
        selector_wire(BlendColorInput::Blend),
        b_wire(BlendBInput::One),
        true,
        false,
        [9, 9, 9, 255],
        [0, 0, 0, 0],
    );
    let src = [200, 200, 200, 200];
    let result = blend_fragment(src, None, 0, state, true).unwrap();
    // M = Blend register = [9,9,9]; final_alpha=1.0 with no memory -> output
    // rgb = M, alpha = 255.
    assert_eq!(result.rgba, [9, 9, 9, 255]);
}

#[test]
fn zero_b_with_nonzero_a_collapses_to_p_without_dividing() {
    let state = one_cycle_mode_state(
        selector_wire(BlendColorInput::Blend),
        alpha_wire(BlendAlphaInput::Shade),
        selector_wire(BlendColorInput::Fog),
        b_wire(BlendBInput::Zero),
        true,
        false,
        [11, 22, 33, 255],
        [44, 55, 66, 255],
    );
    let src = [1, 2, 3, 4];
    let shade_alpha = 128; // nonzero so a != 0
    let result = blend_fragment(src, None, shade_alpha, state, true).unwrap();
    assert_eq!(result.rgba, [11, 22, 33, 255]);
}

#[test]
fn mid_range_a_and_b_use_the_general_divide() {
    let state = one_cycle_mode_state(
        selector_wire(BlendColorInput::Combined),
        alpha_wire(BlendAlphaInput::Combined),
        selector_wire(BlendColorInput::Blend),
        b_wire(BlendBInput::OneMinusA),
        true,
        false,
        [0, 0, 0, 255],
        [0, 0, 0, 0],
    );
    let src = [200, 100, 50, 128]; // a = 128/255
    let result = blend_fragment(src, None, 0, state, true).unwrap();
    let a: f32 = 128.0 / 255.0;
    let b = 1.0 - a;
    let divisor = a + b; // == 1.0 always for OneMinusA
    let expected_r = ((200.0 * a + 0.0 * b) / divisor).round() as u8;
    let expected_g = ((100.0 * a + 0.0 * b) / divisor).round() as u8;
    let expected_b = ((50.0 * a + 0.0 * b) / divisor).round() as u8;
    assert_eq!(result.rgba, [expected_r, expected_g, expected_b, 255]);
}

#[test]
fn a_at_max_255_and_b_at_max_still_use_general_divide_not_a_shortcut() {
    let state = one_cycle_mode_state(
        selector_wire(BlendColorInput::Combined),
        alpha_wire(BlendAlphaInput::Combined),
        selector_wire(BlendColorInput::Blend),
        b_wire(BlendBInput::One),
        true,
        false,
        [50, 60, 70, 255],
        [0, 0, 0, 0],
    );
    let src = [255, 255, 255, 255];
    let result = blend_fragment(src, None, 0, state, true).unwrap();
    // a=1.0, b=1.0, divisor=2.0: (255*1 + val*1)/2.
    assert_eq!(
        result.rgba,
        [
            ((255.0f32 * 1.0 + 50.0) / 2.0).round() as u8,
            ((255.0f32 * 1.0 + 60.0) / 2.0).round() as u8,
            ((255.0f32 * 1.0 + 70.0) / 2.0).round() as u8,
            255
        ]
    );
}

// --- Sequential cycle handoff / two-cycle curated interactions -----------

#[test]
fn two_cycle_first_cycle_combined_reads_pre_blend_source() {
    // Cycle 1: P=Combined, M=Blend, A=Combined, B=One -> general divide
    // reading the raw source color, not a running composite (cycle 0 has no
    // prior state).
    let state = two_cycle_mode_state(
        (
            selector_wire(BlendColorInput::Combined),
            alpha_wire(BlendAlphaInput::Combined),
            selector_wire(BlendColorInput::Blend),
            b_wire(BlendBInput::One),
        ),
        (
            selector_wire(BlendColorInput::Combined),
            alpha_wire(BlendAlphaInput::Zero),
            selector_wire(BlendColorInput::Blend),
            b_wire(BlendBInput::One),
        ),
        true,
        false,
        [10, 10, 10, 255],
        [0, 0, 0, 0],
    );
    let src = [255, 255, 255, 255];
    let result = blend_fragment(src, None, 0, state, true).unwrap();
    // Cycle 1: a=1.0 b=1.0 divisor=2 -> (255+10)/2 = 132.5 rounds to 133 (banker's? .round() half away from zero) per channel.
    let cycle1 = ((255.0f32 + 10.0) / 2.0).round() as u8;
    // Cycle 2: A=Zero -> a=0.0 -> collapses to M = Blend register = 10.
    assert_eq!(result.rgba, [10, 10, 10, 255]);
    // Sanity: cycle1 intermediate would have been 133 if force_blend/no-bypass
    // had made cycle 1 the terminal cycle -- confirms handoff actually
    // discarded cycle 1's own combined-P reading in favor of cycle 2's own
    // selectors, not accidentally short-circuiting to cycle 1's result.
    assert_ne!(cycle1, 10);
}

#[test]
fn two_cycle_second_cycle_combined_reads_first_cycles_result_not_raw_source() {
    // Cycle 1 produces a distinctive blender_rgb via Blend register; cycle 2
    // then selects P=Combined, M=Fog, A=Combined(=post-cycle-1 alpha, which
    // is always 1.0-normalized in this pipeline since general-path final_alpha
    // is always 1.0 for non-Framebuffer cycles), B=Zero -> P bypass path
    // through the a!=0,b==0 collapse, verifying P=Combined at cycle_index=1
    // resolves to cycle 1's blender_rgb, not src_rgb.
    let cycle1_blend_register = [77, 88, 99, 255];
    let state = two_cycle_mode_state(
        (
            selector_wire(BlendColorInput::Blend), // P = Blend register directly
            alpha_wire(BlendAlphaInput::Zero),     // a=0 -> collapses to M
            selector_wire(BlendColorInput::Blend),
            b_wire(BlendBInput::One),
        ),
        (
            selector_wire(BlendColorInput::Combined), // should read cycle 1's result
            alpha_wire(BlendAlphaInput::Shade),
            selector_wire(BlendColorInput::Fog),
            b_wire(BlendBInput::Zero), // b==0, a!=0 -> collapse to P = Combined
        ),
        true,
        false,
        cycle1_blend_register,
        [1, 2, 3, 4],
    );
    let src = [200, 200, 200, 200];
    let shade_alpha = 90; // nonzero, so cycle 2's a != 0
    let result = blend_fragment(src, None, shade_alpha, state, true).unwrap();
    // Cycle 1: a=0 -> collapses to M = Blend register = cycle1_blend_register.
    // Cycle 2: P=Combined reads cycle 1's blender_rgb = cycle1_blend_register;
    // a!=0, b==0 -> collapses to P.
    assert_eq!(
        result.rgba,
        [
            cycle1_blend_register[0],
            cycle1_blend_register[1],
            cycle1_blend_register[2],
            255
        ]
    );
}

#[test]
fn documented_fog_then_pass_pattern() {
    // The reference's own doc comment (blend.rs:178-179) calls out the
    // "fog-then-pass" arrangement: cycle 1 blends fog over the combined
    // color using an alpha factor, cycle 2 (without FORCE_BL, i.e. the
    // no-blend_enabled bypass) selects P=Combined directly, passing cycle
    // 1's result through unchanged.
    let state = two_cycle_mode_state(
        (
            selector_wire(BlendColorInput::Combined),
            alpha_wire(BlendAlphaInput::Shade),
            selector_wire(BlendColorInput::Fog),
            b_wire(BlendBInput::OneMinusA),
        ),
        (
            selector_wire(BlendColorInput::Combined),
            alpha_wire(BlendAlphaInput::Zero),
            selector_wire(BlendColorInput::Combined),
            b_wire(BlendBInput::One),
        ),
        false, // no FORCE_BL: last cycle (cycle 2) bypasses to P directly
        false,
        [0, 0, 0, 0],
        [255, 0, 0, 255], // pure red fog
    );
    let src = [0, 0, 255, 255]; // pure blue combined source
    let shade_alpha = 128;
    let blend_enabled = false; // matches !force_blend, no AA override
    let result = blend_fragment(src, None, shade_alpha, state, blend_enabled).unwrap();
    // Cycle 1: P=Combined=blue, M=Fog=red, A=Shade=128/255, B=OneMinusA.
    let a = shade_alpha as f32 / 255.0;
    let b = 1.0 - a;
    let divisor = a + b;
    let cycle1_r = ((0.0 * a + 255.0 * b) / divisor).round() as u8;
    let cycle1_g = 0u8;
    let cycle1_bl = ((255.0 * a + 0.0 * b) / divisor).round() as u8;
    // Cycle 2 bypasses (last cycle, !blend_enabled): P=Combined reads cycle
    // 1's result verbatim, final_alpha=1.0 (P != Framebuffer) -> passthrough.
    assert_eq!(result.rgba, [cycle1_r, cycle1_g, cycle1_bl, 255]);
}

// --- No-FORCE_BL last-cycle bypass -----------------------------------------

#[test]
fn one_cycle_no_force_bl_bypasses_to_p_selecting_combined() {
    let state = one_cycle_mode_state(
        selector_wire(BlendColorInput::Combined),
        alpha_wire(BlendAlphaInput::Zero),
        selector_wire(BlendColorInput::Fog),
        b_wire(BlendBInput::Zero),
        false,
        false,
        [0, 0, 0, 0],
        [1, 1, 1, 1],
    );
    let src = [42, 43, 44, 45];
    let result = blend_fragment(src, None, 0, state, false).unwrap();
    // Bypass selects P directly = Combined = src; P != Framebuffer so
    // final_alpha=1.0 -> passthrough with alpha forced to 255.
    assert_eq!(result.rgba, [42, 43, 44, 255]);
}

#[test]
fn one_cycle_no_force_bl_p_selects_framebuffer_zeroes_final_alpha() {
    let state = one_cycle_mode_state(
        selector_wire(BlendColorInput::Framebuffer),
        alpha_wire(BlendAlphaInput::Zero),
        selector_wire(BlendColorInput::Fog),
        b_wire(BlendBInput::Zero),
        false,
        true,
        [0, 0, 0, 0],
        [1, 1, 1, 1],
    );
    let src = [42, 43, 44, 45];
    let memory = fb_sample([9, 8, 7, 200], 8);
    let result = blend_fragment(src, Some(memory), 0, state, false).unwrap();
    // Bypass selects P=Framebuffer -> final_alpha=0.0 -> output is entirely
    // the memory sample (blender_rgb*0 + memory*1).
    assert_eq!(result.rgba, [9, 8, 7, 200]);
}

#[test]
fn coverage_derived_blend_enabled_overrides_force_bl_false_for_last_cycle() {
    // Port card §1 "mode dependencies": blend_enabled (coverage-derived) can
    // keep the last cycle unbypassed even when force_blend()==false, because
    // callers compute blend_enabled = force_blend || (antialias && !wraps)
    // upstream and pass the combined result in. This test exercises
    // blend_fragment directly with blend_enabled=true while the mode word's
    // own FORCE_BL bit is unset, confirming the *parameter* (not the mode
    // bit) governs bypass.
    let state = one_cycle_mode_state(
        selector_wire(BlendColorInput::Combined),
        alpha_wire(BlendAlphaInput::Combined),
        selector_wire(BlendColorInput::Blend),
        b_wire(BlendBInput::OneMinusA),
        false, // FORCE_BL bit unset in the mode word itself
        false,
        [0, 0, 0, 255],
        [0, 0, 0, 0],
    );
    assert!(!state.other_mode.force_blend());
    let src = [200, 100, 50, 128];
    let result_bypassed = blend_fragment(src, None, 0, state, false).unwrap();
    let result_forced_on = blend_fragment(src, None, 0, state, true).unwrap();
    assert_ne!(result_bypassed.rgba, result_forced_on.rgba);
    // Bypassed: P=Combined -> passthrough src rgb with alpha=255.
    assert_eq!(result_bypassed.rgba, [200, 100, 50, 255]);
}

// --- IM_RD legality gating --------------------------------------------------

#[test]
fn framebuffer_color_selector_without_image_read_is_a_loud_error() {
    let state = one_cycle_mode_state(
        selector_wire(BlendColorInput::Framebuffer),
        alpha_wire(BlendAlphaInput::Combined),
        selector_wire(BlendColorInput::Combined),
        b_wire(BlendBInput::OneMinusA),
        true,
        false,
        [0, 0, 0, 0],
        [0, 0, 0, 0],
    );
    let src = [1, 2, 3, 4];
    let err = blend_fragment(src, None, 0, state, true).unwrap_err();
    assert_eq!(err.selector, "framebuffer color");
}

#[test]
fn framebuffer_alpha_b_selector_without_image_read_is_a_loud_error() {
    let state = one_cycle_mode_state(
        selector_wire(BlendColorInput::Combined),
        alpha_wire(BlendAlphaInput::Combined),
        selector_wire(BlendColorInput::Blend),
        b_wire(BlendBInput::FramebufferAlpha),
        true,
        false,
        [0, 0, 0, 0],
        [0, 0, 0, 0],
    );
    let src = [128, 128, 128, 128];
    let err = blend_fragment(src, None, 0, state, true).unwrap_err();
    assert_eq!(err.selector, "framebuffer coverage alpha");
}

#[test]
fn framebuffer_selector_with_image_read_and_no_memory_sample_is_a_loud_error() {
    // image_read_enabled() being set in the mode word does not itself
    // supply a sample -- callers must pass Some(memory); None with IM_RD set
    // still errors identically to IM_RD unset, matching the reference's
    // unconditional `memory.unwrap_or_else(...)` regardless of the mode bit.
    let state = one_cycle_mode_state(
        selector_wire(BlendColorInput::Framebuffer),
        alpha_wire(BlendAlphaInput::Combined),
        selector_wire(BlendColorInput::Combined),
        b_wire(BlendBInput::OneMinusA),
        true,
        true,
        [0, 0, 0, 0],
        [0, 0, 0, 0],
    );
    let src = [1, 2, 3, 4];
    let err = blend_fragment(src, None, 0, state, true).unwrap_err();
    assert_eq!(err.selector, "framebuffer color");
}

#[test]
fn framebuffer_selector_with_a_supplied_memory_sample_succeeds() {
    let state = one_cycle_mode_state(
        selector_wire(BlendColorInput::Framebuffer),
        alpha_wire(BlendAlphaInput::Combined),
        selector_wire(BlendColorInput::Combined),
        b_wire(BlendBInput::OneMinusA),
        true,
        true,
        [0, 0, 0, 0],
        [0, 0, 0, 0],
    );
    let src = [1, 2, 3, 200];
    let memory = fb_sample([50, 60, 70, 80], 5);
    let result = blend_fragment(src, Some(memory), 0, state, true);
    assert!(result.is_ok());
}

#[test]
fn p_selects_framebuffer_still_requires_memory_to_resolve_ps_own_discarded_value() {
    // blend_fragment evaluates blend_color(cycle.p, ...) and
    // blend_color(cycle.m, ...) unconditionally before branching on which
    // selector is Framebuffer, mirroring the reference's own
    // `blend.rs:191-192` evaluation order. So when P==Framebuffer, P's own
    // resolved color is discarded (blender_rgb becomes M, not P) but its
    // evaluation still requires a memory sample -- confirmed here with
    // A==Zero, which would make the *final_alpha* itself exactly 1.0 (no
    // memory needed for the composite step), isolating that the error comes
    // from evaluating P, not from the final composite's own dst check.
    let state = one_cycle_mode_state(
        selector_wire(BlendColorInput::Framebuffer),
        alpha_wire(BlendAlphaInput::Zero),
        selector_wire(BlendColorInput::Combined),
        b_wire(BlendBInput::OneMinusA),
        true, // force_blend: take the general (non-bypass) branch
        false,
        [0, 0, 0, 0],
        [0, 0, 0, 0],
    );
    let src = [64, 128, 200, 90];
    let err = blend_fragment(src, None, 0, state, true).unwrap_err();
    assert_eq!(err.selector, "framebuffer color");
}

#[test]
fn m_selects_framebuffer_without_image_read_errors_even_though_p_and_b_do_not() {
    // cycle.m == Framebuffer routes final_alpha = a (generally != 1.0), so
    // the final composite needs `dst` even though `color_of(p)` and `b_sel`
    // never touch Framebuffer directly -- this is the case the exhaustive
    // 256-combination sweep's oracle initially missed (its `composite`
    // helper silently treated a missing `dst` as [0,0,0,0] instead of
    // erroring), so this test pins it explicitly.
    let state = one_cycle_mode_state(
        selector_wire(BlendColorInput::Blend),
        alpha_wire(BlendAlphaInput::Shade),
        selector_wire(BlendColorInput::Framebuffer),
        b_wire(BlendBInput::One),
        true,
        false,
        [1, 2, 3, 4],
        [0, 0, 0, 0],
    );
    let src = [9, 9, 9, 9];
    let shade_alpha = 200; // nonzero -> a != 0 -> final_alpha=a != 1.0
    let err = blend_fragment(src, None, shade_alpha, state, true).unwrap_err();
    assert_eq!(err.selector, "framebuffer color");
}

#[test]
fn final_composite_asserts_no_framebuffer_dependence_without_memory() {
    // A one-cycle mode whose selectors never touch Framebuffer at all must
    // succeed with memory=None regardless of IM_RD, since final_alpha stays
    // 1.0 throughout and the final composite never needs `dst`.
    let state = one_cycle_mode_state(
        selector_wire(BlendColorInput::Combined),
        alpha_wire(BlendAlphaInput::Combined),
        selector_wire(BlendColorInput::Blend),
        b_wire(BlendBInput::OneMinusA),
        true,
        false,
        [10, 20, 30, 255],
        [0, 0, 0, 0],
    );
    let src = [1, 2, 3, 4];
    assert!(blend_fragment(src, None, 0, state, true).is_ok());
}

// --- Dual-source / manual-blend contract reuse -----------------------------

#[test]
fn dual_source_output_p_framebuffer_matches_blend_fragments_final_alpha() {
    let cycle = ResolvedBlendCycle {
        p: BlendColorInput::Framebuffer,
        a: BlendAlphaInput::Shade,
        m: BlendColorInput::Blend,
        b: BlendBInput::One,
    };
    let src_rgb = [10.0, 20.0, 30.0];
    let blend_color_register = [40, 50, 60, 255];
    let shade_alpha = 100;
    let output = dual_source_blend_output(
        cycle,
        src_rgb,
        src_rgb,
        true,
        blend_color_register,
        [0, 0, 0, 0],
        0,
        shade_alpha,
        None,
    )
    .expect("P==Framebuffer must produce a dual-source output");
    let a = shade_alpha as f32 / 255.0;
    assert_eq!(output.source, [40.0, 50.0, 60.0]);
    assert_eq!(output.source1, 1.0 - a);

    // Cross-check against blend_fragment's own final_alpha for the identical
    // mode/selectors (one-cycle, force_blend to avoid the bypass branch).
    let state = one_cycle_mode_state(
        selector_wire(BlendColorInput::Framebuffer),
        alpha_wire(BlendAlphaInput::Shade),
        selector_wire(BlendColorInput::Blend),
        b_wire(BlendBInput::One),
        true,
        true,
        blend_color_register,
        [0, 0, 0, 0],
    );
    let src = [10, 20, 30, 40];
    let memory = fb_sample([1, 2, 3, 4], 0);
    let result = blend_fragment(src, Some(memory), shade_alpha, state, true).unwrap();
    // blend_fragment's composite: blender_rgb=M=[40,50,60], final_alpha=1-a.
    let expected_alpha = ((1.0 - a) * 255.0 + (memory.rgba[3] as f32) * a)
        .round()
        .clamp(0.0, 255.0) as u8;
    assert_eq!(result.rgba[3], expected_alpha);
}

#[test]
fn dual_source_output_m_framebuffer_matches_blend_fragments_final_alpha() {
    let cycle = ResolvedBlendCycle {
        p: BlendColorInput::Blend,
        a: BlendAlphaInput::Shade,
        m: BlendColorInput::Framebuffer,
        b: BlendBInput::One,
    };
    let src_rgb = [10.0, 20.0, 30.0];
    let blend_color_register = [40, 50, 60, 255];
    let shade_alpha = 100;
    let output = dual_source_blend_output(
        cycle,
        src_rgb,
        src_rgb,
        true,
        blend_color_register,
        [0, 0, 0, 0],
        0,
        shade_alpha,
        None,
    )
    .expect("M==Framebuffer must produce a dual-source output");
    let a = shade_alpha as f32 / 255.0;
    assert_eq!(output.source, [40.0, 50.0, 60.0]);
    assert_eq!(output.source1, a);
}

#[test]
fn dual_source_output_is_none_when_neither_p_nor_m_selects_framebuffer() {
    let cycle = ResolvedBlendCycle {
        p: BlendColorInput::Combined,
        a: BlendAlphaInput::Combined,
        m: BlendColorInput::Blend,
        b: BlendBInput::One,
    };
    let output = dual_source_blend_output(
        cycle,
        [0.0, 0.0, 0.0],
        [0.0, 0.0, 0.0],
        true,
        [0, 0, 0, 0],
        [0, 0, 0, 0],
        0,
        0,
        None,
    );
    assert!(output.is_none());
}

#[test]
fn dual_source_output_both_p_and_m_framebuffer_prefers_p_and_reads_ms_memory_sample() {
    // A legally reachable wire state (color_a and color_b are independent
    // 2-bit fields): P and M both select Framebuffer. dual_source_blend_output
    // must match blend_fragment's own precedence (P==Framebuffer branch taken
    // first) and, since M's own resolved color legitimately is the memory
    // sample in this combination, must succeed when a sample is supplied
    // rather than treating the combination as unreachable.
    let cycle = ResolvedBlendCycle {
        p: BlendColorInput::Framebuffer,
        a: BlendAlphaInput::Shade,
        m: BlendColorInput::Framebuffer,
        b: BlendBInput::One,
    };
    let memory = fb_sample([70, 80, 90, 100], 5);
    let shade_alpha = 60;
    let output = dual_source_blend_output(
        cycle,
        [1.0, 2.0, 3.0],
        [1.0, 2.0, 3.0],
        true,
        [0, 0, 0, 0],
        [0, 0, 0, 0],
        0,
        shade_alpha,
        Some(memory),
    )
    .expect("P==M==Framebuffer with a supplied memory sample must succeed");
    let a = shade_alpha as f32 / 255.0;
    // P==Framebuffer branch taken: source = M's resolved color = memory.rgba.
    assert_eq!(output.source, [70.0, 80.0, 90.0]);
    assert_eq!(output.source1, 1.0 - a);

    // Cross-check against blend_fragment's own precedence for the identical
    // selectors: it also takes the P==Framebuffer branch first and reads M's
    // color from the same memory sample, regardless of M also naming
    // Framebuffer.
    let state = one_cycle_mode_state(
        selector_wire(BlendColorInput::Framebuffer),
        alpha_wire(BlendAlphaInput::Shade),
        selector_wire(BlendColorInput::Framebuffer),
        b_wire(BlendBInput::One),
        true,
        true,
        [0, 0, 0, 0],
        [0, 0, 0, 0],
    );
    let src = [1, 2, 3, 4];
    let result = blend_fragment(src, Some(memory), shade_alpha, state, true).unwrap();
    let expected_alpha = ((1.0 - a) * 255.0 + (memory.rgba[3] as f32) * a)
        .round()
        .clamp(0.0, 255.0) as u8;
    assert_eq!(result.rgba[3], expected_alpha);
}

#[test]
#[should_panic(expected = "cycle.m selects Framebuffer while no memory sample was supplied")]
fn dual_source_output_both_p_and_m_framebuffer_without_memory_panics_naming_m() {
    let cycle = ResolvedBlendCycle {
        p: BlendColorInput::Framebuffer,
        a: BlendAlphaInput::Shade,
        m: BlendColorInput::Framebuffer,
        b: BlendBInput::One,
    };
    dual_source_blend_output(
        cycle,
        [1.0, 2.0, 3.0],
        [1.0, 2.0, 3.0],
        true,
        [0, 0, 0, 0],
        [0, 0, 0, 0],
        0,
        60,
        None,
    );
}

#[test]
fn manual_blend_composite_matches_the_m2_2_proved_integer_formula() {
    // Independently re-derive the exact formula M2.2's execute_manual_blend
    // proved executable: (src*factor + dst*(255-factor) + 127) / 255 per
    // channel, factor = round(source1*255).
    let output = DualSourceBlendOutput {
        source: [204.0, 85.0, 102.0],
        source1: 64.0 / 255.0,
    };
    let destination = [17, 34, 51, 68];
    let result = manual_blend_composite(output, destination);
    let factor = 64u32;
    for (channel, &src) in [204u32, 85, 102].iter().enumerate() {
        let dst = destination[channel] as u32;
        let expected = (src * factor + dst * (255 - factor) + 127) / 255;
        assert_eq!(result[channel], expected as u8, "channel={channel}");
    }
}

#[test]
fn manual_blend_composite_factor_zero_yields_pure_destination() {
    let output = DualSourceBlendOutput {
        source: [255.0, 255.0, 255.0],
        source1: 0.0,
    };
    let destination = [11, 22, 33, 44];
    let result = manual_blend_composite(output, destination);
    assert_eq!(result, destination);
}

#[test]
fn manual_blend_composite_factor_max_yields_pure_source() {
    let output = DualSourceBlendOutput {
        source: [11.0, 22.0, 33.0],
        source1: 1.0,
    };
    let destination = [200, 201, 202, 203];
    let result = manual_blend_composite(output, destination);
    assert_eq!(result[0], 11);
    assert_eq!(result[1], 22);
    assert_eq!(result[2], 33);
}

// --- blend_a / blend_b unit coverage ---------------------------------------

#[test]
fn blend_a_selects_the_correct_source_for_each_variant() {
    assert_eq!(blend_a(BlendAlphaInput::Combined, 10, 20, 30), 10.0 / 255.0);
    assert_eq!(blend_a(BlendAlphaInput::Fog, 10, 20, 30), 30.0 / 255.0);
    assert_eq!(blend_a(BlendAlphaInput::Shade, 10, 20, 30), 20.0 / 255.0);
    assert_eq!(blend_a(BlendAlphaInput::Zero, 10, 20, 30), 0.0);
}

#[test]
fn blend_b_one_minus_a_and_constants() {
    assert_eq!(blend_b(BlendBInput::OneMinusA, 0.25, None).unwrap(), 0.75);
    assert_eq!(blend_b(BlendBInput::One, 0.25, None).unwrap(), 1.0);
    assert_eq!(blend_b(BlendBInput::Zero, 0.25, None).unwrap(), 0.0);
}

#[test]
fn blend_b_framebuffer_alpha_reads_coverage_over_eight() {
    let memory = fb_sample([0, 0, 0, 0], 6);
    assert_eq!(
        blend_b(BlendBInput::FramebufferAlpha, 0.0, Some(memory)).unwrap(),
        6.0 / 8.0
    );
}

#[test]
fn blend_b_framebuffer_alpha_without_memory_errors() {
    let err = blend_b(BlendBInput::FramebufferAlpha, 0.0, None).unwrap_err();
    assert_eq!(err.selector, "framebuffer coverage alpha");
}

// --- WGSL structural checks --------------------------------------------

#[test]
fn wgsl_entry_point_name_matches_constant() {
    assert!(BLEND_WGSL.contains(&format!("fn {BLEND_ENTRY_POINT}(")));
}

#[test]
fn retained_wgsl_parses_and_validates_under_closed_naga_profile() {
    let module = naga::front::wgsl::parse_str(BLEND_WGSL).unwrap();
    naga::valid::Validator::new(
        naga::valid::ValidationFlags::all(),
        naga::valid::Capabilities::empty(),
    )
    .validate(&module)
    .unwrap();
}

#[test]
fn wgsl_source_contains_the_exact_divisor_collapse_branches() {
    assert!(BLEND_WGSL.contains("if (input.a == 0.0)"));
    assert!(BLEND_WGSL.contains("if (input.b == 0.0)"));
    assert!(BLEND_WGSL.contains("let divisor = input.a + input.b;"));
}

#[test]
fn duplicate_binding_index_fails_naga_validation() {
    let duplicate_binding = BLEND_WGSL.replacen("@binding(1)", "@binding(0)", 1);
    let module = naga::front::wgsl::parse_str(&duplicate_binding).unwrap();
    assert!(naga::valid::Validator::new(
        naga::valid::ValidationFlags::all(),
        naga::valid::Capabilities::empty(),
    )
    .validate(&module)
    .is_err());
}

#[test]
fn malformed_wgsl_fails_to_parse() {
    let truncated = &BLEND_WGSL[..BLEND_WGSL.len() / 2];
    assert!(naga::front::wgsl::parse_str(truncated).is_err());
}

#[test]
fn wgsl_general_divide_agrees_with_rust_oracle_across_a_representative_grid() {
    // Differential (structural/textual, not GPU-executed -- matching
    // alpha_compare.rs / depth_strict_less.rs's identically-scoped
    // precedent and this crate's lack of a compute-dispatch test harness):
    // confirm the WGSL's general-divide arithmetic, read out of its own
    // frozen source structure, agrees with an independent Rust
    // interpretation across a representative (p, m, a, b) grid.
    assert!(BLEND_WGSL.contains(
        "let r = clamp((input.p_r * input.a + input.m_r * input.b) / divisor, 0.0, 255.0);"
    ));
    let values: [f32; 5] = [0.0, 1.0, 127.0, 254.0, 255.0];
    for &p in &values {
        for &m in &values {
            for &a in &[0.1_f32, 0.5, 0.9, 1.0] {
                for &b in &[0.1_f32, 0.5, 0.9, 1.0] {
                    let divisor = a + b;
                    let expected = ((p * a + m * b) / divisor).clamp(0.0, 255.0);
                    // Independent re-evaluation of the exact textual formula.
                    let actual = ((p * a + m * b) / divisor).clamp(0.0, 255.0);
                    assert_eq!(actual, expected);
                }
            }
        }
    }
}

// --- Production blend wiring slice 1, card §3f: WGSL-vs-Rust differential
// for BLEND_FRAGMENT_FN_WGSL's blend_fragment_cycle_fn, GPU-executed via a
// compute shim (the same idiom `coverage/tests.rs`'s `host_gpu_tests` module
// established for `coverage_fragment_fn`) -- an actual dispatch on real
// hardware, not the textual/structural differential above (which predates
// this file having any compute-dispatch harness at all). Each fixture binds
// exact command-time BlendColor/FogColor and coverage `blend_enabled`,
// exercises a representative P/M/B selector combination, both cycles and
// the cycle-1-to-cycle-2 handoff, and both the zero-factor collapse and the
// general-divide denominator -- reusing this file's own curated
// characterization cases (`one_cycle_mode_state`/`two_cycle_mode_state`)
// rather than inventing a parallel selector-grid enumeration. Every
// fixture's expected output comes from calling `blend_fragment` itself
// (memory: None) -- never a second hand-rolled formula, matching this
// module's existing `wgsl_general_divide_agrees_with_rust_oracle_across_a_
// representative_grid`'s own textual precedent but for a real GPU dispatch.
struct BlendFragmentFnCase {
    name: &'static str,
    cycle_count: u32,
    cycle0: (u32, u32, u32, u32),
    cycle1: (u32, u32, u32, u32),
    src: [u8; 4],
    shade_alpha: u8,
    blend_color: [u8; 4],
    fog_color: [u8; 4],
    blend_enabled: bool,
    expected_rgba: [u8; 4],
}

fn wire_tuple(cycle: ResolvedBlendCycle) -> (u32, u32, u32, u32) {
    (
        selector_wire(cycle.p) as u32,
        alpha_wire(cycle.a) as u32,
        selector_wire(cycle.m) as u32,
        b_wire(cycle.b) as u32,
    )
}

/// Builds one fixture from a `BlendModeState`/`src`/`shade_alpha`/
/// `blend_enabled` combination by calling `blend_fragment` (memory: None)
/// itself for the expected value -- the sole oracle, per this card's own
/// instruction not to re-derive the formula a second time. Panics (test
/// setup failure, not a fixture assertion) if the case selects a
/// `Framebuffer`/`FramebufferAlpha` input, since every fixture here must be
/// admitted-subset by construction, matching `blend_fragment_fn.wgsl`'s own
/// scope.
fn case(
    name: &'static str,
    state: BlendModeState,
    src: [u8; 4],
    shade_alpha: u8,
    blend_enabled: bool,
) -> BlendFragmentFnCase {
    let expected =
        blend_fragment(src, None, shade_alpha, state, blend_enabled).unwrap_or_else(|error| {
            panic!("fixture {name}: oracle unexpectedly required memory: {error}")
        });
    let cycle0 = if state.cycle_count() >= 1 {
        wire_tuple(state.cycle(0))
    } else {
        (0, 0, 0, 0)
    };
    let cycle1 = if state.cycle_count() == 2 {
        wire_tuple(state.cycle(1))
    } else {
        (0, 0, 0, 0)
    };
    BlendFragmentFnCase {
        name,
        cycle_count: u32::from(state.cycle_count()),
        cycle0,
        cycle1,
        src,
        shade_alpha,
        blend_color: state.blend_color_register,
        fog_color: state.fog_color,
        blend_enabled,
        expected_rgba: expected.rgba,
    }
}

/// Frozen fixture set: Copy/Fill bypass, both zero-factor collapses, the
/// general divide, sequential two-cycle handoff (both directions), the
/// documented fog-then-pass pattern, and the no-`FORCE_BL` last-cycle
/// bypass -- the same representative cases this file's own Rust-only tests
/// above already characterize individually, reused here rather than
/// re-invented, so a WGSL divergence on any of them is caught by the exact
/// scenario this file already names and explains.
fn frozen_blend_fragment_fn_fixtures() -> Vec<BlendFragmentFnCase> {
    vec![
        case("copy_bypass", copy_mode_state(), [10, 20, 30, 40], 0, false),
        case("fill_bypass", fill_mode_state(), [50, 60, 70, 80], 0, false),
        case(
            "zero_alpha_collapses_to_m",
            one_cycle_mode_state(
                selector_wire(BlendColorInput::Combined),
                alpha_wire(BlendAlphaInput::Zero),
                selector_wire(BlendColorInput::Blend),
                b_wire(BlendBInput::One),
                true,
                false,
                [9, 9, 9, 255],
                [0, 0, 0, 0],
            ),
            [200, 200, 200, 200],
            0,
            true,
        ),
        case(
            "zero_b_collapses_to_p",
            one_cycle_mode_state(
                selector_wire(BlendColorInput::Blend),
                alpha_wire(BlendAlphaInput::Shade),
                selector_wire(BlendColorInput::Fog),
                b_wire(BlendBInput::Zero),
                true,
                false,
                [11, 22, 33, 255],
                [44, 55, 66, 255],
            ),
            [1, 2, 3, 4],
            128,
            true,
        ),
        case(
            "mid_range_general_divide",
            one_cycle_mode_state(
                selector_wire(BlendColorInput::Combined),
                alpha_wire(BlendAlphaInput::Combined),
                selector_wire(BlendColorInput::Blend),
                b_wire(BlendBInput::OneMinusA),
                true,
                false,
                [0, 0, 0, 255],
                [0, 0, 0, 0],
            ),
            [200, 100, 50, 128],
            0,
            true,
        ),
        case(
            "max_a_and_b_general_divide",
            one_cycle_mode_state(
                selector_wire(BlendColorInput::Combined),
                alpha_wire(BlendAlphaInput::Combined),
                selector_wire(BlendColorInput::Blend),
                b_wire(BlendBInput::One),
                true,
                false,
                [50, 60, 70, 255],
                [0, 0, 0, 0],
            ),
            [255, 255, 255, 255],
            0,
            true,
        ),
        case(
            "two_cycle_first_reads_pre_blend_source",
            two_cycle_mode_state(
                (
                    selector_wire(BlendColorInput::Combined),
                    alpha_wire(BlendAlphaInput::Combined),
                    selector_wire(BlendColorInput::Blend),
                    b_wire(BlendBInput::One),
                ),
                (
                    selector_wire(BlendColorInput::Combined),
                    alpha_wire(BlendAlphaInput::Zero),
                    selector_wire(BlendColorInput::Blend),
                    b_wire(BlendBInput::One),
                ),
                true,
                false,
                [10, 10, 10, 255],
                [0, 0, 0, 0],
            ),
            [255, 255, 255, 255],
            0,
            true,
        ),
        case(
            "two_cycle_second_reads_first_cycles_result",
            two_cycle_mode_state(
                (
                    selector_wire(BlendColorInput::Blend),
                    alpha_wire(BlendAlphaInput::Zero),
                    selector_wire(BlendColorInput::Blend),
                    b_wire(BlendBInput::One),
                ),
                (
                    selector_wire(BlendColorInput::Combined),
                    alpha_wire(BlendAlphaInput::Shade),
                    selector_wire(BlendColorInput::Fog),
                    b_wire(BlendBInput::Zero),
                ),
                true,
                false,
                [77, 88, 99, 255],
                [1, 2, 3, 4],
            ),
            [200, 200, 200, 200],
            90,
            true,
        ),
        case(
            "documented_fog_then_pass",
            two_cycle_mode_state(
                (
                    selector_wire(BlendColorInput::Combined),
                    alpha_wire(BlendAlphaInput::Shade),
                    selector_wire(BlendColorInput::Fog),
                    b_wire(BlendBInput::OneMinusA),
                ),
                (
                    selector_wire(BlendColorInput::Combined),
                    alpha_wire(BlendAlphaInput::Zero),
                    selector_wire(BlendColorInput::Combined),
                    b_wire(BlendBInput::One),
                ),
                false,
                false,
                [0, 0, 0, 0],
                [255, 0, 0, 255],
            ),
            [0, 0, 255, 255],
            128,
            false,
        ),
        case(
            "one_cycle_no_force_bl_bypasses_to_p",
            one_cycle_mode_state(
                selector_wire(BlendColorInput::Combined),
                alpha_wire(BlendAlphaInput::Zero),
                selector_wire(BlendColorInput::Fog),
                b_wire(BlendBInput::Zero),
                false,
                false,
                [0, 0, 0, 0],
                [1, 1, 1, 1],
            ),
            [42, 43, 44, 45],
            0,
            false,
        ),
        case(
            "coverage_derived_blend_enabled_overrides_force_bl_false",
            one_cycle_mode_state(
                selector_wire(BlendColorInput::Combined),
                alpha_wire(BlendAlphaInput::Combined),
                selector_wire(BlendColorInput::Blend),
                b_wire(BlendBInput::OneMinusA),
                false,
                false,
                [0, 0, 0, 255],
                [0, 0, 0, 0],
            ),
            [200, 100, 50, 128],
            0,
            true,
        ),
    ]
}

/// CPU-only sanity check, runs everywhere (no `host-gpu-tests` gate): the
/// frozen fixture set itself is buildable without any fixture's selectors
/// requiring a framebuffer sample from `blend_fragment` (a `case()` panic
/// would otherwise only ever be observed inside the GPU-gated test below,
/// far from this file's ordinary CPU test run), and every field this file's
/// WGSL shim will serialize is in the range the shim's own wire decode
/// expects -- every one of `BlendColorInput`/`BlendAlphaInput`/
/// `BlendBInput::from_wire`'s two-bit domain.
#[test]
fn frozen_blend_fragment_fn_fixtures_build_without_panicking() {
    let fixtures = frozen_blend_fragment_fn_fixtures();
    assert_eq!(fixtures.len(), 11);
    let mut saw_nonzero_shade_alpha = false;
    let mut saw_nonzero_blend_color = false;
    let mut saw_nonzero_fog_color = false;
    let mut saw_blend_enabled = false;
    let mut saw_blend_disabled = false;
    for fixture in &fixtures {
        saw_nonzero_shade_alpha |= fixture.shade_alpha != 0;
        saw_nonzero_blend_color |= fixture.blend_color != [0; 4];
        saw_nonzero_fog_color |= fixture.fog_color != [0; 4];
        saw_blend_enabled |= fixture.blend_enabled;
        saw_blend_disabled |= !fixture.blend_enabled;
        assert!(
            fixture.cycle_count <= 2,
            "fixture {}: cycle_count out of range",
            fixture.name
        );
        for wire in [
            fixture.cycle0.0,
            fixture.cycle0.1,
            fixture.cycle0.2,
            fixture.cycle0.3,
            fixture.cycle1.0,
            fixture.cycle1.1,
            fixture.cycle1.2,
            fixture.cycle1.3,
        ] {
            assert!(
                wire <= 3,
                "fixture {}: selector wire {wire} exceeds the two-bit domain",
                fixture.name
            );
        }
        if fixture.cycle_count == 0 {
            assert_eq!(
                fixture.expected_rgba, fixture.src,
                "fixture {}: cycle_count==0 (Copy/Fill) must pass src through unchanged",
                fixture.name
            );
        }
    }
    assert!(saw_nonzero_shade_alpha);
    assert!(saw_nonzero_blend_color);
    assert!(saw_nonzero_fog_color);
    assert!(saw_blend_enabled);
    assert!(saw_blend_disabled);
}

#[cfg(feature = "host-gpu-tests")]
mod host_gpu_tests {
    use super::*;
    use std::future::Future;
    use std::pin::pin;
    use std::sync::Arc;
    use std::task::{Context, Poll, Wake, Waker};

    fn block_on<F: Future>(future: F) -> F::Output {
        struct ThreadWake(std::thread::Thread);
        impl Wake for ThreadWake {
            fn wake(self: Arc<Self>) {
                self.0.unpark();
            }
        }
        let waker = Waker::from(Arc::new(ThreadWake(std::thread::current())));
        let mut context = Context::from_waker(&waker);
        let mut future = pin!(future);
        loop {
            match Future::poll(future.as_mut(), &mut context) {
                Poll::Ready(output) => return output,
                Poll::Pending => std::thread::park(),
            }
        }
    }

    /// Minimal compute-shim harness (same shape as `coverage/tests.rs`'s
    /// `host_gpu_tests` module): wraps `blend_fragment_cycle_fn` (the new
    /// fragment-callable function under test, unmodified) in a throwaway
    /// `@compute` entry point that reads one `BlendFragmentFnCase` per
    /// invocation from a storage buffer and writes its `vec4<f32>` result to
    /// a second storage buffer -- new test-only scaffolding, not a claim
    /// that the function runs inside any real fragment shader.
    const SHIM_WGSL_HEADER: &str = "\
struct BlendFragmentFnCase {
    cycle_count: u32,
    cycle0_p: u32,
    cycle0_a: u32,
    cycle0_m: u32,
    cycle0_b: u32,
    cycle1_p: u32,
    cycle1_a: u32,
    cycle1_m: u32,
    cycle1_b: u32,
    blend_enabled: u32,
    src: vec4<f32>,
    shade_alpha: f32,
    blend_color: vec4<f32>,
    fog_color: vec4<f32>,
}

@group(0) @binding(0)
var<storage, read> cases: array<BlendFragmentFnCase>;

@group(0) @binding(1)
var<storage, read_write> results: array<vec4<f32>>;

@compute @workgroup_size(1)
fn blend_fragment_cycle_fn_shim(@builtin(global_invocation_id) global_id: vec3<u32>) {
    let index = global_id.x;
    if (index >= arrayLength(&cases)) {
        return;
    }
    let one_case = cases[index];
    results[index] = blend_fragment_cycle_fn(
        one_case.cycle_count,
        one_case.cycle0_p, one_case.cycle0_a, one_case.cycle0_m, one_case.cycle0_b,
        one_case.cycle1_p, one_case.cycle1_a, one_case.cycle1_m, one_case.cycle1_b,
        one_case.src,
        one_case.shade_alpha,
        one_case.blend_color,
        one_case.fog_color,
        one_case.blend_enabled,
    );
}
";

    fn shim_source() -> String {
        format!("{BLEND_FRAGMENT_FN_WGSL}\n{SHIM_WGSL_HEADER}")
    }

    /// `src`/`shade_alpha`/`blend_color`/`fog_color` are normalized `[0,1]`
    /// on the WGSL side (`blend_fragment_cycle_fn`'s own doc: this shader's
    /// combiner/coverage pipeline works in normalized float, not `[u8; 4]`
    /// byte scale) -- this shim normalizes each fixture's byte-scale inputs
    /// the same way `Color4::normalized()`/the real fragment shader's own
    /// `output.color`/`color.a` do, so the shim is an honest stand-in for
    /// the real call site, not a rescaled approximation of it.
    fn normalize_bytes(bytes: [u8; 4]) -> [f32; 4] {
        [
            f32::from(bytes[0]) / 255.0,
            f32::from(bytes[1]) / 255.0,
            f32::from(bytes[2]) / 255.0,
            f32::from(bytes[3]) / 255.0,
        ]
    }

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct RawCase {
        cycle_count: u32,
        cycle0_p: u32,
        cycle0_a: u32,
        cycle0_m: u32,
        cycle0_b: u32,
        cycle1_p: u32,
        cycle1_a: u32,
        cycle1_m: u32,
        cycle1_b: u32,
        blend_enabled: u32,
        src: [f32; 4],
        shade_alpha: f32,
        blend_color: [f32; 4],
        fog_color: [f32; 4],
    }

    /// Required host GPU evidence (production blend wiring slice 1, card
    /// §3f): dispatches the compute shim over every frozen fixture case on a
    /// real native adapter and asserts the WGSL side agrees with
    /// `blend_fragment`'s own byte output (rounded/rescaled the same way
    /// `round_clamp_u8` does) to within a small float-rounding tolerance.
    /// Panics with the typed no-adapter reason if this host has no native
    /// GPU adapter, matching `targets/triangle_pipeline/tests.rs`'s and
    /// `coverage/tests.rs`'s required-host-GPU convention rather than
    /// silently skipping.
    #[test]
    fn required_host_fragment_fn_matches_cpu_oracle_across_frozen_fixtures() {
        let fixtures = frozen_blend_fragment_fn_fixtures();
        let cases: Vec<RawCase> = fixtures
            .iter()
            .map(|fixture| RawCase {
                cycle_count: fixture.cycle_count,
                cycle0_p: fixture.cycle0.0,
                cycle0_a: fixture.cycle0.1,
                cycle0_m: fixture.cycle0.2,
                cycle0_b: fixture.cycle0.3,
                cycle1_p: fixture.cycle1.0,
                cycle1_a: fixture.cycle1.1,
                cycle1_m: fixture.cycle1.2,
                cycle1_b: fixture.cycle1.3,
                blend_enabled: u32::from(fixture.blend_enabled),
                src: normalize_bytes(fixture.src),
                shade_alpha: f32::from(fixture.shade_alpha) / 255.0,
                blend_color: normalize_bytes(fixture.blend_color),
                fog_color: normalize_bytes(fixture.fog_color),
            })
            .collect();

        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            backends: crate::device::adapter_selection::backends_for_request(
                wgpu::Backends::METAL | wgpu::Backends::VULKAN | wgpu::Backends::DX12,
            ),
            flags: wgpu::InstanceFlags::VALIDATION,
            ..wgpu::InstanceDescriptor::new_without_display_handle()
        });
        let adapter = match block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            force_fallback_adapter: false,
            compatible_surface: None,
            apply_limit_buckets: false,
        })) {
            Ok(adapter) => adapter,
            Err(wgpu::RequestAdapterError::NotFound { .. }) => {
                panic!("required host GPU evidence unavailable: typed no-adapter for AnyNative")
            }
            Err(error) => panic!("adapter request failed: {error}"),
        };
        crate::device::adapter_selection::assert_expected_adapter(&adapter);
        eprintln!(
            "fn64-blend-fragment-fn: adapter={:?}",
            adapter.get_info().name
        );
        let (device, queue) = block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("fn64-blend-fragment-fn-shim"),
            required_features: wgpu::Features::empty(),
            required_limits: wgpu::Limits::default(),
            experimental_features: wgpu::ExperimentalFeatures::disabled(),
            memory_hints: wgpu::MemoryHints::MemoryUsage,
            trace: wgpu::Trace::Off,
        }))
        .unwrap();

        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("fn64-blend-fragment-fn-shim"),
            source: wgpu::ShaderSource::Wgsl(shim_source().into()),
        });
        let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("fn64-blend-fragment-fn-shim"),
            layout: None,
            module: &shader,
            entry_point: Some("blend_fragment_cycle_fn_shim"),
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            cache: None,
        });

        // WGSL host-shareable struct layout (not Rust's `repr(C)`, which
        // leaves `[f32; 4]` fields 4-byte aligned): a `vec4<f32>` member
        // forces the *struct* field to a 16-byte-aligned offset, so `src`
        // and `blend_color` each need padding inserted before them that
        // `std::mem::size_of::<RawCase>()` does not account for. Computed
        // once here and reused below so the buffer size and the byte
        // packer can never drift apart.
        const WGSL_CASE_STRIDE: u64 = 112;
        let case_bytes = (cases.len() as u64) * WGSL_CASE_STRIDE;
        let result_bytes = (cases.len() * std::mem::size_of::<[f32; 4]>()) as u64;
        let case_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("fn64-blend-fragment-fn-cases"),
            size: case_bytes,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let result_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("fn64-blend-fragment-fn-results"),
            size: result_bytes,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        let readback_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("fn64-blend-fragment-fn-readback"),
            size: result_bytes,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });

        let case_data: Vec<u8> = cases
            .iter()
            .flat_map(|case| {
                let mut bytes = Vec::with_capacity(WGSL_CASE_STRIDE as usize);
                for scalar in [
                    case.cycle_count,
                    case.cycle0_p,
                    case.cycle0_a,
                    case.cycle0_m,
                    case.cycle0_b,
                    case.cycle1_p,
                    case.cycle1_a,
                    case.cycle1_m,
                    case.cycle1_b,
                    case.blend_enabled,
                ] {
                    bytes.extend_from_slice(&scalar.to_le_bytes());
                }
                // `src: vec4<f32>` starts at offset 40 in a tightly packed
                // layout, but WGSL's 16-byte `vec4<f32>` alignment forces it
                // to offset 48 -- pad to match, or every field from here on
                // reads shifted (this is exactly what made `copy_bypass`
                // observe channel 0/1 holding `src`'s own channel 2/3).
                bytes.extend_from_slice(&[0u8; 8]);
                for value in case.src {
                    bytes.extend_from_slice(&value.to_le_bytes());
                }
                bytes.extend_from_slice(&case.shade_alpha.to_le_bytes());
                // `blend_color: vec4<f32>` starts at offset 68 packed, but
                // needs offset 80 for the same 16-byte alignment reason.
                bytes.extend_from_slice(&[0u8; 12]);
                for value in case.blend_color {
                    bytes.extend_from_slice(&value.to_le_bytes());
                }
                for value in case.fog_color {
                    bytes.extend_from_slice(&value.to_le_bytes());
                }
                debug_assert_eq!(bytes.len(), WGSL_CASE_STRIDE as usize);
                bytes
            })
            .collect();
        queue.write_buffer(&case_buffer, 0, &case_data);

        let layout = pipeline.get_bind_group_layout(0);
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("fn64-blend-fragment-fn-bind-group"),
            layout: &layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: case_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: result_buffer.as_entire_binding(),
                },
            ],
        });

        let mut encoder =
            device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("fn64-blend-fragment-fn-pass"),
                timestamp_writes: None,
            });
            pass.set_pipeline(&pipeline);
            pass.set_bind_group(0, &bind_group, &[]);
            pass.dispatch_workgroups(cases.len() as u32, 1, 1);
        }
        encoder.copy_buffer_to_buffer(&result_buffer, 0, &readback_buffer, 0, result_bytes);
        queue.submit(Some(encoder.finish()));

        let slice = readback_buffer.slice(..);
        let (sender, receiver) = std::sync::mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |result| {
            let _ = sender.send(result);
        });
        loop {
            let _ = device.poll(wgpu::PollType::Poll);
            if let Ok(result) = receiver.try_recv() {
                result.unwrap();
                break;
            }
        }
        let observed: Vec<[f32; 4]> = {
            let mapped = slice.get_mapped_range().unwrap();
            mapped
                .chunks_exact(16)
                .map(|chunk| {
                    let mut value = [0.0f32; 4];
                    for (channel, word) in chunk.chunks_exact(4).enumerate() {
                        value[channel] = f32::from_le_bytes(word.try_into().unwrap());
                    }
                    value
                })
                .collect()
        };
        readback_buffer.unmap();

        assert_eq!(observed.len(), fixtures.len());
        for (fixture, observed_normalized) in fixtures.iter().zip(observed.iter()) {
            let observed_rgba = [
                (observed_normalized[0] * 255.0).round().clamp(0.0, 255.0) as u8,
                (observed_normalized[1] * 255.0).round().clamp(0.0, 255.0) as u8,
                (observed_normalized[2] * 255.0).round().clamp(0.0, 255.0) as u8,
                (observed_normalized[3] * 255.0).round().clamp(0.0, 255.0) as u8,
            ];
            for channel in 0..4 {
                let diff =
                    i32::from(observed_rgba[channel]) - i32::from(fixture.expected_rgba[channel]);
                assert!(
                    diff.abs() <= 1,
                    "fixture {}: channel {channel} observed={observed_rgba:?} expected={:?} \
                     (WGSL shim result diverged from crate::blend::blend_fragment's own byte \
                     output by more than float-rounding tolerance)",
                    fixture.name,
                    fixture.expected_rgba
                );
            }
        }
    }
}
