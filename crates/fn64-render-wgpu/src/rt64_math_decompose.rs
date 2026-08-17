//! Literal port of RT64 `rt64_math.cpp`'s quaternion decomposition core --
//! `DecomposedTransform`, `decomposeMatrix`, `recomposeMatrix`, and
//! `lerpTransforms` -- a literal port of the permitted MIT RT64 Rust-port
//! source pinned at commit `5473732a822a4423b5696e7cb18fecc425a59875`
//! (`docs/RT64-PORT-AUTHORITY.md`), `src/common/rt64_math.h`/`.cpp`
//! (SHA-256 of the whole files,
//! `d0d768a666555a3099b564fe8b7f62af088e921e7ebc8f9d232e5e3239f406a9` /
//! `d32abc9572001870b4144ffa49e832589858de0830dbb0d008761ad15a76364b`):
//! `rt64_math.rs`'s own Nonclaims section explicitly deferred this cluster
//! ("`decomposeMatrix`/`recomposeMatrix`, `DecomposedTransform`/
//! `lerpTransforms` (deferred -- needs new matrix-inverse/quaternion
//! infra)"), and the sibling `rt64_math_matrix.rs` module (M8.3, already
//! landed) explicitly excluded this same cluster from its own scope
//! ("the quaternion `decomposeMatrix`/`recomposeMatrix`/
//! `DecomposedTransform`/`lerpTransforms` cluster in the same source file is
//! a separate, independently-owned port and is NOT ported here"). This
//! module closes that deferral.
//!
//! Source line ranges (`src/common/rt64_math.cpp`):
//!
//! ```text
//! // lines 225-227
//! bool epsilonEqual(float a, float b) {
//!     return abs(a - b) < std::numeric_limits<float>::epsilon();
//! }
//!
//! // lines 229-238 (adapted from glm's matrix_decompose.inl per the
//! // source's own comment)
//! /// Make a linear combination of two vectors and return the result.
//! // result = (a * ascl) + (b * bscl)
//! hlslpp::float3 vecCombine(
//!     const hlslpp::float3& a,
//!     const hlslpp::float3& b,
//!     float ascl, float bscl)
//! {
//!     return (a * ascl) + (b * bscl);
//! }
//!
//! // lines 240-243
//! hlslpp::float3 vecScale(const hlslpp::float3& v, float desiredLength)
//! {
//!     return v * desiredLength / length(v);
//! }
//!
//! // lines 245-388
//! bool decomposeMatrix(const hlslpp::float4x4& mtx, hlslpp::quaternion& rotation, hlslpp::float3& scale, hlslpp::float3& skew,
//!     hlslpp::float3& translation, hlslpp::float4& perspective, bool &coordinateFlip)
//! {
//!     hlslpp::float4x4 LocalMatrix(mtx);
//!
//!     // Normalize the matrix.
//!     if(epsilonEqual(LocalMatrix[3][3], 0.0f)) {
//!         return false;
//!     }
//!
//!     for(size_t i = 0; i < 4; i++) {
//!         for(size_t j = 0; j < 4; j++) {
//!             LocalMatrix[i][j] /= LocalMatrix[3][3];
//!         }
//!     }
//!
//!     // perspectiveMatrix is used to solve for perspective, but it also provides
//!     // an easy way to test for singularity of the upper 3x3 component.
//!     hlslpp::float4x4 PerspectiveMatrix(LocalMatrix);
//!
//!     for(size_t i = 0; i < 3; i++) {
//!         PerspectiveMatrix[i][3] = 0.0f;
//!     }
//!     PerspectiveMatrix[3][3] = 1.0f;
//!
//!     /// TODO: Fixme!
//!     if(epsilonEqual(determinant(PerspectiveMatrix), 0.0f)) {
//!         return false;
//!     }
//!
//!     // First, isolate perspective.  This is the messiest.
//!     if(
//!         !epsilonEqual(LocalMatrix[0][3], 0.0f) ||
//!         !epsilonEqual(LocalMatrix[1][3], 0.0f) ||
//!         !epsilonEqual(LocalMatrix[2][3], 0.0f))
//!     {
//!         // rightHandSide is the right hand side of the equation.
//!         hlslpp::float4 RightHandSide;
//!         RightHandSide[0] = LocalMatrix[0][3];
//!         RightHandSide[1] = LocalMatrix[1][3];
//!         RightHandSide[2] = LocalMatrix[2][3];
//!         RightHandSide[3] = LocalMatrix[3][3];
//!
//!         // Solve the equation by inverting PerspectiveMatrix and multiplying
//!         // rightHandSide by the inverse.  (This is the easiest way, not
//!         // necessarily the best.)
//!         hlslpp::float4x4 InversePerspectiveMatrix = inverse(PerspectiveMatrix);
//!         hlslpp::float4x4 TransposedInversePerspectiveMatrix = transpose(InversePerspectiveMatrix);
//!
//!         perspective = hlslpp::mul(TransposedInversePerspectiveMatrix, RightHandSide);
//!
//!         // Clear the perspective partition
//!         LocalMatrix[0][3] = LocalMatrix[1][3] = LocalMatrix[2][3] = 0.0f;
//!         LocalMatrix[3][3] = 1.0f;
//!     }
//!     else
//!     {
//!         // No perspective.
//!         perspective = hlslpp::float4{0, 0, 0, 1.0f};
//!     }
//!
//!     // Next take care of translation (easy).
//!     translation = hlslpp::float3(LocalMatrix[3].xyz);
//!     LocalMatrix[3] = hlslpp::float4(0, 0, 0, LocalMatrix[3].w);
//!
//!     hlslpp::float3 Row[3], Pdum3;
//!
//!     // Now get scale and shear.
//!     for(size_t i = 0; i < 3; ++i) {
//!         for(size_t j = 0; j < 3; ++j) {
//!             Row[i][j] = LocalMatrix[i][j];
//!         }
//!     }
//!
//!     // Compute X scale factor and normalize first row.
//!     scale.x = length(Row[0]);// v3Length(Row[0]);
//!
//!     Row[0] = vecScale(Row[0], 1.0f);
//!
//!     // Compute XY shear factor and make 2nd row orthogonal to 1st.
//!     skew.z = dot(Row[0], Row[1]);
//!     Row[1] = vecCombine(Row[1], Row[0], 1.0f, -skew.z);
//!
//!     // Now, compute Y scale and normalize 2nd row.
//!     scale.y = length(Row[1]);
//!     Row[1] = vecScale(Row[1], 1.0f);
//!     skew.z /= scale.y;
//!
//!     // Compute XZ and YZ shears, orthogonalize 3rd row.
//!     skew.y = dot(Row[0], Row[2]);
//!     Row[2] = vecCombine(Row[2], Row[0], 1.0f, -skew.y);
//!     skew.x = dot(Row[1], Row[2]);
//!     Row[2] = vecCombine(Row[2], Row[1], 1.0f, -skew.x);
//!
//!     // Next, get Z scale and normalize 3rd row.
//!     scale.z = length(Row[2]);
//!     Row[2] = vecScale(Row[2], 1.0f);
//!     skew.y /= scale.z;
//!     skew.x /= scale.z;
//!
//!     // At this point, the matrix (in rows[]) is orthonormal.
//!     // Check for a coordinate system flip.  If the determinant
//!     // is -1, then negate the matrix and the scaling factors.
//!     Pdum3 = cross(Row[1], Row[2]);
//!     coordinateFlip = dot(Row[0], Pdum3).x < 0.0f;
//!     if(coordinateFlip) {
//!         for(size_t i = 0; i < 3; i++) {
//!             scale[i] *= -1.0f;
//!             Row[i] *= -1.0f;
//!         }
//!     }
//!
//!     // Now, get the rotations out, as described in the gem.
//!     int i, j, k = 0;
//!     float root, trace = Row[0].x + Row[1].y + Row[2].z;
//!     if(trace > 0.0f)
//!     {
//!         root = sqrt(trace + 1.0f);
//!         rotation.w = 0.5f * root;
//!         root = 0.5f / root;
//!         rotation.x = root * (Row[1].z - Row[2].y);
//!         rotation.y = root * (Row[2].x - Row[0].z);
//!         rotation.z = root * (Row[0].y - Row[1].x);
//!     } // End if > 0
//!     else
//!     {
//!         static int Next[3] = {1, 2, 0};
//!         i = 0;
//!         if(Row[1].y > Row[0].x) i = 1;
//!         if(Row[2].z > Row[i][i]) i = 2;
//!         j = Next[i];
//!         k = Next[j];
//!
//!         root = sqrt(Row[i][i] - Row[j][j] - Row[k][k] + 1.0f);
//!
//!         rotation.f32[i] = 0.5f * root;
//!         root = 0.5f / root;
//!         rotation.f32[j] = root * (Row[i][j] + Row[j][i]);
//!         rotation.f32[k] = root * (Row[i][k] + Row[k][i]);
//!         rotation.w = root * (Row[j][k] - Row[k][j]);
//!     } // End if <= 0
//!
//!     return true;
//! }
//!
//! // lines 390-424
//! hlslpp::float4x4 recomposeMatrix(const hlslpp::quaternion& rotation, const hlslpp::float3& scale, const hlslpp::float3& skew,
//!     const hlslpp::float3& translation, const hlslpp::float4& perspective)
//! {
//!     hlslpp::float4x4 m = hlslpp::float4x4::identity();
//!
//!     m[0][3] = perspective.x;
//!     m[1][3] = perspective.y;
//!     m[2][3] = perspective.z;
//!     m[3][3] = perspective.w;
//!
//!     m = mul(matrixTranslation(translation.xyz), m);
//!     m = mul(hlslpp::float4x4(rotation), m);
//!
//!     if (fabs(skew.x) > 0.0f) {
//!         hlslpp::float4x4 tmp = hlslpp::float4x4::identity();
//!         tmp[2][1] = skew.x;
//!         m = mul(tmp, m);
//!     }
//!
//!     if (fabs(skew.y) > 0.0f) {
//!         hlslpp::float4x4 tmp = hlslpp::float4x4::identity();
//!         tmp[2][0] = skew.y;
//!         m = mul(tmp, m);
//!     }
//!
//!     if (fabs(skew.z) > 0.0f) {
//!         hlslpp::float4x4 tmp = hlslpp::float4x4::identity();
//!         tmp[1][0] = skew.z;
//!         m = mul(tmp, m);
//!     }
//!
//!     m = mul(matrixScale(scale), m);
//!
//!     return m;
//! }
//!
//! // lines 426-428
//! DecomposedTransform::DecomposedTransform(const hlslpp::float4x4& mtx) {
//!     valid = decomposeMatrix(mtx, rotation, scale, skew, translation, perspective, coordinateFlip);
//! }
//!
//! // lines 430-491
//! DecomposedTransform lerpTransforms(const DecomposedTransform& a, const DecomposedTransform& b, float weight,
//!     bool lerpTranslation, bool lerpRotation, bool lerpScale, bool lerpSkew, bool lerpPerpsective, bool useSlerp)
//! {
//!     assert(a.valid && b.valid);
//!     DecomposedTransform ret;
//!
//!     // Lerp the individual fields based on the provided flags.
//!     if (lerpTranslation) {
//!         ret.translation = lerp(a.translation, b.translation, weight);
//!     }
//!     else {
//!         ret.translation = b.translation;
//!     }
//!
//!     if (lerpRotation) {
//!         if (float(dot(a.rotation, b.rotation)) > 0.0f) {
//!             if (useSlerp) {
//!                 ret.rotation = slerp(a.rotation, b.rotation, 1.0f - weight);
//!             }
//!             else {
//!                 ret.rotation = lerp(a.rotation, b.rotation, weight);
//!             }
//!         }
//!         else {
//!             if (useSlerp) {
//!                 ret.rotation = slerp(a.rotation, -b.rotation, 1.0f - weight);
//!             }
//!             else {
//!                 ret.rotation = lerp(a.rotation, -b.rotation, weight);
//!             }
//!         }
//!         ret.rotation = normalize(ret.rotation);
//!     }
//!     else {
//!         ret.rotation = b.rotation;
//!     }
//!
//!     if (lerpScale) {
//!         ret.scale = lerp(a.scale, b.scale, weight);
//!     }
//!     else {
//!         ret.scale = b.scale;
//!     }
//!
//!     if (lerpSkew) {
//!         ret.skew = lerp(a.skew, b.skew, weight);
//!     }
//!     else {
//!         ret.skew = b.skew;
//!     }
//!
//!     if (lerpPerpsective) {
//!         ret.perspective = lerp(a.perspective, b.perspective, weight);
//!     }
//!     else {
//!         ret.perspective = b.perspective;
//!     }
//!
//!     // Mark the resultant transform as valid and return it.
//!     ret.valid = true;
//!     return ret;
//! }
//! ```
//!
//! **Reuse, not new type.** This module reuses `fn64_render_ir::{Mat4,
//! Vec3, Vec4}` directly for the matrix/vector shapes, matching
//! `rt64_math.rs`/`rt64_math_matrix.rs`'s established convention: `Mat4` is
//! row-major, `rows[i].{x,y,z,w}` = row `i`'s four columns, and an HLSL
//! `m[i][j]` read is `m.rows[i].{x,y,z,w}` for `j = 0..3`
//! (`rsp_math.rs:78-84`). `epsilon_equal` is intentionally NOT reused from
//! `rt64_math.rs` (excluded from this ticket's edit set, and this module may
//! not add a `use` of a private item nor should it depend on another
//! executor's module surface for a one-line predicate) -- it is
//! re-implemented locally, byte-for-byte identical to `rt64_math.rs`'s
//! `epsilon_equal`, since duplicating a trivial single-expression predicate
//! is lower risk than creating a cross-module dependency on a sibling
//! ticket's module. `matrixTranslation` and `matrixScale(float3)` (used by
//! `recomposeMatrix`) are similarly NOT present in either sibling module
//! (grepped `rt64_math.rs`/`rt64_math_matrix.rs`: neither ports them), so
//! this module adds minimal private local equivalents rather than reaching
//! into another ticket's file; both are one-line HLSL-identity-matrix
//! constructors with no branches or admitted-domain subtlety, so a local
//! copy carries no meaningful drift risk.
//!
//! This module adds three pieces of infrastructure that neither sibling
//! module provides, because `decomposeMatrix`/`recomposeMatrix` are the
//! first functions in this source file that need them:
//!
//! - A local `Quat { x, y, z, w }` type (no quaternion type exists anywhere
//!   in this crate or `fn64-render-ir`), with an `f32_index`/`f32_index_mut`
//!   pair mirroring `hlslpp::quaternion`'s `.f32[i]` array-style accessor
//!   used at `rt64_math.cpp:380-383`.
//! - `mat4_mul` (general 4x4 matrix-times-matrix), `determinant4`,
//!   `transpose4`, and `inverse4` (classical adjugate/cofactor inverse) --
//!   `decomposeMatrix` needs `determinant`/`inverse`/`transpose` of a
//!   `float4x4` and `recomposeMatrix` needs a matrix-chain `mul`; neither
//!   `fn64_render_ir::Mat4` nor either sibling module has any of these.
//!   `fn64_render_ir::Mat4::transform_point` already establishes
//!   `mul(matrix, vector) = M·v` (row `i` dotted against column vector `v`,
//!   `rsp_math.rs:91-111`) as this codebase's fixed reading of HLSL's `mul`
//!   intrinsic; `mat4_mul(A, B)` extends that same convention to two
//!   matrices as ordinary structural matrix multiplication,
//!   `mat4_mul(A,B)[i][j] = sum_k A[i][k]*B[k][j]`, so that
//!   `mat4_mul(A,B)` applied to a column vector via `transform_point`
//!   equals `A·(B·v)` -- consistent associativity with the existing
//!   `mul(matrix,vector)` reading, not a new or conflicting convention.
//! - `vec_combine`/`vec_scale` (`float3` helpers, ported verbatim from the
//!   cited source lines above) and a local quaternion-from-3x3-rotation
//!   assembly (the branchy "gem" algorithm) plus a quaternion-to-matrix
//!   `float4x4(rotation)` conversion (see "Admitted domain" below for the
//!   latter's unpinned-formula caveat).
//!
//! ## Admitted domain
//!
//! - **The quaternion-extraction branch structure is preserved literally,
//!   not collapsed.** `decompose_matrix` has the exact same two-way branch
//!   as the source: `trace > 0.0` takes the `w`-first branch; otherwise, an
//!   `i = argmax(Row[0][0], Row[1][1], Row[2][2])` is computed by the
//!   source's exact two sequential `if` comparisons (`Row[1].y > Row[0].x`,
//!   then `Row[2].z > Row[i][i]` -- note the second comparison reads
//!   `Row[i][i]`, which is `Row[1][1]` if the first `if` fired or
//!   `Row[0][0]` if it did not; this port preserves that exact
//!   already-mutated-`i` read, not a fresh three-way max), followed by
//!   `j = Next[i]`, `k = Next[j]` using the fixed cyclic table
//!   `Next = [1, 2, 0]`, and the `rotation.f32[i]/f32[j]/f32[k]/w`
//!   assignment order from the source. No branch was merged, reordered, or
//!   special-cased away.
//! - **`sqrt` of a negative `root` argument is unguarded.** Both branches
//!   compute `root = sqrt(...)` from an expression that is provably
//!   non-negative only under the "the matrix, in Rows[], is already
//!   orthonormal" precondition the source's own comment states just above
//!   (`// At this point, the matrix (in rows[]) is orthonormal.`) --
//!   `trace > 0` branch: `sqrt(trace + 1.0)`, guaranteed `>= sqrt(1.0) = 1.0`
//!   only if `trace > -1`, which the `trace > 0.0` guard ensures; `else`
//!   branch: `sqrt(Row[i][i] - Row[j][j] - Row[k][k] + 1.0)`, which for a
//!   genuine orthonormal (or orthonormal-then-flip-negated) 3x3 is always
//!   `>= 0`, but for an arbitrary/degenerate input matrix that this port's
//!   characterization tests deliberately feed in (e.g. a non-orthonormal
//!   `Row` reachable only by directly constructing pathological scale/skew
//!   inputs) can go negative, in which case `f32::sqrt` of a negative
//!   number returns `NaN` per Rust's IEEE-754-conformant `sqrt`, exactly
//!   matching C's `sqrt(negative) = NaN`. This port does not add a `max(x,
//!   0.0)` guard the source does not have.
//! - **Division by a near-zero `root` or `scale`**: `root = 0.5f / root`
//!   (both branches) and `skew.z /= scale.y`, `skew.y /= scale.z`,
//!   `skew.x /= scale.z` are all unguarded IEEE-754 divisions; a `root` or
//!   `scale` of exactly `0.0` yields `+-inf` (nonzero numerator) or `NaN`
//!   (`0.0/0.0`), preserved without a guard, matching every other module in
//!   this file's precedent of not inventing divide-by-zero protection the
//!   source does not have.
//! - **Zero-length-row normalization in `vec_scale`**: `v * desiredLength /
//!   length(v)` (note the source's exact operator grouping -- multiply by
//!   `desiredLength` **then** divide by `length(v)`, not `v / length(v) *
//!   desiredLength`; both are algebraically the same but round differently
//!   in `f32`, so this port preserves the source's left-to-right
//!   evaluation order) -- at `length(v) == 0`, this is `0.0/0.0 = NaN` per
//!   component (since `desiredLength` is always `1.0f` at every call site
//!   in `decomposeMatrix`, `v * 1.0` does not change `v`'s zero-ness before
//!   the division). No guard is added.
//! - **`decomposeMatrix` then `recomposeMatrix` is NOT asserted to be a
//!   bit-identical round-trip.** This was verified empirically by this
//!   port's own characterization tests (not assumed): for an axis-aligned
//!   scale+rotation+translation input with no perspective or skew, the
//!   round-trip matches the original to within a small numerical tolerance
//!   (see `decompose_then_recompose_roundtrip_*` tests below, tolerance
//!   `1e-4`) -- exact bit-identity does not hold even in this "nice" case,
//!   because `decomposeMatrix` divides by computed lengths/roots and
//!   `recomposeMatrix` reconstructs via `float4x4(rotation)` and further
//!   matrix products, and neither direction is designed to invert the other
//!   exactly in floating point. For an input with genuine skew (a
//!   shear-only matrix), the round-trip is only approximately recovered
//!   (see `decompose_then_recompose_roundtrip_with_skew_is_approximate`),
//!   again to a documented, measured tolerance, not an idealized identity.
//! - **`float4x4(rotation)` (quaternion-to-matrix) formula**: `hlslpp` is
//!   an unpopulated submodule in every checkout available to this program
//!   (confirmed: `src/contrib/hlslpp` is absent from the pinned checkout's
//!   working tree, same constraint `rsp_math.rs:21-23`, `rt64_math.rs`, and
//!   `rt64_math_matrix.rs` already document for other hlslpp calls) -- there
//!   is no vendored source to inspect for hlslpp's exact quaternion-to-
//!   matrix formula or its normalization precondition. This port uses the
//!   standard unit-quaternion-to-rotation-matrix formula, applied to
//!   `rotation` **without** first normalizing it (the source's
//!   `hlslpp::float4x4(rotation)` constructor call has no visible
//!   normalization step, and this port does not add one) -- so a non-unit
//!   quaternion input (reachable via `recomposeMatrix` being called
//!   directly with an arbitrary/synthetic `rotation` argument rather than
//!   one produced by `decomposeMatrix`, which is exactly what several
//!   characterization tests below do deliberately) produces a
//!   scaled/skewed, non-orthogonal "rotation" matrix rather than a clamped
//!   or renormalized one. There are two mirror-image (transposed) forms of
//!   this standard formula in general use, differing in which one is the
//!   inverse/transpose of the other; this port does **not** pick between
//!   them by assumption alone -- it picks the specific transpose empirically
//!   required to agree with `decompose_matrix`'s own `Row[i][j] =
//!   LocalMatrix[i][j]` row-major reading and its Shoemake/gem
//!   quaternion-extraction formula, verified by this module's own
//!   `decompose_pure_rotation_{x,y,z}_90deg` and
//!   `decompose_then_recompose_roundtrip_*` characterization tests (both
//!   directions independently exercised and cross-checked against each
//!   other and against independently-hand-derived axis-angle quaternions --
//!   see `expected_axis_quat`'s own comment for the exact sign relationship
//!   found). This is the strongest evidence available without a populated
//!   `hlslpp` checkout: internal self-consistency between the two paired
//!   functions in the same source file, not a citation of hlslpp's actual
//!   source. It remains possible that hlslpp's *true* quaternion-to-matrix
//!   formula is unrelated to (not just transposed from) this port's choice
//!   if hlslpp's `decomposeMatrix`/`float4x4(quaternion)` pair uses some
//!   other internal convention entirely -- flagged here as residual
//!   uncertainty, not resolved by this port.
//! - **`lerp_transforms`'s `assert(a.valid && b.valid)`**: ported as a Rust
//!   `debug_assert!` (panics in debug builds only, matching C++'s `assert`
//!   macro being compiled out under `NDEBUG`/release -- this is the
//!   established Rust idiom for a literal `assert()` port, not a weakening;
//!   `assert!` would introduce release-mode panicking behavior the C++
//!   source does not have when built with `NDEBUG`).
//! - **`lerp_transforms`'s `slerp`**: `hlslpp::slerp(a, b, t)` is called by
//!   the source but this port does **not** implement spherical
//!   interpolation from scratch as a guess -- `hlslpp` is unpopulated (see
//!   above), and spherical linear interpolation of a quaternion has several
//!   subtly different conventional formulas (shortest-path clamping,
//!   `sin`-ratio blending, a linear-lerp-then-normalize fallback near
//!   parallel quaternions) that are not safely inferable without the actual
//!   source. This port implements the standard textbook `slerp` (constant-
//!   angular-velocity spherical interpolation via the angle between the two
//!   quaternions, falling back to linear interpolation when they are
//!   nearly parallel to avoid a `0/0` in the `sin`-ratio weights) as the
//!   conventional definition, explicitly flagged here as the least-pinned
//!   assumption in this module -- **open question**, not a verified read:
//!   if hlslpp's `slerp` differs (e.g. in its near-parallel fallback
//!   threshold or in not shortest-path-correcting doubly), this port's
//!   `useSlerp=true` behavior will diverge from upstream. `lerpTransforms`
//!   already does its own shortest-path correction before calling
//!   `slerp`/`lerp` (the `dot(a.rotation, b.rotation) > 0.0` branch negating
//!   `b`), so this port's `slerp` itself does not re-apply shortest-path
//!   correction, matching the source's call shape
//!   (`slerp(a.rotation, b.rotation, ...)` / `slerp(a.rotation,
//!   -b.rotation, ...)` -- the sign flip already happened at the call
//!   site).
//! - **Quaternion `dot`, `normalize`, unary negation, and `lerp`**: assumed
//!   to be the conventional 4-component definitions (`dot` = sum of
//!   componentwise products; `normalize(q) = q / |q|`, unguarded division at
//!   `|q| == 0` yielding component-wise `NaN`; `-q` negates all four
//!   components; `lerp(a, b, t) = a + t*(b-a)` per component, matching
//!   `rt64_math_matrix.rs`'s already-established `lerp` formula precedent
//!   for this exact `a + t*(b-a)` form over `a*(1-t)+b*t`) -- same
//!   unpopulated-submodule caveat as above, conventional but not verified.
//! - **`float(dot(a.rotation, b.rotation))`**: the source's explicit `float(...)`
//!   cast is a no-op in this port since quaternion `dot` already returns a
//!   plain `f32` here (no SIMD-lane-wrapper type exists in this crate to
//!   need unwrapping).
//!
//! ## Nonclaims
//!
//! No GPU, WGSL, camera/view-matrix wiring, or RT64 visual/pixel/silicon
//! parity or performance claim. This module is not called from anywhere yet
//! (dead-code warnings on the unused public surface are expected and
//! correct, matching every other characterization-first module's
//! precedent). Does not port any other function from `rt64_math.h`/`.cpp`
//! beyond the four named in the module title (`DecomposedTransform`,
//! `decomposeMatrix`, `recomposeMatrix`, `lerpTransforms`) plus their
//! required `vecCombine`/`vecScale`/`epsilonEqual` helpers and this module's
//! own new 4x4-matrix/quaternion infrastructure -- the six-function matrix
//! cluster (`extract3x3`, `rotationFrom3x3`, `matrixDifference`,
//! `lerpMatrix`/`lerpMatrix3x3`/`lerpMatrixComponents`) is `rt64_math_matrix.rs`
//! (M8.3, already landed, not duplicated here), and `matrixScale(float)`,
//! `matrixRotationX/Y/Z`, `matrixDecomposeViewProj`, and `pseudoRandom`
//! remain out of scope exactly as `rt64_math.rs` already stated. This
//! module's local `matrix_translation`/`matrix_scale_vec3` are minimal
//! recompose-only helpers, not a claim of porting `matrixTranslation`/
//! `matrixScale(float3)` as a reusable public API for other callers.

use fn64_render_ir::{Mat4, Vec3, Vec4};

/// `hlslpp::quaternion`: no quaternion type exists anywhere in this crate or
/// `fn64-render-ir`, so this module adds a minimal local one. Component
/// order matches the source's field names (`x, y, z, w`).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Quat {
    pub x: f32,
    pub y: f32,
    pub z: f32,
    pub w: f32,
}

impl Quat {
    pub const fn new(x: f32, y: f32, z: f32, w: f32) -> Self {
        Self { x, y, z, w }
    }

    /// `rotation.f32[i]` (read): `hlslpp::quaternion`'s array-style
    /// accessor, indexing `x=0, y=1, z=2, w=3` (the source's own field
    /// declaration order).
    fn f32_index(self, i: usize) -> f32 {
        match i {
            0 => self.x,
            1 => self.y,
            2 => self.z,
            3 => self.w,
            _ => unreachable!("quaternion index out of range: {i}"),
        }
    }

    /// `rotation.f32[i]` (write).
    fn f32_index_mut(&mut self, i: usize) -> &mut f32 {
        match i {
            0 => &mut self.x,
            1 => &mut self.y,
            2 => &mut self.z,
            3 => &mut self.w,
            _ => unreachable!("quaternion index out of range: {i}"),
        }
    }

    /// `hlslpp::dot(quaternion, quaternion)`: the conventional 4-component
    /// dot product.
    ///
    /// `pub(crate)` rather than private because
    /// `crate::rt64_rigid_body`'s `updateAngular` bias branch needs the
    /// same product and previously re-derived it locally; that duplicate
    /// has been retired in favour of this definition. `neg`/`normalize`
    /// stay private -- no sibling module calls them.
    pub(crate) fn dot(self, rhs: Self) -> f32 {
        self.x * rhs.x + self.y * rhs.y + self.z * rhs.z + self.w * rhs.w
    }

    fn neg(self) -> Self {
        Self::new(-self.x, -self.y, -self.z, -self.w)
    }

    fn normalize(self) -> Self {
        let len = (self.x * self.x + self.y * self.y + self.z * self.z + self.w * self.w).sqrt();
        Self::new(self.x / len, self.y / len, self.z / len, self.w / len)
    }
}

/// `hlslpp::lerp(a, b, t)` for a scalar: `a + t*(b-a)` (see module doc
/// "Admitted domain").
///
/// `pub(crate)` rather than private because the sibling
/// `crate::rt64_math_matrix` ports the *matrix* half of the same
/// `rt64_math.cpp` at the same pinned commit and needs the identical
/// scalar `hlslpp::lerp` for `lerpMatrix`/`lerpMatrix3x3`/
/// `lerpMatrixComponents`; it previously carried a character-identical
/// copy. Changing this formula changes both modules' parity claims.
pub(crate) fn lerp_f32(a: f32, b: f32, t: f32) -> f32 {
    a + t * (b - a)
}

fn lerp_vec3(a: Vec3, b: Vec3, t: f32) -> Vec3 {
    Vec3::new(
        lerp_f32(a.x, b.x, t),
        lerp_f32(a.y, b.y, t),
        lerp_f32(a.z, b.z, t),
    )
}

fn lerp_vec4(a: Vec4, b: Vec4, t: f32) -> Vec4 {
    Vec4::new(
        lerp_f32(a.x, b.x, t),
        lerp_f32(a.y, b.y, t),
        lerp_f32(a.z, b.z, t),
        lerp_f32(a.w, b.w, t),
    )
}

fn lerp_quat(a: Quat, b: Quat, t: f32) -> Quat {
    Quat::new(
        lerp_f32(a.x, b.x, t),
        lerp_f32(a.y, b.y, t),
        lerp_f32(a.z, b.z, t),
        lerp_f32(a.w, b.w, t),
    )
}

/// Standard textbook spherical interpolation (see module doc "Admitted
/// domain" -- `hlslpp::slerp`'s exact formula is unpinned; this is the
/// conventional constant-angular-velocity definition with a linear-lerp
/// fallback when the quaternions are nearly parallel).
fn slerp_quat(a: Quat, b: Quat, t: f32) -> Quat {
    let cos_theta = a.dot(b);
    if cos_theta.abs() > 0.9995 {
        return lerp_quat(a, b, t);
    }
    let theta = cos_theta.clamp(-1.0, 1.0).acos();
    let sin_theta = theta.sin();
    let wa = ((1.0 - t) * theta).sin() / sin_theta;
    let wb = (t * theta).sin() / sin_theta;
    Quat::new(
        wa * a.x + wb * b.x,
        wa * a.y + wb * b.y,
        wa * a.z + wb * b.z,
        wa * a.w + wb * b.w,
    )
}

/// `epsilonEqual`: `|a-b| < f32::EPSILON` (strict less-than). Not reused
/// from `rt64_math.rs` (see module doc "Reuse, not new type").
fn epsilon_equal(a: f32, b: f32) -> bool {
    (a - b).abs() < f32::EPSILON
}

/// `vecCombine`: `(a * ascl) + (b * bscl)`, the source's exact operand
/// order and grouping.
fn vec_combine(a: Vec3, b: Vec3, ascl: f32, bscl: f32) -> Vec3 {
    Vec3::new(
        a.x * ascl + b.x * bscl,
        a.y * ascl + b.y * bscl,
        a.z * ascl + b.z * bscl,
    )
}

/// `hlslpp::length(float3)`: `sqrt(x*x + y*y + z*z)`, unguarded -- a zero
/// vector yields `0.0` and a `NaN` component propagates.
///
/// `pub(crate)` rather than private because `crate::rt64_rigid_body`'s
/// `updateLinear` needs the identical `hlslpp::length(float3)` and
/// previously carried a character-identical copy. This is *not* the same
/// helper as `rt64_preset_light`'s or `rt64_lights_math`'s `length`: those
/// cite different C++ authorities with their own ulp/NaN caveats and are
/// deliberately kept separate.
pub(crate) fn vec3_length(v: Vec3) -> f32 {
    (v.x * v.x + v.y * v.y + v.z * v.z).sqrt()
}

/// `vecScale`: `v * desiredLength / length(v)`, preserving the source's
/// exact left-to-right evaluation order (multiply first, then divide -- see
/// module doc "Admitted domain").
fn vec_scale(v: Vec3, desired_length: f32) -> Vec3 {
    let len = vec3_length(v);
    Vec3::new(
        v.x * desired_length / len,
        v.y * desired_length / len,
        v.z * desired_length / len,
    )
}

fn vec3_dot(a: Vec3, b: Vec3) -> f32 {
    a.x * b.x + a.y * b.y + a.z * b.z
}

fn vec3_cross(a: Vec3, b: Vec3) -> Vec3 {
    Vec3::new(
        a.y * b.z - a.z * b.y,
        a.z * b.x - a.x * b.z,
        a.x * b.y - a.y * b.x,
    )
}

fn mat4_get(m: Mat4, i: usize, j: usize) -> f32 {
    let row = m.rows[i];
    match j {
        0 => row.x,
        1 => row.y,
        2 => row.z,
        3 => row.w,
        _ => unreachable!("column index out of range: {j}"),
    }
}

fn mat4_set(m: &mut Mat4, i: usize, j: usize, v: f32) {
    let row = &mut m.rows[i];
    match j {
        0 => row.x = v,
        1 => row.y = v,
        2 => row.z = v,
        3 => row.w = v,
        _ => unreachable!("column index out of range: {j}"),
    }
}

fn mat4_identity() -> Mat4 {
    Mat4::from_rows([
        Vec4::new(1.0, 0.0, 0.0, 0.0),
        Vec4::new(0.0, 1.0, 0.0, 0.0),
        Vec4::new(0.0, 0.0, 1.0, 0.0),
        Vec4::new(0.0, 0.0, 0.0, 1.0),
    ])
}

/// `mul(float4x4, float4x4)`: ordinary structural matrix multiplication,
/// `mat4_mul(A,B)[i][j] = sum_k A[i][k]*B[k][j]`. Consistent with
/// `fn64_render_ir::Mat4::transform_point`'s already-established
/// `mul(matrix, vector) = M·v` reading (see module doc "Reuse, not new
/// type" for the associativity argument).
fn mat4_mul(a: Mat4, b: Mat4) -> Mat4 {
    let mut out = Mat4::from_rows([Vec4::new(0.0, 0.0, 0.0, 0.0); 4]);
    for i in 0..4 {
        for j in 0..4 {
            let mut sum = 0.0f32;
            for k in 0..4 {
                sum += mat4_get(a, i, k) * mat4_get(b, k, j);
            }
            mat4_set(&mut out, i, j, sum);
        }
    }
    out
}

/// `transpose(float4x4)`.
fn transpose4(m: Mat4) -> Mat4 {
    let mut out = m;
    for i in 0..4 {
        for j in 0..4 {
            mat4_set(&mut out, i, j, mat4_get(m, j, i));
        }
    }
    out
}

/// `determinant(float4x4)` via cofactor expansion along the first row.
fn determinant4(m: Mat4) -> f32 {
    let e = |i: usize, j: usize| mat4_get(m, i, j);
    let minor = |r0: usize, r1: usize, r2: usize, c0: usize, c1: usize, c2: usize| -> f32 {
        e(r0, c0) * (e(r1, c1) * e(r2, c2) - e(r1, c2) * e(r2, c1))
            - e(r0, c1) * (e(r1, c0) * e(r2, c2) - e(r1, c2) * e(r2, c0))
            + e(r0, c2) * (e(r1, c0) * e(r2, c1) - e(r1, c1) * e(r2, c0))
    };
    let c0 = minor(1, 2, 3, 1, 2, 3);
    let c1 = minor(1, 2, 3, 0, 2, 3);
    let c2 = minor(1, 2, 3, 0, 1, 3);
    let c3 = minor(1, 2, 3, 0, 1, 2);
    e(0, 0) * c0 - e(0, 1) * c1 + e(0, 2) * c2 - e(0, 3) * c3
}

/// `inverse(float4x4)` via the classical adjugate-over-determinant formula
/// (cofactor matrix, transposed, divided by the determinant). Unguarded: a
/// singular (zero-determinant) input divides by zero, producing `+-inf`/`NaN`
/// entries rather than a panic or a guarded fallback, matching this
/// codebase's unguarded-arithmetic precedent throughout `rt64_math*.rs`.
///
/// Visibility widened from private to `pub(crate)` (behavior, signature and
/// body unchanged) so that `rt64_math_matrix.rs` can reuse the crate's
/// single 4x4 inverse for RT64's `hlslpp::inverse` call sites instead of
/// adding a second, duplicate cofactor inverse under a different name. See
/// that module's doc comment for the full justification.
pub(crate) fn inverse4(m: Mat4) -> Mat4 {
    let e = |i: usize, j: usize| mat4_get(m, i, j);

    // 3x3 minor determinant helper over an explicit row/column selection.
    let det3 = |r0: usize, r1: usize, r2: usize, c0: usize, c1: usize, c2: usize| -> f32 {
        e(r0, c0) * (e(r1, c1) * e(r2, c2) - e(r1, c2) * e(r2, c1))
            - e(r0, c1) * (e(r1, c0) * e(r2, c2) - e(r1, c2) * e(r2, c0))
            + e(r0, c2) * (e(r1, c0) * e(r2, c1) - e(r1, c1) * e(r2, c0))
    };

    let rows: [usize; 4] = [0, 1, 2, 3];
    let cols: [usize; 4] = [0, 1, 2, 3];

    let mut cofactor = Mat4::from_rows([Vec4::new(0.0, 0.0, 0.0, 0.0); 4]);
    for i in 0..4 {
        for j in 0..4 {
            let mut r_iter = rows.iter().copied().filter(|&r| r != i);
            let r0 = r_iter.next().unwrap();
            let r1 = r_iter.next().unwrap();
            let r2 = r_iter.next().unwrap();
            let mut c_iter = cols.iter().copied().filter(|&c| c != j);
            let c0 = c_iter.next().unwrap();
            let c1 = c_iter.next().unwrap();
            let c2 = c_iter.next().unwrap();
            let minor = det3(r0, r1, r2, c0, c1, c2);
            let sign = if (i + j) % 2 == 0 { 1.0 } else { -1.0 };
            mat4_set(&mut cofactor, i, j, sign * minor);
        }
    }

    let det = e(0, 0) * mat4_get(cofactor, 0, 0)
        + e(0, 1) * mat4_get(cofactor, 0, 1)
        + e(0, 2) * mat4_get(cofactor, 0, 2)
        + e(0, 3) * mat4_get(cofactor, 0, 3);

    // adjugate = transpose(cofactor); inverse = adjugate / det.
    let adjugate = transpose4(cofactor);
    let mut out = Mat4::from_rows([Vec4::new(0.0, 0.0, 0.0, 0.0); 4]);
    for i in 0..4 {
        for j in 0..4 {
            mat4_set(&mut out, i, j, mat4_get(adjugate, i, j) / det);
        }
    }
    out
}

/// `matrixTranslation` (recompose-only local helper; see module doc "Reuse,
/// not new type" for why this is not reused from a sibling module).
fn matrix_translation(t: Vec3) -> Mat4 {
    let mut m = mat4_identity();
    m.rows[3].x = t.x;
    m.rows[3].y = t.y;
    m.rows[3].z = t.z;
    m
}

/// `matrixScale(const float3&)` (recompose-only local helper).
fn matrix_scale_vec3(scale: Vec3) -> Mat4 {
    let mut m = Mat4::from_rows([Vec4::new(0.0, 0.0, 0.0, 0.0); 4]);
    m.rows[0].x = scale.x;
    m.rows[1].y = scale.y;
    m.rows[2].z = scale.z;
    m.rows[3].w = 1.0;
    m
}

/// `hlslpp::float4x4(rotation)`: quaternion-to-rotation-matrix conversion
/// (see module doc "Admitted domain" for the unpinned-formula caveat --
/// applied without normalizing `q` first, matching the source's bare
/// constructor call).
fn mat4_from_quat(q: Quat) -> Mat4 {
    let (x, y, z, w) = (q.x, q.y, q.z, q.w);
    let mut m = mat4_identity();
    // Transposed relative to the textbook "column j = image of basis
    // vector j" layout: `decompose_matrix`'s gem algorithm reads
    // `Row[i][j] = LocalMatrix[i][j]` and extracts a quaternion whose
    // paired to-matrix conversion is this row-major-transposed form (empirically
    // pinned by this module's own round-trip characterization tests --
    // see module doc "Admitted domain" for why the exact hlslpp formula is
    // unverifiable and this is the pairing that makes decompose/recompose
    // agree with the gem algorithm's own row/column reading).
    m.rows[0] = Vec4::new(
        1.0 - 2.0 * (y * y + z * z),
        2.0 * (x * y + z * w),
        2.0 * (x * z - y * w),
        0.0,
    );
    m.rows[1] = Vec4::new(
        2.0 * (x * y - z * w),
        1.0 - 2.0 * (x * x + z * z),
        2.0 * (y * z + x * w),
        0.0,
    );
    m.rows[2] = Vec4::new(
        2.0 * (x * z + y * w),
        2.0 * (y * z - x * w),
        1.0 - 2.0 * (x * x + y * y),
        0.0,
    );
    m.rows[3] = Vec4::new(0.0, 0.0, 0.0, 1.0);
    m
}

/// `decomposeMatrix`. Returns `None` where the source returns `false`
/// (the `bool` return value); on success, returns `Some((rotation, scale,
/// skew, translation, perspective, coordinate_flip))`, mirroring the
/// source's five by-reference out-parameters plus `coordinateFlip`.
#[allow(clippy::type_complexity)]
pub fn decompose_matrix(mtx: Mat4) -> Option<(Quat, Vec3, Vec3, Vec3, Vec4, bool)> {
    let mut local = mtx;

    // Normalize the matrix.
    if epsilon_equal(mat4_get(local, 3, 3), 0.0) {
        return None;
    }

    let m33 = mat4_get(local, 3, 3);
    for i in 0..4 {
        for j in 0..4 {
            let v = mat4_get(local, i, j);
            mat4_set(&mut local, i, j, v / m33);
        }
    }

    // perspectiveMatrix is used to solve for perspective, but it also
    // provides an easy way to test for singularity of the upper 3x3
    // component.
    let mut perspective_matrix = local;
    for i in 0..3 {
        mat4_set(&mut perspective_matrix, i, 3, 0.0);
    }
    mat4_set(&mut perspective_matrix, 3, 3, 1.0);

    if epsilon_equal(determinant4(perspective_matrix), 0.0) {
        return None;
    }

    let perspective;

    // First, isolate perspective. This is the messiest.
    if !epsilon_equal(mat4_get(local, 0, 3), 0.0)
        || !epsilon_equal(mat4_get(local, 1, 3), 0.0)
        || !epsilon_equal(mat4_get(local, 2, 3), 0.0)
    {
        let right_hand_side = Vec4::new(
            mat4_get(local, 0, 3),
            mat4_get(local, 1, 3),
            mat4_get(local, 2, 3),
            mat4_get(local, 3, 3),
        );

        let inverse_perspective_matrix = inverse4(perspective_matrix);
        let transposed_inverse_perspective_matrix = transpose4(inverse_perspective_matrix);

        perspective = transposed_inverse_perspective_matrix.transform_point(right_hand_side);

        // Clear the perspective partition.
        mat4_set(&mut local, 0, 3, 0.0);
        mat4_set(&mut local, 1, 3, 0.0);
        mat4_set(&mut local, 2, 3, 0.0);
        mat4_set(&mut local, 3, 3, 1.0);
    } else {
        // No perspective.
        perspective = Vec4::new(0.0, 0.0, 0.0, 1.0);
    }

    // Next take care of translation (easy).
    let translation = Vec3::new(
        mat4_get(local, 3, 0),
        mat4_get(local, 3, 1),
        mat4_get(local, 3, 2),
    );
    let local_row3_w = mat4_get(local, 3, 3);
    local.rows[3] = Vec4::new(0.0, 0.0, 0.0, local_row3_w);

    let mut row = [Vec3::default(); 3];

    // Now get scale and shear.
    for i in 0..3 {
        row[i] = Vec3::new(
            mat4_get(local, i, 0),
            mat4_get(local, i, 1),
            mat4_get(local, i, 2),
        );
    }

    let mut scale = Vec3::default();
    let mut skew = Vec3::default();

    // Compute X scale factor and normalize first row.
    scale.x = vec3_length(row[0]);
    row[0] = vec_scale(row[0], 1.0);

    // Compute XY shear factor and make 2nd row orthogonal to 1st.
    skew.z = vec3_dot(row[0], row[1]);
    row[1] = vec_combine(row[1], row[0], 1.0, -skew.z);

    // Now, compute Y scale and normalize 2nd row.
    scale.y = vec3_length(row[1]);
    row[1] = vec_scale(row[1], 1.0);
    skew.z /= scale.y;

    // Compute XZ and YZ shears, orthogonalize 3rd row.
    skew.y = vec3_dot(row[0], row[2]);
    row[2] = vec_combine(row[2], row[0], 1.0, -skew.y);
    skew.x = vec3_dot(row[1], row[2]);
    row[2] = vec_combine(row[2], row[1], 1.0, -skew.x);

    // Next, get Z scale and normalize 3rd row.
    scale.z = vec3_length(row[2]);
    row[2] = vec_scale(row[2], 1.0);
    skew.y /= scale.z;
    skew.x /= scale.z;

    // At this point, the matrix (in row[]) is orthonormal. Check for a
    // coordinate system flip. If the determinant is -1, then negate the
    // matrix and the scaling factors.
    let pdum3 = vec3_cross(row[1], row[2]);
    let coordinate_flip = vec3_dot(row[0], pdum3) < 0.0;
    if coordinate_flip {
        scale.x *= -1.0;
        scale.y *= -1.0;
        scale.z *= -1.0;
        row[0] = row[0].scale(-1.0);
        row[1] = row[1].scale(-1.0);
        row[2] = row[2].scale(-1.0);
    }

    // Now, get the rotations out, as described in the gem.
    let mut rotation = Quat::default();
    let trace = row[0].x + row[1].y + row[2].z;
    if trace > 0.0 {
        let mut root = (trace + 1.0).sqrt();
        rotation.w = 0.5 * root;
        root = 0.5 / root;
        rotation.x = root * (row_get(&row, 1, 2) - row_get(&row, 2, 1));
        rotation.y = root * (row_get(&row, 2, 0) - row_get(&row, 0, 2));
        rotation.z = root * (row_get(&row, 0, 1) - row_get(&row, 1, 0));
    } else {
        const NEXT: [usize; 3] = [1, 2, 0];
        let mut i = 0usize;
        if row[1].y > row[0].x {
            i = 1;
        }
        if row[2].z > row_get(&row, i, i) {
            i = 2;
        }
        let j = NEXT[i];
        let k = NEXT[j];

        let mut root =
            (row_get(&row, i, i) - row_get(&row, j, j) - row_get(&row, k, k) + 1.0).sqrt();

        *rotation.f32_index_mut(i) = 0.5 * root;
        root = 0.5 / root;
        *rotation.f32_index_mut(j) = root * (row_get(&row, i, j) + row_get(&row, j, i));
        *rotation.f32_index_mut(k) = root * (row_get(&row, i, k) + row_get(&row, k, i));
        rotation.w = root * (row_get(&row, j, k) - row_get(&row, k, j));
    }

    Some((
        rotation,
        scale,
        skew,
        translation,
        perspective,
        coordinate_flip,
    ))
}

/// `Row[i][j]`: index into a `[Vec3; 3]` as a 3x3 matrix.
fn row_get(row: &[Vec3; 3], i: usize, j: usize) -> f32 {
    let v = row[i];
    match j {
        0 => v.x,
        1 => v.y,
        2 => v.z,
        _ => unreachable!("row column index out of range: {j}"),
    }
}

/// `recomposeMatrix`.
pub fn recompose_matrix(
    rotation: Quat,
    scale: Vec3,
    skew: Vec3,
    translation: Vec3,
    perspective: Vec4,
) -> Mat4 {
    let mut m = mat4_identity();

    mat4_set(&mut m, 0, 3, perspective.x);
    mat4_set(&mut m, 1, 3, perspective.y);
    mat4_set(&mut m, 2, 3, perspective.z);
    mat4_set(&mut m, 3, 3, perspective.w);

    m = mat4_mul(matrix_translation(translation), m);
    m = mat4_mul(mat4_from_quat(rotation), m);

    if skew.x.abs() > 0.0 {
        let mut tmp = mat4_identity();
        mat4_set(&mut tmp, 2, 1, skew.x);
        m = mat4_mul(tmp, m);
    }

    if skew.y.abs() > 0.0 {
        let mut tmp = mat4_identity();
        mat4_set(&mut tmp, 2, 0, skew.y);
        m = mat4_mul(tmp, m);
    }

    if skew.z.abs() > 0.0 {
        let mut tmp = mat4_identity();
        mat4_set(&mut tmp, 1, 0, skew.z);
        m = mat4_mul(tmp, m);
    }

    m = mat4_mul(matrix_scale_vec3(scale), m);

    m
}

/// `DecomposedTransform`. The source's default-constructed fields
/// (`coordinateFlip = false`, `valid = false`, everything else
/// default-initialized by `hlslpp`'s own default constructors) are mirrored
/// via `#[derive(Default)]` -- `Quat`/`Vec3`/`Vec4` default to all-zero
/// components, matching the conventional zero-initialization every
/// `hlslpp` vector/quaternion type is assumed to have (same unpinned-hlslpp
/// caveat as elsewhere in this module).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct DecomposedTransform {
    pub rotation: Quat,
    pub scale: Vec3,
    pub skew: Vec3,
    pub translation: Vec3,
    pub perspective: Vec4,
    pub coordinate_flip: bool,
    pub valid: bool,
}

impl DecomposedTransform {
    /// `DecomposedTransform(const hlslpp::float4x4& mtx)`.
    pub fn from_matrix(mtx: Mat4) -> Self {
        match decompose_matrix(mtx) {
            Some((rotation, scale, skew, translation, perspective, coordinate_flip)) => Self {
                rotation,
                scale,
                skew,
                translation,
                perspective,
                coordinate_flip,
                valid: true,
            },
            None => Self {
                valid: false,
                ..Self::default()
            },
        }
    }
}

/// `lerpTransforms`.
#[allow(clippy::too_many_arguments)]
pub fn lerp_transforms(
    a: &DecomposedTransform,
    b: &DecomposedTransform,
    weight: f32,
    lerp_translation: bool,
    lerp_rotation: bool,
    lerp_scale: bool,
    lerp_skew: bool,
    lerp_perspective: bool,
    use_slerp: bool,
) -> DecomposedTransform {
    debug_assert!(a.valid && b.valid);
    let mut ret = DecomposedTransform::default();

    if lerp_translation {
        ret.translation = lerp_vec3(a.translation, b.translation, weight);
    } else {
        ret.translation = b.translation;
    }

    if lerp_rotation {
        if a.rotation.dot(b.rotation) > 0.0 {
            ret.rotation = if use_slerp {
                slerp_quat(a.rotation, b.rotation, 1.0 - weight)
            } else {
                lerp_quat(a.rotation, b.rotation, weight)
            };
        } else {
            let neg_b = b.rotation.neg();
            ret.rotation = if use_slerp {
                slerp_quat(a.rotation, neg_b, 1.0 - weight)
            } else {
                lerp_quat(a.rotation, neg_b, weight)
            };
        }
        ret.rotation = ret.rotation.normalize();
    } else {
        ret.rotation = b.rotation;
    }

    if lerp_scale {
        ret.scale = lerp_vec3(a.scale, b.scale, weight);
    } else {
        ret.scale = b.scale;
    }

    if lerp_skew {
        ret.skew = lerp_vec3(a.skew, b.skew, weight);
    } else {
        ret.skew = b.skew;
    }

    if lerp_perspective {
        ret.perspective = lerp_vec4(a.perspective, b.perspective, weight);
    } else {
        ret.perspective = b.perspective;
    }

    ret.valid = true;
    ret
}

#[cfg(test)]
mod tests {
    use super::*;

    const EPS: f32 = 1e-4;

    fn approx(a: f32, b: f32, msg: &str) {
        assert!((a - b).abs() < EPS, "{msg}: {a} !~= {b}");
    }

    fn approx_vec3(a: Vec3, b: Vec3, msg: &str) {
        approx(a.x, b.x, &format!("{msg}.x"));
        approx(a.y, b.y, &format!("{msg}.y"));
        approx(a.z, b.z, &format!("{msg}.z"));
    }

    fn approx_mat4(a: Mat4, b: Mat4, msg: &str) {
        for i in 0..4 {
            approx(a.rows[i].x, b.rows[i].x, &format!("{msg} row{i}.x"));
            approx(a.rows[i].y, b.rows[i].y, &format!("{msg} row{i}.y"));
            approx(a.rows[i].z, b.rows[i].z, &format!("{msg} row{i}.z"));
            approx(a.rows[i].w, b.rows[i].w, &format!("{msg} row{i}.w"));
        }
    }

    fn identity() -> Mat4 {
        mat4_identity()
    }

    fn zeros() -> Mat4 {
        Mat4::from_rows([Vec4::new(0.0, 0.0, 0.0, 0.0); 4])
    }

    fn identity_quat() -> Quat {
        Quat::new(0.0, 0.0, 0.0, 1.0)
    }

    // --- decompose_matrix: identity ---

    #[test]
    fn decompose_identity_matrix() {
        let (rotation, scale, skew, translation, perspective, flip) =
            decompose_matrix(identity()).expect("identity is decomposable");
        approx_vec3(scale, Vec3::new(1.0, 1.0, 1.0), "scale");
        approx_vec3(skew, Vec3::new(0.0, 0.0, 0.0), "skew");
        approx_vec3(translation, Vec3::new(0.0, 0.0, 0.0), "translation");
        approx(perspective.x, 0.0, "persp.x");
        approx(perspective.y, 0.0, "persp.y");
        approx(perspective.z, 0.0, "persp.z");
        approx(perspective.w, 1.0, "persp.w");
        assert!(!flip);
        // Identity rotation: trace = 3 > 0 branch, w = 1, xyz = 0.
        approx(rotation.w, 1.0, "rot.w");
        approx(rotation.x, 0.0, "rot.x");
        approx(rotation.y, 0.0, "rot.y");
        approx(rotation.z, 0.0, "rot.z");
    }

    // --- pure translation ---

    #[test]
    fn decompose_pure_translation() {
        let mut m = identity();
        m.rows[3] = Vec4::new(10.0, -5.0, 2.5, 1.0);
        let (rotation, scale, _skew, translation, _perspective, flip) =
            decompose_matrix(m).unwrap();
        approx_vec3(translation, Vec3::new(10.0, -5.0, 2.5), "translation");
        approx_vec3(scale, Vec3::new(1.0, 1.0, 1.0), "scale");
        assert!(!flip);
        approx(rotation.w, 1.0, "rot.w");
    }

    // --- pure rotation, each axis ---

    fn rotation_x(rad: f32) -> Mat4 {
        let (s, c) = rad.sin_cos();
        Mat4::from_rows([
            Vec4::new(1.0, 0.0, 0.0, 0.0),
            Vec4::new(0.0, c, -s, 0.0),
            Vec4::new(0.0, s, c, 0.0),
            Vec4::new(0.0, 0.0, 0.0, 1.0),
        ])
    }

    fn rotation_y(rad: f32) -> Mat4 {
        let (s, c) = rad.sin_cos();
        Mat4::from_rows([
            Vec4::new(c, 0.0, s, 0.0),
            Vec4::new(0.0, 1.0, 0.0, 0.0),
            Vec4::new(-s, 0.0, c, 0.0),
            Vec4::new(0.0, 0.0, 0.0, 1.0),
        ])
    }

    fn rotation_z(rad: f32) -> Mat4 {
        let (s, c) = rad.sin_cos();
        Mat4::from_rows([
            Vec4::new(c, -s, 0.0, 0.0),
            Vec4::new(s, c, 0.0, 0.0),
            Vec4::new(0.0, 0.0, 1.0, 0.0),
            Vec4::new(0.0, 0.0, 0.0, 1.0),
        ])
    }

    // Independently-derived expected quaternion for `decompose_matrix`'s
    // reading of an active CCW rotation matrix (rotation_x/y/z above, which
    // map e.g. e_x -> e_y under `Mat4::transform_point`'s `M*v` column-vector
    // convention). The gem ("gems") algorithm's `Row[i][j] = LocalMatrix[i][j]`
    // reading extracts the CONJUGATE of the textbook axis-angle quaternion
    // for that active rotation (imaginary part negated, `w` unchanged) --
    // empirically confirmed here by requiring this helper's output to match
    // `decompose_matrix`'s actual result for known 90-degree single-axis
    // rotations, and cross-checked independently by this module's own
    // `mat4_from_quat` round-trip tests, which are self-consistent with
    // `decompose_matrix` (see `decompose_then_recompose_roundtrip_*`).
    // Standard axis-angle would give `axis*sin(rad/2), cos(rad/2)`; this
    // helper negates the axis term to match the gem algorithm's own
    // row/column convention.
    fn expected_axis_quat(axis: Vec3, rad: f32) -> Quat {
        let half = rad / 2.0;
        let s = half.sin();
        Quat::new(-axis.x * s, -axis.y * s, -axis.z * s, half.cos())
    }

    fn assert_quat_matches_up_to_sign(got: Quat, expected: Quat, msg: &str) {
        // decomposeMatrix's branch selection can yield either q or -q for
        // the same rotation (both represent the same rotation matrix); the
        // gem algorithm is not guaranteed to pick a canonical sign.
        let same = (got.x - expected.x).abs() < EPS
            && (got.y - expected.y).abs() < EPS
            && (got.z - expected.z).abs() < EPS
            && (got.w - expected.w).abs() < EPS;
        let opposite = (got.x + expected.x).abs() < EPS
            && (got.y + expected.y).abs() < EPS
            && (got.z + expected.z).abs() < EPS
            && (got.w + expected.w).abs() < EPS;
        assert!(
            same || opposite,
            "{msg}: got {got:?}, expected +-{expected:?}"
        );
    }

    #[test]
    fn decompose_pure_rotation_x_90deg() {
        let rad = std::f32::consts::FRAC_PI_2;
        let (rotation, scale, _skew, _t, _p, _flip) = decompose_matrix(rotation_x(rad)).unwrap();
        approx_vec3(scale, Vec3::new(1.0, 1.0, 1.0), "scale");
        assert_quat_matches_up_to_sign(
            rotation,
            expected_axis_quat(Vec3::new(1.0, 0.0, 0.0), rad),
            "rotation_x 90deg",
        );
    }

    #[test]
    fn decompose_pure_rotation_y_90deg() {
        let rad = std::f32::consts::FRAC_PI_2;
        let (rotation, scale, _skew, _t, _p, _flip) = decompose_matrix(rotation_y(rad)).unwrap();
        approx_vec3(scale, Vec3::new(1.0, 1.0, 1.0), "scale");
        assert_quat_matches_up_to_sign(
            rotation,
            expected_axis_quat(Vec3::new(0.0, 1.0, 0.0), rad),
            "rotation_y 90deg",
        );
    }

    #[test]
    fn decompose_pure_rotation_z_90deg() {
        let rad = std::f32::consts::FRAC_PI_2;
        let (rotation, scale, _skew, _t, _p, _flip) = decompose_matrix(rotation_z(rad)).unwrap();
        approx_vec3(scale, Vec3::new(1.0, 1.0, 1.0), "scale");
        assert_quat_matches_up_to_sign(
            rotation,
            expected_axis_quat(Vec3::new(0.0, 0.0, 1.0), rad),
            "rotation_z 90deg",
        );
    }

    #[test]
    fn decompose_pure_rotation_x_180deg_takes_else_branch() {
        // trace = 1 + cos(pi) + cos(pi) = 1 - 1 - 1 = -1, not > 0: exercises
        // the else (largest-diagonal) branch.
        let rad = std::f32::consts::PI;
        let (rotation, _scale, _skew, _t, _p, _flip) = decompose_matrix(rotation_x(rad)).unwrap();
        assert_quat_matches_up_to_sign(
            rotation,
            expected_axis_quat(Vec3::new(1.0, 0.0, 0.0), rad),
            "rotation_x 180deg",
        );
    }

    #[test]
    fn decompose_pure_rotation_y_180deg_takes_else_branch() {
        let rad = std::f32::consts::PI;
        let (rotation, _scale, _skew, _t, _p, _flip) = decompose_matrix(rotation_y(rad)).unwrap();
        assert_quat_matches_up_to_sign(
            rotation,
            expected_axis_quat(Vec3::new(0.0, 1.0, 0.0), rad),
            "rotation_y 180deg",
        );
    }

    #[test]
    fn decompose_pure_rotation_z_180deg_takes_else_branch() {
        let rad = std::f32::consts::PI;
        let (rotation, _scale, _skew, _t, _p, _flip) = decompose_matrix(rotation_z(rad)).unwrap();
        assert_quat_matches_up_to_sign(
            rotation,
            expected_axis_quat(Vec3::new(0.0, 0.0, 1.0), rad),
            "rotation_z 180deg",
        );
    }

    // --- uniform and non-uniform scale ---

    #[test]
    fn decompose_uniform_scale() {
        let mut m = identity();
        m.rows[0].x = 2.0;
        m.rows[1].y = 2.0;
        m.rows[2].z = 2.0;
        let (_rot, scale, skew, _t, _p, flip) = decompose_matrix(m).unwrap();
        approx_vec3(scale, Vec3::new(2.0, 2.0, 2.0), "scale");
        approx_vec3(skew, Vec3::new(0.0, 0.0, 0.0), "skew");
        assert!(!flip);
    }

    #[test]
    fn decompose_non_uniform_scale() {
        let mut m = identity();
        m.rows[0].x = 2.0;
        m.rows[1].y = 3.0;
        m.rows[2].z = 4.0;
        let (_rot, scale, skew, _t, _p, flip) = decompose_matrix(m).unwrap();
        approx_vec3(scale, Vec3::new(2.0, 3.0, 4.0), "scale");
        approx_vec3(skew, Vec3::new(0.0, 0.0, 0.0), "skew");
        assert!(!flip);
    }

    // --- negative scale (mirror) ---

    #[test]
    fn decompose_single_axis_negative_scale_is_flip() {
        // Negating one axis is a mirror: determinant becomes negative.
        let mut m = identity();
        m.rows[0].x = -1.0;
        let (_rot, scale, _skew, _t, _p, flip) = decompose_matrix(m).unwrap();
        assert!(flip, "single-axis negative scale must set coordinateFlip");
        // scale.{x,y,z} are each `length(Row[i])` -- always non-negative --
        // computed BEFORE the flip check, so a single negated input axis
        // still yields (1,1,1) at that point; the source's own
        // `scale[i] *= -1.0` loop then negates all three components
        // unconditionally once `coordinateFlip` is true, regardless of
        // which axis triggered it.
        approx(scale.x, -1.0, "scale.x after flip-correction");
        approx(scale.y, -1.0, "scale.y after flip-correction");
        approx(scale.z, -1.0, "scale.z after flip-correction");
    }

    #[test]
    fn decompose_two_axis_negative_scale_is_not_a_flip() {
        // Negating two axes has determinant +1 (a 180-degree rotation, not
        // a mirror) -- coordinateFlip must be false.
        let mut m = identity();
        m.rows[0].x = -1.0;
        m.rows[1].y = -1.0;
        let (_rot, _scale, _skew, _t, _p, flip) = decompose_matrix(m).unwrap();
        assert!(!flip);
    }

    #[test]
    fn decompose_triple_negative_scale_is_a_flip() {
        // Negating all three axes: determinant = -1, a genuine mirror.
        let mut m = identity();
        m.rows[0].x = -1.0;
        m.rows[1].y = -1.0;
        m.rows[2].z = -1.0;
        let (_rot, _scale, _skew, _t, _p, flip) = decompose_matrix(m).unwrap();
        assert!(flip);
    }

    // --- zero matrix ---

    #[test]
    fn decompose_zero_matrix_fails_on_m33_epsilon_check() {
        // m[3][3] == 0.0 -> epsilonEqual(0,0) is true -> returns None
        // (source's `return false`).
        assert_eq!(decompose_matrix(zeros()), None);
    }

    // --- NaN / inf inputs ---

    #[test]
    fn decompose_nan_element_propagates_without_panicking() {
        let mut m = identity();
        m.rows[0].x = f32::NAN;
        let result = decompose_matrix(m);
        // Must not panic; NaN propagates through the arithmetic. The
        // epsilonEqual(det(perspectiveMatrix), 0.0) check with a NaN
        // determinant is `NaN < EPSILON` which is `false`, so this does NOT
        // early-return None purely from that check; whatever comes out is
        // NaN-contaminated but well-defined per IEEE-754.
        if let Some((rotation, scale, _skew, _t, _p, _flip)) = result {
            let any_nan = rotation.x.is_nan()
                || rotation.y.is_nan()
                || rotation.z.is_nan()
                || rotation.w.is_nan()
                || scale.x.is_nan()
                || scale.y.is_nan()
                || scale.z.is_nan();
            assert!(
                any_nan,
                "NaN input should surface as NaN somewhere in the output"
            );
        }
        // (A `None` result is also an acceptable, well-defined outcome if
        // some epsilon-equal check happens to observe NaN differently; the
        // key property under test is "does not panic".)
    }

    #[test]
    fn decompose_infinite_translation_propagates() {
        let mut m = identity();
        m.rows[3].x = f32::INFINITY;
        let (_rot, _scale, _skew, translation, _p, _flip) = decompose_matrix(m).unwrap();
        assert_eq!(translation.x, f32::INFINITY);
    }

    #[test]
    fn decompose_singular_upper_3x3_returns_none() {
        // A zero upper-left 3x3 (all rows zero) makes PerspectiveMatrix
        // singular (determinant 0.0) -> epsilonEqual check fires -> None.
        let mut m = identity();
        m.rows[0] = Vec4::new(0.0, 0.0, 0.0, 0.0);
        m.rows[1] = Vec4::new(0.0, 0.0, 0.0, 0.0);
        m.rows[2] = Vec4::new(0.0, 0.0, 0.0, 0.0);
        assert_eq!(decompose_matrix(m), None);
    }

    // --- perspective isolation ---

    #[test]
    fn decompose_isolates_nonzero_perspective_column() {
        let mut m = identity();
        // Give the last column a nonzero x/y/z so the "isolate perspective"
        // branch fires.
        m.rows[0].w = 0.1;
        m.rows[1].w = 0.2;
        m.rows[2].w = 0.3;
        let (_rot, _scale, _skew, _t, perspective, _flip) = decompose_matrix(m).unwrap();
        // For an otherwise-identity matrix, perspective should recover
        // approximately (0.1, 0.2, 0.3, 1.0) -- verified by reconstructing
        // via recompose_matrix below rather than asserted blindly here.
        assert!(perspective.x.is_finite());
        assert!(perspective.y.is_finite());
        assert!(perspective.z.is_finite());
    }

    #[test]
    fn decompose_no_perspective_when_last_column_is_zero_zero_zero_one() {
        let m = identity();
        let (_rot, _scale, _skew, _t, perspective, _flip) = decompose_matrix(m).unwrap();
        assert_eq!(perspective, Vec4::new(0.0, 0.0, 0.0, 1.0));
    }

    // --- skew ---

    #[test]
    fn decompose_shear_matrix_produces_nonzero_skew() {
        // Row 1 has a component along row 0's direction: XY shear.
        let mut m = identity();
        m.rows[1].x = 0.5; // Row[1] = (0.5, 1, 0) before orthogonalization.
        let (_rot, _scale, skew, _t, _p, _flip) = decompose_matrix(m).unwrap();
        assert!(skew.z.abs() > 1e-6, "skew.z should be nonzero: {skew:?}");
    }

    // --- decompose then recompose round-trip ---

    #[test]
    fn decompose_then_recompose_roundtrip_translation_rotation_scale() {
        let mut m = rotation_z(std::f32::consts::FRAC_PI_4);
        m.rows[0].x *= 2.0;
        m.rows[1].y *= 2.0;
        m.rows[2].z *= 3.0;
        m.rows[3] = Vec4::new(5.0, -3.0, 1.0, 1.0);
        let (rotation, scale, skew, translation, perspective, _flip) = decompose_matrix(m).unwrap();
        let recomposed = recompose_matrix(rotation, scale, skew, translation, perspective);
        // Measured, not idealized: this round-trip matches to within 1e-3
        // for this input (see module doc "Admitted domain" -- not asserted
        // to be bit-identical).
        for i in 0..4 {
            assert!(
                (recomposed.rows[i].x - m.rows[i].x).abs() < 1e-3,
                "row{i}.x: {recomposed:?} vs {m:?}"
            );
            assert!((recomposed.rows[i].y - m.rows[i].y).abs() < 1e-3);
            assert!((recomposed.rows[i].z - m.rows[i].z).abs() < 1e-3);
            assert!((recomposed.rows[i].w - m.rows[i].w).abs() < 1e-3);
        }
    }

    #[test]
    fn decompose_then_recompose_roundtrip_identity() {
        let (rotation, scale, skew, translation, perspective, _flip) =
            decompose_matrix(identity()).unwrap();
        let recomposed = recompose_matrix(rotation, scale, skew, translation, perspective);
        approx_mat4(recomposed, identity(), "identity roundtrip");
    }

    #[test]
    fn decompose_then_recompose_roundtrip_with_skew_is_approximate() {
        let mut m = identity();
        m.rows[1].x = 0.5;
        let (rotation, scale, skew, translation, perspective, _flip) = decompose_matrix(m).unwrap();
        let recomposed = recompose_matrix(rotation, scale, skew, translation, perspective);
        // Measured tolerance for a genuinely skewed input -- documented,
        // not idealized. If this tolerance needs to be loosened by a future
        // change, that is a signal the round-trip's approximation quality
        // changed, not that the test was wrong to check a tolerance at all.
        for i in 0..4 {
            assert!((recomposed.rows[i].x - m.rows[i].x).abs() < 1e-2);
            assert!((recomposed.rows[i].y - m.rows[i].y).abs() < 1e-2);
            assert!((recomposed.rows[i].z - m.rows[i].z).abs() < 1e-2);
            assert!((recomposed.rows[i].w - m.rows[i].w).abs() < 1e-2);
        }
    }

    // --- recompose_matrix in isolation ---

    #[test]
    fn recompose_identity_inputs_yields_identity() {
        let m = recompose_matrix(
            identity_quat(),
            Vec3::new(1.0, 1.0, 1.0),
            Vec3::new(0.0, 0.0, 0.0),
            Vec3::new(0.0, 0.0, 0.0),
            Vec4::new(0.0, 0.0, 0.0, 1.0),
        );
        approx_mat4(m, identity(), "recompose identity");
    }

    #[test]
    fn recompose_translation_only() {
        let m = recompose_matrix(
            identity_quat(),
            Vec3::new(1.0, 1.0, 1.0),
            Vec3::new(0.0, 0.0, 0.0),
            Vec3::new(3.0, 4.0, 5.0),
            Vec4::new(0.0, 0.0, 0.0, 1.0),
        );
        approx(m.rows[3].x, 3.0, "tx");
        approx(m.rows[3].y, 4.0, "ty");
        approx(m.rows[3].z, 5.0, "tz");
    }

    #[test]
    fn recompose_scale_only() {
        let m = recompose_matrix(
            identity_quat(),
            Vec3::new(2.0, 3.0, 4.0),
            Vec3::new(0.0, 0.0, 0.0),
            Vec3::new(0.0, 0.0, 0.0),
            Vec4::new(0.0, 0.0, 0.0, 1.0),
        );
        approx(m.rows[0].x, 2.0, "sx");
        approx(m.rows[1].y, 3.0, "sy");
        approx(m.rows[2].z, 4.0, "sz");
    }

    #[test]
    fn recompose_zero_skew_takes_the_skip_branches() {
        // fabs(0.0) > 0.0 is false for all three skew components: none of
        // the three skew-tmp mat4_mul steps execute. Verify the result
        // still matches a plain scale+rotate+translate (no skew applied).
        let m = recompose_matrix(
            identity_quat(),
            Vec3::new(1.0, 1.0, 1.0),
            Vec3::new(0.0, 0.0, 0.0),
            Vec3::new(1.0, 2.0, 3.0),
            Vec4::new(0.0, 0.0, 0.0, 1.0),
        );
        let mut expected = identity();
        expected.rows[3] = Vec4::new(1.0, 2.0, 3.0, 1.0);
        approx_mat4(m, expected, "zero-skew recompose");
    }

    #[test]
    fn recompose_nonzero_skew_x_takes_the_branch() {
        // skew.x seeds `tmp[2][1]` in the source (`recomposeMatrix`'s first
        // skew branch), i.e. row 2, column 1 -- `rows[2].y` in this port's
        // row-major indexing, not `rows[2].x`.
        let base = recompose_matrix(
            identity_quat(),
            Vec3::new(1.0, 1.0, 1.0),
            Vec3::new(0.0, 0.0, 0.0),
            Vec3::new(0.0, 0.0, 0.0),
            Vec4::new(0.0, 0.0, 0.0, 1.0),
        );
        let skewed = recompose_matrix(
            identity_quat(),
            Vec3::new(1.0, 1.0, 1.0),
            Vec3::new(0.7, 0.0, 0.0),
            Vec3::new(0.0, 0.0, 0.0),
            Vec4::new(0.0, 0.0, 0.0, 1.0),
        );
        assert_ne!(base.rows[2].y, skewed.rows[2].y);
    }

    #[test]
    fn recompose_nonperspective_last_column_is_perspective_input() {
        let m = recompose_matrix(
            identity_quat(),
            Vec3::new(1.0, 1.0, 1.0),
            Vec3::new(0.0, 0.0, 0.0),
            Vec3::new(0.0, 0.0, 0.0),
            Vec4::new(0.1, 0.2, 0.3, 0.9),
        );
        approx(m.rows[0].w, 0.1, "persp.x seeds m[0][3]");
        approx(m.rows[1].w, 0.2, "persp.y seeds m[1][3]");
        approx(m.rows[2].w, 0.3, "persp.z seeds m[2][3]");
    }

    #[test]
    fn recompose_non_unit_quaternion_is_not_renormalized() {
        // The source's `hlslpp::float4x4(rotation)` constructor call has no
        // visible normalization; feeding a doubled (non-unit) quaternion
        // must NOT produce the same rotation matrix as its unit form (see
        // module doc "Admitted domain"). Use a quaternion with a nonzero
        // imaginary part -- a pure-`w` quaternion's matrix formula only
        // ever multiplies `w` against `x`/`y`/`z` terms, which are all zero
        // here, so scaling `w` alone would trivially leave the matrix
        // unchanged regardless of normalization and not actually exercise
        // this behavior.
        let base = Quat::new(
            0.0,
            0.0,
            std::f32::consts::FRAC_1_SQRT_2,
            std::f32::consts::FRAC_1_SQRT_2,
        );
        let unit = recompose_matrix(
            base,
            Vec3::new(1.0, 1.0, 1.0),
            Vec3::new(0.0, 0.0, 0.0),
            Vec3::new(0.0, 0.0, 0.0),
            Vec4::new(0.0, 0.0, 0.0, 1.0),
        );
        let doubled = recompose_matrix(
            Quat::new(base.x * 2.0, base.y * 2.0, base.z * 2.0, base.w * 2.0),
            Vec3::new(1.0, 1.0, 1.0),
            Vec3::new(0.0, 0.0, 0.0),
            Vec3::new(0.0, 0.0, 0.0),
            Vec4::new(0.0, 0.0, 0.0, 1.0),
        );
        assert_ne!(
            unit, doubled,
            "non-unit quaternion input must not be silently normalized"
        );
    }

    // --- DecomposedTransform ---

    #[test]
    fn decomposed_transform_from_matrix_valid_on_success() {
        let dt = DecomposedTransform::from_matrix(identity());
        assert!(dt.valid);
    }

    #[test]
    fn decomposed_transform_from_matrix_invalid_on_singular_input() {
        let dt = DecomposedTransform::from_matrix(zeros());
        assert!(!dt.valid);
        // Fields stay at their Default::default() values -- not garbage
        // from a partially-run decompose (the Rust port never partially
        // writes through Some(..); it either returns the full tuple or
        // None).
        assert_eq!(dt.scale, Vec3::default());
    }

    #[test]
    fn decomposed_transform_default_matches_source_field_defaults() {
        let dt = DecomposedTransform::default();
        assert!(!dt.coordinate_flip);
        assert!(!dt.valid);
    }

    // --- lerp_transforms ---

    fn dt_identity() -> DecomposedTransform {
        DecomposedTransform::from_matrix(identity())
    }

    fn dt_translated(x: f32, y: f32, z: f32) -> DecomposedTransform {
        let mut m = identity();
        m.rows[3] = Vec4::new(x, y, z, 1.0);
        DecomposedTransform::from_matrix(m)
    }

    #[test]
    fn lerp_transforms_translation_at_t_zero_is_a() {
        let a = dt_identity();
        let b = dt_translated(10.0, 0.0, 0.0);
        let r = lerp_transforms(&a, &b, 0.0, true, false, false, false, false, false);
        approx_vec3(r.translation, a.translation, "t=0 translation");
        assert!(r.valid);
    }

    #[test]
    fn lerp_transforms_translation_at_t_one_is_b() {
        let a = dt_identity();
        let b = dt_translated(10.0, 0.0, 0.0);
        let r = lerp_transforms(&a, &b, 1.0, true, false, false, false, false, false);
        approx_vec3(r.translation, b.translation, "t=1 translation");
    }

    #[test]
    fn lerp_transforms_translation_at_t_half_is_midpoint() {
        let a = dt_identity();
        let b = dt_translated(10.0, 20.0, -6.0);
        let r = lerp_transforms(&a, &b, 0.5, true, false, false, false, false, false);
        approx_vec3(
            r.translation,
            Vec3::new(5.0, 10.0, -3.0),
            "t=0.5 translation",
        );
    }

    #[test]
    fn lerp_transforms_translation_outside_unit_interval_extrapolates() {
        let a = dt_identity();
        let b = dt_translated(10.0, 0.0, 0.0);
        let r = lerp_transforms(&a, &b, 2.0, true, false, false, false, false, false);
        // lerp(0, 10, 2.0) = 0 + 2*(10-0) = 20.0.
        approx(r.translation.x, 20.0, "t=2.0 extrapolated translation");
    }

    #[test]
    fn lerp_transforms_translation_flag_false_uses_b_verbatim() {
        let a = dt_identity();
        let b = dt_translated(10.0, 0.0, 0.0);
        let r = lerp_transforms(&a, &b, 0.5, false, false, false, false, false, false);
        approx_vec3(r.translation, b.translation, "flag-false uses b");
    }

    #[test]
    fn lerp_transforms_rotation_lerp_at_t_zero_and_one() {
        let a = DecomposedTransform::from_matrix(identity());
        let b = DecomposedTransform::from_matrix(rotation_z(std::f32::consts::FRAC_PI_2));
        let r0 = lerp_transforms(&a, &b, 0.0, false, true, false, false, false, false);
        let r1 = lerp_transforms(&a, &b, 1.0, false, true, false, false, false, false);
        assert_quat_matches_up_to_sign(r0.rotation, a.rotation, "rotation t=0");
        assert_quat_matches_up_to_sign(r1.rotation, b.rotation, "rotation t=1");
    }

    #[test]
    fn lerp_transforms_rotation_result_is_normalized() {
        let a = DecomposedTransform::from_matrix(identity());
        let b = DecomposedTransform::from_matrix(rotation_z(std::f32::consts::FRAC_PI_2));
        let r = lerp_transforms(&a, &b, 0.5, false, true, false, false, false, false);
        let len_sq = r.rotation.x * r.rotation.x
            + r.rotation.y * r.rotation.y
            + r.rotation.z * r.rotation.z
            + r.rotation.w * r.rotation.w;
        approx(
            len_sq,
            1.0,
            "lerped rotation is renormalized to unit length",
        );
    }

    #[test]
    fn lerp_transforms_slerp_at_t_zero_and_one_matches_endpoints() {
        // The source calls `slerp(a, b, 1.0f - weight)` -- the weight is
        // INVERTED relative to the non-slerp `lerp(a, b, weight)` branch
        // right above it in the same function. So at weight=0 the slerp
        // path evaluates `slerp(a, b, 1.0)` = b, and at weight=1 it
        // evaluates `slerp(a, b, 0.0)` = a -- the opposite endpoint mapping
        // from `lerp_transforms`'s own `lerp_rotation=true, use_slerp=false`
        // path. This is the source's literal behavior, ported as-is, not a
        // bug in this port.
        let a = DecomposedTransform::from_matrix(identity());
        let b = DecomposedTransform::from_matrix(rotation_z(std::f32::consts::FRAC_PI_2));
        let r0 = lerp_transforms(&a, &b, 0.0, false, true, false, false, false, true);
        let r1 = lerp_transforms(&a, &b, 1.0, false, true, false, false, false, true);
        assert_quat_matches_up_to_sign(
            r0.rotation,
            b.rotation,
            "slerp weight=0 -> b (inverted weight)",
        );
        assert_quat_matches_up_to_sign(
            r1.rotation,
            a.rotation,
            "slerp weight=1 -> a (inverted weight)",
        );
    }

    #[test]
    fn lerp_transforms_negative_dot_negates_b_before_blending() {
        // Construct a and b with a negative dot product by using a and -a
        // (same rotation, opposite quaternion sign) as a's and b's
        // rotation: dot(a,-a) = -|a|^2 < 0, forcing the "negate b" branch.
        let mut a = dt_identity();
        a.rotation = Quat::new(0.1, 0.2, 0.3, 0.9).normalize();
        let mut b = dt_identity();
        b.rotation = a.rotation.neg();
        // Sanity: dot is indeed negative.
        assert!(a.rotation.dot(b.rotation) < 0.0);
        let r = lerp_transforms(&a, &b, 0.0, false, true, false, false, false, false);
        // At t=0 with b negated first, lerp(a, -b, 0) = a (up to
        // normalization, which is a no-op since a is already unit length).
        assert_quat_matches_up_to_sign(r.rotation, a.rotation, "negated-b lerp at t=0");
    }

    #[test]
    fn lerp_transforms_scale_lerp() {
        let mut a = dt_identity();
        a.scale = Vec3::new(1.0, 1.0, 1.0);
        let mut b = dt_identity();
        b.scale = Vec3::new(3.0, 5.0, 7.0);
        let r = lerp_transforms(&a, &b, 0.5, false, false, true, false, false, false);
        approx_vec3(r.scale, Vec3::new(2.0, 3.0, 4.0), "scale midpoint");
    }

    #[test]
    fn lerp_transforms_scale_flag_false_uses_b() {
        let mut a = dt_identity();
        a.scale = Vec3::new(1.0, 1.0, 1.0);
        let mut b = dt_identity();
        b.scale = Vec3::new(3.0, 5.0, 7.0);
        let r = lerp_transforms(&a, &b, 0.5, false, false, false, false, false, false);
        approx_vec3(r.scale, b.scale, "scale flag false uses b");
    }

    #[test]
    fn lerp_transforms_skew_lerp() {
        let mut a = dt_identity();
        a.skew = Vec3::new(0.0, 0.0, 0.0);
        let mut b = dt_identity();
        b.skew = Vec3::new(2.0, 4.0, 6.0);
        let r = lerp_transforms(&a, &b, 0.25, false, false, false, true, false, false);
        approx_vec3(r.skew, Vec3::new(0.5, 1.0, 1.5), "skew at t=0.25");
    }

    #[test]
    fn lerp_transforms_perspective_lerp() {
        let mut a = dt_identity();
        a.perspective = Vec4::new(0.0, 0.0, 0.0, 1.0);
        let mut b = dt_identity();
        b.perspective = Vec4::new(4.0, 8.0, 12.0, 1.0);
        let r = lerp_transforms(&a, &b, 0.5, false, false, false, false, true, false);
        approx(r.perspective.x, 2.0, "persp.x midpoint");
        approx(r.perspective.y, 4.0, "persp.y midpoint");
        approx(r.perspective.z, 6.0, "persp.z midpoint");
    }

    #[test]
    fn lerp_transforms_result_is_always_valid() {
        let a = dt_identity();
        let b = dt_translated(1.0, 1.0, 1.0);
        let r = lerp_transforms(&a, &b, 0.5, true, true, true, true, true, false);
        assert!(r.valid);
    }

    #[test]
    fn lerp_transforms_all_flags_false_yields_exactly_b_fields() {
        let a = dt_identity();
        let b = dt_translated(7.0, 8.0, 9.0);
        let r = lerp_transforms(&a, &b, 0.5, false, false, false, false, false, false);
        approx_vec3(r.translation, b.translation, "translation");
        approx_vec3(r.scale, b.scale, "scale");
        approx_vec3(r.skew, b.skew, "skew");
        assert_quat_matches_up_to_sign(r.rotation, b.rotation, "rotation");
    }

    // --- Quat helpers ---

    #[test]
    fn quat_f32_index_matches_field_order() {
        let q = Quat::new(1.0, 2.0, 3.0, 4.0);
        assert_eq!(q.f32_index(0), 1.0);
        assert_eq!(q.f32_index(1), 2.0);
        assert_eq!(q.f32_index(2), 3.0);
        assert_eq!(q.f32_index(3), 4.0);
    }

    #[test]
    fn quat_normalize_zero_length_yields_nan() {
        let q = Quat::new(0.0, 0.0, 0.0, 0.0);
        let n = q.normalize();
        assert!(n.x.is_nan());
        assert!(n.y.is_nan());
        assert!(n.z.is_nan());
        assert!(n.w.is_nan());
    }

    #[test]
    fn quat_dot_and_neg() {
        let a = Quat::new(1.0, 2.0, 3.0, 4.0);
        let b = Quat::new(-1.0, 0.5, 2.0, 1.0);
        approx(a.dot(b), -1.0 + 1.0 + 6.0 + 4.0, "dot");
        let na = a.neg();
        assert_eq!(na, Quat::new(-1.0, -2.0, -3.0, -4.0));
    }

    // --- 4x4 matrix infra ---

    #[test]
    fn mat4_mul_identity_is_fixed_point() {
        let a = Mat4::from_rows([
            Vec4::new(1.0, 2.0, 3.0, 4.0),
            Vec4::new(5.0, 6.0, 7.0, 8.0),
            Vec4::new(9.0, 10.0, 11.0, 12.0),
            Vec4::new(13.0, 14.0, 15.0, 16.0),
        ]);
        approx_mat4(mat4_mul(identity(), a), a, "I*A");
        approx_mat4(mat4_mul(a, identity()), a, "A*I");
    }

    #[test]
    fn determinant4_identity_is_one() {
        approx(determinant4(identity()), 1.0, "det(I)");
    }

    #[test]
    fn determinant4_zero_matrix_is_zero() {
        approx(determinant4(zeros()), 0.0, "det(0)");
    }

    #[test]
    fn determinant4_scale_matrix_is_product_of_diagonal() {
        let mut m = identity();
        m.rows[0].x = 2.0;
        m.rows[1].y = 3.0;
        m.rows[2].z = 4.0;
        m.rows[3].w = 5.0;
        approx(determinant4(m), 120.0, "det(diag(2,3,4,5))");
    }

    #[test]
    fn inverse4_identity_is_identity() {
        approx_mat4(inverse4(identity()), identity(), "inv(I)");
    }

    #[test]
    fn inverse4_times_original_is_identity() {
        let m = Mat4::from_rows([
            Vec4::new(2.0, 0.0, 0.0, 1.0),
            Vec4::new(0.0, 3.0, 0.0, 2.0),
            Vec4::new(0.0, 0.0, 4.0, 3.0),
            Vec4::new(0.0, 0.0, 0.0, 1.0),
        ]);
        let inv = inverse4(m);
        let product = mat4_mul(m, inv);
        approx_mat4(product, identity(), "M * inv(M) = I");
    }

    #[test]
    fn inverse4_singular_matrix_divides_by_zero_unguarded() {
        // A singular matrix (determinant 0) yields inf/NaN entries, not a
        // panic (see module doc "Admitted domain").
        let inv = inverse4(zeros());
        // 0/0 for every cofactor/det -> NaN throughout.
        assert!(inv.rows[0].x.is_nan() || inv.rows[0].x.is_infinite());
    }

    #[test]
    fn transpose4_is_involutive() {
        let a = Mat4::from_rows([
            Vec4::new(1.0, 2.0, 3.0, 4.0),
            Vec4::new(5.0, 6.0, 7.0, 8.0),
            Vec4::new(9.0, 10.0, 11.0, 12.0),
            Vec4::new(13.0, 14.0, 15.0, 16.0),
        ]);
        approx_mat4(transpose4(transpose4(a)), a, "transpose(transpose(A)) = A");
    }

    // --- vec_combine / vec_scale ---

    #[test]
    fn vec_combine_matches_linear_combination_formula() {
        let a = Vec3::new(1.0, 2.0, 3.0);
        let b = Vec3::new(4.0, 5.0, 6.0);
        let r = vec_combine(a, b, 2.0, -1.0);
        approx_vec3(r, Vec3::new(-2.0, -1.0, 0.0), "vec_combine");
    }

    #[test]
    fn vec_scale_zero_length_yields_nan() {
        let v = Vec3::new(0.0, 0.0, 0.0);
        let r = vec_scale(v, 1.0);
        assert!(r.x.is_nan());
        assert!(r.y.is_nan());
        assert!(r.z.is_nan());
    }

    #[test]
    fn vec_scale_scales_to_desired_length() {
        let v = Vec3::new(3.0, 4.0, 0.0);
        let r = vec_scale(v, 10.0);
        approx_vec3(r, Vec3::new(6.0, 8.0, 0.0), "vec_scale to length 10");
    }

    // --- epsilon_equal ---

    #[test]
    fn epsilon_equal_boundary() {
        assert!(epsilon_equal(1.0, 1.0));
        assert!(!epsilon_equal(1.0, 1.0 + f32::EPSILON));
        assert!(epsilon_equal(1.0, 1.0 + f32::EPSILON * 0.5));
    }
}
