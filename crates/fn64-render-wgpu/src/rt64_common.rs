//! Literal port of RT64's `rt64_common` fixed-point rect / fixed-point
//! matrix / Halton-sequence helpers, a literal port of the permitted MIT
//! RT64 Rust-port source pinned at commit
//! `5473732a822a4423b5696e7cb18fecc425a59875`
//! (`docs/RT64-PORT-AUTHORITY.md`), `src/common/rt64_common.h`/`.cpp`
//! (SHA-256 of the whole files,
//! `fd20ae43ea5cad0bcb3510a3fc6419b255455f12fa4cdbfc8d91f868925739b2` /
//! `b8d6d767eedd4b85cb1f0bd33f68feb27b4bd392c09dc109b076528d1bee9315`):
//!
//! Only the `FixedRect`, `FixedMatrix`, and `HaltonSequence`/`HaltonJitter`
//! declarations from `rt64_common.h` are ported (`UpscaleMode`, `RectI`,
//! `adjustVector`, the log macros, and `GlobalLastError`/`GlobalLogFile`
//! globals are not -- see "Nonclaims").
//!
//! ```text
//! // rt64_common.h
//! inline float HaltonSequence(int i, int b) {
//!     float f = 1.0;
//!     float r = 0.0;
//!     while (i > 0) {
//!         f = f / float(b);
//!         r = r + f * float(i % b);
//!         i = i / b;
//!     }
//!
//!     return r;
//! }
//!
//! inline hlslpp::float2 HaltonJitter(int frame, int phases) {
//!     return { HaltonSequence(frame % phases + 1, 2) - 0.5f, HaltonSequence(frame % phases + 1, 3) - 0.5f };
//! }
//!
//! struct FixedRect {
//!     int32_t ulx;
//!     int32_t uly;
//!     int32_t lrx;
//!     int32_t lry;
//!
//!     FixedRect();
//!     FixedRect(int32_t ulx, int32_t uly, int32_t lrx, int32_t lry);
//!     void reset();
//!     bool isEmpty() const;
//!     bool isNull() const;
//!     void merge(const FixedRect &rect);
//!     FixedRect scaled(float x, float y) const;
//!
//!     // Intersections can result in invalid rects if they don't overlap. Check if they're not null after using this.
//!     FixedRect intersection(const FixedRect &rect) const;
//!     bool contains(int32_t x, int32_t y) const;
//!     bool fullyInside(const FixedRect &rect) const;
//!     int32_t left(bool ceil) const;
//!     int32_t top(bool ceil) const;
//!     int32_t right(bool ceil) const;
//!     int32_t bottom(bool ceil) const;
//!     int32_t width(bool leftCeil, bool rightCeil) const;
//!     int32_t height(bool topCeil, bool bottomCeil) const;
//! };
//!
//! struct FixedMatrix {
//!     int16_t integer[4][4];
//!     uint16_t frac[4][4];
//!
//!     float toFloat(uint32_t i, uint32_t j) const;
//!     hlslpp::float4x4 toMatrix4x4() const;
//!
//!     static float fixedToFloat(int16_t integerValue, uint16_t fracValue);
//!     static void modifyMatrix4x4Integer(hlslpp::float4x4 &matrix, uint32_t i, uint32_t j, int16_t integerValue);
//!     static void modifyMatrix4x4Fraction(hlslpp::float4x4 &matrix, uint32_t i, uint32_t j, uint16_t fracValue);
//! };
//!
//! // rt64_common.cpp
//! FixedRect::FixedRect() {
//!     reset();
//! }
//!
//! FixedRect::FixedRect(int32_t ulx, int32_t uly, int32_t lrx, int32_t lry) {
//!     this->ulx = ulx;
//!     this->uly = uly;
//!     this->lrx = lrx;
//!     this->lry = lry;
//! }
//!
//! void FixedRect::reset() {
//!     ulx = INT32_MAX;
//!     uly = INT32_MAX;
//!     lrx = INT32_MIN;
//!     lry = INT32_MIN;
//! }
//!
//! bool FixedRect::isEmpty() const {
//!     return isNull() || (lrx == ulx) || (lry == uly);
//! }
//!
//! bool FixedRect::isNull() const {
//!     return (ulx > lrx) || (uly > lry);
//! }
//!
//! void FixedRect::merge(const FixedRect &rect) {
//!     assert(!rect.isNull());
//!
//!     ulx = std::min(ulx, rect.ulx);
//!     uly = std::min(uly, rect.uly);
//!     lrx = std::max(lrx, rect.lrx);
//!     lry = std::max(lry, rect.lry);
//! }
//!
//! FixedRect FixedRect::scaled(float x, float y) const {
//!     assert(!isNull());
//!     assert(x >= 0.0f);
//!     assert(y >= 0.0f);
//!
//!     return FixedRect(
//!         int32_t(floorf(left(false) * x)) << 2,
//!         int32_t(floorf(top(false) * y)) << 2,
//!         int32_t(ceilf(right(true) * x)) << 2,
//!         int32_t(ceilf(bottom(true) * y)) << 2
//!     );
//! }
//!
//! FixedRect FixedRect::intersection(const FixedRect &rect) const {
//!     if (!isNull() && !rect.isNull()) {
//!         return {
//!             std::max(ulx, rect.ulx),
//!             std::max(uly, rect.uly),
//!             std::min(lrx, rect.lrx),
//!             std::min(lry, rect.lry)
//!         };
//!     }
//!     else {
//!         return FixedRect();
//!     }
//! }
//!
//! bool FixedRect::contains(int32_t x, int32_t y) const {
//!     if (!isNull()) {
//!         return (x >= ulx) && (x <= lrx) && (y >= uly) && (y <= lry);
//!     }
//!     else {
//!         return false;
//!     }
//! }
//!
//! bool FixedRect::fullyInside(const FixedRect &rect) const {
//!     assert(!isNull());
//!     assert(!rect.isNull());
//!     return (rect.ulx >= ulx) && (rect.uly >= uly) && (rect.lrx <= lrx) && (rect.lry <= lry);
//! }
//!
//! int32_t FixedRect::left(bool ceil) const {
//!     assert(!isNull());
//!     return (ulx + (ceil ? 3 : 0)) >> 2;
//! }
//!
//! int32_t FixedRect::top(bool ceil) const {
//!     assert(!isNull());
//!     return (uly + (ceil ? 3 : 0)) >> 2;
//! }
//!
//! int32_t FixedRect::right(bool ceil) const {
//!     assert(!isNull());
//!     return (lrx + (ceil ? 3 : 0)) >> 2;
//! }
//!
//! int32_t FixedRect::bottom(bool ceil) const {
//!     assert(!isNull());
//!     return (lry + (ceil ? 3 : 0)) >> 2;
//! }
//!
//! int32_t FixedRect::width(bool leftCeil, bool rightCeil) const {
//!     assert(!isNull());
//!     return right(rightCeil) - left(leftCeil);
//! }
//!
//! int32_t FixedRect::height(bool topCeil, bool bottomCeil) const {
//!     assert(!isNull());
//!     return bottom(bottomCeil) - top(topCeil);
//! }
//!
//! float FixedMatrix::toFloat(uint32_t i, uint32_t j) const {
//!     const int xorJ = j ^ 1;
//!     return FixedMatrix::fixedToFloat(integer[i][xorJ], frac[i][xorJ]);
//! }
//!
//! hlslpp::float4x4 FixedMatrix::toMatrix4x4() const {
//!     return hlslpp::float4x4(
//!         toFloat(0, 0), toFloat(0, 1), toFloat(0, 2), toFloat(0, 3),
//!         toFloat(1, 0), toFloat(1, 1), toFloat(1, 2), toFloat(1, 3),
//!         toFloat(2, 0), toFloat(2, 1), toFloat(2, 2), toFloat(2, 3),
//!         toFloat(3, 0), toFloat(3, 1), toFloat(3, 2), toFloat(3, 3)
//!     );
//! }
//!
//! float FixedMatrix::fixedToFloat(int16_t integerValue, uint16_t fracValue) {
//!     const uint32_t fullWord = (uint32_t(integerValue) << 16) | fracValue;
//!     return int32_t(fullWord) / 65536.0f;
//! }
//!
//! void FixedMatrix::modifyMatrix4x4Integer(hlslpp::float4x4 &matrix, uint32_t i, uint32_t j, int16_t value) {
//!     const int32_t fixedValue = int32_t(matrix[i][j] * 65536.0f);
//!     matrix[i][j] = fixedToFloat(value, uint16_t(fixedValue & 0xFFFFU));
//! }
//!
//! void FixedMatrix::modifyMatrix4x4Fraction(hlslpp::float4x4 &matrix, uint32_t i, uint32_t j, uint16_t value) {
//!     const int32_t fixedValue = int32_t(matrix[i][j] * 65536.0f);
//!     matrix[i][j] = fixedToFloat(int16_t((fixedValue >> 16) & 0xFFFF), value);
//! }
//! ```
//!
//! **Reuse, not new type.** `FixedMatrix::toMatrix4x4` reuses
//! [`fn64_render_ir::{Mat4, Vec4}`](fn64_render_ir) directly -- no new
//! matrix/vector type, and no `fn64-render-ir` edit. `Mat4` is "a
//! backend-neutral **row-major** 4x4 float matrix, matching HLSL
//! `float4x4`" with `rows[i]` = row `i`, `rows[i].x/y/z/w` = that row's
//! four columns (`rsp_math.rs:78-84`), so an HLSL `m[i][j]` write becomes
//! `m.rows[i].{x,y,z,w}` for `j = 0..3`, matching `rt64_math.rs`'s
//! established convention exactly.
//!
//! `HaltonJitter` returns a plain `(f32, f32)` tuple in place of
//! `hlslpp::float2`, matching `rt64_math.rs::barycentric_coordinates`'s
//! precedent: `fn64_render_ir` has no `Vec2` type (only `Vec3`/`Vec4`), and
//! adding one for this single caller would go against
//! `RENDER-WGPU-PORT-PLAN.md`'s dependency-boundary rule that
//! `fn64-render-ir`'s vector types exist specifically for RSP math.
//!
//! `FixedRect` is ported as an owned Rust struct with the same four
//! `i32` fields (`ulx`/`uly`/`lrx`/`lry`) and the same method surface,
//! since RT64 does not reuse it as any existing `fn64-render-ir` type (no
//! fixed-point rect type exists there).
//!
//! ## Admitted domain
//!
//! - **Debug-only `assert()` preconditions become `debug_assert!`, not
//!   `assert!` or a silent guard.** The C++ source's `assert(!isNull())`
//!   (six `FixedRect` accessors), `assert(!rect.isNull())` (`merge`),
//!   `assert(!isNull()); assert(!rect.isNull())` (`fullyInside`), and
//!   `assert(!isNull()); assert(x >= 0.0f); assert(y >= 0.0f)` (`scaled`)
//!   are *debug-only* in C++ (`NDEBUG` compiles them out in release
//!   builds, and RT64 ships release builds to players) -- so the literal,
//!   most-faithful Rust translation of "a check that exists in debug
//!   builds and is absent in release builds" is `debug_assert!`, not a
//!   release-mode `assert!` (which would be *louder* than the source, an
//!   unrequested behavior widening) and not silently omitting the check
//!   (which would be a silent shrug, contradicting AGENTS.md "loud traps
//!   beat silent shrugs" -- see `coverage.rs`'s `Coverage::new`, which
//!   makes the analogous choice explicit for a *release*-mode C++
//!   `assert!`). Every characterization test below exercises the
//!   `debug_assert!`-free (release-equivalent) arithmetic paths only;
//!   this module does not test panic-on-violated-precondition behavior,
//!   since `cargo nextest` runs are typically debug builds and a
//!   `#[should_panic]` test would be asserting on a build-profile-
//!   dependent property outside this port's characterization scope.
//! - **`FixedRect::intersection` and `contains` have no precondition
//!   asserts in the source** (unlike the six accessors, `merge`,
//!   `fullyInside`, and `scaled`) -- they branch on `isNull()` internally
//!   instead and return a null `FixedRect` / `false` respectively. This
//!   is preserved exactly: no `debug_assert!` was added to either.
//! - **`FixedRect::width`/`height` call `right`/`bottom`/`left`/`top`
//!   internally**, so their own top-level `assert(!isNull())` is
//!   redundant with the ones inside the calls they make -- preserved
//!   verbatim (both asserts present, in the same order as the source),
//!   not deduplicated, since this is a literal port, not a refactor.
//! - **`scaled`'s `int32_t(floorf(...)) << 2`**: left-shifting a
//!   (possibly negative) `i32` by 2 is well-defined 2's-complement
//!   multiply-by-4 in both C++ (implementation-defined pre-C++20, but
//!   universally 2's-complement in practice, which is what RT64 ships
//!   against) and Rust (`<<` on `i32` is a logical/arithmetic-equivalent
//!   bit shift with 2's-complement wraparound on overflow in release,
//!   panic-on-overflow in debug). This port uses plain `<<`, matching
//!   Rust's own debug-overflow-checks convention rather than
//!   `wrapping_shl`, since RT64's `floorf`/`ceilf` inputs are expected to
//!   stay within the fixed-point rect's legal domain and a debug-mode
//!   overflow panic here is a legitimate loud trap, not a widened claim.
//! - **`FixedMatrix::fixedToFloat`'s `(uint32_t(integerValue) << 16) |
//!   fracValue` then `int32_t(fullWord)`**: this is the s16.16
//!   reinterpret-cast round-trip -- ported as `((integer_value as u32) <<
//!   16) | (frac_value as u32)` then `as i32` (a bit-preserving
//!   reinterpret, matching C++ `int32_t(fullWord)` on an unsigned value,
//!   which is implementation-defined pre-C++20 and standardized as
//!   2's-complement-reinterpret from C++20 onward -- the only behavior any
//!   real toolchain implements). Rust's `u32 as i32` is exactly this
//!   bit-preserving reinterpret, so this is a faithful literal port with
//!   no behavior gap.
//! - **`modifyMatrix4x4Integer`/`Fraction`'s `int32_t(matrix[i][j] *
//!   65536.0f)`**: this is a float-to-int C++ `static_cast`, which is
//!   undefined behavior if the scaled value overflows `i32`'s range (RT64
//!   does not clamp before this cast). Rust's `as i32` on an out-of-range
//!   or NaN `f32` **saturates** (`f32::NAN as i32 == 0`, since Rust 1.45)
//!   rather than invoking UB -- this is an intentional, admitted
//!   divergence at this specific input domain (out-of-[i32::MIN,
//!   i32::MAX]-range or NaN `matrix[i][j] * 65536.0`), preserved exactly
//!   as `rt64_math.rs`'s and `depth_strict_less.rs`'s established
//!   precedent of not inventing a panic/guard where the C++ has UB, since
//!   Rust has no UB-preserving float-to-int cast to fall back to.
//!
//! ## Nonclaims
//!
//! No GPU, WGSL, or production wiring (this module is not called from
//! anywhere yet, matching `rt64_math.rs`'s and every other characterization-
//! first module's precedent -- dead-code warnings on the unused public
//! surface are expected and correct), and no RT64 visual/pixel/silicon
//! parity or performance claim. Deliberately not ported from
//! `rt64_common.h`/`.cpp`:
//!
//! - `UpscaleMode` (an unrelated enum, not part of the fixed-point/rect/
//!   matrix/Halton cluster this module scopes to).
//! - `RectI` (a plain `{x, y, w, h}` POD struct with no methods and no
//!   behavior to characterize -- not part of the task's named scope).
//! - `adjustVector<T>` (a `std::vector` capacity/resize helper; no Rust
//!   `Vec<T>` equivalent needed, and out of this module's named scope).
//! - `DepthRayQueryMask`/`NoDepthRayQueryMask`/`ShadowCatcherRayQueryMask`
//!   (plain bitmask constants, no behavior).
//! - `GlobalLastError`/`GlobalLogFile` and the `RT64_LOG_*` macros (global
//!   mutable process state and platform logging plumbing -- out of scope
//!   for a pure-function characterization port, and `fn64-render-wgpu` has
//!   no equivalent logging seam to wire into).
//! - The `DLLEXPORT`/`CPPDLLEXPORT` platform-export macros (build-system
//!   plumbing, not portable behavior).
//! - `FixedRect`'s two constructors (`FixedRect()` / `FixedRect(ulx, uly,
//!   lrx, lry)`) are represented as plain Rust functions (`FixedRect::new`
//!   for the default-constructing form, calling `reset()`'s logic, and
//!   direct struct-literal construction for the four-field form) rather
//!   than a `Default` impl plus a tuple-style constructor -- `Default`
//!   would silently satisfy trait-bound call sites this module does not
//!   yet have, which is an unrequested API-surface widening; `new`/struct-
//!   literal are the minimal literal equivalent.

use fn64_render_ir::{Mat4, Vec4};

/// `HaltonSequence(i, b)`: base-`b` radical inverse of `i`.
pub fn halton_sequence(i: i32, b: i32) -> f32 {
    let mut f: f32 = 1.0;
    let mut r: f32 = 0.0;
    let mut i = i;
    while i > 0 {
        f /= b as f32;
        r += f * ((i % b) as f32);
        i /= b;
    }
    r
}

/// `HaltonJitter(frame, phases)`: returns `(x, y)` in place of
/// `hlslpp::float2` (see module doc -- no `Vec2` type exists in
/// `fn64_render_ir`).
pub fn halton_jitter(frame: i32, phases: i32) -> (f32, f32) {
    let x = halton_sequence(frame % phases + 1, 2) - 0.5;
    let y = halton_sequence(frame % phases + 1, 3) - 0.5;
    (x, y)
}

/// RDP 10.2 fixed-point rectangle: `ulx`/`uly`/`lrx`/`lry` are stored in
/// 1/4-pixel units (2 fractional bits). Mirrors RT64's `FixedRect` exactly.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FixedRect {
    pub ulx: i32,
    pub uly: i32,
    pub lrx: i32,
    pub lry: i32,
}

impl FixedRect {
    /// `FixedRect()`: default-constructs via `reset()`.
    pub fn new() -> Self {
        let mut r = Self {
            ulx: 0,
            uly: 0,
            lrx: 0,
            lry: 0,
        };
        r.reset();
        r
    }

    /// `FixedRect(ulx, uly, lrx, lry)`: four-field constructor.
    pub const fn with_bounds(ulx: i32, uly: i32, lrx: i32, lry: i32) -> Self {
        Self { ulx, uly, lrx, lry }
    }

    /// `reset()`: sets the rect to its canonical "null/empty" sentinel
    /// (`ulx`/`uly` at `i32::MAX`, `lrx`/`lry` at `i32::MIN`).
    pub fn reset(&mut self) {
        self.ulx = i32::MAX;
        self.uly = i32::MAX;
        self.lrx = i32::MIN;
        self.lry = i32::MIN;
    }

    /// `isEmpty()`: null, or zero-width, or zero-height.
    pub fn is_empty(&self) -> bool {
        self.is_null() || (self.lrx == self.ulx) || (self.lry == self.uly)
    }

    /// `isNull()`: upper-left strictly past lower-right on either axis.
    pub fn is_null(&self) -> bool {
        (self.ulx > self.lrx) || (self.uly > self.lry)
    }

    /// `merge(rect)`: in-place bounding union. C++ `assert(!rect.isNull())`
    /// is a debug-only precondition; ported as `debug_assert!` (see module
    /// doc "Admitted domain").
    pub fn merge(&mut self, rect: &FixedRect) {
        debug_assert!(!rect.is_null());

        self.ulx = self.ulx.min(rect.ulx);
        self.uly = self.uly.min(rect.uly);
        self.lrx = self.lrx.max(rect.lrx);
        self.lry = self.lry.max(rect.lry);
    }

    /// `scaled(x, y)`: scales the rect's rounded-pixel bounds by `(x, y)`
    /// and re-quantizes to fixed-point. C++ `assert(!isNull()); assert(x
    /// >= 0.0f); assert(y >= 0.0f)` -- ported as `debug_assert!`.
    pub fn scaled(&self, x: f32, y: f32) -> FixedRect {
        debug_assert!(!self.is_null());
        debug_assert!(x >= 0.0);
        debug_assert!(y >= 0.0);

        FixedRect::with_bounds(
            ((self.left(false) as f32 * x).floor() as i32) << 2,
            ((self.top(false) as f32 * y).floor() as i32) << 2,
            ((self.right(true) as f32 * x).ceil() as i32) << 2,
            ((self.bottom(true) as f32 * y).ceil() as i32) << 2,
        )
    }

    /// `intersection(rect)`: returns the overlap, or a null `FixedRect` if
    /// either operand is null. No precondition assert in the source.
    pub fn intersection(&self, rect: &FixedRect) -> FixedRect {
        if !self.is_null() && !rect.is_null() {
            FixedRect::with_bounds(
                self.ulx.max(rect.ulx),
                self.uly.max(rect.uly),
                self.lrx.min(rect.lrx),
                self.lry.min(rect.lry),
            )
        } else {
            FixedRect::new()
        }
    }

    /// `contains(x, y)`: point-in-rect test; `false` if null. No
    /// precondition assert in the source.
    pub fn contains(&self, x: i32, y: i32) -> bool {
        if !self.is_null() {
            (x >= self.ulx) && (x <= self.lrx) && (y >= self.uly) && (y <= self.lry)
        } else {
            false
        }
    }

    /// `fullyInside(rect)`: true if `self` is fully inside `rect`. C++
    /// `assert(!isNull()); assert(!rect.isNull())` -- ported as
    /// `debug_assert!`.
    pub fn fully_inside(&self, rect: &FixedRect) -> bool {
        debug_assert!(!self.is_null());
        debug_assert!(!rect.is_null());
        (rect.ulx >= self.ulx)
            && (rect.uly >= self.uly)
            && (rect.lrx <= self.lrx)
            && (rect.lry <= self.lry)
    }

    /// `left(ceil)`: `(ulx + (ceil?3:0)) >> 2`. C++ `assert(!isNull())` --
    /// ported as `debug_assert!`.
    pub fn left(&self, ceil: bool) -> i32 {
        debug_assert!(!self.is_null());
        (self.ulx + if ceil { 3 } else { 0 }) >> 2
    }

    /// `top(ceil)`: `(uly + (ceil?3:0)) >> 2`. C++ `assert(!isNull())` --
    /// ported as `debug_assert!`.
    pub fn top(&self, ceil: bool) -> i32 {
        debug_assert!(!self.is_null());
        (self.uly + if ceil { 3 } else { 0 }) >> 2
    }

    /// `right(ceil)`: `(lrx + (ceil?3:0)) >> 2`. C++ `assert(!isNull())` --
    /// ported as `debug_assert!`.
    pub fn right(&self, ceil: bool) -> i32 {
        debug_assert!(!self.is_null());
        (self.lrx + if ceil { 3 } else { 0 }) >> 2
    }

    /// `bottom(ceil)`: `(lry + (ceil?3:0)) >> 2`. C++ `assert(!isNull())`
    /// -- ported as `debug_assert!`.
    pub fn bottom(&self, ceil: bool) -> i32 {
        debug_assert!(!self.is_null());
        (self.lry + if ceil { 3 } else { 0 }) >> 2
    }

    /// `width(leftCeil, rightCeil)`: `right(rightCeil) - left(leftCeil)`.
    /// C++ `assert(!isNull())` (redundant with the asserts inside
    /// `right`/`left`, preserved verbatim -- see module doc).
    pub fn width(&self, left_ceil: bool, right_ceil: bool) -> i32 {
        debug_assert!(!self.is_null());
        self.right(right_ceil) - self.left(left_ceil)
    }

    /// `height(topCeil, bottomCeil)`: `bottom(bottomCeil) - top(topCeil)`.
    /// C++ `assert(!isNull())` (redundant with the asserts inside
    /// `bottom`/`top`, preserved verbatim -- see module doc).
    pub fn height(&self, top_ceil: bool, bottom_ceil: bool) -> i32 {
        debug_assert!(!self.is_null());
        self.bottom(bottom_ceil) - self.top(top_ceil)
    }
}

impl Default for FixedRect {
    fn default() -> Self {
        Self::new()
    }
}

/// RDP s16.16 fixed-point 4x4 matrix: separate integer and fractional
/// planes, N64-microcode style. Mirrors RT64's `FixedMatrix` exactly.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FixedMatrix {
    pub integer: [[i16; 4]; 4],
    pub frac: [[u16; 4]; 4],
}

impl FixedMatrix {
    /// `toFloat(i, j)`: reads element `[i][j ^ 1]` (RDP's column-pair swap)
    /// as an s16.16 fixed-point value.
    pub fn to_float(&self, i: usize, j: usize) -> f32 {
        let xor_j = j ^ 1;
        FixedMatrix::fixed_to_float(self.integer[i][xor_j], self.frac[i][xor_j])
    }

    /// `toMatrix4x4()`: builds a full [`Mat4`] from `toFloat(i, j)` for all
    /// 16 `(i, j)` pairs, row-major (`m.rows[i].{x,y,z,w}` = `j = 0..3`).
    pub fn to_matrix4x4(&self) -> Mat4 {
        Mat4::from_rows([
            Vec4::new(
                self.to_float(0, 0),
                self.to_float(0, 1),
                self.to_float(0, 2),
                self.to_float(0, 3),
            ),
            Vec4::new(
                self.to_float(1, 0),
                self.to_float(1, 1),
                self.to_float(1, 2),
                self.to_float(1, 3),
            ),
            Vec4::new(
                self.to_float(2, 0),
                self.to_float(2, 1),
                self.to_float(2, 2),
                self.to_float(2, 3),
            ),
            Vec4::new(
                self.to_float(3, 0),
                self.to_float(3, 1),
                self.to_float(3, 2),
                self.to_float(3, 3),
            ),
        ])
    }

    /// `fixedToFloat(integerValue, fracValue)`: s16.16 fixed-point ->
    /// `f32`. Bit-preserving reinterpret of the packed 32-bit word as
    /// signed, then divide by 65536 (see module doc "Admitted domain").
    pub fn fixed_to_float(integer_value: i16, frac_value: u16) -> f32 {
        let full_word: u32 = ((integer_value as u32) << 16) | (frac_value as u32);
        (full_word as i32) as f32 / 65536.0
    }

    /// `modifyMatrix4x4Integer(matrix, i, j, value)`: replaces element
    /// `[i][j]`'s integer half in place, preserving its existing
    /// fractional bits.
    pub fn modify_matrix4x4_integer(matrix: &mut Mat4, i: usize, j: usize, value: i16) {
        let current = get_elem(matrix, i, j);
        let fixed_value = (current * 65536.0) as i32;
        let new_value = FixedMatrix::fixed_to_float(value, (fixed_value & 0xFFFF) as u16);
        set_elem(matrix, i, j, new_value);
    }

    /// `modifyMatrix4x4Fraction(matrix, i, j, value)`: replaces element
    /// `[i][j]`'s fractional half in place, preserving its existing
    /// integer bits.
    pub fn modify_matrix4x4_fraction(matrix: &mut Mat4, i: usize, j: usize, value: u16) {
        let current = get_elem(matrix, i, j);
        let fixed_value = (current * 65536.0) as i32;
        let new_integer = ((fixed_value >> 16) & 0xFFFF) as i16;
        let new_value = FixedMatrix::fixed_to_float(new_integer, value);
        set_elem(matrix, i, j, new_value);
    }
}

/// `matrix[i][j]` read helper for [`Mat4`]'s row-major `{x,y,z,w}` shape
/// (see module doc "Reuse, not new type").
fn get_elem(m: &Mat4, i: usize, j: usize) -> f32 {
    let row = &m.rows[i];
    match j {
        0 => row.x,
        1 => row.y,
        2 => row.z,
        3 => row.w,
        _ => panic!("matrix column index out of range: {j}"),
    }
}

/// `matrix[i][j] = value` write helper for [`Mat4`]'s row-major `{x,y,z,w}`
/// shape (see module doc "Reuse, not new type").
fn set_elem(m: &mut Mat4, i: usize, j: usize, value: f32) {
    let row = &mut m.rows[i];
    match j {
        0 => row.x = value,
        1 => row.y = value,
        2 => row.z = value,
        3 => row.w = value,
        _ => panic!("matrix column index out of range: {j}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- halton_sequence ---

    #[test]
    fn halton_sequence_i_zero_is_zero() {
        assert_eq!(halton_sequence(0, 2), 0.0);
    }

    #[test]
    fn halton_sequence_negative_i_is_zero() {
        // while (i > 0) never executes for i <= 0.
        assert_eq!(halton_sequence(-5, 2), 0.0);
    }

    #[test]
    fn halton_sequence_base_2_i_1() {
        assert_eq!(halton_sequence(1, 2), 0.5);
    }

    #[test]
    fn halton_sequence_base_2_i_2() {
        assert_eq!(halton_sequence(2, 2), 0.25);
    }

    #[test]
    fn halton_sequence_base_2_i_3() {
        let r = halton_sequence(3, 2);
        assert!((r - 0.75).abs() < 1e-6, "r={r}");
    }

    #[test]
    fn halton_sequence_base_2_i_4() {
        let r = halton_sequence(4, 2);
        assert!((r - 0.125).abs() < 1e-6, "r={r}");
    }

    #[test]
    fn halton_sequence_base_3_i_1() {
        let r = halton_sequence(1, 3);
        assert!((r - (1.0 / 3.0)).abs() < 1e-6, "r={r}");
    }

    #[test]
    fn halton_sequence_base_3_i_2() {
        let r = halton_sequence(2, 3);
        assert!((r - (2.0 / 3.0)).abs() < 1e-6, "r={r}");
    }

    #[test]
    fn halton_sequence_base_3_i_4() {
        // i=4: iter1: i%3=1 -> r=1/3, i=4/3=1; iter2: i%3=1 -> r += 1/9 ->
        // r=4/9, i=1/3=0 -> loop ends.
        let r = halton_sequence(4, 3);
        assert!((r - (4.0 / 9.0)).abs() < 1e-5, "r={r}");
    }

    #[test]
    fn halton_sequence_stays_within_unit_interval() {
        for i in 1..50 {
            let r = halton_sequence(i, 2);
            assert!((0.0..1.0).contains(&r), "i={i} r={r}");
        }
    }

    // --- halton_jitter ---

    #[test]
    fn halton_jitter_frame_zero_phases_one() {
        // frame % phases + 1 = 0 % 1 + 1 = 1 for both x and y.
        let (x, y) = halton_jitter(0, 1);
        assert!((x - (0.5 - 0.5)).abs() < 1e-6);
        assert!((y - ((1.0 / 3.0) - 0.5)).abs() < 1e-6, "y={y}");
    }

    #[test]
    fn halton_jitter_centered_range_is_within_half_open_interval() {
        for frame in 0..20 {
            let (x, y) = halton_jitter(frame, 8);
            assert!((-0.5..0.5).contains(&x), "frame={frame} x={x}");
            assert!((-0.5..0.5).contains(&y), "frame={frame} y={y}");
        }
    }

    #[test]
    fn halton_jitter_wraps_with_phases() {
        // frame=phases and frame=0 both give frame % phases == 0, so the
        // jitter value must repeat exactly.
        let a = halton_jitter(0, 4);
        let b = halton_jitter(4, 4);
        assert_eq!(a, b);
    }

    // --- FixedRect::reset / new / is_null / is_empty ---

    #[test]
    fn fixed_rect_new_is_null() {
        assert!(FixedRect::new().is_null());
    }

    #[test]
    fn fixed_rect_new_is_empty() {
        assert!(FixedRect::new().is_empty());
    }

    #[test]
    fn fixed_rect_reset_sets_sentinel_bounds() {
        let mut r = FixedRect::with_bounds(1, 2, 3, 4);
        r.reset();
        assert_eq!(r.ulx, i32::MAX);
        assert_eq!(r.uly, i32::MAX);
        assert_eq!(r.lrx, i32::MIN);
        assert_eq!(r.lry, i32::MIN);
    }

    #[test]
    fn fixed_rect_with_bounds_normal_rect_is_not_null() {
        let r = FixedRect::with_bounds(0, 0, 8, 8);
        assert!(!r.is_null());
    }

    #[test]
    fn fixed_rect_zero_width_is_empty_but_not_null() {
        // lrx == ulx, uly < lry: not null (ulx <= lrx, uly <= lry) but empty.
        let r = FixedRect::with_bounds(4, 0, 4, 8);
        assert!(!r.is_null());
        assert!(r.is_empty());
    }

    #[test]
    fn fixed_rect_zero_height_is_empty_but_not_null() {
        let r = FixedRect::with_bounds(0, 4, 8, 4);
        assert!(!r.is_null());
        assert!(r.is_empty());
    }

    #[test]
    fn fixed_rect_ulx_greater_than_lrx_is_null() {
        let r = FixedRect::with_bounds(8, 0, 0, 8);
        assert!(r.is_null());
    }

    #[test]
    fn fixed_rect_uly_greater_than_lry_is_null() {
        let r = FixedRect::with_bounds(0, 8, 8, 0);
        assert!(r.is_null());
    }

    #[test]
    fn fixed_rect_null_implies_empty() {
        // isEmpty() = isNull() || ... -- isNull() alone is sufficient.
        let r = FixedRect::with_bounds(8, 8, 0, 0);
        assert!(r.is_null());
        assert!(r.is_empty());
    }

    #[test]
    fn fixed_rect_nonzero_area_is_not_empty() {
        let r = FixedRect::with_bounds(0, 0, 8, 8);
        assert!(!r.is_empty());
    }

    // --- FixedRect::merge ---

    #[test]
    fn fixed_rect_merge_expands_to_union_bounds() {
        let mut a = FixedRect::with_bounds(2, 2, 6, 6);
        let b = FixedRect::with_bounds(0, 4, 4, 10);
        a.merge(&b);
        assert_eq!(a, FixedRect::with_bounds(0, 2, 6, 10));
    }

    #[test]
    fn fixed_rect_merge_with_reset_rect_yields_the_other_rect() {
        let mut a = FixedRect::new();
        let b = FixedRect::with_bounds(1, 2, 3, 4);
        a.merge(&b);
        assert_eq!(a, b);
    }

    #[test]
    fn fixed_rect_merge_fully_contained_rect_is_no_op() {
        let mut a = FixedRect::with_bounds(0, 0, 10, 10);
        let b = FixedRect::with_bounds(2, 2, 8, 8);
        a.merge(&b);
        assert_eq!(a, FixedRect::with_bounds(0, 0, 10, 10));
    }

    // --- FixedRect::intersection ---

    #[test]
    fn fixed_rect_intersection_overlapping_rects() {
        let a = FixedRect::with_bounds(0, 0, 8, 8);
        let b = FixedRect::with_bounds(4, 4, 12, 12);
        let i = a.intersection(&b);
        assert_eq!(i, FixedRect::with_bounds(4, 4, 8, 8));
        assert!(!i.is_null());
    }

    #[test]
    fn fixed_rect_intersection_non_overlapping_rects_is_null() {
        let a = FixedRect::with_bounds(0, 0, 4, 4);
        let b = FixedRect::with_bounds(10, 10, 14, 14);
        let i = a.intersection(&b);
        assert!(i.is_null());
    }

    #[test]
    fn fixed_rect_intersection_with_null_operand_is_null_result_not_the_input() {
        let a = FixedRect::with_bounds(0, 0, 8, 8);
        let b = FixedRect::new();
        let i = a.intersection(&b);
        assert!(i.is_null());
        // Result is FixedRect::new()'s sentinel, not max/min of a and b.
        assert_eq!(i, FixedRect::new());
    }

    #[test]
    fn fixed_rect_intersection_identical_rects_is_itself() {
        let a = FixedRect::with_bounds(1, 2, 3, 4);
        let i = a.intersection(&a);
        assert_eq!(i, a);
    }

    #[test]
    fn fixed_rect_intersection_touching_edges_is_zero_area_not_null() {
        // Rects share exactly the line lrx==ulx of the other -- min/max
        // produces a zero-width (empty but non-null) rect.
        let a = FixedRect::with_bounds(0, 0, 4, 4);
        let b = FixedRect::with_bounds(4, 0, 8, 4);
        let i = a.intersection(&b);
        assert!(!i.is_null());
        assert!(i.is_empty());
        assert_eq!(i, FixedRect::with_bounds(4, 0, 4, 4));
    }

    // --- FixedRect::contains ---

    #[test]
    fn fixed_rect_contains_point_inside() {
        let r = FixedRect::with_bounds(0, 0, 8, 8);
        assert!(r.contains(4, 4));
    }

    #[test]
    fn fixed_rect_contains_point_on_boundary_is_inclusive() {
        let r = FixedRect::with_bounds(0, 0, 8, 8);
        assert!(r.contains(0, 0));
        assert!(r.contains(8, 8));
    }

    #[test]
    fn fixed_rect_contains_point_outside() {
        let r = FixedRect::with_bounds(0, 0, 8, 8);
        assert!(!r.contains(9, 4));
        assert!(!r.contains(4, 9));
        assert!(!r.contains(-1, 4));
    }

    #[test]
    fn fixed_rect_contains_on_null_rect_is_always_false() {
        let r = FixedRect::new();
        assert!(!r.contains(0, 0));
        assert!(!r.contains(i32::MAX, i32::MAX));
    }

    // --- FixedRect::fully_inside ---

    #[test]
    fn fixed_rect_fully_inside_true_when_self_contains_rect() {
        let outer = FixedRect::with_bounds(0, 0, 10, 10);
        let inner = FixedRect::with_bounds(2, 2, 8, 8);
        assert!(outer.fully_inside(&inner));
    }

    #[test]
    fn fixed_rect_fully_inside_false_when_rect_extends_beyond_self() {
        let outer = FixedRect::with_bounds(0, 0, 10, 10);
        let overflowing = FixedRect::with_bounds(2, 2, 12, 8);
        assert!(!outer.fully_inside(&overflowing));
    }

    #[test]
    fn fixed_rect_fully_inside_identical_rects_true() {
        let r = FixedRect::with_bounds(1, 2, 3, 4);
        assert!(r.fully_inside(&r));
    }

    // --- FixedRect accessors: ceil flag on all six ---

    #[test]
    fn fixed_rect_left_without_ceil_truncates() {
        let r = FixedRect::with_bounds(5, 0, 20, 0);
        // (5 + 0) >> 2 = 1.
        assert_eq!(r.left(false), 1);
    }

    #[test]
    fn fixed_rect_left_with_ceil_rounds_up() {
        let r = FixedRect::with_bounds(5, 0, 20, 0);
        // (5 + 3) >> 2 = 2.
        assert_eq!(r.left(true), 2);
    }

    #[test]
    fn fixed_rect_left_exact_multiple_of_four_ceil_is_a_no_op() {
        let r = FixedRect::with_bounds(8, 0, 20, 0);
        // (8+0)>>2 = 2, (8+3)>>2 = 2 -- ceil only matters on a true remainder.
        assert_eq!(r.left(false), 2);
        assert_eq!(r.left(true), 2);
    }

    #[test]
    fn fixed_rect_top_without_ceil_truncates() {
        let r = FixedRect::with_bounds(0, 5, 0, 20);
        assert_eq!(r.top(false), 1);
    }

    #[test]
    fn fixed_rect_top_with_ceil_rounds_up() {
        let r = FixedRect::with_bounds(0, 5, 0, 20);
        assert_eq!(r.top(true), 2);
    }

    #[test]
    fn fixed_rect_right_without_ceil_truncates() {
        let r = FixedRect::with_bounds(0, 0, 21, 0);
        // (21+0)>>2 = 5.
        assert_eq!(r.right(false), 5);
    }

    #[test]
    fn fixed_rect_right_with_ceil_rounds_up() {
        let r = FixedRect::with_bounds(0, 0, 21, 0);
        // (21+3)>>2 = 6.
        assert_eq!(r.right(true), 6);
    }

    #[test]
    fn fixed_rect_bottom_without_ceil_truncates() {
        let r = FixedRect::with_bounds(0, 0, 0, 21);
        assert_eq!(r.bottom(false), 5);
    }

    #[test]
    fn fixed_rect_bottom_with_ceil_rounds_up() {
        let r = FixedRect::with_bounds(0, 0, 0, 21);
        assert_eq!(r.bottom(true), 6);
    }

    #[test]
    fn fixed_rect_left_negative_ulx_arithmetic_shift_rounds_toward_negative_infinity() {
        // -5 in two's complement: (-5 + 0) >> 2 (arithmetic shift) = -2,
        // not -1 (which truncating-toward-zero division would give).
        let r = FixedRect::with_bounds(-5, 0, 20, 0);
        assert_eq!(r.left(false), -2);
        assert_eq!(-5i32 >> 2, -2);
    }

    #[test]
    fn fixed_rect_left_negative_ulx_with_ceil() {
        // (-5 + 3) >> 2 = (-2) >> 2 = -1.
        let r = FixedRect::with_bounds(-5, 0, 20, 0);
        assert_eq!(r.left(true), -1);
    }

    #[test]
    fn fixed_rect_width_uses_right_minus_left() {
        let r = FixedRect::with_bounds(4, 0, 20, 0);
        // left(false) = 1, right(false) = 5 -> width = 4.
        assert_eq!(r.width(false, false), 4);
    }

    #[test]
    fn fixed_rect_width_mixed_ceil_flags() {
        let r = FixedRect::with_bounds(5, 0, 21, 0);
        // left(true) = 2, right(false) = 5 -> width = 3.
        assert_eq!(r.width(true, false), 3);
    }

    #[test]
    fn fixed_rect_height_uses_bottom_minus_top() {
        let r = FixedRect::with_bounds(0, 4, 0, 20);
        assert_eq!(r.height(false, false), 4);
    }

    #[test]
    fn fixed_rect_height_mixed_ceil_flags() {
        let r = FixedRect::with_bounds(0, 5, 0, 21);
        assert_eq!(r.height(true, false), 3);
    }

    // --- FixedRect::scaled ---

    #[test]
    fn fixed_rect_scaled_identity_scale_round_trips_through_quarter_pixel_quantization() {
        // ulx/uly/lrx/lry all multiples of 4 (whole pixels) round-trip exactly.
        let r = FixedRect::with_bounds(4, 8, 40, 80);
        let s = r.scaled(1.0, 1.0);
        assert_eq!(s, r);
    }

    #[test]
    fn fixed_rect_scaled_doubles_bounds_at_scale_two() {
        let r = FixedRect::with_bounds(4, 8, 40, 80);
        let s = r.scaled(2.0, 2.0);
        // left(false)=1 -> floor(1*2)=2 -> <<2 = 8.
        // top(false)=2 -> floor(2*2)=4 -> <<2 = 16.
        // right(true)=10 -> ceil(10*2)=20 -> <<2 = 80.
        // bottom(true)=20 -> ceil(20*2)=40 -> <<2 = 160.
        assert_eq!(s, FixedRect::with_bounds(8, 16, 80, 160));
    }

    #[test]
    fn fixed_rect_scaled_zero_scale_collapses_to_origin() {
        let r = FixedRect::with_bounds(4, 8, 40, 80);
        let s = r.scaled(0.0, 0.0);
        assert_eq!(s, FixedRect::with_bounds(0, 0, 0, 0));
    }

    #[test]
    fn fixed_rect_scaled_fractional_scale_uses_floor_for_ul_ceil_for_lr() {
        // left(false)=1, 1*1.5=1.5 -> floor=1 -> <<2=4.
        // right(true) with lrx=21: (21+3)>>2=6, 6*1.5=9.0 -> ceil=9 -> <<2=36.
        let r = FixedRect::with_bounds(4, 0, 21, 0);
        let s = r.scaled(1.5, 1.0);
        assert_eq!(s.ulx, 4);
        assert_eq!(s.lrx, 36);
    }

    // --- FixedMatrix::fixed_to_float ---

    #[test]
    fn fixed_to_float_zero_is_zero() {
        assert_eq!(FixedMatrix::fixed_to_float(0, 0), 0.0);
    }

    #[test]
    fn fixed_to_float_one_integer_no_fraction() {
        assert_eq!(FixedMatrix::fixed_to_float(1, 0), 1.0);
    }

    #[test]
    fn fixed_to_float_half_fraction() {
        // fracValue = 0x8000 -> 32768/65536 = 0.5.
        assert_eq!(FixedMatrix::fixed_to_float(0, 0x8000), 0.5);
    }

    #[test]
    fn fixed_to_float_one_and_a_half() {
        assert_eq!(FixedMatrix::fixed_to_float(1, 0x8000), 1.5);
    }

    #[test]
    fn fixed_to_float_negative_one_integer() {
        // integer=-1, frac=0: fullWord = (u32(-1) << 16) | 0 = 0xFFFF0000,
        // reinterpreted signed = -65536, / 65536.0 = -1.0.
        assert_eq!(FixedMatrix::fixed_to_float(-1, 0), -1.0);
    }

    #[test]
    fn fixed_to_float_negative_one_and_a_half() {
        // integer=-2, frac=0x8000: fullWord = 0xFFFE8000 as i32 = -98304,
        // / 65536.0 = -1.5.
        assert_eq!(FixedMatrix::fixed_to_float(-2, 0x8000), -1.5);
    }

    #[test]
    fn fixed_to_float_min_i16_max_frac_rounds_to_negative_32767_at_f32_precision() {
        let v = FixedMatrix::fixed_to_float(i16::MIN, u16::MAX);
        // integer=-32768 (0x8000), frac=65535 (0xFFFF): fullWord =
        // 0x8000FFFF as i32 = -2147418113, exact quotient
        // -32767.0000152587890625 -- just above -32768.0, since the max
        // positive fraction nudges the packed word toward zero from the
        // most-negative integer. At f32 precision (ULP 0.00390625 at this
        // magnitude) this rounds to exactly -32767.0.
        assert_eq!(v, -32767.0);
    }

    #[test]
    fn fixed_to_float_max_i16_max_frac_rounds_to_32768_at_f32_precision() {
        let v = FixedMatrix::fixed_to_float(i16::MAX, u16::MAX);
        // integer=32767, frac=65535: the exact quotient is
        // 2147483647/65536.0 = 32767.999984741..., but f32's 24-bit
        // mantissa cannot distinguish that from 32768.0 at this
        // magnitude (ULP at 2^15 is 2^-8 = 0.00390625) -- so it rounds up
        // to exactly 32768.0. This is genuine upstream C++ `float`
        // behavior (`float` is IEEE-754 binary32 there too), not a Rust
        // divergence.
        assert_eq!(v, 32768.0);
    }

    #[test]
    fn fixed_to_float_sign_wraps_at_integer_zero_with_nonzero_frac() {
        // integer=0, frac nonzero is positive; integer=-1, frac nonzero is
        // still negative overall (frac bits are part of the same 32-bit
        // two's-complement word, not a separate sign-magnitude field).
        let positive = FixedMatrix::fixed_to_float(0, 1);
        let negative = FixedMatrix::fixed_to_float(-1, 1);
        assert!(positive > 0.0);
        assert!(negative < 0.0);
        // -1 + (1/65536) = -0.999984741...
        assert!(
            (negative - (-1.0 + 1.0 / 65536.0)).abs() < 1e-6,
            "negative={negative}"
        );
    }

    // --- FixedMatrix::to_float / j^1 transpose ---

    fn matrix_with(mut set: impl FnMut(&mut FixedMatrix)) -> FixedMatrix {
        let mut m = FixedMatrix {
            integer: [[0; 4]; 4],
            frac: [[0; 4]; 4],
        };
        set(&mut m);
        m
    }

    #[test]
    fn to_float_reads_column_j_xor_1_not_j() {
        // Set integer[0][1] = 5 (i.e. the physical column 1), then read
        // toFloat(0, 0): xorJ = 0^1 = 1, so it must read column 1, not 0.
        let m = matrix_with(|m| m.integer[0][1] = 5);
        assert_eq!(m.to_float(0, 0), 5.0);
        assert_eq!(m.to_float(0, 1), 0.0);
    }

    #[test]
    fn to_float_j_xor_1_swaps_columns_zero_and_one() {
        let m = matrix_with(|m| {
            m.integer[2][0] = 10;
            m.integer[2][1] = 20;
        });
        // j=0 -> xorJ=1 -> reads physical column 1 (value 20).
        // j=1 -> xorJ=0 -> reads physical column 0 (value 10).
        assert_eq!(m.to_float(2, 0), 20.0);
        assert_eq!(m.to_float(2, 1), 10.0);
    }

    #[test]
    fn to_float_j_xor_1_swaps_columns_two_and_three() {
        let m = matrix_with(|m| {
            m.integer[1][2] = 7;
            m.integer[1][3] = 9;
        });
        // j=2 -> xorJ=3 (value 9); j=3 -> xorJ=2 (value 7).
        assert_eq!(m.to_float(1, 2), 9.0);
        assert_eq!(m.to_float(1, 3), 7.0);
    }

    #[test]
    fn to_float_row_index_is_not_permuted() {
        // Only the column index is XORed; the row index i passes straight
        // through.
        let m = matrix_with(|m| m.integer[3][1] = 42);
        assert_eq!(m.to_float(3, 0), 42.0);
        for row in 0..3 {
            assert_eq!(m.to_float(row, 0), 0.0);
        }
    }

    // --- FixedMatrix::to_matrix4x4 ---

    #[test]
    fn to_matrix4x4_zero_matrix_is_all_zero_mat4() {
        let m = FixedMatrix {
            integer: [[0; 4]; 4],
            frac: [[0; 4]; 4],
        };
        let out = m.to_matrix4x4();
        for row in out.rows {
            assert_eq!(row, Vec4::new(0.0, 0.0, 0.0, 0.0));
        }
    }

    #[test]
    fn to_matrix4x4_applies_j_xor_1_across_the_full_row() {
        // Physical row 0: columns [0,1,2,3] = [1,2,3,4]. toFloat(0,j) reads
        // column j^1, so the logical row must be [2,1,4,3].
        let m = matrix_with(|m| {
            m.integer[0] = [1, 2, 3, 4];
        });
        let out = m.to_matrix4x4();
        assert_eq!(out.rows[0], Vec4::new(2.0, 1.0, 4.0, 3.0));
    }

    #[test]
    fn to_matrix4x4_identity_like_fixed_matrix_round_trips_via_modify() {
        // Build a Mat4 identity, then verify to_matrix4x4 . a FixedMatrix
        // populated via modify_matrix4x4_integer reproduces 1.0 on the
        // logical diagonal cells that map through j^1.
        let mut fm = FixedMatrix {
            integer: [[0; 4]; 4],
            frac: [[0; 4]; 4],
        };
        // Physical column 1 holds logical column 0's value (since
        // toFloat(i,0) reads xorJ=1).
        fm.integer[0][1] = 1;
        fm.integer[1][0] = 1;
        fm.integer[2][3] = 1;
        fm.integer[3][2] = 1;
        let out = fm.to_matrix4x4();
        assert_eq!(
            out,
            Mat4::from_rows([
                Vec4::new(1.0, 0.0, 0.0, 0.0),
                Vec4::new(0.0, 1.0, 0.0, 0.0),
                Vec4::new(0.0, 0.0, 1.0, 0.0),
                Vec4::new(0.0, 0.0, 0.0, 1.0),
            ])
        );
    }

    // --- FixedMatrix::modify_matrix4x4_integer / modify_matrix4x4_fraction ---

    #[test]
    fn modify_matrix4x4_integer_sets_integer_half_preserves_fraction() {
        let mut m = Mat4::from_rows([Vec4::new(0.0, 0.0, 0.0, 0.0); 4]);
        // Start at 0.5 (integer=0, frac=0x8000).
        set_elem(&mut m, 0, 0, 0.5);
        FixedMatrix::modify_matrix4x4_integer(&mut m, 0, 0, 3);
        // 0.5 * 65536 = 32768 = 0x8000; low 16 bits preserved as frac.
        assert_eq!(get_elem(&m, 0, 0), 3.5);
    }

    #[test]
    fn modify_matrix4x4_integer_zero_fraction_gives_whole_number() {
        let mut m = Mat4::from_rows([Vec4::new(0.0, 0.0, 0.0, 0.0); 4]);
        FixedMatrix::modify_matrix4x4_integer(&mut m, 1, 2, 7);
        assert_eq!(get_elem(&m, 1, 2), 7.0);
    }

    #[test]
    fn modify_matrix4x4_fraction_sets_fraction_half_preserves_integer() {
        let mut m = Mat4::from_rows([Vec4::new(0.0, 0.0, 0.0, 0.0); 4]);
        set_elem(&mut m, 2, 1, 4.0);
        // frac 0x4000 = 16384/65536 = 0.25.
        FixedMatrix::modify_matrix4x4_fraction(&mut m, 2, 1, 0x4000);
        assert_eq!(get_elem(&m, 2, 1), 4.25);
    }

    #[test]
    fn modify_matrix4x4_fraction_on_negative_integer_preserves_sign() {
        let mut m = Mat4::from_rows([Vec4::new(0.0, 0.0, 0.0, 0.0); 4]);
        set_elem(&mut m, 0, 0, -3.0);
        FixedMatrix::modify_matrix4x4_fraction(&mut m, 0, 0, 0x8000);
        assert_eq!(get_elem(&m, 0, 0), -2.5);
    }

    #[test]
    fn modify_matrix4x4_integer_then_fraction_composes() {
        let mut m = Mat4::from_rows([Vec4::new(0.0, 0.0, 0.0, 0.0); 4]);
        FixedMatrix::modify_matrix4x4_integer(&mut m, 3, 3, 2);
        FixedMatrix::modify_matrix4x4_fraction(&mut m, 3, 3, 0x8000);
        assert_eq!(get_elem(&m, 3, 3), 2.5);
    }

    #[test]
    fn modify_matrix4x4_integer_negative_value_round_trips() {
        let mut m = Mat4::from_rows([Vec4::new(0.0, 0.0, 0.0, 0.0); 4]);
        FixedMatrix::modify_matrix4x4_integer(&mut m, 0, 0, -5);
        assert_eq!(get_elem(&m, 0, 0), -5.0);
    }
}
