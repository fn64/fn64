// RT64 color combiner: selector decode and one-cycle arithmetic.
//
// Characterization-first port, Slice 1 of
// /private/tmp/rt64-combiner-characterization-card.md
// (sha256 e67751ff975eaf970b8179b2b62bd0093ccddac3d73c3dc0539b611006b345a).
// Source: MIT RT64, pinned commit 5473732a822a4423b5696e7cb18fecc425a59875,
// src/shared/rt64_color_combiner.h (the GPU/#else branch) and
// src/shaders/RasterPS.hlsl:165-184 (combiner input assembly/invocation
// shape, not its D3D12/Vulkan-specific surroundings).
//
// This is an owned WGSL transcription, not a compiled/transpiled HLSL file.
// It must match crates/fn64-render-wgpu/src/combiner.rs's Rust oracle
// exactly; see that file's module docs for the full scope statement.
//
// Decode is exact and complete for every wire-legal index (decode_color/
// decode_alpha and the per-slot tables below reproduce RT64's
// decodeColorInput/decodeAlphaInput bit-for-bit, including NOISE/
// KEY_CENTER/KEY_SCALE/K4/K5/LOD_FRACTION/PRIM_LOD_FRAC/*_ALPHA — only
// genuine RT64 out-of-range indices alias ZERO, matching the pinned
// source's own `default:` arms). Scope narrowing happens one layer later,
// in resolve_color_input/resolve_alpha_input (the arithmetic layer): only
// COMBINED/TEXEL0/TEXEL1/PRIMITIVE/SHADE/ENVIRONMENT/ONE/ZERO are
// evaluated there; any other selector returns the OUT_OF_SCOPE success
// flag rather than a silently substituted ZERO. One-cycle mode only, no
// two-cycle/copy mode/draw wiring.
//
// Selector encoding below matches RT64's enum ordinal order exactly
// (rt64_color_combiner.h ColorInput/AlphaInput), so a decode function's
// returned u32 is directly comparable to these constants without a
// separate mapping table.

const COLOR_COMBINED: u32 = 0u;
const COLOR_TEXEL0: u32 = 1u;
const COLOR_TEXEL1: u32 = 2u;
const COLOR_PRIMITIVE: u32 = 3u;
const COLOR_SHADE: u32 = 4u;
const COLOR_ENVIRONMENT: u32 = 5u;
const COLOR_KEY_CENTER: u32 = 6u;
const COLOR_KEY_SCALE: u32 = 7u;
const COLOR_COMBINED_ALPHA: u32 = 8u;
const COLOR_TEXEL0_ALPHA: u32 = 9u;
const COLOR_TEXEL1_ALPHA: u32 = 10u;
const COLOR_PRIMITIVE_ALPHA: u32 = 11u;
const COLOR_SHADE_ALPHA: u32 = 12u;
const COLOR_ENV_ALPHA: u32 = 13u;
const COLOR_LOD_FRACTION: u32 = 14u;
const COLOR_PRIM_LOD_FRAC: u32 = 15u;
const COLOR_NOISE: u32 = 16u;
const COLOR_K4: u32 = 17u;
const COLOR_K5: u32 = 18u;
const COLOR_ONE: u32 = 19u;
const COLOR_ZERO: u32 = 20u;

const ALPHA_COMBINED: u32 = 0u;
const ALPHA_TEXEL0: u32 = 1u;
const ALPHA_TEXEL1: u32 = 2u;
const ALPHA_PRIMITIVE: u32 = 3u;
const ALPHA_SHADE: u32 = 4u;
const ALPHA_ENVIRONMENT: u32 = 5u;
const ALPHA_LOD_FRACTION: u32 = 6u;
const ALPHA_PRIM_LOD_FRAC: u32 = 7u;
const ALPHA_ONE: u32 = 8u;
const ALPHA_ZERO: u32 = 9u;

// SLOT_A/B/C/D match ColorInputSlot/AlphaInputSlot's Rust ordinal order.
const SLOT_A: u32 = 0u;
const SLOT_B: u32 = 1u;
const SLOT_C: u32 = 2u;
const SLOT_D: u32 = 3u;

// `Combiner selector is out of Slice 1 scope` — the sentinel this module's
// functions return instead of silently substituting ZERO. Not one of the
// encoded selector values above, so a caller can distinguish "decoded to
// ZERO" from "decoded to something Slice 1 doesn't implement yet."
const OUT_OF_SCOPE: u32 = 0xffffffffu;

struct CombineParams {
    low: u32,
    high: u32,
}

fn parse_color_a(params: CombineParams, second_cycle: bool) -> u32 {
    if second_cycle {
        return (params.low >> 5u) & 0xFu;
    }
    return (params.low >> 20u) & 0xFu;
}

fn parse_color_b(params: CombineParams, second_cycle: bool) -> u32 {
    if second_cycle {
        return (params.high >> 24u) & 0xFu;
    }
    return (params.high >> 28u) & 0xFu;
}

fn parse_color_c(params: CombineParams, second_cycle: bool) -> u32 {
    if second_cycle {
        return params.low & 0x1Fu;
    }
    return (params.low >> 15u) & 0x1Fu;
}

fn parse_color_d(params: CombineParams, second_cycle: bool) -> u32 {
    if second_cycle {
        return (params.high >> 6u) & 0x7u;
    }
    return (params.high >> 15u) & 0x7u;
}

fn parse_alpha_a(params: CombineParams, second_cycle: bool) -> u32 {
    if second_cycle {
        return (params.high >> 21u) & 0x7u;
    }
    return (params.low >> 12u) & 0x7u;
}

fn parse_alpha_b(params: CombineParams, second_cycle: bool) -> u32 {
    if second_cycle {
        return (params.high >> 3u) & 0x7u;
    }
    return (params.high >> 12u) & 0x7u;
}

fn parse_alpha_c(params: CombineParams, second_cycle: bool) -> u32 {
    if second_cycle {
        return (params.high >> 18u) & 0x7u;
    }
    return (params.low >> 9u) & 0x7u;
}

fn parse_alpha_d(params: CombineParams, second_cycle: bool) -> u32 {
    if second_cycle {
        return params.high & 0x7u;
    }
    return (params.high >> 9u) & 0x7u;
}

fn color_input_common(index: u32) -> u32 {
    switch index {
        case 0u: { return COLOR_COMBINED; }
        case 1u: { return COLOR_TEXEL0; }
        case 2u: { return COLOR_TEXEL1; }
        case 3u: { return COLOR_PRIMITIVE; }
        case 4u: { return COLOR_SHADE; }
        case 5u: { return COLOR_ENVIRONMENT; }
        default: { return COLOR_ZERO; }
    }
}

// Decode tables below are exact per RT64's colorInputA/B/C/D/alphaInputABD/
// alphaInputC (rt64_color_combiner.h) — every wire-legal index decodes to
// the same selector RT64 itself produces, including selectors this file's
// arithmetic layer does not yet evaluate (NOISE, KEY_CENTER, K4, KEY_SCALE,
// *_ALPHA, LOD_FRACTION, PRIM_LOD_FRAC). Only genuine RT64 out-of-range
// indices (the unmapped upper portion of each field) alias ZERO.

fn color_input_a(index: u32) -> u32 {
    if index <= 5u {
        return color_input_common(index);
    }
    switch index {
        case 6u: { return COLOR_ONE; }
        case 7u: { return COLOR_NOISE; }
        default: { return COLOR_ZERO; }
    }
}

fn color_input_b(index: u32) -> u32 {
    if index <= 5u {
        return color_input_common(index);
    }
    switch index {
        case 6u: { return COLOR_KEY_CENTER; }
        case 7u: { return COLOR_K4; }
        default: { return COLOR_ZERO; }
    }
}

fn color_input_c(index: u32) -> u32 {
    if index <= 5u {
        return color_input_common(index);
    }
    switch index {
        case 6u: { return COLOR_KEY_SCALE; }
        case 7u: { return COLOR_COMBINED_ALPHA; }
        case 8u: { return COLOR_TEXEL0_ALPHA; }
        case 9u: { return COLOR_TEXEL1_ALPHA; }
        case 10u: { return COLOR_PRIMITIVE_ALPHA; }
        case 11u: { return COLOR_SHADE_ALPHA; }
        case 12u: { return COLOR_ENV_ALPHA; }
        case 13u: { return COLOR_LOD_FRACTION; }
        case 14u: { return COLOR_PRIM_LOD_FRAC; }
        case 15u: { return COLOR_K5; }
        default: { return COLOR_ZERO; }
    }
}

fn color_input_d(index: u32) -> u32 {
    if index <= 5u {
        return color_input_common(index);
    }
    switch index {
        case 6u: { return COLOR_ONE; }
        default: { return COLOR_ZERO; }
    }
}

fn alpha_input_abd(index: u32) -> u32 {
    switch index {
        case 0u: { return ALPHA_COMBINED; }
        case 1u: { return ALPHA_TEXEL0; }
        case 2u: { return ALPHA_TEXEL1; }
        case 3u: { return ALPHA_PRIMITIVE; }
        case 4u: { return ALPHA_SHADE; }
        case 5u: { return ALPHA_ENVIRONMENT; }
        case 6u: { return ALPHA_ONE; }
        default: { return ALPHA_ZERO; }
    }
}

fn alpha_input_c(index: u32) -> u32 {
    switch index {
        case 0u: { return ALPHA_LOD_FRACTION; }
        case 1u: { return ALPHA_TEXEL0; }
        case 2u: { return ALPHA_TEXEL1; }
        case 3u: { return ALPHA_PRIMITIVE; }
        case 4u: { return ALPHA_SHADE; }
        case 5u: { return ALPHA_ENVIRONMENT; }
        case 6u: { return ALPHA_PRIM_LOD_FRAC; }
        default: { return ALPHA_ZERO; }
    }
}

fn decode_color(params: CombineParams, slot: u32, second_cycle: bool) -> u32 {
    switch slot {
        case SLOT_A: { return color_input_a(parse_color_a(params, second_cycle)); }
        case SLOT_B: { return color_input_b(parse_color_b(params, second_cycle)); }
        case SLOT_C: { return color_input_c(parse_color_c(params, second_cycle)); }
        case SLOT_D: { return color_input_d(parse_color_d(params, second_cycle)); }
        default: { return COLOR_ZERO; }
    }
}

fn decode_alpha(params: CombineParams, slot: u32, second_cycle: bool) -> u32 {
    switch slot {
        case SLOT_A: { return alpha_input_abd(parse_alpha_a(params, second_cycle)); }
        case SLOT_B: { return alpha_input_abd(parse_alpha_b(params, second_cycle)); }
        case SLOT_C: { return alpha_input_c(parse_alpha_c(params, second_cycle)); }
        case SLOT_D: { return alpha_input_abd(parse_alpha_d(params, second_cycle)); }
        default: { return ALPHA_ZERO; }
    }
}

struct CombinerInputs {
    tex_val0: vec4<f32>,
    tex_val1: vec4<f32>,
    prim_color: vec4<f32>,
    shade_color: vec4<f32>,
    env_color: vec4<f32>,
}

// Resolves one color selector to RGB, per `fromColorInput`. `combiner_color`
// stands in for RT64's `combinerColor.rgb` accumulator, always [0,0,0] in
// one-cycle mode (RT64 `run`'s zero-init, unwritten before the single
// `runCycle` call) — see combiner.rs's matching function for the full note,
// including why the TEXEL0/TEXEL1 secondCycle swap never triggers here.
// This is the ONLY place selector scope is narrowed in this file: `selector`
// may legitimately be any value decode_color/decode_alpha can produce
// (including NOISE, KEY_CENTER, etc. — decode above is exact), and every
// value outside {COMBINED, TEXEL0, TEXEL1, PRIMITIVE, SHADE, ENVIRONMENT,
// ONE, ZERO} falls to `default`, which reports OUT_OF_SCOPE via the success
// flag rather than substituting a wrong value for it. Returns vec4(rgb, 1.0)
// on success or vec4(_, _, _, 0.0) — the w component is a success flag,
// since WGSL has no Result type (component xyz are then 0.0, not
// meaningful, since the selector's real arithmetic was never evaluated).
fn resolve_color_input(inputs: CombinerInputs, combiner_color: vec3<f32>, selector: u32) -> vec4<f32> {
    switch selector {
        case COLOR_COMBINED: { return vec4<f32>(combiner_color, 1.0); }
        case COLOR_TEXEL0: { return vec4<f32>(inputs.tex_val0.rgb, 1.0); }
        case COLOR_TEXEL1: { return vec4<f32>(inputs.tex_val1.rgb, 1.0); }
        case COLOR_PRIMITIVE: { return vec4<f32>(inputs.prim_color.rgb, 1.0); }
        case COLOR_SHADE: { return vec4<f32>(inputs.shade_color.rgb, 1.0); }
        case COLOR_ENVIRONMENT: { return vec4<f32>(inputs.env_color.rgb, 1.0); }
        case COLOR_ONE: { return vec4<f32>(1.0, 1.0, 1.0, 1.0); }
        case COLOR_ZERO: { return vec4<f32>(0.0, 0.0, 0.0, 1.0); }
        default: { return vec4<f32>(0.0, 0.0, 0.0, 0.0); }
    }
}

// Resolves one alpha selector, per `fromAlphaInput`. Returns
// vec2(value, success_flag) — see resolve_color_input's note on the
// success-flag convention.
fn resolve_alpha_input(inputs: CombinerInputs, combiner_alpha: f32, selector: u32) -> vec2<f32> {
    switch selector {
        case ALPHA_COMBINED: { return vec2<f32>(combiner_alpha, 1.0); }
        case ALPHA_TEXEL0: { return vec2<f32>(inputs.tex_val0.a, 1.0); }
        case ALPHA_TEXEL1: { return vec2<f32>(inputs.tex_val1.a, 1.0); }
        case ALPHA_PRIMITIVE: { return vec2<f32>(inputs.prim_color.a, 1.0); }
        case ALPHA_SHADE: { return vec2<f32>(inputs.shade_color.a, 1.0); }
        case ALPHA_ENVIRONMENT: { return vec2<f32>(inputs.env_color.a, 1.0); }
        case ALPHA_ONE: { return vec2<f32>(1.0, 1.0); }
        case ALPHA_ZERO: { return vec2<f32>(0.0, 1.0); }
        default: { return vec2<f32>(0.0, 0.0); }
    }
}

// The final wrapClamp RT64 always applies to the finished color and to
// alphaCompareValue, regardless of cycle count (rt64_color_combiner.h's
// wrapClamp = wrapInputABD then clamp(0,1)). See combiner.rs's wrap_clamp
// for the exact reasoning on why this reduces to a plain clamp for every
// input Slice 1 can produce, while still being implemented as the full
// two-step formula.
fn wrap_clamp(i: f32) -> f32 {
    let rounding: f32 = 1.0 / 255.0;
    let low: f32 = -0.5 - rounding;
    let high: f32 = 1.5 + rounding;
    let range: f32 = high - low;
    var wrapped: f32 = i;
    if wrapped <= low {
        wrapped = wrapped + range;
    }
    if high <= wrapped {
        wrapped = wrapped - range;
    }
    return clamp(wrapped, 0.0, 1.0);
}

// Runs one-cycle combiner arithmetic and the final wrapClamp pass, mirroring
// combiner.rs's run_one_cycle exactly. Decode (ca/cb/cc/cd/aa/ab/ac/ad
// below) is always exact per RT64, regardless of scope — only
// resolve_color_input/resolve_alpha_input narrow what gets evaluated.
// Returns RunOneCycleResult: on success, rejected_selector is
// OUT_OF_SCOPE (never a real selector's ordinal, since OUT_OF_SCOPE is
// disjoint from every COLOR_*/ALPHA_* constant) and combiner_color/
// alpha_compare_value hold the real result; on failure, rejected_selector
// is the exact decoded ordinal (color or alpha, whichever's resolve call
// hit `default` first — see the priority comment below) that this file's
// arithmetic does not yet implement, and combiner_color/alpha_compare_value
// are not meaningful. WGSL has no Result type, so this is the loud-
// rejection mechanism's shader-side shape: a caller must check
// rejected_selector != OUT_OF_SCOPE before trusting the rest of the struct.
struct RunOneCycleResult {
    combiner_color: vec4<f32>,
    alpha_compare_value: f32,
    rejected_selector: u32,
}

fn run_one_cycle(params: CombineParams, inputs: CombinerInputs) -> RunOneCycleResult {
    let second_cycle: bool = true;

    let ca = decode_color(params, SLOT_A, second_cycle);
    let cb = decode_color(params, SLOT_B, second_cycle);
    let cc = decode_color(params, SLOT_C, second_cycle);
    let cd = decode_color(params, SLOT_D, second_cycle);
    let aa = decode_alpha(params, SLOT_A, second_cycle);
    let ab = decode_alpha(params, SLOT_B, second_cycle);
    let ac = decode_alpha(params, SLOT_C, second_cycle);
    let ad = decode_alpha(params, SLOT_D, second_cycle);

    let combiner_color_in = vec3<f32>(0.0, 0.0, 0.0);
    let combiner_alpha_in = 0.0;

    let a = resolve_color_input(inputs, combiner_color_in, ca);
    let b = resolve_color_input(inputs, combiner_color_in, cb);
    let c = resolve_color_input(inputs, combiner_color_in, cc);
    let d = resolve_color_input(inputs, combiner_color_in, cd);

    let aa_v = resolve_alpha_input(inputs, combiner_alpha_in, aa);
    let ab_v = resolve_alpha_input(inputs, combiner_alpha_in, ab);
    let ac_v = resolve_alpha_input(inputs, combiner_alpha_in, ac);
    let ad_v = resolve_alpha_input(inputs, combiner_alpha_in, ad);

    // Priority order (first rejection wins) matches combiner.rs's `?`
    // early-return order: color A, B, C, D, then alpha A, B, C, D.
    var rejected_selector: u32 = OUT_OF_SCOPE;
    if a.w == 0.0 { rejected_selector = ca; }
    else if b.w == 0.0 { rejected_selector = cb; }
    else if c.w == 0.0 { rejected_selector = cc; }
    else if d.w == 0.0 { rejected_selector = cd; }
    else if aa_v.y == 0.0 { rejected_selector = aa; }
    else if ab_v.y == 0.0 { rejected_selector = ab; }
    else if ac_v.y == 0.0 { rejected_selector = ac; }
    else if ad_v.y == 0.0 { rejected_selector = ad; }

    let rgb = (a.rgb - b.rgb) * c.rgb + d.rgb;
    let alpha = (aa_v.x - ab_v.x) * ac_v.x + ad_v.x;

    var result: RunOneCycleResult;
    result.rejected_selector = rejected_selector;
    result.alpha_compare_value = wrap_clamp(alpha);
    result.combiner_color = vec4<f32>(
        wrap_clamp(rgb.x),
        wrap_clamp(rgb.y),
        wrap_clamp(rgb.z),
        wrap_clamp(alpha),
    );
    return result;
}
