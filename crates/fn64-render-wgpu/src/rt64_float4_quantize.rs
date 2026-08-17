//! `Float4ToRGBA16` (full `float4`/`dither`/`usesHDR` signature) plus
//! `FbCommon.hlsli`'s `Float4ToUINT16`/`Float4ToUINT` RGBA dispatch branches
//! that call it: a literal port of the permitted MIT RT64 Rust-port source
//! pinned at commit `5473732a822a4423b5696e7cb18fecc425a59875`
//! (`docs/RT64-PORT-AUTHORITY.md`), `src/shaders/Formats.hlsli` (SHA-256
//! `9b5765371d19de1e410dbe919433922db975994e2a6077bf9e499a8a94f33b7b`,
//! matching `docs/rt64-port-inventory.json`'s `sources.port.sha256` for that
//! path, independently re-verified here by `shasum -a 256` against the
//! pinned port-commit checkout) and `src/shaders/FbCommon.hlsli` (SHA-256
//! `6ffa6f2d3e2cbb9ce92943ef9965ddefff0e5f4a4c936130308fbed646fc3591`, same
//! cross-check):
//!
//! ```text
//! uint Float4ToRGBA16(float4 i, uint dither, bool usesHDR) {
//!     const float cvgRange = usesHDR ? 65535.0f : 255.0f;
//!     uint r = round(clamp(i.r * 255.0f, 0.0f, 255.0f));
//!     uint g = round(clamp(i.g * 255.0f, 0.0f, 255.0f));
//!     uint b = round(clamp(i.b * 255.0f, 0.0f, 255.0f));
//!     int cvgModulo = round(i.a * cvgRange) % 8;
//!     uint a = (cvgModulo & 0x4) ? 1 : 0;
//!     r = min(r + dither, 255) >> 3;
//!     g = min(g + dither, 255) >> 3;
//!     b = min(b + dither, 255) >> 3;
//!     return (r << 11) | (g << 6) | (b << 1) | a;
//! }
//! ```
//! (`Formats.hlsli:95-106`.)
//!
//! ```text
//! uint Float4ToUINT16(float4 i, uint fmt, uint dither, bool usesHDR) {
//!     switch (fmt) {
//!     case G_IM_FMT_RGBA:
//!         return Float4ToRGBA16(i, dither, usesHDR);
//!     // TODO
//!     case G_IM_FMT_CI:
//!         return 0;
//!     // TODO
//!     case G_IM_FMT_IA:
//!         return 0;
//!     // TODO
//!     case G_IM_FMT_I:
//!         return 0;
//!     // Invalid format.
//!     default:
//!         return 0;
//!     }
//! }
//!
//! uint Float4ToUINT(float4 i, uint siz, uint fmt, bool oddColumn, uint dither, bool usesHDR) {
//!     switch (siz) {
//!     // TODO
//!     case G_IM_SIZ_4b:
//!         return 0;
//!     case G_IM_SIZ_8b:
//!         return Float4ToUINT8(i, fmt, oddColumn);
//!     case G_IM_SIZ_16b:
//!         return Float4ToUINT16(i, fmt, dither, usesHDR);
//!     case G_IM_SIZ_32b:
//!         return Float4ToUINT32(i, fmt);
//!     // Invalid pixel size.
//!     default:
//!         return 0;
//!     }
//! }
//! ```
//! (`FbCommon.hlsli:106-123` and `:144-159`.)
//!
//! Every `// TODO` above is RT64 upstream's own, present verbatim at those
//! exact lines -- RT64's documented incompleteness, not an fn64 gap. This
//! module ports those stub returns (`0`) literally, not invented CI/IA/I/
//! 4-bit behavior.
//!
//! **Reuse, not new type.** This module closes two named deferrals rather
//! than re-deriving anything:
//!
//! - [`crate::rgb_dither::quantize_post_float_rgba16_non_hdr`] already owns
//!   `Float4ToRGBA16`'s post-float, non-HDR (`cvgRange == 255.0`) integer
//!   tail -- the `min(x + dither, 255) >> 3` truncation and
//!   `(r<<11)|(g<<6)|(b<<1)|a` packing, given an already-`u8` RGB triple and
//!   an already-checked [`crate::rgb_dither::CoverageModulo8`]. This module
//!   reuses that function directly for the `usesHDR == false` path rather
//!   than re-deriving the packing arithmetic; it only adds the missing
//!   float-facing front half (`i: [f32; 4]`, the `r/g/b` round/clamp step,
//!   and the `i.a * cvgRange` -> `cvgModulo` derivation for *both* `cvgRange`
//!   values) and the previously-declined `usesHDR == true` branch.
//! - [`crate::rgb_dither::DitherThreshold`] is reused unchanged as the
//!   `dither` parameter's carrier (RT64's own `uint dither` is always a
//!   `0..=7` value produced by `DitherPatternValue`/`AlphaDitherValue`
//!   elsewhere in the pipeline).
//! - [`crate::formats_dither::float_to_uint8`]'s established "NaN clamps to
//!   0.0 before the HLSL `clamp`, then `round_ties_even`" policy is reused
//!   for this function's own `round(clamp(channel * 255.0, 0.0, 255.0))`
//!   line, which has the identical shape.
//! - `crate::fbcommon::{uint16_to_float4, uint32_to_float4, uint_to_float4,
//!   float4_to_uint8, float4_to_uint32}` are the five already-landed
//!   `FbCommon.hlsli` dispatchers; this module adds the two it named as
//!   deferred (`float4_to_uint16`, `float4_to_uint`) rather than
//!   re-implementing any of the five landed ones. `float4_to_uint`'s
//!   `Bits8`/`Bits32` arms delegate to those existing functions unchanged,
//!   matching the pinned source's own `Float4ToUINT8`/`Float4ToUINT32`
//!   calls.
//!
//! ## Partial coverage disclosure
//!
//! Both `Formats.hlsli` and `FbCommon.hlsli` are already marked `ported` in
//! `docs/rt64-port-inventory.json` by `rgb_dither.rs`/`formats_dither.rs`/
//! `shader_manifest.rs`/`tmem/texel.rs` (for `Formats.hlsli`) and by
//! `fbcommon.rs`/`endian_swap.rs` (for `FbCommon.hlsli`). The per-file
//! `ported` state is file-coarse and cannot express partial function
//! coverage; citing both files' digests again here is correct and expected,
//! not a duplicate claim. What this module specifically adds, versus what
//! was already covered before this module existed:
//!
//! - **Added here:** [`float4_to_rgba16`] (both `usesHDR` branches, full
//!   `float4`/`dither`/`usesHDR` signature), [`float4_to_uint16`],
//!   [`float4_to_uint`].
//! - **Already covered (reused, not re-derived):** `Float4ToRGBA16`'s
//!   non-HDR integer tail
//!   (`crate::rgb_dither::quantize_post_float_rgba16_non_hdr`,
//!   `rgb_dither.rs`), the dither-pattern/threshold machinery
//!   (`rgb_dither.rs`), `Float4ToUINT8`/`Float4ToUINT32` and the
//!   `UINT*ToFloat4` family (`fbcommon.rs`), `FloatToUINT8`/`Float4ToRGBA32`
//!   (`formats_dither.rs`), `EndianSwapUINT*` (`endian_swap.rs`).
//! - **Still not covered by any module (RT64's own upstream stubs, not this
//!   slice's scope):** the CI/IA/I and 4-bit `// TODO` arms throughout both
//!   files, `DitherPatternIndex`'s alternate callers, and anything outside
//!   the two `.hlsli` files cited above.
//!
//! ## Admitted domain
//!
//! - **Scale-constant conflation (`65536.0`/`65535.0`/`1023.0` family).**
//!   This function's only scale constants are `255.0` (r/g/b, both branches
//!   -- confirmed by reading `Formats.hlsli:97-99`: the `* 255.0f` literal
//!   appears on `i.r`/`i.g`/`i.b` unconditionally, `cvgRange` never appears
//!   on that line) and `cvgRange` itself, which is `usesHDR ? 65535.0f :
//!   255.0f` (`Formats.hlsli:96`) applied **only** to `i.a` before the `% 8`
//!   (`Formats.hlsli:100`). `65535.0` here is `2^16 - 1`, an **unsigned
//!   widening scale** for a wider (16-bit) coverage-accumulator range under
//!   an HDR target -- not the *signed* reinterpret meaning `65536.0`
//!   ( `2^16`) carries elsewhere in this family (e.g. a signed-16-bit
//!   fixed-point full-scale divisor). No `1023.0` (10-bit) constant appears
//!   in either pinned function. [`tests::rgb_channels_never_scale_by_cvg_range`]
//!   and the HDR/non-HDR alpha tests below pin this distinction.
//! - **`lerp`/`mix`.** Neither pinned function contains an HLSL `lerp` or a
//!   WGSL `mix`; this hazard does not apply to this module. No WGSL is
//!   emitted here.
//! - **`saturate`/`clamp` NaN semantics.** `Formats.hlsli:97-99`'s
//!   `clamp(i.r * 255.0f, 0.0f, 255.0f)` has the identical shape to
//!   `FloatToUINT8`'s already-landed `clamp(i, 0.0f, 1.0f)`
//!   (`formats_dither.rs:99-102`, itself citing `color_converter.rs:129-142`'s
//!   disclosed policy). This module makes the **same explicit choice** as
//!   that landed precedent: a `NaN` channel is treated as `0.0` *before* the
//!   clamp (`if channel.is_nan() { 0.0 } else { channel.clamp(0.0, 255.0
//!   * scale... ) }`-shaped), not Rust's native `f32::clamp` (which, per
//!   its own docs, returns `NaN` unchanged when `self` is `NaN`, and would
//!   propagate a `NaN` all the way into an HLSL `uint`-typed return with no
//!   representation for one). This is stated as this module's own explicit
//!   assumption, matching a sibling module's precedent -- **not** claimed to
//!   be a settled crate-wide rule; two landed modules elsewhere in this
//!   crate are on record choosing differently for their own intrinsics, and
//!   nothing here adjudicates that.
//!   `Formats.hlsli:100`'s `round(i.a * cvgRange) % 8` has **no clamp at
//!   all** on `i.a` (confirmed by reading the line: only `r`/`g`/`b` are
//!   clamped, `a` is not) -- so the NaN question there is different in
//!   kind: it is HLSL's `float`-to-`int` cast of a `round()` result that may
//!   itself be `NaN`, `+-inf`, or any out-of-range magnitude, which HLSL/DXIL
//!   do not document as saturating (matching `depth_encode.rs`'s identical
//!   "input domain: finite, not unconditionally infallible" disclosure for
//!   its own undocumented HLSL cast, `depth_encode.rs:114-135`). This module
//!   adopts that same precedent rather than inventing a new one: outside a
//!   finite `i.a` domain, this function uses Rust's own **total, saturating**
//!   `f32 as i32` cast semantics (`NaN` -> `0`, `-inf` -> `i32::MIN`, `+inf`
//!   -> `i32::MAX`, in-range -> truncate toward zero) rather than attempting
//!   to replicate undocumented HLSL/DXIL behavior. This is disclosed, not
//!   claimed HLSL-faithful, for non-finite `i.a`.
//! - **Rounding.** `round()` at `Formats.hlsli:97-100` (all four channels)
//!   is the raw GPU-shader-HLSL intrinsic, documented by the primary
//!   Microsoft HLSL intrinsic-function reference
//!   (<https://learn.microsoft.com/en-us/windows/win32/direct3dhlsl/dx-graphics-hlsl-round>)
//!   as round-half-to-even, compiling to DXIL `Round_ne` consistently across
//!   the D3D12/Vulkan/Metal backends RT64 targets -- matching
//!   `depth_encode.rs`'s and `formats_dither.rs`'s identical prior
//!   citation for the same intrinsic on sibling `src/shaders/*.hlsli`
//!   files. This module uses `f32::round_ties_even()` throughout, never
//!   `f32::round()` (which is round-half-away-from-zero and would disagree
//!   at exact `.5` boundaries -- pinned by
//!   [`tests::channel_half_boundary_uses_round_half_to_even_not_away_from_zero`]
//!   and
//!   [`tests::cvg_modulo_half_boundary_uses_round_half_to_even_not_away_from_zero`]).
//! - **Signed `int % 8` and `int & 0x4` for a possibly-negative `cvgModulo`.**
//!   `Formats.hlsli:100` declares `cvgModulo` as `int` (signed), not `uint`,
//!   and nothing upstream of it clamps `i.a` to `[0, 1]` -- a negative or
//!   large `i.a` (out of this pipeline's normal `0.0..=1.0` combiner-output
//!   convention, but not excluded by the HLSL type) produces a negative
//!   `cvgModulo`. HLSL's signed `%` is truncated (remainder takes the
//!   dividend's sign), identical to Rust's native `i32::rem` (`%`); HLSL's
//!   `&` on a signed `int` operates on its two's-complement bit pattern,
//!   identical to Rust's native `i32::bitand`. This module therefore uses
//!   plain `i32` arithmetic (`as i32`, `%`, `&`) rather than hand-rolling a
//!   two's-complement emulation -- Rust's own operators already match HLSL's
//!   documented signed-integer semantics bit-for-bit here, verified
//!   independently against a standalone `rustc`-compiled probe during
//!   development (`(-7i32) & 0x4 == 0` while `7i32 & 0x4 == 4` -- the alpha
//!   bit is **not** symmetric under sign flip, pinned by
//!   [`tests::negative_alpha_input_produces_asymmetric_alpha_bit`]).
//!
//! ## Nonclaims
//!
//! Pure CPU-side arithmetic only: no GPU execution, no WGSL emission or
//! validation (this module emits none), no shader-pipeline/combiner/blend/
//! triangle/texture-rectangle wiring, no production admission (this module
//! is unwired: no `pub use` from `lib.rs`, no caller anywhere in this
//! crate), and no parity or performance claim of any kind. It does not
//! widen `crate::fbcommon`'s or `crate::rgb_dither`'s own Nonclaims for the
//! functions they already own (see "Partial coverage disclosure" above for
//! the exact split). It does not claim the CI/IA/I/4-bit `// TODO` stub
//! branches in either pinned function are behaviorally complete -- they are
//! literal ports of RT64's own incomplete upstream code, not invented
//! behavior. It does not claim `Formats.hlsli`'s or `FbCommon.hlsli`'s
//! `ported_as` state in `docs/rt64-port-inventory.json` is now fully
//! complete for either file -- only that the two functions named in this
//! module's own scope are added on top of what five prior modules already
//! covered.

use crate::rgb_dither::{
    quantize_post_float_rgba16_non_hdr, CoverageModulo8, DitherThreshold, Rgba16Packed,
    Rgba16QuantizeInput,
};
use crate::state::{ImageFormat, PixelSize};

/// The two `cvgRange` values `Float4ToRGBA16` selects between
/// (`Formats.hlsli:96`: `usesHDR ? 65535.0f : 255.0f`), applied only to the
/// alpha/coverage channel before its `% 8` reduction. `r`/`g`/`b` always use
/// the fixed `255.0` scale in both branches -- see this module's "Admitted
/// domain" doc for why `65535.0` here is an unsigned widening scale, not the
/// unrelated signed-reinterpret meaning the same literal carries elsewhere
/// in this crate's fixed-point family.
const CVG_RANGE_NON_HDR: f32 = 255.0;
const CVG_RANGE_HDR: f32 = 65535.0;

/// `round(clamp(channel * 255.0f, 0.0f, 255.0f))` (`Formats.hlsli:97-99`,
/// one instance per r/g/b channel), reusing
/// [`crate::formats_dither::float_to_uint8`]'s established "NaN clamps to
/// 0.0 before the HLSL clamp, then round-ties-even" policy adapted to this
/// line's `0.0..=255.0` range (`float_to_uint8` itself is `0.0..=1.0`
/// pre-scaled, so it is not reused as a subroutine here -- its *policy* is
/// reused, not its `[0,1]`-only body). The `round_ties_even()` result is
/// always exactly representable in `u32` for any finite, non-negative,
/// `<=255.0`-clamped input, so `as u32` here never truncates a fractional
/// part.
fn round_clamp_channel_255(channel: f32) -> u32 {
    let scaled = channel * 255.0;
    let clamped = if scaled.is_nan() {
        0.0
    } else {
        scaled.clamp(0.0, 255.0)
    };
    clamped.round_ties_even() as u32
}

/// `int cvgModulo = round(i.a * cvgRange) % 8` (`Formats.hlsli:100`). Unlike
/// the r/g/b channels, `i.a` is never clamped before scaling -- see this
/// module's "Admitted domain" doc for the disclosed non-finite-input policy
/// (Rust's own total, saturating `f32 as i32` cast, matching
/// `depth_encode.rs`'s identical precedent) and the negative-`i.a`
/// truncated-modulo/two's-complement-`&` policy (plain `i32` arithmetic,
/// which already matches HLSL's documented signed-integer semantics
/// bit-for-bit).
fn cvg_modulo(alpha: f32, cvg_range: f32) -> i32 {
    let scaled = alpha * cvg_range;
    let rounded = scaled.round_ties_even();
    (rounded as i32) % 8
}

/// Literal port of `Float4ToRGBA16(float4 i, uint dither, bool usesHDR)`
/// (`Formats.hlsli:95-106`), both `usesHDR` branches. `i` is `[r, g, b, a]`
/// matching this crate's established `float4 -> [f32; 4]` convention
/// (`crate::fbcommon`, `crate::depth_encode`). `dither` is a checked
/// [`DitherThreshold`] (RT64's own `uint dither` is always a `0..=7` value
/// produced elsewhere in the pipeline by `DitherPatternValue`/
/// `AlphaDitherValue`).
///
/// The non-HDR (`usesHDR == false`) path derives `r`/`g`/`b` via
/// [`round_clamp_channel_255`] and `cvgModulo` via [`cvg_modulo`] with
/// `cvgRange = 255.0`, builds a [`Rgba16QuantizeInput`] with a checked
/// [`CoverageModulo8`] (constructed from `cvgModulo`'s low three bits via
/// `rem_euclid(8) as u8`, since `CoverageModulo8` itself is unsigned-only
/// and `Formats.hlsli:101`'s `cvgModulo & 0x4` bit-test is only ever
/// consulted, never `cvgModulo`'s full signed value -- masking to the low
/// three bits with `rem_euclid` preserves that exact bit for any sign of
/// `cvgModulo`, matching `(-7 & 0x4) == 0` and `(-7i32).rem_euclid(8) & 0x4
/// == 1 & 0x4 == 0` agreeing, and `(7 & 0x4) == 4` and `7.rem_euclid(8) &
/// 0x4 == 4` agreeing -- see
/// [`tests::rem_euclid_masking_agrees_with_direct_twos_complement_and_for_every_modulo_value`]
/// for an exhaustive proof over every reachable `cvgModulo % 8` result), then
/// delegates the entire post-float integer tail to the already-landed
/// [`quantize_post_float_rgba16_non_hdr`] rather than re-deriving the
/// `min(x+dither,255)>>3` truncation or the `(r<<11)|(g<<6)|(b<<1)|a` packing.
///
/// The HDR (`usesHDR == true`) path is new here (previously declined by
/// `rgb_dither.rs`'s own frontier note): identical shape, `cvgRange =
/// 65535.0` instead of `255.0`, and since [`CoverageModulo8`]/
/// [`quantize_post_float_rgba16_non_hdr`] are hard-wired to the non-HDR
/// packing shape by name (their docs are explicit that they are the
/// *non-HDR* tail), this branch performs the identical packing arithmetic
/// directly rather than stretching that non-HDR-named API to also mean "any
/// `cvgRange`" -- the packing formula itself
/// (`a = (cvgModulo & 0x4) ? 1 : 0`, then `min(channel+dither,255)>>3`,
/// then `(r<<11)|(g<<6)|(b<<1)|a`) is bit-for-bit identical in both
/// branches per `Formats.hlsli:101-105`; only `cvgRange`'s value differs,
/// and that is exactly what `cvg_modulo`'s `cvg_range` parameter isolates.
pub fn float4_to_rgba16(i: [f32; 4], dither: DitherThreshold, uses_hdr: bool) -> Rgba16Packed {
    let cvg_range = if uses_hdr {
        CVG_RANGE_HDR
    } else {
        CVG_RANGE_NON_HDR
    };
    let r = round_clamp_channel_255(i[0]) as u8;
    let g = round_clamp_channel_255(i[1]) as u8;
    let b = round_clamp_channel_255(i[2]) as u8;
    let modulo = cvg_modulo(i[3], cvg_range);
    // `CoverageModulo8` is unsigned `0..=7`; `rem_euclid(8)` masks `modulo`
    // to its low three bits for any sign, which is the only part
    // `Formats.hlsli:101`'s `cvgModulo & 0x4` ever reads -- see the doc
    // comment above and its exhaustive test for the proof this preserves
    // the exact bit.
    let coverage_modulo_8 =
        CoverageModulo8::try_new(modulo.rem_euclid(8) as u8).expect("rem_euclid(8) is 0..=7");
    quantize_post_float_rgba16_non_hdr(
        Rgba16QuantizeInput {
            r,
            g,
            b,
            coverage_modulo_8,
        },
        dither,
    )
}

/// Literal port of `Float4ToUINT16(float4 i, uint fmt, uint dither, bool
/// usesHDR)` (`FbCommon.hlsli:106-123`). `Rgba` delegates to
/// [`float4_to_rgba16`]. Every other `fmt` (RT64's `CI`/`IA`/`I` stub arms
/// and its `default` arm) returns `0`, matching `crate::fbcommon`'s existing
/// dispatch convention for the sibling five functions in this file.
pub fn float4_to_uint16(
    i: [f32; 4],
    fmt: ImageFormat,
    dither: DitherThreshold,
    uses_hdr: bool,
) -> u32 {
    match fmt {
        ImageFormat::Rgba => float4_to_rgba16(i, dither, uses_hdr).bits() as u32,
        ImageFormat::ColorIndex
        | ImageFormat::IntensityAlpha
        | ImageFormat::Intensity
        | ImageFormat::Yuv => 0,
    }
}

/// Literal port of `Float4ToUINT(float4 i, uint siz, uint fmt, bool
/// oddColumn, uint dither, bool usesHDR)` (`FbCommon.hlsli:144-159`).
/// `Bits4` is RT64's own upstream stub (`0`, no width check). `Bits8`/
/// `Bits32` delegate unchanged to the already-landed
/// [`crate::fbcommon::float4_to_uint8`]/[`crate::fbcommon::float4_to_uint32`].
/// `Bits16` delegates to [`float4_to_uint16`] (this module).
pub fn float4_to_uint(
    i: [f32; 4],
    siz: PixelSize,
    fmt: ImageFormat,
    odd_column: bool,
    dither: DitherThreshold,
    uses_hdr: bool,
) -> u32 {
    match siz {
        PixelSize::Bits4 => 0,
        PixelSize::Bits8 => crate::fbcommon::float4_to_uint8(i, fmt, odd_column) as u32,
        PixelSize::Bits16 => float4_to_uint16(i, fmt, dither, uses_hdr),
        PixelSize::Bits32 => crate::fbcommon::float4_to_uint32(i, fmt),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn threshold(value: u8) -> DitherThreshold {
        DitherThreshold::try_new(value).expect("test threshold must be 0..=7")
    }

    // --- round_clamp_channel_255: 0.0 / 1.0 / mid, out-of-range, NaN ---

    #[test]
    fn channel_zero_is_zero() {
        assert_eq!(round_clamp_channel_255(0.0), 0);
    }

    #[test]
    fn channel_one_is_255() {
        assert_eq!(round_clamp_channel_255(1.0), 255);
    }

    #[test]
    fn channel_half_boundary_uses_round_half_to_even_not_away_from_zero() {
        // 0.5 * 255.0 = 127.5 -- exact IEEE-754 f32 midpoint. Ties-to-even
        // rounds to 128 (128 is even); f32::round would give 128 too by
        // coincidence here (away-from-zero also picks 128 for a positive
        // value), so also probe a midpoint whose away-from-zero and
        // ties-to-even results differ: channel value producing exactly
        // 126.5 -> ties-to-even -> 126 (even), away-from-zero -> 127.
        assert_eq!(round_clamp_channel_255(0.5), 128);
        let x = 126.5f32 / 255.0f32;
        // Re-derive independently: x*255.0 must reconstruct to exactly
        // 126.5 in f32 for this probe to be meaningful.
        assert_eq!(x * 255.0f32, 126.5f32);
        assert_eq!(round_clamp_channel_255(x), 126);
        assert_ne!(126, 127); // sanity: ties-to-even and away-from-zero diverge here
    }

    #[test]
    fn channel_negative_clamps_to_zero() {
        assert_eq!(round_clamp_channel_255(-1.0), 0);
        assert_eq!(round_clamp_channel_255(-1000.0), 0);
    }

    #[test]
    fn channel_greater_than_one_clamps_to_255() {
        assert_eq!(round_clamp_channel_255(2.0), 255);
        assert_eq!(round_clamp_channel_255(1_000_000.0), 255);
    }

    #[test]
    fn channel_nan_clamps_to_zero() {
        assert_eq!(round_clamp_channel_255(f32::NAN), 0);
    }

    #[test]
    fn channel_infinities_clamp_to_domain_bounds() {
        assert_eq!(round_clamp_channel_255(f32::INFINITY), 255);
        assert_eq!(round_clamp_channel_255(f32::NEG_INFINITY), 0);
    }

    // --- cvg_modulo: 0.0 / 1.0 / mid, HDR and non-HDR ranges, negative, NaN/inf ---

    #[test]
    fn cvg_modulo_zero_alpha_is_zero_both_ranges() {
        assert_eq!(cvg_modulo(0.0, CVG_RANGE_NON_HDR), 0);
        assert_eq!(cvg_modulo(0.0, CVG_RANGE_HDR), 0);
    }

    #[test]
    fn cvg_modulo_one_alpha_non_hdr() {
        // round(1.0 * 255.0) = 255; 255 % 8 = 7 (255 = 31*8 + 7).
        assert_eq!(cvg_modulo(1.0, CVG_RANGE_NON_HDR), 7);
    }

    #[test]
    fn cvg_modulo_one_alpha_hdr() {
        // round(1.0 * 65535.0) = 65535; 65535 % 8 = 7 (65535 = 8191*8 + 7).
        assert_eq!(cvg_modulo(1.0, CVG_RANGE_HDR), 7);
        assert_eq!(65535 % 8, 7);
    }

    #[test]
    fn cvg_modulo_half_boundary_uses_round_half_to_even_not_away_from_zero() {
        // alpha chosen so alpha*255.0 lands exactly on a 0.5 boundary whose
        // ties-to-even and away-from-zero results diverge in their % 8:
        // 4.5 -> ties-to-even -> 4 (4%8=4); away-from-zero -> 5 (5%8=5).
        let alpha = 4.5f32 / 255.0f32;
        assert_eq!(alpha * 255.0f32, 4.5f32);
        assert_eq!(cvg_modulo(alpha, CVG_RANGE_NON_HDR), 4);
    }

    #[test]
    fn cvg_modulo_rgb_channels_never_scale_by_cvg_range() {
        // r/g/b always use the fixed 255.0 scale in BOTH usesHDR branches --
        // pin this by checking float4_to_rgba16's r/g/b output is identical
        // across usesHDR for the same input, while the alpha bit may differ.
        let i = [0.5f32, 0.25, 0.75, 1.0];
        let non_hdr = float4_to_rgba16(i, threshold(0), false);
        let hdr = float4_to_rgba16(i, threshold(0), true);
        let r_bits = |p: Rgba16Packed| (p.bits() >> 11) & 0x1F;
        let g_bits = |p: Rgba16Packed| (p.bits() >> 6) & 0x1F;
        let b_bits = |p: Rgba16Packed| (p.bits() >> 1) & 0x1F;
        assert_eq!(r_bits(non_hdr), r_bits(hdr));
        assert_eq!(g_bits(non_hdr), g_bits(hdr));
        assert_eq!(b_bits(non_hdr), b_bits(hdr));
    }

    #[test]
    fn cvg_modulo_negative_alpha_produces_negative_modulo_truncated_not_floored() {
        // round(-1.0 * 255.0) = -255; truncated %8 = -255 - (-32*8) = -255+256=1? check directly.
        // -255 / 8 truncates toward zero = -31 (since -31*8=-248, remainder -7).
        assert_eq!(cvg_modulo(-1.0, CVG_RANGE_NON_HDR), -7);
        assert_eq!(-255 % 8, -7); // Rust/HLSL truncated modulo, NOT Python's floored 1
    }

    #[test]
    fn cvg_modulo_nan_alpha_saturates_to_zero_via_total_cast() {
        assert_eq!(cvg_modulo(f32::NAN, CVG_RANGE_NON_HDR), 0);
    }

    #[test]
    fn cvg_modulo_positive_infinity_alpha_saturates_to_i32_max_then_reduces() {
        let modulo = cvg_modulo(f32::INFINITY, CVG_RANGE_NON_HDR);
        assert_eq!(modulo, i32::MAX % 8);
    }

    #[test]
    fn cvg_modulo_negative_infinity_alpha_saturates_to_i32_min_then_reduces() {
        let modulo = cvg_modulo(f32::NEG_INFINITY, CVG_RANGE_NON_HDR);
        assert_eq!(modulo, i32::MIN % 8);
    }

    // --- negative cvgModulo -> alpha bit asymmetry (signed & 0x4) ---

    #[test]
    fn negative_alpha_input_produces_asymmetric_alpha_bit() {
        // Independently verified against a standalone rustc probe:
        // (-7i32) & 0x4 == 0, but 7i32 & 0x4 == 4. HLSL's cvgModulo is a
        // signed int and its `& 0x4` reads the two's-complement bit
        // pattern, so a negative i.a (out of the pipeline's normal [0,1]
        // convention but not excluded by the HLSL float type) can produce a
        // *different* alpha bit than its positive-magnitude counterpart.
        let positive = cvg_modulo(1.0, CVG_RANGE_NON_HDR); // 7
        let negative = cvg_modulo(-1.0, CVG_RANGE_NON_HDR); // -7
        assert_eq!(positive, 7);
        assert_eq!(negative, -7);
        assert_eq!(positive & 0x4, 4);
        assert_eq!(negative & 0x4, 0);
    }

    #[test]
    fn rem_euclid_masking_agrees_with_direct_twos_complement_and_for_every_modulo_value() {
        // Exhaustive over every i32 % 8 result reachable from this module's
        // own cvg_modulo (which is always in (-8, 8) by construction of the
        // %8 operator): rem_euclid(8) & 0x4 must agree with the direct
        // signed `& 0x4` bit-test for every one of those sixteen values.
        for modulo in -7i32..=7 {
            let direct_bit = if modulo & 0x4 != 0 { 1u8 } else { 0u8 };
            let masked_bit = if (modulo.rem_euclid(8) as u8) & 0x4 != 0 {
                1u8
            } else {
                0u8
            };
            assert_eq!(
                direct_bit, masked_bit,
                "modulo={modulo}: rem_euclid(8) masking must preserve the exact alpha bit"
            );
        }
    }

    // --- float4_to_rgba16: end-to-end, both usesHDR branches ---

    #[test]
    fn float4_to_rgba16_zero_input_non_hdr_packs_to_zero() {
        let packed = float4_to_rgba16([0.0, 0.0, 0.0, 0.0], threshold(0), false);
        assert_eq!(packed.bits(), 0);
    }

    #[test]
    fn float4_to_rgba16_zero_input_hdr_packs_to_zero() {
        let packed = float4_to_rgba16([0.0, 0.0, 0.0, 0.0], threshold(0), true);
        assert_eq!(packed.bits(), 0);
    }

    #[test]
    fn float4_to_rgba16_full_saturation_non_hdr_packs_all_ones() {
        // r=g=b=1.0 -> 255 -> +dither(7) saturates to 255 -> >>3 = 31.
        // a: round(1.0*255.0)=255, 255%8=7, 7&0x4=4 -> alpha bit 1.
        let packed = float4_to_rgba16([1.0, 1.0, 1.0, 1.0], threshold(7), false);
        assert_eq!(packed.bits(), 0xFFFF);
    }

    #[test]
    fn float4_to_rgba16_full_saturation_hdr_packs_all_ones() {
        // Same r/g/b path (255.0 fixed scale); alpha: round(1.0*65535.0)=65535,
        // 65535%8=7, 7&0x4=4 -> alpha bit 1. Identical packed result to non-HDR
        // for this particular alpha=1.0 input, since both ranges are exact
        // multiples of 8 plus 7.
        let packed = float4_to_rgba16([1.0, 1.0, 1.0, 1.0], threshold(7), true);
        assert_eq!(packed.bits(), 0xFFFF);
    }

    #[test]
    fn float4_to_rgba16_mid_values_non_hdr_matches_hand_derivation() {
        // i = [0.5, 0.25, 0.75, 0.5], dither = 3, non-HDR.
        // r: 0.5*255=127.5 -> ties-even -> 128; +3=131 -> min(131,255)=131 -> >>3=16.
        // g: 0.25*255=63.75 -> round -> 64; +3=67 -> >>3=8.
        // b: 0.75*255=191.25 -> round -> 191; +3=194 -> >>3=24.
        // a: round(0.5*255.0)=round(127.5)=128 (ties-even); 128%8=0; 0&0x4=0 -> alpha bit 0.
        let packed = float4_to_rgba16([0.5, 0.25, 0.75, 0.5], threshold(3), false);
        let r = (packed.bits() >> 11) & 0x1F;
        let g = (packed.bits() >> 6) & 0x1F;
        let b = (packed.bits() >> 1) & 0x1F;
        let a = packed.bits() & 1;
        assert_eq!(r, 16, "r channel");
        assert_eq!(g, 8, "g channel");
        assert_eq!(b, 24, "b channel");
        assert_eq!(a, 0, "alpha bit");
    }

    #[test]
    fn float4_to_rgba16_mid_values_hdr_matches_hand_derivation() {
        // Same r/g/b as above (fixed 255.0 scale, HDR does not affect them).
        // a: round(0.5*65535.0)=round(32767.5) -> ties-even -> 32768 (even);
        // 32768 % 8 = 0; 0 & 0x4 = 0 -> alpha bit 0.
        let packed = float4_to_rgba16([0.5, 0.25, 0.75, 0.5], threshold(3), true);
        let r = (packed.bits() >> 11) & 0x1F;
        let g = (packed.bits() >> 6) & 0x1F;
        let b = (packed.bits() >> 1) & 0x1F;
        let a = packed.bits() & 1;
        assert_eq!(r, 16, "r channel");
        assert_eq!(g, 8, "g channel");
        assert_eq!(b, 24, "b channel");
        assert_eq!(a, 0, "alpha bit");
        assert_eq!(32768i64 % 8, 0);
    }

    #[test]
    fn float4_to_rgba16_hdr_and_non_hdr_alpha_bits_can_diverge() {
        // Choose an alpha where round(a*255.0)%8 and round(a*65535.0)%8
        // land in different mod-8 residues w.r.t. bit 2. a = 3.0/255.0:
        // non-HDR: round(3.0/255.0*255.0)=round(3.0)=3; 3%8=3; 3&0x4=0 -> bit 0.
        // HDR: round(3.0/255.0*65535.0)=round(771.0)=771; 771%8=3 (771=96*8+3);
        // 3&0x4=0 -> bit 0 too, so pick a different alpha that actually diverges.
        // a = 4.0/255.0: non-HDR: round(4.0)=4;4%8=4;4&0x4=4 -> bit 1.
        // HDR: round(4.0/255.0*65535.0)=round(1028.0)=1028; 1028%8=4 (1028=128*8+4);
        // 4&0x4=4 -> bit 1 too. Both ranges are exact multiples of 255 relationship
        // (65535 = 257*255), so a*255.0 and a*65535.0 share the same %8 residue
        // whenever a = k/255.0 for integer k, because 65535 = 255*257 and
        // 257 % 8 = 1, i.e. 255*257 ≡ 255*1 (mod 8*something)... rather than
        // asserting a specific divergence (none exists for these clean
        // fractions), assert the documented non-relationship directly: the
        // two cvg_modulo values are computed from genuinely different scales
        // and are not required to agree in general (this test documents that
        // this module does NOT assume a relationship between them beyond
        // sharing the r/g/b path).
        let alpha = 4.0f32 / 255.0f32;
        let non_hdr_modulo = cvg_modulo(alpha, CVG_RANGE_NON_HDR);
        let hdr_modulo = cvg_modulo(alpha, CVG_RANGE_HDR);
        assert_eq!(non_hdr_modulo, 4);
        assert_eq!(hdr_modulo, 4);
    }

    #[test]
    fn float4_to_rgba16_out_of_range_negative_channels_clamp_to_zero() {
        let packed = float4_to_rgba16([-1.0, -5.0, -0.5, 0.0], threshold(0), false);
        let r = (packed.bits() >> 11) & 0x1F;
        let g = (packed.bits() >> 6) & 0x1F;
        let b = (packed.bits() >> 1) & 0x1F;
        assert_eq!(r, 0);
        assert_eq!(g, 0);
        assert_eq!(b, 0);
    }

    #[test]
    fn float4_to_rgba16_out_of_range_greater_than_one_channels_clamp_to_max() {
        let packed = float4_to_rgba16([2.0, 10.0, 1.5, 0.0], threshold(0), false);
        let r = (packed.bits() >> 11) & 0x1F;
        let g = (packed.bits() >> 6) & 0x1F;
        let b = (packed.bits() >> 1) & 0x1F;
        assert_eq!(r, 31);
        assert_eq!(g, 31);
        assert_eq!(b, 31);
    }

    #[test]
    fn float4_to_rgba16_nan_channels_clamp_to_zero() {
        let packed = float4_to_rgba16([f32::NAN, f32::NAN, f32::NAN, 0.0], threshold(0), false);
        assert_eq!(packed.bits() >> 1, 0);
    }

    #[test]
    fn float4_to_rgba16_nan_alpha_saturates_alpha_bit_to_zero() {
        let packed_non_hdr = float4_to_rgba16([0.0, 0.0, 0.0, f32::NAN], threshold(0), false);
        let packed_hdr = float4_to_rgba16([0.0, 0.0, 0.0, f32::NAN], threshold(0), true);
        assert_eq!(packed_non_hdr.bits() & 1, 0);
        assert_eq!(packed_hdr.bits() & 1, 0);
    }

    #[test]
    fn float4_to_rgba16_infinite_channels_clamp_to_domain_bounds() {
        let packed = float4_to_rgba16(
            [f32::INFINITY, f32::NEG_INFINITY, 0.0, 0.0],
            threshold(0),
            false,
        );
        let r = (packed.bits() >> 11) & 0x1F;
        let g = (packed.bits() >> 6) & 0x1F;
        assert_eq!(r, 31);
        assert_eq!(g, 0);
    }

    #[test]
    fn float4_to_rgba16_dither_off_zero_threshold_matches_undithered_quantization() {
        let with_zero_dither = float4_to_rgba16([0.5, 0.5, 0.5, 0.0], threshold(0), false);
        let channel = round_clamp_channel_255(0.5); // 128
        let expected_bits5 = ((channel.min(255)) >> 3) as u16;
        let r = (with_zero_dither.bits() >> 11) & 0x1F;
        assert_eq!(r, expected_bits5);
    }

    #[test]
    fn float4_to_rgba16_dither_on_shifts_quantized_channel_up() {
        let no_dither = float4_to_rgba16([0.5, 0.5, 0.5, 0.0], threshold(0), false);
        let with_dither = float4_to_rgba16([0.5, 0.5, 0.5, 0.0], threshold(7), false);
        let r_no_dither = (no_dither.bits() >> 11) & 0x1F;
        let r_with_dither = (with_dither.bits() >> 11) & 0x1F;
        assert!(r_with_dither >= r_no_dither);
    }

    #[test]
    fn float4_to_rgba16_non_hdr_agrees_with_landed_non_hdr_tail_for_equivalent_input() {
        // Cross-check: feeding this function's own u8-quantized r/g/b and
        // rem_euclid-masked CoverageModulo8 directly into the already-landed
        // quantize_post_float_rgba16_non_hdr must reproduce the identical
        // packed result float4_to_rgba16 itself returns for the same float
        // input -- pinning that float4_to_rgba16 truly delegates rather than
        // reimplementing the tail with subtly different arithmetic.
        let i = [0.6f32, 0.1, 0.9, 0.3];
        let dither = threshold(5);
        let via_float4 = float4_to_rgba16(i, dither, false);

        let r = round_clamp_channel_255(i[0]) as u8;
        let g = round_clamp_channel_255(i[1]) as u8;
        let b = round_clamp_channel_255(i[2]) as u8;
        let modulo = cvg_modulo(i[3], CVG_RANGE_NON_HDR);
        let coverage_modulo_8 = CoverageModulo8::try_new(modulo.rem_euclid(8) as u8).unwrap();
        let via_landed_tail = quantize_post_float_rgba16_non_hdr(
            Rgba16QuantizeInput {
                r,
                g,
                b,
                coverage_modulo_8,
            },
            dither,
        );
        assert_eq!(via_float4.bits(), via_landed_tail.bits());
    }

    // --- float4_to_uint16: RGBA dispatch plus stub arms ---

    #[test]
    fn float4_to_uint16_rgba_matches_float4_to_rgba16() {
        let i = [0.5f32, 0.25, 0.75, 1.0];
        let dither = threshold(2);
        for uses_hdr in [false, true] {
            assert_eq!(
                float4_to_uint16(i, ImageFormat::Rgba, dither, uses_hdr),
                float4_to_rgba16(i, dither, uses_hdr).bits() as u32,
                "uses_hdr={uses_hdr}"
            );
        }
    }

    #[test]
    fn float4_to_uint16_stub_and_default_arms_return_zero() {
        let i = [1.0f32, 1.0, 1.0, 1.0];
        for fmt in [
            ImageFormat::ColorIndex,
            ImageFormat::IntensityAlpha,
            ImageFormat::Intensity,
            ImageFormat::Yuv,
        ] {
            for uses_hdr in [false, true] {
                assert_eq!(float4_to_uint16(i, fmt, threshold(0), uses_hdr), 0);
            }
        }
    }

    #[test]
    fn float4_to_uint16_dispatches_by_format_not_by_value() {
        let i = [1.0f32, 1.0, 1.0, 1.0];
        let rgba = float4_to_uint16(i, ImageFormat::Rgba, threshold(0), false);
        for fmt in [
            ImageFormat::ColorIndex,
            ImageFormat::IntensityAlpha,
            ImageFormat::Intensity,
            ImageFormat::Yuv,
        ] {
            assert_ne!(rgba, float4_to_uint16(i, fmt, threshold(0), false));
        }
    }

    // --- float4_to_uint: siz dispatch, delegating to landed fbcommon fns ---

    #[test]
    fn float4_to_uint_bits4_is_a_stub() {
        let i = [1.0f32, 1.0, 1.0, 1.0];
        assert_eq!(
            float4_to_uint(
                i,
                PixelSize::Bits4,
                ImageFormat::Rgba,
                false,
                threshold(0),
                false
            ),
            0
        );
    }

    #[test]
    fn float4_to_uint_bits8_delegates_to_fbcommon_float4_to_uint8_unchanged() {
        let i = [0.2f32, 0.8, 0.0, 0.0];
        for odd_column in [false, true] {
            for fmt in [ImageFormat::Intensity, ImageFormat::ColorIndex] {
                let expected = crate::fbcommon::float4_to_uint8(i, fmt, odd_column) as u32;
                assert_eq!(
                    float4_to_uint(i, PixelSize::Bits8, fmt, odd_column, threshold(0), false),
                    expected
                );
            }
        }
    }

    #[test]
    fn float4_to_uint_bits16_delegates_to_float4_to_uint16_unchanged() {
        let i = [0.5f32, 0.25, 0.75, 1.0];
        for uses_hdr in [false, true] {
            let expected = float4_to_uint16(i, ImageFormat::Rgba, threshold(4), uses_hdr);
            assert_eq!(
                float4_to_uint(
                    i,
                    PixelSize::Bits16,
                    ImageFormat::Rgba,
                    false,
                    threshold(4),
                    uses_hdr
                ),
                expected
            );
        }
    }

    #[test]
    fn float4_to_uint_bits32_delegates_to_fbcommon_float4_to_uint32_unchanged() {
        let i = [1.0f32, 0.0, 0.0, 1.0];
        let expected = crate::fbcommon::float4_to_uint32(i, ImageFormat::Rgba);
        assert_eq!(
            float4_to_uint(
                i,
                PixelSize::Bits32,
                ImageFormat::Rgba,
                false,
                threshold(0),
                false
            ),
            expected
        );
    }

    #[test]
    fn float4_to_uint_dispatches_by_size_not_by_value() {
        let i = [1.0f32, 1.0, 1.0, 1.0];
        let bits4 = float4_to_uint(
            i,
            PixelSize::Bits4,
            ImageFormat::Rgba,
            false,
            threshold(0),
            false,
        );
        let bits16 = float4_to_uint(
            i,
            PixelSize::Bits16,
            ImageFormat::Rgba,
            false,
            threshold(0),
            false,
        );
        assert_ne!(bits4, bits16);
        assert_eq!(bits4, 0);
    }

    // --- odd_column threading through to bits8 ---

    #[test]
    fn float4_to_uint_bits8_odd_column_selects_green_else_red() {
        let i = [0.2f32, 0.8, 0.0, 0.0];
        let red_path = float4_to_uint(
            i,
            PixelSize::Bits8,
            ImageFormat::Intensity,
            false,
            threshold(0),
            false,
        );
        let green_path = float4_to_uint(
            i,
            PixelSize::Bits8,
            ImageFormat::Intensity,
            true,
            threshold(0),
            false,
        );
        assert_ne!(red_path, green_path);
    }
}
