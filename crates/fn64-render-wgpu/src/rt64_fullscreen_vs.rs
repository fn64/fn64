//! Literal port of RT64's `VSMain` (fullscreen-triangle vertex shader), a
//! permitted MIT RT64 Rust-port source pinned at commit
//! `5473732a822a4423b5696e7cb18fecc425a59875` (`docs/RT64-PORT-AUTHORITY.md`).
//! Both source files are self-contained: `src/shaders/FullScreenVS.hlsl`
//! (SHA-256 `e03453509bbb8271f687861068190528e0a0f5a82b08e3ffaee062dcde1683dc`,
//! whole file, 9 lines) does not `#include` `src/shaders/Constants.hlsli`
//! (SHA-256 `551bcc54ec095634bbce77b9e3c7f629449835fc1443f12cbc47c7591523878b`,
//! whole file, 12 lines); both are cited here per the ticket's pinning
//! requirement, but only `FullScreenVS.hlsl` contributes executable
//! behavior -- `Constants.hlsli` is a header of `#define` constants
//! (`APPLY_LIGHTS_MINIMUM_ALPHA`, `APPLY_LIGHTS_DITHER_ALPHA`,
//! `DEPTH_RAY_QUERY_MASK`, `NO_DEPTH_RAY_QUERY_MASK`,
//! `SHADOW_CATCHER_RAY_QUERY_MASK`) that `VSMain` never references, and no
//! symbol from it is used, admitted, or re-expressed below.
//!
//! ```text
//! //
//! // RT64
//! //
//!
//! void VSMain(in uint id : SV_VertexID, out float4 pos : SV_Position, out float2 uv : TEXCOORD0) {
//!     uv.x = (id == 2) ? 2.0f : 0.0f;
//!     uv.y = (id == 1) ? 2.0f : 0.0f;
//!     pos = float4(uv * float2(2.0f, -2.0f) + float2(-1.0f, 1.0f), 1.0f, 1.0f);
//! }
//! ```
//!
//! ```text
//! //
//! // RT64
//! //
//!
//! #pragma once
//!
//! #define APPLY_LIGHTS_MINIMUM_ALPHA          0.5
//! #define APPLY_LIGHTS_DITHER_ALPHA           0.125
//!
//! #define DEPTH_RAY_QUERY_MASK                0x1
//! #define NO_DEPTH_RAY_QUERY_MASK             0x2
//! #define SHADOW_CATCHER_RAY_QUERY_MASK       0x4
//! ```
//!
//! **Reuse, not new type.** [`FullScreenVertex`] is the one owned
//! representation of `VSMain`'s two `out` values (`pos`/`uv`); the CPU oracle
//! ([`fullscreen_vs`]) and the WGSL differential sibling
//! ([`FULLSCREEN_VS_WGSL`]) both produce it, and the characterization tests
//! below compare the two independent derivations against hand-computed
//! values rather than one implementation against itself.
//!
//! ## Admitted domain
//!
//! `VSMain` is the standard "oversized fullscreen triangle" trick: three
//! vertices (`SV_VertexID` 0, 1, 2), each dispatched with no vertex buffer,
//! that together cover the full `[-1,1]` clip-space square (and more) with a
//! single triangle, avoiding the diagonal seam a two-triangle quad would
//! need. This port re-expresses that exact `id`-indexed selection and the
//! exact `uv * (2,-2) + (-1,1)` affine transform; it does not redesign it.
//!
//! HLSL-to-WGSL semantic differences admitted by this port:
//!
//! - **`SV_VertexID` vs `@builtin(vertex_index)`**: identical semantics (a
//!   zero-based per-invocation index into the current draw), just a
//!   different builtin name and, in this port, a `u32` parameter rather
//!   than an `out`-parameter convention.
//! - **`SV_Position` vs `@builtin(position)`**: identical semantics (the
//!   clip-space output position consumed by the rasterizer), different
//!   builtin name.
//! - **Clip-space Y direction**: no difference and no flip here. Both
//!   Direct3D (HLSL's target API) and WGPU/Vulkan/Metal (WGSL's target
//!   APIs) place `+Y` up in clip space; this shader's own `float2(2.0f,
//!   -2.0f)` Y-negation is RT64's *intentional* UV-to-clip-space flip
//!   (UV space has `+Y` down, matching the RDP's top-down screen-space
//!   convention used elsewhere in this crate -- e.g. `crate::raster_vs`'s
//!   own screen-to-NDC Y flip), not a D3D-vs-WGPU convention difference.
//!   That literal `-2.0` multiplier is reproduced unchanged in the WGSL.
//! - **Depth range `[0,1]` vs `[-1,1]`**: no remap needed. Direct3D's
//!   device depth range is `[0,1]`; WGPU (unlike OpenGL, which uses
//!   `[-1,1]`) also standardized on `[0,1]` for `@builtin(position).z`
//!   specifically to match D3D/Metal/Vulkan. `VSMain` writes a literal
//!   `pos.z = 1.0f` (the far/degenerate depth plane for this pass; this
//!   shader relies on `Z`/depth-test state, not vertex depth, to matter to
//!   whatever consumes it), which is a valid, in-range value under both
//!   conventions and is reproduced unchanged (`out.position.z = 1.0`) with
//!   no scale/bias applied.
//! - **Vertex-index-driven procedural geometry**: no vertex/index buffer is
//!   read; all three vertex positions are computed purely from `id`. This
//!   port preserves that -- [`fullscreen_vs`] and the WGSL entry point both
//!   take only the index as input.
//! - **Winding/orientation**: the three admitted vertices are, in `id`
//!   order 0, 1, 2: `(-1,1)`, `(-1,-3)`, `(3,1)` (clip-space `x,y`; see the
//!   [`fullscreen_vs`] doc comment for the per-vertex derivation). Signed
//!   area of that triangle in standard (mathematical, `+Y` up) orientation
//!   is `((-1)*(-3-1) - 1*(-1-(-1))) ... ` -- computed directly via the
//!   shoelace formula: `x0*(y1-y2) + x1*(y2-y0) + x2*(y0-y1)` =
//!   `(-1)*(-3-1) + (-1)*(1-1) + 3*(1-(-3))` = `4 + 0 + 12` = `16` (positive
//!   -- counter-clockwise in a `+Y`-up frame). This port does not assert
//!   which winding a consuming pipeline should cull as front-facing (no
//!   pipeline/rasterizer state exists in this module to make that
//!   determination); it only asserts the vertex positions themselves,
//!   which are winding-order-agnostic facts, are reproduced exactly.
//! - **Float literal precision**: all literals in `VSMain` (`2.0f`, `0.0f`,
//!   `-2.0f`, `-1.0f`, `1.0f`) are exactly representable in IEEE-754
//!   single precision (`f32`/`float`), so no precision concern exists --
//!   every arithmetic step in both the Rust oracle and the WGSL sibling
//!   uses the same exact binary values HLSL's `float` would, and every
//!   characterization test below asserts bit-exact (`assert_eq!`) equality
//!   rather than an epsilon comparison.
//!
//! ## Nonclaims
//!
//! This module makes no GPU execution claim: the WGSL differential test
//! below validates [`FULLSCREEN_VS_WGSL`] through Naga's WGSL front-end and
//! validator only (a plain, non-GPU test), not by dispatching it on a real
//! adapter/device. It makes no production-wiring claim: no pipeline,
//! `wgpu::ShaderModule`, bind group layout, or `targets/` integration is
//! created here, and this module is not referenced from any draw path. It
//! makes no parity or performance claim against RT64's own renderer.

/// One `VSMain` output: the clip-space position (`SV_Position/@builtin(position)`)
/// and the UV coordinate (`TEXCOORD0`). Both fields are produced together by
/// [`fullscreen_vs`] and by the WGSL sibling ([`FULLSCREEN_VS_WGSL`]); no
/// other type represents this shader's output.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FullScreenVertex {
    /// `pos` (`SV_Position`): `(x, y, z, w)` clip-space position.
    pub position: [f32; 4],
    /// `uv` (`TEXCOORD0`): `(u, v)` texture coordinate.
    pub uv: [f32; 2],
}

/// Literal port of `VSMain`'s full body
/// (`src/shaders/FullScreenVS.hlsl:5-9`). Takes `SV_VertexID` (`id`, 0/1/2 --
/// any other value is out of the shader's admitted domain: RT64 always
/// dispatches this as a 3-vertex, no-buffer draw, so `VSMain` never receives
/// any other index) and returns the `(pos, uv)` pair the HLSL writes to its
/// two `out` parameters.
///
/// Per-vertex results (each computed by substituting `id` directly into the
/// HLSL, not by pattern):
/// - `id == 0`: `uv = (0, 0)` (neither branch fires) ->
///   `pos = (0,0)*(2,-2) + (-1,1) = (-1, 1, 1, 1)`.
/// - `id == 1`: `uv.y` branch fires (`id == 1`), `uv.x` branch does not ->
///   `uv = (0, 2)` -> `pos = (0*2 + -1, 2*-2 + 1, 1, 1) = (-1, -3, 1, 1)`.
/// - `id == 2`: `uv.x` branch fires (`id == 2`), `uv.y` branch does not ->
///   `uv = (2, 0)` -> `pos = (2*2 + -1, 0*-2 + 1, 1, 1) = (3, 1, 1, 1)`.
///
/// These three vertices form a single triangle that fully covers the
/// `[-1,1]` clip-space square (and overshoots it), the standard fullscreen-
/// triangle trick (see module doc "Admitted domain").
pub fn fullscreen_vs(id: u32) -> FullScreenVertex {
    let mut uv = [0.0f32, 0.0f32];
    uv[0] = if id == 2 { 2.0 } else { 0.0 };
    uv[1] = if id == 1 { 2.0 } else { 0.0 };

    let position = [uv[0] * 2.0 + (-1.0), uv[1] * (-2.0) + 1.0, 1.0, 1.0];

    FullScreenVertex { position, uv }
}

pub const FULLSCREEN_VS_WGSL: &str = include_str!("shaders/fullscreen_vs.wgsl");
pub const FULLSCREEN_VS_ENTRY_POINT: &str = "fullscreen_vs";

#[cfg(test)]
mod tests;
