//! Literal port of RT64's RSP matrix-stack algebra: `RSP::matrixCommon`'s
//! projection/model branches, `RSP::pushProjectionMatrix`/
//! `RSP::popProjectionMatrix`, `RSP::computeModelViewProj`, plus the
//! deferred `matrixDecomposeViewProj` helper it calls -- a literal port of
//! the permitted MIT RT64 Rust-port source pinned at commit
//! `5473732a822a4423b5696e7cb18fecc425a59875` (`docs/RT64-PORT-AUTHORITY.md`),
//! `src/hle/rt64_rsp.cpp`/`.h` (SHA-256 of the whole files,
//! `7dfdf40254d44d92c247d9c876bb8ca55995927ad534981bd48868bb44f1f695` /
//! `832c092bf7021ec08a46de85c95d9973b69fa7c560ca96e43215c2fb18f54d95`), plus
//! `matrixDecomposeViewProj` from `src/common/rt64_math.cpp` (SHA-256 of the
//! whole file, `d32abc9572001870b4144ffa49e832589858de0830dbb0d008761ad15a76364b`,
//! same file already cited by `rt64_math.rs`/`rt64_math_matrix.rs`):
//!
//! ```text
//! // src/hle/rt64_rsp.h:25-26
//! #define RSP_MATRIX_STACK_SIZE       32
//! #define RSP_EXTENDED_STACK_SIZE     16
//!
//! // src/hle/rt64_rsp.cpp:134-188
//! void RSP::matrixCommon(const hlslpp::float4x4 &floatMatrix, uint32_t address, uint8_t params) {
//!     // Projection matrix.
//!     hlslpp::float4x4 &viewMatrix = viewMatrixStack[projectionMatrixStackSize - 1];
//!     hlslpp::float4x4 &projMatrix = projMatrixStack[projectionMatrixStackSize - 1];
//!     hlslpp::float4x4 &viewProjMatrix = viewProjMatrixStack[projectionMatrixStackSize - 1];
//!     uint32_t &projectionMatrixSegmentedAddress = projectionMatrixSegmentedAddressStack[projectionMatrixStackSize - 1];
//!     uint32_t &projectionMatrixPhysicalAddress = projectionMatrixPhysicalAddressStack[projectionMatrixStackSize - 1];
//!     if (params & projMask) {
//!         if (params & loadMask) {
//!             viewProjMatrix = floatMatrix;
//!
//!             if (isMatrixViewProj(floatMatrix)) {
//!                 matrixDecomposeViewProj(floatMatrix, viewMatrix, projMatrix);
//!             }
//!             else {
//!                 projMatrix = floatMatrix;
//!                 viewMatrix = hlslpp::float4x4::identity();
//!             }
//!         }
//!         else {
//!             viewProjMatrix = hlslpp::mul(floatMatrix, viewProjMatrix);
//!
//!             if (isMatrixAffine(floatMatrix) && !isMatrixIdentity(floatMatrix)) {
//!                 viewMatrix = hlslpp::mul(floatMatrix, viewMatrix);
//!             }
//!             else {
//!                 projMatrix = hlslpp::mul(floatMatrix, projMatrix);
//!             }
//!         }
//!
//!         projectionMatrixSegmentedAddress = address;
//!         projectionMatrixPhysicalAddress = fromSegmentedMasked(address);
//!         projectionMatrixChanged = true;
//!         projectionMatrixInversed = false;
//!     }
//!     // Modelview matrix.
//!     else {
//!         if ((params & pushMask) && (modelMatrixStackSize < RSP_MATRIX_STACK_SIZE)) {
//!             modelMatrixStackSize++;
//!             modelMatrixStack[modelMatrixStackSize - 1] = modelMatrixStack[modelMatrixStackSize - 2];
//!         }
//!
//!         if (params & loadMask) {
//!             modelMatrixStack[modelMatrixStackSize - 1] = floatMatrix;
//!         }
//!         else {
//!             modelMatrixStack[modelMatrixStackSize - 1] = hlslpp::mul(floatMatrix, modelMatrixStack[modelMatrixStackSize - 1]);
//!         }
//!
//!         modelMatrixSegmentedAddressStack[modelMatrixStackSize - 1] = address;
//!         modelMatrixPhysicalAddressStack[modelMatrixStackSize - 1] = fromSegmentedMasked(address);
//!     }
//!
//!     modelViewProjChanged = true;
//! }
//!
//! // src/hle/rt64_rsp.cpp:219-238
//! void RSP::pushProjectionMatrix() {
//!     if (projectionMatrixStackSize < RSP_EXTENDED_STACK_SIZE) {
//!         viewMatrixStack[projectionMatrixStackSize] = viewMatrixStack[projectionMatrixStackSize - 1];
//!         projMatrixStack[projectionMatrixStackSize] = projMatrixStack[projectionMatrixStackSize - 1];
//!         viewProjMatrixStack[projectionMatrixStackSize] = viewProjMatrixStack[projectionMatrixStackSize - 1];
//!         invViewProjMatrixStack[projectionMatrixStackSize] = invViewProjMatrixStack[projectionMatrixStackSize - 1];
//!         projectionMatrixSegmentedAddressStack[projectionMatrixStackSize] = projectionMatrixSegmentedAddressStack[projectionMatrixStackSize - 1];
//!         projectionMatrixPhysicalAddressStack[projectionMatrixStackSize] = projectionMatrixPhysicalAddressStack[projectionMatrixStackSize - 1];
//!         projectionMatrixStackSize++;
//!     }
//! }
//!
//! void RSP::popProjectionMatrix() {
//!     if (projectionMatrixStackSize > 1) {
//!         projectionMatrixStackSize--;
//!         modelViewProjChanged = true;
//!         projectionMatrixChanged = true;
//!         projectionMatrixInversed = false;
//!     }
//! }
//!
//! // src/hle/rt64_rsp.cpp:344-349
//! void RSP::computeModelViewProj() {
//!     const hlslpp::float4x4 &viewProjMatrix = viewProjMatrixStack[projectionMatrixStackSize - 1];
//!     modelViewProjMatrix = hlslpp::mul(modelMatrixStack[modelMatrixStackSize - 1], viewProjMatrix);
//!     modelViewProjInserted = false;
//!     modelViewProjChanged = false;
//! }
//!
//! // src/common/rt64_math.cpp:44-75
//! void matrixDecomposeViewProj(const hlslpp::float4x4 &vp, hlslpp::float4x4 &v, hlslpp::float4x4 &p) {
//!     v = hlslpp::float4x4::identity();
//!     p = hlslpp::float4x4::identity();
//!
//!     p[2][3] = -1.0f;
//!     p[3][3] = 0.0f;
//!     v[0][2] = -vp[0][3];
//!     v[1][2] = -vp[1][3];
//!     v[2][2] = -vp[2][3];
//!     v[3][2] = -vp[3][3];
//!
//!     p[2][2] = vp[0][2] / v[0][2];
//!     p[3][2] = vp[3][2] - p[2][2] * v[3][2];
//!
//!     p[0][0] = sqrtf(sqr(vp[0][0]) + sqr(vp[1][0]) + sqr(vp[2][0]));
//!     p[1][1] = sqrtf(sqr(vp[0][1]) + sqr(vp[1][1]) + sqr(vp[2][1]));
//!
//!     v[0][0] = vp[0][0] / p[0][0];
//!     v[1][0] = vp[1][0] / p[0][0];
//!     v[2][0] = vp[2][0] / p[0][0];
//!     v[3][0] = vp[3][0] / p[0][0];
//!
//!     v[0][1] = vp[0][1] / p[1][1];
//!     v[1][1] = vp[1][1] / p[1][1];
//!     v[2][1] = vp[2][1] / p[1][1];
//!     v[3][1] = vp[3][1] / p[1][1];
//!
//!     if (matrixIsNaN(v) || matrixIsNaN(p)) {
//!         v = hlslpp::float4x4::identity();
//!         p = vp;
//!     }
//! }
//! ```
//!
//! This is a **second partial port** of `rt64_rsp.cpp`. `rt64_rsp_segment.rs`
//! (ticket M5.1) already ported this same 1,314-line source file's
//! segmented-address translation cluster (`fromSegmented`/
//! `fromSegmentedMasked`/`fromSegmentedMaskedPD`/`maskPhysicalAddress`/
//! `setSegment`) and explicitly enumerated `matrixCommon` (among others) as
//! deliberately NOT ported by that module. This module ports exactly that
//! deferred function plus its three small siblings
//! (`pushProjectionMatrix`/`popProjectionMatrix`/`computeModelViewProj`);
//! together the two modules still cover only a small fraction of
//! `rt64_rsp.cpp` -- see "Nonclaims" below for the (large) remainder.
//!
//! **Reuse, not new type.** This module reuses `fn64_render_ir::{Mat4,
//! Vec4}` directly (same convention as `rt64_math.rs`/`rt64_math_matrix.rs`:
//! `Mat4` is row-major, `rows[i].{x,y,z,w}` = row `i`'s four columns, an
//! HLSL `m[i][j]` read is `m.rows[i].{x,y,z,w}` for `j = 0..3`), and reuses
//! `rt64_math::{is_matrix_affine, is_matrix_identity, is_matrix_view_proj,
//! sqr, matrix_is_nan}` verbatim (all `pub fn` in that module, per the
//! M5.2 ticket's "Prerequisites ALREADY LANDED" note) rather than
//! reimplementing any of them. No helper needed by this module was found to
//! be private in a reused sibling module: `rt64_math_decompose.rs`'s
//! `mat4_mul` is module-private (`fn`, not `pub fn`), so this module adds
//! its own local `mat4_mul` rather than depending on another ticket's
//! internal helper (the same "no cross-module dependency on a sibling
//! ticket's private surface" precedent `rt64_math_decompose.rs` itself
//! documents for `epsilon_equal`/`matrixTranslation`/`matrixScale`). This
//! local `mat4_mul` is byte-for-byte the same formula as the sibling
//! module's (`mat4_mul(A,B)[i][j] = sum_k A[i][k]*B[k][j]`, ordinary
//! structural matrix multiplication, consistent with
//! `Mat4::transform_point`'s established `mul(matrix, vector) = M·v`
//! reading of HLSL's `mul` intrinsic), not an independently-derived
//! convention.
//!
//! This module does **not** reuse `rt64_rsp_segment.rs`'s
//! `mask_physical_address`/`SegmentTable` machinery to compute
//! `projectionMatrixPhysicalAddress`/`modelMatrixPhysicalAddressStack`
//! entries. Per the M5.2 ticket text ("Inject the segment-translated
//! address rather than calling into State; M5.1 owns that translation"),
//! every function below that needs `fromSegmentedMasked(address)`'s result
//! takes it as an already-computed `physical_address: u32` parameter
//! supplied by the caller, exactly mirroring the C++ `state->` indirection
//! this port deliberately does not reproduce (see "Nonclaims").
//!
//! ## Admitted domain
//!
//! - **Matrix multiplication order: `hlslpp::mul(A, B)` is left-to-right
//!   ordinary structural product, `A·B`, not `B·A`.** This is not guessed;
//!   it follows from two facts already fixed elsewhere in this crate: (1)
//!   `fn64_render_ir::Mat4::transform_point` is documented and implemented
//!   as `mul(matrix, vector) = M·v` (`rsp_math.rs:106-126`); (2)
//!   `rt64_math_decompose.rs` extends that exact same convention to
//!   matrix-times-matrix, stating explicitly that its `mat4_mul(A, B)`
//!   "extends that same convention to two matrices as ordinary structural
//!   matrix multiplication, `mat4_mul(A,B)[i][j] = sum_k A[i][k]*B[k][j]`,
//!   so that `mat4_mul(A,B)` applied to a column vector via
//!   `transform_point` equals `A·(B·v)` -- consistent associativity with
//!   the existing `mul(matrix,vector)` reading, not a new or conflicting
//!   convention" (`rt64_math_decompose.rs:326-339`). This module's local
//!   `mat4_mul` reproduces that identical formula. Consequently
//!   `hlslpp::mul(floatMatrix, viewProjMatrix)` is ported as
//!   `mat4_mul(float_matrix, view_proj_matrix)` with `float_matrix` as the
//!   **left** operand in every one of `matrixCommon`'s four `mul` call
//!   sites and `computeModelViewProj`'s one -- getting this backwards would
//!   transpose every downstream transform, so it is called out here
//!   explicitly rather than left implicit.
//! - **Float addition/multiplication is not reassociated anywhere.** Every
//!   arithmetic expression (`p[3][2] = vp[3][2] - p[2][2] * v[3][2]`,
//!   `sqrtf(sqr(a) + sqr(b) + sqr(c))`, `mat4_mul`'s four-term row/column
//!   dot products) preserves the exact left-to-right evaluation order the
//!   C++ source performs; the four-term dot products inside `mat4_mul`
//!   accumulate in the same `((a+b)+c)+d` order as the source's implied
//!   `hlslpp` row/column reduction, matching `Mat4::transform_point`'s own
//!   already-established four-term addition order in this crate.
//! - **`matrixDecomposeViewProj`'s six unguarded divisions
//!   (`vp[0][2] / v[0][2]`, `vp[i][0] / p[0][0]` x4, `vp[i][1] / p[1][1]`
//!   x4 -- ten total, not six; corrected count below) are ported as plain
//!   IEEE-754 `/`, no guard added.** For a singular/degenerate `vp` (e.g.
//!   `vp[0][3] == 0`, making `v[0][2] == -vp[0][3] == 0.0`, or `p[0][0]`/
//!   `p[1][1]` evaluating to `0.0` when the corresponding `vp` column has
//!   zero magnitude), these divisions legitimately produce `±inf` or `NaN`
//!   per plain IEEE-754 division semantics (matching `rt64_math.rs`'s
//!   established "unguarded upstream arithmetic, not an invented guard"
//!   policy for `barycentric_coordinates`/`fov_from_proj`). This is exactly
//!   the case the source's own subsequent `matrixIsNaN(v) || matrixIsNaN(p)`
//!   check exists to catch and fall back from -- ported here as a real
//!   branch (`v = identity(); p = vp;`), not an error path, and unit-tested
//!   directly with a singular `vp` (`v[0][2] == 0.0`) below. (Ten divisions
//!   total: one for `p[2][2]`, four for `v[i][0]` via `p[0][0]`, four for
//!   `v[i][1]` via `p[1][1]`, and `p[3][2]`'s subtraction has no division --
//!   corrected from an earlier miscount in drafting this note.)
//! - **`sqrtf` of a negative argument** (reachable if `sqr(a)+sqr(b)+sqr(c)`
//!   were negative -- it cannot be, since `sqr` squares each term and IEEE
//!   float addition of three non-negative finite values stays non-negative
//!   except when a `NaN` operand is present, in which case the sum is
//!   already `NaN` and `sqrtf(NaN) == NaN`) is not a reachable branch here;
//!   `f32::sqrt` on a `NaN` input returns `NaN` (matching `sqrtf`), which is
//!   exactly the value the subsequent `matrixIsNaN` check is designed to
//!   detect. No special-case guard is added.
//! - **`hlslpp::float4x4::identity()`** is assumed to be the standard
//!   mathematical identity matrix, matching `rt64_math.rs`'s identical
//!   assumption for `is_matrix_identity` (`hlslpp` is an unpopulated
//!   submodule in every checkout available to this program; this is a
//!   stated assumption, not a verified read of `hlslpp` source).
//! - **`params & projMask`/`params & loadMask`/`params & pushMask` truth
//!   tests.** In the source, `params` is `uint8_t` and `projMask`/
//!   `loadMask`/`pushMask` are `uint32_t` RSP instance fields
//!   (`rt64_rsp.h:184-186`); C++'s usual arithmetic conversions promote
//!   `params` to `uint32_t` before the `&`, and the `if` tests the result
//!   against nonzero. This port widens `params: u8` to `u32` before each
//!   `&` (`u32::from(params) & proj_mask != 0`), matching that promotion
//!   exactly, rather than truncating the masks to `u8` (which could silently
//!   change behavior if a caller ever passed a mask with bits set above bit
//!   7 -- out of scope to prove impossible here, so the widening direction
//!   that cannot lose information is the one this port takes).
//! - **Stack-size types.** `modelMatrixStackSize`/`projectionMatrixStackSize`
//!   are C++ `int` (`rt64_rsp.h:136,143`), always non-negative in the reset
//!   state (`reset()` sets both to `1`, out of this port's scope) and only
//!   ever incremented/decremented by these ported functions under an
//!   explicit bound check on one side (`< RSP_MATRIX_STACK_SIZE` /
//!   `< RSP_EXTENDED_STACK_SIZE`) and a `> 1` guard on the other, so the
//!   value never leaves `1..=32` (model) or `1..=16` (projection) through
//!   these functions alone. Ported as `usize` (used directly as an array
//!   index, matching the C++ `[stackSize - 1]` usage) rather than `i32`,
//!   since a negative stack size is never produced by any function this
//!   module ports and `usize` avoids an extra cast at every indexing site.
//! - **`RSP_MATRIX_STACK_SIZE == 32` bounds the *model* stack;
//!   `RSP_EXTENDED_STACK_SIZE == 16` bounds the *projection* stack -- these
//!   are two different constants, not the same limit reused twice.** Easy
//!   to conflate since both stacks hold `float4x4`s and both are pushed by
//!   a "push" operation; ported as two distinct constants
//!   (`MODEL_MATRIX_STACK_SIZE`/`PROJECTION_MATRIX_STACK_SIZE`) to keep the
//!   two limits from ever being accidentally unified.
//!
//! ## Nonclaims
//!
//! No GPU, WGSL, or production wiring (this module is not called from
//! anywhere -- dead-code warnings on the unused public surface are expected
//! and correct, matching `rt64_rsp_segment.rs`'s and every other
//! characterization-first module's precedent in this crate), and no RT64
//! visual/pixel/silicon parity or performance claim.
//!
//! This is a **partial port of an already-partially-ported source file**:
//! `rt64_rsp_segment.rs` (M5.1) ported the segmented-address translation
//! cluster from this same `rt64_rsp.cpp`; this module (M5.2) ports the
//! matrix-stack algebra. Both together still leave the overwhelming
//! majority of `rt64_rsp.cpp`'s 1,314 lines unported. Specifically not
//! ported here, by category:
//!
//! - **The `matrix`/`matrixFloat` entry points** (`rt64_rsp.cpp:190-208`)
//!   that decode a `FixedMatrix`/raw `float[16]` out of RDRAM and call
//!   `matrixCommon` -- this module ports `matrixCommon` itself, taking the
//!   already-decoded `Mat4` as a parameter, not the RDRAM-reading wrappers
//!   (`state->fromRDRAM`, `FixedMatrix::toMatrix4x4` -- the latter already
//!   exists as `rt64_common.rs::FixedMatrix`, not called from here).
//! - **`popMatrix`** (`rt64_rsp.cpp:210-217`, the *model*-stack pop by
//!   `count`) is a distinct function from this module's ported
//!   `pop_projection_matrix` (the *projection*-stack pop) and is not
//!   ported here -- not named in the M5.2 ticket's findings, unlike
//!   `pushProjectionMatrix`/`popProjectionMatrix` which are.
//! - **`insertMatrix`** (`rt64_rsp.cpp:240-...`, the mid-command-stream
//!   32-bit patch into the model/viewProj/modelViewProj matrices by
//!   relative byte address) -- a distinct, considerably more involved
//!   function, not named in the M5.2 ticket's findings.
//! - **`forceMatrix`, `recalculateMatrices`, `getCurrentProjectionType`,
//!   `addCurrentProjection`, `RSP::RSP`, `RSP::reset`, `setGBI`
//!   (the call site that assigns `projMask`/`loadMask`/`pushMask` from
//!   `gbi->constants[...]`), and every other RSP-state orchestrator** --
//!   this module's functions take `proj_mask`/`load_mask`/`push_mask` as
//!   plain caller-supplied `u32` parameters, the same "decision logic over
//!   caller-supplied scalar inputs" pattern already established for
//!   `rt64_rsp_segment.rs`'s `extend_rdram: bool` parameter.
//! - **`state->fromRDRAM`/segment-table lookups.** As stated above, this
//!   module takes any already-segment-translated physical address as a
//!   plain `u32` input parameter rather than calling into `State` or
//!   `rt64_rsp_segment.rs`'s `SegmentTable`/`mask_physical_address` --
//!   M5.1 owns that translation and its call sites are integration work,
//!   out of scope here.
//! - **Vertex/geometry/display-list orchestrators, lighting, fog, viewport,
//!   other-mode, and every non-matrix-stack RSP state** -- entirely out of
//!   scope for this module, matching `rt64_rsp_segment.rs`'s equivalent
//!   disclosure.
//! - **`decomposeMatrix`/`recomposeMatrix`/`DecomposedTransform`/
//!   `lerpTransforms`** (a different `rt64_math.cpp` cluster, owned by
//!   M8.4/`rt64_math_decompose.rs`) and **`extract3x3`/`rotationFrom3x3`/
//!   `matrixDifference`/`lerpMatrix`/`lerpMatrix3x3`/`lerpMatrixComponents`**
//!   (owned by `rt64_math_matrix.rs`) are not re-ported here; this module
//!   only reuses their sibling module's already-landed
//!   `is_matrix_affine`/`is_matrix_identity`/`is_matrix_view_proj`/`sqr`/
//!   `matrix_is_nan` from `rt64_math.rs`.

use fn64_render_ir::{Mat4, Vec4};

use crate::rt64_math::{
    is_matrix_affine, is_matrix_identity, is_matrix_view_proj, matrix_is_nan, sqr,
};

/// `RSP_MATRIX_STACK_SIZE` (`rt64_rsp.h:25`): bounds the *model* matrix
/// stack (distinct from `PROJECTION_MATRIX_STACK_SIZE` -- see module doc
/// "Admitted domain").
pub const MODEL_MATRIX_STACK_SIZE: usize = 32;

/// `RSP_EXTENDED_STACK_SIZE` (`rt64_rsp.h:26`): bounds the *projection*
/// matrix stack (and several other unrelated RSP stacks not ported here).
pub const PROJECTION_MATRIX_STACK_SIZE: usize = 16;

/// `hlslpp::float4x4::identity()`: the standard 4x4 identity matrix. See
/// module doc "Admitted domain" for the assumption this rests on.
fn identity() -> Mat4 {
    Mat4::from_rows([
        Vec4::new(1.0, 0.0, 0.0, 0.0),
        Vec4::new(0.0, 1.0, 0.0, 0.0),
        Vec4::new(0.0, 0.0, 1.0, 0.0),
        Vec4::new(0.0, 0.0, 0.0, 1.0),
    ])
}

/// `hlslpp::mul(a, b)` for two `float4x4`s: ordinary structural matrix
/// product, `a` on the left. `mat4_mul(a,b)[i][j] = sum_k a[i][k]*b[k][j]`.
/// Not reused from `rt64_math_decompose.rs` because that module's
/// equivalent helper is private (`fn`, not `pub fn`); see module doc
/// "Reuse, not new type". Byte-for-byte the same formula.
fn mat4_mul(a: Mat4, b: Mat4) -> Mat4 {
    let mut rows = [Vec4::new(0.0, 0.0, 0.0, 0.0); 4];
    for i in 0..4 {
        let ai = a.rows[i];
        let a_i = [ai.x, ai.y, ai.z, ai.w];
        let mut out = [0.0f32; 4];
        for (j, out_j) in out.iter_mut().enumerate() {
            let mut sum = a_i[0] * col(b, 0, j);
            sum += a_i[1] * col(b, 1, j);
            sum += a_i[2] * col(b, 2, j);
            sum += a_i[3] * col(b, 3, j);
            *out_j = sum;
        }
        rows[i] = Vec4::new(out[0], out[1], out[2], out[3]);
    }
    Mat4::from_rows(rows)
}

/// `m[i][j]` read helper (row `i`, column `j`) matching the HLSL indexing
/// convention documented in `rt64_math.rs`.
fn col(m: Mat4, i: usize, j: usize) -> f32 {
    let row = m.rows[i];
    match j {
        0 => row.x,
        1 => row.y,
        2 => row.z,
        3 => row.w,
        _ => unreachable!("float4x4 column index out of range"),
    }
}

/// Sets `m[i][j] = value` (row `i`, column `j`).
fn set(m: &mut Mat4, i: usize, j: usize, value: f32) {
    let row = &mut m.rows[i];
    match j {
        0 => row.x = value,
        1 => row.y = value,
        2 => row.z = value,
        3 => row.w = value,
        _ => unreachable!("float4x4 column index out of range"),
    }
}

/// `RT64::matrixDecomposeViewProj` (`src/common/rt64_math.cpp:44-75`).
/// Decomposes a combined view-projection matrix `vp` into separate `view`
/// and `proj` matrices under the fixed-function N64 camera assumption
/// (see source comment context: this only holds for a specific
/// view/projection factorization, not an arbitrary `vp`). Falls back to
/// `view = identity, proj = vp` when the decomposition is singular/NaN --
/// see module doc "Admitted domain" for why the ten unguarded divisions are
/// preserved without a guard.
///
/// Returns `(view, proj)`, matching the source's `float4x4 &v, float4x4 &p`
/// out-parameters.
pub fn matrix_decompose_view_proj(vp: Mat4) -> (Mat4, Mat4) {
    let mut v = identity();
    let mut p = identity();

    set(&mut p, 2, 3, -1.0);
    set(&mut p, 3, 3, 0.0);
    set(&mut v, 0, 2, -col(vp, 0, 3));
    set(&mut v, 1, 2, -col(vp, 1, 3));
    set(&mut v, 2, 2, -col(vp, 2, 3));
    set(&mut v, 3, 2, -col(vp, 3, 3));

    set(&mut p, 2, 2, col(vp, 0, 2) / col(v, 0, 2));
    let p22 = col(p, 2, 2);
    set(&mut p, 3, 2, col(vp, 3, 2) - p22 * col(v, 3, 2));

    set(
        &mut p,
        0,
        0,
        (sqr(col(vp, 0, 0)) + sqr(col(vp, 1, 0)) + sqr(col(vp, 2, 0))).sqrt(),
    );
    set(
        &mut p,
        1,
        1,
        (sqr(col(vp, 0, 1)) + sqr(col(vp, 1, 1)) + sqr(col(vp, 2, 1))).sqrt(),
    );

    set(&mut v, 0, 0, col(vp, 0, 0) / col(p, 0, 0));
    set(&mut v, 1, 0, col(vp, 1, 0) / col(p, 0, 0));
    set(&mut v, 2, 0, col(vp, 2, 0) / col(p, 0, 0));
    set(&mut v, 3, 0, col(vp, 3, 0) / col(p, 0, 0));

    set(&mut v, 0, 1, col(vp, 0, 1) / col(p, 1, 1));
    set(&mut v, 1, 1, col(vp, 1, 1) / col(p, 1, 1));
    set(&mut v, 2, 1, col(vp, 2, 1) / col(p, 1, 1));
    set(&mut v, 3, 1, col(vp, 3, 1) / col(p, 1, 1));

    if matrix_is_nan(v) || matrix_is_nan(p) {
        v = identity();
        p = vp;
    }

    (v, p)
}

/// The projection matrix stack's per-slot state:
/// `viewMatrixStack`/`projMatrixStack`/`viewProjMatrixStack`/
/// `projectionMatrixSegmentedAddressStack`/
/// `projectionMatrixPhysicalAddressStack` at one stack index
/// (`rt64_rsp.h:137-142`). `invViewProjMatrixStack` is carried through
/// `push_projection_matrix` (the source copies it) but is never written by
/// any function this module ports, so it is represented but never mutated
/// here.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ProjectionSlot {
    pub view_matrix: Mat4,
    pub proj_matrix: Mat4,
    pub view_proj_matrix: Mat4,
    pub inv_view_proj_matrix: Mat4,
    pub segmented_address: u32,
    pub physical_address: u32,
}

impl ProjectionSlot {
    pub const fn zero() -> Self {
        let zero = Mat4::from_rows([Vec4::new(0.0, 0.0, 0.0, 0.0); 4]);
        Self {
            view_matrix: zero,
            proj_matrix: zero,
            view_proj_matrix: zero,
            inv_view_proj_matrix: zero,
            segmented_address: 0,
            physical_address: 0,
        }
    }
}

/// The projection matrix stack: `viewMatrixStack`/`projMatrixStack`/
/// `viewProjMatrixStack`/`invViewProjMatrixStack`/
/// `projectionMatrixSegmentedAddressStack`/
/// `projectionMatrixPhysicalAddressStack` plus `projectionMatrixStackSize`
/// (`rt64_rsp.h:137-143`), bounded by `PROJECTION_MATRIX_STACK_SIZE`.
#[derive(Clone, Debug, PartialEq)]
pub struct ProjectionStack {
    pub slots: [ProjectionSlot; PROJECTION_MATRIX_STACK_SIZE],
    pub size: usize,
}

impl ProjectionStack {
    /// Matches `RSP::reset`'s effective starting state for the fields this
    /// module owns: `projectionMatrixStackSize = 1`,
    /// `viewMatrixStack[0]`/`projMatrixStack[0]`/`viewProjMatrixStack[0]`/
    /// `invViewProjMatrixStack[0]` all zero matrices (`rt64_rsp.cpp:42,49-52`).
    /// `reset()` itself is out of this port's scope; this constructor only
    /// reproduces the resulting field values as a convenience starting
    /// point for tests.
    pub fn new() -> Self {
        Self {
            slots: [ProjectionSlot::zero(); PROJECTION_MATRIX_STACK_SIZE],
            size: 1,
        }
    }

    fn top(&self) -> usize {
        self.size - 1
    }
}

impl Default for ProjectionStack {
    fn default() -> Self {
        Self::new()
    }
}

/// `RSP::pushProjectionMatrix` (`rt64_rsp.cpp:219-229`). No-op past
/// `PROJECTION_MATRIX_STACK_SIZE` (`RSP_EXTENDED_STACK_SIZE == 16`).
pub fn push_projection_matrix(stack: &mut ProjectionStack) {
    if stack.size < PROJECTION_MATRIX_STACK_SIZE {
        let top = stack.slots[stack.top()];
        stack.slots[stack.size] = top;
        stack.size += 1;
    }
}

/// `RSP::popProjectionMatrix` (`rt64_rsp.cpp:231-238`). No-op at stack size
/// `1` (the source never pops the last slot). Returns
/// `(modelViewProjChanged, projectionMatrixChanged, projectionMatrixInversed)`
/// deltas as a tuple of the three `bool`s the source sets when the pop is
/// taken, `None` when it is not (i.e. `size == 1`), so callers can observe
/// exactly which branch executed without this module owning the caller's
/// full `RSP` state.
pub fn pop_projection_matrix(stack: &mut ProjectionStack) -> Option<(bool, bool, bool)> {
    if stack.size > 1 {
        stack.size -= 1;
        // modelViewProjChanged = true; projectionMatrixChanged = true; projectionMatrixInversed = false;
        Some((true, true, false))
    } else {
        None
    }
}

/// The model matrix stack: `modelMatrixStack`/
/// `modelMatrixSegmentedAddressStack`/`modelMatrixPhysicalAddressStack`
/// plus `modelMatrixStackSize` (`rt64_rsp.h:133-136`), bounded by
/// `MODEL_MATRIX_STACK_SIZE`.
#[derive(Clone, Debug, PartialEq)]
pub struct ModelStack {
    pub matrices: [Mat4; MODEL_MATRIX_STACK_SIZE],
    pub segmented_addresses: [u32; MODEL_MATRIX_STACK_SIZE],
    pub physical_addresses: [u32; MODEL_MATRIX_STACK_SIZE],
    pub size: usize,
}

impl ModelStack {
    /// Matches `RSP::reset`'s effective starting state for the fields this
    /// module owns: `modelMatrixStackSize = 1`,
    /// `modelMatrixStack.fill(hlslpp::float4x4(0.0f))`,
    /// `modelMatrixSegmentedAddressStack.fill(0)` (`rt64_rsp.cpp:41,46-48`;
    /// `modelMatrixPhysicalAddressStack` is not explicitly filled by
    /// `reset()` in the cited source lines, but starts zero as a freshly
    /// constructed `std::array` member -- reproduced as zero here too).
    /// `reset()` itself is out of this port's scope.
    pub fn new() -> Self {
        let zero = Mat4::from_rows([Vec4::new(0.0, 0.0, 0.0, 0.0); 4]);
        Self {
            matrices: [zero; MODEL_MATRIX_STACK_SIZE],
            segmented_addresses: [0; MODEL_MATRIX_STACK_SIZE],
            physical_addresses: [0; MODEL_MATRIX_STACK_SIZE],
            size: 1,
        }
    }

    fn top(&self) -> usize {
        self.size - 1
    }
}

impl Default for ModelStack {
    fn default() -> Self {
        Self::new()
    }
}

/// `RSP::matrixCommon` (`rt64_rsp.cpp:134-188`). `float_matrix` is the
/// already-decoded input matrix (source: `matrix`/`matrixFloat`'s
/// `floatMatrix`, not ported here). `address` is the raw segmented address
/// as given to the source (stored verbatim into the `*SegmentedAddress*`
/// slot). `physical_address` is the caller-supplied result of
/// `fromSegmentedMasked(address)` (M5.1 owns that translation; see module
/// doc "Reuse, not new type"). `params` is the raw command byte;
/// `proj_mask`/`load_mask`/`push_mask` are the RSP instance's
/// `projMask`/`loadMask`/`pushMask` fields.
///
/// Returns the value the source's trailing `modelViewProjChanged = true;`
/// leaves behind -- always `true`, on every path through this function,
/// exactly like the source (the assignment is unconditional, outside both
/// branches).
pub fn matrix_common(
    projection: &mut ProjectionStack,
    model: &mut ModelStack,
    float_matrix: Mat4,
    address: u32,
    physical_address: u32,
    params: u8,
    proj_mask: u32,
    load_mask: u32,
    push_mask: u32,
) -> bool {
    let params = u32::from(params);
    if params & proj_mask != 0 {
        let slot = projection.top();
        if params & load_mask != 0 {
            projection.slots[slot].view_proj_matrix = float_matrix;

            if is_matrix_view_proj(float_matrix) {
                let (v, p) = matrix_decompose_view_proj(float_matrix);
                projection.slots[slot].view_matrix = v;
                projection.slots[slot].proj_matrix = p;
            } else {
                projection.slots[slot].proj_matrix = float_matrix;
                projection.slots[slot].view_matrix = identity();
            }
        } else {
            let view_proj = projection.slots[slot].view_proj_matrix;
            projection.slots[slot].view_proj_matrix = mat4_mul(float_matrix, view_proj);

            if is_matrix_affine(float_matrix) && !is_matrix_identity(float_matrix) {
                let view = projection.slots[slot].view_matrix;
                projection.slots[slot].view_matrix = mat4_mul(float_matrix, view);
            } else {
                let proj = projection.slots[slot].proj_matrix;
                projection.slots[slot].proj_matrix = mat4_mul(float_matrix, proj);
            }
        }

        projection.slots[slot].segmented_address = address;
        projection.slots[slot].physical_address = physical_address;
        // projectionMatrixChanged = true; projectionMatrixInversed = false; (caller-observed, see below)
    } else {
        if params & push_mask != 0 && model.size < MODEL_MATRIX_STACK_SIZE {
            let top = model.matrices[model.top()];
            model.size += 1;
            model.matrices[model.top()] = top;
        }

        if params & load_mask != 0 {
            model.matrices[model.top()] = float_matrix;
        } else {
            let current = model.matrices[model.top()];
            model.matrices[model.top()] = mat4_mul(float_matrix, current);
        }

        model.segmented_addresses[model.top()] = address;
        model.physical_addresses[model.top()] = physical_address;
    }

    true
}

/// `RSP::computeModelViewProj` (`rt64_rsp.cpp:344-349`). Returns
/// `modelViewProjMatrix`, matching `mul(modelMatrixStack[top], viewProjMatrix)`
/// with the model matrix as the **left** operand. The source's
/// `modelViewProjInserted = false; modelViewProjChanged = false;` side
/// effects are not represented in this pure function's return value --
/// callers observe them as the fixed `(false, false)` pair, matching every
/// call to this function unconditionally clearing both flags.
pub fn compute_model_view_proj(projection: &ProjectionStack, model: &ModelStack) -> Mat4 {
    let view_proj = projection.slots[projection.top()].view_proj_matrix;
    let model_matrix = model.matrices[model.top()];
    mat4_mul(model_matrix, view_proj)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn zeros() -> Mat4 {
        Mat4::from_rows([Vec4::new(0.0, 0.0, 0.0, 0.0); 4])
    }

    fn diag(x: f32, y: f32, z: f32, w: f32) -> Mat4 {
        Mat4::from_rows([
            Vec4::new(x, 0.0, 0.0, 0.0),
            Vec4::new(0.0, y, 0.0, 0.0),
            Vec4::new(0.0, 0.0, z, 0.0),
            Vec4::new(0.0, 0.0, 0.0, w),
        ])
    }

    fn translation(x: f32, y: f32, z: f32) -> Mat4 {
        Mat4::from_rows([
            Vec4::new(1.0, 0.0, 0.0, 0.0),
            Vec4::new(0.0, 1.0, 0.0, 0.0),
            Vec4::new(0.0, 0.0, 1.0, 0.0),
            Vec4::new(x, y, z, 1.0),
        ])
    }

    fn arbitrary_a() -> Mat4 {
        Mat4::from_rows([
            Vec4::new(1.0, 2.0, 3.0, 4.0),
            Vec4::new(5.0, 6.0, 7.0, 8.0),
            Vec4::new(9.0, 10.0, 11.0, 12.0),
            Vec4::new(13.0, 14.0, 15.0, 16.0),
        ])
    }

    fn approx_mat4(actual: Mat4, expected: Mat4, label: &str) {
        for i in 0..4 {
            let a = actual.rows[i];
            let e = expected.rows[i];
            let av = [a.x, a.y, a.z, a.w];
            let ev = [e.x, e.y, e.z, e.w];
            for j in 0..4 {
                if ev[j].is_nan() {
                    assert!(
                        av[j].is_nan(),
                        "{label} row {i} col {j}: expected NaN, got {}",
                        av[j]
                    );
                } else if ev[j].is_infinite() {
                    assert_eq!(
                        av[j], ev[j],
                        "{label} row {i} col {j}: expected {}, got {}",
                        ev[j], av[j]
                    );
                } else {
                    assert!(
                        (av[j] - ev[j]).abs() < 1e-4,
                        "{label} row {i} col {j}: expected {}, got {}",
                        ev[j],
                        av[j]
                    );
                }
            }
        }
    }

    // --- mat4_mul ---

    #[test]
    fn mat4_mul_identity_left_is_fixed_point() {
        approx_mat4(mat4_mul(identity(), arbitrary_a()), arbitrary_a(), "I*A");
    }

    #[test]
    fn mat4_mul_identity_right_is_fixed_point() {
        approx_mat4(mat4_mul(arbitrary_a(), identity()), arbitrary_a(), "A*I");
    }

    #[test]
    fn mat4_mul_zero_is_zero() {
        approx_mat4(mat4_mul(zeros(), arbitrary_a()), zeros(), "0*A");
    }

    #[test]
    fn mat4_mul_not_commutative_for_translation_and_scale() {
        // T * S versus S * T differ in the translation row, proving operand
        // order matters and confirming which side is "left".
        let t = translation(1.0, 2.0, 3.0);
        let s = diag(2.0, 2.0, 2.0, 1.0);
        let ts = mat4_mul(t, s);
        let st = mat4_mul(s, t);
        // T*S: row 3 (translation row) of T is (1,2,3,1); multiplied by S's
        // columns (diag 2,2,2,1) gives (2,4,6,1).
        assert_eq!(ts.rows[3], Vec4::new(2.0, 4.0, 6.0, 1.0));
        // S*T: row 3 of S is (0,0,0,1); dotted with T's columns gives T's
        // translation row unchanged (1,2,3,1) since S's row 3 only picks up
        // T's row 3 via the w=1 lane.
        assert_eq!(st.rows[3], Vec4::new(1.0, 2.0, 3.0, 1.0));
        assert_ne!(ts, st);
    }

    #[test]
    fn mat4_mul_hand_computed_2x2_style_case() {
        // a = [[1,2],[3,4]] embedded top-left, b = [[5,6],[7,8]] embedded
        // top-left, rest identity. a*b top-left = [[1*5+2*7, 1*6+2*8],[3*5+4*7,3*6+4*8]]
        // = [[19,22],[43,50]].
        let a = Mat4::from_rows([
            Vec4::new(1.0, 2.0, 0.0, 0.0),
            Vec4::new(3.0, 4.0, 0.0, 0.0),
            Vec4::new(0.0, 0.0, 1.0, 0.0),
            Vec4::new(0.0, 0.0, 0.0, 1.0),
        ]);
        let b = Mat4::from_rows([
            Vec4::new(5.0, 6.0, 0.0, 0.0),
            Vec4::new(7.0, 8.0, 0.0, 0.0),
            Vec4::new(0.0, 0.0, 1.0, 0.0),
            Vec4::new(0.0, 0.0, 0.0, 1.0),
        ]);
        let ab = mat4_mul(a, b);
        assert_eq!(ab.rows[0], Vec4::new(19.0, 22.0, 0.0, 0.0));
        assert_eq!(ab.rows[1], Vec4::new(43.0, 50.0, 0.0, 0.0));
    }

    // --- matrix_decompose_view_proj ---

    #[test]
    fn matrix_decompose_view_proj_identity_view_proj() {
        // vp = identity is NOT a valid full decomposition input under this
        // formula (v[0][2] = -vp[0][3] = 0 triggers the NaN fallback), so
        // this exercises the fallback path with a hand-verifiable input:
        // identity has vp[0][3]==vp[1][3]==vp[2][3]==vp[3][3]==0 (w column
        // all zero for rows 0-2, and vp[3][3]=1). v[3][2] = -vp[3][3] = -1,
        // nonzero, but v[0][2] = -vp[0][3] = -0.0 = 0 -> p[2][2] = vp[0][2]/0
        // = 0/0 = NaN. Fallback: v = identity, p = vp = identity.
        let (v, p) = matrix_decompose_view_proj(identity());
        approx_mat4(v, identity(), "v (NaN fallback)");
        approx_mat4(p, identity(), "p (NaN fallback, p=vp)");
    }

    #[test]
    fn matrix_decompose_view_proj_singular_v02_triggers_nan_fallback() {
        // vp[0][3] = 0 directly forces v[0][2] = -0.0, so
        // p[2][2] = vp[0][2] / -0.0. With vp[0][2] = 0.0 too, this is
        // 0.0 / -0.0 = NaN per IEEE-754 (zero divided by zero, regardless
        // of sign, is always NaN -- unlike a nonzero numerator over a
        // signed zero denominator, which is a signed infinity, not NaN;
        // see the sibling `..._inf_vp02_..._without_fallback` test for that
        // contrasting case), reliably tripping matrix_is_nan(p) regardless
        // of any other entry.
        let mut vp = zeros();
        // Give every other slot a well-defined nonzero value so only the
        // targeted division is degenerate.
        vp.rows[0] = Vec4::new(1.0, 2.0, 0.0, 0.0);
        vp.rows[1] = Vec4::new(4.0, 5.0, 6.0, 1.0);
        vp.rows[2] = Vec4::new(7.0, 8.0, 9.0, 1.0);
        vp.rows[3] = Vec4::new(10.0, 11.0, 12.0, 1.0);
        let (v, p) = matrix_decompose_view_proj(vp);
        approx_mat4(v, identity(), "v (NaN fallback)");
        approx_mat4(p, vp, "p (NaN fallback, p=vp)");
    }

    #[test]
    fn matrix_decompose_view_proj_hand_computed_nondegenerate() {
        // Hand-construct a vp for which every division is well-defined and
        // independently pre-compute the expected v/p by evaluating the
        // exact same formula the source uses, by hand (not by running this
        // port).
        //
        // Choose: vp[0][3] = -2 => v[0][2] = 2
        //         vp[1][3] = 0  => v[1][2] = 0
        //         vp[2][3] = 0  => v[2][2] = 0
        //         vp[3][3] = -1 => v[3][2] = 1
        //         vp[0][2] = 6  => p[2][2] = vp[0][2]/v[0][2] = 6/2 = 3
        //         vp[3][2] = 10 => p[3][2] = vp[3][2] - p[2][2]*v[3][2] = 10 - 3*1 = 7
        //         vp[0][0]=3, vp[1][0]=4, vp[2][0]=0 => p[0][0] = sqrt(9+16+0) = 5
        //         vp[0][1]=0, vp[1][1]=0, vp[2][1]=2 => p[1][1] = sqrt(0+0+4) = 2
        //         v[0][0] = vp[0][0]/p[0][0] = 3/5 = 0.6
        //         v[1][0] = vp[1][0]/p[0][0] = 4/5 = 0.8
        //         v[2][0] = vp[2][0]/p[0][0] = 0/5 = 0.0
        //         v[3][0] = vp[3][0]/p[0][0] (pick vp[3][0]=10 => 10/5=2.0)
        //         v[0][1] = vp[0][1]/p[1][1] = 0/2 = 0.0
        //         v[1][1] = vp[1][1]/p[1][1] = 0/2 = 0.0
        //         v[2][1] = vp[2][1]/p[1][1] = 2/2 = 1.0
        //         v[3][1] = vp[3][1]/p[1][1] (pick vp[3][1]=6 => 6/2=3.0)
        let mut vp = zeros();
        vp.rows[0] = Vec4::new(3.0, 0.0, 6.0, -2.0);
        vp.rows[1] = Vec4::new(4.0, 0.0, 0.0, 0.0);
        vp.rows[2] = Vec4::new(0.0, 2.0, 0.0, 0.0);
        vp.rows[3] = Vec4::new(10.0, 6.0, 10.0, -1.0);

        let (v, p) = matrix_decompose_view_proj(vp);

        let expected_v = Mat4::from_rows([
            Vec4::new(0.6, 0.0, 2.0, 0.0),
            Vec4::new(0.8, 0.0, 0.0, 0.0),
            Vec4::new(0.0, 1.0, 0.0, 0.0),
            Vec4::new(2.0, 3.0, 1.0, 1.0),
        ]);
        let expected_p = Mat4::from_rows([
            Vec4::new(5.0, 0.0, 0.0, 0.0),
            Vec4::new(0.0, 2.0, 0.0, 0.0),
            Vec4::new(0.0, 0.0, 3.0, -1.0),
            Vec4::new(0.0, 0.0, 7.0, 0.0),
        ]);
        approx_mat4(v, expected_v, "v (hand-computed)");
        approx_mat4(p, expected_p, "p (hand-computed)");
    }

    #[test]
    fn matrix_decompose_view_proj_all_nan_input_falls_back() {
        let vp = Mat4::from_rows([Vec4::new(f32::NAN, f32::NAN, f32::NAN, f32::NAN); 4]);
        let (v, p) = matrix_decompose_view_proj(vp);
        approx_mat4(v, identity(), "v (all-NaN fallback)");
        for i in 0..4 {
            let row = p.rows[i];
            assert!(row.x.is_nan() && row.y.is_nan() && row.z.is_nan() && row.w.is_nan());
        }
    }

    #[test]
    fn matrix_decompose_view_proj_inf_vp02_produces_inf_p22_without_fallback() {
        // v[0][2] well-defined nonzero (-vp[0][3] = 1), vp[0][2] = +inf =>
        // p[2][2] = inf/1 = +inf (finite/nonzero denom), which is NOT NaN.
        // vp[3][3] = 1.0 (NOT the default 0.0) so v[3][2] = -vp[3][3] = -1.0
        // is a proper nonzero finite value: leaving vp[3][3] at 0.0 would
        // make v[3][2] = -0.0, and p[3][2]'s `p[2][2]*v[3][2]` term would
        // then be `inf * -0.0 = NaN` per IEEE-754 (any finite-sign-times-inf
        // times *zero* is NaN, not the finite product this test wants to
        // exercise) -- an earlier draft of this test missed exactly that
        // trap and asserted the wrong (non-fallback) branch. p[0][0]/p[1][1]
        // are kept finite and nonzero (vp[0][0]=3, vp[0][1]=2, all other
        // magnitude terms 0) so v[i][0]/v[i][1] stay finite too and the NaN
        // fallback is genuinely NOT triggered -- the +inf carries through
        // unguarded, exactly as IEEE-754 division defines it.
        let mut vp = zeros();
        vp.rows[0] = Vec4::new(3.0, 2.0, f32::INFINITY, -1.0);
        vp.rows[3] = Vec4::new(0.0, 0.0, 0.0, 1.0);
        let (v, p) = matrix_decompose_view_proj(vp);
        // v[0][2] = -vp[0][3] = -(-1.0) = 1.0
        assert_eq!(v.rows[0].z, 1.0);
        // v[3][2] = -vp[3][3] = -1.0
        assert_eq!(v.rows[3].z, -1.0);
        // p[2][2] = vp[0][2] / v[0][2] = inf / 1.0 = +inf.
        assert_eq!(p.rows[2].z, f32::INFINITY);
        // p[3][2] = vp[3][2] - p[2][2]*v[3][2] = 0 - inf*(-1) = 0 - (-inf) = +inf.
        assert_eq!(p.rows[3].z, f32::INFINITY);
        // p[0][0] = sqrt(3^2) = 3.0, p[1][1] = sqrt(2^2) = 2.0 (both finite,
        // nonzero), so no fallback: neither v nor p contains a NaN.
        assert_eq!(p.rows[0].x, 3.0);
        assert_eq!(p.rows[1].y, 2.0);
        assert!(!matrix_is_nan(v));
        assert!(!matrix_is_nan(p));
    }

    // --- matrix_common: projection branch ---

    fn empty_projection() -> ProjectionStack {
        ProjectionStack::new()
    }

    fn empty_model() -> ModelStack {
        ModelStack::new()
    }

    #[test]
    fn matrix_common_projection_load_non_viewproj_sets_proj_and_identity_view() {
        // params & projMask != 0, params & loadMask != 0. isMatrixViewProj
        // is false when m[3][3] is ~0 or ~1; use m[3][3] = 1.0 (affine-style
        // matrix, not a real view-proj) so the else branch (projMatrix =
        // floatMatrix, viewMatrix = identity) is taken.
        let mut proj = empty_projection();
        let mut model = empty_model();
        let m = translation(5.0, 6.0, 7.0); // m[3][3] = 1.0 -> not view-proj
        let changed = matrix_common(
            &mut proj, &mut model, m, 0x1234, 0x0034, 0b11, 0b01, 0b10, 0b100,
        );
        assert!(changed);
        approx_mat4(proj.slots[0].proj_matrix, m, "proj_matrix == floatMatrix");
        approx_mat4(
            proj.slots[0].view_matrix,
            identity(),
            "view_matrix == identity",
        );
        approx_mat4(
            proj.slots[0].view_proj_matrix,
            m,
            "view_proj_matrix == floatMatrix (load)",
        );
        assert_eq!(proj.slots[0].segmented_address, 0x1234);
        assert_eq!(proj.slots[0].physical_address, 0x0034);
    }

    #[test]
    fn matrix_common_projection_load_viewproj_decomposes() {
        // m[3][3] = 0.5 (neither ~0 nor ~1) => isMatrixViewProj is true.
        let mut proj = empty_projection();
        let mut model = empty_model();
        let mut vp = zeros();
        vp.rows[0] = Vec4::new(3.0, 0.0, 6.0, -2.0);
        vp.rows[1] = Vec4::new(4.0, 0.0, 0.0, 0.0);
        vp.rows[2] = Vec4::new(0.0, 2.0, 0.0, 0.0);
        vp.rows[3] = Vec4::new(10.0, 6.0, 10.0, -1.0);
        let params: u8 = 0b11; // load | proj
        let changed = matrix_common(&mut proj, &mut model, vp, 0, 0, params, 0b01, 0b10, 0b100);
        assert!(changed);
        let (expected_v, expected_p) = matrix_decompose_view_proj(vp);
        approx_mat4(
            proj.slots[0].view_matrix,
            expected_v,
            "view_matrix from decompose",
        );
        approx_mat4(
            proj.slots[0].proj_matrix,
            expected_p,
            "proj_matrix from decompose",
        );
        approx_mat4(
            proj.slots[0].view_proj_matrix,
            vp,
            "view_proj_matrix == vp (load)",
        );
    }

    #[test]
    fn matrix_common_projection_mul_affine_nonidentity_updates_view() {
        // params & loadMask == 0 (mul path). floatMatrix affine
        // (m[i][3]=0,0,0 and m[3][3]=1) and not identity => viewMatrix path.
        let mut proj = empty_projection();
        proj.slots[0].view_matrix = identity();
        proj.slots[0].proj_matrix = diag(2.0, 2.0, 2.0, 1.0);
        proj.slots[0].view_proj_matrix = identity();
        let mut model = empty_model();
        let m = translation(1.0, 2.0, 3.0); // affine, not identity
        let params: u8 = 0b01; // proj only, no load bit
        let changed = matrix_common(&mut proj, &mut model, m, 0, 0, params, 0b01, 0b10, 0b100);
        assert!(changed);
        // viewMatrix = mul(m, identity) = m
        approx_mat4(proj.slots[0].view_matrix, m, "view_matrix = m*I");
        // projMatrix untouched (still diag(2,2,2,1))
        approx_mat4(
            proj.slots[0].proj_matrix,
            diag(2.0, 2.0, 2.0, 1.0),
            "proj_matrix untouched",
        );
        // viewProjMatrix = mul(m, identity) = m
        approx_mat4(proj.slots[0].view_proj_matrix, m, "view_proj_matrix = m*I");
    }

    #[test]
    fn matrix_common_projection_mul_non_affine_updates_proj() {
        // floatMatrix not affine (m[0][3] != 0) => projMatrix path.
        let mut proj = empty_projection();
        proj.slots[0].view_matrix = identity();
        proj.slots[0].proj_matrix = identity();
        proj.slots[0].view_proj_matrix = identity();
        let mut model = empty_model();
        let mut m = identity();
        m.rows[0].w = 0.3; // breaks affine-ness
        let params: u8 = 0b01;
        let changed = matrix_common(&mut proj, &mut model, m, 0, 0, params, 0b01, 0b10, 0b100);
        assert!(changed);
        approx_mat4(proj.slots[0].proj_matrix, m, "proj_matrix = m*I");
        approx_mat4(
            proj.slots[0].view_matrix,
            identity(),
            "view_matrix untouched",
        );
    }

    #[test]
    fn matrix_common_projection_mul_affine_but_identity_updates_proj_not_view() {
        // floatMatrix affine AND identity => falls to the projMatrix branch
        // (isMatrixAffine && !isMatrixIdentity is false when identity).
        let mut proj = empty_projection();
        proj.slots[0].view_matrix = diag(9.0, 9.0, 9.0, 1.0);
        proj.slots[0].proj_matrix = diag(2.0, 2.0, 2.0, 1.0);
        proj.slots[0].view_proj_matrix = identity();
        let mut model = empty_model();
        let m = identity();
        let params: u8 = 0b01;
        matrix_common(&mut proj, &mut model, m, 0, 0, params, 0b01, 0b10, 0b100);
        // view_matrix untouched (still the pre-set diag(9,9,9,1))
        approx_mat4(
            proj.slots[0].view_matrix,
            diag(9.0, 9.0, 9.0, 1.0),
            "view_matrix untouched",
        );
        // proj_matrix = mul(identity, diag(2,2,2,1)) = diag(2,2,2,1)
        approx_mat4(
            proj.slots[0].proj_matrix,
            diag(2.0, 2.0, 2.0, 1.0),
            "proj_matrix = I*proj",
        );
    }

    // --- matrix_common: model branch ---

    #[test]
    fn matrix_common_model_load_no_push_sets_top_directly() {
        let mut proj = empty_projection();
        let mut model = empty_model();
        model.matrices[0] = diag(9.0, 9.0, 9.0, 1.0);
        let m = translation(1.0, 2.0, 3.0);
        let params: u8 = 0b10; // load, no proj bit, no push bit
        let changed = matrix_common(
            &mut proj, &mut model, m, 0xAAAA, 0x00AA, params, 0b01, 0b10, 0b100,
        );
        assert!(changed);
        assert_eq!(model.size, 1);
        approx_mat4(model.matrices[0], m, "model top = floatMatrix (load)");
        assert_eq!(model.segmented_addresses[0], 0xAAAA);
        assert_eq!(model.physical_addresses[0], 0x00AA);
    }

    #[test]
    fn matrix_common_model_mul_no_push_multiplies_top() {
        let mut proj = empty_projection();
        let mut model = empty_model();
        model.matrices[0] = diag(2.0, 2.0, 2.0, 1.0);
        let m = translation(1.0, 2.0, 3.0);
        let params: u8 = 0b000; // no proj, no load, no push
        matrix_common(&mut proj, &mut model, m, 0, 0, params, 0b001, 0b010, 0b100);
        assert_eq!(model.size, 1);
        let expected = mat4_mul(m, diag(2.0, 2.0, 2.0, 1.0));
        approx_mat4(model.matrices[0], expected, "model top = m * old_top");
    }

    #[test]
    fn matrix_common_model_push_copies_stack_then_applies() {
        let mut proj = empty_projection();
        let mut model = empty_model();
        model.matrices[0] = diag(3.0, 3.0, 3.0, 1.0);
        let m = translation(1.0, 0.0, 0.0);
        let params: u8 = 0b110; // push | load, no proj
        matrix_common(&mut proj, &mut model, m, 0, 0, params, 0b001, 0b010, 0b100);
        assert_eq!(model.size, 2);
        // Pushed slot copied the OLD top (diag(3,3,3,1)) before load overwrote it.
        approx_mat4(model.matrices[1], m, "new top = loaded matrix");
        // Old slot (index 0) remains diag(3,3,3,1), untouched by load.
        approx_mat4(
            model.matrices[0],
            diag(3.0, 3.0, 3.0, 1.0),
            "old top unchanged",
        );
    }

    #[test]
    fn matrix_common_model_push_mul_multiplies_the_copy() {
        let mut proj = empty_projection();
        let mut model = empty_model();
        model.matrices[0] = diag(5.0, 5.0, 5.0, 1.0);
        let m = translation(2.0, 0.0, 0.0);
        let params: u8 = 0b100; // push only, no load, no proj
        matrix_common(&mut proj, &mut model, m, 0, 0, params, 0b001, 0b010, 0b100);
        assert_eq!(model.size, 2);
        // New top = mul(m, copy_of_old_top) = mul(m, diag(5,5,5,1))
        let expected = mat4_mul(m, diag(5.0, 5.0, 5.0, 1.0));
        approx_mat4(model.matrices[1], expected, "new top = m * copied_old_top");
        approx_mat4(
            model.matrices[0],
            diag(5.0, 5.0, 5.0, 1.0),
            "old slot unchanged",
        );
    }

    #[test]
    fn matrix_common_model_push_stops_at_stack_limit() {
        let mut proj = empty_projection();
        let mut model = empty_model();
        model.size = MODEL_MATRIX_STACK_SIZE;
        for i in 0..MODEL_MATRIX_STACK_SIZE {
            model.matrices[i] = diag(i as f32 + 1.0, 1.0, 1.0, 1.0);
        }
        let m = translation(1.0, 1.0, 1.0);
        let params: u8 = 0b110; // push | load
        matrix_common(&mut proj, &mut model, m, 0, 0, params, 0b001, 0b010, 0b100);
        // Still capped at MODEL_MATRIX_STACK_SIZE -- the push is a no-op.
        assert_eq!(model.size, MODEL_MATRIX_STACK_SIZE);
        // But the load still applies to the (unchanged) top slot.
        approx_mat4(
            model.matrices[MODEL_MATRIX_STACK_SIZE - 1],
            m,
            "top slot still receives the load at the cap",
        );
    }

    #[test]
    fn matrix_common_model_push_one_below_limit_succeeds() {
        let mut proj = empty_projection();
        let mut model = empty_model();
        model.size = MODEL_MATRIX_STACK_SIZE - 1;
        let m = identity();
        let params: u8 = 0b100; // push only
        matrix_common(&mut proj, &mut model, m, 0, 0, params, 0b001, 0b010, 0b100);
        assert_eq!(model.size, MODEL_MATRIX_STACK_SIZE);
    }

    #[test]
    fn matrix_common_always_returns_true() {
        // modelViewProjChanged = true is unconditional -- both branches, and
        // the "no push, no load" sub-case too.
        let mut proj = empty_projection();
        let mut model = empty_model();
        let m = identity();
        assert!(matrix_common(
            &mut proj, &mut model, m, 0, 0, 0b000, 0b001, 0b010, 0b100
        ));
        assert!(matrix_common(
            &mut proj, &mut model, m, 0, 0, 0b001, 0b001, 0b010, 0b100
        ));
        assert!(matrix_common(
            &mut proj, &mut model, m, 0, 0, 0b011, 0b001, 0b010, 0b100
        ));
        assert!(matrix_common(
            &mut proj, &mut model, m, 0, 0, 0b100, 0b001, 0b010, 0b100
        ));
    }

    #[test]
    fn matrix_common_params_widened_before_and_matches_low_byte_of_wide_masks() {
        // proj_mask has bits set above bit 7; only the low byte can ever
        // match an 8-bit params value. Confirms the u32::from(params) & mask
        // widening does not truncate the mask (it truncates nothing -- the
        // mask stays full-width, params is widened up to meet it).
        let mut proj = empty_projection();
        let mut model = empty_model();
        let m = identity();
        // proj_mask = 0x100 (bit 8, unreachable by an 8-bit params) plus
        // bit 0 (reachable). params = 0b1 should still match via bit 0.
        let proj_mask = 0x101u32;
        let params: u8 = 0b1;
        matrix_common(
            &mut proj, &mut model, m, 0, 0, params, proj_mask, 0b10, 0b100,
        );
        // Took the projection branch (proj slot's view_proj_matrix updated),
        // not the model branch (model.size stays 1, model.matrices[0] stays
        // default zero, not `m`).
        approx_mat4(model.matrices[0], zeros(), "model branch NOT taken");
    }

    // --- push_projection_matrix / pop_projection_matrix ---

    #[test]
    fn push_projection_matrix_copies_top_into_new_slot() {
        let mut stack = empty_projection();
        stack.slots[0].view_matrix = diag(1.0, 2.0, 3.0, 1.0);
        stack.slots[0].proj_matrix = diag(4.0, 5.0, 6.0, 1.0);
        stack.slots[0].view_proj_matrix = diag(7.0, 8.0, 9.0, 1.0);
        stack.slots[0].segmented_address = 0x10;
        stack.slots[0].physical_address = 0x20;
        push_projection_matrix(&mut stack);
        assert_eq!(stack.size, 2);
        assert_eq!(stack.slots[1], stack.slots[0]);
    }

    #[test]
    fn push_projection_matrix_stops_at_limit() {
        let mut stack = empty_projection();
        stack.size = PROJECTION_MATRIX_STACK_SIZE;
        push_projection_matrix(&mut stack);
        assert_eq!(stack.size, PROJECTION_MATRIX_STACK_SIZE);
    }

    #[test]
    fn push_projection_matrix_one_below_limit_succeeds() {
        let mut stack = empty_projection();
        stack.size = PROJECTION_MATRIX_STACK_SIZE - 1;
        push_projection_matrix(&mut stack);
        assert_eq!(stack.size, PROJECTION_MATRIX_STACK_SIZE);
    }

    #[test]
    fn push_projection_matrix_sixteen_consecutive_pushes_cap_at_sixteen() {
        let mut stack = empty_projection();
        for _ in 0..32 {
            push_projection_matrix(&mut stack);
        }
        assert_eq!(stack.size, PROJECTION_MATRIX_STACK_SIZE);
    }

    #[test]
    fn pop_projection_matrix_decrements_and_reports_flags() {
        let mut stack = empty_projection();
        push_projection_matrix(&mut stack);
        assert_eq!(stack.size, 2);
        let result = pop_projection_matrix(&mut stack);
        assert_eq!(result, Some((true, true, false)));
        assert_eq!(stack.size, 1);
    }

    #[test]
    fn pop_projection_matrix_at_size_one_is_noop_and_reports_none() {
        let mut stack = empty_projection();
        assert_eq!(stack.size, 1);
        let result = pop_projection_matrix(&mut stack);
        assert_eq!(result, None);
        assert_eq!(stack.size, 1);
    }

    #[test]
    fn pop_projection_matrix_repeated_past_one_stays_at_one() {
        let mut stack = empty_projection();
        push_projection_matrix(&mut stack);
        push_projection_matrix(&mut stack);
        assert_eq!(stack.size, 3);
        for _ in 0..5 {
            pop_projection_matrix(&mut stack);
        }
        assert_eq!(stack.size, 1);
    }

    // --- compute_model_view_proj ---

    #[test]
    fn compute_model_view_proj_identity_both_is_identity() {
        let proj = empty_projection();
        let mut model = empty_model();
        model.matrices[0] = identity();
        let mut proj = proj;
        proj.slots[0].view_proj_matrix = identity();
        let result = compute_model_view_proj(&proj, &model);
        approx_mat4(result, identity(), "identity * identity");
    }

    #[test]
    fn compute_model_view_proj_model_is_left_operand() {
        let mut proj = empty_projection();
        proj.slots[0].view_proj_matrix = diag(2.0, 2.0, 2.0, 1.0);
        let mut model = empty_model();
        model.matrices[0] = translation(1.0, 0.0, 0.0);
        let result = compute_model_view_proj(&proj, &model);
        // mul(translation, scale) with model on the left: row 3 (translation
        // row) of the model matrix is (1,0,0,1); dotted against the scale
        // matrix's columns gives (2,0,0,1) since scale's diagonal is 2.
        let expected = mat4_mul(translation(1.0, 0.0, 0.0), diag(2.0, 2.0, 2.0, 1.0));
        approx_mat4(result, expected, "model-left composition");
        assert_eq!(result.rows[3], Vec4::new(2.0, 0.0, 0.0, 1.0));
    }

    #[test]
    fn compute_model_view_proj_reads_the_top_of_both_stacks() {
        let mut proj = empty_projection();
        push_projection_matrix(&mut proj);
        proj.slots[1].view_proj_matrix = diag(3.0, 3.0, 3.0, 1.0);
        proj.slots[0].view_proj_matrix = diag(99.0, 99.0, 99.0, 1.0);
        let mut model = empty_model();
        model.matrices[0] = identity();
        let result = compute_model_view_proj(&proj, &model);
        approx_mat4(result, diag(3.0, 3.0, 3.0, 1.0), "reads slot 1, not slot 0");
    }

    // --- ProjectionStack / ModelStack defaults ---

    #[test]
    fn projection_stack_new_matches_reset_defaults() {
        let stack = empty_projection();
        assert_eq!(stack.size, 1);
        assert_eq!(stack.slots[0].view_matrix, zeros());
        assert_eq!(stack.slots[0].proj_matrix, zeros());
        assert_eq!(stack.slots[0].view_proj_matrix, zeros());
    }

    #[test]
    fn model_stack_new_matches_reset_defaults() {
        let stack = empty_model();
        assert_eq!(stack.size, 1);
        assert_eq!(stack.matrices[0], zeros());
        assert_eq!(stack.segmented_addresses[0], 0);
    }

    // --- additional coverage: NaN/inf inputs, translation/rotation/scale,
    // and boundary depths, per the M5.2 ticket's 40-60 test target ---

    #[test]
    fn matrix_common_pure_translation_load_model() {
        let mut proj = empty_projection();
        let mut model = empty_model();
        let m = translation(7.0, -3.0, 2.5);
        let params: u8 = 0b10; // load, no proj, no push
        matrix_common(&mut proj, &mut model, m, 0, 0, params, 0b001, 0b010, 0b100);
        approx_mat4(model.matrices[0], m, "pure translation load");
    }

    #[test]
    fn matrix_common_pure_scale_load_model() {
        let mut proj = empty_projection();
        let mut model = empty_model();
        let m = diag(2.0, 3.0, 4.0, 1.0);
        let params: u8 = 0b10;
        matrix_common(&mut proj, &mut model, m, 0, 0, params, 0b001, 0b010, 0b100);
        approx_mat4(model.matrices[0], m, "pure scale load");
    }

    fn rotation_z(rad: f32) -> Mat4 {
        // Standard 3D rotation about Z, embedded in a float4x4 with an
        // identity translation/perspective row and column, matching the
        // same row-major HLSL convention used throughout this module.
        let c = rad.cos();
        let s = rad.sin();
        Mat4::from_rows([
            Vec4::new(c, -s, 0.0, 0.0),
            Vec4::new(s, c, 0.0, 0.0),
            Vec4::new(0.0, 0.0, 1.0, 0.0),
            Vec4::new(0.0, 0.0, 0.0, 1.0),
        ])
    }

    #[test]
    fn matrix_common_pure_rotation_load_model() {
        let mut proj = empty_projection();
        let mut model = empty_model();
        let m = rotation_z(std::f32::consts::FRAC_PI_2);
        let params: u8 = 0b10;
        matrix_common(&mut proj, &mut model, m, 0, 0, params, 0b001, 0b010, 0b100);
        approx_mat4(model.matrices[0], m, "pure rotation load");
    }

    #[test]
    fn matrix_common_rotation_mul_composes_with_existing_top() {
        let mut proj = empty_projection();
        let mut model = empty_model();
        model.matrices[0] = translation(1.0, 0.0, 0.0);
        let r = rotation_z(std::f32::consts::FRAC_PI_2);
        let params: u8 = 0b000; // mul path
        matrix_common(&mut proj, &mut model, r, 0, 0, params, 0b001, 0b010, 0b100);
        let expected = mat4_mul(r, translation(1.0, 0.0, 0.0));
        approx_mat4(
            model.matrices[0],
            expected,
            "rotation composed with translation (r on left)",
        );
    }

    #[test]
    fn matrix_common_model_input_nan_propagates_on_load() {
        let mut proj = empty_projection();
        let mut model = empty_model();
        let mut m = identity();
        m.rows[1].y = f32::NAN;
        let params: u8 = 0b10;
        matrix_common(&mut proj, &mut model, m, 0, 0, params, 0b001, 0b010, 0b100);
        assert!(model.matrices[0].rows[1].y.is_nan());
    }

    #[test]
    fn matrix_common_model_input_nan_propagates_on_mul() {
        let mut proj = empty_projection();
        let mut model = empty_model();
        model.matrices[0] = identity();
        let mut m = identity();
        m.rows[0].x = f32::NAN;
        let params: u8 = 0b000;
        matrix_common(&mut proj, &mut model, m, 0, 0, params, 0b001, 0b010, 0b100);
        assert!(model.matrices[0].rows[0].x.is_nan());
    }

    #[test]
    fn matrix_common_projection_load_inf_input_is_not_view_proj_when_m33_is_one() {
        // isMatrixViewProj is false when m[3][3] is ~1, regardless of other
        // entries being infinite -- takes the non-decompose else branch.
        let mut proj = empty_projection();
        let mut model = empty_model();
        let mut m = identity();
        m.rows[0].x = f32::INFINITY;
        let params: u8 = 0b11; // proj | load
        matrix_common(&mut proj, &mut model, m, 0, 0, params, 0b01, 0b10, 0b100);
        approx_mat4(
            proj.slots[0].proj_matrix,
            m,
            "proj_matrix = floatMatrix (not view-proj)",
        );
        approx_mat4(
            proj.slots[0].view_matrix,
            identity(),
            "view_matrix = identity",
        );
    }

    #[test]
    fn matrix_common_model_stack_exactly_at_thirty_one_can_still_push_to_thirty_two() {
        let mut proj = empty_projection();
        let mut model = empty_model();
        model.size = 31;
        let m = identity();
        let params: u8 = 0b100; // push only
        matrix_common(&mut proj, &mut model, m, 0, 0, params, 0b001, 0b010, 0b100);
        assert_eq!(model.size, 32);
    }

    #[test]
    fn matrix_common_zero_params_takes_model_mul_branch_with_no_push() {
        // params == 0: proj bit clear (model branch), load bit clear (mul),
        // push bit clear (no push). Exercises the "all bits off" corner.
        let mut proj = empty_projection();
        let mut model = empty_model();
        model.matrices[0] = diag(3.0, 3.0, 3.0, 1.0);
        let m = translation(1.0, 1.0, 1.0);
        matrix_common(&mut proj, &mut model, m, 0, 0, 0, 0b001, 0b010, 0b100);
        assert_eq!(model.size, 1);
        let expected = mat4_mul(m, diag(3.0, 3.0, 3.0, 1.0));
        approx_mat4(
            model.matrices[0],
            expected,
            "all-zero params still multiplies",
        );
    }

    #[test]
    fn matrix_common_projection_address_stored_verbatim_not_translated() {
        // The raw segmented `address` is stored as-is in
        // `segmented_address`; only `physical_address` carries the
        // caller-supplied translated value. They must not be conflated.
        let mut proj = empty_projection();
        let mut model = empty_model();
        let m = identity();
        let params: u8 = 0b11;
        matrix_common(
            &mut proj,
            &mut model,
            m,
            0xDEAD_0000,
            0x0000_00A0,
            params,
            0b01,
            0b10,
            0b100,
        );
        assert_eq!(proj.slots[0].segmented_address, 0xDEAD_0000);
        assert_eq!(proj.slots[0].physical_address, 0x0000_00A0);
        assert_ne!(
            proj.slots[0].segmented_address,
            proj.slots[0].physical_address
        );
    }

    #[test]
    fn matrix_common_model_address_stored_verbatim_not_translated() {
        let mut proj = empty_projection();
        let mut model = empty_model();
        let m = identity();
        let params: u8 = 0b10;
        matrix_common(
            &mut proj,
            &mut model,
            m,
            0xBEEF_0001,
            0x0000_00F8,
            params,
            0b001,
            0b010,
            0b100,
        );
        assert_eq!(model.segmented_addresses[0], 0xBEEF_0001);
        assert_eq!(model.physical_addresses[0], 0x0000_00F8);
    }

    #[test]
    fn is_matrix_view_proj_boundary_reused_from_rt64_math_matches_matrix_common_gate() {
        // Sanity-check that this module's projection/load branch truly
        // delegates the isMatrixViewProj test to the reused `rt64_math`
        // predicate rather than a locally reimplemented copy: m[3][3] = 0.5
        // must decompose, m[3][3] = 1.0 must not.
        assert!(is_matrix_view_proj(diag(1.0, 1.0, 1.0, 0.5)));
        assert!(!is_matrix_view_proj(diag(1.0, 1.0, 1.0, 1.0)));
        assert!(!is_matrix_view_proj(diag(1.0, 1.0, 1.0, 0.0)));
    }

    #[test]
    fn push_then_pop_projection_matrix_round_trips_slot_contents() {
        let mut stack = empty_projection();
        stack.slots[0].view_matrix = diag(5.0, 5.0, 5.0, 1.0);
        push_projection_matrix(&mut stack);
        stack.slots[1].view_matrix = diag(9.0, 9.0, 9.0, 1.0);
        assert_eq!(stack.size, 2);
        pop_projection_matrix(&mut stack);
        assert_eq!(stack.size, 1);
        // Popping only decrements size; slot 0's contents are untouched by
        // the pop itself (the source never clears popped-past slots).
        approx_mat4(
            stack.slots[0].view_matrix,
            diag(5.0, 5.0, 5.0, 1.0),
            "slot 0 preserved across push/pop",
        );
    }

    #[test]
    fn compute_model_view_proj_with_nan_model_propagates_nan() {
        let mut proj = empty_projection();
        proj.slots[0].view_proj_matrix = identity();
        let mut model = empty_model();
        model.matrices[0] = identity();
        model.matrices[0].rows[2].z = f32::NAN;
        let result = compute_model_view_proj(&proj, &model);
        assert!(result.rows[2].z.is_nan());
    }

    #[test]
    fn matrix_decompose_view_proj_negative_zero_vp33_is_not_view_proj_relevant_but_decompose_runs_anyway(
    ) {
        // matrix_decompose_view_proj itself has no isMatrixViewProj gate --
        // that gating lives in matrix_common, not this pure function. Direct
        // calls always run the full formula regardless of m[3][3].
        let mut vp = zeros();
        vp.rows[0] = Vec4::new(2.0, 0.0, 4.0, -1.0);
        vp.rows[3] = Vec4::new(0.0, 0.0, 5.0, -0.0);
        let (v, _p) = matrix_decompose_view_proj(vp);
        // v[3][2] = -vp[3][3] = -(-0.0) = 0.0 (not -0.0), a well-defined
        // finite value regardless of the input's sign of zero.
        assert_eq!(v.rows[3].z, 0.0);
    }
}
