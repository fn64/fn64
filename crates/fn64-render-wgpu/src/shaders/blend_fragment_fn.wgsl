// Blender, fragment-callable form (production blend wiring slice 1).
// Concatenated by `shader_manifest.rs` into the production triangle fragment
// shader (`shaders/triangle_pipeline_fragment.wgsl`) and called from that
// file's `fs_main` after combiner evaluation and coverage, gating the same
// `blend_enabled` value `coverage_fragment_fn` already produced -- no second
// gate is invented here. See `crate::blend`'s module doc and this crate's
// README for the exact admitted-subset boundary this file shares.
//
// Admitted subset only (card §1/§2): Copy/Fill bypass, and OneCycle/TwoCycle
// whose ACTIVE cycles never select a framebuffer-dependent input
// (`BlendColorInput::Framebuffer` on `P`/`M`, `BlendBInput::FramebufferAlpha`
// on `B`). A triangle whose real `OtherMode` needs a framebuffer sample is
// rejected host-side, before this shader ever runs, by
// `crate::blend::ResolvedBlendCycle::requires_framebuffer_sample` --
// `blend_fragment_cycle_fn` below has no `memory` parameter at all and never
// reads a destination color; the "requires a memory sample" branches
// `crate::blend::blend_fragment` (the Rust oracle) implements are
// structurally absent here, not silently defaulted.
//
// Ordinary WGSL function re-expression of `crate::blend::blend_fragment`'s
// admitted-subset control flow (`blend.rs:382-482`): sequential 1-or-2-cycle
// composite, the no-`FORCE_BL` last-cycle bypass (`blend_enabled == 0u`),
// the zero-factor (`a==0`/`b==0`) divisor collapse, and the general
// `(P*A + M*B) / (A+B)` divide. Selector encodings match
// `BlendColorInput`/`BlendAlphaInput`/`BlendBInput::from_wire` exactly,
// reusing the same wire numbering `shaders/blend.wgsl`'s header already
// documents:
//   color (P/M): 0=Combined, 1=Framebuffer, 2=Blend, 3=Fog
//   alpha (A):   0=Combined, 1=Fog, 2=Shade, 3=Zero
//   b:           0=OneMinusA, 1=FramebufferAlpha, 2=One, 3=Zero
// `P`/`M` == 1 (Framebuffer) and `B` == 1 (FramebufferAlpha) are host-
// rejected before this function is ever called with such a cycle active
// (see module doc above); this function still treats them as architecturally
// unreachable inputs (falls back to `0.0`) rather than indexing out of
// bounds, matching this crate's "no undefined behavior even on an input a
// caller contract already rules out" convention elsewhere (e.g.
// `alpha_compare_fragment_fn`'s defensive `false` return for Reserved).
//
// Unlike `crate::blend::blend_fragment`, this function takes plain scalar
// arguments already available in fragment-shader scope (no `Option`/
// `Result`, no destination-color parameter) and returns a plain `vec4<f32>`
// in `[0,1]` (not `[u8; 4]` in `[0,255]`) -- WGSL has no `Option`/`Result`,
// and this fragment shader's own combiner/coverage pipeline already works in
// normalized float, so this function stays in that same space rather than
// round-tripping through byte scale. `src` is the combiner's own output
// color (`result.combiner_color` in `fs_main`), matching `blend_fragment`'s
// own `src` parameter.

const BLEND_COLOR_COMBINED: u32 = 0u;
const BLEND_COLOR_FRAMEBUFFER: u32 = 1u;
const BLEND_COLOR_BLEND: u32 = 2u;
const BLEND_COLOR_FOG: u32 = 3u;

const BLEND_ALPHA_COMBINED: u32 = 0u;
const BLEND_ALPHA_FOG: u32 = 1u;
const BLEND_ALPHA_SHADE: u32 = 2u;
const BLEND_ALPHA_ZERO: u32 = 3u;

const BLEND_B_ONE_MINUS_A: u32 = 0u;
const BLEND_B_FRAMEBUFFER_ALPHA: u32 = 1u;
const BLEND_B_ONE: u32 = 2u;
const BLEND_B_ZERO: u32 = 3u;

// `blend_color`'s admitted-subset resolution: literal re-expression of
// `crate::blend::blend_color` (`blend.rs:226-259`) restricted to the three
// selectors this shader can serve without a memory sample
// (`Combined`/`Blend`/`Fog`); `Framebuffer` returns `running_rgb` (the
// current running color) as a defensive fallback, matching this file's own
// "architecturally unreachable, never undefined" convention above -- a real
// draw never reaches this arm (host-side rejection, module doc).
fn blend_color_fragment_fn(
    selector: u32,
    src_rgb: vec3<f32>,
    blend_color_rgb: vec3<f32>,
    fog_color_rgb: vec3<f32>,
    is_first_cycle: u32,
    running_rgb: vec3<f32>,
) -> vec3<f32> {
    if (selector == BLEND_COLOR_COMBINED) {
        if (is_first_cycle != 0u) {
            return src_rgb;
        }
        return running_rgb;
    }
    if (selector == BLEND_COLOR_BLEND) {
        return blend_color_rgb;
    }
    if (selector == BLEND_COLOR_FOG) {
        return fog_color_rgb;
    }
    // BLEND_COLOR_FRAMEBUFFER: host-rejected before this call (module doc).
    return running_rgb;
}

// Literal re-expression of `crate::blend::blend_a` (`blend.rs:263-271`).
fn blend_a_fragment_fn(selector: u32, combined: f32, shade: f32, fog: f32) -> f32 {
    if (selector == BLEND_ALPHA_FOG) {
        return fog;
    }
    if (selector == BLEND_ALPHA_SHADE) {
        return shade;
    }
    if (selector == BLEND_ALPHA_ZERO) {
        return 0.0;
    }
    return combined;
}

// Admitted-subset re-expression of `crate::blend::blend_b`
// (`blend.rs:279-295`): `FramebufferAlpha` returns `0.0` as the same
// defensive, host-rejected-before-reached fallback `blend_color_fragment_fn`
// uses for `Framebuffer`.
fn blend_b_fragment_fn(selector: u32, a: f32) -> f32 {
    if (selector == BLEND_B_ONE_MINUS_A) {
        return 1.0 - a;
    }
    if (selector == BLEND_B_ONE) {
        return 1.0;
    }
    if (selector == BLEND_B_FRAMEBUFFER_ALPHA) {
        return 0.0;
    }
    return 0.0;
}

// One blend cycle's admitted-subset evaluation: literal re-expression of
// `blend_fragment`'s per-cycle body (`blend.rs:398-461`) restricted to the
// non-`Framebuffer`/non-`FramebufferAlpha` arms (the general A/B divide and
// its zero-factor collapses) plus the no-`FORCE_BL` last-cycle bypass.
// Returns the cycle's `blender_rgb` result; `final_alpha` (needed by the
// caller's own memory-composite step, out of scope for this admitted
// subset) is not computed here since the admitted subset never blends
// against a memory sample -- see `blend_fragment_cycle_fn`'s own doc for why
// its return is `blender_rgb` alone.
fn blend_one_cycle_fn(
    p_selector: u32,
    a_selector: u32,
    m_selector: u32,
    b_selector: u32,
    src_rgb: vec3<f32>,
    src_alpha: f32,
    shade_alpha: f32,
    blend_color_rgb: vec3<f32>,
    blend_color_alpha: f32,
    fog_color_rgb: vec3<f32>,
    fog_color_alpha: f32,
    is_first_cycle: u32,
    is_last_cycle: u32,
    blend_enabled: u32,
    running_rgb: vec3<f32>,
) -> vec3<f32> {
    if (is_last_cycle != 0u && blend_enabled == 0u) {
        return blend_color_fragment_fn(
            p_selector, src_rgb, blend_color_rgb, fog_color_rgb, is_first_cycle, running_rgb
        );
    }

    let a = blend_a_fragment_fn(a_selector, src_alpha, shade_alpha, fog_color_alpha);
    let p = blend_color_fragment_fn(
        p_selector, src_rgb, blend_color_rgb, fog_color_rgb, is_first_cycle, running_rgb
    );
    let m = blend_color_fragment_fn(
        m_selector, src_rgb, blend_color_rgb, fog_color_rgb, is_first_cycle, running_rgb
    );
    let b = blend_b_fragment_fn(b_selector, a);

    if (a == 0.0) {
        return m;
    }
    if (b == 0.0) {
        return p;
    }
    let divisor = a + b;
    return clamp((p * a + m * b) / divisor, vec3<f32>(0.0), vec3<f32>(255.0));
}

// Full admitted-subset composite over every active cycle: literal
// re-expression of `blend_fragment`'s `cycle_count == 0` bypass and its
// cycle loop (`blend.rs:389-462`), restricted to the memory-independent
// subset (module doc). `cycle_count == 0u` (Copy/Fill) returns `src`
// unchanged, matching `blend_fragment`'s own first line exactly.
//
// `src`/`shade_alpha` are normalized `[0,1]` (this shader's own combiner
// output space); `blend_color`/`fog_color` are the raw `G_SETBLENDCOLOR`/
// `G_SETFOGCOLOR` registers, also normalized `[0,1]` (host-serialized via
// `Color4::normalized()`, the same normalization `fragment_material_params`
// already uses for env/prim color).
fn blend_fragment_cycle_fn(
    cycle_count: u32,
    cycle0_p: u32, cycle0_a: u32, cycle0_m: u32, cycle0_b: u32,
    cycle1_p: u32, cycle1_a: u32, cycle1_m: u32, cycle1_b: u32,
    src: vec4<f32>,
    shade_alpha: f32,
    blend_color: vec4<f32>,
    fog_color: vec4<f32>,
    blend_enabled: u32,
) -> vec4<f32> {
    if (cycle_count == 0u) {
        return src;
    }

    let src_rgb = src.rgb * 255.0;
    let blend_color_rgb = blend_color.rgb * 255.0;
    let fog_color_rgb = fog_color.rgb * 255.0;
    var running_rgb = src_rgb;

    running_rgb = blend_one_cycle_fn(
        cycle0_p, cycle0_a, cycle0_m, cycle0_b,
        src_rgb, src.a, shade_alpha,
        blend_color_rgb, blend_color.a,
        fog_color_rgb, fog_color.a,
        1u,
        select(0u, 1u, cycle_count == 1u),
        blend_enabled,
        running_rgb,
    );

    if (cycle_count == 2u) {
        running_rgb = blend_one_cycle_fn(
            cycle1_p, cycle1_a, cycle1_m, cycle1_b,
            src_rgb, src.a, shade_alpha,
            blend_color_rgb, blend_color.a,
            fog_color_rgb, fog_color.a,
            0u,
            1u,
            blend_enabled,
            running_rgb,
        );
    }

    return vec4<f32>(running_rgb / 255.0, 1.0);
}
