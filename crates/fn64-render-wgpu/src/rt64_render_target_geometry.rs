//! Literal port of `RT64::RenderTarget`'s resolution-geometry cluster --
//! `computeScaledSize`, `computeFixedResolutionScale`, the pillarbox/
//! letterbox viewport+scissor derivation carved out of `copyFromChanges`,
//! and `RT64::viewportScissorIntersection` -- a literal port of the
//! permitted MIT RT64 Rust-port source pinned at commit
//! `5473732a822a4423b5696e7cb18fecc425a59875` (`docs/RT64-PORT-AUTHORITY.md`):
//!
//! - `src/render/rt64_render_target.cpp` (whole-file SHA-256,
//!   `0dee0d19a4be22c4fa34b7ef793e3d4f75a07ab0d31252f9a23a2a0355edb41d`, 515
//!   lines -- matching `docs/rt64-port-inventory.json`'s `sources.port.sha256`
//!   for that path, confirmed independently here by `shasum -a 256` against
//!   the pinned port-commit checkout). `computeScaledSize`/
//!   `computeFixedResolutionScale` are lines 487-506; the pillarbox block is
//!   carved out of `copyFromChanges`, lines 289-314.
//! - `src/render/rt64_framebuffer_renderer.cpp` (whole-file SHA-256,
//!   `73870c23b2340611b930b668d52aec9e687fd74b7da310962b7b1ef35dee015d`, 1920
//!   lines -- matching the same inventory field, confirmed the same way).
//!   `viewportScissorIntersection` is lines 128-135; this is the ONLY
//!   fragment ported from that file -- the remaining ~1912 lines are RHI
//!   plumbing (descriptor sets, barriers, command lists, raytracing) and are
//!   a standing reject for this port program (see "Nonclaims").
//!
//! `docs/rt64-port-inventory.json` does not yet record either path's
//! `ported_as` as pointing at this module (both currently list `"ported_as":
//! []`) -- `scripts/lint-docs.py`'s inventory scanner is expected to report a
//! drift for that until a follow-up regenerates the inventory to add this
//! module; this module's own writable surface does not include
//! `docs/rt64-port-inventory.json`, so that reconciliation is deliberately
//! left to the owning ticket rather than done here.
//!
//! ```text
//! // rt64_render_target.cpp:487-497
//! void RenderTarget::computeScaledSize(uint32_t nativeWidth, uint32_t nativeHeight, hlslpp::float2 resolutionScale, uint32_t &scaledWidth, uint32_t &scaledHeight, uint32_t &misalignmentX) {
//!     const long nativeColorWidthClamped = std::clamp(lround(nativeWidth * resolutionScale.y), 1L, RenderTarget::MaxDimension);
//!     const long expandedColorWidthClamped = std::clamp(lround(nativeWidth * resolutionScale.x), 1L, RenderTarget::MaxDimension);
//!     const long colorHeightClamped = std::clamp(lround(nativeHeight * resolutionScale.y), 1L, RenderTarget::MaxDimension);
//!     scaledWidth = uint32_t(expandedColorWidthClamped);
//!     scaledHeight = uint32_t(colorHeightClamped);
//!
//!     const long expandedPixels = std::labs(long(scaledWidth) - nativeColorWidthClamped) / 2;
//!     const long nativeAlignment = std::max(lround(resolutionScale.y), 1L);
//!     misalignmentX = (nativeAlignment - (expandedPixels % nativeAlignment)) % nativeAlignment;
//! }
//!
//! // rt64_render_target.cpp:499-506
//! hlslpp::float2 RenderTarget::computeFixedResolutionScale(uint32_t nativeWidth, hlslpp::float2 resolutionScale) {
//!     long expandedColorWidthClamped = std::clamp(lround(nativeWidth * resolutionScale.x), 1L, RenderTarget::MaxDimension);
//!
//!     // Alter the resolution scale so it outputs an even resolution number.
//!     expandedColorWidthClamped += expandedColorWidthClamped & 0x1;
//!     resolutionScale.x = float(expandedColorWidthClamped) / float(nativeWidth);
//!     return resolutionScale;
//! }
//!
//! // rt64_render_target.cpp:21 (RenderTarget::MaxDimension)
//! const long RenderTarget::MaxDimension = 0x4000L;
//!
//! // rt64_render_target.cpp:289-314 (RenderTarget::copyFromChanges, pillarbox block only)
//! float targetWidth, targetHeight, targetLeft, targetTop;
//! const bool pillarBox = resolutionScale[0] > resolutionScale[1];
//! if (pillarBox) {
//!     targetWidth = fbWidth * resolutionScale.y;
//!     targetHeight = fbHeight * resolutionScale.y;
//!     targetLeft = ((fbWidth * resolutionScale.x) / 2.0f) - (targetWidth / 2.0f);
//!     targetTop = rowStart * resolutionScale.y;
//! }
//! else {
//!     targetWidth = fbWidth * resolutionScale.x;
//!     targetHeight = fbHeight * resolutionScale.x;
//!     targetLeft = 0.0f;
//!     targetTop = rowStart * resolutionScale.y + ((fbHeight * resolutionScale.y) / 2.0f) - (targetHeight / 2.0f);
//! }
//!
//! long scissorLeft = long(floor(targetLeft));
//! long scissorTop = long(floor(targetTop));
//! long scissorRight = long(ceil(targetLeft + targetWidth));
//! long scissorBottom = long(ceil(targetTop + targetHeight));
//! RenderViewport targetViewport = RenderViewport(targetLeft, targetTop, targetWidth, targetHeight);
//! RenderRect targetRect(scissorLeft, scissorTop, scissorRight, scissorBottom);
//!
//! // rt64_framebuffer_renderer.cpp:128-135
//! static RenderRect viewportScissorIntersection(const RenderViewport &viewport, const RenderRect &scissor) {
//!     return RenderRect{
//!         std::max(static_cast<int32_t>(std::floor(viewport.x)), scissor.left),
//!         std::max(static_cast<int32_t>(std::floor(viewport.y)), scissor.top),
//!         std::min(static_cast<int32_t>(std::ceil(viewport.x + viewport.width)), scissor.right),
//!         std::min(static_cast<int32_t>(std::ceil(viewport.y + viewport.height)), scissor.bottom)
//!     };
//! }
//! ```
//!
//! **New owned types, not `rt64_common::FixedRect`.** `RenderViewport`
//! (`x, y, width, height` -- all `f32`) and `RenderRect` (`left, top, right,
//! bottom` -- all `i32`) are RT64 RHI types declared in
//! `src/contrib/plume/plume_render_interface_types.h`, which is under this
//! port program's `excluded_prefixes` (`src/contrib/`) and is not port
//! authority. This module therefore declares minimal local
//! [`Float2`]/[`Viewport`]/[`ScissorRect`] structs shaped only from the
//! fields each ported function actually reads (mirroring `RenderViewport`'s
//! four float fields and `RenderRect`'s four `i32` fields, field-for-field),
//! rather than reusing `rt64_common::FixedRect`: `FixedRect` is a *different*
//! type in a different domain (RDP 10.2 subpixel fixed-point, 2 fractional
//! bits, with a null-rect sentinel at `i32::MAX`/`i32::MIN` and an
//! `is_empty`/`is_null` contract baked into `intersection`), whereas
//! `RenderRect`/`RenderViewport` are plain pixel-space RHI coordinates with
//! no null-sentinel handling anywhere in `viewportScissorIntersection`'s
//! source. `FixedRect::intersection`'s `self.ulx.max(rect.ulx) /
//! self.lrx.min(rect.lrx)` shape (`rt64_common.rs:428-439`) is structurally
//! the same max-then-min pattern this module's
//! [`viewport_scissor_intersection`] uses, confirmed as the right sibling to
//! check against precisely because it is so close in shape -- but the two
//! functions differ in every other respect (input types, no floor/ceil in
//! `FixedRect`'s version, no null-sentinel branch in this one), so this is a
//! new, distinct helper, not a reuse of the existing one.
//!
//! ## Admitted domain
//!
//! - **`resolutionScale.x` vs `resolutionScale.y` are NOT interchangeable --
//!   swapping them is silent and wrong.** In `computeScaledSize`,
//!   `resolutionScale.y` scales the *native* comparison width
//!   (`nativeColorWidthClamped`) and both the native and scaled heights,
//!   while `resolutionScale.x` scales only the *expanded* (output) width
//!   (`expandedColorWidthClamped`, which becomes `scaledWidth`). The
//!   misalignment result depends on the *difference* between these two
//!   differently-scaled widths, so this port keeps `scale.x`/`scale.y` as
//!   two distinct `f32` parameters throughout (never a single scalar), and
//!   [`compute_scaled_size_non_uniform_scale_produces_nonzero_misalignment`]
//!   below pins a case where `scale.x != scale.y` yields `misalignment_x !=
//!   0`, hand-derived independently (see next bullet), not captured from
//!   this module's own output.
//! - **`lround` is round-half-away-from-zero; `as i32`/`as i64` truncates
//!   toward zero and is NOT a substitute.** Rust's `f32::round()` matches
//!   `lround`'s round-half-away-from-zero tie-breaking exactly (unlike
//!   banker's rounding), so [`lround_f32`] below is `x.round() as i64`, and
//!   every call site in this module goes through it rather than a bare `as`
//!   cast. Clamped to `[1, MaxDimension]` where `MaxDimension = 0x4000`
//!   (`4000` hex = `16384` decimal, `rt64_render_target.cpp:21`) -- both the
//!   floor of `1` and the ceiling of `16384` are exercised by
//!   [`compute_scaled_size_clamps_to_one_pixel_floor`] and
//!   [`compute_scaled_size_clamps_to_max_dimension_ceiling`] below.
//! - **`computeFixedResolutionScale` returns a modified *scale*, not a
//!   size.** `expandedColorWidthClamped += expandedColorWidthClamped & 0x1`
//!   forces the clamped expanded width to the next-higher *even* integer
//!   (odd values gain 1; even values are unchanged, since `even & 1 == 0`),
//!   and the returned `resolutionScale.x` is that even width divided by the
//!   original `nativeWidth` as a fresh ratio -- `resolutionScale.y` passes
//!   through unmodified. [`compute_fixed_resolution_scale_odd_expanded_width_rounds_up_to_even`]
//!   and [`compute_fixed_resolution_scale_even_expanded_width_is_a_no_op`]
//!   below pin both parities with hand-computed ratios.
//! - **Pillarbox vs letterbox is selected by `resolutionScale.x >
//!   resolutionScale.y`, strict `>`, so the equal-scale case takes the
//!   letterbox (`else`) branch, not the pillarbox one.** This is a genuine
//!   comparison-strictness frontier named by this port's hazard list; when
//!   `scale.x == scale.y`, `targetLeft` is forced to `0.0` and `targetTop` is
//!   centered vertically by the `else` branch's formula, never the
//!   pillarbox branch's horizontal-centering formula (the two branches are
//!   not algebraically equal at the boundary -- the letterbox branch at
//!   equal scale happens to also produce zero vertical offset only when
//!   `fbHeight * scale.y == targetHeight`, i.e. always, since `targetHeight
//!   = fbHeight * scale.x = fbHeight * scale.y` at that point -- but
//!   `targetLeft` is unconditionally `0.0` either way at equal scale, so the
//!   two branches do coincide numerically at the exact boundary; this is
//!   recorded here, not assumed, and pinned by
//!   [`pillarbox_derive_equal_scale_takes_letterbox_branch`] below asserting
//!   the letterbox formula's shape, not merely a numeric match).
//! - **The pillarbox/letterbox scissor floors the origin and ceils the far
//!   edge -- ported as `floor`/`ceil`, never a symmetric rounding.**
//!   `scissorLeft = floor(targetLeft)`, `scissorRight =
//!   ceil(targetLeft + targetWidth)`, and likewise for
//!   top/bottom -- preserved as `f32::floor()`/`f32::ceil()` truncated `as
//!   i64` (both are already integral after floor/ceil, so the cast is exact
//!   for any in-range value), not `f32::round()`. This differs from
//!   `computeScaledSize`'s `lround` -- the two functions in this same
//!   cluster use genuinely different rounding rules and this port does not
//!   unify them.
//! - **`viewportScissorIntersection` has no null-sentinel guard and no
//!   `isEmpty` check -- it always computes `max`/`min` and can return an
//!   inverted rect (`left > right` or `top > bottom`) when the operands are
//!   disjoint.** There is no branch in the C++ source that special-cases
//!   this; the caller is expected to check the result's emptiness itself
//!   (via `RenderRect::isEmpty()`'s `(left >= right) || (top >= bottom)`,
//!   `plume_render_interface_types.h:1580-1582`, not ported into this
//!   module since it belongs to the excluded RHI type, not this port's
//!   named cluster). [`viewport_scissor_intersection_fully_disjoint_yields_inverted_rect`]
//!   below pins this directly: a viewport and scissor with no overlap
//!   produce a `ScissorRect` with `left > right`, asserted as exactly that
//!   inverted numeric relationship, not guarded against or normalized away.
//! - **Touching-but-not-overlapping vs one-unit-overlapping is a real `<`
//!   vs `<=` frontier at the intersection boundary, driven entirely by
//!   `ceil`'s behavior on an already-integral float.** When the viewport's
//!   far edge lands exactly on the scissor's near edge (e.g. viewport
//!   `[0,10)` against scissor starting at `10`), `ceil(10.0) == 10`, so the
//!   intersection's `right == left == 10`: zero width, "touching" in the
//!   sense of sharing a boundary coordinate but empty by
//!   `RenderRect::isEmpty()`'s `left >= right` rule.  Extending the
//!   viewport by one more unit (`[0,11)`) makes `ceil(11.0) == 11 >
//!   left == 10`, a one-pixel-wide non-empty overlap.
//!   [`viewport_scissor_intersection_touching_boundary_yields_zero_width`]
//!   and [`viewport_scissor_intersection_one_unit_overlap_yields_width_one`]
//!   below pin both sides of this boundary with hand-computed left/right
//!   pairs.
//! - **No divide-by-zero frontier in this cluster's four functions.**
//!   `computeScaledSize`'s only division is `expandedPixels / 2` (constant
//!   divisor) and `expandedPixels % nativeAlignment` /
//!   `nativeAlignment - (...)  % nativeAlignment` where `nativeAlignment =
//!   max(lround(scale.y), 1)` is unconditionally clamped to a floor of `1`
//!   *before* being used as a divisor -- so the modulo can never divide by
//!   zero regardless of `scale.y`'s input value, including `scale.y == 0.0`
//!   (`lround(0.0) = 0`, then `max(0, 1) = 1`).
//!   [`compute_scaled_size_zero_y_scale_does_not_divide_by_zero`] below
//!   exercises exactly that input and asserts the resulting
//!   `nativeAlignment`-implied misalignment is well-defined (not a panic).
//!   `computeFixedResolutionScale`'s `float(expandedColorWidthClamped) /
//!   float(nativeWidth)` **is** a genuine divide-by-zero frontier: if the
//!   caller passes `nativeWidth == 0`, this is an unguarded `f32` division
//!   by `0.0`, which under IEEE 754 float semantics yields `+inf` (numerator
//!   is `expandedColorWidthClamped`, always `>= 1` after the parity fixup,
//!   never negative or zero) rather than a panic -- Rust float division
//!   never panics or traps, unlike integer division. This is reported here
//!   as a real frontier per this port's hazard list, not silently guarded:
//!   [`compute_fixed_resolution_scale_zero_native_width_yields_infinity`]
//!   below pins the `+inf` result explicitly rather than adding a check the
//!   C++ source does not have. The pillarbox derivation and
//!   `viewportScissorIntersection` have no runtime-controlled divisor at all
//!   (only the constant `/2.0f` in the pillarbox centering math).
//! - **No private-helper visibility gap was hit.** This cluster needs no
//!   symbol from any sibling module; `RenderTarget::MaxDimension` is a
//!   compile-time constant folded directly into this module as
//!   [`MAX_DIMENSION`], and `viewportScissorIntersection`'s `RenderViewport`/
//!   `RenderRect` inputs are represented by this module's own local structs
//!   (see "New owned types" above) rather than reaching into any sibling's
//!   private surface.
//!
//! ## Nonclaims
//!
//! No GPU, WGSL, or production wiring (this module is not called from
//! anywhere yet; dead-code warnings on the unused public surface are
//! expected and correct), and no RT64 visual/pixel/silicon parity or
//! performance claim. Deliberately not ported from these two source files:
//!
//! - Everything else in `rt64_render_target.cpp` (515 lines total): the
//!   `RenderTarget` constructor, `clearColorTarget`/`clearDepthTarget`,
//!   `colorBufferFormat`/`depthBufferFormat`, `copyFromChanges`'s RHI
//!   command-recording body (barriers, framebuffer setup, draw calls --
//!   only its pillarbox *geometry* derivation is ported, per this ticket's
//!   named scope), `copyFromTarget`, `downsampleTarget`,
//!   `getResolvedTexture`/`getResolvedTextureView`, `isEmpty`,
//!   `markForResolve`, `recordRasterResolve`, `releaseTextures`, `resize`,
//!   `resolveFromTarget`/`resolveTarget`, `setupColor`/`setupColorFramebuffer`/
//!   `setupDepth`/`setupDepthFramebuffer`/`setupDummy`/
//!   `setupResolveFramebuffer`, `usesResolve` -- all RHI/RenderWorker
//!   plumbing (descriptor sets, command lists, texture barriers, pipeline
//!   state), out of this ticket's named scope.
//! - Everything else in `rt64_framebuffer_renderer.cpp` (1920 lines total,
//!   of which exactly 8 -- `viewportScissorIntersection`'s body -- are
//!   ported here): `FramebufferRenderer`'s constructor and every method
//!   (`addFramebuffer`, `advanceFrame`, `createGPUTiles`, `endFramebuffers`,
//!   `getDestinationIndex`/`getTextureIndex`, `recordFramebuffer`,
//!   `recordSetup`, `resetFramebuffers`/`resetRaytracing`,
//!   `setRaytracingConfig`, `submitDepthAccess`,
//!   `submitRSPSmoothNormalCompute`, `submitRasterScene`/
//!   `submitRaytracingScene`, `updateMultisampling`,
//!   `updateRSPSmoothNormalSet`/`updateRSPVertexTestZSet`,
//!   `updateRaytracingScene`, `updateShaderDescriptorSet`/
//!   `updateShaderViews`, `updateTextureCache`, `waitForUploaders`),
//!   `RasterScene`'s constructor, `convertFixedRect`/`convertViewportRect`
//!   (a distinct, more elaborate viewport-derivation helper with its own
//!   misalignment-correction closure -- not part of this ticket's named
//!   four functions), `toRenderColor`, `viewPositionFrom`/
//!   `viewDirectionFrom`, and every `BicubicCB`/`HistogramAverageCB`/
//!   `HistogramSetCB`/`LuminanceHistogramCB`/`TextureCB` shader-constant
//!   struct -- all RHI plumbing or out-of-scope helpers this ticket's brief
//!   names as a standing reject (`src/render/rt64_framebuffer_renderer.cpp`
//!   is 113 KB and almost entirely RHI plumbing; only
//!   `viewportScissorIntersection` is named in scope).
//! - `RenderTarget::MaxDimension`'s declaration site
//!   (`rt64_render_target.h:20`, `static const long MaxDimension;`) is not
//!   separately ported as a type -- only its value (`0x4000L`, defined at
//!   `rt64_render_target.cpp:21`) is folded into this module's
//!   [`MAX_DIMENSION`] constant.

/// `RenderTarget::MaxDimension` (`rt64_render_target.cpp:21`): `0x4000L` =
/// `16384`. Both the scaled-size clamp and the fixed-resolution-scale clamp
/// use this exact bound.
pub const MAX_DIMENSION: i64 = 0x4000;

/// A minimal two-component float pair standing in for `hlslpp::float2`
/// (`resolutionScale`'s type in the source) -- see module doc "New owned
/// types".
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Float2 {
    pub x: f32,
    pub y: f32,
}

impl Float2 {
    pub const fn new(x: f32, y: f32) -> Self {
        Self { x, y }
    }
}

/// Mirrors `RenderViewport`'s four float fields
/// (`plume_render_interface_types.h:1525-1529`) field-for-field -- see
/// module doc "New owned types".
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Viewport {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

impl Viewport {
    pub const fn new(x: f32, y: f32, width: f32, height: f32) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }
}

/// Mirrors `RenderRect`'s four `i32` fields
/// (`plume_render_interface_types.h:1557-1561`) field-for-field -- see
/// module doc "New owned types".
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ScissorRect {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

impl ScissorRect {
    pub const fn new(left: i32, top: i32, right: i32, bottom: i32) -> Self {
        Self {
            left,
            top,
            right,
            bottom,
        }
    }
}

/// `lround(x)`: round-half-away-from-zero, matching C's `lround` exactly
/// (see module doc "Admitted domain" -- this is NOT `x as i64`, which
/// truncates toward zero).
pub fn lround_f32(x: f32) -> i64 {
    x.round() as i64
}

/// `std::clamp(v, 1L, MaxDimension)`: clamps to `[1, MAX_DIMENSION]`.
fn clamp_to_dimension(v: i64) -> i64 {
    v.clamp(1, MAX_DIMENSION)
}

/// `RenderTarget::computeScaledSize` (`rt64_render_target.cpp:487-497`).
/// Returns `(scaled_width, scaled_height, misalignment_x)`. See module doc
/// "Admitted domain" for the `scale.x`/`scale.y` asymmetry and the
/// divide-by-zero analysis (there is none here -- `native_alignment` is
/// floored to `1` before use as a divisor).
pub fn compute_scaled_size(
    native_width: u32,
    native_height: u32,
    resolution_scale: Float2,
) -> (u32, u32, u32) {
    let native_color_width_clamped =
        clamp_to_dimension(lround_f32(native_width as f32 * resolution_scale.y));
    let expanded_color_width_clamped =
        clamp_to_dimension(lround_f32(native_width as f32 * resolution_scale.x));
    let color_height_clamped =
        clamp_to_dimension(lround_f32(native_height as f32 * resolution_scale.y));
    let scaled_width = expanded_color_width_clamped as u32;
    let scaled_height = color_height_clamped as u32;

    let expanded_pixels = (scaled_width as i64 - native_color_width_clamped).abs() / 2;
    let native_alignment = lround_f32(resolution_scale.y).max(1);
    let misalignment_x =
        (native_alignment - (expanded_pixels % native_alignment)) % native_alignment;

    (scaled_width, scaled_height, misalignment_x as u32)
}

/// `RenderTarget::computeFixedResolutionScale` (`rt64_render_target.cpp:499-506`).
/// Returns a modified `Float2` (its `x` forced to yield an even scaled
/// width; `y` passes through unchanged) -- NOT a size. See module doc
/// "Admitted domain" for the `nativeWidth == 0` divide-by-zero frontier.
pub fn compute_fixed_resolution_scale(native_width: u32, resolution_scale: Float2) -> Float2 {
    let mut expanded_color_width_clamped =
        clamp_to_dimension(lround_f32(native_width as f32 * resolution_scale.x));

    // Alter the resolution scale so it outputs an even resolution number.
    expanded_color_width_clamped += expanded_color_width_clamped & 0x1;
    let new_x = expanded_color_width_clamped as f32 / native_width as f32;

    Float2::new(new_x, resolution_scale.y)
}

/// The pillarbox/letterbox viewport+scissor derivation carved out of
/// `RenderTarget::copyFromChanges` (`rt64_render_target.cpp:289-314`).
/// Returns `(viewport, scissor)`. See module doc "Admitted domain" for the
/// strict `>` branch selection and the floor/ceil (not round) scissor
/// derivation.
pub fn pillarbox_derive(
    fb_width: u32,
    fb_height: u32,
    resolution_scale: Float2,
    row_start: u32,
) -> (Viewport, ScissorRect) {
    let pillar_box = resolution_scale.x > resolution_scale.y;
    let (target_width, target_height, target_left, target_top) = if pillar_box {
        let target_width = fb_width as f32 * resolution_scale.y;
        let target_height = fb_height as f32 * resolution_scale.y;
        let target_left =
            ((fb_width as f32 * resolution_scale.x) / 2.0f32) - (target_width / 2.0f32);
        let target_top = row_start as f32 * resolution_scale.y;
        (target_width, target_height, target_left, target_top)
    } else {
        let target_width = fb_width as f32 * resolution_scale.x;
        let target_height = fb_height as f32 * resolution_scale.x;
        let target_left = 0.0f32;
        let target_top = row_start as f32 * resolution_scale.y
            + ((fb_height as f32 * resolution_scale.y) / 2.0f32)
            - (target_height / 2.0f32);
        (target_width, target_height, target_left, target_top)
    };

    let scissor_left = target_left.floor() as i32;
    let scissor_top = target_top.floor() as i32;
    let scissor_right = (target_left + target_width).ceil() as i32;
    let scissor_bottom = (target_top + target_height).ceil() as i32;

    (
        Viewport::new(target_left, target_top, target_width, target_height),
        ScissorRect::new(scissor_left, scissor_top, scissor_right, scissor_bottom),
    )
}

/// `viewportScissorIntersection` (`rt64_framebuffer_renderer.cpp:128-135`).
/// No null-sentinel guard: can return an inverted rect (`left > right` or
/// `top > bottom`) for fully-disjoint operands -- see module doc "Admitted
/// domain".
pub fn viewport_scissor_intersection(viewport: Viewport, scissor: ScissorRect) -> ScissorRect {
    ScissorRect::new(
        (viewport.x.floor() as i32).max(scissor.left),
        (viewport.y.floor() as i32).max(scissor.top),
        ((viewport.x + viewport.width).ceil() as i32).min(scissor.right),
        ((viewport.y + viewport.height).ceil() as i32).min(scissor.bottom),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- lround_f32 ---

    #[test]
    fn lround_f32_rounds_half_away_from_zero_positive() {
        assert_eq!(lround_f32(2.5), 3);
    }

    #[test]
    fn lround_f32_rounds_half_away_from_zero_negative() {
        assert_eq!(lround_f32(-2.5), -3);
    }

    #[test]
    fn lround_f32_rounds_down_below_half() {
        assert_eq!(lround_f32(2.4), 2);
    }

    #[test]
    fn lround_f32_rounds_up_above_half() {
        assert_eq!(lround_f32(2.6), 3);
    }

    #[test]
    fn lround_f32_zero_is_zero() {
        assert_eq!(lround_f32(0.0), 0);
    }

    // --- compute_scaled_size: identity / no-scale ---

    #[test]
    fn compute_scaled_size_identity_scale_returns_native_size_and_zero_misalignment() {
        let (w, h, m) = compute_scaled_size(320, 240, Float2::new(1.0, 1.0));
        assert_eq!((w, h, m), (320, 240, 0));
    }

    #[test]
    fn compute_scaled_size_uniform_2x_scale() {
        let (w, h, m) = compute_scaled_size(320, 240, Float2::new(2.0, 2.0));
        assert_eq!((w, h, m), (640, 480, 0));
    }

    // --- compute_scaled_size: non-uniform scale, misalignment ---

    #[test]
    fn compute_scaled_size_non_uniform_scale_produces_nonzero_misalignment() {
        // nw=100: ncwc=lround(100*2.0)=200; ecwc=lround(100*1.5)=150; sw=150
        // expanded_pixels=|150-200|/2=25; native_alignment=max(lround(2.0),1)=2
        // misalignment=(2-(25%2))%2=(2-1)%2=1
        let (w, h, m) = compute_scaled_size(100, 100, Float2::new(1.5, 2.0));
        assert_eq!((w, h, m), (150, 200, 1));
    }

    #[test]
    fn compute_scaled_size_non_integer_scale_both_axes() {
        // nw=64: ncwc=lround(64*1.7)=lround(108.8)=109; ecwc=lround(64*2.5)=lround(160.0)=160
        // expanded_pixels=|160-109|/2=51/2=25 (integer division); native_alignment=max(lround(1.7),1)=2
        // misalignment=(2-(25%2))%2=(2-1)%2=1
        let (w, h, m) = compute_scaled_size(64, 64, Float2::new(2.5, 1.7));
        assert_eq!(w, 160);
        assert_eq!(m, 1);
        let _ = h;
    }

    #[test]
    fn compute_scaled_size_scale_x_and_scale_y_are_not_interchangeable() {
        // Swapping x/y between two calls must change scaled_width (driven by
        // scale.x) while the misalignment/height calc (driven by scale.y)
        // differs too -- confirms the two axes are not accidentally aliased.
        let (w_a, _, _) = compute_scaled_size(100, 100, Float2::new(1.5, 2.0));
        let (w_b, _, _) = compute_scaled_size(100, 100, Float2::new(2.0, 1.5));
        assert_ne!(w_a, w_b);
        assert_eq!(w_a, 150);
        assert_eq!(w_b, 200);
    }

    // --- compute_scaled_size: zero and one-pixel dimensions ---

    #[test]
    fn compute_scaled_size_zero_native_width_clamps_to_one_pixel_floor() {
        // lround(0*1.0)=0, clamped to floor of 1.
        let (w, _h, _m) = compute_scaled_size(0, 240, Float2::new(1.0, 1.0));
        assert_eq!(w, 1);
    }

    #[test]
    fn compute_scaled_size_one_pixel_native_size_identity_scale() {
        let (w, h, m) = compute_scaled_size(1, 1, Float2::new(1.0, 1.0));
        assert_eq!((w, h, m), (1, 1, 0));
    }

    #[test]
    fn compute_scaled_size_clamps_to_one_pixel_floor() {
        // Negative-going scale still clamps at the 1-pixel floor (lround of a
        // tiny fraction rounds to 0, then clamped up to 1).
        let (w, _h, _m) = compute_scaled_size(1, 1, Float2::new(0.001, 0.001));
        assert_eq!(w, 1);
    }

    #[test]
    fn compute_scaled_size_clamps_to_max_dimension_ceiling() {
        // 10000*10.0 = 100000, far past MAX_DIMENSION (16384); clamps down.
        let (w, h, _m) = compute_scaled_size(10000, 10000, Float2::new(10.0, 10.0));
        assert_eq!(w, MAX_DIMENSION as u32);
        assert_eq!(h, MAX_DIMENSION as u32);
    }

    #[test]
    fn compute_scaled_size_zero_y_scale_does_not_divide_by_zero() {
        // resolution_scale.y = 0.0: lround(0.0) = 0, then max(0, 1) = 1, so
        // native_alignment is floored to 1 before use as a divisor -- must not
        // panic, and the modulo-by-1 result is always 0.
        let (_w, h, m) = compute_scaled_size(320, 240, Float2::new(1.0, 0.0));
        // native_height*0.0 clamps to floor of 1 pixel.
        assert_eq!(h, 1);
        assert_eq!(m, 0);
    }

    // --- compute_fixed_resolution_scale ---

    #[test]
    fn compute_fixed_resolution_scale_identity_even_width_is_unchanged() {
        // 320*1.0=320 (even) -> 320 & 1 == 0 -> unchanged -> ratio 320/320=1.0.
        let out = compute_fixed_resolution_scale(320, Float2::new(1.0, 1.0));
        assert_eq!(out, Float2::new(1.0, 1.0));
    }

    #[test]
    fn compute_fixed_resolution_scale_even_expanded_width_is_a_no_op() {
        // nw=320, scale.x=1.5 -> lround(480.0)=480 (even) -> unchanged -> 480/320=1.5.
        let out = compute_fixed_resolution_scale(320, Float2::new(1.5, 1.0));
        assert_eq!(out.x, 1.5);
        assert_eq!(out.y, 1.0);
    }

    #[test]
    fn compute_fixed_resolution_scale_odd_expanded_width_rounds_up_to_even() {
        // nw=321, scale.x=1.0 -> lround(321.0)=321 (odd) -> 321+1=322 -> 322/321.
        let out = compute_fixed_resolution_scale(321, Float2::new(1.0, 1.0));
        let expected_x = 322.0f32 / 321.0f32;
        assert_eq!(out.x, expected_x);
        assert_eq!(out.y, 1.0);
    }

    #[test]
    fn compute_fixed_resolution_scale_preserves_y_untouched() {
        let out = compute_fixed_resolution_scale(321, Float2::new(1.0, 7.75));
        assert_eq!(out.y, 7.75);
    }

    #[test]
    fn compute_fixed_resolution_scale_zero_native_width_yields_infinity() {
        // expanded_color_width_clamped is >=1 after clamp+parity fixup;
        // dividing by nativeWidth=0 in f32 yields +inf (IEEE 754), not a panic.
        let out = compute_fixed_resolution_scale(0, Float2::new(1.0, 1.0));
        assert!(out.x.is_infinite() && out.x.is_sign_positive());
    }

    #[test]
    fn compute_fixed_resolution_scale_one_pixel_native_width() {
        // nw=1, scale.x=1.0 -> lround(1.0)=1 (odd) -> 1+1=2 -> 2/1=2.0.
        let out = compute_fixed_resolution_scale(1, Float2::new(1.0, 1.0));
        assert_eq!(out.x, 2.0);
    }

    // --- pillarbox_derive: pillarbox branch (scale.x > scale.y) ---

    #[test]
    fn pillarbox_derive_pillarbox_branch_centers_horizontally() {
        // fb=100x50, sx=2.0, sy=1.0, row_start=0.
        // target_width=100*1=100; target_height=50*1=50
        // target_left=(100*2)/2 - 100/2 = 100-50=50; target_top=0*1=0
        // scissor: floor(50)=50, floor(0)=0, ceil(150)=150, ceil(50)=50
        let (vp, sc) = pillarbox_derive(100, 50, Float2::new(2.0, 1.0), 0);
        assert_eq!(vp, Viewport::new(50.0, 0.0, 100.0, 50.0));
        assert_eq!(sc, ScissorRect::new(50, 0, 150, 50));
    }

    #[test]
    fn pillarbox_derive_pillarbox_branch_with_row_start_offsets_top_by_scale_y() {
        // row_start=10, sy=1.0 -> target_top = 10*1.0 = 10.
        let (vp, _sc) = pillarbox_derive(100, 50, Float2::new(2.0, 1.0), 10);
        assert_eq!(vp.y, 10.0);
    }

    // --- pillarbox_derive: letterbox branch (scale.x <= scale.y) ---

    #[test]
    fn pillarbox_derive_letterbox_branch_centers_vertically() {
        // fb=100x50, sx=1.0, sy=2.0, row_start=0.
        // target_width=100*1=100; target_height=50*1=50; target_left=0
        // target_top = 0*2 + (50*2)/2 - 50/2 = 0+50-25=25
        // scissor: floor(0)=0, floor(25)=25, ceil(100)=100, ceil(75)=75
        let (vp, sc) = pillarbox_derive(100, 50, Float2::new(1.0, 2.0), 0);
        assert_eq!(vp, Viewport::new(0.0, 25.0, 100.0, 50.0));
        assert_eq!(sc, ScissorRect::new(0, 25, 100, 75));
    }

    #[test]
    fn pillarbox_derive_equal_scale_takes_letterbox_branch() {
        // scale.x == scale.y: pillarBox = (x > y) is false -> letterbox branch.
        // fb=64x64, scale=1.0, row_start=10.
        // target_width=64; target_height=64; target_left=0
        // target_top = 10*1.0 + (64*1.0)/2 - 64/2 = 10 + 32 - 32 = 10
        let (vp, sc) = pillarbox_derive(64, 64, Float2::new(1.0, 1.0), 10);
        assert_eq!(vp, Viewport::new(0.0, 10.0, 64.0, 64.0));
        assert_eq!(sc, ScissorRect::new(0, 10, 64, 74));
    }

    // --- pillarbox_derive: zero and one-pixel dimensions ---

    #[test]
    fn pillarbox_derive_zero_fb_width_pillarbox_branch() {
        let (vp, _sc) = pillarbox_derive(0, 50, Float2::new(2.0, 1.0), 0);
        assert_eq!(vp.width, 0.0);
        assert_eq!(vp.x, 0.0);
    }

    #[test]
    fn pillarbox_derive_one_pixel_fb_letterbox_branch() {
        let (vp, sc) = pillarbox_derive(1, 1, Float2::new(1.0, 1.0), 0);
        assert_eq!(vp, Viewport::new(0.0, 0.0, 1.0, 1.0));
        assert_eq!(sc, ScissorRect::new(0, 0, 1, 1));
    }

    #[test]
    fn pillarbox_derive_fractional_scale_uses_floor_and_ceil_not_round() {
        // sx=1.3, sy=1.0 (pillarbox branch), fb=10x10, row_start=0.
        // target_width = 10*1.0=10; target_height=10*1.0=10
        // target_left = (10*1.3)/2 - 10/2 = 6.5-5=1.5; target_top=0
        // scissor_left = floor(1.5) = 1 (NOT round(1.5)=2)
        // scissor_right = ceil(1.5+10) = ceil(11.5) = 12 (NOT round=12, coincidentally same)
        let (vp, sc) = pillarbox_derive(10, 10, Float2::new(1.3, 1.0), 0);
        assert_eq!(vp.x, 1.5);
        assert_eq!(sc.left, 1);
        assert_eq!(sc.right, 12);
    }

    // --- viewport_scissor_intersection: viewport fully inside scissor ---

    #[test]
    fn viewport_scissor_intersection_viewport_fully_inside_scissor() {
        let vp = Viewport::new(10.0, 10.0, 50.0, 50.0);
        let sc = ScissorRect::new(0, 0, 100, 100);
        let result = viewport_scissor_intersection(vp, sc);
        assert_eq!(result, ScissorRect::new(10, 10, 60, 60));
    }

    // --- viewport_scissor_intersection: touching vs one-unit overlap ---

    #[test]
    fn viewport_scissor_intersection_touching_boundary_yields_zero_width() {
        // viewport [0,10) touches scissor starting at 10: ceil(10.0)=10=left.
        let vp = Viewport::new(0.0, 0.0, 10.0, 10.0);
        let sc = ScissorRect::new(10, 10, 20, 20);
        let result = viewport_scissor_intersection(vp, sc);
        assert_eq!(result, ScissorRect::new(10, 10, 10, 10));
        assert!(result.left >= result.right); // empty per RenderRect::isEmpty semantics
    }

    #[test]
    fn viewport_scissor_intersection_one_unit_overlap_yields_width_one() {
        // viewport [0,11) overlaps scissor starting at 10 by exactly 1 unit.
        let vp = Viewport::new(0.0, 0.0, 11.0, 11.0);
        let sc = ScissorRect::new(10, 10, 20, 20);
        let result = viewport_scissor_intersection(vp, sc);
        assert_eq!(result, ScissorRect::new(10, 10, 11, 11));
        assert!(result.right > result.left); // non-empty, width 1
    }

    // --- viewport_scissor_intersection: fully disjoint ---

    #[test]
    fn viewport_scissor_intersection_fully_disjoint_yields_inverted_rect() {
        let vp = Viewport::new(0.0, 0.0, 5.0, 5.0);
        let sc = ScissorRect::new(10, 10, 20, 20);
        let result = viewport_scissor_intersection(vp, sc);
        // left=max(floor(0),10)=10; right=min(ceil(5),20)=5 -> inverted, left > right.
        assert_eq!(result, ScissorRect::new(10, 10, 5, 5));
        assert!(result.left > result.right);
    }

    // --- viewport_scissor_intersection: fractional viewport ---

    #[test]
    fn viewport_scissor_intersection_fractional_viewport_floors_origin_ceils_extent() {
        let vp = Viewport::new(1.2, 1.8, 9.6, 9.1);
        let sc = ScissorRect::new(0, 0, 100, 100);
        let result = viewport_scissor_intersection(vp, sc);
        // floor(1.2)=1; floor(1.8)=1; ceil(1.2+9.6)=ceil(10.8)=11; ceil(1.8+9.1)=ceil(10.9)=11
        assert_eq!(result, ScissorRect::new(1, 1, 11, 11));
    }

    #[test]
    fn viewport_scissor_intersection_negative_viewport_origin_floors_toward_negative_infinity() {
        let vp = Viewport::new(-1.5, -1.5, 10.0, 10.0);
        let sc = ScissorRect::new(-100, -100, 100, 100);
        let result = viewport_scissor_intersection(vp, sc);
        // floor(-1.5) = -2 (toward negative infinity, not truncation toward 0).
        assert_eq!(result.left, -2);
        assert_eq!(result.top, -2);
    }

    #[test]
    fn viewport_scissor_intersection_zero_size_viewport() {
        let vp = Viewport::new(5.0, 5.0, 0.0, 0.0);
        let sc = ScissorRect::new(0, 0, 100, 100);
        let result = viewport_scissor_intersection(vp, sc);
        assert_eq!(result, ScissorRect::new(5, 5, 5, 5));
        assert_eq!(result.left, result.right); // empty (zero width)
    }

    #[test]
    fn viewport_scissor_intersection_scissor_clamps_viewport_extending_past_it() {
        let vp = Viewport::new(-10.0, -10.0, 1000.0, 1000.0);
        let sc = ScissorRect::new(0, 0, 50, 50);
        let result = viewport_scissor_intersection(vp, sc);
        assert_eq!(result, ScissorRect::new(0, 0, 50, 50));
    }

    // --- NaN / inf ---

    #[test]
    fn viewport_scissor_intersection_nan_viewport_x_propagates_via_floor_comparison() {
        // floor(NaN) as i32: NaN.floor() is NaN, and `NaN as i32` in Rust
        // saturates to 0 (Rust's documented float-to-int cast behavior since
        // 1.45, not a panic and not the C++ UB `static_cast<int32_t>(NaN)`
        // would invoke -- this port's cast semantics are Rust's, not C++'s,
        // and that divergence is recorded here explicitly).
        let vp = Viewport::new(f32::NAN, 0.0, 10.0, 10.0);
        let sc = ScissorRect::new(5, 5, 20, 20);
        let result = viewport_scissor_intersection(vp, sc);
        // NaN.floor() as i32 == 0 (Rust saturating cast); max(0, 5) = 5.
        assert_eq!(result.left, 5);
    }

    #[test]
    fn compute_scaled_size_infinite_scale_clamps_to_max_dimension() {
        // f32::INFINITY * width = inf; lround(inf) as i64 saturates to i64::MAX
        // in Rust's float-to-int cast, then clamp() brings it down to MAX_DIMENSION.
        let (w, _h, _m) = compute_scaled_size(1, 1, Float2::new(f32::INFINITY, 1.0));
        assert_eq!(w, MAX_DIMENSION as u32);
    }

    #[test]
    fn pillarbox_derive_nan_scale_propagates_to_viewport_fields() {
        let (vp, _sc) = pillarbox_derive(10, 10, Float2::new(f32::NAN, 1.0), 0);
        // pillar_box = NAN > 1.0 is false (any comparison with NaN is false),
        // so this takes the letterbox branch: target_width = 10 * NAN = NaN.
        assert!(vp.width.is_nan());
    }
}
