//! Literal port of RT64 `rt64_math.cpp`'s deferred matrix cluster --
//! `extract3x3`, `rotationFrom3x3`, `matrixDifference`, `lerpMatrix`,
//! `lerpMatrix3x3`, `lerpMatrixComponents`, plus the transform-constructor
//! family `matrixScale` (both overloads), `matrixTranslation` and
//! `matrixRotationX/Y/Z` -- a literal port of the
//! permitted MIT RT64 Rust-port source pinned at commit
//! `5473732a822a4423b5696e7cb18fecc425a59875`
//! (`docs/RT64-PORT-AUTHORITY.md`), `src/common/rt64_math.h`/`.cpp`
//! (SHA-256 of the whole files,
//! `d0d768a666555a3099b564fe8b7f62af088e921e7ebc8f9d232e5e3239f406a9` /
//! `d32abc9572001870b4144ffa49e832589858de0830dbb0d008761ad15a76364b`):
//! `rt64_math.rs`'s own Nonclaims section explicitly deferred this cluster
//! ("Does not port ... `extract3x3`, `rotationFrom3x3`, `matrixDifference`,
//! `lerpMatrix`/`lerpMatrix3x3`/`lerpMatrixComponents` ... (deferred --
//! needs new matrix-inverse/quaternion infra)"); this module closes that
//! deferral for the six matrix-shaped functions only. The quaternion
//! `decomposeMatrix`/`recomposeMatrix`/`DecomposedTransform`/
//! `lerpTransforms` cluster in the same source file is a separate,
//! independently-owned port and is NOT ported here (see "Nonclaims").
//!
//! Source line ranges (`src/common/rt64_math.cpp`):
//!
//! ```text
//! // lines 116-122
//! hlslpp::float3x3 extract3x3(const hlslpp::float4x4 &m) {
//!     return hlslpp::float3x3(
//!         m[0][0], m[0][1], m[0][2],
//!         m[1][0], m[1][1], m[1][2],
//!         m[2][0], m[2][1], m[2][2]
//!     );
//! }
//!
//! // lines 124-126
//! hlslpp::float3x3 rotationFrom3x3(const hlslpp::float3x3 &m) {
//!     return hlslpp::float3x3(hlslpp::normalize(m[0]), hlslpp::normalize(m[1]), hlslpp::normalize(m[2]));
//! }
//!
//! // lines 132-139
//! float matrixDifference(const hlslpp::float4x4 &a, const hlslpp::float4x4 &b) {
//!     float difference = 0.0f;
//!     for (int i = 0; i < 4; i++) {
//!         difference += hlslpp::dot(hlslpp::abs(a[i] - b[i]), 1.0f);
//!     }
//!
//!     return difference;
//! }
//!
//! // lines 153-163
//! hlslpp::float4x4 lerpMatrix(const hlslpp::float4x4 &a, const hlslpp::float4x4 &b, float t) {
//!     // Copy b into the result.
//!     hlslpp::float4x4 c = b;
//!
//!     // Replace with a component-wise linear interpolation between a and b.
//!     for (int i = 0; i < 4; i++) {
//!         c[i] = hlslpp::lerp(a[i], b[i], t);
//!     }
//!
//!     return c;
//! }
//!
//! // lines 165-175
//! hlslpp::float4x4 lerpMatrix3x3(const hlslpp::float4x4 &a, const hlslpp::float4x4 &b, float t) {
//!     // Copy b into the result.
//!     hlslpp::float4x4 c = b;
//!
//!     // Replace the result's top left 3x3 with a component-wise linear interpolation between a and b.
//!     for (int i = 0; i < 3; i++) {
//!         c[i].xyz = hlslpp::lerp(a[i].xyz, b[i].xyz, t);
//!     }
//!
//!     return c;
//! }
//!
//! // lines 177-199
//! hlslpp::float4x4 lerpMatrixComponents(const hlslpp::float4x4 &a, const hlslpp::float4x4 &b, bool linear, bool angular, bool perspective, float t) {
//!     hlslpp::float4x4 ret;
//!     // Start by either component-wise lerping the top left 3x3 if rotation is enabled or directly copying it otherwise.
//!     // This leaves the last row and last column as a copy of b's in either case.
//!     if (angular) {
//!         ret = lerpMatrix3x3(a, b, t);
//!     }
//!     else {
//!         ret = b;
//!     }
//!     // Next, lerp the translation component of the last row if enabled, otherwise leave it intact from the initial copy step.
//!     if (linear) {
//!         ret[3].xyz = lerp(a[3].xyz, b[3].xyz, t);
//!     }
//!     // Finally, do the same for the last column if perspective is enabled.
//!     if (perspective) {
//!         ret[0].w = lerp(a[0].w, b[0].w, t);
//!         ret[1].w = lerp(a[1].w, b[1].w, t);
//!         ret[2].w = lerp(a[2].w, b[2].w, t);
//!         ret[3].w = lerp(a[3].w, b[3].w, t);
//!     }
//!     return ret;
//! }
//!
//! // lines 26-33
//! hlslpp::float4x4 matrixScale(float scale) {
//!     hlslpp::float4x4 scaleMatrix(0.0f);
//!     scaleMatrix[0][0] = scale;
//!     scaleMatrix[1][1] = scale;
//!     scaleMatrix[2][2] = scale;
//!     scaleMatrix[3][3] = 1.0f;
//!     return scaleMatrix;
//! }
//!
//! // lines 35-42
//! hlslpp::float4x4 matrixScale(const hlslpp::float3& scale) {
//!     hlslpp::float4x4 scaleMatrix(0.0f);
//!     scaleMatrix[0][0] = scale.x;
//!     scaleMatrix[1][1] = scale.y;
//!     scaleMatrix[2][2] = scale.z;
//!     scaleMatrix[3][3] = 1.0f;
//!     return scaleMatrix;
//! }
//!
//! // lines 77-81
//! hlslpp::float4x4 matrixTranslation(const hlslpp::float3 &t) {
//!     hlslpp::float4x4 m = hlslpp::float4x4::identity();
//!     m[3].xyz = t;
//!     return m;
//! }
//!
//! // lines 83-92
//! hlslpp::float3x3 matrixRotationX(float rad) {
//!     hlslpp::float3x3 m = hlslpp::float3x3::identity();
//!     const float rollCos = cos(rad);
//!     const float rollSin = sin(rad);
//!     m[0][0] = rollCos;
//!     m[0][1] = -rollSin;
//!     m[1][0] = rollSin;
//!     m[1][1] = rollCos;
//!     return m;
//! }
//!
//! // lines 94-103
//! hlslpp::float3x3 matrixRotationY(float rad) {
//!     hlslpp::float3x3 m = hlslpp::float3x3::identity();
//!     const float pitchCos = cos(rad);
//!     const float pitchSin = sin(rad);
//!     m[0][0] = pitchCos;
//!     m[0][2] = pitchSin;
//!     m[2][0] = -pitchSin;
//!     m[2][2] = pitchCos;
//!     return m;
//! }
//!
//! // lines 105-114
//! hlslpp::float3x3 matrixRotationZ(float rad) {
//!     hlslpp::float3x3 m = hlslpp::float3x3::identity();
//!     const float yawCos = cos(rad);
//!     const float yawSin = sin(rad);
//!     m[1][1] = yawCos;
//!     m[1][2] = -yawSin;
//!     m[2][1] = yawSin;
//!     m[2][2] = yawCos;
//!     return m;
//! }
//! ```
//!
//! **Reuse, not new type.** This module reuses
//! [`fn64_render_ir::{Mat4, Vec4}`](fn64_render_ir) directly, matching
//! `rt64_math.rs`'s established convention exactly: `Mat4` is "a
//! backend-neutral **row-major** 4x4 float matrix, matching HLSL
//! `float4x4`" with `rows[i]` = row `i`, `rows[i].x/y/z/w` = that row's
//! four columns (`rsp_math.rs:78-84`), so an HLSL `m[i][j]` read becomes
//! `m.rows[i].{x,y,z,w}` for `j = 0..3`. `extract3x3` and
//! `rotation_from_3x3` reuse the sibling `rt64_math` module's already-`pub`
//! `Mat3` type (a local row-major 3x3 type built from
//! `fn64_render_ir::Vec3`, `rt64_math.rs:143-145`) rather than inventing a
//! second 3x3 type, since `rt64_math.rs` already established `Mat3` for
//! this exact upper-left-3x3 shape (`trace_from_3x3`) and this module is
//! that same source file's continuation. This is a read-only, same-crate
//! `crate::rt64_math::Mat3` reference -- it does not edit `rt64_math.rs`
//! (excluded from this ticket's edit set) and does not `pub use` it either;
//! `rt64_math_matrix` stays reachable only via `crate::rt64_math_matrix::*`,
//! same as every other unwired characterization module in this crate.
//!
//! The scalar `hlslpp::lerp` is **not** redefined here: it is
//! [`crate::rt64_math_decompose::lerp_f32`], widened to `pub(crate)` and
//! shared. Both modules port halves of the same `rt64_math.cpp` at the same
//! pinned commit and the same file SHA-256, so they cite one authority for
//! one formula (`a + t*(b-a)`, not `a*(1-t)+b*t` -- see "Admitted domain").
//! This module previously carried a character-identical private copy; the
//! duplicate was retired rather than left to drift apart.
//!
//! ## Admitted domain / unpinned HLSL semantics (bound, not invented)
//!
//! - **`hlslpp::normalize` in `rotation_from_3x3`**: `hlslpp` is an
//!   unpopulated submodule in every checkout available to this program
//!   (`rsp_math.rs:21-23`, `rt64_math.rs:95-101` document this same
//!   constraint for `hlslpp::any`/`isnan`) -- there is no vendored source to
//!   inspect for a possible platform-dependent divergence. This port
//!   assumes the conventional, universal `normalize(v) = v / length(v)`
//!   definition (an unguarded division -- see below), matching every known
//!   HLSL/SIMD-math-library convention, stated here as an assumption rather
//!   than a verified fact.
//! - **`rotation_from_3x3` at a zero-length row** (degenerate/non-invertible
//!   3x3 input): `normalize`'s unguarded `v / length(v)` yields component-wise
//!   `0.0 / 0.0 = NaN` for that row. This port lets plain IEEE-754 division
//!   propagate, matching `rt64_math.rs::barycentric_coordinates`'s and
//!   `depth_strict_less.rs`'s precedent of preserving unguarded upstream
//!   arithmetic rather than inventing a guard the source does not have.
//! - **`hlslpp::dot(hlslpp::abs(a[i]-b[i]), 1.0f)` in `matrix_difference`**:
//!   `dot` of a `float4` with a scalar literal broadcasts the scalar to
//!   `float4(1,1,1,1)` per HLSL's standard scalar-to-vector broadcast rule
//!   (an unpinned but conventional HLSL/hlslpp semantic, same
//!   unpopulated-submodule caveat as above), making `dot(v, 1.0f)` the sum
//!   of `v`'s four components. This port implements that sum directly
//!   (`v.x + v.y + v.z + v.w`) rather than route through a generic `dot`
//!   helper, since no `Vec4::dot` exists in `fn64_render_ir` (only
//!   `Vec3::dot`) and adding one for this single caller would be an
//!   unrequested `fn64-render-ir` API-surface widening outside this
//!   module's exclusive-paths boundary.
//! - **`matrix_difference`'s summation order**: preserved exactly as the
//!   source's row-major, left-to-right element order (`i=0..4`, each row's
//!   `x,y,z,w` in that order) -- float addition is not associative, so this
//!   port does not reassociate, reorder, or use a different reduction
//!   (e.g. pairwise/Kahan summation) than the source's straight linear
//!   accumulation.
//! - **`hlslpp::abs` on `f32`**: assumed to be plain `f32::abs()`
//!   (magnitude, sign-independent; `abs(-0.0) == 0.0`, `abs(NaN)` is
//!   `NaN` with an unspecified sign bit per IEEE-754) -- conventional, not
//!   contradicted by any hlslpp source citation in this repo, but likewise
//!   not a verified read.
//! - **`lerp_matrix`/`lerp_matrix3x3`/`lerp_matrix_components`'s lerp
//!   formula**: `hlslpp::lerp(a, b, t)` and the source's own bare `lerp(a,
//!   b, t)` calls (unqualified in `lerpMatrixComponents`, resolved via
//!   using-directive/ADL to the same `hlslpp::lerp`) are assumed to be the
//!   conventional `a + t * (b - a)` formula, matching HLSL's `lerp`
//!   intrinsic contract and `rsp_math.rs`'s established assumption-with-
//!   citation style for unpopulated-submodule hlslpp calls. This is **not**
//!   the same formula as `a*(1-t) + b*t` -- the two are algebraically equal
//!   in real-number arithmetic but differ in floating-point rounding and in
//!   `NaN`/infinity propagation at extreme `t`, so this port picks the
//!   `a + t*(b-a)` form specifically (HLSL's own documented definition) and
//!   does not use the other. At `t=0` and *finite* `a`/`b`, this yields
//!   exactly `a` (`a + 0*(b-a) = a`, since `0 * finite = 0`) -- but the
//!   formula is not special-cased to `return a` at `t=0`, so two corner
//!   cases genuinely diverge from "just `a`": (1) if `b-a` is itself
//!   infinite (e.g. `a=+inf, b=0.0`), `0 * (b-a) = 0 * -inf = NaN` (`0 *
//!   infinity` is `NaN` per IEEE-754), so the whole sum is `NaN`, not `a`;
//!   (2) if `a` is `-0.0` and `b` is `0.0`, `b-a = 0.0-(-0.0) = +0.0`,
//!   `0*+0.0 = +0.0`, and `-0.0 + +0.0 = +0.0` in round-to-nearest mode --
//!   so the result's sign bit flips from `a`'s `-0.0` to `+0.0`, not
//!   "exactly `a`" in the bit-identical sense. Both are exercised as
//!   characterization tests below, not asserted from this doc comment
//!   alone. At `t=1`, `a + 1*(b-a) = a + b - a`, which is `b` only when
//!   `a`'s cancellation is exact in `f32` -- for most inputs this is
//!   bit-identical to `b`, but is not a `t=1`-special-cased identity in the
//!   source and is not special-cased here either.
//! - **`lerp_matrix`/`lerp_matrix3x3` at out-of-`[0,1]` `t`**: no clamp in
//!   the source (`hlslpp::lerp`/HLSL `lerp` never clamps `t`) -- this port
//!   does not add one; `t<0` or `t>1` extrapolates linearly past `a`/`b` as
//!   plain arithmetic dictates.
//! - **`lerp_matrix_components`'s four independent boolean gates**: ported
//!   exactly as four independent `if`s over the same `ret` accumulator, in
//!   the source's exact order (angular-or-copy first, then linear, then
//!   perspective) -- not restructured into a single expression, since the
//!   gates read from `ret` (already possibly mutated by an earlier gate in
//!   the linear/perspective steps only through `a`/`b`, not `ret`, so gate
//!   order does not change the *result*, but this port preserves the
//!   source's exact statement order rather than relying on that
//!   independence argument to justify reordering).
//! - **`matrixRotationX`/`matrixRotationZ` are misnamed at the source, and
//!   the names are ported verbatim.** `matrixRotationX` writes the
//!   `[0][0],[0][1],[1][0],[1][1]` block -- the **XY** block, which is
//!   conventionally a rotation about **Z**. `matrixRotationZ` writes the
//!   `[1][1],[1][2],[2][1],[2][2]` block -- the **YZ** block, conventionally
//!   a rotation about **X**. Only `matrixRotationY` (the `XZ` block) matches
//!   its name. This port does **not** rename them to match the axis each
//!   actually rotates about: RT64's own `CameraController::rotatePerspective`
//!   (`rt64_camera_controller.cpp:53-63`) calls `matrixRotationY(yaw)` and
//!   `matrixRotationZ(pitch)` by these names, so "correcting" them would
//!   silently rewire every call site. The mismatch is documented here and
//!   pinned by a test rather than fixed.
//! - **`matrixRotationY`'s sign placement is mirrored relative to its two
//!   siblings, and is preserved exactly.** `X` and `Z` both put `-sin`
//!   *above* the diagonal and `+sin` below; `Y` puts `+sin` above
//!   (`m[0][2]`) and `-sin` below (`m[2][0]`). This is the standard
//!   right-handed-basis asymmetry for a Y rotation, it is what the source
//!   literally writes, and the two forms are **not** interchangeable -- a
//!   transposed or sign-flipped `Y` is a rotation by `-rad`, not `rad`.
//!   Each of the three sign conventions is pinned by its own explicit test.
//! - **`cos`/`sin` resolution and `f32` libm semantics**: `rt64_math.cpp`
//!   includes `<cmath>` and has no `using namespace hlslpp` directive, so
//!   the unqualified `cos(rad)`/`sin(rad)` on a `float` argument resolve to
//!   the `float` overloads (`cosf`/`sinf`), *not* to any `hlslpp` vector
//!   intrinsic -- these five helpers are open-coded scalar arithmetic on
//!   `hlslpp` *containers*, not calls into `hlslpp`'s unpopulated internals,
//!   which is why they are portable at all where the quaternion cluster's
//!   `hlslpp::` calls were not. Rust's `f32::cos`/`f32::sin` lower to the
//!   same platform libm. Verified on this host that both agree bit-for-bit
//!   with an independent computation at `0`, `f32::PI/2`, `f32::PI` and
//!   `0.5` -- but libm transcendentals are not bit-guaranteed *across*
//!   platforms by either language, so the exact-value tests below pin the
//!   values this host produces and are characterization, not a portability
//!   claim.
//! - **`cos(f32::FRAC_PI_2)` is not `0.0`.** It is `-4.371139e-8`
//!   (`0xb33bbd2e`), because `f32::FRAC_PI_2` is not exactly `π/2`.
//!   Likewise `sin(f32::PI)` is `-8.742278e-8` (`0xb3bbbd2e`), not `0.0`,
//!   and is *negative*. The tests below assert these real values rather
//!   than an idealized zero, and the `-sin` entries therefore carry the
//!   opposite sign from the naive expectation.
//! - **`-sin(0.0)` is `-0.0`, not `+0.0`.** The negation in the `-sin`
//!   slots is applied to a `+0.0` at `rad == 0`, yielding a negative zero
//!   in `m[0][1]` (X), `m[2][0]` (Y) and `m[1][2]` (Z) at the identity
//!   angle. `-0.0 == 0.0` compares true in IEEE-754, so this is invisible
//!   to an `assert_eq!` on the value; it is pinned explicitly by a
//!   `to_bits()` test instead, since it is a real bit-level difference from
//!   `hlslpp::float3x3::identity()`.
//! - **`matrixScale` starts from `float4x4(0.0f)`, not the identity.** The
//!   scalar and `float3` overloads both zero-initialize and then write only
//!   four elements, so `[3][3]` is explicitly set to `1.0f` while every
//!   other off-diagonal stays `0.0`. `matrixTranslation`, by contrast,
//!   starts from `identity()`. This port preserves that difference rather
//!   than unifying the two on one starting point.
//! - **`matrixTranslation` writes row 3, not column 3.** `m[3].xyz = t`
//!   indexes `m[3]` as a *row* under this codebase's fixed row-major
//!   reading of `float4x4` (`rsp_math.rs:78-84`), so the translation lands
//!   in `rows[3].{x,y,z}`. This is the row-vector convention (`v * M`), the
//!   transpose of the column-vector convention some other engines use --
//!   getting it backwards would transpose every camera transform, so it is
//!   pinned by a test that distinguishes the two.
//! - **`inverse4` is re-exported, not reimplemented.** `hlslpp::inverse` on
//!   a `float4x4` already has exactly one implementation in this crate:
//!   `rt64_math_decompose.rs`'s classical adjugate/cofactor `inverse4`,
//!   landed by another card. This module widens that function's visibility
//!   from private to `pub(crate)` -- a *visibility-only* change, with its
//!   signature, body, arithmetic and unguarded-singular-input behavior all
//!   untouched -- and re-exports it here as `inverse4`. The alternative,
//!   a second ~40-line cofactor inverse under a new name, would duplicate
//!   identical arithmetic and create two places for one formula to drift.
//!
//! ## Nonclaims
//!
//! No GPU, WGSL, camera/view-matrix wiring, or RT64 visual/pixel/silicon
//! parity or performance claim. This module is not called from anywhere
//! yet (dead-code warnings on the unused public surface are expected and
//! correct, matching `rt64_math.rs`'s and every other characterization-
//! first module's precedent). Does not port any other function from
//! `rt64_math.h`/`.cpp` beyond the six named above -- in particular, the
//! quaternion cluster `decomposeMatrix`, `recomposeMatrix`,
//! `DecomposedTransform` (and its constructor), and `lerpTransforms`, plus
//! their `vecCombine`/`vecScale` helpers, are a separate ticket (owned by
//! another executor) and are deliberately NOT ported here: they need a
//! `hlslpp::quaternion`-equivalent type that this module does not add.
//! `matrixDecomposeViewProj` and `pseudoRandom` remain out of scope exactly
//! as `rt64_math.rs` already stated.
//!
//! **`rt64_math.rs`'s Nonclaims list is now stale** and needs a follow-up
//! edit outside this module's exclusive paths: it still reads "Does not
//! port `matrixScale`, `matrixTranslation`, `matrixRotationX/Y/Z`,
//! `extract3x3`, `rotationFrom3x3`, ... (deferred -- needs new
//! matrix-inverse/quaternion infra)", but all of those except the
//! quaternion cluster are now ported here. The stated reason is also no
//! longer accurate for them: none of these five needs quaternion infra, and
//! the matrix-inverse infra they were waiting on exists and is now
//! `pub(crate)`.
//!
//! This module's `matrix_scale_vec3` and `matrix_translation` intentionally
//! duplicate, in *shape*, two private recompose-only helpers of the same
//! names inside `rt64_math_decompose.rs`. Those are explicitly documented
//! there as "minimal recompose-only helpers, not a claim of porting
//! `matrixTranslation`/`matrixScale(float3)` as a reusable public API for
//! other callers"; this module makes that public-API claim. They are not
//! consolidated because doing so would edit another card's landed module
//! *behaviorally* (rerouting its call sites) rather than by visibility
//! alone -- flagged here as a known, deliberate redundancy for a later
//! consolidation pass, not an oversight.

use crate::rt64_math::Mat3;
use crate::rt64_math_decompose::lerp_f32;
use fn64_render_ir::Vec3;

/// `extract3x3`: copies the upper-left 3x3 of a row-major `float4x4`,
/// element-for-element, into the sibling `rt64_math` module's `Mat3` (see
/// module doc "Reuse, not new type").
pub fn extract_3x3(m: fn64_render_ir::Mat4) -> Mat3 {
    Mat3 {
        rows: [
            Vec3::new(m.rows[0].x, m.rows[0].y, m.rows[0].z),
            Vec3::new(m.rows[1].x, m.rows[1].y, m.rows[1].z),
            Vec3::new(m.rows[2].x, m.rows[2].y, m.rows[2].z),
        ],
    }
}

/// `rotationFrom3x3`: normalizes each row of a 3x3 matrix independently.
/// Unguarded division (`normalize(v) = v / length(v)`); a zero-length row
/// yields `NaN` lanes, propagated per IEEE-754 (see module doc "Admitted
/// domain").
pub fn rotation_from_3x3(m: Mat3) -> Mat3 {
    Mat3 {
        rows: [
            normalize_vec3(m.rows[0]),
            normalize_vec3(m.rows[1]),
            normalize_vec3(m.rows[2]),
        ],
    }
}

fn normalize_vec3(v: Vec3) -> Vec3 {
    let len = (v.x * v.x + v.y * v.y + v.z * v.z).sqrt();
    Vec3::new(v.x / len, v.y / len, v.z / len)
}

/// `matrixDifference`: sum over all 16 elements of `|a[i][j] - b[i][j]|`,
/// accumulated row-major, left-to-right, in the source's exact order (see
/// module doc "Admitted domain" -- float addition is not associative).
pub fn matrix_difference(a: fn64_render_ir::Mat4, b: fn64_render_ir::Mat4) -> f32 {
    let mut difference = 0.0f32;
    for i in 0..4 {
        let ar = a.rows[i];
        let br = b.rows[i];
        let d = fn64_render_ir::Vec4::new(
            (ar.x - br.x).abs(),
            (ar.y - br.y).abs(),
            (ar.z - br.z).abs(),
            (ar.w - br.w).abs(),
        );
        difference += d.x + d.y + d.z + d.w;
    }
    difference
}

/// `lerpMatrix`: component-wise linear interpolation of every row,
/// `a + t*(b-a)` per element (see module doc "Admitted domain" for the
/// exact formula and its `t=0`/`t=1`/out-of-range behavior).
pub fn lerp_matrix(
    a: fn64_render_ir::Mat4,
    b: fn64_render_ir::Mat4,
    t: f32,
) -> fn64_render_ir::Mat4 {
    let mut c = b;
    for i in 0..4 {
        c.rows[i] = lerp_vec4(a.rows[i], b.rows[i], t);
    }
    c
}

/// `lerpMatrix3x3`: lerps only the upper-left 3x3 (rows 0-2's `x,y,z`);
/// row 3 and every row's `w` column are copied verbatim from `b`.
pub fn lerp_matrix_3x3(
    a: fn64_render_ir::Mat4,
    b: fn64_render_ir::Mat4,
    t: f32,
) -> fn64_render_ir::Mat4 {
    let mut c = b;
    for i in 0..3 {
        let av = a.rows[i];
        let bv = b.rows[i];
        c.rows[i] = fn64_render_ir::Vec4::new(
            lerp_f32(av.x, bv.x, t),
            lerp_f32(av.y, bv.y, t),
            lerp_f32(av.z, bv.z, t),
            bv.w,
        );
    }
    c
}

/// `lerpMatrixComponents`: gated composition of `lerp_matrix_3x3` (or a
/// plain copy of `b`) for the rotation/scale block, an independent lerp of
/// row 3's `xyz` translation, and an independent lerp of every row's `w`
/// perspective column -- each gate applied only if its flag is set,
/// otherwise leaving that part as `b`'s value from the initial copy step
/// (see module doc "Admitted domain" for the exact gate order preserved).
#[allow(clippy::too_many_arguments)]
pub fn lerp_matrix_components(
    a: fn64_render_ir::Mat4,
    b: fn64_render_ir::Mat4,
    linear: bool,
    angular: bool,
    perspective: bool,
    t: f32,
) -> fn64_render_ir::Mat4 {
    let mut ret = if angular { lerp_matrix_3x3(a, b, t) } else { b };

    if linear {
        let av = a.rows[3];
        let bv = b.rows[3];
        ret.rows[3] = fn64_render_ir::Vec4::new(
            lerp_f32(av.x, bv.x, t),
            lerp_f32(av.y, bv.y, t),
            lerp_f32(av.z, bv.z, t),
            ret.rows[3].w,
        );
    }

    if perspective {
        for i in 0..4 {
            let aw = a.rows[i].w;
            let bw = b.rows[i].w;
            ret.rows[i].w = lerp_f32(aw, bw, t);
        }
    }

    ret
}

/// `hlslpp::lerp(a, b, t)` applied component-wise to a `float4`.
fn lerp_vec4(a: fn64_render_ir::Vec4, b: fn64_render_ir::Vec4, t: f32) -> fn64_render_ir::Vec4 {
    fn64_render_ir::Vec4::new(
        lerp_f32(a.x, b.x, t),
        lerp_f32(a.y, b.y, t),
        lerp_f32(a.z, b.z, t),
        lerp_f32(a.w, b.w, t),
    )
}

/// The row-major 4x4 identity, matching `hlslpp::float4x4::identity()` (see
/// `rt64_math.rs`'s same assumption-with-citation for that call).
fn mat4_identity() -> fn64_render_ir::Mat4 {
    fn64_render_ir::Mat4::from_rows([
        fn64_render_ir::Vec4::new(1.0, 0.0, 0.0, 0.0),
        fn64_render_ir::Vec4::new(0.0, 1.0, 0.0, 0.0),
        fn64_render_ir::Vec4::new(0.0, 0.0, 1.0, 0.0),
        fn64_render_ir::Vec4::new(0.0, 0.0, 0.0, 1.0),
    ])
}

/// `matrixScale(float)`: a uniform-scale matrix built from an all-zero
/// `float4x4`, with `scale` on the first three diagonal entries and a literal
/// `1.0f` at `[3][3]`. Every off-diagonal entry stays `0.0` from the
/// zero-initialized start -- the source does **not** begin from the identity
/// here (unlike `matrixTranslation`), so `[3][3]` is the only element written
/// to `1.0`.
pub fn matrix_scale(scale: f32) -> fn64_render_ir::Mat4 {
    let mut scale_matrix =
        fn64_render_ir::Mat4::from_rows([fn64_render_ir::Vec4::new(0.0, 0.0, 0.0, 0.0); 4]);
    scale_matrix.rows[0].x = scale;
    scale_matrix.rows[1].y = scale;
    scale_matrix.rows[2].z = scale;
    scale_matrix.rows[3].w = 1.0;
    scale_matrix
}

/// `matrixScale(const float3&)`: the non-uniform overload of the above,
/// taking `scale.x/y/z` for the three diagonal entries. Same zero-initialized
/// start and same literal `1.0f` at `[3][3]`.
pub fn matrix_scale_vec3(scale: Vec3) -> fn64_render_ir::Mat4 {
    let mut scale_matrix =
        fn64_render_ir::Mat4::from_rows([fn64_render_ir::Vec4::new(0.0, 0.0, 0.0, 0.0); 4]);
    scale_matrix.rows[0].x = scale.x;
    scale_matrix.rows[1].y = scale.y;
    scale_matrix.rows[2].z = scale.z;
    scale_matrix.rows[3].w = 1.0;
    scale_matrix
}

/// `matrixTranslation`: the identity with `t` written into **row 3**'s
/// `xyz` (`m[3].xyz = t`), leaving `[3][3]` at the identity's `1.0`. Row 3 --
/// not column 3 -- because `Mat4` is row-major and the source indexes
/// `m[3]` as a row (see module doc "Reuse, not new type").
pub fn matrix_translation(t: Vec3) -> fn64_render_ir::Mat4 {
    let mut m = mat4_identity();
    m.rows[3].x = t.x;
    m.rows[3].y = t.y;
    m.rows[3].z = t.z;
    m
}

/// `matrixRotationX`: the 3x3 identity with `cos`/`sin` written into the
/// **`[0][0]`,`[0][1]`,`[1][0]`,`[1][1]` block** -- i.e. the XY block, which
/// by the usual mathematical convention is a rotation about *Z*, not X.
/// RT64's name is ported verbatim rather than corrected: `rotatePerspective`
/// (`rt64_camera_controller.cpp:53-63`) calls these by RT64's names, so
/// renaming them to match convention would silently break every call site.
/// Sign convention, exactly as the source writes it: `-sin` above the
/// diagonal at `[0][1]`, `+sin` below at `[1][0]`. `[2][2]` stays `1.0`
/// from the identity.
pub fn matrix_rotation_x(rad: f32) -> Mat3 {
    let mut m = mat3_identity();
    let roll_cos = rad.cos();
    let roll_sin = rad.sin();
    m.rows[0].x = roll_cos;
    m.rows[0].y = -roll_sin;
    m.rows[1].x = roll_sin;
    m.rows[1].y = roll_cos;
    m
}

/// `matrixRotationY`: the 3x3 identity with `cos`/`sin` in the
/// **`[0][0]`,`[0][2]`,`[2][0]`,`[2][2]` block** (the XZ block -- this one
/// *does* match the conventional Y-axis rotation). Its sign convention is
/// **mirrored** relative to `matrix_rotation_x`/`matrix_rotation_z`:
/// `+sin` above the diagonal at `[0][2]` and `-sin` below at `[2][0]`, the
/// opposite placement from the other two. That asymmetry is the source's,
/// is standard for a Y rotation in a right-handed basis, and is preserved
/// literally -- it is *not* normalized to match its siblings. `[1][1]`
/// stays `1.0` from the identity.
pub fn matrix_rotation_y(rad: f32) -> Mat3 {
    let mut m = mat3_identity();
    let pitch_cos = rad.cos();
    let pitch_sin = rad.sin();
    m.rows[0].x = pitch_cos;
    m.rows[0].z = pitch_sin;
    m.rows[2].x = -pitch_sin;
    m.rows[2].z = pitch_cos;
    m
}

/// `matrixRotationZ`: the 3x3 identity with `cos`/`sin` in the
/// **`[1][1]`,`[1][2]`,`[2][1]`,`[2][2]` block** -- the YZ block, which by
/// the usual convention is a rotation about *X*, not Z. Same verbatim-name
/// rationale as `matrix_rotation_x`. Sign convention: `-sin` above the
/// diagonal at `[1][2]`, `+sin` below at `[2][1]`. `[0][0]` stays `1.0`
/// from the identity.
pub fn matrix_rotation_z(rad: f32) -> Mat3 {
    let mut m = mat3_identity();
    let yaw_cos = rad.cos();
    let yaw_sin = rad.sin();
    m.rows[1].y = yaw_cos;
    m.rows[1].z = -yaw_sin;
    m.rows[2].y = yaw_sin;
    m.rows[2].z = yaw_cos;
    m
}

/// The row-major 3x3 identity, matching `hlslpp::float3x3::identity()`.
fn mat3_identity() -> Mat3 {
    Mat3 {
        rows: [
            Vec3::new(1.0, 0.0, 0.0),
            Vec3::new(0.0, 1.0, 0.0),
            Vec3::new(0.0, 0.0, 1.0),
        ],
    }
}

/// `hlslpp::inverse(float4x4)`: re-exported from `rt64_math_decompose.rs`'s
/// already-landed classical adjugate/cofactor inverse rather than
/// reimplemented. RT64's camera controller terminates all three of its
/// perspective methods in `hlslpp::inverse`
/// (`rt64_camera_controller.cpp:50,62,74`); this crate already has exactly
/// one 4x4 inverse, and a second would be a ~40-line duplicate of identical
/// arithmetic under a different name. See module doc "Reuse, not new type".
pub fn inverse4(m: fn64_render_ir::Mat4) -> fn64_render_ir::Mat4 {
    crate::rt64_math_decompose::inverse4(m)
}

#[cfg(test)]
mod tests {
    use super::*;
    use fn64_render_ir::{Mat4, Vec4};

    fn identity() -> Mat4 {
        Mat4::from_rows([
            Vec4::new(1.0, 0.0, 0.0, 0.0),
            Vec4::new(0.0, 1.0, 0.0, 0.0),
            Vec4::new(0.0, 0.0, 1.0, 0.0),
            Vec4::new(0.0, 0.0, 0.0, 1.0),
        ])
    }

    fn zeros() -> Mat4 {
        Mat4::from_rows([Vec4::new(0.0, 0.0, 0.0, 0.0); 4])
    }

    fn arbitrary_a() -> Mat4 {
        Mat4::from_rows([
            Vec4::new(1.0, 2.0, 3.0, 4.0),
            Vec4::new(5.0, 6.0, 7.0, 8.0),
            Vec4::new(9.0, 10.0, 11.0, 12.0),
            Vec4::new(13.0, 14.0, 15.0, 16.0),
        ])
    }

    fn arbitrary_b() -> Mat4 {
        Mat4::from_rows([
            Vec4::new(-1.0, 0.0, 2.0, 4.0),
            Vec4::new(10.0, -6.0, 7.5, 0.0),
            Vec4::new(9.0, 20.0, -11.0, 12.5),
            Vec4::new(-13.0, 14.0, 0.0, 100.0),
        ])
    }

    // --- extract_3x3 ---

    #[test]
    fn extract_3x3_identity_is_3x3_identity() {
        let e = extract_3x3(identity());
        assert_eq!(e.rows[0], Vec3::new(1.0, 0.0, 0.0));
        assert_eq!(e.rows[1], Vec3::new(0.0, 1.0, 0.0));
        assert_eq!(e.rows[2], Vec3::new(0.0, 0.0, 1.0));
    }

    #[test]
    fn extract_3x3_drops_last_row_and_column() {
        let m = arbitrary_a();
        let e = extract_3x3(m);
        assert_eq!(e.rows[0], Vec3::new(1.0, 2.0, 3.0));
        assert_eq!(e.rows[1], Vec3::new(5.0, 6.0, 7.0));
        assert_eq!(e.rows[2], Vec3::new(9.0, 10.0, 11.0));
        // Row 3 and column 3 (the 4.0/8.0/12.0/13..16 values) do not appear.
    }

    #[test]
    fn extract_3x3_zero_matrix_is_zero() {
        let e = extract_3x3(zeros());
        for row in e.rows {
            assert_eq!(row, Vec3::new(0.0, 0.0, 0.0));
        }
    }

    #[test]
    fn extract_3x3_passes_through_nan_and_infinity_unmodified() {
        let m = Mat4::from_rows([
            Vec4::new(f32::NAN, f32::INFINITY, f32::NEG_INFINITY, 999.0),
            Vec4::new(0.0, 0.0, 0.0, 0.0),
            Vec4::new(0.0, 0.0, 0.0, 0.0),
            Vec4::new(0.0, 0.0, 0.0, 0.0),
        ]);
        let e = extract_3x3(m);
        assert!(e.rows[0].x.is_nan());
        assert_eq!(e.rows[0].y, f32::INFINITY);
        assert_eq!(e.rows[0].z, f32::NEG_INFINITY);
    }

    // --- rotation_from_3x3 ---

    #[test]
    fn rotation_from_3x3_identity_rows_are_already_unit_length() {
        let m = Mat3 {
            rows: [
                Vec3::new(1.0, 0.0, 0.0),
                Vec3::new(0.0, 1.0, 0.0),
                Vec3::new(0.0, 0.0, 1.0),
            ],
        };
        let r = rotation_from_3x3(m);
        assert_eq!(r.rows[0], Vec3::new(1.0, 0.0, 0.0));
        assert_eq!(r.rows[1], Vec3::new(0.0, 1.0, 0.0));
        assert_eq!(r.rows[2], Vec3::new(0.0, 0.0, 1.0));
    }

    #[test]
    fn rotation_from_3x3_scales_each_row_to_unit_length() {
        let m = Mat3 {
            rows: [
                Vec3::new(2.0, 0.0, 0.0),
                Vec3::new(0.0, 0.0, 5.0),
                Vec3::new(3.0, 4.0, 0.0),
            ],
        };
        let r = rotation_from_3x3(m);
        assert_eq!(r.rows[0], Vec3::new(1.0, 0.0, 0.0));
        assert_eq!(r.rows[1], Vec3::new(0.0, 0.0, 1.0));
        assert!((r.rows[2].x - 0.6).abs() < 1e-6);
        assert!((r.rows[2].y - 0.8).abs() < 1e-6);
        assert_eq!(r.rows[2].z, 0.0);
    }

    #[test]
    fn rotation_from_3x3_negative_components_preserve_sign_after_normalize() {
        let m = Mat3 {
            rows: [
                Vec3::new(-3.0, 4.0, 0.0),
                Vec3::new(0.0, 1.0, 0.0),
                Vec3::new(0.0, 0.0, 1.0),
            ],
        };
        let r = rotation_from_3x3(m);
        assert!((r.rows[0].x - (-0.6)).abs() < 1e-6);
        assert!((r.rows[0].y - 0.8).abs() < 1e-6);
    }

    #[test]
    fn rotation_from_3x3_zero_row_yields_nan_unguarded() {
        let m = Mat3 {
            rows: [
                Vec3::new(0.0, 0.0, 0.0),
                Vec3::new(1.0, 0.0, 0.0),
                Vec3::new(0.0, 1.0, 0.0),
            ],
        };
        let r = rotation_from_3x3(m);
        assert!(r.rows[0].x.is_nan());
        assert!(r.rows[0].y.is_nan());
        assert!(r.rows[0].z.is_nan());
        // Other rows are unaffected.
        assert_eq!(r.rows[1], Vec3::new(1.0, 0.0, 0.0));
        assert_eq!(r.rows[2], Vec3::new(0.0, 1.0, 0.0));
    }

    #[test]
    fn rotation_from_3x3_infinite_component_yields_nan_via_inf_over_inf() {
        // length(inf,0,0) = sqrt(inf^2) = inf; inf/inf = NaN for the x
        // lane, 0/inf = 0.0 for the others -- unguarded, no special case.
        let m = Mat3 {
            rows: [
                Vec3::new(f32::INFINITY, 0.0, 0.0),
                Vec3::new(1.0, 0.0, 0.0),
                Vec3::new(0.0, 1.0, 0.0),
            ],
        };
        let r = rotation_from_3x3(m);
        assert!(r.rows[0].x.is_nan());
        assert_eq!(r.rows[0].y, 0.0);
        assert_eq!(r.rows[0].z, 0.0);
    }

    // --- matrix_difference ---

    #[test]
    fn matrix_difference_identical_matrices_is_zero() {
        let m = arbitrary_a();
        assert_eq!(matrix_difference(m, m), 0.0);
    }

    #[test]
    fn matrix_difference_zero_vs_identity_sums_the_four_diagonal_ones() {
        assert_eq!(matrix_difference(zeros(), identity()), 4.0);
    }

    #[test]
    fn matrix_difference_is_symmetric_under_swap() {
        let a = arbitrary_a();
        let b = arbitrary_b();
        assert_eq!(matrix_difference(a, b), matrix_difference(b, a));
    }

    #[test]
    fn matrix_difference_exact_value_hand_computed() {
        // Row-by-row |a-b| sums, hand-computed independently of the port:
        // row0: |1-(-1)|+|2-0|+|3-2|+|4-4| = 2+2+1+0 = 5
        // row1: |5-10|+|6-(-6)|+|7-7.5|+|8-0| = 5+12+0.5+8 = 25.5
        // row2: |9-9|+|10-20|+|11-(-11)|+|12-12.5| = 0+10+22+0.5 = 32.5
        // row3: |13-(-13)|+|14-14|+|15-0|+|16-100| = 26+0+15+84 = 125
        // total = 5 + 25.5 + 32.5 + 125 = 188.0
        let d = matrix_difference(arbitrary_a(), arbitrary_b());
        assert!((d - 188.0).abs() < 1e-3, "d={d}");
    }

    #[test]
    fn matrix_difference_negative_zero_abs_is_zero() {
        let a = Mat4::from_rows([Vec4::new(-0.0, 0.0, 0.0, 0.0); 4]);
        let b = zeros();
        assert_eq!(matrix_difference(a, b), 0.0);
    }

    #[test]
    fn matrix_difference_nan_propagates() {
        let mut a = zeros();
        a.rows[0].x = f32::NAN;
        assert!(matrix_difference(a, zeros()).is_nan());
    }

    #[test]
    fn matrix_difference_infinite_element_yields_infinite_sum() {
        let mut a = zeros();
        a.rows[0].x = f32::INFINITY;
        assert_eq!(matrix_difference(a, zeros()), f32::INFINITY);
    }

    #[test]
    fn matrix_difference_opposite_infinities_still_yields_infinite_abs_diff() {
        // |(+inf) - (-inf)| = |+inf| = +inf, not NaN: unlike same-sign
        // inf-inf, opposite-sign infinities subtract to a well-defined
        // infinity here.
        let mut a = zeros();
        a.rows[0].x = f32::INFINITY;
        let mut b = zeros();
        b.rows[0].x = f32::NEG_INFINITY;
        assert_eq!(matrix_difference(a, b), f32::INFINITY);
    }

    // --- lerp_matrix ---

    #[test]
    fn lerp_matrix_at_t_zero_is_a() {
        let a = arbitrary_a();
        let b = arbitrary_b();
        assert_eq!(lerp_matrix(a, b, 0.0), a);
    }

    #[test]
    fn lerp_matrix_at_t_one_is_b() {
        let a = arbitrary_a();
        let b = arbitrary_b();
        let r = lerp_matrix(a, b, 1.0);
        for i in 0..4 {
            let rr = r.rows[i];
            let br = b.rows[i];
            assert!((rr.x - br.x).abs() < 1e-4);
            assert!((rr.y - br.y).abs() < 1e-4);
            assert!((rr.z - br.z).abs() < 1e-4);
            assert!((rr.w - br.w).abs() < 1e-4);
        }
    }

    #[test]
    fn lerp_matrix_at_t_half_is_the_midpoint() {
        let a = identity();
        let b = zeros();
        let r = lerp_matrix(a, b, 0.5);
        // identity[0][0]=1 -> lerp(1,0,0.5) = 0.5.
        assert_eq!(r.rows[0].x, 0.5);
        assert_eq!(r.rows[1].y, 0.5);
        assert_eq!(r.rows[2].z, 0.5);
        assert_eq!(r.rows[3].w, 0.5);
        // Off-diagonal: lerp(0,0,0.5) = 0.
        assert_eq!(r.rows[0].y, 0.0);
    }

    #[test]
    fn lerp_matrix_t_outside_unit_interval_extrapolates() {
        let a = Mat4::from_rows([Vec4::new(0.0, 0.0, 0.0, 0.0); 4]);
        let b = Mat4::from_rows([Vec4::new(1.0, 0.0, 0.0, 0.0); 4]);
        // lerp(0, 1, 2.0) = 0 + 2*(1-0) = 2.0 -- past b.
        let r = lerp_matrix(a, b, 2.0);
        assert_eq!(r.rows[0].x, 2.0);
        // lerp(0, 1, -1.0) = 0 + (-1)*(1-0) = -1.0 -- before a.
        let r2 = lerp_matrix(a, b, -1.0);
        assert_eq!(r2.rows[0].x, -1.0);
    }

    #[test]
    fn lerp_matrix_exact_value_hand_computed_at_quarter() {
        // a=2.0, b=10.0, t=0.25: 2.0 + 0.25*(10.0-2.0) = 2.0 + 2.0 = 4.0.
        let a = Mat4::from_rows([Vec4::new(2.0, 0.0, 0.0, 0.0); 4]);
        let b = Mat4::from_rows([Vec4::new(10.0, 0.0, 0.0, 0.0); 4]);
        let r = lerp_matrix(a, b, 0.25);
        assert_eq!(r.rows[0].x, 4.0);
    }

    #[test]
    fn lerp_matrix_nan_in_a_propagates_even_at_t_zero() {
        // a + t*(b-a) with t=0 still multiplies 0 * (b-a); if a itself is
        // NaN the whole expression is NaN even though "conceptually" t=0
        // should just return a. This is the literal formula's behavior,
        // not a bug (see module doc).
        let mut a = zeros();
        a.rows[0].x = f32::NAN;
        let b = zeros();
        let r = lerp_matrix(a, b, 0.0);
        assert!(r.rows[0].x.is_nan());
    }

    #[test]
    fn lerp_matrix_nan_t_propagates_to_every_lerped_element() {
        let a = arbitrary_a();
        let b = arbitrary_b();
        let r = lerp_matrix(a, b, f32::NAN);
        for row in r.rows {
            assert!(row.x.is_nan());
            assert!(row.y.is_nan());
            assert!(row.z.is_nan());
            assert!(row.w.is_nan());
        }
    }

    #[test]
    fn lerp_matrix_infinite_a_at_t_zero_yields_nan_not_a_shortcut() {
        // a=+inf, b=0.0, t=0.0: a + t*(b-a) = inf + 0*(0-inf) =
        // inf + 0*(-inf) = inf + NaN = NaN (0 * infinity is NaN per
        // IEEE-754). The literal `a + t*(b-a)` formula does NOT
        // special-case t=0 to just return `a` -- this is the formula's
        // genuine behavior at this input, not a bug (see module doc).
        let a = Mat4::from_rows([Vec4::new(f32::INFINITY, 0.0, 0.0, 0.0); 4]);
        let b = zeros();
        let r = lerp_matrix(a, b, 0.0);
        assert!(r.rows[0].x.is_nan());
    }

    #[test]
    fn lerp_matrix_infinity_minus_infinity_yields_nan() {
        // a=+inf, b=+inf, t=0.5: a + 0.5*(b-a) = inf + 0.5*(inf-inf) =
        // inf + 0.5*NaN = NaN. Not a special-cased "same value" shortcut.
        let a = Mat4::from_rows([Vec4::new(f32::INFINITY, 0.0, 0.0, 0.0); 4]);
        let b = Mat4::from_rows([Vec4::new(f32::INFINITY, 0.0, 0.0, 0.0); 4]);
        let r = lerp_matrix(a, b, 0.5);
        assert!(r.rows[0].x.is_nan());
    }

    #[test]
    fn lerp_matrix_negative_zero_at_t_zero_flips_to_positive_zero() {
        // a=-0.0, b=0.0, t=0.0: b-a = 0.0-(-0.0) = +0.0 (IEEE-754: this
        // subtraction is exact and positive), t*(b-a) = 0*+0.0 = +0.0,
        // a+0.0 = -0.0+0.0 = +0.0 (IEEE-754 default rounding: x+0.0 with
        // x=-0.0 and the addend +0.0 produces +0.0, not -0.0 -- the sign
        // of a zero result from adding two zeros of opposite sign is
        // always +0.0 in round-to-nearest mode). So the formula does NOT
        // preserve a's negative-zero sign at t=0 -- another instance of
        // `a + t*(b-a)` not degenerating to a bare copy of `a` (see module
        // doc).
        let a = Mat4::from_rows([Vec4::new(-0.0, 0.0, 0.0, 0.0); 4]);
        let b = zeros();
        let r = lerp_matrix(a, b, 0.0);
        assert_eq!(r.rows[0].x, 0.0);
        assert!(r.rows[0].x.is_sign_positive());
    }

    // --- lerp_matrix_3x3 ---

    #[test]
    fn lerp_matrix_3x3_row_3_always_copies_b_regardless_of_t() {
        let a = arbitrary_a();
        let b = arbitrary_b();
        let r = lerp_matrix_3x3(a, b, 0.0);
        assert_eq!(r.rows[3], b.rows[3]);
        let r2 = lerp_matrix_3x3(a, b, 1.0);
        assert_eq!(r2.rows[3], b.rows[3]);
    }

    #[test]
    fn lerp_matrix_3x3_w_column_always_copies_b_regardless_of_t() {
        let a = arbitrary_a();
        let b = arbitrary_b();
        let r = lerp_matrix_3x3(a, b, 0.0);
        assert_eq!(r.rows[0].w, b.rows[0].w);
        assert_eq!(r.rows[1].w, b.rows[1].w);
        assert_eq!(r.rows[2].w, b.rows[2].w);
    }

    #[test]
    fn lerp_matrix_3x3_upper_left_lerps_at_t_zero_is_a() {
        let a = arbitrary_a();
        let b = arbitrary_b();
        let r = lerp_matrix_3x3(a, b, 0.0);
        assert_eq!(r.rows[0].x, a.rows[0].x);
        assert_eq!(r.rows[0].y, a.rows[0].y);
        assert_eq!(r.rows[0].z, a.rows[0].z);
        assert_eq!(r.rows[2].z, a.rows[2].z);
    }

    #[test]
    fn lerp_matrix_3x3_upper_left_lerps_at_t_half() {
        let a = Mat4::from_rows([Vec4::new(0.0, 0.0, 0.0, 99.0); 4]);
        let b = Mat4::from_rows([Vec4::new(10.0, 10.0, 10.0, 5.0); 4]);
        let r = lerp_matrix_3x3(a, b, 0.5);
        assert_eq!(r.rows[0].x, 5.0);
        assert_eq!(r.rows[0].y, 5.0);
        assert_eq!(r.rows[0].z, 5.0);
        // w column is b's regardless: 5.0.
        assert_eq!(r.rows[0].w, 5.0);
        // Row 3 is fully b's: (10,10,10,5).
        assert_eq!(r.rows[3], Vec4::new(10.0, 10.0, 10.0, 5.0));
    }

    #[test]
    fn lerp_matrix_3x3_nan_in_upper_left_does_not_contaminate_row_3_or_w() {
        let mut a = arbitrary_a();
        a.rows[0].x = f32::NAN;
        let b = arbitrary_b();
        let r = lerp_matrix_3x3(a, b, 0.5);
        assert!(r.rows[0].x.is_nan());
        // row 3 and the w column are plain copies of b, untouched by a's NaN.
        assert_eq!(r.rows[3], b.rows[3]);
        assert_eq!(r.rows[1].w, b.rows[1].w);
    }

    // --- lerp_matrix_components ---

    #[test]
    fn lerp_matrix_components_all_flags_false_is_exactly_b() {
        let a = arbitrary_a();
        let b = arbitrary_b();
        let r = lerp_matrix_components(a, b, false, false, false, 0.5);
        assert_eq!(r, b);
    }

    #[test]
    fn lerp_matrix_components_angular_only_matches_lerp_matrix_3x3() {
        let a = arbitrary_a();
        let b = arbitrary_b();
        let r = lerp_matrix_components(a, b, false, true, false, 0.5);
        let expected = lerp_matrix_3x3(a, b, 0.5);
        assert_eq!(r, expected);
    }

    #[test]
    fn lerp_matrix_components_linear_only_lerps_row_3_xyz_leaves_rest_as_b() {
        let a = arbitrary_a();
        let b = arbitrary_b();
        let r = lerp_matrix_components(a, b, true, false, false, 0.5);
        // rows 0-2 untouched (angular=false -> ret starts as b).
        assert_eq!(r.rows[0], b.rows[0]);
        assert_eq!(r.rows[1], b.rows[1]);
        assert_eq!(r.rows[2], b.rows[2]);
        // row 3's xyz is lerped; w stays b's (only linear touches xyz).
        let expected_x = a.rows[3].x + 0.5 * (b.rows[3].x - a.rows[3].x);
        assert!((r.rows[3].x - expected_x).abs() < 1e-4);
        assert_eq!(r.rows[3].w, b.rows[3].w);
    }

    #[test]
    fn lerp_matrix_components_perspective_only_lerps_w_column_leaves_rest_as_b() {
        let a = arbitrary_a();
        let b = arbitrary_b();
        let r = lerp_matrix_components(a, b, false, false, true, 0.5);
        // xyz of every row untouched (still b's, since angular=false and
        // linear=false leave rows 0-2 and row 3's xyz as b's).
        assert_eq!(r.rows[0].x, b.rows[0].x);
        assert_eq!(r.rows[3].x, b.rows[3].x);
        // w column of every row is lerped.
        for i in 0..4 {
            let expected_w = a.rows[i].w + 0.5 * (b.rows[i].w - a.rows[i].w);
            assert!((r.rows[i].w - expected_w).abs() < 1e-4, "row {i}");
        }
    }

    #[test]
    fn lerp_matrix_components_all_flags_true_at_t_zero_is_a() {
        let a = arbitrary_a();
        let b = arbitrary_b();
        let r = lerp_matrix_components(a, b, true, true, true, 0.0);
        for i in 0..4 {
            let rr = r.rows[i];
            let ar = a.rows[i];
            assert!((rr.x - ar.x).abs() < 1e-4, "row {i} x");
            assert!((rr.y - ar.y).abs() < 1e-4, "row {i} y");
            assert!((rr.z - ar.z).abs() < 1e-4, "row {i} z");
            assert!((rr.w - ar.w).abs() < 1e-4, "row {i} w");
        }
    }

    #[test]
    fn lerp_matrix_components_all_flags_true_at_t_one_is_b() {
        let a = arbitrary_a();
        let b = arbitrary_b();
        let r = lerp_matrix_components(a, b, true, true, true, 1.0);
        for i in 0..4 {
            let rr = r.rows[i];
            let br = b.rows[i];
            assert!((rr.x - br.x).abs() < 1e-4, "row {i} x");
            assert!((rr.y - br.y).abs() < 1e-4, "row {i} y");
            assert!((rr.z - br.z).abs() < 1e-4, "row {i} z");
            assert!((rr.w - br.w).abs() < 1e-4, "row {i} w");
        }
    }

    #[test]
    fn lerp_matrix_components_angular_and_perspective_together_leave_row3_xyz_as_b() {
        // angular lerps rows 0-2's xyz; perspective lerps every row's w;
        // row 3's xyz is untouched by either (only `linear` touches it),
        // so it must remain exactly b's.
        let a = arbitrary_a();
        let b = arbitrary_b();
        let r = lerp_matrix_components(a, b, false, true, true, 0.5);
        assert_eq!(r.rows[3].x, b.rows[3].x);
        assert_eq!(r.rows[3].y, b.rows[3].y);
        assert_eq!(r.rows[3].z, b.rows[3].z);
    }

    #[test]
    fn lerp_matrix_components_linear_and_perspective_together_leave_rows_0_2_xyz_as_b() {
        // linear lerps row 3's xyz; perspective lerps every row's w;
        // rows 0-2's xyz are untouched by either (only `angular` touches
        // them), so they must remain exactly b's.
        let a = arbitrary_a();
        let b = arbitrary_b();
        let r = lerp_matrix_components(a, b, true, false, true, 0.5);
        assert_eq!(r.rows[0].x, b.rows[0].x);
        assert_eq!(r.rows[1].y, b.rows[1].y);
        assert_eq!(r.rows[2].z, b.rows[2].z);
        // But row 3's xyz and every row's w ARE lerped.
        let expected_row3_x = a.rows[3].x + 0.5 * (b.rows[3].x - a.rows[3].x);
        assert!((r.rows[3].x - expected_row3_x).abs() < 1e-4);
        let expected_w0 = a.rows[0].w + 0.5 * (b.rows[0].w - a.rows[0].w);
        assert!((r.rows[0].w - expected_w0).abs() < 1e-4);
    }

    #[test]
    fn lerp_matrix_components_nan_t_propagates_only_through_active_gates() {
        let a = arbitrary_a();
        let b = arbitrary_b();
        let r = lerp_matrix_components(a, b, false, true, false, f32::NAN);
        // angular gate lerped -> NaN.
        assert!(r.rows[0].x.is_nan());
        // linear/perspective gates inactive -> untouched b values, not NaN.
        assert_eq!(r.rows[3].x, b.rows[3].x);
        assert_eq!(r.rows[0].w, b.rows[0].w);
    }

    // --- matrix_scale / matrix_scale_vec3 ---

    /// Asserts every one of the 16 elements, so a stray write anywhere is
    /// caught rather than only the four the function intends to touch.
    fn assert_mat4_exact(m: Mat4, expected: [[f32; 4]; 4], what: &str) {
        for i in 0..4 {
            let r = m.rows[i];
            for (j, got) in [r.x, r.y, r.z, r.w].into_iter().enumerate() {
                assert_eq!(got, expected[i][j], "{what}: element [{i}][{j}]");
            }
        }
    }

    #[test]
    fn matrix_scale_writes_three_diagonals_and_a_literal_one_at_3_3() {
        assert_mat4_exact(
            matrix_scale(2.5),
            [
                [2.5, 0.0, 0.0, 0.0],
                [0.0, 2.5, 0.0, 0.0],
                [0.0, 0.0, 2.5, 0.0],
                [0.0, 0.0, 0.0, 1.0],
            ],
            "matrix_scale(2.5)",
        );
    }

    /// `matrixScale` starts from `float4x4(0.0f)`, NOT the identity -- so a
    /// scale of 0 must leave `[3][3]` at `1.0` and everything else `0.0`.
    /// If the implementation were rewritten to start from the identity, the
    /// three diagonal entries would be overwritten with 0 correctly but the
    /// distinction would be invisible; this pins the zero-start by checking
    /// the whole matrix at `scale == 0`.
    #[test]
    fn matrix_scale_zero_is_all_zero_except_a_one_at_3_3() {
        assert_mat4_exact(
            matrix_scale(0.0),
            [
                [0.0, 0.0, 0.0, 0.0],
                [0.0, 0.0, 0.0, 0.0],
                [0.0, 0.0, 0.0, 0.0],
                [0.0, 0.0, 0.0, 1.0],
            ],
            "matrix_scale(0.0)",
        );
    }

    /// `[3][3]` is a literal `1.0f` in the source, independent of `scale` --
    /// it is not `scale` and not derived from it.
    #[test]
    fn matrix_scale_does_not_scale_the_3_3_element() {
        assert_eq!(matrix_scale(7.0).rows[3].w, 1.0);
        assert_eq!(matrix_scale(-3.0).rows[3].w, 1.0);
    }

    /// The three diagonal entries must take `x`, `y`, `z` in that order and
    /// land on rows 0, 1, 2 respectively -- a permutation would survive a
    /// uniform-scale test but not this one.
    #[test]
    fn matrix_scale_vec3_maps_xyz_to_the_diagonal_in_order() {
        assert_mat4_exact(
            matrix_scale_vec3(Vec3::new(2.0, 3.0, 5.0)),
            [
                [2.0, 0.0, 0.0, 0.0],
                [0.0, 3.0, 0.0, 0.0],
                [0.0, 0.0, 5.0, 0.0],
                [0.0, 0.0, 0.0, 1.0],
            ],
            "matrix_scale_vec3(2,3,5)",
        );
    }

    #[test]
    fn matrix_scale_vec3_with_equal_components_matches_the_scalar_overload() {
        let uniform = matrix_scale(1.5);
        let by_vec = matrix_scale_vec3(Vec3::new(1.5, 1.5, 1.5));
        assert_eq!(uniform, by_vec);
    }

    // --- matrix_translation ---

    #[test]
    fn matrix_translation_writes_row_three_and_keeps_the_identity_elsewhere() {
        assert_mat4_exact(
            matrix_translation(Vec3::new(10.0, 20.0, 30.0)),
            [
                [1.0, 0.0, 0.0, 0.0],
                [0.0, 1.0, 0.0, 0.0],
                [0.0, 0.0, 1.0, 0.0],
                [10.0, 20.0, 30.0, 1.0],
            ],
            "matrix_translation(10,20,30)",
        );
    }

    /// Row-major row 3, NOT column 3. A column-vector-convention
    /// implementation would put the translation in `rows[0].w`,
    /// `rows[1].w`, `rows[2].w` -- the exact transpose. This asserts the
    /// column is still the identity's, which is what distinguishes them.
    #[test]
    fn matrix_translation_uses_the_row_vector_convention_not_the_transpose() {
        let m = matrix_translation(Vec3::new(4.0, 5.0, 6.0));
        assert_eq!((m.rows[3].x, m.rows[3].y, m.rows[3].z), (4.0, 5.0, 6.0));
        // The transposed (column) slots must be untouched zeros.
        assert_eq!(m.rows[0].w, 0.0);
        assert_eq!(m.rows[1].w, 0.0);
        assert_eq!(m.rows[2].w, 0.0);
    }

    #[test]
    fn matrix_translation_of_zero_is_exactly_the_identity() {
        assert_eq!(matrix_translation(Vec3::new(0.0, 0.0, 0.0)), identity());
    }

    /// `[3][3]` comes from the identity and is never overwritten by `t`.
    #[test]
    fn matrix_translation_leaves_3_3_at_one() {
        assert_eq!(
            matrix_translation(Vec3::new(-1.0, -2.0, -3.0)).rows[3].w,
            1.0
        );
    }

    // --- rotation helpers ---

    /// Asserts all nine elements of a `Mat3`, so a write to the wrong block
    /// is caught even when the intended block is correct.
    fn assert_mat3_exact(m: Mat3, expected: [[f32; 3]; 3], what: &str) {
        for i in 0..3 {
            let r = m.rows[i];
            for (j, got) in [r.x, r.y, r.z].into_iter().enumerate() {
                assert_eq!(got, expected[i][j], "{what}: element [{i}][{j}]");
            }
        }
    }

    fn mat3_ident() -> [[f32; 3]; 3] {
        [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]]
    }

    // matrix_rotation_x: XY block, -sin above the diagonal.

    #[test]
    fn matrix_rotation_x_at_zero_is_the_identity_by_value() {
        assert_mat3_exact(matrix_rotation_x(0.0), mat3_ident(), "rot_x(0)");
    }

    /// At `rad == 0` the `-sin` slot holds `-0.0`, which compares equal to
    /// `0.0` and so is invisible to the by-value test above. Pinned at the
    /// bit level because it is a genuine difference from `identity()`.
    #[test]
    fn matrix_rotation_x_at_zero_has_negative_zero_in_the_minus_sin_slot() {
        let m = matrix_rotation_x(0.0);
        assert_eq!(
            m.rows[0].y.to_bits(),
            (-0.0f32).to_bits(),
            "m[0][1] is -0.0"
        );
        assert_eq!(m.rows[1].x.to_bits(), 0.0f32.to_bits(), "m[1][0] is +0.0");
    }

    /// `cos(f32::FRAC_PI_2)` is `-4.371139e-8`, not `0.0`, and `sin` is
    /// exactly `1.0`. Values independently confirmed against a separate
    /// computation; `-sin` therefore lands as exactly `-1.0`.
    #[test]
    fn matrix_rotation_x_at_half_pi_is_exact() {
        let c = std::f32::consts::FRAC_PI_2.cos();
        assert_eq!(c.to_bits(), 0xb33bbd2e, "cos(f32 pi/2) is not zero");
        assert_mat3_exact(
            matrix_rotation_x(std::f32::consts::FRAC_PI_2),
            [[c, -1.0, 0.0], [1.0, c, 0.0], [0.0, 0.0, 1.0]],
            "rot_x(pi/2)",
        );
    }

    /// `sin(f32::PI)` is `-8.742278e-8` -- negative -- so the `-sin` slot
    /// holds a small *positive* value here, the opposite of the naive
    /// expectation.
    #[test]
    fn matrix_rotation_x_at_pi_is_exact() {
        let s = std::f32::consts::PI.sin();
        assert_eq!(s.to_bits(), 0xb3bbbd2e, "sin(f32 pi) is small and negative");
        assert_mat3_exact(
            matrix_rotation_x(std::f32::consts::PI),
            [[-1.0, -s, 0.0], [s, -1.0, 0.0], [0.0, 0.0, 1.0]],
            "rot_x(pi)",
        );
        assert!(matrix_rotation_x(std::f32::consts::PI).rows[0].y > 0.0);
    }

    #[test]
    fn matrix_rotation_x_at_a_non_special_angle_is_exact() {
        let (c, s) = (0.5f32.cos(), 0.5f32.sin());
        assert_mat3_exact(
            matrix_rotation_x(0.5),
            [[c, -s, 0.0], [s, c, 0.0], [0.0, 0.0, 1.0]],
            "rot_x(0.5)",
        );
    }

    /// The sign convention, stated explicitly: `-sin` ABOVE the diagonal at
    /// `[0][1]`, `+sin` BELOW at `[1][0]`, and the untouched axis is Z.
    /// A transpose or a sign flip fails this.
    #[test]
    fn matrix_rotation_x_sign_convention_and_block_are_pinned() {
        let m = matrix_rotation_x(0.5);
        let s = 0.5f32.sin();
        assert!(
            s > 0.0,
            "sin(0.5) is positive, so the signs below are readable"
        );
        assert_eq!(m.rows[0].y, -s, "[0][1] is -sin (above the diagonal)");
        assert_eq!(m.rows[1].x, s, "[1][0] is +sin (below the diagonal)");
        // Named "X" but the untouched axis is Z: this is really a Z rotation.
        assert_eq!(m.rows[2].z, 1.0, "[2][2] untouched -> rotates about Z");
    }

    // matrix_rotation_y: XZ block, +sin above the diagonal (MIRRORED).

    #[test]
    fn matrix_rotation_y_at_zero_is_the_identity_by_value() {
        assert_mat3_exact(matrix_rotation_y(0.0), mat3_ident(), "rot_y(0)");
    }

    #[test]
    fn matrix_rotation_y_at_half_pi_is_exact() {
        let c = std::f32::consts::FRAC_PI_2.cos();
        assert_mat3_exact(
            matrix_rotation_y(std::f32::consts::FRAC_PI_2),
            [[c, 0.0, 1.0], [0.0, 1.0, 0.0], [-1.0, 0.0, c]],
            "rot_y(pi/2)",
        );
    }

    #[test]
    fn matrix_rotation_y_at_pi_is_exact() {
        let s = std::f32::consts::PI.sin();
        assert_mat3_exact(
            matrix_rotation_y(std::f32::consts::PI),
            [[-1.0, 0.0, s], [0.0, 1.0, 0.0], [-s, 0.0, -1.0]],
            "rot_y(pi)",
        );
    }

    #[test]
    fn matrix_rotation_y_at_a_non_special_angle_is_exact() {
        let (c, s) = (0.5f32.cos(), 0.5f32.sin());
        assert_mat3_exact(
            matrix_rotation_y(0.5),
            [[c, 0.0, s], [0.0, 1.0, 0.0], [-s, 0.0, c]],
            "rot_y(0.5)",
        );
    }

    /// Y's sign convention is the MIRROR of X's and Z's: `+sin` above the
    /// diagonal, `-sin` below. This is the trap the module doc warns about
    /// -- a "normalized" Y matching its siblings would be a rotation by
    /// `-rad`. Pinned against X so the asymmetry itself is asserted, not
    /// just Y's values in isolation.
    #[test]
    fn matrix_rotation_y_sign_convention_is_mirrored_relative_to_x_and_z() {
        let s = 0.5f32.sin();
        let y = matrix_rotation_y(0.5);
        assert_eq!(y.rows[0].z, s, "Y: [0][2] is +sin (above the diagonal)");
        assert_eq!(y.rows[2].x, -s, "Y: [2][0] is -sin (below the diagonal)");
        // X puts the signs the other way round.
        let x = matrix_rotation_x(0.5);
        assert_eq!(x.rows[0].y, -s, "X: -sin above");
        assert_eq!(x.rows[1].x, s, "X: +sin below");
        // Y's untouched axis is Y: this one IS named for the axis it rotates.
        assert_eq!(y.rows[1].y, 1.0, "[1][1] untouched -> rotates about Y");
    }

    // matrix_rotation_z: YZ block, -sin above the diagonal.

    #[test]
    fn matrix_rotation_z_at_zero_is_the_identity_by_value() {
        assert_mat3_exact(matrix_rotation_z(0.0), mat3_ident(), "rot_z(0)");
    }

    #[test]
    fn matrix_rotation_z_at_half_pi_is_exact() {
        let c = std::f32::consts::FRAC_PI_2.cos();
        assert_mat3_exact(
            matrix_rotation_z(std::f32::consts::FRAC_PI_2),
            [[1.0, 0.0, 0.0], [0.0, c, -1.0], [0.0, 1.0, c]],
            "rot_z(pi/2)",
        );
    }

    #[test]
    fn matrix_rotation_z_at_pi_is_exact() {
        let s = std::f32::consts::PI.sin();
        assert_mat3_exact(
            matrix_rotation_z(std::f32::consts::PI),
            [[1.0, 0.0, 0.0], [0.0, -1.0, -s], [0.0, s, -1.0]],
            "rot_z(pi)",
        );
    }

    #[test]
    fn matrix_rotation_z_at_a_non_special_angle_is_exact() {
        let (c, s) = (0.5f32.cos(), 0.5f32.sin());
        assert_mat3_exact(
            matrix_rotation_z(0.5),
            [[1.0, 0.0, 0.0], [0.0, c, -s], [0.0, s, c]],
            "rot_z(0.5)",
        );
    }

    /// Z's sign convention: `-sin` above the diagonal at `[1][2]`, `+sin`
    /// below at `[2][1]`, matching X and opposing Y. Named "Z" but the
    /// untouched axis is X.
    #[test]
    fn matrix_rotation_z_sign_convention_and_block_are_pinned() {
        let m = matrix_rotation_z(0.5);
        let s = 0.5f32.sin();
        assert_eq!(m.rows[1].z, -s, "[1][2] is -sin (above the diagonal)");
        assert_eq!(m.rows[2].y, s, "[2][1] is +sin (below the diagonal)");
        assert_eq!(m.rows[0].x, 1.0, "[0][0] untouched -> rotates about X");
    }

    /// The three helpers touch three DISJOINT 2x2 blocks. Swapping any two
    /// implementations, or writing to the wrong block, fails here.
    #[test]
    fn the_three_rotations_touch_three_disjoint_blocks() {
        let (x, y, z) = (
            matrix_rotation_x(0.5),
            matrix_rotation_y(0.5),
            matrix_rotation_z(0.5),
        );
        // X leaves row2/col2 alone; Y leaves row1/col1 alone; Z leaves row0/col0.
        assert_eq!((x.rows[2].x, x.rows[2].y, x.rows[2].z), (0.0, 0.0, 1.0));
        assert_eq!((y.rows[1].x, y.rows[1].y, y.rows[1].z), (0.0, 1.0, 0.0));
        assert_eq!((z.rows[0].x, z.rows[0].y, z.rows[0].z), (1.0, 0.0, 0.0));
        // And they are pairwise distinct at the same angle.
        assert_ne!(x, y);
        assert_ne!(y, z);
        assert_ne!(x, z);
    }

    // --- inverse4 re-export ---

    /// The re-export reaches the decompose module's landed inverse and
    /// behaves as an inverse through THIS module's public API.
    #[test]
    fn inverse4_reexport_inverts_a_translation_through_the_public_api() {
        let t = matrix_translation(Vec3::new(3.0, -4.0, 5.0));
        let inv = inverse4(t);
        // Inverse of a translation is the negated translation.
        assert!((inv.rows[3].x - -3.0).abs() < 1e-5);
        assert!((inv.rows[3].y - 4.0).abs() < 1e-5);
        assert!((inv.rows[3].z - -5.0).abs() < 1e-5);
        assert!((inv.rows[3].w - 1.0).abs() < 1e-5);
    }

    #[test]
    fn inverse4_reexport_inverts_a_uniform_scale_through_the_public_api() {
        let inv = inverse4(matrix_scale(4.0));
        assert!((inv.rows[0].x - 0.25).abs() < 1e-6);
        assert!((inv.rows[1].y - 0.25).abs() < 1e-6);
        assert!((inv.rows[2].z - 0.25).abs() < 1e-6);
        assert!((inv.rows[3].w - 1.0).abs() < 1e-6);
    }

    /// Unguarded singular input is preserved by the re-export -- it does not
    /// add a guard the underlying function does not have.
    #[test]
    fn inverse4_reexport_preserves_unguarded_singular_behavior() {
        let inv = inverse4(matrix_scale(0.0));
        let any_non_finite = (0..4).any(|i| {
            let r = inv.rows[i];
            [r.x, r.y, r.z, r.w].iter().any(|v| !v.is_finite())
        });
        assert!(any_non_finite, "singular input divides by zero, unguarded");
    }
}
