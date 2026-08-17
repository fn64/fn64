//! Literal port of RT64's `CSMain` region weight table, tap offsets, and
//! per-channel combine from `GaussianFilterRGB3x3CS.hlsl`, a permitted MIT
//! RT64 Rust-port source pinned at commit
//! `5473732a822a4423b5696e7cb18fecc425a59875` (`docs/RT64-PORT-AUTHORITY.md`).
//! SHA-256 of the whole file (82 lines),
//! `523f3e2ea3a92c452267d3843a3901a1a9b07c57d9d17b880e96171b6755f2f1`,
//! matching `docs/rt64-port-inventory.json`'s `sources.port.sha256` for
//! `src/shaders/GaussianFilterRGB3x3CS.hlsl` (confirmed independently here
//! by `shasum -a 256` against the pinned port-commit checkout).
//!
//! ```text
//! //
//! // RT64
//! //
//!
//! // Based on the shader from the D3D12 Raytracing Real Time Denoised Ambient Occlusion sample by Microsoft.
//! // https://github.com/microsoft/DirectX-Graphics-Samples/blob/master/Samples/Desktop/D3D12Raytracing/src/D3D12RaytracingRealTimeDenoisedAmbientOcclusion/RTAO/Shaders/Denoising/GaussianFilterRG3x3CS.hlsl
//! // Copyright (c) Microsoft. All rights reserved.
//!
//! #define BLOCK_SIZE 8
//!
//! struct TextureCB {
//!     uint2 TextureSize;
//!     float2 TexelSize;
//! };
//!
//! [[vk::push_constant]] ConstantBuffer<TextureCB> gConstants : register(b0);
//! Texture2D<float4> gInput : register(t1);
//! RWTexture2D<float4> gOutput : register(u2);
//! SamplerState gSampler : register(s3);
//!
//! [numthreads(BLOCK_SIZE, BLOCK_SIZE, 1)]
//! void CSMain(uint2 DTid : SV_DispatchThreadID) {
//!     float4 weights;
//!
//!     // Non-border pixels
//!     if (DTid.x > 0 && DTid.y > 0 && DTid.x < gConstants.TextureSize.x - 1 && DTid.y < gConstants.TextureSize.y - 1) {
//!         weights = float4(0.077847 + 0.123317 + 0.123317 + 0.195346,
//!             0.077847 + 0.123317,
//!             0.077847 + 0.123317,
//!             0.077847);
//!     }
//!     // Top-left corner
//!     else if (DTid.x == 0 && DTid.y == 0) {
//!         weights = float4(0.195346, 0.123317, 0.123317, 0.077847) / 0.519827;
//!     }
//!     // Top-right corner
//!     else if (DTid.x == gConstants.TextureSize.x - 1 && DTid.y == 0) {
//!         weights = float4(0.123317 + 0.195346, 0, 0.201164, 0) / 0.519827;
//!     }
//!     // Bottom-left corner
//!     else if (DTid.x == 0 && DTid.y == gConstants.TextureSize.y - 1) {
//!         weights = float4(0.123317 + 0.195346, 0.077847 + 0.123317, 0, 0) / 0.519827;
//!     }
//!     // Bottom-right corner
//!     else if (DTid.x == gConstants.TextureSize.x - 1 && DTid.y == gConstants.TextureSize.y - 1) {
//!         weights = float4(0.077847 + 0.123317 + 0.123317 + 0.195346, 0, 0, 0) / 0.519827;
//!     }
//!     // Left border
//!     else if (DTid.x == 0) {
//!         weights = float4(0.123317 + 0.195346, 0.077847 + 0.123317, 0.123317, 0.077847) / 0.720991;
//!     }
//!     // Right border
//!     else if (DTid.x == gConstants.TextureSize.x - 1) {
//!         weights = float4(0.077847 + 0.123317 + 0.123317 + 0.195346, 0, 0.077847 + 0.123317, 0) / 0.720991;
//!     }
//!     // Top border
//!     else if (DTid.y == 0) {
//!         weights = float4(0.123317 + 0.195346, 0.123317, 0.077847 + 0.123317, 0.077847) / 0.720991;
//!     }
//!     // Bottom border
//!     else {
//!         weights = float4(0.077847 + 0.123317 + 0.123317 + 0.195346, 0.077847 + 0.123317, 0, 0) / 0.720991;
//!     }
//!
//!     const float2 offsets[3] = {
//!         float2(0.5, 0.5) + float2(-0.123317 / (0.123317 + 0.195346), -0.123317 / (0.123317 + 0.195346)),
//!         float2(0.5, 0.5) + float2(1, -0.077847 / (0.077847 + 0.123317)),
//!         float2(0.5, 0.5) + float2(-0.077847 / (0.077847 + 0.123317), 1) };
//!
//!     float4 samples[4];
//!     samples[0] = gInput.SampleLevel(gSampler, (DTid + offsets[0]) * gConstants.TexelSize, 0);
//!     samples[1] = gInput.SampleLevel(gSampler, (DTid + offsets[1]) * gConstants.TexelSize, 0);
//!     samples[2] = gInput.SampleLevel(gSampler, (DTid + offsets[2]) * gConstants.TexelSize, 0);
//!     samples[3] = gInput[DTid + 1];
//!
//!     float4 samplesR = float4(samples[0].x, samples[1].x, samples[2].x, samples[3].x);
//!     float4 samplesG = float4(samples[0].y, samples[1].y, samples[2].y, samples[3].y);
//!     float4 samplesB = float4(samples[0].z, samples[1].z, samples[2].z, samples[3].z);
//!     float4 samplesA = float4(samples[0].w, samples[1].w, samples[2].w, samples[3].w);
//!
//!     gOutput[DTid] = float4(dot(samplesR, weights), dot(samplesG, weights), dot(samplesB, weights), dot(samplesA, weights));
//! }
//! ```
//!
//! **Reuse, not new type.** [`RegionWeights`] is the one owned representation
//! of `CSMain`'s per-pixel `weights` local; [`GaussianTaps`] is the one owned
//! representation of its `samples[4]` local (already split by channel, as the
//! shader itself does with `samplesR/G/B/A`). The CPU oracle
//! ([`region_weights`], [`tap_offsets`], [`combine_channel`]) and the WGSL
//! differential sibling ([`GAUSSIAN_FILTER_RGB3X3_WGSL`]) both operate over
//! these types; characterization tests below compare independent derivations
//! against hand-computed values, not one implementation against itself.
//!
//! ## Admitted domain
//!
//! This port covers exactly three pieces of `CSMain`, per the ticket: the
//! region weight table (the nine `if`/`else if` branches selecting `weights`
//! by pixel position, `GaussianFilterRGB3x3CS.hlsl:27-64`), the three
//! fractional tap offsets (`offsets[0..2]`, lines 66-69), and the per-channel
//! `dot(samples*, weights)` combine (lines 71-81, minus the actual texture
//! reads). It does **not** port texture sampling, binding, or dispatch: the
//! four `samples[0..3]` values are admitted here as plain caller-supplied
//! `[f32; 4]` RGBA arguments (as if already returned by
//! `gInput.SampleLevel`/`gInput[...]`), matching the ticket's framing that
//! "the combine is `dot(samples_per_channel, weights)` over four
//! already-sampled values passed as plain arguments."
//!
//! **Weight constants and their sums.** The three raw kernel-cell literals
//! are `a = 0.077847`, `b = 0.123317`, `c = 0.195346` (a separable-Gaussian-
//! like 3x3 stencil with corner/edge/center weights collapsed by symmetry
//! into these three numbers upstream, before this file). Every region's
//! `float4` is built purely from `a`, `b`, `c`, plus (for eight of the nine
//! regions) a literal renormalizing divisor:
//!
//! - **Interior** (no divisor): `(a+b+b+c, a+b, a+b, a)`. In `f32`,
//!   sequential left-to-right addition (`((w0+w1)+w2)+w3`, matching how a
//!   `dot()`/reduction would naturally associate against an unweighted `1`)
//!   gives **`1.0000020265579224`**, not exactly `1.0`. **This region alone
//!   carries a real, if minuscule (~2e-6), DC gain** -- every other region's
//!   `float4` is *divided* by its own raw component sum
//!   (`0.519827` for the four corners, `0.720991` for the four borders),
//!   which cancels the same rounding error by construction; only the
//!   interior region skips that division, so only it is left with the
//!   residual. This port preserves that asymmetry exactly rather than
//!   "fixing" the interior region to sum to 1.0 -- see "Upstream-observation"
//!   below.
//! - **Four corners**, divisor `0.519827` (verified equal in `f32` to the
//!   interior region's undivided first component, `a+b+b+c`): each corner's
//!   raw numerator vector sums to exactly `0.519827` in `f32`, so after
//!   division each corner's four weights sum to exactly `1.0` in `f32` (bit-
//!   exact, verified by test, not merely close).
//! - **Four borders**, divisor `0.720991`: same story -- each border's raw
//!   numerator sums to exactly `0.720991` in `f32`, so each border's four
//!   weights sum to exactly `1.0` in `f32` after division.
//!
//! **Accumulation order.** Float addition is not associative and is never
//! reassociated here. `region_weights` builds each `float4` component with
//! the exact left-to-right grouping the HLSL text shows (e.g. interior
//! component 0 is `((a + b) + b) + c`, not `(a + b) + (b + c)` or any other
//! grouping -- both parse identically for `+` under HLSL/Rust's left-
//! associative binary `+`, but the distinction matters for any future
//! reader diffing against the source). Divisions are applied only to the
//! four corner/border regions, each numerator computed first, then divided
//! by the literal divisor, matching `float4(...) / 0.519827` in the source.
//! `combine_channel`'s `dot()` re-expression sums the four
//! `sample[i] * weight[i]` products in index order `0, 1, 2, 3` (`w[0]*s[0] +
//! w[1]*s[1] + w[2]*s[2] + w[3]*s[3]`, left-to-right), matching HLSL's
//! `dot(a, b)` which is defined as that exact left-to-right component-wise
//! sum-of-products for a 4-vector; this is not reassociated, factored, or
//! computed via `fma` anywhere in this port.
//!
//! **Tap-offset convention and edge/corner clamping.** The three fractional
//! offsets are each `(0.5, 0.5) + delta`, where `delta` depends only on `a`,
//! `b`, `c` (not on pixel position): `offsets[0] = (0.5 - b/(b+c), 0.5 -
//! b/(b+c))` (same value in both `x` and `y`), `offsets[1] = (1.5, 0.5 -
//! a/(a+b))`, `offsets[2] = (0.5 - a/(a+b), 1.5)`. These are added to the
//! integer pixel coordinate `DTid` and then multiplied by `TexelSize` before
//! sampling -- that texel-space conversion and the resulting bilinear
//! fetches are explicitly **not** ported here (see Nonclaims). There is no
//! separate edge/corner *clamp* on the offsets or sample coordinates
//! anywhere in this shader: `CSMain` handles edges/corners entirely by
//! *reweighting* (selecting a different, renormalized `weights` vector per
//! region, per above) rather than by clamping sample coordinates or tap
//! offsets -- an out-of-bounds fetch at an edge is prevented by construction
//! because the corresponding weight component for that missing tap is
//! forced to `0` in the region's weight vector (e.g. the top-left corner's
//! weights are `(c, b, b, a)/0.519827` -- all four nonzero, since a 3x3
//! interior kernel collapsed to 4 samples still needs all 4 taps near a
//! corner -- while the top-right corner's weights are `(b+c, 0, a+b,
//! 0)/0.519827`, zeroing the components that would correspond to samples
//! reaching past the right edge). This port reproduces those exact per-
//! region zero/nonzero patterns as the weight table; it does not implement
//! or claim any independent bounds-clamp.
//!
//! **The fourth sample is not filtered/bilinear at all.** `samples[3] =
//! gInput[DTid + 1]` is a direct integer texel load (no sampler, no
//! filtering, offset by exactly `(1,1)` from the dispatch pixel), unlike
//! `samples[0..2]` which are `SampleLevel` bilinear fetches at fractional
//! offsets. This port's [`GaussianTaps`]/`combine_channel` treat all four
//! samples as opaque `f32` inputs uniformly (matching the ticket's framing),
//! but the doc here records that source distinction since a caller wiring
//! real sampling must reproduce it: `samples[3]` must come from a plain
//! texel load at `(DTid.x + 1, DTid.y + 1)`, not a `SampleLevel` call.
//!
//! **Alpha channel is filtered, not passed through.** `samplesA` is built
//! and combined with `weights` exactly like R/G/B (`dot(samplesA,
//! weights)`); there is no special-cased alpha passthrough anywhere in this
//! shader. `combine_channel` is channel-agnostic for exactly this reason --
//! it is called once per channel including alpha, with no different
//! treatment.
//!
//! **HLSL intrinsics with a subtly different Rust/WGSL equivalent.**
//! `dot(float4, float4)` has no built-in Rust equivalent; `combine_channel`
//! re-expresses it as an explicit four-term sum in source order (see
//! "Accumulation order" above) rather than using any vector-math crate,
//! since crate-level vector types could silently reassociate or use `fma`.
//! Plain `/` (both HLSL and Rust/WGSL IEEE-754 `f32` division) needs no
//! semantic adjustment. `float4(...)` HLSL constructor syntax has no direct
//! parallel; [`RegionWeights`] uses named `f32` fields instead, matching
//! this crate's existing convention (e.g. `raster_vs.rs`).
//!
//! ## Upstream-observation
//!
//! The interior region's weight vector is the only one of the nine regions
//! that is *not* divided by a literal renormalizing constant (see "Weight
//! constants and their sums" above) -- every corner and border region *is*
//! divided by its own raw sum, which happens to cancel `f32` rounding error
//! and land the divided weights at bit-exact `1.0`. Whether this asymmetry
//! is an intentional micro-optimization (the interior's `0.519827` divisor
//! would be a no-op multiply by `~1.0` anyway, so it was presumably elided
//! for that one hot-path branch) or an oversight is not decidable from the
//! source alone; either way it leaves the interior with the shader's only
//! nonzero DC gain (~+2e-6, i.e. every interior pixel's filtered output is
//! ~0.0002% brighter than a true weighted average). This is reported, not
//! fixed, per this port's characterization-not-repair mandate. It is a much
//! smaller effect than the two sibling defects named in the ticket
//! (`HistogramClearCS`'s colliding-address bug); it is asymmetry/precision-
//! residue, not a correctness bug that drops or duplicates work.
//!
//! ## Nonclaims
//!
//! This module makes no GPU execution claim: the WGSL differential test
//! below validates [`GAUSSIAN_FILTER_RGB3X3_WGSL`] through Naga's WGSL
//! front-end and validator only (a plain, non-GPU test), not by dispatching
//! it on a real adapter/device. It makes no production-wiring claim: no
//! pipeline, `wgpu::ShaderModule`, bind group layout, compute dispatch, or
//! `targets/` integration is created here, and this module is not
//! referenced from any draw/dispatch path. Compute-dispatch scaffolding is
//! explicitly **not** ported: `[numthreads(8,8,1)]`/`BLOCK_SIZE`,
//! `SV_DispatchThreadID`-driven region *selection* is reproduced as a pure
//! function of `(x, y, width, height)` (data selection, not GPU scaffolding
//! -- the ticket calls this out as `weights_for_pixel`), but no
//! `groupshared` memory, barrier, texture bind (`register(t1/u2/s3)`),
//! `ConstantBuffer`/push-constant layout (`register(b0)`), or actual
//! `SampleLevel`/texel-load call is admitted. This module also makes **no
//! end-to-end filtered-image equivalence claim**: real equivalence to
//! `CSMain`'s output additionally depends on a bilinear-clamp sampler
//! correctly producing `samples[0..2]` at the exact fractional offsets this
//! port characterizes, which this CPU oracle does not implement or invoke.
//! It makes no parity or performance claim against RT64's own renderer.

/// One `CSMain` `weights` local: the four-component weight vector selected
/// for a given dispatch pixel by the nine-way region `if`/`else if` chain
/// (`GaussianFilterRGB3x3CS.hlsl:27-64`). Component order matches the
/// shader's `samples[0..3]` tap order (see module doc "The fourth sample is
/// not filtered/bilinear at all").
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RegionWeights {
    pub w0: f32,
    pub w1: f32,
    pub w2: f32,
    pub w3: f32,
}

impl RegionWeights {
    fn new(w0: f32, w1: f32, w2: f32, w3: f32) -> Self {
        Self { w0, w1, w2, w3 }
    }

    /// Component-wise array view, matching `float4`'s `.x/.y/.z/.w` order.
    pub fn as_array(&self) -> [f32; 4] {
        [self.w0, self.w1, self.w2, self.w3]
    }
}

/// One `CSMain` `samples[4]` local, already split by RGBA channel as the
/// shader itself does with `samplesR/G/B/A` (`GaussianFilterRGB3x3CS.hlsl:76-79`).
/// Each field holds one channel's four tap values in `samples[0..3]` order.
/// These are admitted as opaque caller-supplied values, not computed by
/// this port -- see module doc "Admitted domain".
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GaussianTaps {
    pub r: [f32; 4],
    pub g: [f32; 4],
    pub b: [f32; 4],
    pub a: [f32; 4],
}

/// The three raw HLSL literal constants `CSMain`'s weight table and offsets
/// are built from (`GaussianFilterRGB3x3CS.hlsl:31,33-34,...`).
const KERNEL_A: f32 = 0.077847;
const KERNEL_B: f32 = 0.123317;
const KERNEL_C: f32 = 0.195346;

/// The two literal renormalizing divisors (`GaussianFilterRGB3x3CS.hlsl:38,49`).
const CORNER_DIVISOR: f32 = 0.519827;
const BORDER_DIVISOR: f32 = 0.720991;

/// Literal port of `CSMain`'s nine-way region `weights` selection
/// (`GaussianFilterRGB3x3CS.hlsl:23-64`), re-expressed as a pure function of
/// pixel position and texture size rather than reading `DTid`/`gConstants`
/// from shader state. `width`/`height` correspond to
/// `gConstants.TextureSize.x/.y`; both must be at least `1` for the interior/
/// edge conditions to be meaningful (a `0`-sized texture is out of this
/// port's admitted domain, matching the source, which never guards against
/// it either).
///
/// Branch order and conditions are reproduced exactly, including the
/// non-obvious fact that the four corner checks are tested *before* the
/// four edge checks, so e.g. `(0, 0)` on a 1-wide-or-1-tall texture still
/// resolves via the corner branches, never falling through to an edge or
/// the final `else` (bottom border) branch.
pub fn region_weights(x: u32, y: u32, width: u32, height: u32) -> RegionWeights {
    let interior = x > 0 && y > 0 && x < width - 1 && y < height - 1;
    if interior {
        // weights = float4(a+b+b+c, a+b, a+b, a);
        return RegionWeights::new(
            KERNEL_A + KERNEL_B + KERNEL_B + KERNEL_C,
            KERNEL_A + KERNEL_B,
            KERNEL_A + KERNEL_B,
            KERNEL_A,
        );
    }
    if x == 0 && y == 0 {
        // Top-left corner: float4(c, b, b, a) / 0.519827
        return RegionWeights::new(
            KERNEL_C / CORNER_DIVISOR,
            KERNEL_B / CORNER_DIVISOR,
            KERNEL_B / CORNER_DIVISOR,
            KERNEL_A / CORNER_DIVISOR,
        );
    }
    if x == width - 1 && y == 0 {
        // Top-right corner: float4(b+c, 0, a+b, 0) / 0.519827
        return RegionWeights::new(
            (KERNEL_B + KERNEL_C) / CORNER_DIVISOR,
            0.0 / CORNER_DIVISOR,
            (KERNEL_A + KERNEL_B) / CORNER_DIVISOR,
            0.0 / CORNER_DIVISOR,
        );
    }
    if x == 0 && y == height - 1 {
        // Bottom-left corner: float4(b+c, a+b, 0, 0) / 0.519827
        return RegionWeights::new(
            (KERNEL_B + KERNEL_C) / CORNER_DIVISOR,
            (KERNEL_A + KERNEL_B) / CORNER_DIVISOR,
            0.0 / CORNER_DIVISOR,
            0.0 / CORNER_DIVISOR,
        );
    }
    if x == width - 1 && y == height - 1 {
        // Bottom-right corner: float4(a+b+b+c, 0, 0, 0) / 0.519827
        return RegionWeights::new(
            (KERNEL_A + KERNEL_B + KERNEL_B + KERNEL_C) / CORNER_DIVISOR,
            0.0 / CORNER_DIVISOR,
            0.0 / CORNER_DIVISOR,
            0.0 / CORNER_DIVISOR,
        );
    }
    if x == 0 {
        // Left border: float4(b+c, a+b, b, a) / 0.720991
        return RegionWeights::new(
            (KERNEL_B + KERNEL_C) / BORDER_DIVISOR,
            (KERNEL_A + KERNEL_B) / BORDER_DIVISOR,
            KERNEL_B / BORDER_DIVISOR,
            KERNEL_A / BORDER_DIVISOR,
        );
    }
    if x == width - 1 {
        // Right border: float4(a+b+b+c, 0, a+b, 0) / 0.720991
        return RegionWeights::new(
            (KERNEL_A + KERNEL_B + KERNEL_B + KERNEL_C) / BORDER_DIVISOR,
            0.0 / BORDER_DIVISOR,
            (KERNEL_A + KERNEL_B) / BORDER_DIVISOR,
            0.0 / BORDER_DIVISOR,
        );
    }
    if y == 0 {
        // Top border: float4(b+c, b, a+b, a) / 0.720991
        return RegionWeights::new(
            (KERNEL_B + KERNEL_C) / BORDER_DIVISOR,
            KERNEL_B / BORDER_DIVISOR,
            (KERNEL_A + KERNEL_B) / BORDER_DIVISOR,
            KERNEL_A / BORDER_DIVISOR,
        );
    }
    // Bottom border (final else): float4(a+b+b+c, a+b, 0, 0) / 0.720991
    RegionWeights::new(
        (KERNEL_A + KERNEL_B + KERNEL_B + KERNEL_C) / BORDER_DIVISOR,
        (KERNEL_A + KERNEL_B) / BORDER_DIVISOR,
        0.0 / BORDER_DIVISOR,
        0.0 / BORDER_DIVISOR,
    )
}

/// Literal port of `CSMain`'s `const float2 offsets[3]`
/// (`GaussianFilterRGB3x3CS.hlsl:66-69`). Returns the three `(x, y)`
/// fractional pixel-space offsets (each still needs `+ DTid`, then `*
/// TexelSize`, to become a normalized sample coordinate -- neither of which
/// is performed here; see module doc "Admitted domain"). These depend only
/// on the three kernel constants, not on pixel position or texture size.
pub fn tap_offsets() -> [[f32; 2]; 3] {
    let d0 = KERNEL_B / (KERNEL_B + KERNEL_C);
    let d1 = KERNEL_A / (KERNEL_A + KERNEL_B);
    [
        [0.5 + (-d0), 0.5 + (-d0)],
        [0.5 + 1.0, 0.5 + (-d1)],
        [0.5 + (-d1), 0.5 + 1.0],
    ]
}

/// Literal port of `CSMain`'s final `dot(samples*, weights)` combine
/// (`GaussianFilterRGB3x3CS.hlsl:81`), applied to one channel's four already-
/// sampled tap values. Re-expresses HLSL's `dot(float4, float4)` as the
/// exact same left-to-right sum of four products it's defined to compute;
/// see module doc "Accumulation order" -- this is not reassociated.
pub fn combine_channel(samples: [f32; 4], weights: RegionWeights) -> f32 {
    let w = weights.as_array();
    samples[0] * w[0] + samples[1] * w[1] + samples[2] * w[2] + samples[3] * w[3]
}

/// Convenience wrapper applying [`combine_channel`] to all four channels of
/// a [`GaussianTaps`], matching `CSMain`'s final
/// `gOutput[DTid] = float4(dot(samplesR, weights), dot(samplesG, weights),
/// dot(samplesB, weights), dot(samplesA, weights))` -- alpha is filtered
/// exactly like R/G/B, with no passthrough special-case (module doc "Alpha
/// channel is filtered, not passed through").
pub fn combine_rgba(taps: GaussianTaps, weights: RegionWeights) -> [f32; 4] {
    [
        combine_channel(taps.r, weights),
        combine_channel(taps.g, weights),
        combine_channel(taps.b, weights),
        combine_channel(taps.a, weights),
    ]
}

pub const GAUSSIAN_FILTER_RGB3X3_WGSL: &str = include_str!("shaders/gaussian_filter_rgb3x3.wgsl");

#[cfg(test)]
mod tests;
