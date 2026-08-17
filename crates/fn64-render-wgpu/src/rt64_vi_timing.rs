//! RT64's `VI` accessor arithmetic: framebuffer size/address estimation, the
//! scale reciprocals, the gamma exponent, and the `VIHistory` ring.
//!
//! Ported from the Rust-port authority pin
//! `5473732a822a4423b5696e7cb18fecc425a59875`
//! (`docs/rt64-port-authority.json`'s `port_source.commit`). The cited file is
//! `port_delta: unchanged` with an identical `oracle.sha256`, so the citation
//! is unambiguous against either pin.
//!
//! ## Cited sources and their digests
//!
//! | file | whole-file SHA-256 | lines | ported |
//! |---|---|---:|---|
//! | `src/hle/rt64_vi.cpp` | `9b3cf39bb15fc0c7d52085566197042f4960cc410b241e38457bb817f2501e5b` | 177 | partial (~74/177) |
//!
//! The digest was recomputed with `shasum -a 256` against the pinned checkout
//! and cross-checked three ways: against `docs/rt64-port-inventory.json`'s
//! `sources.port.sha256` for `src/hle/rt64_vi.cpp`, against that entry's
//! `sources.oracle.sha256` (byte-identical, hence `port_delta: unchanged`),
//! and against the independent copy the build gate pins at
//! `crates/fn64-render-rt64/ffi/CMakeLists.txt:1093`.
//!
//! ## Why an `authority-gated` file is portable here
//!
//! `docs/rt64-port-inventory.json` records `src/hle/rt64_vi.cpp` as
//! `authority-gated`. That state is a **build-time tripwire, not a
//! prohibition**: fn64 does not vendor RT64's C++, it rewrites two exact
//! string contexts at configure time
//! (`crates/fn64-render-rt64/ffi/CMakeLists.txt:1085-1140`), and the gate pins
//! the pristine upstream digest so a silent upstream change cannot make a
//! patch land somewhere unintended.
//!
//! The overlay's reach is worth stating precisely, because it bounds what this
//! module may safely compare against. It replaces exactly two contexts:
//!
//! 1. the `#include "rt64_vi.h"` / `namespace RT64 {` block, to declare
//!    `extern "C" uint32_t fn64_rt64_nominal_full_rate(const void *);`, and
//! 2. the single line `const uint32_t FullRate = 60; // TODO: PAL support.`
//!    (`src/hle/rt64_vi.cpp:166`), to bind the rate to fn64's IPL-selected
//!    50/60 Hz standard.
//!
//! **Nothing else in the file is patched.** Every construct this module ports
//! is therefore compared against text that the fn64 build consumes verbatim.
//! `logicalRateFromFactors`'s numerator is the one construct that is *not*,
//! and it is refused below for exactly that reason.
//!
//! ## Inventory drift, per file
//!
//! `src/hle/rt64_vi.cpp` reads `authority-gated` before this module. A sibling
//! lane is concurrently changing `port_state_for` so digest evidence outranks
//! the gate; if that lands, this entry moves to `ported`. **The file is not
//! fully ported** -- ~74 of 177 lines, itemized below. If the entry still
//! reads `authority-gated` after this module lands, that is the expected
//! outcome of the sibling lane not having landed, not a defect here.
//!
//! The inventory is deliberately **not** regenerated (the standing brief §8):
//! a concurrent lane owns `docs/rt64-port-inventory.json`, and a separate
//! `docs: regenerate inventory for ...` commit is the only writer.
//!
//! ## Ported / refused boundary, and the criterion
//!
//! The standing criterion: *a construct is ported when its behavior is fully
//! determined by values and control flow present in the cited file -- no GPU,
//! no ImGui context, no type from an uncited file.*
//!
//! **Ported (~74 of 177 lines):** `gamma` (`:26-29`), `visible` (`:44-46`),
//! `fbSiz` (`:66-76`), `fbAddress` (`:78-93`), `fbSize` (`:95-125`), and the
//! four scale/offset accessors (`:127-141`).
//!
//! **Refused:**
//!
//! - **`viewRectangle` / `cropRectangle`** (`:18-24`) -- both return the
//!   constant `{0,0,1,1}`. Porting them would mean introducing a four-float
//!   vector type for two constants that carry no arithmetic, against
//!   `AGENTS.md`'s one-vector-type-per-port rule. Recorded, not typed.
//! - **`compatibleWith`** (`:31-42`) and **`operator!=`** (`:48-64`) -- both
//!   are field-by-field comparisons over the `VI` struct declared in
//!   `src/hle/rt64_vi.h`, an uncited file, and both would require this module
//!   to define a competing `VI` struct. `fn64_render::ViScanoutRegisters`
//!   already owns the fourteen-word image and
//!   `crates/fn64-render-wgpu/src/rt64_vi_registers.rs` already owns the
//!   bitfield extents; a third owner is exactly the duplicate the standing
//!   brief §1 warns about. Their *predicates* are compared in prose below
//!   without being reimplemented.
//! - **`VIHistory`** (`:143-177`) -- `pushVI`, `pushFactor`, `top` and the
//!   constructor are a three-entry ring over a `Present { VI, uint32_t }`,
//!   which again needs the uncited `VI` struct.
//! - **`logicalRateFromFactors`** (`:164-172`) -- **already owned, and already
//!   diverged.** `crates/fn64-render-rt64/` binds this exact function through
//!   FFI (`fn64_rt64_probe_logical_rate`,
//!   `crates/fn64-render-rt64/src/ffi/config_wire.rs:442-447`) and its tests
//!   pin `(60,1)->60`, `(60,2)->30`, `(50,1)->50`, `(50,2)->25`
//!   (`crates/fn64-render-rt64/src/ffi/tests.rs:653-658`). Those PAL rows
//!   cannot come from pinned RT64 at all: the overlay replaces the hardcoded
//!   `FullRate = 60`. Re-deriving the function here would build a third owner
//!   of a fact fn64 has already both ported *and* corrected. Cited and
//!   refused, per the standing brief §4.
//!
//! ## Verbatim key logic
//!
//! `src/hle/rt64_vi.cpp:95-141`, the reciprocal and the estimator it feeds --
//! the excerpt a reviewer should read this module against:
//!
//! ```text
//! hlslpp::uint2 VI::fbSize() const {
//!     hlslpp::uint2 size = { width, 0 };
//!
//!     if (status.serrate) {
//!         const float estimatedWidth = (hRegion.hEnd - hRegion.hStart) / xScaleFloat();
//!         const float interlacedTolerance = 1.875f;
//!         if (estimatedWidth < (width / interlacedTolerance)) {
//!             size.x = width / 2;
//!         }
//!     }
//!
//!     size.y = lround(float(vRegion.vEnd - vRegion.vStart) / (2.0f * yScaleFloat() * (float(size.x) / float(width))));
//!
//!     if ((size.x > 0) && (size.y > 0)) {
//!         const uint32_t ExtraRows = 2;
//!         const uint32_t Divisor = 4;
//!         size.y += ExtraRows;
//!         size.y = lround(float(size.y) / Divisor) * Divisor;
//!         return size;
//!     } else {
//!         return hlslpp::uint2(0, 0);
//!     }
//! }
//!
//! float VI::xScaleFloat() const {
//!     return (1024.0f / xTransform.xScale);
//! }
//! ```
//!
//! ## Reuse, not new type
//!
//! No new vector type. `fbSize`'s `hlslpp::uint2` is returned as a plain
//! `(u32, u32)` tuple, per `AGENTS.md`'s one-vector-type-per-port rule --
//! there is no arithmetic on the pair, only two independent components. The
//! register decode is not redefined either: this module takes already-decoded
//! `u32` field values, because
//! `crates/fn64-render-wgpu/src/rt64_vi_registers.rs` owns the extents and
//! `fn64_render::ViScanoutRegisters` owns the word image.
//!
//! ## Overlap with fn64's own types
//!
//! fn64 owns substantial VI ground already. Each overlapping fact is reported
//! as a **comparison**; nothing here is wired into any pipeline and no caller
//! is rewired.
//!
//! | fact | RT64 | fn64 owner | verdict |
//! |---|---|---|---|
//! | pixel type -> size | `fbSiz` `:66-76` maps type 2/3 to `G_IM_SIZ_16b/32b` | `ViPixelType` -> 2/4 bytes, `crates/fn64-render/src/vi_source.rs:68-72` | **agree** (`G_IM_SIZ_16b = 2` encodes 2 bytes, `32b = 3` encodes 4; RT64's own `1U << (siz - 1)` at `:84` is the same map) |
//! | STATUS bit extents | `rt64_vi.h` unions | `ViFilterControl::from_status`, `crates/fn64-render/src/lib.rs:292-306` | already adjudicated as agreeing in `rt64_vi_registers.rs`; not re-derived |
//! | scale field encoding | `xScale : 12`, divided into 1024.0f | `ViScaleAxis`, 10 fraction bits, `crates/fn64-render/src/lib.rs:322-341` | same U2.10 field, **different composition** -- see the disagreement below |
//! | active-window predicate | `visible()` `:44-46` | `ViActiveWindow::try_from_registers`, `crates/fn64-render/src/lib.rs:374-386` | **deliberate difference**, see below |
//! | gamma | `1.0f / 2.2f` exponent `:26-29` | integer square-root curve, `crates/fn64-render-reference/src/vi.rs:570-572` | **different mechanism, not a disagreement** -- RT64 hands a shader an exponent; fn64 reproduces the silicon curve from public documentation. Neither is an approximation of the other. |
//! | origin back-off | `fbAddress` `:78-93` subtracts one or two rows | `ViScanoutRegisters::origin()` returns the raw word, `crates/fn64-render/src/lib.rs:475-477` | **RT64-only heuristic.** Its own comment (`:81`) calls it an estimate. fn64 has no counterpart and this module does not propose one. |
//! | logical rate | `logicalRateFromFactors` `:164-172` | `fn64-render-rt64` FFI, already region-parameterized | **already owned and already corrected**; refused above |
//!
//! ## Admitted domain
//!
//! Every function here admits the full range of its decoded register fields:
//! 12-bit scale/offset (`0..=4095`), 10-bit region bounds (`0..=1023`),
//! `u32` width and origin, and any `u32` STATUS word. Zero scale is admitted
//! and returns infinity, matching the C++ (see `Open questions`). Region
//! bounds are admitted **reversed**, because the C++ admits them; the
//! resulting wrap is pinned as an observed C++ property, not reproduced as a
//! recommendation.
//!
//! ## Scope status
//!
//! DONE. `VIHistory`, the two rectangle constants, `compatibleWith` and
//! `operator!=` are deliberately not ported -- a scope boundary this card
//! chose for the reasons itemized above, not work this module is waiting on.
//!
//! ## Nonclaims
//!
//! - Unwired: declared `mod`, not `pub mod`; no production admission.
//! - No behavior change. fn64's `crates/fn64-render/src/vi_source.rs` and
//!   `crates/fn64-render/src/lib.rs` are untouched and remain authoritative;
//!   no caller is rewired.
//! - No `repr(C)`, size, alignment or ABI claim.
//! - No claim that either side of any reported disagreement is correct against
//!   hardware. Nothing here was tested against silicon.
//! - No field-declaration-order pin (the standing brief §3.7).
//! - **DEVIATION, labelled in the tests:** `fb_size_row_count_wraps_on_a_reversed_v_region`
//!   pins a C++ signed-to-unsigned wrap. Rust has no such implicit conversion,
//!   so [`fb_size`] reproduces it through an explicit `as u32` with the wrap
//!   named at the cast site. The test claims only what the C++ computes; it is
//!   not a recommendation, and fn64's own decoder asserts against reversed
//!   windows instead (`crates/fn64-render/src/lib.rs:394-401`).
//! - **DEVIATION, labelled in the tests:** `lround` is C's round-half-away-from-zero,
//!   which is *not* Rust's `f32::round_ties_even` and *is* Rust's `f32::round`.
//!   [`lround`] is written out rather than assumed, and
//!   `lround_is_half_away_from_zero_not_ties_even` pins the difference.
//!
//! ## Open questions
//!
//! Reported rather than silently guarded:
//!
//! - **Zero scale.** `xScaleFloat()` with `xTransform.xScale == 0` is
//!   `1024.0f / 0.0f`, i.e. `+inf` in IEEE-754. `fbSize` then divides by it
//!   and reaches `lround(0.0f)`. No guard exists in the C++ and none is added
//!   here; [`fb_size`] returns `(0, 0)` for that input, which is pinned. A
//!   `0.0f / 0.0f` NaN cannot be reached from `fbSize`'s expression because
//!   the numerator is an integer span.
//! - Whether RT64's `fbAddress` one-or-two-row back-off is correct against
//!   hardware. Its own comment says "Estimate".

/// C's `lround`: round half **away from zero**, not to even.
///
/// Rust's `f32::round` has this behavior and `f32::round_ties_even` does not.
/// Written out longhand because an algebraically-equal spelling is not the
/// same function at a tie, which is precisely where this file's two
/// implementations of the same quantity part company.
fn lround(value: f32) -> i64 {
    if value >= 0.0 {
        (value + 0.5).floor() as i64
    } else {
        -((-value + 0.5).floor() as i64)
    }
}

/// `G_IM_SIZ_*` -- `src/gbi/rt64_f3d.h`, reached through `rt64_vi.cpp:69-71`.
///
/// Cited by value rather than by digest: `rt64_f3d.h` is a dependency this
/// module does not port (the standing brief §4). The two values used are
/// pinned against RT64's own consistency check at `rt64_vi.cpp:84`, where
/// `1U << (siz - 1)` must yield the byte width.
pub(crate) const G_IM_SIZ_16B: u8 = 2;
pub(crate) const G_IM_SIZ_32B: u8 = 3;

/// `VI::fbSiz` -- `src/hle/rt64_vi.cpp:66-76`.
///
/// Maps the two-bit STATUS type onto a G_IM_SIZ code. Both `BLANK` and the
/// `default` arm return `0`, which is *not* a valid `G_IM_SIZ` value and is
/// used downstream only through the `siz >= G_IM_SIZ_16b` test at `:82`.
pub(crate) fn fb_siz(status_type: u32) -> u8 {
    match status_type {
        crate::rt64_vi_registers::VI_STATUS_TYPE_16_BIT => G_IM_SIZ_16B,
        crate::rt64_vi_registers::VI_STATUS_TYPE_32_BIT => G_IM_SIZ_32B,
        // `VI_STATUS_TYPE_BLANK` and `default` share one arm in the source.
        _ => 0,
    }
}

/// `VI::gamma` -- `src/hle/rt64_vi.cpp:26-29`.
///
/// `const float GammaCorrection = 1.0f / 2.2f;` -- an **f32 division**, whose
/// result is one ULP below `2.2f64.recip() as f32`. See
/// [`tests::gamma_constant_is_an_f32_division_not_an_f64_reciprocal`].
pub(crate) fn gamma(gamma_enable: bool) -> f32 {
    // Written as the source's f32 division rather than as a decimal literal,
    // so the ULP the division produces cannot be lost to a transcription.
    let gamma_correction: f32 = 1.0f32 / 2.2f32;
    if gamma_enable {
        gamma_correction
    } else {
        1.0f32
    }
}

/// `VI::visible` -- `src/hle/rt64_vi.cpp:44-46`.
///
/// `(status.type != VI_STATUS_TYPE_BLANK) && (hRegion.hStart > 0)`.
///
/// This is **not** the same predicate as
/// `fn64_render::ViActiveWindow::try_from_registers`, and the difference is
/// deliberate on both sides -- see the module's `Overlap` table and
/// [`tests::visible_and_fn64_active_window_answer_different_questions`].
pub(crate) fn visible(status_type: u32, h_start: u32) -> bool {
    (status_type != crate::rt64_vi_registers::VI_STATUS_TYPE_BLANK) && (h_start > 0)
}

/// `VI::xScaleFloat` / `VI::yScaleFloat` -- `src/hle/rt64_vi.cpp:127-137`.
///
/// **A reciprocal, rounded to f32.** `1024.0f / scale` is the host-pixels-per-
/// source-pixel step's inverse; the caller then *divides* by it, so the field
/// is round-tripped through an f32 reciprocal instead of being multiplied
/// directly. That double rounding is the module's headline disagreement with
/// fn64 -- see [`tests::the_f32_reciprocal_disagrees_with_the_direct_step_only_at_exact_ties`].
pub(crate) fn scale_float(scale_u2_10: u32) -> f32 {
    1024.0f32 / (scale_u2_10 as f32)
}

/// `VI::xOffsetFloat` / `VI::yOffsetFloat` -- `src/hle/rt64_vi.cpp:131-141`.
///
/// A division by a power of two, so unlike [`scale_float`] this one is exact
/// for every 12-bit input -- pinned in
/// [`tests::offset_float_is_exact_for_every_twelve_bit_field`].
pub(crate) fn offset_float(offset_u2_10: u32) -> f32 {
    (offset_u2_10 as f32) / 1024.0f32
}

/// `VI::fbAddress` -- `src/hle/rt64_vi.cpp:78-93`.
///
/// Backs the origin off by one row, or two when interlacing is stepping an odd
/// field. RT64's own comment calls this an estimate. fn64 has no counterpart;
/// `ViScanoutRegisters::origin()` returns the raw register word.
pub(crate) fn fb_address(
    status_type: u32,
    serrate: bool,
    v_current_line: u32,
    width: u32,
    origin: u32,
) -> u32 {
    let siz = fb_siz(status_type);
    if siz >= G_IM_SIZ_16B {
        let interlaced_step = serrate && (v_current_line & 0x1) != 0;
        // `1U << (siz - 1)` is RT64's own byte-width derivation at `:84`; it
        // is the second, independent derivation of the byte widths that
        // `fb_siz` encodes as G_IM_SIZ codes.
        let row_bytes = width.wrapping_mul(1u32 << (siz - 1));
        let row_count: u32 = if interlaced_step { 2 } else { 1 };
        let row_offset = row_bytes.wrapping_mul(row_count);
        if origin >= row_offset {
            return origin - row_offset;
        }
    }
    origin
}

/// `VI::fbSize` -- `src/hle/rt64_vi.cpp:95-125`.
///
/// Returns `hlslpp::uint2` as a plain `(width, height)` tuple; see the
/// module's `Reuse, not new type`.
///
/// `h_span` is `hEnd - hStart` and `v_span` is `vEnd - vStart`. Both are
/// **`i32`**, not `u32`, because the C++ subtracts two `unsigned : 10`
/// bitfields, which integer-promote to `int`; a reversed region therefore
/// yields a negative span in the C++ too. See the module's DEVIATION note.
pub(crate) fn fb_size(
    serrate: bool,
    h_span: i32,
    v_span: i32,
    width: u32,
    x_scale: u32,
    y_scale: u32,
) -> (u32, u32) {
    let mut size_x = width;

    // In interlaced without deflickering, the stride is usually double the
    // real row size (`:98-99`).
    if serrate {
        let estimated_width = (h_span as f32) / scale_float(x_scale);
        let interlaced_tolerance = 1.875f32;
        if estimated_width < ((width as f32) / interlaced_tolerance) {
            size_x = width / 2;
        }
    }

    let ratio = (size_x as f32) / (width as f32);
    let quotient = (v_span as f32) / (2.0f32 * scale_float(y_scale) * ratio);
    // DEVIATION (labelled): the C++ assigns a `long` into `unsigned size.y`,
    // so a negative `lround` wraps modulo 2^32. Rust has no implicit
    // conversion, so the wrap is written out here and pinned by
    // `fb_size_row_count_wraps_on_a_reversed_v_region`.
    let mut size_y = lround(quotient) as u32;

    if (size_x > 0) && (size_y > 0) {
        const EXTRA_ROWS: u32 = 2;
        const DIVISOR: u32 = 4;
        size_y = size_y.wrapping_add(EXTRA_ROWS);
        size_y = (lround((size_y as f32) / (DIVISOR as f32)) as u32).wrapping_mul(DIVISOR);
        (size_x, size_y)
    } else {
        (0, 0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rt64_vi_registers::{
        VI_STATUS_TYPE_16_BIT, VI_STATUS_TYPE_32_BIT, VI_STATUS_TYPE_BLANK, VI_STATUS_TYPE_RESERVED,
    };

    /// The exact-rational form of `fbSize`'s row count, with C's
    /// round-half-away-from-zero. This is the *direct step* composition fn64
    /// uses (`crates/fn64-render/src/vi_source.rs:86-90` keeps the raw U2.10
    /// field and multiplies), evaluated without any intermediate rounding.
    ///
    /// `span / (2 * (1024 / step))` == `span * step / 2048`.
    fn exact_row_count(v_span: i64, y_scale: i64) -> i64 {
        let numerator = v_span * y_scale;
        // Round half away from zero over the exact rational `numerator/2048`.
        if numerator >= 0 {
            (2 * numerator + 2048) / 4096
        } else {
            -((-2 * numerator + 2048) / 4096)
        }
    }

    /// RT64's composition, isolated from the rest of `fbSize`: the reciprocal
    /// is rounded to f32 first, then divided into.
    fn rt64_row_count(v_span: i32, y_scale: u32) -> i64 {
        lround((v_span as f32) / (2.0f32 * scale_float(y_scale) * 1.0f32))
    }

    #[test]
    fn lround_is_half_away_from_zero_not_ties_even() {
        // DEVIATION guard: C's `lround` is not `round_ties_even`. If this
        // module's `lround` were quietly replaced by the ties-even spelling,
        // every tie in the reciprocal analysis below would move.
        assert_eq!(lround(0.5), 1);
        assert_eq!(lround(1.5), 2);
        assert_eq!(lround(2.5), 3);
        assert_eq!(lround(-0.5), -1);
        assert_eq!(lround(-1.5), -2);
        // Derived a second way: agrees with Rust's `f32::round` everywhere,
        // and differs from `round_ties_even` at exactly the even-tie cases.
        for raw in [
            0.5f32, 1.5, 2.5, 3.5, -0.5, -1.5, -2.5, 0.4999999, 0.5000001,
        ] {
            assert_eq!(
                lround(raw),
                raw.round() as i64,
                "lround({raw}) vs f32::round"
            );
        }
        assert_ne!(lround(2.5), 2.5f32.round_ties_even() as i64);
        assert_ne!(lround(0.5), 0.5f32.round_ties_even() as i64);
    }

    #[test]
    fn gamma_constant_is_an_f32_division_not_an_f64_reciprocal() {
        // `rt64_vi.cpp:27` is `const float GammaCorrection = 1.0f / 2.2f;`.
        let ported = gamma(true);
        // Asserted two independent ways (the standing brief §3.2): as the bit
        // pattern, and as the division re-spelled.
        assert_eq!(ported.to_bits(), 0x3ee8_ba2e);
        assert_eq!(ported, 1.0f32 / 2.2f32);

        // The §3.3 hazard, and a *new* finding for this file: computing the
        // same constant in f64 and rounding down lands one ULP HIGHER. An
        // executor who wrote `(1.0f64 / 2.2f64) as f32`, or who transcribed a
        // f64-printed decimal, would have shipped the wrong constant.
        let via_f64 = (1.0f64 / 2.2f64) as f32;
        assert_eq!(via_f64.to_bits(), 0x3ee8_ba2f);
        assert_ne!(ported, via_f64);
        assert_eq!(
            via_f64.to_bits() - ported.to_bits(),
            1,
            "exactly one ULP apart"
        );

        // The disabled arm is exactly 1.0, not the correction.
        assert_eq!(gamma(false), 1.0f32);
        assert_eq!(gamma(false).to_bits(), 1.0f32.to_bits());
    }

    #[test]
    fn fb_siz_maps_only_the_two_real_pixel_types() {
        assert_eq!(fb_siz(VI_STATUS_TYPE_16_BIT), G_IM_SIZ_16B);
        assert_eq!(fb_siz(VI_STATUS_TYPE_32_BIT), G_IM_SIZ_32B);
        // BLANK and the `default` arm share one branch in the source.
        assert_eq!(fb_siz(VI_STATUS_TYPE_BLANK), 0);
        assert_eq!(fb_siz(VI_STATUS_TYPE_RESERVED), 0);
        // The two-bit field cannot exceed 3, but `default` covers it anyway.
        assert_eq!(fb_siz(4), 0);
        assert_eq!(fb_siz(u32::MAX), 0);

        // Derived a second way (the standing brief §3.2): RT64's own byte
        // width at `:84` is `1U << (siz - 1)`. Reconciled against fn64's
        // independent map in `crates/fn64-render/src/vi_source.rs:68-72`,
        // which assigns 2 bytes to Rgba16 and 4 to Rgba32.
        assert_eq!(1u32 << (G_IM_SIZ_16B - 1), 2);
        assert_eq!(1u32 << (G_IM_SIZ_32B - 1), 4);
        // And the codes themselves are ordered so `siz >= G_IM_SIZ_16b`
        // (`:82`) admits exactly the two real types.
        assert!(G_IM_SIZ_16B >= G_IM_SIZ_16B && G_IM_SIZ_32B >= G_IM_SIZ_16B);
        assert!(0 < G_IM_SIZ_16B);
    }

    #[test]
    fn visible_and_fn64_active_window_answer_different_questions() {
        // RT64: `(type != BLANK) && (hStart > 0)` -- `rt64_vi.cpp:44-46`.
        assert!(visible(VI_STATUS_TYPE_16_BIT, 108));
        assert!(visible(VI_STATUS_TYPE_32_BIT, 1));
        assert!(!visible(VI_STATUS_TYPE_BLANK, 108));
        assert!(!visible(VI_STATUS_TYPE_16_BIT, 0));
        // RESERVED is not BLANK, so RT64 calls it visible.
        assert!(visible(VI_STATUS_TYPE_RESERVED, 108));

        // The pinned difference. `H_START = 0x2d0` is hStart 0, hEnd 720:
        // RT64 says not-visible, fn64 says programmed.
        let h_register: u32 = 0x0000_02d0;
        let h_start = (h_register >> 16) & 0x03ff;
        let h_end = h_register & 0x03ff;
        assert_eq!((h_start, h_end), (0, 720));
        assert!(!visible(VI_STATUS_TYPE_16_BIT, h_start));

        // fn64's predicate, re-spelled from
        // `crates/fn64-render/src/lib.rs:379-385`: `Some` when *either*
        // 10-bit subfield of each register is nonzero.
        let used = 0x03ffu32 | (0x03ffu32 << 16);
        let fn64_programmed = (h_register & used) != 0;
        assert!(fn64_programmed);

        // This is DELIBERATE ON BOTH SIDES, not a defect in either. fn64's own
        // doc comment (`crates/fn64-render/src/lib.rs:374-378`) states the
        // reason: register initialization is not atomic, so software may fill
        // V_START while H_START is still zero. RT64 asks "should I present
        // this field?"; fn64 asks "has this interval been programmed at all?".
        // A future card must not harmonize them.
        //
        // Where the two questions coincide, they agree:
        for h_start in [1u32, 108, 1023] {
            let register = (h_start << 16) | 720;
            assert_eq!(
                visible(VI_STATUS_TYPE_16_BIT, h_start),
                (register & used) != 0
            );
        }
    }

    #[test]
    fn offset_float_is_exact_for_every_twelve_bit_field() {
        // Unlike `scale_float`, this divisor is a power of two, so no input in
        // the field's domain loses a bit. Exhaustive over the 12-bit field.
        for raw in 0u32..4096 {
            let ported = offset_float(raw);
            // Second, independent derivation: the exact rational in f64.
            assert_eq!(
                f64::from(ported),
                f64::from(raw) / 1024.0f64,
                "offset_float({raw}) is not exact"
            );
        }
        // Spot values, pinned as literals.
        assert_eq!(offset_float(0), 0.0);
        assert_eq!(offset_float(1024), 1.0);
        assert_eq!(offset_float(512), 0.5);
        assert_eq!(offset_float(4095), 4095.0 / 1024.0);
    }

    #[test]
    fn scale_float_is_a_reciprocal_exact_only_at_powers_of_two() {
        // `1024.0f / scale`. Exact exactly when `scale` divides 1024 into a
        // representable f32, i.e. at the powers of two in the 12-bit domain.
        let mut exact = Vec::new();
        for raw in 1u32..4096 {
            let ported = scale_float(raw);
            if f64::from(ported) * f64::from(raw) == 1024.0f64 {
                exact.push(raw);
            }
        }
        // Derived three independent ways (the standing brief §3.2): as an
        // enumerated literal, as the shift sequence, and as a predicate.
        assert_eq!(
            exact,
            vec![1u32, 2, 4, 8, 16, 32, 64, 128, 256, 512, 1024, 2048]
        );
        assert_eq!(
            exact,
            (0u32..12).map(|shift| 1u32 << shift).collect::<Vec<_>>()
        );
        assert_eq!(
            exact,
            (1u32..4096)
                .filter(|value| value.is_power_of_two())
                .collect::<Vec<_>>()
        );
        assert_eq!(exact.len(), 12);

        // The identity step: U2.10 `ONE` is 0x400, and 1024/1024 == 1.0.
        assert_eq!(scale_float(1024), 1.0f32);
        // Half-rate source stepping.
        assert_eq!(scale_float(512), 2.0f32);
        assert_eq!(scale_float(2048), 0.5f32);

        // Open question, pinned rather than guarded: zero scale is admitted by
        // the C++ and yields infinity.
        assert!(scale_float(0).is_infinite());
        assert!(scale_float(0).is_sign_positive());
    }

    #[test]
    fn the_f32_reciprocal_disagrees_with_the_direct_step_only_at_exact_ties() {
        // The headline finding, re-derived here rather than quoted.
        //
        // RT64 (`rt64_vi.cpp:110` with `:135-137`) computes the row count as
        // `lround(span / (2 * (1024/step)))`, rounding the reciprocal to f32
        // FIRST. fn64 (`crates/fn64-render/src/vi_source.rs:86-90`) keeps the
        // raw U2.10 step and multiplies, i.e. `span * step / 2048`.
        // Algebraically identical; numerically not.
        let mut disagreements = 0u32;
        let mut tie_disagreements = 0u32;
        let mut non_tie_disagreements = 0u32;
        let mut rt64_rounded_down = 0u32;
        let mut rt64_rounded_up = 0u32;
        let mut realistic = 0u32;
        let mut total_ties = 0u32;
        for step in 1u32..=4096 {
            for span in (2i32..=1024).step_by(2) {
                let rt64 = rt64_row_count(span, step);
                let direct = exact_row_count(i64::from(span), i64::from(step));
                let is_tie = (i64::from(span) * i64::from(step)) % 2048 == 1024;
                if is_tie {
                    total_ties += 1;
                }
                if rt64 == direct {
                    continue;
                }
                disagreements += 1;
                if is_tie {
                    tie_disagreements += 1;
                } else {
                    non_tie_disagreements += 1;
                }
                if rt64 < direct {
                    rt64_rounded_down += 1;
                } else {
                    rt64_rounded_up += 1;
                }
                if (0x100..=0x800).contains(&step) && (400..=540).contains(&span) {
                    realistic += 1;
                }
            }
        }

        // The measured count over `step` in `1..=4096` x even `span` in
        // `2..=1024`. A prior card reported 1,036 for this sweep; this
        // re-derivation measures 1,247, and the mechanism below explains why
        // the figure is sharp rather than approximate.
        assert_eq!(disagreements, 1_247);
        assert_eq!(realistic, 102);

        // THE MECHANISM, which is stronger than a raw count: EVERY
        // disagreement is an exact half-integer tie, and RT64 always rounds
        // DOWN. The f32 reciprocal lands a hair below the true value, so a
        // quantity that is exactly `k + 0.5` becomes `k + 0.4999...`.
        assert_eq!(
            non_tie_disagreements, 0,
            "a non-tie disagreement would be a different defect"
        );
        assert_eq!(tie_disagreements, disagreements);
        assert_eq!(rt64_rounded_up, 0);
        assert_eq!(rt64_rounded_down, disagreements);

        // And the disagreements are a strict minority of the ties: most ties
        // survive the reciprocal intact.
        assert_eq!(total_ties, 11_264);
        assert!(tie_disagreements < total_ties);

        // The witness, confirmed: step 0x1e0, span 480 -> 112 rows vs 113.
        assert_eq!(rt64_row_count(480, 0x1e0), 112);
        assert_eq!(exact_row_count(480, 0x1e0), 113);
        // Derived a second way: the exact product IS a half-integer, and the
        // f32 reciprocal path lands strictly below it.
        assert_eq!(480i64 * 0x1e0, 230_400);
        assert_eq!(230_400f64 / 2048.0, 112.5);
        let quotient = 480.0f32 / (2.0f32 * scale_float(0x1e0));
        assert!(
            f64::from(quotient) < 112.5,
            "{quotient} should undershoot the tie"
        );
        assert_eq!(quotient.to_bits(), 112.499_99f32.to_bits());
    }

    #[test]
    fn every_power_of_two_step_agrees_which_is_why_this_hides() {
        // The brief's claim, confirmed AND sharpened. Powers of two are not
        // tie-free -- there are 512 exact ties among them -- they are
        // disagreement-free, because `1024/2^k` is exact in f32 so no
        // undershoot exists to break the tie downward.
        let mut pow2_ties = 0u32;
        for shift in 0u32..13 {
            let step = 1u32 << shift;
            for span in (2i32..=1024).step_by(2) {
                if (i64::from(span) * i64::from(step)) % 2048 == 1024 {
                    pow2_ties += 1;
                }
                assert_eq!(
                    rt64_row_count(span, step),
                    exact_row_count(i64::from(span), i64::from(step)),
                    "power-of-two step {step:#x} disagreed at span {span}"
                );
            }
        }
        assert_eq!(
            pow2_ties, 512,
            "powers of two do hit ties; they just survive them"
        );

        // The converse, stated as the actual predicate: a disagreement needs
        // an INEXACT reciprocal. Every one of the 1,247 has one.
        let mut checked = 0u32;
        for step in 1u32..=4096 {
            let reciprocal_exact = f64::from(scale_float(step)) * f64::from(step) == 1024.0;
            if !reciprocal_exact {
                continue;
            }
            for span in (2i32..=1024).step_by(2) {
                assert_eq!(
                    rt64_row_count(span, step),
                    exact_row_count(i64::from(span), i64::from(step)),
                    "step {step:#x} has an exact reciprocal but disagreed"
                );
                checked += 1;
            }
        }
        // 12 exact-reciprocal steps within `1..=4096` inclusive, plus 4096
        // itself, over 512 even spans.
        assert_eq!(checked, 13 * 512);
    }

    #[test]
    fn fb_address_backs_the_origin_off_by_one_or_two_rows() {
        // 16-bit, progressive: one row of `width * 2` bytes.
        assert_eq!(
            fb_address(VI_STATUS_TYPE_16_BIT, false, 0, 320, 0x10_0000),
            0x10_0000 - 640
        );
        // 32-bit, progressive: one row of `width * 4` bytes.
        assert_eq!(
            fb_address(VI_STATUS_TYPE_32_BIT, false, 0, 320, 0x10_0000),
            0x10_0000 - 1280
        );
        // Interlaced on an ODD current line: two rows.
        assert_eq!(
            fb_address(VI_STATUS_TYPE_16_BIT, true, 1, 320, 0x10_0000),
            0x10_0000 - 1280
        );
        // Interlaced on an EVEN current line: still one row -- the `& 0x1`
        // at `:83` gates on the line parity, not on `serrate` alone.
        assert_eq!(
            fb_address(VI_STATUS_TYPE_16_BIT, true, 2, 320, 0x10_0000),
            0x10_0000 - 640
        );
        // Serrate off, odd line: one row.
        assert_eq!(
            fb_address(VI_STATUS_TYPE_16_BIT, false, 1, 320, 0x10_0000),
            0x10_0000 - 640
        );

        // BLANK and RESERVED skip the whole back-off (`siz >= G_IM_SIZ_16b`
        // is false at `siz == 0`) and return the raw origin.
        assert_eq!(
            fb_address(VI_STATUS_TYPE_BLANK, true, 1, 320, 0x10_0000),
            0x10_0000
        );
        assert_eq!(
            fb_address(VI_STATUS_TYPE_RESERVED, true, 1, 320, 0x10_0000),
            0x10_0000
        );

        // The underflow guard at `:87`: when the origin is smaller than one
        // row, the raw origin is returned rather than wrapping.
        assert_eq!(fb_address(VI_STATUS_TYPE_16_BIT, false, 0, 320, 639), 639);
        // Exactly one row is NOT underflow -- `>=`, not `>`.
        assert_eq!(fb_address(VI_STATUS_TYPE_16_BIT, false, 0, 320, 640), 0);
        assert_eq!(fb_address(VI_STATUS_TYPE_16_BIT, false, 0, 320, 641), 1);

        // fn64 has no counterpart: `ViScanoutRegisters::origin()`
        // (`crates/fn64-render/src/lib.rs:475-477`) returns the raw word. This
        // is reported as an RT64-only heuristic, not proposed for fn64.
    }

    #[test]
    fn fb_size_halves_the_width_only_for_interlaced_double_stride() {
        let one = 1024u32; // U2.10 identity step.

        // Progressive: the serrate branch is skipped entirely, so the width
        // passes through even when the estimate would have tripped it.
        let (w, _) = fb_size(false, 320, 480, 640, one, one);
        assert_eq!(w, 640);

        // Interlaced with a stride that is genuinely double: an h_span of 320
        // at identity scale estimates 320 source pixels against a 640 stride,
        // and 320 < 640/1.875 == 341.33, so the width halves.
        //
        // The HEIGHT is asserted here too, and deliberately so. `rt64_vi.cpp:110`
        // scales the row count by `float(size.x) / float(width)` -- the halved
        // width over the original -- and that ratio is the ONLY place the two
        // widths are ever divided. Everywhere else `size.x == width` makes the
        // ratio 1.0, where an inverted `width / size.x` is indistinguishable.
        // Checking only `size.x` on this path leaves the ratio's orientation
        // untested; halving the width doubles the row count, so the ratio must
        // be the halved-over-original one.
        let (w, h) = fb_size(true, 320, 480, 640, one, one);
        assert_eq!(w, 320);
        assert_eq!(h, 484, "halving the width must DOUBLE the row count");
        // Derived a second way: ratio 0.5 halves the divisor, so the raw count
        // is 480 rather than 240; +2 is 482; snapped to a multiple of 4 is 484.
        assert_eq!(lround(482.0 / 4.0) * 4, 484);
        // And the inverted ratio would give a quarter of that, which it does
        // not: 484 != 124.
        assert_ne!(h, 124);

        // Interlaced but NOT double-stride: an h_span of 640 estimates 640,
        // which is not below 341.33, so the width is kept.
        let (w, _) = fb_size(true, 640, 480, 640, one, one);
        assert_eq!(w, 640);

        // The tolerance boundary, derived twice. `640 / 1.875f == 341.333`,
        // and the comparison is strict `<`.
        assert_eq!(640.0f32 / 1.875f32, 341.33334f32);
        let (kept, _) = fb_size(true, 342, 480, 640, one, one);
        assert_eq!(kept, 640, "342 is above the tolerance and must not halve");
        let (halved, _) = fb_size(true, 341, 480, 640, one, one);
        assert_eq!(halved, 320, "341 is below the tolerance and must halve");
        // 1.875 is exactly representable, so the boundary is not itself a
        // rounding hazard (contrast `scale_float`).
        assert_eq!(f64::from(1.875f32), 1.875f64);
    }

    #[test]
    fn fb_size_tolerance_comparison_is_strict_at_an_exactly_representable_boundary() {
        // A mutation-reach test. `rt64_vi.cpp:103` is `estimatedWidth <
        // (width / interlacedTolerance)` -- a STRICT `<`. Relaxing it to `<=`
        // survives every inequality-flavored spot check, because the two
        // sides are almost never exactly equal in f32.
        //
        // They can be, though, and the boundary is genuinely reachable inside
        // the register domain: `width = 480`, `xScale = 0x200` (a half-rate
        // step, so `xScaleFloat() == 2.0` exactly) and `hEnd - hStart = 512`
        // make the estimate exactly `256.0`, while `480 / 1.875f` is also
        // exactly `256.0`. Both quantities are exact because 1.875 and the
        // power-of-two reciprocal are exact -- the very property that makes
        // the reciprocal disagreement hide is what makes this tie exist.
        let width = 480u32;
        let x_scale = 0x200u32;
        let h_span = 512i32;

        assert_eq!(scale_float(x_scale), 2.0f32);
        let estimated_width = (h_span as f32) / scale_float(x_scale);
        let threshold = (width as f32) / 1.875f32;
        assert_eq!(estimated_width, 256.0f32);
        assert_eq!(threshold, 256.0f32);
        // Derived a second way, as bit patterns, so an ULP cannot hide here.
        assert_eq!(estimated_width.to_bits(), threshold.to_bits());

        // Strict `<` is false at equality, so the width is KEPT. A `<=`
        // mutant would halve it to 240 and this assertion fails.
        assert!(!(estimated_width < threshold));
        let (kept, _) = fb_size(true, h_span, 480, width, x_scale, 1024);
        assert_eq!(kept, width, "an exact tie must not trip the halving");

        // One representable step below the boundary does halve, which proves
        // the assertion above is testing the comparison and not a dead branch.
        let (halved, _) = fb_size(true, h_span - 1, 480, width, x_scale, 1024);
        assert_eq!(halved, width / 2);
    }

    #[test]
    fn fb_size_adds_two_rows_and_snaps_to_a_multiple_of_four() {
        let one = 1024u32;

        // 480 half-lines at identity scale is 240 rows; +2 is 242; snapped to
        // the nearest multiple of 4 is 244.
        let (w, h) = fb_size(false, 640, 480, 640, one, one);
        assert_eq!((w, h), (640, 244));
        // Derived a second way: `lround(242/4) * 4 == 61 * 4 == 244`.
        assert_eq!(lround(242.0 / 4.0) * 4, 244);
        assert_eq!(h % 4, 0);

        // The snap rounds, it does not truncate: 240 rows +2 = 242 -> 244
        // (up), while 238 rows +2 = 240 -> 240 (already a multiple).
        let (_, h) = fb_size(false, 640, 476, 640, one, one);
        assert_eq!(h, 240);
        assert_eq!(lround(240.0 / 4.0) * 4, 240);

        // Every output is a multiple of four across a realistic sweep, which
        // is the invariant the Divisor step exists to create.
        for v_span in (400i32..=540).step_by(2) {
            let (_, height) = fb_size(false, 640, v_span, 640, one, one);
            assert_eq!(height % 4, 0, "v_span {v_span} produced {height}");
        }
    }

    #[test]
    fn fb_size_returns_zero_when_either_component_is_zero() {
        let one = 1024u32;
        // Zero width short-circuits before the ExtraRows/Divisor step.
        assert_eq!(fb_size(false, 640, 480, 0, one, one), (0, 0));
        // Zero v_span makes `size.y` zero, which also short-circuits -- note
        // this means the +2 is NOT applied, so the answer is (0,0) and not
        // (width, 4).
        assert_eq!(fb_size(false, 640, 0, 640, one, one), (0, 0));

        // Open question, pinned: zero Y scale divides by infinity, reaching
        // `lround(0.0)` and the same zero short-circuit. No guard exists in
        // the C++ and none is added.
        assert!(scale_float(0).is_infinite());
        assert_eq!(fb_size(false, 640, 480, 640, one, 0), (0, 0));
        // Zero X scale on a progressive image never reaches the estimate.
        assert_eq!(fb_size(false, 640, 480, 640, 0, one), (640, 244));
    }

    #[test]
    fn fb_size_row_count_wraps_on_a_reversed_v_region() {
        // DEVIATION (labelled): this pins a C++ signed-to-unsigned wrap, not a
        // recommendation. `vEnd - vStart` promotes to `int`, so a reversed
        // region is negative; `lround` returns a negative `long`; assigning it
        // into `unsigned size.y` wraps modulo 2^32.
        let one = 1024u32;

        // -50 half-lines at identity scale is `lround(-25.0) == -25`... but
        // the C++ divides by `2 * yScaleFloat`, giving -25 rows, which wraps.
        let wrapped = (-25i64) as u32;
        assert_eq!(wrapped, 4_294_967_271);
        // The wrapped value is > 0, so it passes the guard at `:112` and is
        // carried into the ExtraRows/Divisor step -- where the *second* wrap
        // collapses it back to zero.
        let (w, h) = fb_size(false, 640, -50, 640, one, one);
        assert_eq!(w, 640);
        assert_eq!(h, 0, "the second wrap lands exactly on zero");
        // Derived a second way, following the C++ step by step.
        let after_extra = wrapped.wrapping_add(2);
        let snapped = (lround((after_extra as f32) / 4.0) as u32).wrapping_mul(4);
        assert_eq!(snapped, 0);
        assert_eq!(snapped, h);

        // fn64 does NOT admit this input at all: `ViActiveWindow::from_registers`
        // asserts `vertical_end_half_line > vertical_start_half_line`
        // (`crates/fn64-render/src/lib.rs:398-401`). RT64's accidental
        // zero here is benign by overflow, not by design, and this module
        // reports it rather than proposing it.
    }
}
