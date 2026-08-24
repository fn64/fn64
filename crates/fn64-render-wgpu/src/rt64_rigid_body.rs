//! Literal port of RT64's `RigidBody` interpolation heuristics --
//! `RigidBody()` (constructor), `updateLinear`, `updateAngular`,
//! `updatePerspective`, `updateDecomposition`, and `lerp` -- a literal port
//! of the permitted MIT RT64 Rust-port source pinned at commit
//! `5473732a822a4423b5696e7cb18fecc425a59875` (`docs/RT64-PORT-AUTHORITY.md`),
//! `src/hle/rt64_rigid_body.cpp`/`.h` (SHA-256 of the whole files,
//! `b04a8571c9b2f5882ce41c501c95b9b30486e7cb11dcd999c60d1f4552155ac0` /
//! `40a633ad290bc05a5cbb43d592f34d65e91856aedfc0ba3a57d63b641c9f0315`,
//! cross-checked against `docs/rt64-port-inventory.json`'s
//! `sources.port.sha256` for both `src/hle/rt64_rigid_body.cpp` and
//! `src/hle/rt64_rigid_body.h`):
//!
//! Source (`src/hle/rt64_rigid_body.h`, whole file, 42 lines):
//!
//! ```text
//! // lines 11-21
//! struct RigidBody {
//!     DecomposedTransform transforms[2];
//!     hlslpp::float3 linearVelocity = {};
//!     float angularVelocity = 0.0f;
//!     uint8_t transformIndex = 0;
//!     bool lerpTranslation = false;
//!     bool lerpRotation = false;
//!     bool lerpScale = false;
//!     bool lerpSkew = false;
//!     bool lerpPerspective = false;
//!     bool lerpDecompose = true;
//! ```
//!
//! Source (`src/hle/rt64_rigid_body.cpp`, whole file, 138 lines):
//!
//! ```text
//! // lines 13-17
//! RigidBody::RigidBody() {
//!     transforms[0] = {};
//!     transforms[1] = {};
//!     linearVelocity = { 0.0f, 0.0f, 0.0f };
//! }
//!
//! // lines 19-39
//! void RigidBody::updateLinear(const hlslpp::float4x4 &prevTransform, const hlslpp::float4x4 &curTransform, uint8_t componentInterpolation) {
//!     if (componentInterpolation == G_EX_COMPONENT_AUTO) {
//!         const float Epsilon = 1e-6f;
//!         const float VelocityTolerance = 5.0f; // TODO: Make configurable.
//!         const float MagnitudeThreshold = 10.0f; // TODO: Make configurable.
//!         hlslpp::float3 prevPosition = prevTransform[3].xyz;
//!         hlslpp::float3 curPosition = curTransform[3].xyz;
//!         hlslpp::float3 curLinearVelocity = curPosition - prevPosition;
//!         hlslpp::float3 curAcceleration = (curLinearVelocity - linearVelocity);
//!         float prevVelMag = hlslpp::length(linearVelocity);
//!         float curVelMag = hlslpp::length(curLinearVelocity);
//!         float dotCurVel = std::max(hlslpp::dot(linearVelocity / std::max(prevVelMag, Epsilon), curLinearVelocity / std::max(curVelMag, Epsilon))[0], Epsilon);
//!         curVelMag /= dotCurVel;
//!         lerpTranslation = (curVelMag < VelocityTolerance) || (curVelMag / std::max(prevVelMag, Epsilon)) < MagnitudeThreshold;
//!         linearVelocity = curLinearVelocity;
//!     }
//!     else {
//!         lerpTranslation = (componentInterpolation == G_EX_COMPONENT_INTERPOLATE);
//!         linearVelocity = 0.0f;
//!     }
//! }
//!
//! // lines 41-71
//! void RigidBody::updateAngular(const hlslpp::float4x4 &prevTransform, const hlslpp::float4x4 &curTransform, uint8_t rotInterpolation, uint8_t scaleInterpolation, uint8_t skewInterpolation) {
//!     // TODO independent scale and skew auto, currently assumed to match the result of rotation auto calculation.
//!     // If rotation isn't auto then these default to false for their auto settings.
//!     lerpScale = (scaleInterpolation == G_EX_COMPONENT_INTERPOLATE);
//!     lerpSkew = (skewInterpolation == G_EX_COMPONENT_INTERPOLATE);
//!
//!     if (rotInterpolation == G_EX_COMPONENT_AUTO) {
//!         // Track angular velocity.
//!         const hlslpp::float3x3 invPrevRotation = hlslpp::inverse(rotationFrom3x3(extract3x3(prevTransform)));
//!         const hlslpp::float3x3 diffRotation = hlslpp::mul(invPrevRotation, rotationFrom3x3(extract3x3(curTransform)));
//!         float diffTrace = traceFrom3x3(diffRotation);
//!         float curAngularVelocity = std::acos((diffTrace - 1.0f) / 2.0f);
//!         angularVelocity = curAngularVelocity;
//!
//!         // FIXME: Defaults to always interpolate.
//!         lerpRotation = true;
//!
//!         // If scale or skew are also set to auto, use the result of rotation auto calculation for their value as well.
//!         if (scaleInterpolation == G_EX_COMPONENT_AUTO) {
//!             lerpScale = lerpRotation;
//!         }
//!
//!         if (skewInterpolation == G_EX_COMPONENT_AUTO) {
//!             lerpSkew = lerpRotation;
//!         }
//!     }
//!     else {
//!         lerpRotation = (rotInterpolation == G_EX_COMPONENT_INTERPOLATE);
//!         angularVelocity = 0.0f;
//!     }
//! }
//!
//! // lines 73-76
//! void RigidBody::updatePerspective(const hlslpp::float4x4 &prevTransform, const hlslpp::float4x4 &curTransform, uint8_t perspInterpolation) {
//!     // TODO auto perspective interpolation.
//!     lerpPerspective = (perspInterpolation == G_EX_COMPONENT_INTERPOLATE);
//! }
//!
//! // lines 78-87
//! void RigidBody::updateDecomposition(const hlslpp::float4x4 &curTransform, bool decompose) {
//!     uint8_t newTransformIndex = transformIndex ^ 1;
//!     if (decompose) {
//!         transforms[newTransformIndex] = DecomposedTransform(curTransform);
//!     } else {
//!         transforms[newTransformIndex] = DecomposedTransform();
//!     }
//!     transformIndex = newTransformIndex;
//!     lerpDecompose = decompose;
//! }
//!
//! // lines 90-137
//! hlslpp::float4x4 RigidBody::lerp(float weight, const hlslpp::float4x4& fallbackPrev, const hlslpp::float4x4& fallbackCur, bool slerp) const {
//!     // Return a linear component-wise interpolation of the fallback matrices if decomposition is disabled or if either decomposition is invalid.
//!     if (!lerpDecompose || !transforms[0].valid || !transforms[1].valid) {
//!         return lerpMatrixComponents(fallbackPrev, fallbackCur, lerpTranslation, lerpRotation, lerpPerspective, weight);
//!     }
//!
//!     const DecomposedTransform &prevTransform = transforms[transformIndex ^ 1];
//!     DecomposedTransform prevTransformCopy = prevTransform;
//!     const DecomposedTransform &curTransform = transforms[transformIndex];
//!     DecomposedTransform lerpedTransform;
//!
//!     // When the coordinate system is flipped between transforms due to a different sign in the determinant, we bias the rotation and scale of the
//!     // previous transform to be similar to the new one by producing a transform that produces an equivalent matrix but with a rotation and scale
//!     // that are closer to what's intended. This is necessary to improve interpolation between objects that use mirroring in animations.
//!     if (prevTransformCopy.coordinateFlip != curTransform.coordinateFlip) {
//!         constexpr float Pi = 3.14159265f;
//!         const hlslpp::quaternion &prevRot = prevTransformCopy.rotation;
//!         hlslpp::quaternion xRot = hlslpp::mul(prevTransformCopy.rotation, hlslpp::quaternion::rotation_axis(hlslpp::float3(1.0f, 0.0, 0.0f), Pi));
//!         hlslpp::quaternion yRot = hlslpp::mul(prevTransformCopy.rotation, hlslpp::quaternion::rotation_axis(hlslpp::float3(0.0f, 1.0, 0.0f), Pi));
//!         hlslpp::quaternion zRot = hlslpp::mul(prevTransformCopy.rotation, hlslpp::quaternion::rotation_axis(hlslpp::float3(0.0f, 0.0, 1.0f), Pi));
//!         float rotDotProduct = abs(hlslpp::dot(prevTransformCopy.rotation, curTransform.rotation));
//!         float xRotDotProduct = abs(hlslpp::dot(xRot, curTransform.rotation));
//!         float yRotDotProduct = abs(hlslpp::dot(yRot, curTransform.rotation));
//!         float zRotDotProduct = abs(hlslpp::dot(zRot, curTransform.rotation));
//!         if (xRotDotProduct > rotDotProduct) {
//!             prevTransformCopy.rotation = xRot;
//!             prevTransformCopy.scale = hlslpp::float3(prevTransform.scale.x, -prevTransform.scale.y, -prevTransform.scale.z);
//!             rotDotProduct = xRotDotProduct;
//!         }
//!
//!         if (yRotDotProduct > rotDotProduct) {
//!             prevTransformCopy.rotation = yRot;
//!             prevTransformCopy.scale = hlslpp::float3(-prevTransform.scale.x, prevTransform.scale.y, -prevTransform.scale.z);
//!             rotDotProduct = yRotDotProduct;
//!         }
//!
//!         if (zRotDotProduct > rotDotProduct) {
//!             prevTransformCopy.rotation = zRot;
//!             prevTransformCopy.scale = hlslpp::float3(-prevTransform.scale.x, -prevTransform.scale.y, prevTransform.scale.z);
//!         }
//!     }
//!
//!     // Lerp the two transforms.
//!     lerpedTransform = lerpTransforms(prevTransformCopy, curTransform, weight, lerpTranslation, lerpRotation, lerpScale, lerpSkew, lerpPerspective, slerp);
//!
//!     // Compose a matrix from the resultant transform.
//!     return recomposeMatrix(lerpedTransform.rotation, lerpedTransform.scale, lerpedTransform.skew, lerpedTransform.translation, lerpedTransform.perspective);
//! }
//! ```
//!
//! **Reuse, not new type.** This module reuses, unmodified, all of the
//! following already-landed sibling infrastructure rather than writing a
//! second copy:
//!
//! - `crate::rt64_math_matrix::{extract_3x3, rotation_from_3x3}` for
//!   `extract3x3`/`rotationFrom3x3` in `updateAngular`.
//! - `crate::rt64_math::{Mat3, trace_from_3x3}` for `traceFrom3x3` and the
//!   3x3 matrix shape `extract_3x3`/`rotation_from_3x3` already return (both
//!   already `pub` in the sibling `rt64_math` module; this module only
//!   reads them via `crate::rt64_math::...`, it does not edit `rt64_math.rs`,
//!   which is outside this ticket's exclusive-paths edit set).
//! - `crate::rt64_math_decompose::{DecomposedTransform, Quat,
//!   decompose_matrix (transitively, via DecomposedTransform::from_matrix),
//!   recompose_matrix, lerp_transforms}` for `updateDecomposition` and
//!   `RigidBody::lerp`'s decomposed-transform path.
//! - `crate::rt64_math_matrix::lerp_matrix_components` for `RigidBody::lerp`'s
//!   fallback (non-decomposed) path.
//! - `crate::rt64_extended_gbi::{G_EX_COMPONENT_SKIP, G_EX_COMPONENT_INTERPOLATE,
//!   G_EX_COMPONENT_AUTO}` (already-landed `pub const ...: u32` opcode
//!   constants) for the `G_EX_COMPONENT_*` comparisons in `updateLinear`,
//!   `updateAngular`, and `updatePerspective`, rather than redefining a
//!   second copy of these three constants under a new name. Because that
//!   module's constants are typed `u32` (not the source's `uint8_t`), this
//!   port's `component_interpolation`/`rot_interpolation`/
//!   `scale_interpolation`/`skew_interpolation`/`persp_interpolation`
//!   parameters are `u32` to match directly -- an equality-comparison-only
//!   parameter has no `u8`-specific overflow/wraparound behavior to lose by
//!   widening, so this is a type-compatibility accommodation, not a
//!   behavior change.
//!
//! This module adds local infrastructure that none of the sibling modules
//! provide, because `updateAngular` and `RigidBody::lerp` are the first
//! functions in this file cluster that need them:
//!
//! - `vec3_scale_div` (vector-scalar-divide for `float3`): neither
//!   `fn64_render_ir::Vec3` nor any sibling module exposes it.
//!   `hlslpp::length(float3)` itself is **not** redefined here -- this
//!   module calls [`crate::rt64_math_decompose::vec3_length`], which was
//!   widened to `pub(crate)` for exactly this caller. (An earlier revision
//!   of this doc said that helper was private and "not reusable across
//!   module boundaries without making it `pub`, which is outside this
//!   ticket's edit rights"; that is no longer true and the local duplicate
//!   it justified has been retired.) Note that
//!   `rt64_preset_light`/`rt64_lights_math`/`rt64_rsp_smooth_normal` each
//!   have their own `length`, cited to *different* C++ authorities with
//!   their own ulp and NaN caveats -- those are deliberately separate and
//!   must not be folded into this one.
//! - `mat3_mul`/`mat3_inverse` (`hlslpp::mul(float3x3,float3x3)` and
//!   `hlslpp::inverse(float3x3)`): no 3x3 matrix multiply or inverse exists
//!   anywhere in this crate or `fn64_render_ir` -- `rt64_math_decompose.rs`
//!   only built a **4x4** `mat4_mul`/`inverse4` (for `decomposeMatrix`'s
//!   `float4x4` perspective-matrix inverse), which is a different-sized,
//!   non-reusable operation for `updateAngular`'s `float3x3` inverse.
//! - `quat_mul` (`hlslpp::mul(quaternion,quaternion)`, Hamilton product) and
//!   `quat_rotation_axis` (`hlslpp::quaternion::rotation_axis(axis, angle)`,
//!   axis-angle quaternion constructor) are genuinely new operations: no
//!   `mul`/`rotation_axis` exists on `Quat` at any visibility. Both are
//!   implemented as free functions taking/returning
//!   `crate::rt64_math_decompose::Quat` directly (not a second quaternion
//!   type), since `Quat`'s fields (`x, y, z, w`) are `pub`.
//!
//!   The quaternion dot product is **not** among them. An earlier revision
//!   of this module carried a local `quat_dot`, justified by the claim that
//!   `Quat`'s `dot`/`neg`/`normalize` "are private ... so this module cannot
//!   call `Quat::dot` directly". `Quat::dot` is now `pub(crate)` and the
//!   `updateAngular` bias branch calls it directly; the duplicate is gone.
//!   `neg` and `normalize` remain private -- nothing outside
//!   `rt64_math_decompose` calls them, so neither was widened.
//!
//! ## Admitted domain
//!
//! - **`updateLinear`'s division chain is unguarded except where the source
//!   itself guards it.** `linearVelocity / max(prevVelMag, Epsilon)` and
//!   `curLinearVelocity / max(curVelMag, Epsilon)` are guarded (the source's
//!   own `std::max(_, Epsilon)` with `Epsilon = 1e-6f`, preventing a literal
//!   `0.0/0.0` there), but `curVelMag /= dotCurVel` is divided by
//!   `max(dot(...), Epsilon)` -- also guarded, per the source's explicit
//!   `std::max` wrapping the `dot(...)` call itself (not just its operands).
//!   No division in this function is genuinely unguarded; this port
//!   preserves the source's exact guard placement (which operand of each
//!   division is clamped, and by what) rather than adding or removing any
//!   guard.
//! - **`curAcceleration` is computed and discarded.** The source computes
//!   `hlslpp::float3 curAcceleration = (curLinearVelocity - linearVelocity)`
//!   and never reads it again in this function body -- this is dead
//!   upstream code (confirmed by reading the full 138-line source above; no
//!   later use exists in `updateLinear`, and `curAcceleration` is a local,
//!   not a field). This port preserves the computation itself (in case a
//!   later ticket needs to reference this exact intermediate value, and to
//!   keep the arithmetic-order trace faithful) but does not use its result
//!   for anything, matching the source.
//! - **`updateLinear`'s boolean assignment is short-circuiting `||`, not a
//!   ported bitwise `|`.** `lerpTranslation = (curVelMag < VelocityTolerance)
//!   || (curVelMag / std::max(prevVelMag, Epsilon)) < MagnitudeThreshold` --
//!   Rust's `||` matches C++'s `||` exactly (short-circuit, left operand
//!   evaluated first); since the right operand here has no side effects
//!   (pure arithmetic), the short-circuit vs. always-evaluate distinction is
//!   unobservable for this specific expression, but this port uses `||` to
//!   match the source's operator literally rather than for that reason
//!   alone.
//! - **`updateAngular`'s `acos((diffTrace - 1.0) / 2.0)` is unguarded
//!   against out-of-`[-1,1]` domain.** `f32::acos` of an argument outside
//!   `[-1.0, 1.0]` returns `NaN` per Rust's IEEE-754-conformant `acos`
//!   (matching C's `acos(out-of-domain) = NaN`); `diffTrace` (the trace of a
//!   product of two rotation matrices assembled from *normalized* rows, not
//!   a true orthonormal rotation matrix -- `rotation_from_3x3` only
//!   normalizes each row independently, it does not orthogonalize the whole
//!   3x3, so `diffRotation` is not guaranteed to be a proper rotation and
//!   `diffTrace` is not guaranteed to lie in `[-1, 3]`) can therefore push
//!   `(diffTrace-1)/2` outside `[-1,1]` for degenerate/non-orthogonal inputs,
//!   in which case `curAngularVelocity` (and the stored `angular_velocity`
//!   field) becomes `NaN`. This port adds no clamp the source does not have.
//! - **`updateAngular`'s `mat3_inverse` is unguarded at a singular input.**
//!   A singular (zero-determinant) 3x3 divides by zero in the
//!   adjugate-over-determinant formula, producing `+-inf`/`NaN` entries
//!   rather than a panic, mirroring `rt64_math_decompose.rs::inverse4`'s
//!   same unguarded-singular-input precedent for its 4x4 sibling.
//! - **`RigidBody::lerp`'s coordinate-flip bias branch reads
//!   `prevTransform.scale.x/y/z` (the pre-copy original), not
//!   `prevTransformCopy.scale`, at each of the three `hlslpp::float3(...)`
//!   constructions.** This is the source's own exact behavior
//!   (`hlslpp::float3(prevTransform.scale.x, -prevTransform.scale.y,
//!   -prevTransform.scale.z)` etc. all read the `const DecomposedTransform
//!   &prevTransform` reference, never `prevTransformCopy`, even though
//!   `prevTransformCopy.rotation` was just reassigned in the same `if`
//!   block) -- so even if the `x`-rotation and `y`-rotation branches both
//!   fire in sequence (the source's three `if`s are independent, not
//!   `else if`), the `y`-branch's scale negation pattern
//!   (`-prevTransform.scale.x, prevTransform.scale.y,
//!   -prevTransform.scale.z`) is always computed from the **original**
//!   `prevTransform.scale`, never from a scale already negated by an earlier
//!   branch in the same call. This port preserves that exact
//!   copy/no-copy asymmetry between `.rotation` (mutated copy feeds forward)
//!   and `.scale` (always the pristine original) -- it is not a bug in this
//!   port; it is a literal read of the source's variable usage.
//! - **The three `if`s in the coordinate-flip bias branch are independent,
//!   not `else if`-chained**, exactly as the source's braces show -- so
//!   `zRotDotProduct > rotDotProduct` is compared against whatever
//!   `rotDotProduct` was left as by the *previous* `if` (which may have
//!   updated it from the `x`-branch, or the original `abs(dot(prevRot,
//!   curRot)))` value if the `x`-branch's condition was false), preserving
//!   the exact sequential-mutation dependency between the three checks. No
//!   branch was converted into a mutually-exclusive `match` or `else if`.
//! - **`abs(hlslpp::dot(...))` in the bias branch**: `hlslpp::dot` of two
//!   quaternions is assumed to be the conventional 4-component dot product
//!   (matching `rt64_math_decompose.rs`'s own `Quat::dot`, which this module
//!   reuses via the sibling `Quat` type -- not re-derived independently),
//!   and the bare `abs(...)` call (unqualified, ADL/using-directive resolved
//!   in the source) is assumed to be `f32::abs`, same convention as
//!   `rt64_math_matrix.rs`'s `matrix_difference` admitted-domain note for
//!   `hlslpp::abs`.
//! - **`quat_rotation_axis`'s exact formula is the standard axis-angle
//!   quaternion construction** (`(axis * sin(angle/2), cos(angle/2))`,
//!   assuming `axis` is already unit-length, which the three call sites
//!   always pass literally as `hlslpp::float3(1,0,0)`/`(0,1,0)`/`(0,0,1)` --
//!   already unit vectors, so this port does not need to decide whether
//!   `hlslpp::quaternion::rotation_axis` internally normalizes a non-unit
//!   axis, since no call site in this file ever supplies one). Same
//!   unpopulated-`hlslpp`-submodule caveat as `rt64_math_decompose.rs`
//!   documents throughout: this is the universal textbook convention, not a
//!   verified read of hlslpp's actual source.
//! - **`quat_mul`'s exact formula is the standard Hamilton quaternion
//!   product**, with the same unpopulated-`hlslpp` caveat. This port does
//!   not attempt to infer hlslpp's storage/multiplication convention
//!   (left-handed vs. right-handed, `xyzw` vs. `wxyz` internal layout)
//!   beyond the field order `crate::rt64_math_decompose::Quat` already
//!   established (`x, y, z, w`, matching the source's field declaration
//!   order) -- this is the same residual uncertainty
//!   `rt64_math_decompose.rs`'s own module doc already flags for
//!   `float4x4(quaternion)` and does not re-litigate here; this port simply
//!   inherits it, unresolved.
//! - **`RigidBody::lerp`'s `slerp: bool` parameter is passed straight
//!   through to `crate::rt64_math_decompose::lerp_transforms`'s
//!   `use_slerp` parameter without alteration.** This means this port
//!   **inherits, unresolved, `rt64_math_decompose.rs`'s own open question**:
//!   hlslpp's exact `slerp` near-parallel fallback threshold is unverified
//!   (`hlslpp` is an unpopulated submodule in every checkout available to
//!   this program) -- `rt64_math_decompose.rs::slerp_quat` implements the
//!   conventional textbook constant-angular-velocity formula with a
//!   `cos_theta.abs() > 0.9995` linear-lerp fallback as its own explicitly-
//!   flagged least-pinned assumption, not a verified read. `RigidBody::lerp`
//!   calling into that function with `slerp=true` does not add, narrow, or
//!   resolve that caveat in any way; it is restated here rather than
//!   silently dropped, per this ticket's instruction to inherit rather than
//!   re-settle it.
//! - **`updateDecomposition`'s `transformIndex ^ 1` toggles between the two
//!   fixed slots `0`/`1`.** Ported as plain `u8` XOR, matching C++'s
//!   `uint8_t transformIndex` and `newTransformIndex` exactly (`u8 ^ u8`
//!   has the same wraparound-free bit-toggle semantics as C++'s
//!   `uint8_t ^ int`-promoted-back-to-`uint8_t` here, since the operand `1`
//!   fits in a `uint8_t` and no overflow is reachable from `{0,1} ^ 1`).
//!
//! ## Nonclaims
//!
//! No GPU, WGSL, or production wiring -- this module is not called from
//! anywhere yet (dead-code warnings on the unused public surface are
//! expected and correct, matching every other characterization-first module
//! in this crate). No parity or performance claim against RT64's actual
//! runtime behavior; only the arithmetic and branch structure of the four
//! `RigidBody` methods (plus its constructor) named above are ported. Their
//! surrounding object graph -- `RigidBody`'s owning `GameFrame`/mesh/scene
//! objects -- is absent. No ported call sites decide interpolation modes from
//! parsed extended-GBI commands or schedule those methods frame-to-frame. These
//! are RT64's own heuristic interpolation-mode-selection logic (explicitly
//! marked `TODO`/`FIXME` by the source itself in three places -- configurable
//! velocity/magnitude thresholds, independent scale/skew auto-detection, and
//! auto perspective interpolation are all upstream-acknowledged
//! placeholders), not physically-derived rigid-body dynamics, and this port
//! makes no claim that the heuristic is correct, tuned, or complete --
//! merely that this Rust code reproduces the cited C++ literally.

use crate::rt64_extended_gbi::{G_EX_COMPONENT_AUTO, G_EX_COMPONENT_INTERPOLATE};
use crate::rt64_math::{trace_from_3x3, Mat3};
use crate::rt64_math_decompose::{
    lerp_transforms, recompose_matrix, vec3_length, DecomposedTransform, Quat,
};
use crate::rt64_math_matrix::{extract_3x3, lerp_matrix_components, rotation_from_3x3};
use fn64_render_ir::{Mat4, Vec3};

/// `RigidBody`. Field order and defaults match the source's struct
/// declaration (`src/hle/rt64_rigid_body.h:11-21`) exactly.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RigidBody {
    pub transforms: [DecomposedTransform; 2],
    pub linear_velocity: Vec3,
    pub angular_velocity: f32,
    pub transform_index: u8,
    pub lerp_translation: bool,
    pub lerp_rotation: bool,
    pub lerp_scale: bool,
    pub lerp_skew: bool,
    pub lerp_perspective: bool,
    pub lerp_decompose: bool,
}

impl RigidBody {
    /// `RigidBody::RigidBody()`. Explicitly re-zeroes `transforms` and
    /// `linearVelocity` even though the header's in-class initializers
    /// (`= {}` / default) would already leave them zeroed -- ported as a
    /// literal 1:1 read of the constructor body, not simplified to
    /// `Self::default()`, since `angular_velocity`/`transform_index`/the
    /// five `lerp_*` bools are set only by the header's in-class
    /// initializers (the constructor body never touches them), matching
    /// that same source-level split between "set in the .h defaults" and
    /// "set in the .cpp constructor body".
    pub fn new() -> Self {
        Self {
            transforms: [
                DecomposedTransform::default(),
                DecomposedTransform::default(),
            ],
            linear_velocity: Vec3::new(0.0, 0.0, 0.0),
            angular_velocity: 0.0,
            transform_index: 0,
            lerp_translation: false,
            lerp_rotation: false,
            lerp_scale: false,
            lerp_skew: false,
            lerp_perspective: false,
            lerp_decompose: true,
        }
    }

    /// `updateLinear`. `component_interpolation` is `u32` to match
    /// `crate::rt64_extended_gbi`'s already-landed constant type (see module
    /// doc "Reuse, not new type").
    pub fn update_linear(
        &mut self,
        prev_transform: Mat4,
        cur_transform: Mat4,
        component_interpolation: u32,
    ) {
        if component_interpolation == G_EX_COMPONENT_AUTO {
            const EPSILON: f32 = 1e-6;
            const VELOCITY_TOLERANCE: f32 = 5.0;
            const MAGNITUDE_THRESHOLD: f32 = 10.0;

            let prev_position = prev_transform.rows[3].xyz();
            let cur_position = cur_transform.rows[3].xyz();
            let cur_linear_velocity = cur_position.sub(prev_position);
            let _cur_acceleration = cur_linear_velocity.sub(self.linear_velocity);

            let prev_vel_mag = vec3_length(self.linear_velocity);
            let mut cur_vel_mag = vec3_length(cur_linear_velocity);

            let prev_vel_norm = vec3_scale_div(self.linear_velocity, prev_vel_mag.max(EPSILON));
            let cur_vel_norm = vec3_scale_div(cur_linear_velocity, cur_vel_mag.max(EPSILON));
            let dot_cur_vel = prev_vel_norm.dot(cur_vel_norm).max(EPSILON);

            cur_vel_mag /= dot_cur_vel;

            self.lerp_translation = (cur_vel_mag < VELOCITY_TOLERANCE)
                || (cur_vel_mag / prev_vel_mag.max(EPSILON)) < MAGNITUDE_THRESHOLD;

            self.linear_velocity = cur_linear_velocity;
        } else {
            self.lerp_translation = component_interpolation == G_EX_COMPONENT_INTERPOLATE;
            self.linear_velocity = Vec3::new(0.0, 0.0, 0.0);
        }
    }

    /// `updateAngular`. All four `*_interpolation` parameters are `u32` (see
    /// `update_linear`'s doc for why).
    pub fn update_angular(
        &mut self,
        prev_transform: Mat4,
        cur_transform: Mat4,
        rot_interpolation: u32,
        scale_interpolation: u32,
        skew_interpolation: u32,
    ) {
        self.lerp_scale = scale_interpolation == G_EX_COMPONENT_INTERPOLATE;
        self.lerp_skew = skew_interpolation == G_EX_COMPONENT_INTERPOLATE;

        if rot_interpolation == G_EX_COMPONENT_AUTO {
            let inv_prev_rotation = mat3_inverse(rotation_from_3x3(extract_3x3(prev_transform)));
            let diff_rotation = mat3_mul(
                inv_prev_rotation,
                rotation_from_3x3(extract_3x3(cur_transform)),
            );
            let diff_trace = trace_from_3x3(diff_rotation);
            let cur_angular_velocity = ((diff_trace - 1.0) / 2.0).acos();
            self.angular_velocity = cur_angular_velocity;

            self.lerp_rotation = true;

            if scale_interpolation == G_EX_COMPONENT_AUTO {
                self.lerp_scale = self.lerp_rotation;
            }

            if skew_interpolation == G_EX_COMPONENT_AUTO {
                self.lerp_skew = self.lerp_rotation;
            }
        } else {
            self.lerp_rotation = rot_interpolation == G_EX_COMPONENT_INTERPOLATE;
            self.angular_velocity = 0.0;
        }
    }

    /// `updatePerspective`.
    pub fn update_perspective(
        &mut self,
        _prev_transform: Mat4,
        _cur_transform: Mat4,
        persp_interpolation: u32,
    ) {
        self.lerp_perspective = persp_interpolation == G_EX_COMPONENT_INTERPOLATE;
    }

    /// `updateDecomposition`.
    pub fn update_decomposition(&mut self, cur_transform: Mat4, decompose: bool) {
        let new_transform_index = self.transform_index ^ 1;
        self.transforms[new_transform_index as usize] = if decompose {
            DecomposedTransform::from_matrix(cur_transform)
        } else {
            DecomposedTransform::default()
        };
        self.transform_index = new_transform_index;
        self.lerp_decompose = decompose;
    }

    /// `RigidBody::lerp`.
    pub fn lerp(&self, weight: f32, fallback_prev: Mat4, fallback_cur: Mat4, slerp: bool) -> Mat4 {
        if !self.lerp_decompose || !self.transforms[0].valid || !self.transforms[1].valid {
            return lerp_matrix_components(
                fallback_prev,
                fallback_cur,
                self.lerp_translation,
                self.lerp_rotation,
                self.lerp_perspective,
                weight,
            );
        }

        let prev_transform = self.transforms[(self.transform_index ^ 1) as usize];
        let mut prev_transform_copy = prev_transform;
        let cur_transform = self.transforms[self.transform_index as usize];

        if prev_transform_copy.coordinate_flip != cur_transform.coordinate_flip {
            const PI: f32 = 3.14159265;

            let x_rot = quat_mul(
                prev_transform_copy.rotation,
                quat_rotation_axis(Vec3::new(1.0, 0.0, 0.0), PI),
            );
            let y_rot = quat_mul(
                prev_transform_copy.rotation,
                quat_rotation_axis(Vec3::new(0.0, 1.0, 0.0), PI),
            );
            let z_rot = quat_mul(
                prev_transform_copy.rotation,
                quat_rotation_axis(Vec3::new(0.0, 0.0, 1.0), PI),
            );

            let mut rot_dot_product =
                Quat::dot(prev_transform_copy.rotation, cur_transform.rotation).abs();
            let x_rot_dot_product = Quat::dot(x_rot, cur_transform.rotation).abs();
            let y_rot_dot_product = Quat::dot(y_rot, cur_transform.rotation).abs();
            let z_rot_dot_product = Quat::dot(z_rot, cur_transform.rotation).abs();

            if x_rot_dot_product > rot_dot_product {
                prev_transform_copy.rotation = x_rot;
                prev_transform_copy.scale = Vec3::new(
                    prev_transform.scale.x,
                    -prev_transform.scale.y,
                    -prev_transform.scale.z,
                );
                rot_dot_product = x_rot_dot_product;
            }

            if y_rot_dot_product > rot_dot_product {
                prev_transform_copy.rotation = y_rot;
                prev_transform_copy.scale = Vec3::new(
                    -prev_transform.scale.x,
                    prev_transform.scale.y,
                    -prev_transform.scale.z,
                );
                rot_dot_product = y_rot_dot_product;
            }

            if z_rot_dot_product > rot_dot_product {
                prev_transform_copy.rotation = z_rot;
                prev_transform_copy.scale = Vec3::new(
                    -prev_transform.scale.x,
                    -prev_transform.scale.y,
                    prev_transform.scale.z,
                );
            }
        }

        let lerped_transform = lerp_transforms(
            &prev_transform_copy,
            &cur_transform,
            weight,
            self.lerp_translation,
            self.lerp_rotation,
            self.lerp_scale,
            self.lerp_skew,
            self.lerp_perspective,
            slerp,
        );

        recompose_matrix(
            lerped_transform.rotation,
            lerped_transform.scale,
            lerped_transform.skew,
            lerped_transform.translation,
            lerped_transform.perspective,
        )
    }
}

impl Default for RigidBody {
    fn default() -> Self {
        Self::new()
    }
}

/// `v / s` for a `float3` divided by a scalar (used for
/// `linearVelocity / std::max(prevVelMag, Epsilon)` and its `cur` sibling).
fn vec3_scale_div(v: Vec3, s: f32) -> Vec3 {
    Vec3::new(v.x / s, v.y / s, v.z / s)
}

/// `hlslpp::mul(float3x3, float3x3)`: ordinary structural matrix
/// multiplication, `mat3_mul(A,B)[i][j] = sum_k A[i][k]*B[k][j]`, matching
/// `rt64_math_decompose.rs::mat4_mul`'s same convention for the 4x4 case
/// (see module doc "Reuse, not new type").
fn mat3_mul(a: Mat3, b: Mat3) -> Mat3 {
    let get = |m: &Mat3, i: usize, j: usize| -> f32 {
        let row = m.rows[i];
        match j {
            0 => row.x,
            1 => row.y,
            2 => row.z,
            _ => unreachable!("column index out of range: {j}"),
        }
    };
    let mut rows = [Vec3::new(0.0, 0.0, 0.0); 3];
    for i in 0..3 {
        let mut out = [0.0f32; 3];
        for (j, out_j) in out.iter_mut().enumerate() {
            let mut sum = 0.0f32;
            for k in 0..3 {
                sum += get(&a, i, k) * get(&b, k, j);
            }
            *out_j = sum;
        }
        rows[i] = Vec3::new(out[0], out[1], out[2]);
    }
    Mat3 { rows }
}

/// `hlslpp::inverse(float3x3)` via the classical adjugate-over-determinant
/// formula. Unguarded: a singular (zero-determinant) input divides by zero,
/// producing `+-inf`/`NaN` entries rather than a panic, mirroring
/// `rt64_math_decompose.rs::inverse4`'s same unguarded-singular-input
/// precedent for its 4x4 sibling (see module doc "Admitted domain").
fn mat3_inverse(m: Mat3) -> Mat3 {
    let get = |i: usize, j: usize| -> f32 {
        let row = m.rows[i];
        match j {
            0 => row.x,
            1 => row.y,
            2 => row.z,
            _ => unreachable!("column index out of range: {j}"),
        }
    };

    // Cofactor matrix.
    let c00 = get(1, 1) * get(2, 2) - get(1, 2) * get(2, 1);
    let c01 = -(get(1, 0) * get(2, 2) - get(1, 2) * get(2, 0));
    let c02 = get(1, 0) * get(2, 1) - get(1, 1) * get(2, 0);
    let c10 = -(get(0, 1) * get(2, 2) - get(0, 2) * get(2, 1));
    let c11 = get(0, 0) * get(2, 2) - get(0, 2) * get(2, 0);
    let c12 = -(get(0, 0) * get(2, 1) - get(0, 1) * get(2, 0));
    let c20 = get(0, 1) * get(1, 2) - get(0, 2) * get(1, 1);
    let c21 = -(get(0, 0) * get(1, 2) - get(0, 2) * get(1, 0));
    let c22 = get(0, 0) * get(1, 1) - get(0, 1) * get(1, 0);

    let det = get(0, 0) * c00 + get(0, 1) * c01 + get(0, 2) * c02;

    // adjugate = transpose(cofactor); inverse = adjugate / det.
    Mat3 {
        rows: [
            Vec3::new(c00 / det, c10 / det, c20 / det),
            Vec3::new(c01 / det, c11 / det, c21 / det),
            Vec3::new(c02 / det, c12 / det, c22 / det),
        ],
    }
}

/// `hlslpp::mul(quaternion, quaternion)`: standard Hamilton quaternion
/// product (see module doc "Admitted domain" for the unpinned-formula
/// caveat).
fn quat_mul(a: Quat, b: Quat) -> Quat {
    Quat::new(
        a.w * b.x + a.x * b.w + a.y * b.z - a.z * b.y,
        a.w * b.y - a.x * b.z + a.y * b.w + a.z * b.x,
        a.w * b.z + a.x * b.y - a.y * b.x + a.z * b.w,
        a.w * b.w - a.x * b.x - a.y * b.y - a.z * b.z,
    )
}

/// `hlslpp::quaternion::rotation_axis(axis, angle)`: standard axis-angle
/// quaternion construction, `(axis * sin(angle/2), cos(angle/2))`, assuming
/// `axis` is already unit-length (see module doc "Admitted domain" -- every
/// call site in this file passes a literal unit axis).
fn quat_rotation_axis(axis: Vec3, angle: f32) -> Quat {
    let half = angle * 0.5;
    let s = half.sin();
    let c = half.cos();
    Quat::new(axis.x * s, axis.y * s, axis.z * s, c)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn identity() -> Mat4 {
        Mat4::from_rows([
            fn64_render_ir::Vec4::new(1.0, 0.0, 0.0, 0.0),
            fn64_render_ir::Vec4::new(0.0, 1.0, 0.0, 0.0),
            fn64_render_ir::Vec4::new(0.0, 0.0, 1.0, 0.0),
            fn64_render_ir::Vec4::new(0.0, 0.0, 0.0, 1.0),
        ])
    }

    fn translated(x: f32, y: f32, z: f32) -> Mat4 {
        let mut m = identity();
        m.rows[3] = fn64_render_ir::Vec4::new(x, y, z, 1.0);
        m
    }

    // --- RigidBody::new / Default ---

    #[test]
    fn new_matches_header_and_constructor_defaults() {
        let rb = RigidBody::new();
        assert_eq!(rb.linear_velocity, Vec3::new(0.0, 0.0, 0.0));
        assert_eq!(rb.angular_velocity, 0.0);
        assert_eq!(rb.transform_index, 0);
        assert!(!rb.lerp_translation);
        assert!(!rb.lerp_rotation);
        assert!(!rb.lerp_scale);
        assert!(!rb.lerp_skew);
        assert!(!rb.lerp_perspective);
        assert!(rb.lerp_decompose);
        assert!(!rb.transforms[0].valid);
        assert!(!rb.transforms[1].valid);
    }

    #[test]
    fn default_matches_new() {
        assert_eq!(RigidBody::default(), RigidBody::new());
    }

    // --- update_linear ---

    #[test]
    fn update_linear_no_motion_zero_velocity_is_below_tolerance_lerps() {
        // prev == cur -> curLinearVelocity = 0. prevVelMag = length(0) = 0
        // (linear_velocity starts at 0 too). dotCurVel = max(dot(0/eps,
        // 0/eps), eps) = max(0, eps) = eps. curVelMag = 0 / eps = 0.
        // 0 < VelocityTolerance(5.0) -> true.
        let mut rb = RigidBody::new();
        let m = translated(1.0, 2.0, 3.0);
        rb.update_linear(m, m, G_EX_COMPONENT_AUTO);
        assert!(rb.lerp_translation);
        assert_eq!(rb.linear_velocity, Vec3::new(0.0, 0.0, 0.0));
    }

    #[test]
    fn update_linear_small_translation_lerps_true() {
        let mut rb = RigidBody::new();
        let prev = translated(0.0, 0.0, 0.0);
        let cur = translated(1.0, 0.0, 0.0);
        rb.update_linear(prev, cur, G_EX_COMPONENT_AUTO);
        // curLinearVelocity = (1,0,0), magnitude 1.0, well under
        // VelocityTolerance (5.0) at any dotCurVel <= 1 boundary here since
        // prevVelMag = 0 (first call) forces prev_vel_norm = 0/eps = 0, so
        // dotCurVel = max(dot(0, cur_norm), eps) = eps -> curVelMag =
        // 1.0/eps, a huge number, NOT < 5.0. Fall through to the second
        // clause: (curVelMag / max(prevVelMag, eps)) = (1e6ish / eps) which
        // is enormous, NOT < 10.0 either -- so lerp_translation is false on
        // this very first call from a zero rest velocity. This exercises
        // the "prevVelMag == 0" edge explicitly rather than assuming small
        // motion always lerps.
        assert!(!rb.lerp_translation);
        assert_eq!(rb.linear_velocity, Vec3::new(1.0, 0.0, 0.0));
    }

    #[test]
    fn update_linear_second_call_same_direction_velocity_lerps_true() {
        // Seed linear_velocity with a first call, then a second call with
        // the same direction and magnitude gives dotCurVel ~= 1 (parallel
        // unit vectors), so curVelMag ~= prevVelMag ~= 1.0, well under both
        // thresholds -> lerp_translation = true.
        let mut rb = RigidBody::new();
        let a = translated(0.0, 0.0, 0.0);
        let b = translated(1.0, 0.0, 0.0);
        let c = translated(2.0, 0.0, 0.0);
        rb.update_linear(a, b, G_EX_COMPONENT_AUTO);
        rb.update_linear(b, c, G_EX_COMPONENT_AUTO);
        assert!(rb.lerp_translation);
        assert_eq!(rb.linear_velocity, Vec3::new(1.0, 0.0, 0.0));
    }

    #[test]
    fn update_linear_large_velocity_jump_lerps_false() {
        // First call establishes velocity (1,0,0). Second call jumps to a
        // much larger, same-direction velocity (100,0,0): dotCurVel ~= 1
        // (still parallel), curVelMag ~= 100, both thresholds exceeded ->
        // lerp_translation = false.
        let mut rb = RigidBody::new();
        let a = translated(0.0, 0.0, 0.0);
        let b = translated(1.0, 0.0, 0.0);
        let c = translated(101.0, 0.0, 0.0);
        rb.update_linear(a, b, G_EX_COMPONENT_AUTO);
        rb.update_linear(b, c, G_EX_COMPONENT_AUTO);
        assert!(!rb.lerp_translation);
        assert_eq!(rb.linear_velocity, Vec3::new(100.0, 0.0, 0.0));
    }

    #[test]
    fn update_linear_opposite_direction_velocity_flip_reduces_dot_and_inflates_mag() {
        // First call: velocity (1,0,0). Second call: velocity flips to
        // (-1,0,0) (a reversal). dotCurVel = dot((1,0,0),(-1,0,0)) = -1,
        // max(-1, eps) = eps (since eps > -1). curVelMag = 1.0/eps, a huge
        // number -> NOT < VelocityTolerance, NOT < MagnitudeThreshold either
        // after the second division -> lerp_translation = false.
        let mut rb = RigidBody::new();
        let a = translated(0.0, 0.0, 0.0);
        let b = translated(1.0, 0.0, 0.0);
        let c = translated(0.0, 0.0, 0.0);
        rb.update_linear(a, b, G_EX_COMPONENT_AUTO);
        rb.update_linear(b, c, G_EX_COMPONENT_AUTO);
        assert!(!rb.lerp_translation);
        assert_eq!(rb.linear_velocity, Vec3::new(-1.0, 0.0, 0.0));
    }

    #[test]
    fn update_linear_non_auto_interpolate_sets_true_and_zeroes_velocity() {
        let mut rb = RigidBody::new();
        rb.linear_velocity = Vec3::new(5.0, 5.0, 5.0);
        let m = translated(1.0, 1.0, 1.0);
        rb.update_linear(identity(), m, G_EX_COMPONENT_INTERPOLATE);
        assert!(rb.lerp_translation);
        assert_eq!(rb.linear_velocity, Vec3::new(0.0, 0.0, 0.0));
    }

    #[test]
    fn update_linear_non_auto_skip_sets_false_and_zeroes_velocity() {
        let mut rb = RigidBody::new();
        rb.linear_velocity = Vec3::new(5.0, 5.0, 5.0);
        let m = translated(1.0, 1.0, 1.0);
        rb.update_linear(identity(), m, crate::rt64_extended_gbi::G_EX_COMPONENT_SKIP);
        assert!(!rb.lerp_translation);
        assert_eq!(rb.linear_velocity, Vec3::new(0.0, 0.0, 0.0));
    }

    #[test]
    fn update_linear_nan_position_propagates_to_lerp_translation() {
        let mut rb = RigidBody::new();
        let prev = translated(0.0, 0.0, 0.0);
        let cur = translated(f32::NAN, 0.0, 0.0);
        rb.update_linear(prev, cur, G_EX_COMPONENT_AUTO);
        // curLinearVelocity.x is NaN; any comparison against it is false,
        // so `<` in the first clause is false; the second clause's division
        // also propagates NaN, also comparing false. lerp_translation ends
        // up false (both `||` operands are false-under-NaN), not panicking.
        assert!(!rb.lerp_translation);
        assert!(rb.linear_velocity.x.is_nan());
    }

    #[test]
    fn update_linear_infinite_position_yields_infinite_velocity() {
        let mut rb = RigidBody::new();
        let prev = translated(0.0, 0.0, 0.0);
        let cur = translated(f32::INFINITY, 0.0, 0.0);
        rb.update_linear(prev, cur, G_EX_COMPONENT_AUTO);
        assert_eq!(rb.linear_velocity.x, f32::INFINITY);
        // length(inf,0,0) = inf; prevVelMag = 0. dot(0/eps, inf/inf=NaN...)
        // propagates NaN through dotCurVel -> curVelMag division is NaN;
        // NaN comparisons are false -> lerp_translation false.
        assert!(!rb.lerp_translation);
    }

    // --- update_angular ---

    #[test]
    fn update_angular_identity_to_identity_zero_angle_lerps_true_always() {
        // FIXME upstream: lerp_rotation is unconditionally set true in the
        // AUTO branch regardless of the computed angular velocity.
        let mut rb = RigidBody::new();
        rb.update_angular(
            identity(),
            identity(),
            G_EX_COMPONENT_AUTO,
            G_EX_COMPONENT_SKIP_CONST,
            G_EX_COMPONENT_SKIP_CONST,
        );
        assert!(rb.lerp_rotation);
        // diffRotation = inverse(I) * I = I; trace = 3; acos((3-1)/2) =
        // acos(1) = 0.0.
        assert!((rb.angular_velocity - 0.0).abs() < 1e-5);
        assert!(!rb.lerp_scale);
        assert!(!rb.lerp_skew);
    }

    #[test]
    fn update_angular_90_degree_rotation_about_z_hand_computed_angle() {
        // Rotate the x/y basis rows by 90 degrees about Z: row0=(0,1,0),
        // row1=(-1,0,0), row2=(0,0,1). This is already unit-row and
        // orthonormal, so rotation_from_3x3 leaves it unchanged.
        let mut cur = identity();
        cur.rows[0] = fn64_render_ir::Vec4::new(0.0, 1.0, 0.0, 0.0);
        cur.rows[1] = fn64_render_ir::Vec4::new(-1.0, 0.0, 0.0, 0.0);
        cur.rows[2] = fn64_render_ir::Vec4::new(0.0, 0.0, 1.0, 0.0);

        let mut rb = RigidBody::new();
        rb.update_angular(
            identity(),
            cur,
            G_EX_COMPONENT_AUTO,
            G_EX_COMPONENT_SKIP_CONST,
            G_EX_COMPONENT_SKIP_CONST,
        );
        // trace(diffRotation) = trace(I^-1 * R) = trace(R) = 0+0+1 = 1.
        // acos((1-1)/2) = acos(0) = pi/2.
        assert!((rb.angular_velocity - std::f32::consts::FRAC_PI_2).abs() < 1e-4);
        assert!(rb.lerp_rotation);
    }

    #[test]
    fn update_angular_scale_auto_inherits_rotation_auto_result() {
        let mut rb = RigidBody::new();
        rb.update_angular(
            identity(),
            identity(),
            G_EX_COMPONENT_AUTO,
            G_EX_COMPONENT_AUTO,
            G_EX_COMPONENT_AUTO,
        );
        assert!(rb.lerp_rotation);
        assert_eq!(rb.lerp_scale, rb.lerp_rotation);
        assert_eq!(rb.lerp_skew, rb.lerp_rotation);
    }

    #[test]
    fn update_angular_scale_interpolate_independent_of_rotation_auto() {
        let mut rb = RigidBody::new();
        rb.update_angular(
            identity(),
            identity(),
            G_EX_COMPONENT_AUTO,
            G_EX_COMPONENT_INTERPOLATE,
            crate::rt64_extended_gbi::G_EX_COMPONENT_SKIP,
        );
        // scale_interpolation != AUTO, so lerp_scale keeps its pre-AUTO-block
        // value: (scale_interpolation == INTERPOLATE) = true.
        assert!(rb.lerp_scale);
        // skew_interpolation == SKIP -> lerp_skew = false, and stays false
        // since skew_interpolation != AUTO.
        assert!(!rb.lerp_skew);
    }

    #[test]
    fn update_angular_non_auto_rotation_interpolate_sets_true_zero_velocity() {
        let mut rb = RigidBody::new();
        rb.angular_velocity = 99.0;
        rb.update_angular(
            identity(),
            identity(),
            G_EX_COMPONENT_INTERPOLATE,
            crate::rt64_extended_gbi::G_EX_COMPONENT_SKIP,
            crate::rt64_extended_gbi::G_EX_COMPONENT_SKIP,
        );
        assert!(rb.lerp_rotation);
        assert_eq!(rb.angular_velocity, 0.0);
    }

    #[test]
    fn update_angular_non_auto_rotation_skip_sets_false_zero_velocity() {
        let mut rb = RigidBody::new();
        rb.angular_velocity = 99.0;
        rb.update_angular(
            identity(),
            identity(),
            crate::rt64_extended_gbi::G_EX_COMPONENT_SKIP,
            crate::rt64_extended_gbi::G_EX_COMPONENT_SKIP,
            crate::rt64_extended_gbi::G_EX_COMPONENT_SKIP,
        );
        assert!(!rb.lerp_rotation);
        assert_eq!(rb.angular_velocity, 0.0);
    }

    #[test]
    fn update_angular_non_auto_scale_and_skew_read_before_rotation_branch() {
        // lerp_scale/lerp_skew are set from scale_interpolation/
        // skew_interpolation BEFORE the rotInterpolation branch runs, and
        // are untouched by the non-AUTO rotation `else` branch.
        let mut rb = RigidBody::new();
        rb.update_angular(
            identity(),
            identity(),
            crate::rt64_extended_gbi::G_EX_COMPONENT_SKIP,
            G_EX_COMPONENT_INTERPOLATE,
            G_EX_COMPONENT_INTERPOLATE,
        );
        assert!(rb.lerp_scale);
        assert!(rb.lerp_skew);
        assert!(!rb.lerp_rotation);
    }

    #[test]
    fn update_angular_degenerate_zero_row_yields_nan_angular_velocity() {
        // A prevTransform with a zero row makes rotation_from_3x3 divide by
        // zero (0/0 = NaN), propagating through mat3_mul/trace/acos.
        let mut degenerate = identity();
        degenerate.rows[0] = fn64_render_ir::Vec4::new(0.0, 0.0, 0.0, 0.0);
        let mut rb = RigidBody::new();
        rb.update_angular(
            degenerate,
            identity(),
            G_EX_COMPONENT_AUTO,
            G_EX_COMPONENT_SKIP_CONST,
            G_EX_COMPONENT_SKIP_CONST,
        );
        assert!(rb.angular_velocity.is_nan());
        // lerp_rotation is still unconditionally set true in the AUTO
        // branch, even though angular_velocity is NaN (the FIXME'd
        // always-true assignment does not depend on the computed value).
        assert!(rb.lerp_rotation);
    }

    #[test]
    fn update_angular_180_degree_rotation_trace_below_minus_one_domain() {
        // A 180-degree rotation about Z: row0=(-1,0,0), row1=(0,-1,0),
        // row2=(0,0,1). trace(R) = -1-1+1 = -1. acos((-1-1)/2) = acos(-1) =
        // pi -- exactly at the acos domain boundary, not NaN.
        let mut cur = identity();
        cur.rows[0] = fn64_render_ir::Vec4::new(-1.0, 0.0, 0.0, 0.0);
        cur.rows[1] = fn64_render_ir::Vec4::new(0.0, -1.0, 0.0, 0.0);
        let mut rb = RigidBody::new();
        rb.update_angular(
            identity(),
            cur,
            G_EX_COMPONENT_AUTO,
            G_EX_COMPONENT_SKIP_CONST,
            G_EX_COMPONENT_SKIP_CONST,
        );
        assert!((rb.angular_velocity - std::f32::consts::PI).abs() < 1e-4);
    }

    // --- update_perspective ---

    #[test]
    fn update_perspective_interpolate_sets_true() {
        let mut rb = RigidBody::new();
        rb.update_perspective(identity(), identity(), G_EX_COMPONENT_INTERPOLATE);
        assert!(rb.lerp_perspective);
    }

    #[test]
    fn update_perspective_skip_sets_false() {
        let mut rb = RigidBody::new();
        rb.lerp_perspective = true;
        rb.update_perspective(
            identity(),
            identity(),
            crate::rt64_extended_gbi::G_EX_COMPONENT_SKIP,
        );
        assert!(!rb.lerp_perspective);
    }

    #[test]
    fn update_perspective_auto_is_treated_as_not_interpolate() {
        // updatePerspective has no AUTO branch at all -- G_EX_COMPONENT_AUTO
        // simply fails the `== INTERPOLATE` check, same as SKIP.
        let mut rb = RigidBody::new();
        rb.lerp_perspective = true;
        rb.update_perspective(identity(), identity(), G_EX_COMPONENT_AUTO);
        assert!(!rb.lerp_perspective);
    }

    #[test]
    fn update_perspective_ignores_transform_arguments() {
        let mut rb1 = RigidBody::new();
        let mut rb2 = RigidBody::new();
        rb1.update_perspective(identity(), identity(), G_EX_COMPONENT_INTERPOLATE);
        rb2.update_perspective(
            translated(1.0, 2.0, 3.0),
            translated(-9.0, 9.0, 9.0),
            G_EX_COMPONENT_INTERPOLATE,
        );
        assert_eq!(rb1.lerp_perspective, rb2.lerp_perspective);
    }

    // --- update_decomposition ---

    #[test]
    fn update_decomposition_toggles_index_and_stores_decomposed_transform() {
        let mut rb = RigidBody::new();
        assert_eq!(rb.transform_index, 0);
        rb.update_decomposition(translated(1.0, 2.0, 3.0), true);
        assert_eq!(rb.transform_index, 1);
        assert!(rb.transforms[1].valid);
        assert!(!rb.transforms[0].valid);
        assert_eq!(rb.transforms[1].translation, Vec3::new(1.0, 2.0, 3.0));
        assert!(rb.lerp_decompose);

        rb.update_decomposition(translated(4.0, 5.0, 6.0), true);
        assert_eq!(rb.transform_index, 0);
        assert!(rb.transforms[0].valid);
        assert_eq!(rb.transforms[0].translation, Vec3::new(4.0, 5.0, 6.0));
        // Slot 1 from the previous call is untouched.
        assert_eq!(rb.transforms[1].translation, Vec3::new(1.0, 2.0, 3.0));
    }

    #[test]
    fn update_decomposition_false_stores_invalid_default_transform() {
        let mut rb = RigidBody::new();
        rb.update_decomposition(translated(9.0, 9.0, 9.0), false);
        assert!(!rb.transforms[1].valid);
        assert!(!rb.lerp_decompose);
        assert_eq!(rb.transforms[1], DecomposedTransform::default());
    }

    #[test]
    fn update_decomposition_singular_matrix_yields_invalid_even_when_requested() {
        // A matrix with m[3][3] == 0 fails decomposeMatrix's own
        // epsilon-zero guard and returns invalid, even though `decompose`
        // was requested as true.
        let mut singular = identity();
        singular.rows[3].w = 0.0;
        let mut rb = RigidBody::new();
        rb.update_decomposition(singular, true);
        assert!(!rb.transforms[1].valid);
        // lerp_decompose still reflects the caller's request, not the
        // decomposition's success -- ported literally from the source's
        // unconditional `lerpDecompose = decompose;` at the end.
        assert!(rb.lerp_decompose);
    }

    // --- lerp (fallback / non-decomposed path) ---

    #[test]
    fn lerp_falls_back_when_lerp_decompose_false() {
        let mut rb = RigidBody::new();
        rb.lerp_decompose = false;
        rb.lerp_translation = true;
        let prev = translated(0.0, 0.0, 0.0);
        let cur = translated(10.0, 0.0, 0.0);
        let r = rb.lerp(0.5, prev, cur, false);
        // lerp_matrix_components with only `linear` set lerps row 3's xyz;
        // angular=false leaves rows 0-2 as cur's (already identity in both).
        assert!((r.rows[3].x - 5.0).abs() < 1e-4);
    }

    #[test]
    fn lerp_falls_back_when_transforms_not_yet_populated() {
        // Fresh RigidBody: both transforms[0]/[1] start invalid even though
        // lerp_decompose defaults to true.
        let rb = RigidBody::new();
        let prev = translated(0.0, 0.0, 0.0);
        let cur = translated(10.0, 0.0, 0.0);
        let r = rb.lerp(0.5, prev, cur, false);
        // Falls back to lerp_matrix_components with all lerp_* flags false
        // (defaults) -> exactly `cur` (fallback path returns `b` verbatim
        // when linear/angular/perspective are all false).
        assert_eq!(r, cur);
    }

    #[test]
    fn lerp_fallback_at_weight_zero_and_one() {
        let mut rb = RigidBody::new();
        rb.lerp_decompose = false;
        rb.lerp_translation = true;
        let prev = translated(1.0, 2.0, 3.0);
        let cur = translated(9.0, 9.0, 9.0);
        let r0 = rb.lerp(0.0, prev, cur, false);
        assert!((r0.rows[3].x - 1.0).abs() < 1e-4);
        let r1 = rb.lerp(1.0, prev, cur, false);
        assert!((r1.rows[3].x - 9.0).abs() < 1e-4);
    }

    // --- lerp (decomposed path, no coordinate flip) ---

    #[test]
    fn lerp_decomposed_path_pure_translation_interpolates_linearly() {
        let mut rb = RigidBody::new();
        rb.lerp_translation = true;
        rb.update_decomposition(translated(0.0, 0.0, 0.0), true);
        rb.update_decomposition(translated(10.0, 0.0, 0.0), true);
        assert!(rb.transforms[0].valid && rb.transforms[1].valid);
        let r = rb.lerp(0.5, identity(), identity(), false);
        assert!((r.rows[3].x - 5.0).abs() < 1e-3, "row3.x={}", r.rows[3].x);
    }

    #[test]
    fn lerp_decomposed_path_no_motion_identity_stays_identity() {
        let mut rb = RigidBody::new();
        rb.lerp_translation = true;
        rb.lerp_rotation = true;
        rb.lerp_scale = true;
        rb.lerp_skew = true;
        rb.lerp_perspective = true;
        rb.update_decomposition(identity(), true);
        rb.update_decomposition(identity(), true);
        let r = rb.lerp(0.5, identity(), identity(), false);
        for i in 0..4 {
            assert!(
                (r.rows[i].x - identity().rows[i].x).abs() < 1e-3,
                "row {i} x"
            );
            assert!(
                (r.rows[i].y - identity().rows[i].y).abs() < 1e-3,
                "row {i} y"
            );
            assert!(
                (r.rows[i].z - identity().rows[i].z).abs() < 1e-3,
                "row {i} z"
            );
            assert!(
                (r.rows[i].w - identity().rows[i].w).abs() < 1e-3,
                "row {i} w"
            );
        }
    }

    #[test]
    fn lerp_decomposed_path_at_weight_zero_and_one_matches_endpoints() {
        let mut rb = RigidBody::new();
        rb.lerp_translation = true;
        rb.update_decomposition(translated(1.0, 2.0, 3.0), true);
        rb.update_decomposition(translated(9.0, 9.0, 9.0), true);
        let r0 = rb.lerp(0.0, identity(), identity(), false);
        assert!((r0.rows[3].x - 1.0).abs() < 1e-3);
        let r1 = rb.lerp(1.0, identity(), identity(), false);
        assert!((r1.rows[3].x - 9.0).abs() < 1e-3);
    }

    #[test]
    fn lerp_decomposed_path_negative_weight_extrapolates() {
        let mut rb = RigidBody::new();
        rb.lerp_translation = true;
        rb.update_decomposition(translated(0.0, 0.0, 0.0), true);
        rb.update_decomposition(translated(10.0, 0.0, 0.0), true);
        let r = rb.lerp(-1.0, identity(), identity(), false);
        // lerp(a=0, b=10, t=-1) = 0 + (-1)*(10-0) = -10.0.
        assert!(
            (r.rows[3].x - (-10.0)).abs() < 1e-3,
            "row3.x={}",
            r.rows[3].x
        );
    }

    #[test]
    fn lerp_decomposed_path_slerp_and_lerp_agree_for_small_rotation() {
        // For a small rotation angle, slerp and linear-quat-lerp (then
        // normalize) should agree closely -- exercising both code paths
        // through the same decomposed transforms.
        let mut cur = identity();
        let angle: f32 = 0.05;
        cur.rows[0] = fn64_render_ir::Vec4::new(angle.cos(), angle.sin(), 0.0, 0.0);
        cur.rows[1] = fn64_render_ir::Vec4::new(-angle.sin(), angle.cos(), 0.0, 0.0);

        let mut rb_lerp = RigidBody::new();
        rb_lerp.lerp_rotation = true;
        rb_lerp.update_decomposition(identity(), true);
        rb_lerp.update_decomposition(cur, true);
        let r_lerp = rb_lerp.lerp(0.5, identity(), identity(), false);

        let mut rb_slerp = RigidBody::new();
        rb_slerp.lerp_rotation = true;
        rb_slerp.update_decomposition(identity(), true);
        rb_slerp.update_decomposition(cur, true);
        let r_slerp = rb_slerp.lerp(0.5, identity(), identity(), true);

        assert!((r_lerp.rows[0].x - r_slerp.rows[0].x).abs() < 1e-3);
        assert!((r_lerp.rows[0].y - r_slerp.rows[0].y).abs() < 1e-3);
    }

    // --- lerp (decomposed path, coordinate flip bias branch) ---

    #[test]
    fn lerp_coordinate_flip_bias_branch_triggers_on_mismatched_flip() {
        // Build a prev transform with a positive-determinant rotation
        // (coordinate_flip = false) and a cur transform with a mirrored
        // (negative-determinant) one (coordinate_flip = true), forcing the
        // bias branch to run. The exact numeric result is not asserted
        // (it depends on the unpinned quat_mul/rotation_axis formulas) --
        // only that the branch executes without panicking and produces a
        // finite, valid matrix.
        let prev = identity();
        let mut cur = identity();
        // Mirror the X axis: determinant becomes -1.
        cur.rows[0] = fn64_render_ir::Vec4::new(-1.0, 0.0, 0.0, 0.0);

        let mut rb = RigidBody::new();
        rb.lerp_translation = true;
        rb.lerp_rotation = true;
        rb.lerp_scale = true;
        rb.update_decomposition(prev, true);
        rb.update_decomposition(cur, true);
        assert_ne!(
            rb.transforms[0].coordinate_flip,
            rb.transforms[1].coordinate_flip
        );

        let r = rb.lerp(0.5, identity(), identity(), false);
        for row in r.rows {
            assert!(row.x.is_finite());
            assert!(row.y.is_finite());
            assert!(row.z.is_finite());
            assert!(row.w.is_finite());
        }
    }

    #[test]
    fn lerp_no_bias_branch_when_flips_match() {
        // Both transforms have the same (false) coordinate_flip, so the
        // bias branch's condition `prevTransformCopy.coordinateFlip !=
        // curTransform.coordinateFlip` is false -- prev_transform_copy's
        // rotation/scale are never touched by the branch, only by
        // lerp_transforms afterward.
        let prev = identity();
        let cur = translated(5.0, 0.0, 0.0);
        let mut rb = RigidBody::new();
        rb.lerp_translation = true;
        rb.update_decomposition(prev, true);
        rb.update_decomposition(cur, true);
        assert_eq!(
            rb.transforms[0].coordinate_flip,
            rb.transforms[1].coordinate_flip
        );
        let r = rb.lerp(0.5, identity(), identity(), false);
        assert!((r.rows[3].x - 2.5).abs() < 1e-3);
    }

    // --- helper function unit tests (vec3_length / mat3_mul / mat3_inverse / quat_mul / quat_rotation_axis) ---

    #[test]
    fn vec3_length_pythagorean() {
        assert!((vec3_length(Vec3::new(3.0, 4.0, 0.0)) - 5.0).abs() < 1e-6);
    }

    #[test]
    fn vec3_length_zero_is_zero() {
        assert_eq!(vec3_length(Vec3::new(0.0, 0.0, 0.0)), 0.0);
    }

    #[test]
    fn vec3_length_nan_component_propagates() {
        assert!(vec3_length(Vec3::new(f32::NAN, 0.0, 0.0)).is_nan());
    }

    #[test]
    fn mat3_mul_identity_is_identity() {
        let id = Mat3 {
            rows: [
                Vec3::new(1.0, 0.0, 0.0),
                Vec3::new(0.0, 1.0, 0.0),
                Vec3::new(0.0, 0.0, 1.0),
            ],
        };
        let r = mat3_mul(id, id);
        assert_eq!(r, id);
    }

    #[test]
    fn mat3_mul_hand_computed() {
        let a = Mat3 {
            rows: [
                Vec3::new(1.0, 2.0, 3.0),
                Vec3::new(4.0, 5.0, 6.0),
                Vec3::new(7.0, 8.0, 9.0),
            ],
        };
        let id = Mat3 {
            rows: [
                Vec3::new(1.0, 0.0, 0.0),
                Vec3::new(0.0, 1.0, 0.0),
                Vec3::new(0.0, 0.0, 1.0),
            ],
        };
        let r = mat3_mul(a, id);
        assert_eq!(r, a);
    }

    #[test]
    fn mat3_inverse_identity_is_identity() {
        let id = Mat3 {
            rows: [
                Vec3::new(1.0, 0.0, 0.0),
                Vec3::new(0.0, 1.0, 0.0),
                Vec3::new(0.0, 0.0, 1.0),
            ],
        };
        let inv = mat3_inverse(id);
        assert!((inv.rows[0].x - 1.0).abs() < 1e-5);
        assert!((inv.rows[1].y - 1.0).abs() < 1e-5);
        assert!((inv.rows[2].z - 1.0).abs() < 1e-5);
    }

    #[test]
    fn mat3_inverse_times_original_is_identity() {
        let m = Mat3 {
            rows: [
                Vec3::new(2.0, 0.0, 0.0),
                Vec3::new(0.0, 3.0, 0.0),
                Vec3::new(0.0, 0.0, 4.0),
            ],
        };
        let inv = mat3_inverse(m);
        let prod = mat3_mul(inv, m);
        assert!((prod.rows[0].x - 1.0).abs() < 1e-4);
        assert!((prod.rows[1].y - 1.0).abs() < 1e-4);
        assert!((prod.rows[2].z - 1.0).abs() < 1e-4);
        assert!(prod.rows[0].y.abs() < 1e-4);
    }

    #[test]
    fn mat3_inverse_singular_matrix_divides_by_zero_unguarded() {
        let zero = Mat3 {
            rows: [
                Vec3::new(0.0, 0.0, 0.0),
                Vec3::new(0.0, 0.0, 0.0),
                Vec3::new(0.0, 0.0, 0.0),
            ],
        };
        let inv = mat3_inverse(zero);
        // det = 0 -> 0/0 = NaN for every entry.
        assert!(inv.rows[0].x.is_nan());
    }

    #[test]
    fn quat_mul_identity_is_left_operand() {
        let id = Quat::new(0.0, 0.0, 0.0, 1.0);
        let q = Quat::new(0.1, 0.2, 0.3, 0.9);
        let r = quat_mul(q, id);
        assert!((r.x - q.x).abs() < 1e-6);
        assert!((r.y - q.y).abs() < 1e-6);
        assert!((r.z - q.z).abs() < 1e-6);
        assert!((r.w - q.w).abs() < 1e-6);
    }

    #[test]
    fn quat_rotation_axis_zero_angle_is_identity_quat() {
        let q = quat_rotation_axis(Vec3::new(1.0, 0.0, 0.0), 0.0);
        assert!((q.x - 0.0).abs() < 1e-6);
        assert!((q.w - 1.0).abs() < 1e-6);
    }

    #[test]
    fn quat_rotation_axis_pi_about_x_hand_computed() {
        // angle=pi: half=pi/2, sin(pi/2)=1, cos(pi/2)~=0.
        let q = quat_rotation_axis(Vec3::new(1.0, 0.0, 0.0), std::f32::consts::PI);
        assert!((q.x - 1.0).abs() < 1e-5);
        assert!(q.y.abs() < 1e-5);
        assert!(q.z.abs() < 1e-5);
        assert!(q.w.abs() < 1e-5);
    }

    // Local helper: mirrors the values `G_EX_COMPONENT_SKIP` names, spelled
    // out at the call sites above without importing the constant a second
    // time under a different local name.
    const G_EX_COMPONENT_SKIP_CONST: u32 = crate::rt64_extended_gbi::G_EX_COMPONENT_SKIP;
}
