//! Literal port of RT64's auto-exposure histogram arithmetic --
//! `LuminanceHistogramCS.hlsl`'s `GetLuminance`/`HDRToHistogramBin` binning
//! functions and `HistogramAverageCS.hlsl`'s per-group-index weight and
//! final weighted-average/normalization math -- a literal port of the
//! permitted MIT RT64 Rust-port source pinned at commit
//! `5473732a822a4423b5696e7cb18fecc425a59875` (`docs/RT64-PORT-AUTHORITY.md`),
//! `src/shaders/LuminanceHistogramCS.hlsl` (SHA-256 of the whole file,
//! `dd4c6fa7637d2c1bfdcde71bcf2210b5e76f71dda2795282a0ce3ef690307ef1`) and
//! `src/shaders/HistogramAverageCS.hlsl` (SHA-256 of the whole file,
//! `e39a45adab1258f32569c6edc23fb609aedb71f3d4728f15598e9fb581041f26`):
//!
//! ```text
//! // LuminanceHistogramCS.hlsl
//! #define NUM_HISTOGRAM_BINS 64
//! #define HISTOGRAM_THREADS_PER_DIMENSION 8
//! #define EPSILON 1e-6
//!
//! struct LuminanceHistogramCB {
//!     uint inputWidth;
//!     uint inputHeight;
//!     float minLuminance;
//!     float oneOverLuminanceRange;
//! };
//!
//! float GetLuminance(float3 color) {
//!     return dot(color, float3(0.2127f, 0.7152f, 0.0722f));
//! }
//!
//! uint HDRToHistogramBin(float3 hdrColor)
//! {
//!     float luminance = GetLuminance(hdrColor);
//!     if (luminance < EPSILON) {
//!         return 0;
//!     }
//!
//!     float satLuminance = saturate((luminance - gConstants.minLuminance) * gConstants.oneOverLuminanceRange);
//!     return (uint) (satLuminance * 62.0 + 1.0);
//! }
//!
//! [numthreads(HISTOGRAM_THREADS_PER_DIMENSION, HISTOGRAM_THREADS_PER_DIMENSION, 1)]
//! void CSMain(uint groupIndex : SV_GroupIndex, uint3 threadId : SV_DispatchThreadID) {
//!     HistogramShared[groupIndex] = 0;
//!
//!     GroupMemoryBarrierWithGroupSync();
//!     if (threadId.x < gConstants.inputWidth && threadId.y < gConstants.inputHeight) {
//!         float3 hdrColor = HDRTexture.Load(int3(threadId.xy, 0)).rgb;
//!         uint binIndex = HDRToHistogramBin(hdrColor);
//!         InterlockedAdd(HistogramShared[binIndex], 1);
//!     }
//!     GroupMemoryBarrierWithGroupSync();
//!
//!     LuminanceHistogram.InterlockedAdd(groupIndex * 4, HistogramShared[groupIndex]);
//! }
//! ```
//!
//! ```text
//! // HistogramAverageCS.hlsl
//! #define NUM_HISTOGRAM_BINS 64
//! #define HISTOGRAM_AVERAGE_THREADS_PER_DIMENSION 8
//!
//! struct HistogramAverageCB {
//!     uint pixelCount;
//!     float minLuminance;
//!     float luminanceRange;
//!     float timeDelta;
//!     float tau;
//! };
//!
//! groupshared float HistogramShared[NUM_HISTOGRAM_BINS];
//!
//! [numthreads(HISTOGRAM_AVERAGE_THREADS_PER_DIMENSION, HISTOGRAM_AVERAGE_THREADS_PER_DIMENSION, 1)]
//! void CSMain(uint groupIndex : SV_GroupIndex) {
//!     float countForThisBin = (float)LuminanceHistogram.Load(groupIndex * 4);
//!     HistogramShared[groupIndex] = countForThisBin * (float)groupIndex;
//!     GroupMemoryBarrierWithGroupSync();
//!
//!     [unroll]
//!     for (uint histogramSampleIndex = (NUM_HISTOGRAM_BINS >> 1); histogramSampleIndex > 0; histogramSampleIndex >>= 1) {
//!         if (groupIndex < histogramSampleIndex) {
//!             HistogramShared[groupIndex] += HistogramShared[groupIndex + histogramSampleIndex];
//!         }
//!         GroupMemoryBarrierWithGroupSync();
//!     }
//!
//!     if (groupIndex == 0) {
//!         float weightedAverage = (HistogramShared[0].x / max(float(gConstants.pixelCount) - countForThisBin, 1.0)) - 1.0;
//!         float weightedAverageLuminance = ((weightedAverage / 62.0) * gConstants.luminanceRange) + gConstants.minLuminance;
//!         float luminanceLastFrame = LuminanceOutput[uint2(0, 0)];
//!         if (isinf(luminanceLastFrame) || isnan(luminanceLastFrame)) {
//!             luminanceLastFrame = 1.0;
//!         }
//!
//!         float adaptedLuminance = luminanceLastFrame + (weightedAverageLuminance - luminanceLastFrame) * (1 - exp(-gConstants.timeDelta * gConstants.tau));
//!         LuminanceOutput[uint2(0, 0)] = max(adaptedLuminance, EPSILON);
//!     }
//! }
//! ```
//!
//! (`HistogramAverageCS.hlsl`'s file-scope `EPSILON` comes from its
//! `#include "Math.hlsli"`, which `#define`s `EPSILON 1e-6` -- the same
//! numeric value as `LuminanceHistogramCS.hlsl`'s own local `#define
//! EPSILON 1e-6`, so both files' `EPSILON` are `1e-6_f32` here.)
//!
//! **Ported (arithmetic only):**
//! - `GetLuminance`: the BT.709-coefficient dot product.
//! - `HDRToHistogramBin`: the `EPSILON` early-out, the `saturate`d linear
//!   remap, and the bin-index cast.
//! - `HistogramAverageCS`'s per-bin weight: `countForThisBin *
//!   (float)groupIndex` (line 31).
//! - `HistogramAverageCS`'s parallel-reduction *arithmetic* (the pairwise
//!   tree sum lines 35-40 compute), re-expressed as a same-order tree sum
//!   over a 64-element array -- see [`weighted_bin_sum`] and "Admitted
//!   domain" below for why the pairwise order is preserved rather than
//!   reassociated into a linear fold.
//! - `HistogramAverageCS`'s `groupIndex == 0` weighted-average block (lines
//!   43-51): the division/subtraction chain, the log-range renormalization,
//!   the last-frame inf/NaN guard, the exponential-decay temporal blend,
//!   and the final `EPSILON` floor.
//!
//! **NOT ported (compute-dispatch scaffolding):**
//! - `CSMain` in both files: `SV_GroupIndex`/`SV_DispatchThreadID` thread
//!   indexing, the `threadId.x < inputWidth && threadId.y < inputHeight`
//!   bounds check, `HDRTexture.Load`, `groupshared` array declarations and
//!   their zero-init (`HistogramShared[groupIndex] = 0`),
//!   `GroupMemoryBarrierWithGroupSync()` barriers, `InterlockedAdd` (both
//!   the groupshared bin increment and the `RWByteAddressBuffer` commit),
//!   `ByteAddressBuffer.Load`/`RWByteAddressBuffer.Store`/`.InterlockedAdd`
//!   byte-offset addressing, `RWTexture2D<float> LuminanceOutput` reads/
//!   writes, `[[vk::push_constant]] ConstantBuffer` resource binds, and
//!   `[numthreads(...)]` group-size declarations.
//! - `LuminanceHistogramCB`/`HistogramAverageCB`: not ported as Rust
//!   structs; their fields are taken as plain function parameters on the
//!   ported functions instead (no GPU constant-buffer layout exists here to
//!   preserve).
//!
//! **Reuse, not new type.** No new struct is introduced. `GetLuminance`
//! reuses a plain `[f32; 3]` (the same convention as
//! [`crate::color_hlsli::rgb_to_luminance`]'s `RGBtoLuminance`, which is the
//! established precedent for a `dot(color, float3(...))`-shaped luminance
//! port in this crate), and every other function takes/returns plain
//! `f32`/`u32` scalars or a `&[f32]` slice -- there is no vector/struct
//! shape in either shader's arithmetic worth a new type.
//!
//! ## Admitted domain
//!
//! - **`GetLuminance`'s dot-product order is NOT reassociated.** `dot(color,
//!   float3(0.2127f, 0.7152f, 0.0722f))` is ported as the literal
//!   left-to-right sum `color[0] * 0.2127 + color[1] * 0.7152 + color[2] *
//!   0.0722` (two sequential `f32` adds, in that exact order), matching
//!   [`crate::color_hlsli::rgb_to_luminance`]'s established precedent for
//!   this exact shader idiom in this crate. Float addition is not
//!   associative, so this order is preserved exactly, not reordered for
//!   readability or SIMD-friendliness.
//! - **`HDRToHistogramBin`'s `EPSILON` early-out uses `<`, and it runs
//!   BEFORE any other computation -- there is no `log2` call anywhere in
//!   either pinned file.** The upstream Alex Tardif histogram-luminance
//!   technique this shader implements is commonly *described* as
//!   "log2-binned," but that log2 mapping is baked into the caller-supplied
//!   `minLuminance`/`oneOverLuminanceRange` (computed CPU-side, outside
//!   this shader) -- `HDRToHistogramBin` itself only ever does a *linear*
//!   remap of whatever `luminance` value it's given: `(luminance -
//!   minLuminance) * oneOverLuminanceRange`. This port does not invent a
//!   `log2` call that is not in the pinned source.
//! - **NaN behavior of the `EPSILON` early-out**: HLSL `luminance <
//!   EPSILON` on a NaN `luminance` is IEEE-754-unordered, so the comparison
//!   is `false` and the early-out does NOT fire for NaN -- execution falls
//!   through to the `saturate(...)` line with a NaN input. Rust's `f32 <
//!   f32` has identical IEEE-754 unordered-comparison semantics (NaN
//!   compares `false` against everything, including itself), so
//!   `luminance < EPSILON` in [`hdr_to_histogram_bin`] preserves this
//!   exactly: NaN does not return bin 0 early, it proceeds to `saturate`.
//! - **`saturate` is `x.clamp(0.0, 1.0)`, matching the crate's established
//!   precedent** ([`crate::color_hlsli`]'s `hue_to_rgb`/`mod_rgb_with_hsl`
//!   doc comment: literal `0.0`/`1.0` bounds are never computed, so Rust's
//!   `f32::clamp` panic-on-`min > max`/NaN-*bound* conditions can never
//!   trigger). On a NaN `self` (which reaches `saturate` here specifically
//!   because of the `EPSILON`-early-out NaN pass-through noted above),
//!   `f32::clamp` returns the NaN unchanged (both bound comparisons are
//!   IEEE-754-unordered-false) -- this port lets that NaN propagate rather
//!   than special-casing it, exactly as `color_hlsli.rs` does for the same
//!   reason (its return type, `f32`, can represent NaN). `saturate` is
//!   applied to the already-linearly-remapped luminance ratio -- there is
//!   no log anywhere in this file to apply it before or after (see above).
//! - **Bin-index rounding is C-style truncate-toward-zero, via `as u32`,
//!   NOT floor/round.** `(uint) (satLuminance * 62.0 + 1.0)` is an HLSL
//!   C-style cast from `float` to `uint`, which truncates toward zero (not
//!   HLSL's separate `round()`/`floor()` intrinsics). For the *admitted
//!   input domain here* -- `satLuminance` is always in `[0.0, 1.0]` after
//!   `saturate` (a NaN is the only non-finite possibility, handled
//!   separately below), so `satLuminance * 62.0 + 1.0` is always in `[1.0,
//!   63.0]`, strictly non-negative -- truncate-toward-zero and floor
//!   produce IDENTICAL results (they only differ for negative operands,
//!   which cannot occur once `saturate` has run on a non-NaN input). This
//!   port uses Rust's `as u32` on the `f32`, which is also
//!   truncate-toward-zero for finite in-range values, so the two
//!   conventions coincide exactly across this function's entire non-NaN
//!   domain; the choice of "truncate" over "floor" is therefore
//!   immaterial to any test in this module, and is called out here only
//!   because the ticket asks for the exact rounding mode used.
//!   **`f32 as u32` on a NaN input saturates to `0`** (Rust's documented
//!   `as`-cast behavior since 1.45, replacing the pre-1.45 UB) -- so
//!   `hdr_to_histogram_bin` on a NaN `hdrColor` (whose dot product is
//!   itself NaN, since any float arithmetic on a NaN operand is NaN)
//!   returns bin `0`. This coincides with, but is not the same code path
//!   as, the deliberate `luminance < EPSILON` bin-0 return for
//!   near-black/negative-luminance inputs; both are preserved and
//!   distinguished in the tests below.
//! - **`HistogramAverageCS`'s per-bin weight cast order**: `(float)
//!   LuminanceHistogram.Load(groupIndex * 4)` casts the *bin count* to
//!   `float` first (an exact `u32`-to-`f32` widening for all `u32` values
//!   representable without loss up to 2^24, which every plausible pixel
//!   count in a single 64-bin histogram bucket is), then multiplies by
//!   `(float)groupIndex` (also exact for `groupIndex` in `0..64`). This
//!   port takes `count_for_bin: f32` directly (the cast itself carries no
//!   interesting float behavior to characterize at these magnitudes) and
//!   multiplies by `group_index as f32`.
//! - **The pairwise-tree reduction order is preserved, NOT reassociated
//!   into a linear/`sum()` fold.** `HistogramShared[groupIndex] +=
//!   HistogramShared[groupIndex + histogramSampleIndex]` for
//!   `histogramSampleIndex` = 32, 16, 8, 4, 2, 1 is a standard parallel
//!   binary-tree reduction: after all 6 passes, slot 0 holds the sum of all
//!   64 weighted bin values, but combined in a specific pairwise order
//!   (`(a0+a32)`, then `((a0+a32)+(a16+a48))`, etc. -- NOT `a0+a1+a2+...
//!   +a63` left-to-right). Since float addition is not associative, a
//!   naive `.iter().sum()` (left-to-right fold) can produce a bit-different
//!   result than this tree order for the same 64 inputs. [`weighted_bin_sum`]
//!   reproduces the exact tree order the shader computes (six passes,
//!   `histogramSampleIndex` halving from 32 down to 1, each pass updating
//!   the low half in place from the high half, exactly mirroring the
//!   `[unroll]` loop body) rather than any other summation order.
//! - **The weighted-average division `HistogramShared[0] / max(pixelCount -
//!   countForThisBin, 1.0)`**: the denominator is floored at `1.0` via
//!   `max`, so it can never be `0.0` or negative from that `max` -- but
//!   `pixelCount` and `countForThisBin` are caller-supplied `f32` inputs to
//!   this ported function ([`weighted_average`]/[`weighted_average_luminance`]
//!   take them as plain parameters), so a caller *can* still pass a
//!   denominator whose `max(..., 1.0)` result is `1.0` exactly (nothing
//!   prevents this at the type level) or, if `pixelCount`/`countForThisBin`
//!   are themselves NaN/inf, propagate accordingly -- `max` on a NaN
//!   operand in HLSL and Rust's `f32::max` both suppress the NaN if the
//!   *other* operand is a number (IEEE-754 `fmax`-style, NaN-non-propagating
//!   for a single NaN operand), which this port preserves via `f32::max`
//!   (not `.max()` on an `Ord`-style total order, which does not exist for
//!   `f32`). No `EPSILON`/zero-guard is added beyond what the source's own
//!   `max(..., 1.0)` already provides -- this port does not add a second,
//!   unrequested division-by-zero guard.
//! - **The final `LuminanceOutput[uint2(0,0)] = max(adaptedLuminance,
//!   EPSILON)` floor and the `isinf(luminanceLastFrame) ||
//!   isnan(luminanceLastFrame)` guard**: both are ported as literal `f32`
//!   operations -- `f32::is_infinite()`/`f32::is_nan()` for the `isinf`/
//!   `isnan` HLSL intrinsics (identical IEEE-754 predicates), and
//!   `f32::max` for the final floor, preserving the same
//!   NaN-non-propagating-if-one-operand-is-a-number behavior noted above.
//!   If `adaptedLuminance` itself is NaN (reachable if
//!   `weightedAverageLuminance` is NaN, e.g. from a NaN `HistogramShared[0]`
//!   sum), `f32::max(NaN, EPSILON)` returns `EPSILON`, not NaN -- this is
//!   the correct, literal translation of HLSL's `max` intrinsic, which is
//!   NaN-suppressing exactly like Rust's `f32::max`, not a special case
//!   added by this port.
//! - **No `rcp`, no `mad`.** Neither pinned file calls HLSL's `rcp()` or
//!   `mad()` intrinsics anywhere -- `oneOverLuminanceRange` is a
//!   caller-supplied reciprocal (computed outside these two shaders, not
//!   via `rcp()` inside them), and every multiply-add in both files is
//!   written as separate `*`/`+`/`-` operators, which this port reproduces
//!   as separate Rust `*`/`+`/`-` operators (`f32 * f32 + f32`, evaluated
//!   with normal Rust operator-precedence/left-to-right-for-same-precedence
//!   rules, which exactly matches C/HLSL's evaluation of the same
//!   expression shape). No GPU-side `rcp`-approximation-precision claim is
//!   made or needed since `rcp` is never invoked in the ported arithmetic.
//!
//! ## Nonclaims
//!
//! No GPU execution, no WGSL, and no production wiring: this module is not
//! referenced from any draw/dispatch path, has no `wgpu` type, no compute
//! pipeline, and no shader-manifest entry -- dead-code warnings on its
//! public surface are expected and correct, matching every other
//! characterization-first module's precedent in this crate. No RT64
//! visual/pixel/silicon parity or performance claim. The compute-dispatch
//! scaffolding for both shaders -- thread/group indexing
//! (`SV_GroupIndex`/`SV_DispatchThreadID`), the `groupshared` array
//! declarations and their barriers (`GroupMemoryBarrierWithGroupSync`),
//! `InterlockedAdd`, `ByteAddressBuffer`/`RWByteAddressBuffer` byte
//! addressing, `RWTexture2D`/`Texture2D` resource reads/writes, and
//! `[[vk::push_constant]] ConstantBuffer` binds -- is deliberately NOT
//! ported (see "Ported"/"NOT ported" lists above). No claim is made about
//! GPU-side `rcp` approximation precision, since neither pinned file calls
//! `rcp()` (see "Admitted domain"). `HistogramClearCS.hlsl` and
//! `HistogramSetCS.hlsl` are out of this module's scope entirely and are
//! not read or ported here.

/// Literal port of `float GetLuminance(float3 color)`
/// (`LuminanceHistogramCS.hlsl:27-29`). BT.709 coefficients, left-to-right
/// dot-product order (see module doc "Admitted domain" -- not reassociated).
pub fn get_luminance(color: [f32; 3]) -> f32 {
    color[0] * 0.2127_f32 + color[1] * 0.7152_f32 + color[2] * 0.0722_f32
}

/// Literal port of `uint HDRToHistogramBin(float3 hdrColor)`
/// (`LuminanceHistogramCS.hlsl:31-40`). `min_luminance` and
/// `one_over_luminance_range` are `gConstants.minLuminance` /
/// `gConstants.oneOverLuminanceRange` taken as plain parameters (the
/// constant-buffer struct itself is not ported -- see module doc "NOT
/// ported").
pub fn hdr_to_histogram_bin(
    hdr_color: [f32; 3],
    min_luminance: f32,
    one_over_luminance_range: f32,
) -> u32 {
    const EPSILON: f32 = 1e-6;

    let luminance = get_luminance(hdr_color);
    if luminance < EPSILON {
        return 0;
    }

    let sat_luminance = ((luminance - min_luminance) * one_over_luminance_range).clamp(0.0, 1.0);
    (sat_luminance * 62.0 + 1.0) as u32
}

/// Literal port of `HistogramAverageCS.hlsl:30-31`'s per-bin weight:
/// `HistogramShared[groupIndex] = countForThisBin * (float)groupIndex`.
/// `count_for_bin` is the already-`(float)`-cast bin count (`(float)
/// LuminanceHistogram.Load(groupIndex * 4)`); the `ByteAddressBuffer.Load`
/// byte-address read itself is scaffolding and not ported (see module doc).
pub fn weighted_bin_value(count_for_bin: f32, group_index: u32) -> f32 {
    count_for_bin * (group_index as f32)
}

/// Literal port of `HistogramAverageCS.hlsl:35-40`'s parallel-reduction
/// arithmetic: sums all 64 `weighted_bins` entries using the exact same
/// pairwise binary-tree order the `[unroll]` loop computes (NOT a
/// left-to-right fold -- see module doc "Admitted domain": float addition
/// is not associative). `weighted_bins` must have exactly
/// `NUM_HISTOGRAM_BINS` (64) elements, one per histogram bin, each already
/// through [`weighted_bin_value`]; the groupshared array, its
/// `GroupMemoryBarrierWithGroupSync()` barriers between passes, and the
/// `groupIndex < histogramSampleIndex` per-thread predication are
/// scaffolding and not reproduced here -- this function computes the same
/// final combined value slot 0 would hold after all six barrier-separated
/// passes complete.
pub fn weighted_bin_sum(weighted_bins: &[f32; 64]) -> f32 {
    let mut shared = *weighted_bins;
    let mut histogram_sample_index = 64u32 >> 1; // 32
    while histogram_sample_index > 0 {
        for group_index in 0..histogram_sample_index as usize {
            shared[group_index] += shared[group_index + histogram_sample_index as usize];
        }
        histogram_sample_index >>= 1;
    }
    shared[0]
}

/// Literal port of `HistogramAverageCS.hlsl:43`'s `weightedAverage`:
/// `(HistogramShared[0].x / max(float(pixelCount) - countForThisBin, 1.0))
/// - 1.0`. `bin_sum` is [`weighted_bin_sum`]'s result (`HistogramShared[0]`
/// after the reduction); `count_for_bin_zero` is `countForThisBin` as it
/// stood in bin-0's thread (`HistogramShared[0]`'s own pre-reduction weight,
/// i.e. `weighted_bin_value(count_of_bin_0, 0)` = `0.0` always, since
/// `group_index = 0` -- reproduced as a caller-supplied parameter rather
/// than hard-coded `0.0`, matching the source's literal shape exactly,
/// since the source re-reads the *same* per-thread `countForThisBin`
/// local this line was already holding, not a fresh load).
pub fn weighted_average(bin_sum: f32, pixel_count: f32, count_for_bin_zero: f32) -> f32 {
    (bin_sum / (pixel_count - count_for_bin_zero).max(1.0)) - 1.0
}

/// Literal port of `HistogramAverageCS.hlsl:44`'s `weightedAverageLuminance`:
/// `((weightedAverage / 62.0) * luminanceRange) + minLuminance`.
pub fn weighted_average_luminance(
    weighted_average: f32,
    luminance_range: f32,
    min_luminance: f32,
) -> f32 {
    (weighted_average / 62.0) * luminance_range + min_luminance
}

/// Literal port of `HistogramAverageCS.hlsl:45-51`'s temporal-adaptation
/// blend and final floor: the `isinf`/`isnan` guard on the previous frame's
/// luminance, the exponential-decay blend toward `weighted_average_luminance`,
/// and the `max(adaptedLuminance, EPSILON)` floor. `luminance_last_frame` is
/// `LuminanceOutput[uint2(0,0)]` (the texture read is scaffolding and not
/// reproduced -- see module doc); the texture write-back is likewise not
/// reproduced, only the value it would receive.
pub fn adapt_luminance(
    weighted_average_luminance: f32,
    luminance_last_frame: f32,
    time_delta: f32,
    tau: f32,
) -> f32 {
    const EPSILON: f32 = 1e-6;

    let luminance_last_frame =
        if luminance_last_frame.is_infinite() || luminance_last_frame.is_nan() {
            1.0
        } else {
            luminance_last_frame
        };

    let adapted_luminance = luminance_last_frame
        + (weighted_average_luminance - luminance_last_frame) * (1.0 - (-time_delta * tau).exp());
    adapted_luminance.max(EPSILON)
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- get_luminance ---

    #[test]
    fn get_luminance_black_is_zero() {
        assert_eq!(get_luminance([0.0, 0.0, 0.0]), 0.0);
    }

    #[test]
    fn get_luminance_white_sums_coefficients() {
        // 0.2127 + 0.7152 + 0.0722 = 1.0001, computed left-to-right as
        // (0.2127 + 0.7152) + 0.0722 in f32.
        let expected = (0.2127_f32 + 0.7152_f32) + 0.0722_f32;
        assert_eq!(get_luminance([1.0, 1.0, 1.0]), expected);
    }

    #[test]
    fn get_luminance_red_only() {
        assert_eq!(get_luminance([1.0, 0.0, 0.0]), 0.2127);
    }

    #[test]
    fn get_luminance_green_only() {
        assert_eq!(get_luminance([0.0, 1.0, 0.0]), 0.7152);
    }

    #[test]
    fn get_luminance_blue_only() {
        assert_eq!(get_luminance([0.0, 0.0, 1.0]), 0.0722);
    }

    #[test]
    fn get_luminance_negative_component_no_clamp() {
        // GetLuminance itself never clamps -- only HDRToHistogramBin's
        // saturate() does, downstream.
        let result = get_luminance([-1.0, 1.0, 1.0]);
        let expected = (-0.2127_f32 + 0.7152_f32) + 0.0722_f32;
        assert_eq!(result, expected);
    }

    #[test]
    fn get_luminance_nan_component_propagates() {
        assert!(get_luminance([f32::NAN, 0.0, 0.0]).is_nan());
    }

    // --- hdr_to_histogram_bin ---

    #[test]
    fn hdr_to_histogram_bin_black_is_bin_zero_via_epsilon_early_out() {
        assert_eq!(hdr_to_histogram_bin([0.0, 0.0, 0.0], 0.0, 1.0), 0);
    }

    #[test]
    fn hdr_to_histogram_bin_luminance_exactly_at_epsilon_does_not_early_out() {
        // `luminance < EPSILON` is false when luminance == EPSILON (strict
        // `<`), so this falls through to the saturate/bin-index path rather
        // than returning bin 0 via the early-out. Construct hdr_color =
        // [x, 0, 0] where x = f32(EPSILON / 0.2127_f32), so that
        // get_luminance(hdr_color) == EPSILON exactly in f32 arithmetic
        // (verified independently in Python: EPSILON as f32 is
        // 9.999999974752427e-07, coeff 0.2127 as f32 is
        // 0.2126999944448471, x = 4.701457783085061e-06, and x * coeff
        // rounds back to exactly EPSILON).
        let epsilon = 1e-6_f32;
        let coeff = 0.2127_f32;
        let x = epsilon / coeff;
        let luminance_check = x * coeff;
        assert_eq!(luminance_check, epsilon, "boundary construction invariant");

        // min_luminance=0.0, one_over_luminance_range=1.0: satLuminance =
        // saturate(EPSILON * 1.0) = EPSILON (unclamped, since 0 < EPSILON <
        // 1). bin = (uint)(EPSILON * 62.0 + 1.0) = (uint)(1.0000619...) = 1,
        // NOT 0 -- proving the early-out did not fire.
        let bin = hdr_to_histogram_bin([x, 0.0, 0.0], 0.0, 1.0);
        assert_eq!(bin, 1);
    }

    #[test]
    fn hdr_to_histogram_bin_luminance_just_above_epsilon_takes_arithmetic_path() {
        // Pick a color whose dot-product luminance clears EPSILON.
        let hdr_color = [1.0_f32, 0.0, 0.0]; // luminance = 0.2127
        let bin = hdr_to_histogram_bin(hdr_color, 0.0, 1.0);
        // satLuminance = saturate((0.2127 - 0.0) * 1.0) = 0.2127
        // bin = (uint)(0.2127 * 62.0 + 1.0) = (uint)(14.1874) = 14
        assert_eq!(bin, 14);
    }

    #[test]
    fn hdr_to_histogram_bin_min_luminance_maps_to_bin_one() {
        // satLuminance = saturate((luminance - minLuminance) * range) = 0.0
        // at the exact minimum -> bin = (uint)(0*62+1) = 1.
        let hdr_color = [1.0_f32, 0.0, 0.0]; // luminance = 0.2127
        let bin = hdr_to_histogram_bin(hdr_color, 0.2127, 1.0);
        assert_eq!(bin, 1);
    }

    #[test]
    fn hdr_to_histogram_bin_max_luminance_maps_to_bin_sixty_three() {
        // satLuminance saturates to 1.0 at/after the max -> bin =
        // (uint)(1.0*62+1.0) = 63, the last bin.
        let hdr_color = [1.0_f32, 1.0, 1.0]; // luminance ~= 1.0001
        let bin = hdr_to_histogram_bin(hdr_color, 0.0, 1.0);
        assert_eq!(bin, 63);
    }

    #[test]
    fn hdr_to_histogram_bin_beyond_max_range_saturates_not_overflows() {
        // luminance far past minLuminance + 1/oneOverRange: saturate clamps
        // to 1.0, bin stays 63, does not overflow past NUM_HISTOGRAM_BINS.
        let hdr_color = [1000.0_f32, 1000.0, 1000.0];
        let bin = hdr_to_histogram_bin(hdr_color, 0.0, 1.0);
        assert_eq!(bin, 63);
    }

    #[test]
    fn hdr_to_histogram_bin_negative_luminance_below_epsilon_is_bin_zero() {
        // Negative luminance is < EPSILON (1e-6), early-out fires.
        let hdr_color = [-1.0_f32, -1.0, -1.0];
        assert_eq!(hdr_to_histogram_bin(hdr_color, 0.0, 1.0), 0);
    }

    #[test]
    fn hdr_to_histogram_bin_negative_luminance_after_remap_saturates_to_bin_one() {
        // Choose a positive-luminance color (clears the EPSILON early-out)
        // whose (luminance - minLuminance) is negative -- saturate clamps
        // the ratio to 0.0, giving bin 1, not a negative/wrapped bin.
        let hdr_color = [1.0_f32, 0.0, 0.0]; // luminance = 0.2127
        let bin = hdr_to_histogram_bin(hdr_color, 10.0, 1.0);
        assert_eq!(bin, 1);
    }

    #[test]
    fn hdr_to_histogram_bin_nan_luminance_does_not_early_out_and_casts_to_zero() {
        // luminance is NaN: `luminance < EPSILON` is false (IEEE-754
        // unordered), so it falls through to saturate(NaN) = NaN, then
        // (NaN * 62.0 + 1.0) as u32 saturates to 0 per Rust's documented
        // float-to-int cast behavior.
        let hdr_color = [f32::NAN, 0.0, 0.0];
        assert_eq!(hdr_to_histogram_bin(hdr_color, 0.0, 1.0), 0);
    }

    #[test]
    fn hdr_to_histogram_bin_nan_min_luminance_propagates_to_bin_zero_cast() {
        let hdr_color = [1.0_f32, 0.0, 0.0];
        assert_eq!(hdr_to_histogram_bin(hdr_color, f32::NAN, 1.0), 0);
    }

    #[test]
    fn hdr_to_histogram_bin_midpoint_rounds_down_via_truncation() {
        // satLuminance chosen so satLuminance*62+1 has a fractional part,
        // confirming truncate-toward-zero (not round-to-nearest).
        // luminance=0.5 exactly reachable via a synthetic ratio:
        // satLuminance = 0.5 -> 0.5*62+1 = 32.0 exactly (no fraction) --
        // pick a value that does have a fraction instead.
        let hdr_color = [1.0_f32, 0.0, 0.0]; // luminance = 0.2127
                                             // minLuminance = 0, range so satLuminance = 0.51 (choose
                                             // one_over_luminance_range = 0.51/0.2127).
        let one_over_range = 0.51_f32 / 0.2127_f32;
        let bin = hdr_to_histogram_bin(hdr_color, 0.0, one_over_range);
        // satLuminance*62+1 = 0.51*62+1 = 32.62 -> truncates to 32, not
        // rounds to 33.
        assert_eq!(bin, 32);
    }

    // --- weighted_bin_value ---

    #[test]
    fn weighted_bin_value_zero_index_is_always_zero() {
        assert_eq!(weighted_bin_value(1000.0, 0), 0.0);
    }

    #[test]
    fn weighted_bin_value_multiplies_count_by_index() {
        assert_eq!(weighted_bin_value(5.0, 10), 50.0);
    }

    #[test]
    fn weighted_bin_value_zero_count_is_zero_at_any_index() {
        assert_eq!(weighted_bin_value(0.0, 63), 0.0);
    }

    #[test]
    fn weighted_bin_value_last_bin_index() {
        assert_eq!(weighted_bin_value(2.0, 63), 126.0);
    }

    // --- weighted_bin_sum ---

    #[test]
    fn weighted_bin_sum_all_zero_is_zero() {
        let bins = [0.0_f32; 64];
        assert_eq!(weighted_bin_sum(&bins), 0.0);
    }

    #[test]
    fn weighted_bin_sum_single_bin_one_contributes_its_own_value() {
        let mut bins = [0.0_f32; 64];
        bins[1] = 7.0;
        assert_eq!(weighted_bin_sum(&bins), 7.0);
    }

    #[test]
    fn weighted_bin_sum_single_bin_sixty_three_contributes_its_own_value() {
        let mut bins = [0.0_f32; 64];
        bins[63] = 42.0;
        assert_eq!(weighted_bin_sum(&bins), 42.0);
    }

    #[test]
    fn weighted_bin_sum_all_ones_sums_to_bin_count() {
        let bins = [1.0_f32; 64];
        assert_eq!(weighted_bin_sum(&bins), 64.0);
    }

    #[test]
    fn weighted_bin_sum_matches_realistic_weighted_bins() {
        // bin i has count 1 -> weighted value i (weighted_bin_value(1, i)).
        // Sum of 0..63 = 63*64/2 = 2016, exactly representable in f32, so
        // the tree-order sum and the closed-form sum coincide here.
        let mut bins = [0.0_f32; 64];
        for i in 0..64u32 {
            bins[i as usize] = weighted_bin_value(1.0, i);
        }
        assert_eq!(weighted_bin_sum(&bins), 2016.0);
    }

    #[test]
    fn weighted_bin_sum_nan_bin_poisons_the_whole_sum() {
        let mut bins = [1.0_f32; 64];
        bins[40] = f32::NAN;
        assert!(weighted_bin_sum(&bins).is_nan());
    }

    // --- weighted_average ---

    #[test]
    fn weighted_average_zero_sum_and_zero_bin_zero_count() {
        // (0.0 / max(100.0 - 0.0, 1.0)) - 1.0 = -1.0.
        assert_eq!(weighted_average(0.0, 100.0, 0.0), -1.0);
    }

    #[test]
    fn weighted_average_typical_values() {
        // (500.0 / max(1000.0 - 10.0, 1.0)) - 1.0 = 500/990 - 1.
        let expected = 500.0_f32 / 990.0_f32 - 1.0;
        assert_eq!(weighted_average(500.0, 1000.0, 10.0), expected);
    }

    #[test]
    fn weighted_average_zero_total_count_floors_denominator_to_one() {
        // pixelCount - countForThisBin = 0 - 0 = 0, max(0, 1.0) = 1.0 --
        // denominator never reaches zero because of the max floor.
        let result = weighted_average(5.0, 0.0, 0.0);
        assert_eq!(result, 5.0 / 1.0 - 1.0);
        assert!(result.is_finite());
    }

    #[test]
    fn weighted_average_negative_denominator_before_max_is_floored_to_one() {
        // pixelCount - countForThisBin = -50, max(-50, 1.0) = 1.0.
        let result = weighted_average(3.0, 10.0, 60.0);
        assert_eq!(result, 3.0 / 1.0 - 1.0);
    }

    #[test]
    fn weighted_average_single_sample_bin_zero_only() {
        // A single sample landing in bin 0: bin_sum contribution from bin 0
        // is always 0 (weighted_bin_value(count, 0) == 0), pixelCount=1,
        // countForThisBin(bin 0)=1.
        // weighted_average(bin_sum=0.0, pixel_count=1.0, count_for_bin_zero=1.0)
        // = (0.0 / max(1.0-1.0, 1.0)) - 1.0 = 0.0/1.0 - 1.0 = -1.0.
        assert_eq!(weighted_average(0.0, 1.0, 1.0), -1.0);
    }

    #[test]
    fn weighted_average_nan_bin_sum_propagates() {
        assert!(weighted_average(f32::NAN, 100.0, 0.0).is_nan());
    }

    #[test]
    fn weighted_average_infinite_bin_sum_propagates_infinity() {
        assert_eq!(weighted_average(f32::INFINITY, 100.0, 0.0), f32::INFINITY);
    }

    // --- weighted_average_luminance ---

    #[test]
    fn weighted_average_luminance_zero_weighted_average_is_min_luminance() {
        assert_eq!(weighted_average_luminance(0.0, 10.0, 0.5), 0.5);
    }

    #[test]
    fn weighted_average_luminance_full_range_scale() {
        // (62.0/62.0) * 10.0 + 0.5 = 10.5.
        assert_eq!(weighted_average_luminance(62.0, 10.0, 0.5), 10.5);
    }

    #[test]
    fn weighted_average_luminance_negative_weighted_average() {
        // (-1.0/62.0) * 8.0 + 1.0.
        let expected = (-1.0_f32 / 62.0_f32) * 8.0 + 1.0;
        assert_eq!(weighted_average_luminance(-1.0, 8.0, 1.0), expected);
    }

    #[test]
    fn weighted_average_luminance_nan_propagates() {
        assert!(weighted_average_luminance(f32::NAN, 10.0, 0.5).is_nan());
    }

    // --- adapt_luminance ---

    #[test]
    fn adapt_luminance_zero_time_delta_holds_last_frame_value() {
        // (1 - exp(-0*tau)) = (1 - 1) = 0, so adapted = lastFrame + 0.
        let result = adapt_luminance(5.0, 2.0, 0.0, 1.0);
        assert_eq!(result, 2.0);
    }

    #[test]
    fn adapt_luminance_large_time_delta_converges_toward_target() {
        // Large timeDelta*tau -> exp(-large) ~ 0 -> factor ~1 -> result ~
        // target.
        let result = adapt_luminance(5.0, 1.0, 1000.0, 1.0);
        assert!((result - 5.0).abs() < 1e-4, "result={result}");
    }

    #[test]
    fn adapt_luminance_infinite_last_frame_is_replaced_with_one() {
        let result = adapt_luminance(5.0, f32::INFINITY, 0.0, 1.0);
        // timeDelta=0 -> factor=0 -> adapted = replaced_last_frame (1.0) + 0
        // = 1.0, matching the isinf guard's replacement value, not INFINITY.
        assert_eq!(result, 1.0);
    }

    #[test]
    fn adapt_luminance_negative_infinite_last_frame_is_replaced_with_one() {
        let result = adapt_luminance(5.0, f32::NEG_INFINITY, 0.0, 1.0);
        assert_eq!(result, 1.0);
    }

    #[test]
    fn adapt_luminance_nan_last_frame_is_replaced_with_one() {
        let result = adapt_luminance(5.0, f32::NAN, 0.0, 1.0);
        assert_eq!(result, 1.0);
    }

    #[test]
    fn adapt_luminance_finite_last_frame_is_not_replaced() {
        // With timeDelta=0 the blend factor is 0, so adaptedLuminance
        // equals lastFrame verbatim if it was not replaced.
        let result = adapt_luminance(999.0, 3.5, 0.0, 1.0);
        assert_eq!(result, 3.5);
    }

    #[test]
    fn adapt_luminance_result_below_epsilon_is_floored() {
        // Target and lastFrame both drive the result below EPSILON (1e-6);
        // the final max(adaptedLuminance, EPSILON) floors it.
        let result = adapt_luminance(-5.0, -5.0, 0.0, 1.0);
        // timeDelta=0 -> adapted = -5.0 -> max(-5.0, 1e-6) = 1e-6.
        assert_eq!(result, 1e-6);
    }

    #[test]
    fn adapt_luminance_nan_weighted_average_luminance_poisons_result_but_floor_suppresses_it() {
        // adaptedLuminance becomes NaN (lastFrame + NaN*factor), but the
        // final max(NaN, EPSILON) is NaN-suppressing (f32::max semantics),
        // so the floor returns EPSILON, not NaN.
        let result = adapt_luminance(f32::NAN, 2.0, 1.0, 1.0);
        assert_eq!(result, 1e-6);
    }

    #[test]
    fn adapt_luminance_zero_tau_holds_last_frame_value() {
        // tau=0 -> -timeDelta*0 = 0 (or -0.0) -> exp(0)=1 -> factor=0 ->
        // adapted = lastFrame.
        let result = adapt_luminance(5.0, 2.0, 1.0, 0.0);
        assert_eq!(result, 2.0);
    }
}
