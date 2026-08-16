//! `RasterVS` (port card §2 Slice 3): RDP raster vertex-shader screen-scale
//! transform and fixed prim-depth Z override.
//!
//! Characterization-first literal port of RT64's `RasterVS`
//! (`src/shaders/RasterVS.hlsl:14-36`, pinned commit
//! `5473732a822a4423b5696e7cb18fecc425a59875` per
//! `docs/RT64-PORT-AUTHORITY.md`). Two independent behaviors, each gated by
//! its own condition:
//!
//! 1. **Screen scale/offset**: unless the draw is a rectangle
//!    (`renderFlagRect`, `shared/rt64_render_flags.h:52-54`, wire bit 0 of
//!    the render-flags word), the incoming vertex position is first
//!    converted from RDP screen-pixel coordinates to true `[-1,1]` clip-space
//!    NDC (`ndcPos.xy -= resolution/2; ndcPos.xy /= (resolution.x/2,
//!    -resolution.y/2); ndcPos.xyz *= ndcPos.w`), then RT64's own
//!    upscale/render-target `screenScale`/`screenOffset` push constant is
//!    applied on top, unconditionally of the rect flag.
//! 2. **Prim-depth Z override**: when the mode is not `G_CYC_COPY` and
//!    `zSource()` selects `G_ZS_PRIM`, the vertex's Z is replaced outright
//!    with `primDepth.x * ndcPos.w`, discarding whatever Z the vertex/NDC
//!    step produced.
//!
//! ## Boundary: this is not the RSP vertex transform
//!
//! This module is downstream of, and architecturally distinct from, the
//! RSP-side `guMtxCatF`/projection transform already ported at
//! `crates/fn64-render-reference/src/gbi/geometry.rs::project_vertex`
//! (lines 331-380 there). `project_vertex` implements libultra's MVP
//! transform plus viewport scale/translate, producing the **RDP's own final
//! screen-pixel coordinates** (`px`/`py`/`pz` in pixels, matching N64
//! hardware's top-down-Y screen space) from model-space input -- that stage
//! runs once per vertex, on the CPU/RSP side, before the RDP ever sees a
//! triangle.
//!
//! `RasterVS` starts from those already-computed RDP screen-pixel
//! coordinates (RT64's `iPosition`, confusingly named `ndcPos` in the HLSL
//! despite not yet being NDC when the function begins) and re-projects them
//! into the *host GPU's* `[-1,1]` clip-space convention so wgpu's rasterizer
//! can consume them, then layers RT64's own render-target upscale/offset
//! push constant on top. It is a GPU vertex-shader stage that exists only
//! because RT64 renders through a modern GPU pipeline instead of a
//! software RDP rasterizer; it has no counterpart in real N64 hardware and
//! no counterpart in `project_vertex`, which already finished the
//! libultra-defined transform before this stage ever runs.
//!
//! `fn64-render-wgpu` has no crate dependency on `fn64-render-reference`
//! (see `depth_strict_less.rs`), so this is a self-contained literal
//! re-expression citing the reference's line numbers for the boundary
//! description above, matching this crate's existing citation-comment
//! convention.
//!
//! ## Scope
//!
//! In scope: the exact screen-space transform arithmetic (rect-skip branch,
//! screen scale/offset), and the exact copy-mode/`zSource` Z-override
//! branch, as a pure CPU function plus a WGSL differential sibling
//! ([`RASTER_VS_WGSL`]). Explicitly out of scope: `oUV`/`oSmoothColor`/
//! `oFlatColor` passthrough (RT64 copies these fields unchanged from input
//! to output; there is no arithmetic to port), vertex buffer layout,
//! pipeline/bind-group wiring, actual triangle rasterization, any
//! `targets/raster.rs` integration, and any RT64 parity or performance
//! claim.

use crate::state::{CycleType, OtherMode, PrimDepth};

/// One RDP-rasterizer vertex position as `RasterVS` receives it: RDP
/// screen-pixel `x`/`y`, RDP depth `z`, and clip-space `w` (already produced
/// by the upstream RSP transform -- see module doc). Matches the HLSL's
/// `float4 iPosition`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RasterVsPosition {
    pub x: f32,
    pub y: f32,
    pub z: f32,
    pub w: f32,
}

/// `FbParams.resolution` (`shared/rt64_framebuffer_params.h:13`): the
/// framebuffer's pixel dimensions, used to convert RDP screen-pixel
/// coordinates into `[-1,1]` NDC.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Resolution {
    pub width: f32,
    pub height: f32,
}

/// `RasterParams.screenScale`/`screenOffset`
/// (`shared/rt64_raster_params.h:15-16`): RT64's own render-target
/// upscale/window push constant, applied after the NDC conversion.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ScreenTransform {
    pub scale: [f32; 2],
    pub offset: [f32; 2],
}

/// Mode-dependent inputs `RasterVS` needs beyond the vertex position and
/// screen transform: whether this draw is a rectangle (`renderFlagRect`),
/// the decoded `OtherMode` (for `cycleType()`/`zSource()`), and the
/// primitive depth register consulted only when the Z-override branch
/// fires.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RasterVsParams {
    pub is_rect: bool,
    pub other_mode: OtherMode,
    pub prim_depth: PrimDepth,
}

/// Literal port of `RasterVS`'s position-output computation
/// (`src/shaders/RasterVS.hlsl:15-33`). Returns the transformed `x`/`y`/`z`/`w`
/// RT64 writes to `oPosition`; `oUV`/`oSmoothColor`/`oFlatColor` are
/// unconditional passthrough (see module doc "Scope") and are not
/// represented here.
///
/// - `params.is_rect` skips the RDP-screen-to-NDC conversion entirely
///   (`renderFlagRect` short-circuit, HLSL line 18); `screen`'s
///   scale/offset still applies afterward regardless of `is_rect`.
/// - The Z-override fires when `cycleType() != Copy` AND
///   `zSource() == G_ZS_PRIM`; it replaces `z` with
///   `prim_depth.z_normalized() * w` using the *final* `w` (post
///   scale/offset -- `screenScale`/`screenOffset` never touch `w`, so this
///   is the same `w` as the input `position.w` whenever `is_rect` is
///   `false` XOR skipped, and always the same value the NDC step would
///   have produced, since that step only multiplies `x`/`y`/`z` by `w`,
///   never reassigning it).
pub fn raster_vs(
    position: RasterVsPosition,
    resolution: Resolution,
    screen: ScreenTransform,
    params: RasterVsParams,
) -> RasterVsPosition {
    let mut x = position.x;
    let mut y = position.y;
    let mut z = position.z;
    let w = position.w;

    if !params.is_rect {
        x -= resolution.width / 2.0;
        y -= resolution.height / 2.0;
        x /= resolution.width / 2.0;
        y /= resolution.height / -2.0;
        x *= w;
        y *= w;
        z *= w;
    }

    x = (x * screen.scale[0]) + screen.offset[0] * w;
    y = (y * screen.scale[1]) + screen.offset[1] * w;

    let copy_mode = params.other_mode.cycle_type() == CycleType::Copy;
    let z_source_prim = params.other_mode.primitive_depth_source();
    if !copy_mode && z_source_prim {
        z = params.prim_depth.z_normalized() * w;
    }

    RasterVsPosition { x, y, z, w }
}

pub const RASTER_VS_WGSL: &str = include_str!("shaders/raster_vs.wgsl");
pub const RASTER_VS_ENTRY_POINT: &str = "raster_vs";

#[cfg(test)]
mod tests;
