//! Literal port of `RT64::TMEMHasher`'s two pure TMEM-budget predicates: a
//! literal port of the permitted MIT RT64 Rust-port source pinned at commit
//! `5473732a822a4423b5696e7cb18fecc425a59875` (`docs/RT64-PORT-AUTHORITY.md`),
//! `src/common/rt64_tmem_hasher.h:39-46` (`needsToHashRowsIndividually`) and
//! `src/common/rt64_tmem_hasher.h:200-207` (`requiresRawTMEM`) (SHA-256 of
//! the whole file,
//! `3267cd4a85c61e1a960df175eb64641a75dceccb8d38e680bb0fcf85912d15c5`):
//!
//! ```text
//! static bool needsToHashRowsIndividually(const LoadTile &loadTile, uint32_t width) {
//!     // When using 32-bit formats, TMEM contents are split in half in the lower and upper half, so the size per row is effectively
//!     // the same as a 16-bit format as far as TMEM is concerned.
//!     const bool RGBA32 = (loadTile.siz == G_IM_SIZ_32b) && (loadTile.fmt == G_IM_FMT_RGBA);
//!     uint32_t drawBytesPerRow = std::max(width << (RGBA32 ? G_IM_SIZ_16b : loadTile.siz) >> 1U, 1U);
//!     uint32_t tmemBytesPerRow = loadTile.line << 3;
//!     return tmemBytesPerRow > drawBytesPerRow;
//! }
//!
//! static bool requiresRawTMEM(const LoadTile &loadTile, uint16_t width, uint16_t height, uint32_t tlutFormat) {
//!     const uint32_t TMEMBytes = 4096;
//!     const bool RGBA32 = (loadTile.siz == G_IM_SIZ_32b) && (loadTile.fmt == G_IM_FMT_RGBA);
//!     const uint32_t tmemSize = RGBA32 || (tlutFormat > 0) ? (TMEMBytes >> 1) : TMEMBytes;
//!     const uint32_t lastRowBytes = width << std::min(loadTile.siz, uint8_t(G_IM_SIZ_16b)) >> 1;
//!     const uint32_t bytesToHash = (loadTile.line << 3) * (height - 1) + lastRowBytes;
//!     return (bytesToHash > tmemSize);
//! }
//! ```
//!
//! The `LoadTile` fields these two predicates touch (`src/common/rt64_load_types.h:12-27`):
//!
//! ```text
//! struct LoadTile {
//!     uint8_t fmt;
//!     uint8_t siz;
//!     uint16_t line;
//!     // ...(other fields unused by these two predicates: tmem, palette, cms,
//!     // cmt, masks, maskt, shifts, shiftt, uls, ult, lrs, lrt)
//! };
//! ```
//!
//! **Reuse, not new type.** `fmt`'s only use in either predicate is the
//! equality check `loadTile.fmt == G_IM_FMT_RGBA`, so this port reuses
//! `crate::state::ImageFormat` directly (`ImageFormat::Rgba` stands in for
//! `G_IM_FMT_RGBA`) -- no raw integer, no new type. `siz`, however, is used
//! for *raw arithmetic* in both predicates: as a left-shift amount
//! (`width << ... siz`) and as an operand to `std::min(loadTile.siz,
//! uint8_t(G_IM_SIZ_16b))`. `crate::state::PixelSize` (`Bits4`/`Bits8`/
//! `Bits16`/`Bits32`) already exists and is the crate's established
//! `G_IM_SIZ_*` equivalent (see `endian_swap.rs`'s `EndianSwapUINT` port),
//! but it is deliberately opaque -- an exhaustive four-variant enum with no
//! numeric accessor, used elsewhere purely for `match` dispatch. These two
//! predicates need `siz`'s *numeric* `G_IM_SIZ_*` value (0/1/2/3), not a
//! dispatch target, so this module adds one local, private, non-`pub`
//! `pixel_size_g_im_siz(PixelSize) -> u32` const-fn ordinal mapping (the
//! well-known libultra `gbi.h` values `G_IM_SIZ_4b=0, G_IM_SIZ_8b=1,
//! G_IM_SIZ_16b=2, G_IM_SIZ_32b=3`, cross-checked against `endian_swap.rs`'s
//! `EndianSwapUINT` match order, which lists the same four sizes in the same
//! ascending order). This reuses `PixelSize` as the *type* (no new size
//! enum) while adding the minimal numeric bridge the arithmetic needs;
//! defining a whole new `LoadTile` mirror or a raw-`u8` `siz` field would
//! have thrown away the crate's existing, exhaustive size type for no
//! benefit.
//!
//! A minimal local `LoadTile` mirror (`fmt: ImageFormat, siz: PixelSize,
//! line: u16`) is still necessary: RT64's `LoadTile` also carries `tmem`,
//! `palette`, `cms`, `cmt`, `masks`, `maskt`, `shifts`, `shiftt`, `uls`,
//! `ult`, `lrs`, `lrt`, none of which either predicate reads, and
//! `crates/fn64-render-wgpu/src/tmem/types.rs` has no existing type that
//! carries exactly (and only) `{fmt, siz, line}` with these Rust types.
//!
//! ## Admitted domain
//!
//! - **`<<`/`>>` are same-precedence, left-associative in C++.** Both
//!   `width << (RGBA32 ? G_IM_SIZ_16b : loadTile.siz) >> 1U` and
//!   `width << std::min(loadTile.siz, uint8_t(G_IM_SIZ_16b)) >> 1` parse as
//!   `(width << shift) >> 1`, **not** `width << (shift >> 1)` -- the ternary
//!   and the `std::min` call each bind tighter than either shift operator
//!   (they are primary/function-call-level expressions), and once the shift
//!   amount is resolved, the two `<<`/`>>` operators are peers evaluated
//!   left-to-right. Verified against a standalone C++17 probe
//!   (`/tmp/probe2.cpp`/`probe4.cpp` in this session): `width=8, shift=3`
//!   gives `(8<<3)>>1 = 32`, the correct left-associative reading, versus
//!   `8<<(3>>1) = 16` for the wrong right-first reading -- confirming C++
//!   really does take the left-associative parse. [`draw_bytes_per_row`] and
//!   [`last_row_bytes`] each write this as an explicit `(width << shift) >>
//!   1` in Rust to make the already-resolved precedence visible at the call
//!   site rather than relying on Rust's (identical, but easy to
//!   second-guess) left-to-right same-precedence rule.
//! - **`std::max(..., 1U)` / `std::max(width << shift >> 1, 1)` clamp.**
//!   `needsToHashRowsIndividually`'s `drawBytesPerRow` is clamped to a
//!   minimum of 1 (covers `width == 0`, or any `width`/`shift` combination
//!   that right-shifts to 0) -- ported as Rust `.max(1)`.
//!   `requiresRawTMEM` has **no such clamp** on `lastRowBytes` -- it can
//!   legitimately be 0 (e.g. `width == 0`), and that 0 is used unclamped in
//!   the subsequent sum. This asymmetry between the two predicates is
//!   preserved exactly, not harmonized.
//! - **`std::min(loadTile.siz, uint8_t(G_IM_SIZ_16b))` in `requiresRawTMEM`.**
//!   Clamps the *shift amount* to at most `G_IM_SIZ_16b` (2): `siz` values
//!   0/1/2 (4b/8b/16b) pass through unchanged, and `siz == 3` (32b) is
//!   clamped down to 2 -- so `Bits32` and `Bits16` compute an *identical*
//!   `lastRowBytes` in `requiresRawTMEM`. This is a distinct policy from
//!   `needsToHashRowsIndividually`'s `RGBA32 ? G_IM_SIZ_16b : loadTile.siz`
//!   ternary (which only overrides the shift when the format is *also*
//!   `RGBA` at 32-bit, not for every 32-bit format) -- `requiresRawTMEM`
//!   clamps on `siz` alone, irrespective of `fmt`. Verified with a
//!   standalone probe: `siz=2` (16b) and `siz=3` (32b) at the same
//!   `width`/`fmt=CI` both produce `lastRowBytes=128` in `requiresRawTMEM`.
//! - **`(height - 1)` underflow when `height == 0`.** `height` is
//!   `uint16_t`. C++'s integer-promotion rules promote it to `int` for the
//!   subtraction, so `height - 1` is computed in `int` arithmetic: at
//!   `height == 0` this is `int(-1)`, a well-defined (non-UB) signed value,
//!   **not** a `uint16_t` wraparound to 65535. That `int(-1)` (or, for
//!   `height >= 1`, a nonnegative `int`) is then multiplied by
//!   `(loadTile.line << 3)` -- also `int` after promotion -- giving an
//!   `int` product (e.g. `line=1 -> 8 * -1 = -8`, itself well-defined `int`
//!   arithmetic at this magnitude). That `int` product is then added to
//!   `lastRowBytes` (`uint32_t`): the usual arithmetic conversions convert
//!   the `int` product to `uint32_t` *before* the addition (reinterpreting
//!   its bit pattern modulo 2^32, well-defined per the C++ standard for
//!   signed-to-unsigned conversion), and the subsequent `uint32_t + uint32_t`
//!   addition wraps modularly (also well-defined, unlike signed overflow).
//!   Net effect, confirmed with a standalone probe: `line=1, height=0`
//!   gives `bytesToHash = 4_294_967_292` (`u32::MAX - 3`) for any
//!   `lastRowBytes >= 4`, and more generally `line=0` (so the product term
//!   is legitimately `0 * -1 = 0`, no wraparound) leaves `bytesToHash ==
//!   lastRowBytes` unaffected by the `height == 0` case. So `height == 0`
//!   is **not** UB and **not** a `uint16_t` wraparound -- it is a
//!   well-defined signed-to-unsigned conversion the C++ standard guarantees,
//!   and this port reproduces it exactly (see [`bytes_to_hash`]) rather than
//!   rejecting it: `line.wrapping_shl(3) as i64` is not needed because the
//!   entire chain fits in `i64` before the final `as u32` reinterpret-cast,
//!   which matches C++'s int/uint32_t conversion bit-for-bit. Rust's debug
//!   build would otherwise panic on a literal `(height - 1)` `u16`
//!   subtraction underflow or an `i32 * i32` / `i32 as u32` path taken
//!   naively with wrapping assumed only at the cast -- this port computes
//!   the subtraction and product in `i64` (never overflows at these input
//!   magnitudes: max is roughly `65535 << 3` times `65535`, well inside
//!   `i64`) and performs the sign-reinterpreting conversion to `u32`
//!   explicitly via `as u32` on the full `i64` sum, truncating to the low 32
//!   bits exactly as C++'s `int -> uint32_t` conversion would for any value
//!   in this function's domain (`u16 << 3` and `u16 - 1` both stay within
//!   `i32` range, so the truncation is a no-op in practice, but the `i64`
//!   staging keeps every intermediate step panic-free and explicit rather
//!   than relying on `wrapping_*` calls scattered through the expression).
//! - **`uint32_t` wrapping in `tmemBytesPerRow = loadTile.line << 3`.**
//!   `line` is `u16`, promoted to `int`/effectively `u32` range here (max
//!   `65535 << 3 = 524280`, well within `u32`); no wraparound is reachable
//!   for any representable `u16` `line`, so this is a plain widening
//!   shift, ported as `(line as u32) << 3`.
//!
//! ## Nonclaims
//!
//! No GPU, WGSL, or production wiring (this module is not called from
//! anywhere yet, matching `rt64_common.rs`/`rt64_math.rs`'s
//! characterization-first precedent -- dead-code warnings on the unused
//! public surface are expected and correct), and no RT64 visual/pixel/
//! silicon parity or performance claim. `TMEMHasher::hash`
//! (`rt64_tmem_hasher.h:48-198`) is **deliberately not ported**: it depends
//! on XXH3 (`XXH3_64bits_reset`/`_update`/`_digest`), a hashing library not
//! currently a dependency of this crate, and pulling it in for a single
//! characterization-only module would be new dependency surface far beyond
//! this task's two-predicate scope. `TMEMHasher::CurrentHashVersion` and
//! `bitScanForward64` (both only used by `hash`) are likewise not ported for
//! the same reason. This module makes no claim of being wired to fn64's own
//! TMEM path (`crates/fn64-render-wgpu/src/tmem/` exists but cites no RT64
//! source for its own design -- see its module doc, "RT64 is not a hardware
//! authority here" -- and this module is not wired into it).

use crate::state::{ImageFormat, PixelSize};

/// The `G_IM_SIZ_*` numeric ordinal `PixelSize` deliberately does not expose
/// elsewhere (see module doc "Reuse, not new type"). Matches libultra
/// `gbi.h`'s well-known `G_IM_SIZ_4b=0, G_IM_SIZ_8b=1, G_IM_SIZ_16b=2,
/// G_IM_SIZ_32b=3`, and `endian_swap.rs`'s `EndianSwapUINT` match order.
const fn pixel_size_g_im_siz(siz: PixelSize) -> u32 {
    match siz {
        PixelSize::Bits4 => 0,
        PixelSize::Bits8 => 1,
        PixelSize::Bits16 => 2,
        PixelSize::Bits32 => 3,
    }
}

/// `G_IM_SIZ_16b`'s numeric value, standing in for the C++ enumerator of the
/// same name used directly in both ported predicates.
const G_IM_SIZ_16B: u32 = 2;

/// Minimal local mirror of RT64's `LoadTile` (`rt64_load_types.h:12-27`),
/// carrying only the three fields either ported predicate reads: `fmt`,
/// `siz`, `line` (see module doc "Reuse, not new type" for why the other
/// eleven `LoadTile` fields are omitted and why `fmt`/`siz` reuse
/// `crate::state::{ImageFormat, PixelSize}` rather than raw integers).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LoadTile {
    pub fmt: ImageFormat,
    pub siz: PixelSize,
    pub line: u16,
}

/// `RGBA32 = (loadTile.siz == G_IM_SIZ_32b) && (loadTile.fmt == G_IM_FMT_RGBA)`,
/// the shared boolean both predicates compute identically.
const fn is_rgba32(load_tile: &LoadTile) -> bool {
    matches!(load_tile.siz, PixelSize::Bits32) && matches!(load_tile.fmt, ImageFormat::Rgba)
}

/// `uint32_t drawBytesPerRow = std::max(width << (RGBA32 ? G_IM_SIZ_16b : loadTile.siz) >> 1U, 1U);`
/// shared shape used by `needsToHashRowsIndividually`. See module doc
/// "Admitted domain" for the `<<`/`>>` left-associative precedence and the
/// `max(...,1)` clamp.
const fn draw_bytes_per_row(load_tile: &LoadTile, width: u32) -> u32 {
    let shift = if is_rgba32(load_tile) {
        G_IM_SIZ_16B
    } else {
        pixel_size_g_im_siz(load_tile.siz)
    };
    let shifted = (width << shift) >> 1u32;
    if shifted > 1 {
        shifted
    } else {
        1
    }
}

/// Literal port of `TMEMHasher::needsToHashRowsIndividually`
/// (`rt64_tmem_hasher.h:39-46`). See module doc for the verbatim C++ and
/// "Admitted domain" for the shift-precedence and clamp notes.
pub fn needs_to_hash_rows_individually(load_tile: &LoadTile, width: u32) -> bool {
    let draw_bytes_per_row = draw_bytes_per_row(load_tile, width);
    let tmem_bytes_per_row = (load_tile.line as u32) << 3;
    tmem_bytes_per_row > draw_bytes_per_row
}

/// `uint32_t lastRowBytes = width << std::min(loadTile.siz, uint8_t(G_IM_SIZ_16b)) >> 1;`
/// See module doc "Admitted domain" for why this clamp differs from
/// `draw_bytes_per_row`'s `RGBA32 ? G_IM_SIZ_16b : siz` ternary, and why
/// there is no `max(...,1)` clamp here (unlike `draw_bytes_per_row`).
const fn last_row_bytes(load_tile: &LoadTile, width: u16) -> u32 {
    let siz_ordinal = pixel_size_g_im_siz(load_tile.siz);
    let shift = if siz_ordinal < G_IM_SIZ_16B {
        siz_ordinal
    } else {
        G_IM_SIZ_16B
    };
    ((width as u32) << shift) >> 1u32
}

/// `uint32_t bytesToHash = (loadTile.line << 3) * (height - 1) + lastRowBytes;`
/// See module doc "Admitted domain" for the exact C++ `height == 0`
/// signed-int-promotion-then-wrap-to-`uint32_t` semantics this reproduces
/// bit-for-bit via `i64` staging.
const fn bytes_to_hash(load_tile: &LoadTile, width: u16, height: u16) -> u32 {
    let line_shifted: i64 = (load_tile.line as i64) << 3;
    let height_minus_one: i64 = (height as i64) - 1;
    let product: i64 = line_shifted * height_minus_one;
    let last_row = last_row_bytes(load_tile, width) as i64;
    let sum: i64 = product + last_row;
    // Truncate to the low 32 bits, matching C++'s `int -> uint32_t`
    // conversion (well-defined modular reinterpretation).
    (sum & 0xFFFF_FFFF) as u32
}

/// Literal port of `TMEMHasher::requiresRawTMEM` (`rt64_tmem_hasher.h:200-207`).
/// See module doc for the verbatim C++ and "Admitted domain" for the
/// `min(siz, 16b)` clamp, the `height == 0` underflow semantics, and the
/// 4096-vs-2048 `tmemSize` split.
pub fn requires_raw_tmem(load_tile: &LoadTile, width: u16, height: u16, tlut_format: u32) -> bool {
    const TMEM_BYTES: u32 = 4096;
    let tmem_size = if is_rgba32(load_tile) || (tlut_format > 0) {
        TMEM_BYTES >> 1
    } else {
        TMEM_BYTES
    };
    let bytes_to_hash = bytes_to_hash(load_tile, width, height);
    bytes_to_hash > tmem_size
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tile(fmt: ImageFormat, siz: PixelSize, line: u16) -> LoadTile {
        LoadTile { fmt, siz, line }
    }

    // --- needs_to_hash_rows_individually: non-RGBA32 branch, every siz ---

    #[test]
    fn needs_rows_4b_tmem_wider_than_draw_is_true() {
        let lt = tile(ImageFormat::ColorIndex, PixelSize::Bits4, 512);
        // tmemBytesPerRow = 512<<3 = 4096; drawBytesPerRow = (64<<0)>>1 = 32.
        assert!(needs_to_hash_rows_individually(&lt, 64));
    }

    #[test]
    fn needs_rows_4b_tmem_equal_to_draw_is_false() {
        let lt = tile(ImageFormat::ColorIndex, PixelSize::Bits4, 1);
        // tmemBytesPerRow = 1<<3 = 8; drawBytesPerRow = (16<<0)>>1 = 8. 8>8 false.
        assert!(!needs_to_hash_rows_individually(&lt, 16));
    }

    #[test]
    fn needs_rows_8b_uses_shift_one() {
        let lt = tile(ImageFormat::ColorIndex, PixelSize::Bits8, 1);
        // drawBytesPerRow = (16<<1)>>1 = 16; tmemBytesPerRow = 8. 8>16 false.
        assert!(!needs_to_hash_rows_individually(&lt, 16));
    }

    #[test]
    fn needs_rows_16b_uses_shift_two() {
        let lt = tile(ImageFormat::IntensityAlpha, PixelSize::Bits16, 1);
        // drawBytesPerRow = (16<<2)>>1 = 32; tmemBytesPerRow = 8. false.
        assert!(!needs_to_hash_rows_individually(&lt, 16));
    }

    #[test]
    fn needs_rows_32b_non_rgba_uses_own_siz_shift_three() {
        // fmt is not RGBA, so RGBA32 is false even though siz is 32b:
        // shift stays loadTile.siz (3), not forced to G_IM_SIZ_16b.
        let lt = tile(ImageFormat::ColorIndex, PixelSize::Bits32, 512);
        // drawBytesPerRow = (64<<3)>>1 = 256; tmemBytesPerRow = 512<<3=4096. true.
        assert!(needs_to_hash_rows_individually(&lt, 64));
    }

    // --- needs_to_hash_rows_individually: RGBA32 branch forces shift=16b ---

    #[test]
    fn needs_rows_rgba32_forces_shift_to_16b_not_32b() {
        let lt = tile(ImageFormat::Rgba, PixelSize::Bits32, 1);
        // RGBA32 true: shift = G_IM_SIZ_16b = 2. drawBytesPerRow = (8<<2)>>1=16.
        // tmemBytesPerRow = 1<<3 = 8. 8>16 false.
        assert!(!needs_to_hash_rows_individually(&lt, 8));
    }

    #[test]
    fn needs_rows_rgba32_vs_non_rgba_32b_differ_for_same_inputs() {
        let rgba = tile(ImageFormat::Rgba, PixelSize::Bits32, 512);
        let ci = tile(ImageFormat::ColorIndex, PixelSize::Bits32, 512);
        // RGBA32: drawBytesPerRow=(64<<2)>>1=128; tmemBytesPerRow=4096 -> true.
        // Non-RGBA 32b: drawBytesPerRow=(64<<3)>>1=256; tmemBytesPerRow=4096 -> true too here,
        // so pick a width where only one crosses the threshold.
        assert!(needs_to_hash_rows_individually(&rgba, 64));
        assert!(needs_to_hash_rows_individually(&ci, 64));
        // Now a width where the RGBA32 (shift=2) draw bytes are smaller and
        // the non-RGBA32 (shift=3) draw bytes exceed tmemBytesPerRow's
        // complement, demonstrating the two really do use different shifts.
        let lt_line = tile(ImageFormat::Rgba, PixelSize::Bits32, 16);
        let lt_line_ci = tile(ImageFormat::ColorIndex, PixelSize::Bits32, 16);
        // tmemBytesPerRow = 16<<3 = 128.
        // width=64: RGBA32 drawBytesPerRow=(64<<2)>>1=128 -> 128>128 false.
        // width=64: CI drawBytesPerRow=(64<<3)>>1=256 -> 128>256 false too.
        assert!(!needs_to_hash_rows_individually(&lt_line, 64));
        assert!(!needs_to_hash_rows_individually(&lt_line_ci, 64));
    }

    // --- needs_to_hash_rows_individually: max(...,1) clamp boundary ---

    #[test]
    fn needs_rows_width_zero_clamps_draw_bytes_to_one() {
        let lt = tile(ImageFormat::ColorIndex, PixelSize::Bits16, 0);
        // drawBytesPerRow = max((0<<2)>>1, 1) = max(0,1) = 1.
        // tmemBytesPerRow = 0<<3 = 0. 0>1 false.
        assert!(!needs_to_hash_rows_individually(&lt, 0));
    }

    #[test]
    fn needs_rows_width_zero_with_nonzero_line_is_true() {
        let lt = tile(ImageFormat::ColorIndex, PixelSize::Bits16, 1);
        // drawBytesPerRow clamps to 1; tmemBytesPerRow = 1<<3 = 8. 8>1 true.
        assert!(needs_to_hash_rows_individually(&lt, 0));
    }

    #[test]
    fn needs_rows_shift_result_zero_from_small_width_clamps_to_one() {
        // width=1, siz=4b (shift 0): (1<<0)>>1 = 0, clamped to 1.
        let lt = tile(ImageFormat::ColorIndex, PixelSize::Bits4, 0);
        assert!(!needs_to_hash_rows_individually(&lt, 1));
    }

    #[test]
    fn needs_rows_width_one_exact_boundary_below_clamp() {
        // width=2, siz=4b (shift 0): (2<<0)>>1 = 1, exactly at the clamp
        // floor without needing the clamp to activate.
        let lt = tile(ImageFormat::ColorIndex, PixelSize::Bits4, 0);
        assert!(!needs_to_hash_rows_individually(&lt, 2));
    }

    // --- needs_to_hash_rows_individually: line = 0 ---

    #[test]
    fn needs_rows_line_zero_tmem_bytes_per_row_is_zero() {
        let lt = tile(ImageFormat::ColorIndex, PixelSize::Bits8, 0);
        // tmemBytesPerRow = 0<<3 = 0; drawBytesPerRow >= 1 always. 0 > x is
        // always false for tmemBytesPerRow=0, drawBytesPerRow>=1.
        assert!(!needs_to_hash_rows_individually(&lt, 100));
    }

    // --- needs_to_hash_rows_individually: exact-equality / straddling values ---

    #[test]
    fn needs_rows_straddle_just_above_threshold() {
        let lt = tile(ImageFormat::ColorIndex, PixelSize::Bits8, 9);
        // tmemBytesPerRow = 9<<3 = 72. drawBytesPerRow with width=142,
        // siz=8b(shift1): (142<<1)>>1 = 142. 72>142 false.
        assert!(!needs_to_hash_rows_individually(&lt, 142));
        // Now shrink width so drawBytesPerRow drops below 72.
        // width=71: (71<<1)>>1 = 71. 72>71 true.
        assert!(needs_to_hash_rows_individually(&lt, 71));
    }

    #[test]
    fn needs_rows_straddle_exact_equality_is_false() {
        let lt = tile(ImageFormat::ColorIndex, PixelSize::Bits8, 4);
        // tmemBytesPerRow = 4<<3 = 32. width=32, shift=1: (32<<1)>>1=32.
        // 32>32 false (strict greater-than).
        assert!(!needs_to_hash_rows_individually(&lt, 32));
    }

    // --- draw_bytes_per_row / helper sanity across siz for coverage breadth ---

    #[test]
    fn needs_rows_all_four_siz_values_non_rgba_produce_expected_shifts() {
        // width=256 fixed; line chosen so tmemBytesPerRow=2048 for all,
        // isolating the shift's effect on drawBytesPerRow.
        let line = 256; // tmemBytesPerRow = 256<<3 = 2048
        let siz_and_expected_draw_bytes = [
            (PixelSize::Bits4, (256u32 << 0) >> 1),  // 128
            (PixelSize::Bits8, (256u32 << 1) >> 1),  // 256
            (PixelSize::Bits16, (256u32 << 2) >> 1), // 512
            (PixelSize::Bits32, (256u32 << 3) >> 1), // 1024
        ];
        for (siz, expected_draw_bytes) in siz_and_expected_draw_bytes {
            let lt = tile(ImageFormat::ColorIndex, siz, line);
            let expected = 2048u32 > expected_draw_bytes;
            assert_eq!(
                needs_to_hash_rows_individually(&lt, 256),
                expected,
                "siz={siz:?} expected_draw_bytes={expected_draw_bytes}"
            );
        }
    }

    // --- requires_raw_tmem: RGBA32 vs non-RGBA32, every siz ---

    #[test]
    fn requires_raw_tmem_rgba32_uses_half_tmem_size() {
        let lt = tile(ImageFormat::Rgba, PixelSize::Bits32, 0);
        // tmemSize = 2048. lastRowBytes with siz clamped to 16b: (2048<<2)>>1=4096.
        // bytesToHash = 0 + 4096 = 4096 > 2048 -> true.
        assert!(requires_raw_tmem(&lt, 2048, 1, 0));
    }

    #[test]
    fn requires_raw_tmem_non_rgba32_uses_full_tmem_size() {
        let lt = tile(ImageFormat::ColorIndex, PixelSize::Bits32, 0);
        // Not RGBA -> RGBA32 false -> tmemSize = 4096 (no tlut).
        // lastRowBytes: siz clamped to min(3,2)=2: (2048<<2)>>1=4096.
        // bytesToHash = 4096 > 4096 -> false.
        assert!(!requires_raw_tmem(&lt, 2048, 1, 0));
    }

    #[test]
    fn requires_raw_tmem_siz_4b() {
        let lt = tile(ImageFormat::ColorIndex, PixelSize::Bits4, 512);
        // lastRowBytes = (64<<0)>>1 = 32. bytesToHash=(512<<3)*63+32=258080>4096 true.
        assert!(requires_raw_tmem(&lt, 64, 64, 0));
    }

    #[test]
    fn requires_raw_tmem_siz_8b() {
        let lt = tile(ImageFormat::ColorIndex, PixelSize::Bits8, 512);
        assert!(requires_raw_tmem(&lt, 64, 64, 0));
    }

    #[test]
    fn requires_raw_tmem_siz_16b() {
        let lt = tile(ImageFormat::ColorIndex, PixelSize::Bits16, 512);
        assert!(requires_raw_tmem(&lt, 64, 64, 0));
    }

    #[test]
    fn requires_raw_tmem_siz_32b_non_rgba_still_clamps_shift_to_16b() {
        // fmt is not RGBA, so tmemSize stays 4096, but the *shift* clamp in
        // lastRowBytes applies regardless of fmt.
        let lt = tile(ImageFormat::ColorIndex, PixelSize::Bits32, 512);
        assert!(requires_raw_tmem(&lt, 64, 64, 0));
    }

    #[test]
    fn requires_raw_tmem_siz_16b_and_32b_produce_identical_last_row_bytes() {
        // std::min(siz, G_IM_SIZ_16b) makes Bits16 and Bits32 compute the
        // same lastRowBytes in requiresRawTMEM (unlike
        // needsToHashRowsIndividually's fmt-gated RGBA32 ternary).
        let lt16 = tile(ImageFormat::ColorIndex, PixelSize::Bits16, 3);
        let lt32 = tile(ImageFormat::ColorIndex, PixelSize::Bits32, 3);
        assert_eq!(
            requires_raw_tmem(&lt16, 64, 64, 0),
            requires_raw_tmem(&lt32, 64, 64, 0)
        );
    }

    // --- requires_raw_tmem: min(siz, 16b) clamp boundary, explicit numbers ---

    #[test]
    fn requires_raw_tmem_min_clamp_siz_below_16b_passes_through() {
        let lt = tile(ImageFormat::ColorIndex, PixelSize::Bits8, 0);
        // siz=1 < G_IM_SIZ_16b(2): shift stays 1. lastRowBytes=(100<<1)>>1=100.
        assert!(!requires_raw_tmem(&lt, 100, 1, 0)); // 100 <= 4096
    }

    #[test]
    fn requires_raw_tmem_min_clamp_siz_at_16b_unchanged() {
        let lt = tile(ImageFormat::ColorIndex, PixelSize::Bits16, 0);
        // siz=2 == G_IM_SIZ_16b: min(2,2)=2. lastRowBytes=(100<<2)>>1=200.
        assert!(!requires_raw_tmem(&lt, 100, 1, 0));
    }

    #[test]
    fn requires_raw_tmem_min_clamp_siz_above_16b_clamped_down() {
        let lt = tile(ImageFormat::ColorIndex, PixelSize::Bits32, 0);
        // siz=3 > G_IM_SIZ_16b: min(3,2)=2. Same lastRowBytes=200 as 16b case.
        assert!(!requires_raw_tmem(&lt, 100, 1, 0));
    }

    // --- requires_raw_tmem: line = 0 ---

    #[test]
    fn requires_raw_tmem_line_zero_zeroes_the_product_term() {
        let lt = tile(ImageFormat::ColorIndex, PixelSize::Bits8, 0);
        // (0<<3)*(height-1) = 0 regardless of height, even height=0's -1:
        // 0 * -1 = 0, no wraparound reachable when line == 0.
        assert!(!requires_raw_tmem(&lt, 8, 0, 0));
        assert!(!requires_raw_tmem(&lt, 8, 100, 0));
    }

    // --- requires_raw_tmem: height = 0 underflow boundary (the critical case) ---

    #[test]
    fn requires_raw_tmem_height_zero_with_nonzero_line_wraps_to_huge_value() {
        let lt = tile(ImageFormat::ColorIndex, PixelSize::Bits16, 1);
        // line=1, height=0: (1<<3)*(0-1) = 8*(-1) = -8 (i64), + lastRowBytes.
        // lastRowBytes = (8<<2)>>1 = 16. sum = -8+16 = 8 as u32 -- NOT huge
        // here because line is small; pick a case that actually crosses
        // u32 zero to demonstrate the wraparound explicitly below.
        let bytes = bytes_to_hash(&lt, 8, 0);
        assert_eq!(bytes, 8);
    }

    #[test]
    fn requires_raw_tmem_height_zero_product_exceeds_last_row_bytes_wraps_near_u32_max() {
        // Reproduces the standalone C++ probe exactly: line=1, width=8,
        // siz=4b, height=0 -> lastRowBytes=(8<<0)>>1=4;
        // (1<<3)*(0-1) = -8 (i64); -8+4 = -4, reinterpreted as u32 ->
        // bytesToHash = 4_294_967_292 (u32::MAX - 3).
        let lt = tile(ImageFormat::ColorIndex, PixelSize::Bits4, 1);
        let bytes = bytes_to_hash(&lt, 8, 0);
        assert_eq!(bytes, 4_294_967_292);
        assert!(requires_raw_tmem(&lt, 8, 0, 0));
    }

    #[test]
    fn requires_raw_tmem_height_one_no_underflow_baseline() {
        let lt = tile(ImageFormat::ColorIndex, PixelSize::Bits16, 1);
        // height=1: (height-1)=0, product=0, bytesToHash = lastRowBytes = 16.
        let bytes = bytes_to_hash(&lt, 8, 1);
        assert_eq!(bytes, 16);
        assert!(!requires_raw_tmem(&lt, 8, 1, 0));
    }

    #[test]
    fn requires_raw_tmem_height_zero_vs_height_one_differ_sharply() {
        // siz=4b so the (line<<3)*(height-1) term at height=0 wraps past
        // lastRowBytes (see the wraps_near_u32_max test above) -- with 16b
        // the small line/width here would not wrap, so 4b is deliberate.
        let lt = tile(ImageFormat::ColorIndex, PixelSize::Bits4, 1);
        let at_zero = bytes_to_hash(&lt, 8, 0);
        let at_one = bytes_to_hash(&lt, 8, 1);
        assert_ne!(at_zero, at_one);
        assert!(at_zero > at_one, "at_zero={at_zero} at_one={at_one}");
    }

    #[test]
    fn requires_raw_tmem_height_two_no_underflow() {
        let lt = tile(ImageFormat::ColorIndex, PixelSize::Bits16, 1);
        // height=2: (height-1)=1, product=(1<<3)*1=8, + lastRowBytes=16 -> 24.
        let bytes = bytes_to_hash(&lt, 8, 2);
        assert_eq!(bytes, 24);
        assert!(!requires_raw_tmem(&lt, 8, 2, 0));
    }

    // --- requires_raw_tmem: width = 0 ---

    #[test]
    fn requires_raw_tmem_width_zero_last_row_bytes_is_zero() {
        let lt = tile(ImageFormat::ColorIndex, PixelSize::Bits16, 4);
        // lastRowBytes = (0<<2)>>1 = 0 (no max(...,1) clamp here, unlike
        // needsToHashRowsIndividually).
        let bytes = bytes_to_hash(&lt, 0, 1);
        assert_eq!(bytes, 0);
        assert!(!requires_raw_tmem(&lt, 0, 1, 0));
    }

    #[test]
    fn requires_raw_tmem_width_zero_and_height_zero_still_wraps_from_product() {
        let lt = tile(ImageFormat::ColorIndex, PixelSize::Bits16, 4);
        // lastRowBytes = 0, but (line<<3)*(height-1) = 32*(-1) = -32 still
        // wraps the total near u32::MAX.
        let bytes = bytes_to_hash(&lt, 0, 0);
        assert_eq!(bytes, (-32i64 & 0xFFFF_FFFF) as u32);
        assert!(requires_raw_tmem(&lt, 0, 0, 0));
    }

    // --- requires_raw_tmem: 4096 vs 2048 tmemSize split (RGBA32 or tlutFormat > 0) ---

    #[test]
    fn requires_raw_tmem_tlut_format_nonzero_halves_tmem_size_even_without_rgba32() {
        let lt = tile(ImageFormat::ColorIndex, PixelSize::Bits8, 0);
        // lastRowBytes = (2048<<1)>>1 = 2048. Full tmemSize (4096): false.
        // Halved tmemSize (2048, from tlutFormat>0): 2048>2048 false too --
        // pick a width that straddles only the halved boundary.
        assert!(!requires_raw_tmem(&lt, 2048, 1, 0));
        assert!(!requires_raw_tmem(&lt, 2048, 1, 1));
        let bytes = bytes_to_hash(&lt, 2049, 1);
        assert_eq!(bytes, 2049); // (2049<<1)>>1 = 2049 (odd width truncates by >>1 after <<1, no loss)
        assert!(!requires_raw_tmem(&lt, 2049, 1, 0)); // 2049 <= 4096
        assert!(requires_raw_tmem(&lt, 2049, 1, 1)); // 2049 > 2048
    }

    #[test]
    fn requires_raw_tmem_tlut_format_zero_keeps_full_tmem_size() {
        let lt = tile(ImageFormat::ColorIndex, PixelSize::Bits8, 0);
        assert!(!requires_raw_tmem(&lt, 4096, 1, 0)); // 4096 <= 4096
        assert!(requires_raw_tmem(&lt, 4097, 1, 0)); // 4097 > 4096
    }

    #[test]
    fn requires_raw_tmem_rgba32_and_tlut_format_nonzero_both_halve_identically() {
        let rgba = tile(ImageFormat::Rgba, PixelSize::Bits32, 0);
        let ci_with_tlut = tile(ImageFormat::ColorIndex, PixelSize::Bits8, 0);
        // Both should use tmemSize=2048 via different routes: RGBA32 alone,
        // or tlutFormat>0 alone.
        assert!(requires_raw_tmem(&rgba, 2049, 1, 0));
        assert!(requires_raw_tmem(&ci_with_tlut, 2049, 1, 1));
    }

    // --- requires_raw_tmem: values straddling the byte-budget comparison ---

    #[test]
    fn requires_raw_tmem_straddle_exact_equality_full_tmem_is_false() {
        let lt = tile(ImageFormat::ColorIndex, PixelSize::Bits4, 0);
        // lastRowBytes = (8192<<0)>>1 = 4096. bytesToHash = 4096. 4096>4096 false.
        assert!(!requires_raw_tmem(&lt, 8192, 1, 0));
    }

    #[test]
    fn requires_raw_tmem_straddle_one_over_full_tmem_is_true() {
        let lt = tile(ImageFormat::ColorIndex, PixelSize::Bits4, 0);
        // width=8194: (8194<<0)>>1 = 4097. 4097>4096 true.
        assert!(requires_raw_tmem(&lt, 8194, 1, 0));
    }

    #[test]
    fn requires_raw_tmem_straddle_via_line_term_crossing_budget() {
        let lt = tile(ImageFormat::ColorIndex, PixelSize::Bits8, 511);
        // line=511: tmemBytesPerRow-equivalent term = 511<<3 = 4088.
        // height=2: product = 4088*(2-1) = 4088. lastRowBytes=(2<<1)>>1=2.
        // bytesToHash = 4088+2 = 4090 <= 4096 -> false.
        assert!(!requires_raw_tmem(&lt, 2, 2, 0));
        // height=3: product = 4088*2 = 8176. bytesToHash = 8178 > 4096 -> true.
        assert!(requires_raw_tmem(&lt, 2, 3, 0));
    }

    #[test]
    fn requires_raw_tmem_straddle_halved_budget_via_tlut() {
        let lt = tile(ImageFormat::ColorIndex, PixelSize::Bits8, 255);
        // line=255: term = 255<<3 = 2040. height=2: product=2040.
        // lastRowBytes=(2<<1)>>1=2. bytesToHash=2042.
        // Full tmemSize 4096: false. Halved tmemSize 2048: 2042<=2048 false.
        assert!(!requires_raw_tmem(&lt, 2, 2, 0));
        assert!(!requires_raw_tmem(&lt, 2, 2, 1));
        // height=3: product=4080. bytesToHash=4082.
        // Full: 4082<=4096 false. Halved: 4082>2048 true.
        assert!(!requires_raw_tmem(&lt, 2, 3, 0));
        assert!(requires_raw_tmem(&lt, 2, 3, 1));
    }

    // --- pixel_size_g_im_siz / is_rgba32 direct coverage ---

    #[test]
    fn pixel_size_ordinal_matches_libultra_g_im_siz_values() {
        assert_eq!(pixel_size_g_im_siz(PixelSize::Bits4), 0);
        assert_eq!(pixel_size_g_im_siz(PixelSize::Bits8), 1);
        assert_eq!(pixel_size_g_im_siz(PixelSize::Bits16), 2);
        assert_eq!(pixel_size_g_im_siz(PixelSize::Bits32), 3);
    }

    #[test]
    fn is_rgba32_requires_both_32b_and_rgba_fmt() {
        assert!(is_rgba32(&tile(ImageFormat::Rgba, PixelSize::Bits32, 0)));
        assert!(!is_rgba32(&tile(
            ImageFormat::ColorIndex,
            PixelSize::Bits32,
            0
        )));
        assert!(!is_rgba32(&tile(ImageFormat::Rgba, PixelSize::Bits16, 0)));
        assert!(!is_rgba32(&tile(ImageFormat::Rgba, PixelSize::Bits4, 0)));
        assert!(!is_rgba32(&tile(ImageFormat::Rgba, PixelSize::Bits8, 0)));
    }

    // --- tlut_format boundary: 0 vs 1 (the > 0 comparison itself) ---

    #[test]
    fn requires_raw_tmem_tlut_format_zero_is_not_greater_than_zero() {
        let lt = tile(ImageFormat::ColorIndex, PixelSize::Bits8, 0);
        // tlutFormat=0 does not trigger the halved tmemSize; only strictly
        // positive values do (`tlutFormat > 0`).
        assert!(!requires_raw_tmem(&lt, 2049, 1, 0));
    }

    #[test]
    fn requires_raw_tmem_tlut_format_large_value_still_only_checked_for_positivity() {
        let lt = tile(ImageFormat::ColorIndex, PixelSize::Bits8, 0);
        // Any positive tlutFormat halves tmemSize identically -- the
        // predicate only checks `> 0`, not a specific format value.
        assert!(requires_raw_tmem(&lt, 2049, 1, 1));
        assert!(requires_raw_tmem(&lt, 2049, 1, u32::MAX));
    }
}
