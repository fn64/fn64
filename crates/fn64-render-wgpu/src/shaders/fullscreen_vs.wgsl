// FullScreenVS seam. Characterization-only; not wired into any draw path or
// bind group layout used elsewhere in this crate.
//
// Literal WGSL re-expression of `fullscreen_vs` (`crate::rt64_fullscreen_vs`,
// mirroring RT64's `src/shaders/FullScreenVS.hlsl:5-9`, pinned commit
// 5473732a822a4423b5696e7cb18fecc425a59875). Reproduces exactly:
// - the vertex-index-selected UV corner (id==1 -> v=2, id==2 -> u=2, else 0)
// - the uv * (2,-2) + (-1,1) position transform
// - the fixed z=1.0, w=1.0 depth/homogeneous output
//
// `@builtin(position)` is WGPU's analogue of HLSL's `SV_Position`; both use a
// `[0,1]` device depth range (unlike OpenGL's `[-1,1]`), so the shader's
// literal `z = 1.0f` needs no depth-range remap when re-expressed here. See
// `crate::rt64_fullscreen_vs` module doc "Admitted domain" for the full
// HLSL-to-WGSL semantic-difference accounting.

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
}

@vertex
fn fullscreen_vs(@builtin(vertex_index) id: u32) -> VertexOutput {
    var uv: vec2<f32>;
    uv.x = select(0.0, 2.0, id == 2u);
    uv.y = select(0.0, 2.0, id == 1u);

    var out: VertexOutput;
    out.position = vec4<f32>(uv * vec2<f32>(2.0, -2.0) + vec2<f32>(-1.0, 1.0), 1.0, 1.0);
    out.uv = uv;
    return out;
}
