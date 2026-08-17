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

// Framebuffer-blend Slice B: `blend_color_fragment_fn`'s memory-aware
// counterpart. Identical except for one additional `dst_rgb: vec3<f32>`
// parameter and its `BLEND_COLOR_FRAMEBUFFER` arm, which returns the real
// destination sample (`dst_rgb`) instead of falling back to `running_rgb` --
// the fallback in `blend_color_fragment_fn` exists precisely because
// `Framebuffer` is host-rejected there and unreachable; here it is the whole
// point of the function, so the real value must be returned, not the
// placeholder. Literal re-expression of `crate::blend::blend_color`
// (`blend.rs:244-277`), color-only (no alpha).
fn blend_color_memory_fragment_fn(
    selector: u32,
    src_rgb: vec3<f32>,
    dst_rgb: vec3<f32>,
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
    if (selector == BLEND_COLOR_FRAMEBUFFER) {
        return dst_rgb;
    }
    if (selector == BLEND_COLOR_BLEND) {
        return blend_color_rgb;
    }
    return fog_color_rgb;
}

// One blend cycle's FULL evaluation (not the admitted subset): literal
// re-expression of `blend_fragment`'s per-cycle body (`blend.rs:416-480`),
// including the `Framebuffer`-selecting three-way branch
// (`cycle.p == Framebuffer`/`cycle.m == Framebuffer`/general divide,
// `blend.rs:459-479`) and the no-`FORCE_BL` last-cycle bypass arm
// (`blend.rs:421-437`), which for the last cycle only evaluates `P` (never
// `M`/`A`/`B`) and sets `final_alpha` to `0.0` iff `P == Framebuffer` else
// `1.0` -- a distinct arm from the three-way branch below it, checked first,
// exactly mirroring `blend_one_cycle_fn`'s own `is_last_cycle`-gated bypass
// priority but with memory-aware arithmetic. Returns a `vec4<f32>` whose
// `.rgb` is the cycle's `blender_rgb` and whose `.a` is the cycle's
// `final_alpha` (a `[0,1]` blend-fraction scalar, NOT a color channel --
// distinct from this function's own final output alpha, which the caller
// computes once after the whole cycle loop, not per cycle).
//
// `is_first_cycle`/`is_last_cycle` are host-known call-site literals derived
// from `cycle_count`, exactly the same pattern `blend_fragment_cycle_fn`
// already establishes for its own calls to `blend_one_cycle_fn`: cycle 0's
// `is_last_cycle` is `select(0u, 1u, cycle_count == 1u)`, cycle 1's (only
// reached when `cycle_count == 2u`) is always `1u`. No new signature
// parameter is needed to carry "is this the last cycle" beyond these two
// existing flags.
fn blend_one_memory_cycle_fn(
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
    dst_rgb: vec3<f32>,
) -> vec4<f32> {
    if (is_last_cycle != 0u && blend_enabled == 0u) {
        let blender_rgb = blend_color_memory_fragment_fn(
            p_selector, src_rgb, dst_rgb, blend_color_rgb, fog_color_rgb, is_first_cycle,
            running_rgb,
        );
        let final_alpha = select(1.0, 0.0, p_selector == BLEND_COLOR_FRAMEBUFFER);
        return vec4<f32>(blender_rgb, final_alpha);
    }

    let a = blend_a_fragment_fn(a_selector, src_alpha, shade_alpha, fog_color_alpha);
    let p = blend_color_memory_fragment_fn(
        p_selector, src_rgb, dst_rgb, blend_color_rgb, fog_color_rgb, is_first_cycle, running_rgb,
    );
    let m = blend_color_memory_fragment_fn(
        m_selector, src_rgb, dst_rgb, blend_color_rgb, fog_color_rgb, is_first_cycle, running_rgb,
    );

    if (p_selector == BLEND_COLOR_FRAMEBUFFER) {
        return vec4<f32>(m, 1.0 - a);
    }
    if (m_selector == BLEND_COLOR_FRAMEBUFFER) {
        return vec4<f32>(p, a);
    }
    let b = blend_b_fragment_fn(b_selector, a);
    var blender_rgb: vec3<f32>;
    if (a == 0.0) {
        blender_rgb = m;
    } else if (b == 0.0) {
        blender_rgb = p;
    } else {
        let divisor = a + b;
        blender_rgb = clamp((p * a + m * b) / divisor, vec3<f32>(0.0), vec3<f32>(255.0));
    }
    return vec4<f32>(blender_rgb, 1.0);
}

// Framebuffer-blend Slice B: full memory-aware composite over every active
// cycle, called INSTEAD OF `blend_fragment_cycle_fn` (never chained after
// it) when the fragment's `has_framebuffer_color` flag is set -- the two
// functions are mutually exclusive per fragment, matching
// `crate::blend::blend_fragment`'s own single dispatch (one Rust oracle
// function, not two composed ones). A second, independent transcription of
// the ENTIRE `blend_fragment` cycle loop (`blend.rs:400-499`), not a
// post-hoc patch applied after `blend_fragment_cycle_fn`'s own two-cycle
// collapse: `blend_fragment_cycle_fn` has already discarded the per-cycle
// `final_alpha` history and any memory-composite involvement by the time it
// returns, so a patch bolted on afterward cannot reproduce cycle 0's
// `Combined`-reads-prior-cycle handoff into cycle 1 or the last cycle's
// `final_alpha` correctly.
//
// `src` is the combiner's own pre-blend output color (`result.combiner_color`
// in `fs_main`, i.e. `output.color` BEFORE `blend_fragment_cycle_fn` would
// have run -- never that function's own result, which has already thrown
// away the state this function's cycle-0 `Combined` arm needs).
// `dst_rgb`/`dst_alpha` are the framebuffer-color snapshot's decoded
// destination sample, `[0,255]`-scaled to match this file's existing
// `src_rgb = src.rgb * 255.0` convention.
//
// Final composite (`blend.rs:488-496`, both channels): RGB is
// `blender_rgb[c] * final_alpha + dst_rgb[c] * (1.0 - final_alpha)` per
// channel (the last cycle's `blender_rgb`/`final_alpha`); alpha is the
// SEPARATE `255.0 * final_alpha + dst_alpha * (1.0 - final_alpha)` byte
// composite -- not `src.a` passed through unchanged (drops the
// destination's own alpha contribution whenever `final_alpha != 1.0`) and
// not `final_alpha` itself returned as the output alpha (conflates the
// blend fraction with the actual output alpha channel).
fn blend_fragment_memory_composite_fn(
    cycle_count: u32,
    cycle0_p: u32, cycle0_a: u32, cycle0_m: u32, cycle0_b: u32,
    cycle1_p: u32, cycle1_a: u32, cycle1_m: u32, cycle1_b: u32,
    src: vec4<f32>,
    shade_alpha: f32,
    blend_color: vec4<f32>,
    fog_color: vec4<f32>,
    blend_enabled: u32,
    dst: vec4<f32>,
) -> vec4<f32> {
    let src_rgb = src.rgb * 255.0;
    let blend_color_rgb = blend_color.rgb * 255.0;
    let fog_color_rgb = fog_color.rgb * 255.0;
    let dst_rgb = dst.rgb * 255.0;
    let dst_alpha = dst.a * 255.0;
    var running_rgb = src_rgb;
    var final_alpha = 1.0;

    let cycle0_result = blend_one_memory_cycle_fn(
        cycle0_p, cycle0_a, cycle0_m, cycle0_b,
        src_rgb, src.a, shade_alpha,
        blend_color_rgb, blend_color.a,
        fog_color_rgb, fog_color.a,
        1u,
        select(0u, 1u, cycle_count == 1u),
        blend_enabled,
        running_rgb,
        dst_rgb,
    );
    running_rgb = cycle0_result.rgb;
    final_alpha = cycle0_result.a;

    if (cycle_count == 2u) {
        let cycle1_result = blend_one_memory_cycle_fn(
            cycle1_p, cycle1_a, cycle1_m, cycle1_b,
            src_rgb, src.a, shade_alpha,
            blend_color_rgb, blend_color.a,
            fog_color_rgb, fog_color.a,
            0u,
            1u,
            blend_enabled,
            running_rgb,
            dst_rgb,
        );
        running_rgb = cycle1_result.rgb;
        final_alpha = cycle1_result.a;
    }

    var out_rgb: vec3<f32>;
    for (var channel = 0u; channel < 3u; channel = channel + 1u) {
        out_rgb[channel] = clamp(
            running_rgb[channel] * final_alpha + dst_rgb[channel] * (1.0 - final_alpha),
            0.0, 255.0,
        );
    }
    let out_alpha = clamp(255.0 * final_alpha + dst_alpha * (1.0 - final_alpha), 0.0, 255.0);
    return vec4<f32>(out_rgb / 255.0, out_alpha / 255.0);
}
