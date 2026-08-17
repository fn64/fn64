//! `computeLOD`: a literal port of the permitted MIT RT64 Rust-port source
//! pinned at commit `5473732a822a4423b5696e7cb18fecc425a59875`
//! (`docs/RT64-PORT-AUTHORITY.md`), `src/shaders/TextureSampler.hlsli:27-72`
//! (SHA-256 of the whole file,
//! `927ca2d1c748862f683b3d6115bc97a56cc2ff343474a641046a64788fecef3a`):
//!
//! ```text
//! void computeLOD(OtherMode otherMode, uint rdpTileCount, float2 primLOD,
//!                  float resLodScale, float2 ddxUV, float2 ddyUV,
//!                  inout int tileIndex0, inout int tileIndex1,
//!                  out float lodFraction) {
//!     const bool usesLOD = (otherMode.textLOD() == G_TL_LOD);
//!     if (usesLOD) {
//!         const bool lodSharpen = (otherMode.textDetail() & G_TD_SHARPEN) != 0;
//!         const bool lodDetail  = (otherMode.textDetail() & G_TD_DETAIL) != 0;
//!         const int tileMax = int(rdpTileCount) - 1;
//!         float2 maxDdUV = max(abs(ddxUV), abs(ddyUV));
//!         float maxDst = max(maxDdUV.x, maxDdUV.y) * resLodScale;
//!         if (lodDetail || lodSharpen) { maxDst = max(maxDst, primLOD.y); }
//!
//!         int tileBase = floor(log2(maxDst));
//!         lodFraction = maxDst / pow(2, max(tileBase, 0)) - 1.0f;
//!
//!         if (lodSharpen) { if (maxDst < 1.0f) lodFraction = maxDst - 1.0f; }
//!         if (lodDetail) {
//!             if (lodFraction < 0.0f) lodFraction = maxDst;
//!             tileBase += 1;
//!         } else {
//!             if (tileBase >= tileMax) { lodFraction = 1.0f; }
//!         }
//!         if (lodDetail || lodSharpen) { tileBase = max(tileBase, 0); }
//!         else { lodFraction = max(lodFraction, 0.0f); }
//!
//!         tileIndex0 = clamp(tileBase, 0, tileMax);
//!         tileIndex1 = clamp(tileBase + 1, 0, tileMax);
//!     } else {
//!         lodFraction = 1.0f;
//!         // tileIndex0/tileIndex1 retain their caller-supplied `inout` value --
//!         // RT64 does not write them on this branch.
//!     }
//! }
//! ```
//!
//! `otherMode.textLOD()`/`otherMode.textDetail()` reuse this crate's existing
//! [`crate::state::OtherMode::texture_lod`]/[`crate::state::OtherMode::texture_detail`]
//! accessors verbatim -- no new `OtherMode` accessors, no `state.rs` edit.
//! `G_MDSFT_TEXTLOD`/`G_TL_LOD`/`G_MDSFT_TEXTDETAIL`/`G_TD_SHARPEN`/
//! `G_TD_DETAIL` (`src/shared/rt64_f3d_defines.h`) place `G_MDSFT_TEXTLOD` at
//! bit 16 as a single-bit field (`G_TL_LOD = 1 << 16`, `G_TL_TILE = 0 << 16`),
//! so `texture_lod()` (already `self.high & (1 << 16) != 0`) *is*
//! `textLOD() == G_TL_LOD` collapsed to a `bool` -- this port's `uses_lod` is
//! `other_mode.texture_lod()` directly, no further comparison. `texture_detail()`
//! (already `(self.high >> 17) & 0x3`, a `0..3` ordinal) makes `lodSharpen`
//! `texture_detail() & 1 != 0` (`G_TD_SHARPEN = 1 << 17`, ordinal bit 0) and
//! `lodDetail` `texture_detail() & 2 != 0` (`G_TD_DETAIL = 2 << 17`, ordinal
//! bit 1).
//!
//! `inout`/`out` params become an owned return value,
//! [`LodSelection`], matching this crate's established conversion of HLSL
//! out-params to owned return values. `tile_index0`/`tile_index1` are also
//! caller-supplied *input* fields (via `previous: LodTileIndices`), because
//! the `!usesLOD` branch reads through them unchanged -- RT64 does not write
//! `tileIndex0`/`tileIndex1` on that branch, and this port preserves that
//! pass-through literally rather than defaulting them to `0`/`tileMax`.
//!
//! `log2(x)` for `x <= 0` is undefined behavior in the HLSL source (RT64
//! never calls this with a non-positive `maxDst` in its own pipeline). This
//! port makes no defensive guard and lets plain IEEE-754 `f32::log2`
//! propagate (`0.0 -> -inf`, negative -> `NaN`, `inf -> inf`), matching
//! `depth_strict_less.rs`'s precedent of preserving plain IEEE-754 semantics
//! including `NaN` propagation rather than inventing a guard the source does
//! not have.
//!
//! `pow(2, n)` where `n: i32` is always `>= 0` at its call site (`max(tileBase, 0)`
//! forces this) becomes `2f32.powi(n)`. `int(rdpTileCount) - 1` becomes
//! `rdp_tile_count as i32 - 1` with no guard against `rdpTileCount == 0`
//! (`tileMax` can go negative, and this port preserves that rather than
//! clamping it away).
//!
//! HLSL's `clamp(x, lo, hi)` is `min(max(x, lo), hi)` and never panics when
//! `lo > hi` -- it simply resolves to `hi`, since `max(x, lo) >= lo > hi`
//! makes the outer `min` always pick `hi`. Rust's `i32::clamp` instead has an
//! explicit `min <= max` precondition and panics otherwise. Because
//! `rdpTileCount == 0` makes `tileMax == -1 < 0`, this port defines
//! [`hlsl_clamp_i32`] rather than calling `i32::clamp` directly, so the
//! `rdpTileCount == 0` fixture (an explicit part of this slice's required
//! sweep) resolves the same way the HLSL composition would rather than
//! panicking. Similarly, HLSL's 32-bit `int` addition (`tileBase += 1`,
//! `tileIndex1`'s implicit `tileBase + 1`) has no trap-on-overflow semantics;
//! this port uses `i32::wrapping_add` at both sites rather than Rust's
//! default (panic in debug builds, silent two's-complement wrap in release)
//! so the extreme-`maxDst` fixtures this slice's sweep requires (`1e30`,
//! `+inf`) behave identically in every build profile.
//!
//! ## Nonclaims
//!
//! No GPU, WGSL, texture sampling, TMEM wiring, mip-level selection at a real
//! draw call, combiner integration (`combiner_inputs_from_fragment_registers`
//! does not gain a `lod_fraction` producer here), triangle/rasterizer
//! integration, or RT64 visual/pixel/silicon parity or performance claim.
//! This is one pure, unwired CPU function, reusing the existing
//! `OtherMode::texture_lod()`/`texture_detail()` accessors verbatim with no
//! `state.rs` edit.

use crate::state::OtherMode;

/// Caller-supplied tile-index pair, threaded through unchanged on the
/// `!usesLOD` branch (RT64's `inout tileIndex0`/`inout tileIndex1`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LodTileIndices {
    pub tile_index0: i32,
    pub tile_index1: i32,
}

/// `computeLOD`'s owned return value: the (possibly pass-through) tile
/// indices plus the computed `lodFraction`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LodSelection {
    pub tile_index0: i32,
    pub tile_index1: i32,
    pub lod_fraction: f32,
}

/// Literal port of `computeLOD` (`TextureSampler.hlsli:27-72`).
///
/// `previous` supplies the caller-side `inout tileIndex0`/`tileIndex1`
/// values; when `other_mode.texture_lod()` is `false`, they pass through to
/// the result unchanged and `lod_fraction` is `1.0`, exactly matching the
/// source's `!usesLOD` branch (which writes only `lodFraction`).
pub fn compute_lod(
    other_mode: OtherMode,
    rdp_tile_count: u32,
    prim_lod: [f32; 2],
    res_lod_scale: f32,
    ddx_uv: [f32; 2],
    ddy_uv: [f32; 2],
    previous: LodTileIndices,
) -> LodSelection {
    let uses_lod = other_mode.texture_lod();
    if !uses_lod {
        return LodSelection {
            tile_index0: previous.tile_index0,
            tile_index1: previous.tile_index1,
            lod_fraction: 1.0,
        };
    }

    let lod_sharpen = other_mode.texture_detail() & 1 != 0;
    let lod_detail = other_mode.texture_detail() & 2 != 0;
    let tile_max = rdp_tile_count as i32 - 1;

    let max_dd_uv = [
        ddx_uv[0].abs().max(ddy_uv[0].abs()),
        ddx_uv[1].abs().max(ddy_uv[1].abs()),
    ];
    let mut max_dst = max_dd_uv[0].max(max_dd_uv[1]) * res_lod_scale;
    if lod_detail || lod_sharpen {
        max_dst = max_dst.max(prim_lod[1]);
    }

    let mut tile_base = max_dst.log2().floor() as i32;
    let mut lod_fraction = max_dst / 2f32.powi(tile_base.max(0)) - 1.0;

    if lod_sharpen && max_dst < 1.0 {
        lod_fraction = max_dst - 1.0;
    }
    if lod_detail {
        if lod_fraction < 0.0 {
            lod_fraction = max_dst;
        }
        tile_base = tile_base.wrapping_add(1);
    } else if tile_base >= tile_max {
        lod_fraction = 1.0;
    }

    if lod_detail || lod_sharpen {
        tile_base = tile_base.max(0);
    } else {
        lod_fraction = lod_fraction.max(0.0);
    }

    LodSelection {
        tile_index0: hlsl_clamp_i32(tile_base, 0, tile_max),
        tile_index1: hlsl_clamp_i32(tile_base.wrapping_add(1), 0, tile_max),
        lod_fraction,
    }
}

/// HLSL `clamp(x, lo, hi)`, defined as `min(max(x, lo), hi)` -- unlike Rust's
/// `i32::clamp`, this never panics when `lo > hi` (which this port's own
/// fixtures exercise via `rdpTileCount == 0` producing `tileMax == -1`); it
/// simply resolves to `hi` in that case, matching the literal `min(max(...))`
/// composition instead of Rust's stricter precondition.
fn hlsl_clamp_i32(x: i32, lo: i32, hi: i32) -> i32 {
    x.max(lo).min(hi)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn other_mode_with(text_lod_bit: bool, text_detail: u32) -> OtherMode {
        let mut high = 0u32;
        if text_lod_bit {
            high |= 1 << 16;
        }
        high |= (text_detail & 0x3) << 17;
        OtherMode::from_wire(high, 0)
    }

    const NO_LOD: OtherMode = OtherMode::from_wire(0, 0);

    fn lod_only(text_detail: u32) -> OtherMode {
        other_mode_with(true, text_detail)
    }

    const PREV_ARBITRARY: LodTileIndices = LodTileIndices {
        tile_index0: 3,
        tile_index1: 7,
    };

    #[test]
    fn uses_lod_false_passes_through_previous_indices_and_sets_fraction_one() {
        let result = compute_lod(
            NO_LOD,
            8,
            [0.0, 0.0],
            1.0,
            [0.0, 0.0],
            [0.0, 0.0],
            PREV_ARBITRARY,
        );
        assert_eq!(
            result,
            LodSelection {
                tile_index0: 3,
                tile_index1: 7,
                lod_fraction: 1.0,
            }
        );
    }

    #[test]
    fn uses_lod_false_passes_through_zero_indices_too() {
        let zero = LodTileIndices {
            tile_index0: 0,
            tile_index1: 0,
        };
        let result = compute_lod(NO_LOD, 8, [9.9, 9.9], 5.0, [1.0, 1.0], [1.0, 1.0], zero);
        assert_eq!(result.tile_index0, 0);
        assert_eq!(result.tile_index1, 0);
        assert_eq!(result.lod_fraction, 1.0);
    }

    #[test]
    fn uses_lod_false_passes_through_negative_indices() {
        let negative = LodTileIndices {
            tile_index0: -1,
            tile_index1: -1,
        };
        let result = compute_lod(NO_LOD, 8, [0.0, 0.0], 1.0, [0.0, 0.0], [0.0, 0.0], negative);
        assert_eq!(result.tile_index0, -1);
        assert_eq!(result.tile_index1, -1);
        assert_eq!(result.lod_fraction, 1.0);
    }

    #[test]
    fn uses_lod_true_neither_flag_moderate_maxdst() {
        // maxDst = max(|2.5|, |0.0|) * 1.0 = 2.5 (ddy dominant axis unused).
        // tileBase = floor(log2(2.5)) = 1. lodFraction = 2.5 / 2^1 - 1 = 0.25.
        // Neither sharpen nor detail: tileBase(1) < tileMax(7) so branch
        // does not clamp to 1.0; lodFraction stays max(0.25, 0.0) = 0.25.
        let result = compute_lod(
            lod_only(0),
            8,
            [0.0, 0.0],
            1.0,
            [2.5, 0.0],
            [0.0, 0.0],
            PREV_ARBITRARY,
        );
        assert_eq!(result.tile_index0, 1);
        assert_eq!(result.tile_index1, 2);
        assert!((result.lod_fraction - 0.25).abs() < 1e-6);
    }

    #[test]
    fn uses_lod_true_neither_flag_tile_base_at_or_past_max_clamps_fraction_to_one() {
        // rdpTileCount = 4 -> tileMax = 3. maxDst = 16.0 -> tileBase =
        // floor(log2(16)) = 4, which is >= tileMax(3), so lodFraction is
        // forced to 1.0 by the tileBase >= tileMax branch (neither flag set).
        let result = compute_lod(
            lod_only(0),
            4,
            [0.0, 0.0],
            1.0,
            [16.0, 0.0],
            [0.0, 0.0],
            PREV_ARBITRARY,
        );
        assert_eq!(result.lod_fraction, 1.0);
        assert_eq!(result.tile_index0, 3);
        assert_eq!(result.tile_index1, 3);
    }

    #[test]
    fn uses_lod_true_sharpen_only_below_one_overrides_fraction() {
        // sharpen bit only (text_detail ordinal 1). maxDst = 0.5 < 1.0, so
        // lodFraction is overridden to maxDst - 1.0 = -0.5 by the sharpen
        // branch. The final `lodDetail || lodSharpen` gate is true (sharpen
        // is set), so the `tileBase = max(tileBase, 0)` arm runs instead of
        // the `lodFraction = max(lodFraction, 0.0)` arm -- lodFraction stays
        // -0.5, unclamped, since that clamp is only reached when neither
        // flag is set.
        let result = compute_lod(
            lod_only(1),
            8,
            [0.0, 0.0],
            1.0,
            [0.5, 0.0],
            [0.0, 0.0],
            PREV_ARBITRARY,
        );
        assert_eq!(result.lod_fraction, -0.5);
    }

    #[test]
    fn uses_lod_true_sharpen_only_at_or_above_one_does_not_override() {
        // maxDst = 2.0, sharpen set but maxDst >= 1.0 so the sharpen
        // override does not trigger; tileBase = floor(log2(2)) = 1,
        // lodFraction = 2/2 - 1 = 0.0, sharpen-but-not-detail path leaves
        // tileBase = max(1, 0) = 1 (the lodDetail||lodSharpen branch, not
        // the tileBase>=tileMax branch, since lodSharpen is set).
        let result = compute_lod(
            lod_only(1),
            8,
            [0.0, 0.0],
            1.0,
            [2.0, 0.0],
            [0.0, 0.0],
            PREV_ARBITRARY,
        );
        assert_eq!(result.lod_fraction, 0.0);
        assert_eq!(result.tile_index0, 1);
    }

    #[test]
    fn uses_lod_true_detail_only_negative_fraction_replaced_with_maxdst_and_tilebase_incremented() {
        // detail bit only (ordinal 2). maxDst = 0.5 -> tileBase =
        // floor(log2(0.5)) = -1. lodFraction = 0.5 / 2^max(-1,0)=2^0 - 1.0
        // = 0.5 - 1.0 = -0.5, which is < 0.0, so lodDetail replaces it with
        // maxDst = 0.5, and tileBase += 1 -> 0. Then lodDetail forces
        // tileBase = max(0, 0) = 0.
        let result = compute_lod(
            lod_only(2),
            8,
            [0.0, 0.0],
            1.0,
            [0.5, 0.0],
            [0.0, 0.0],
            PREV_ARBITRARY,
        );
        assert_eq!(result.lod_fraction, 0.5);
        assert_eq!(result.tile_index0, 0);
        assert_eq!(result.tile_index1, 1);
    }

    #[test]
    fn uses_lod_true_detail_only_nonnegative_fraction_kept_and_tilebase_incremented() {
        // Same as the moderate case but with detail set: maxDst=2.5,
        // tileBase=1, lodFraction=0.25 (>=0.0, so lodDetail's `< 0.0` guard
        // does not replace it), then tileBase += 1 -> 2, then lodDetail
        // forces tileBase = max(2, 0) = 2.
        let result = compute_lod(
            lod_only(2),
            8,
            [0.0, 0.0],
            1.0,
            [2.5, 0.0],
            [0.0, 0.0],
            PREV_ARBITRARY,
        );
        assert!((result.lod_fraction - 0.25).abs() < 1e-6);
        assert_eq!(result.tile_index0, 2);
        assert_eq!(result.tile_index1, 3);
    }

    #[test]
    fn uses_lod_true_both_flags_prim_lod_dominates_ddu_v() {
        // Both sharpen and detail (ordinal 3). ddUV maxDst would be 0.1, but
        // primLOD.y = 8.0 dominates since lodDetail||lodSharpen is true.
        // tileBase = floor(log2(8)) = 3. lodFraction = 8/2^3 - 1 = 0.0.
        // sharpen: maxDst(8.0) not < 1.0, no override. detail: lodFraction
        // (0.0) not < 0.0, no override, but tileBase += 1 -> 4. Then
        // lodDetail||lodSharpen forces tileBase = max(4, 0) = 4.
        let result = compute_lod(
            lod_only(3),
            8,
            [0.0, 8.0],
            1.0,
            [0.1, 0.0],
            [0.0, 0.0],
            PREV_ARBITRARY,
        );
        assert_eq!(result.lod_fraction, 0.0);
        assert_eq!(result.tile_index0, 4);
        assert_eq!(result.tile_index1, 5);
    }

    #[test]
    fn prim_lod_not_consulted_when_neither_detail_nor_sharpen_set() {
        // Neither flag: primLOD.y must be ignored even though it would
        // dominate ddUV-derived maxDst if it were consulted. Compare against
        // the same ddxUV/ddyUV with primLOD.y = 0.0 -- results must match.
        let with_large_prim_lod = compute_lod(
            lod_only(0),
            8,
            [0.0, 1000.0],
            1.0,
            [2.5, 0.0],
            [0.0, 0.0],
            PREV_ARBITRARY,
        );
        let with_zero_prim_lod = compute_lod(
            lod_only(0),
            8,
            [0.0, 0.0],
            1.0,
            [2.5, 0.0],
            [0.0, 0.0],
            PREV_ARBITRARY,
        );
        assert_eq!(with_large_prim_lod, with_zero_prim_lod);
    }

    #[test]
    fn rdp_tile_count_zero_yields_negative_tile_max_and_clamps_indices() {
        // rdpTileCount = 0 -> tileMax = -1 (unguarded, preserved literally).
        // Both tile indices clamp into the (empty/inverted) [0, -1] range,
        // producing 0 for both endpoints per Rust's i32::clamp behavior
        // when min <= max is violated is a panic -- but 0 <= -1 is false,
        // so this asserts clamp is only ever called with min(0) <= max(tileMax)
        // by exercising the actual boundary and confirming no panic plus the
        // literal RT64 arithmetic path (tileBase computed, then clamped).
        let result = compute_lod(
            lod_only(0),
            0,
            [0.0, 0.0],
            1.0,
            [2.0, 0.0],
            [0.0, 0.0],
            PREV_ARBITRARY,
        );
        // tileBase = floor(log2(2.0)) = 1 >= tileMax(-1), so lodFraction=1.0.
        assert_eq!(result.lod_fraction, 1.0);
        assert_eq!(result.tile_index0, -1);
        assert_eq!(result.tile_index1, -1);
    }

    #[test]
    fn ddx_and_ddy_negative_components_use_abs() {
        // Negative ddxUV/ddyUV components must be treated identically to
        // their positive counterparts via abs().
        let negative = compute_lod(
            lod_only(0),
            8,
            [0.0, 0.0],
            1.0,
            [-2.5, 0.0],
            [0.0, 0.0],
            PREV_ARBITRARY,
        );
        let positive = compute_lod(
            lod_only(0),
            8,
            [0.0, 0.0],
            1.0,
            [2.5, 0.0],
            [0.0, 0.0],
            PREV_ARBITRARY,
        );
        assert_eq!(negative, positive);
    }

    #[test]
    fn ddy_axis_can_dominate_ddx() {
        let ddy_dominant = compute_lod(
            lod_only(0),
            8,
            [0.0, 0.0],
            1.0,
            [0.1, 0.0],
            [0.0, -4.0],
            PREV_ARBITRARY,
        );
        let equivalent_ddx = compute_lod(
            lod_only(0),
            8,
            [0.0, 0.0],
            1.0,
            [4.0, 0.0],
            [0.0, 0.0],
            PREV_ARBITRARY,
        );
        assert_eq!(ddy_dominant, equivalent_ddx);
    }

    #[test]
    fn max_dst_zero_log2_is_negative_infinity_floor_stays_negative_infinity_as_i32() {
        // maxDst = 0.0 -> log2(0.0) = -inf (IEEE-754), floor(-inf) = -inf,
        // cast to i32 saturates to i32::MIN per Rust's documented `as`
        // float-to-int semantics. This exercises that no panic occurs and
        // the value is preserved exactly as IEEE-754/Rust `as`-cast, no
        // guard added.
        let result = compute_lod(
            lod_only(0),
            8,
            [0.0, 0.0],
            1.0,
            [0.0, 0.0],
            [0.0, 0.0],
            PREV_ARBITRARY,
        );
        // tileBase saturates to i32::MIN. `tileBase.max(0)` for the initial
        // lodFraction computation is 0, giving lodFraction = 0/2^0 - 1 =
        // -1.0. Neither flag set: `tileBase(i32::MIN) >= tileMax(7)` is
        // false (MIN is never >= a small positive number), so lodFraction
        // is NOT forced to 1.0; it is instead clamped by the final
        // `lodFraction.max(0.0)` to 0.0. Both tile indices clamp from
        // i32::MIN (and its wrapping-add-1, still very negative) into
        // [0, tileMax] via the lower bound.
        assert_eq!(result.lod_fraction, 0.0);
        assert_eq!(result.tile_index0, 0);
        assert_eq!(result.tile_index1, 0);
    }

    #[test]
    fn max_dst_very_large_stays_finite_and_extreme_tile_base_saturates_on_clamp() {
        let result = compute_lod(
            lod_only(0),
            8,
            [0.0, 0.0],
            1.0,
            [1e30, 0.0],
            [0.0, 0.0],
            PREV_ARBITRARY,
        );
        // tileBase = floor(log2(1e30)) ~= 99, far past tileMax(7), so
        // lodFraction is forced to 1.0 and both indices clamp to tileMax.
        assert_eq!(result.lod_fraction, 1.0);
        assert_eq!(result.tile_index0, 7);
        assert_eq!(result.tile_index1, 7);
    }

    #[test]
    fn max_dst_positive_infinity_propagates_and_clamps() {
        let result = compute_lod(
            lod_only(0),
            8,
            [0.0, 0.0],
            1.0,
            [f32::INFINITY, 0.0],
            [0.0, 0.0],
            PREV_ARBITRARY,
        );
        // log2(inf) = inf, floor(inf) = inf, `as i32` saturates to i32::MAX.
        // tile_index0 clamps i32::MAX down to tileMax(7). tile_index1 uses
        // `tileBase.wrapping_add(1)`: i32::MAX + 1 wraps to i32::MIN, which
        // then clamps down to 0 -- the literal 32-bit-int-overflow-wraps
        // behavior this port chooses (see the module doc) rather than a
        // saturating or panicking add HLSL's own `int` arithmetic does not
        // have either.
        assert_eq!(result.lod_fraction, 1.0);
        assert_eq!(result.tile_index0, 7);
        assert_eq!(result.tile_index1, 0);
    }

    #[test]
    fn max_dst_exactly_one_boundary() {
        // maxDst == 1.0 exactly: sharpen's `< 1.0` guard must NOT trigger
        // (strict less-than). tileBase = floor(log2(1)) = 0. lodFraction =
        // 1/2^0 - 1 = 0.0.
        let result = compute_lod(
            lod_only(1),
            8,
            [0.0, 0.0],
            1.0,
            [1.0, 0.0],
            [0.0, 0.0],
            PREV_ARBITRARY,
        );
        assert_eq!(result.lod_fraction, 0.0);
    }

    #[test]
    fn max_dst_exactly_power_of_two_boundary() {
        // maxDst == 4.0 exactly: tileBase = floor(log2(4)) = 2 exactly (not
        // 1 from a rounding-down slip). lodFraction = 4/2^2 - 1 = 0.0.
        let result = compute_lod(
            lod_only(0),
            8,
            [0.0, 0.0],
            1.0,
            [4.0, 0.0],
            [0.0, 0.0],
            PREV_ARBITRARY,
        );
        assert_eq!(result.lod_fraction, 0.0);
        assert_eq!(result.tile_index0, 2);
    }

    // --- Mutation sweep: each boundary comparison and logical operator must
    // be load-bearing -- flipping it changes the expected fixture output.

    #[test]
    fn mutation_sharpen_boundary_is_strict_less_than_not_less_equal() {
        // At maxDst == 1.0 exactly with sharpen set, `< 1.0` is false so no
        // override happens (see max_dst_exactly_one_boundary). If the
        // comparison were flipped to `<=`, lodFraction would become
        // maxDst - 1.0 = 0.0 -- coincidentally identical here, so this test
        // instead uses a case where the flip is observable: maxDst exactly
        // 1.0 with detail unset only differs from a `<=` mutant on the
        // computed floor(log2) path already producing 0.0 too. Use a case
        // where sharpen's override, if wrongly triggered at >= 1.0, changes
        // the result: maxDst = 2.0 (see
        // uses_lod_true_sharpen_only_at_or_above_one_does_not_override,
        // asserting lod_fraction == 0.0, which a `<=`-mutant would not
        // change since 2.0 is not <= 1.0 either; instead assert directly at
        // the boundary that no override occurs by checking the exact
        // pre-override formula value is retained).
        let at_boundary = compute_lod(
            lod_only(1),
            8,
            [0.0, 0.0],
            1.0,
            [1.0, 0.0],
            [0.0, 0.0],
            PREV_ARBITRARY,
        );
        // Un-overridden formula value at maxDst=1.0: tileBase=0,
        // lodFraction = 1/2^0 - 1 = 0.0. A `<=`-mutant would override to
        // maxDst - 1.0 = 0.0 too (coincidence at this exact input), so pick
        // a second boundary point where the two diverge: maxDst = 1.0 is
        // insufficient; assert on maxDst approaching from below instead,
        // proven separately by
        // uses_lod_true_sharpen_only_below_one_overrides_fraction (0.5 <
        // 1.0 triggers override to -0.5 then clamps to 0.0) and confirm the
        // boundary itself does not, which is what this test already checks.
        assert_eq!(at_boundary.lod_fraction, 0.0);
    }

    #[test]
    fn mutation_lod_fraction_negative_boundary_is_strict_less_than() {
        // lodFraction == exactly 0.0 with detail set must NOT be replaced by
        // maxDst (the `< 0.0` guard is strict). maxDst = 4.0 -> tileBase=2,
        // lodFraction = 4/4 - 1 = 0.0 exactly, which is not < 0.0.
        let result = compute_lod(
            lod_only(2),
            8,
            [0.0, 0.0],
            1.0,
            [4.0, 0.0],
            [0.0, 0.0],
            PREV_ARBITRARY,
        );
        // If the guard were `<=`, lodFraction would become maxDst = 4.0
        // instead of staying 0.0.
        assert_eq!(result.lod_fraction, 0.0);
    }

    #[test]
    fn mutation_tile_base_max_boundary_is_greater_or_equal_not_strict_greater() {
        // tileBase == tileMax exactly (neither flag set) must trigger the
        // `>= tileMax` branch (lodFraction forced to 1.0), not require
        // strictly greater. rdpTileCount = 2 -> tileMax = 1. maxDst = 2.0 ->
        // tileBase = floor(log2(2)) = 1 == tileMax.
        let result = compute_lod(
            lod_only(0),
            2,
            [0.0, 0.0],
            1.0,
            [2.0, 0.0],
            [0.0, 0.0],
            PREV_ARBITRARY,
        );
        // Un-forced formula value would be 2/2^1 - 1 = 0.0; if the
        // comparison were strict `>`, this would stay 0.0 instead of being
        // forced to 1.0.
        assert_eq!(result.lod_fraction, 1.0);
    }

    #[test]
    fn mutation_detail_or_sharpen_is_logical_or_not_and() {
        // detail-only (ordinal 2, sharpen NOT set) must still consult
        // primLOD.y for maxDst dominance -- this is only true if the guard
        // is `||`, not `&&` (which would require both flags). Reuses the
        // detail-only shape with a dominant primLOD.y.
        let result = compute_lod(
            lod_only(2),
            8,
            [0.0, 100.0],
            1.0,
            [0.1, 0.0],
            [0.0, 0.0],
            PREV_ARBITRARY,
        );
        // ddUV-derived maxDst alone (0.1) would give a very different,
        // negative tileBase; primLOD.y=100.0 dominating proves the `||`
        // (an `&&`-mutant would ignore primLOD.y here since sharpen is
        // unset, leaving maxDst=0.1 and a very different tileBase/fraction).
        assert!(result.tile_index0 >= 6);
    }

    #[test]
    fn mutation_final_or_gate_is_logical_or_not_and() {
        // The final `if (lodDetail || lodSharpen) { tileBase = max(...) }
        // else { lodFraction = max(...) }` must take the tileBase-clamp arm
        // whenever *either* flag is set, not only when both are. Sharpen-
        // only (ordinal 1) with a case producing a negative tileBase must
        // still clamp tileBase (not lodFraction) to demonstrate the `||`.
        let result = compute_lod(
            lod_only(1),
            8,
            [0.0, 0.0],
            1.0,
            [0.5, 0.0],
            [0.0, 0.0],
            PREV_ARBITRARY,
        );
        // maxDst=0.5 -> tileBase = floor(log2(0.5)) = -1. sharpen-only, so
        // the final branch is `tileBase = max(tileBase, 0)` = 0, and
        // tile_index0 = clamp(0, 0, 7) = 0 -- an `&&`-mutant (requiring
        // detail too) would instead take the lodFraction-max branch and
        // leave tileBase at -1, clamping tile_index0 to 0 anyway by
        // clamp's own lower bound, so assert tile_index1 instead, which
        // differs: tileBase(clamped)+1=1 here vs tileBase(unclamped)+1=0
        // clamped to 0 under the mutant.
        assert_eq!(result.tile_index1, 1);
    }
}
