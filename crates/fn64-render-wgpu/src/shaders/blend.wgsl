// Blender seam. Characterization-only; not wired into any draw path or bind
// group layout used elsewhere in this crate.
//
// Literal WGSL re-expression of `blend_fragment`/`blend_color`/`blend_a`/
// `blend_b` (`crate::blend`, mirroring
// `fn64-render-reference/src/raster/blend.rs:157-292`). Selector encodings
// match `BlendColorInput`/`BlendAlphaInput`/`BlendBInput::from_wire`:
// color (P/M): 0=Combined, 1=Framebuffer, 2=Blend, 3=Fog
// alpha (A):   0=Combined, 1=Fog, 2=Shade, 3=Zero
// b:           0=OneMinusA, 1=FramebufferAlpha, 2=One, 3=Zero
//
// This shader evaluates one cycle's general A/B divide arithmetic
// (non-Framebuffer P and M only -- the Framebuffer-selecting dual-source
// path is a distinct fixed-function-blend-state contract, not shader
// arithmetic, per `crate::blend::dual_source_blend_output`'s module doc; a
// real draw path dispatches to one or the other based on cycle.p/cycle.m,
// which this closed characterization shader does not do since it has no
// bind-group access to a live render target). The zero-factor divisor
// collapse (`a==0 -> M`, else `b==0 -> P`) is reproduced exactly.

struct BlendCycleInput {
    p_r: f32,
    p_g: f32,
    p_b: f32,
    m_r: f32,
    m_g: f32,
    m_b: f32,
    a: f32,
    b: f32,
}

struct BlendCycleOutput {
    r: f32,
    g: f32,
    bl: f32,
}

@group(0) @binding(0)
var<storage, read> inputs: array<BlendCycleInput>;

@group(0) @binding(1)
var<storage, read_write> outputs: array<BlendCycleOutput>;

fn evaluate(input: BlendCycleInput) -> BlendCycleOutput {
    if (input.a == 0.0) {
        return BlendCycleOutput(input.m_r, input.m_g, input.m_b);
    }
    if (input.b == 0.0) {
        return BlendCycleOutput(input.p_r, input.p_g, input.p_b);
    }
    let divisor = input.a + input.b;
    let r = clamp((input.p_r * input.a + input.m_r * input.b) / divisor, 0.0, 255.0);
    let g = clamp((input.p_g * input.a + input.m_g * input.b) / divisor, 0.0, 255.0);
    let bl = clamp((input.p_b * input.a + input.m_b * input.b) / divisor, 0.0, 255.0);
    return BlendCycleOutput(r, g, bl);
}

@compute @workgroup_size(64)
fn blend_fragment_cycle(@builtin(global_invocation_id) global_id: vec3<u32>) {
    let index = global_id.x;
    if (index >= arrayLength(&inputs)) {
        return;
    }
    outputs[index] = evaluate(inputs[index]);
}
