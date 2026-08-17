//! Literal port of `GameFrame::areFramebufferPairsCompatible` and
//! `GameFrame::isSceneCompatible`'s decision logic: a literal port of the
//! permitted MIT RT64 Rust-port source pinned at commit
//! `5473732a822a4423b5696e7cb18fecc425a59875`
//! (`docs/RT64-PORT-AUTHORITY.md`), `src/hle/rt64_game_frame.h`/`.cpp`
//! (SHA-256 of the whole files,
//! `7e483175a33989d1fc290c9ffc5fa2f04fa2c99530a0e48c4e3a7d0d44e8c3f7` /
//! `a9527bfcdb06b876a45f06a181ca76b6cf7869d01e2c06a0c81ed9f96f1dd87a`):
//!
//! ```text
//! // rt64_game_frame.h
//! struct GameIndices {
//!     struct FramebufferPair {
//!         uint32_t workloadIndex;
//!         uint32_t fbPairIndex;
//!     };
//! };
//!
//! inline bool operator==(const GameIndices::FramebufferPair &lhs, const GameIndices::FramebufferPair &rhs) {
//!     return (lhs.workloadIndex == rhs.workloadIndex) && (lhs.fbPairIndex == rhs.fbPairIndex);
//! }
//!
//! // rt64_framebuffer_pair.h (the fields areFramebufferPairsCompatible reads)
//! struct FramebufferPair {
//!     struct {
//!         uint32_t address = 0;
//!         uint8_t fmt = 0;
//!         uint8_t siz = 0;
//!         uint16_t width = 0;
//!         bool formatChanged = false;
//!     } colorImage;
//!
//!     struct {
//!         uint32_t address = 0;
//!         bool formatChanged = false;
//!     } depthImage;
//!
//!     bool depthRead;
//!     bool depthWrite;
//!     // ...
//! };
//!
//! // rt64_game_frame.cpp lines 15-73
//! bool GameFrame::areFramebufferPairsCompatible(const WorkloadQueue &workloadQueue, const GameIndices::FramebufferPair &first, const GameIndices::FramebufferPair &second) {
//!     if (first == second) {
//!         return true;
//!     }
//!
//!     const Workload &firstWorkload = workloadQueue.workloads[first.workloadIndex];
//!     const Workload &secondWorkload = workloadQueue.workloads[second.workloadIndex];
//!     const auto &firstFbPair = firstWorkload.fbPairs[first.fbPairIndex];
//!     const auto &secondFbPair = secondWorkload.fbPairs[second.fbPairIndex];
//!     if ((firstFbPair.depthRead || firstFbPair.depthWrite) && (secondFbPair.depthRead || secondFbPair.depthWrite)) {
//!         if (firstFbPair.depthImage.address != secondFbPair.depthImage.address) {
//!             return false;
//!         }
//!     }
//!
//!     const auto &firstColorImage = firstFbPair.colorImage;
//!     const auto &secondColorImage = secondFbPair.colorImage;
//!     if ((firstColorImage.address != secondColorImage.address) ||
//!         (firstColorImage.fmt != secondColorImage.fmt) ||
//!         (firstColorImage.siz != secondColorImage.siz) ||
//!         (firstColorImage.width != secondColorImage.width))
//!     {
//!         return false;
//!     }
//!
//!     return true;
//! }
//!
//! bool GameFrame::isSceneCompatible(const WorkloadQueue &workloadQueue, const GameScene &scene, const GameIndices::Projection &proj) {
//!     assert(!scene.projections.empty());
//!
//!     const float MatrixDiffTolerance = 1e-6f;
//!     const Workload &workload = workloadQueue.workloads[proj.workloadIndex];
//!     const FramebufferPair &fbPair = workload.fbPairs[proj.fbPairIndex];
//!     const Projection &fbProj = fbPair.projections[proj.projectionIndex];
//!     const GameIndices::Projection &firstProj = scene.projections.front();
//!     if (!areFramebufferPairsCompatible(workloadQueue, { firstProj.workloadIndex, firstProj.fbPairIndex }, { proj.workloadIndex, proj.fbPairIndex })) {
//!         return false;
//!     }
//!
//!     const Workload &cmpWorkload = workloadQueue.workloads[firstProj.workloadIndex];
//!     const FramebufferPair &cmpFbPair = cmpWorkload.fbPairs[firstProj.fbPairIndex];
//!     const Projection &cmpProj = cmpFbPair.projections[firstProj.projectionIndex];
//!     const interop::float4x4 &cmpViewMatrix = cmpWorkload.drawData.viewTransforms[cmpProj.transformsIndex];
//!     const interop::float4x4 &fbViewMatrix = workload.drawData.viewTransforms[fbProj.transformsIndex];
//!     const float viewMatrixDiff = matrixDifference(cmpViewMatrix, fbViewMatrix);
//!     if (viewMatrixDiff > MatrixDiffTolerance) {
//!         return false;
//!     }
//!
//!     const interop::float4x4 &cmpProjMatrix = cmpWorkload.drawData.projTransforms[cmpProj.transformsIndex];
//!     const interop::float4x4 &fbProjMatrix = workload.drawData.projTransforms[fbProj.transformsIndex];
//!     const float projMatrixDiff = matrixDifference(cmpProjMatrix, fbProjMatrix);
//!     if (projMatrixDiff > MatrixDiffTolerance) {
//!         return false;
//!     }
//!
//!     return true;
//! }
//! ```
//!
//! **Reuse, not new type.** `state.rs::ColorImage` looks superficially
//! similar (it has `format`/`size`/`width`/`address` fields) but it is the
//! RDP-command-decoded color image target (`pub(crate)`-constructed via
//! `from_wire`, paired with `FillColor`/`PrimColor`/`RdpState` in the RDP
//! command-stream domain) -- not RT64's `FramebufferPair::colorImage`/
//! `depthImage` (a plain `{address, fmt, siz, width}` POD inside the
//! `GameFrame`/`WorkloadQueue` HLE object graph this card explicitly must
//! NOT port wholesale). Reusing `state::ColorImage` here would silently
//! couple this predicate to RDP command-decode semantics (its `address`
//! field is `fn64_render_ir::PhysicalAddress`, a fallibly-constructed,
//! 24-bit-bounded type with validation the C++ `uint32_t address` field
//! does not have) that the source does not have, so this module defines its
//! own minimal `ColorImageFields`/`DepthImageFields`/`FbPairFields` structs
//! matching the C++ field types and names exactly (`address: u32`,
//! `fmt: u8`, `siz: u8`, `width: u16`), per `rt64_framebuffer_pair.h`. No
//! existing `fn64-render-wgpu`/`fn64-render-ir` type already represents
//! RT64's `GameIndices::FramebufferPair` (a workload/fbPair *index pair*,
//! not an image descriptor) either, so it is ported as its own small struct
//! too, with the C++ `operator==` as `PartialEq`/`Eq` (structural equality
//! over both `u32` fields matches the C++ operator's `&&` of two `==`s
//! exactly).
//!
//! ## Admitted domain
//!
//! - **`areFramebufferPairsCompatible`'s `first == second` short-circuit
//!   only skips the depth/color-image checks; it does not change their
//!   outcome.** If two `GameIndices::FramebufferPair` values are equal (same
//!   `workloadIndex` and `fbPairIndex`), they necessarily reference the
//!   same `FramebufferPair` object, whose color/depth image fields would
//!   trivially match themselves -- the early `return true` is purely an
//!   optimization to skip the `WorkloadQueue`/`Workload` indexing that this
//!   port does not model, not a behavior divergence from "fall through and
//!   compare fields". This port takes `first == second` as `PartialEq` on
//!   the two-`u32` index-pair struct (identical semantics to the C++
//!   `operator==`), preserved as the same first branch, in the same order.
//! - **`(firstFbPair.depthRead || firstFbPair.depthWrite) &&
//!   (secondFbPair.depthRead || secondFbPair.depthWrite)` gates whether the
//!   depth-image-address check runs at all.** All four `bool`s are
//!   caller-supplied scalars with no side effects, so `&&`/`||`
//!   short-circuit order only affects which redundant boolean reads are
//!   skipped, never the observable result -- ported as plain Rust
//!   `||`/`&&`, same grouping, same order.
//! - **The color-image check is an unconditional 4-way `||` of `!=`
//!   comparisons** (`address`, `fmt`, `siz`, `width`) -- any single
//!   mismatch fails the whole predicate, with no field given priority over
//!   another for the *return value* (only for which comparison the CPU
//!   evaluates first, if it doesn't short-circuit the whole expression).
//!   Ported as a plain Rust `||` chain in the same field order.
//! - **`fmt`/`siz` are `u8`, `width` is `u16`, `address` is `u32`, all
//!   compared by `!=` (exact equality, no tolerance)** -- unlike the matrix
//!   comparisons below, these are exact-value fields (RDP register/format
//!   encodings and raw physical addresses), so bitwise/integer `==`/`!=` is
//!   the literal, only-possible translation; there is no signed/unsigned
//!   pitfall since all four fields are unsigned in both languages.
//! - **`isSceneCompatible`'s decision logic is ported taking the two
//!   `matrixDifference(...)` results as caller-supplied `f32` inputs
//!   (`view_matrix_diff`, `proj_matrix_diff`) rather than computing them
//!   from raw matrices.** `matrixDifference` itself is explicitly deferred
//!   by `rt64_math.rs`'s own Nonclaims ("Does not port ... `matrixDifference`
//!   ... (deferred -- needs new matrix-inverse/quaternion infra)"), and
//!   `docs/RT64-PORT-DASHBOARD.md`'s `M8.12` card records this exact
//!   dependency on the (not-yet-landed-in-this-worktree) `M8.3` card: "Port
//!   the two predicates over local descriptor structs once M8.3 supplies
//!   matrixDifference." Rather than block this entire card on an unlanded
//!   prerequisite, or silently reimplement `matrixDifference` here (a
//!   second, unauthorized definition of a symbol another card owns), this
//!   port takes the tolerance-comparison decision logic literally --
//!   `viewMatrixDiff > MatrixDiffTolerance` / `projMatrixDiff >
//!   MatrixDiffTolerance`, both strict greater-than against the literal
//!   `1e-6f` constant, in the same order, with the same early-return
//!   short-circuit on the view check alone failing -- and defers only the
//!   matrix-difference *computation* to the caller, matching this ticket's
//!   own instruction to extract "decision logic over a minimal set of
//!   caller-supplied scalar/struct inputs". See "Nonclaims" for what this
//!   means is NOT claimed.
//! - **`isSceneCompatible`'s `areFramebufferPairsCompatible` sub-call is
//!   reused, not reimplemented** -- it calls this module's own
//!   `are_framebuffer_pairs_compatible` with the `FbPairFields` for
//!   `scene.projections.front()`'s framebuffer pair and `proj`'s framebuffer
//!   pair as caller-supplied inputs (in place of the C++'s
//!   `workloadQueue`/index-pair indexing), preserving the exact "fail fast
//!   if framebuffer pairs are incompatible, before ever computing a matrix
//!   difference" short-circuit order from the source (the view/proj matrix
//!   diffs are cheaper to *pass in* here since this port doesn't compute
//!   them, but the *order of checks* -- fb-pair compatibility, then view
//!   diff, then proj diff -- is preserved exactly).
//! - **`scene.projections.front()` / `assert(!scene.projections.empty())`**:
//!   the C++ takes the *first* element of a caller-owned vector and asserts
//!   it is non-empty (debug-only precondition, `NDEBUG`-compiled-out in
//!   release, matching `rt64_common.rs`'s established "debug-only `assert()`
//!   becomes `debug_assert!`" precedent). Since this port does not carry
//!   the `GameScene`/`std::vector<GameIndices::Projection>` graph at all
//!   (out of scope -- see Nonclaims), there is no `Vec` to assert
//!   non-emptiness on here; the *comparison* logic downstream of that
//!   lookup (fb-pair compatibility, then the two tolerance checks) is what
//!   this module ports, with the "first" projection's already-resolved
//!   `FbPairFields` and view/proj-diff floats supplied directly by the
//!   caller. This is a genuine scope boundary, not a silently-dropped
//!   assert: the emptiness precondition belongs to the un-ported
//!   `GameScene`-traversal caller, not to this predicate's own decision
//!   logic.
//! - **Both tolerance checks use strict `>` (not `>=`) against `1e-6f`.**
//!   A `view_matrix_diff` or `proj_matrix_diff` of exactly `1e-6` is
//!   compatible (`false` for "reject"); anything strictly greater fails.
//!   Ported as plain Rust `>`, matching `f32` exactly (no epsilon-widening
//!   or rounding introduced).
//!
//! ## Nonclaims
//!
//! No GPU, WGSL, or production wiring (this module is not called from
//! anywhere yet -- dead-code warnings on the unused public surface are
//! expected and correct, matching `rt64_common.rs`'s and `rt64_math.rs`'s
//! precedent), and no RT64 visual/pixel/silicon parity or performance
//! claim. The surrounding `GameFrame`/`GameFrameMap`/`WorkloadQueue`/
//! `Workload`/`GameScene`/`Projection`/`GameCall` object graph
//! (`rt64_game_frame.h`'s `GameFrame::set`/`match`/`matchScene`/
//! `matchTransform`/`buildCallHashMap`/`buildTransformIdMap`/`hashFromCall`/
//! `isDebuggerCameraEnabled`, and every non-predicate type in the header:
//! `GameFrameMap`, `GameCallMap`, `ModifiedBuffers`, `GameScene`) is
//! deliberately NOT ported here -- it requires `WorkloadQueue`, `Workload`,
//! `RenderWorker`, and `BufferUploader`, none of which this crate has an
//! equivalent for, and porting it would mean vendoring roughly 970 more
//! lines of `rt64_game_frame.cpp` far beyond this card's two named
//! predicates (`docs/RT64-PORT-DASHBOARD.md`'s own `M8.12` finding: "port
//! ONLY the two predicates at lines 15-73"). `matrixDifference` itself is
//! also not ported here (owned by the separate, not-yet-landed `M8.3`
//! card) -- this module's `isSceneCompatible` port takes its two results as
//! `f32` inputs instead (see "Admitted domain" above). This module updates
//! no other file's doc comments (in particular, `rt64_math.rs`'s Nonclaims
//! sentence "`rt64_game_frame.cpp` itself remains unported" is left as-is;
//! narrowing it is out of this card's exclusive-paths scope, which permits
//! editing only this new file and one `mod` line in `lib.rs`).

/// `GameIndices::FramebufferPair`: a `(workloadIndex, fbPairIndex)` index
/// pair identifying one framebuffer pair inside a `WorkloadQueue`. Ported
/// as a plain index-pair struct (see module doc "Reuse, not new type") --
/// this module's predicates use it only for the `operator==` comparison
/// `areFramebufferPairsCompatible`'s first line performs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FramebufferPairIndex {
    pub workload_index: u32,
    pub fb_pair_index: u32,
}

/// `FramebufferPair::colorImage`'s four compared fields
/// (`rt64_framebuffer_pair.h`): `address: u32`, `fmt: u8`, `siz: u8`,
/// `width: u16`. `formatChanged` is not read by either predicate and is
/// not carried here.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ColorImageFields {
    pub address: u32,
    pub fmt: u8,
    pub siz: u8,
    pub width: u16,
}

/// `FramebufferPair::depthImage`'s one compared field
/// (`rt64_framebuffer_pair.h`): `address: u32`. `formatChanged` is not read
/// by either predicate and is not carried here.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DepthImageFields {
    pub address: u32,
}

/// The minimal slice of `FramebufferPair` (`rt64_framebuffer_pair.h`) that
/// `areFramebufferPairsCompatible` reads: `colorImage`, `depthImage`,
/// `depthRead`, `depthWrite`. The other ~15 fields of the real
/// `FramebufferPair` (scissor/draw rects, discard lists, dither patterns,
/// flush reason, projections, ...) are not read by either predicate and are
/// deliberately not carried here (see module doc "Reuse, not new type").
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FbPairFields {
    pub color_image: ColorImageFields,
    pub depth_image: DepthImageFields,
    pub depth_read: bool,
    pub depth_write: bool,
}

/// `GameFrame::areFramebufferPairsCompatible`'s decision logic, over
/// caller-supplied `FbPairFields` in place of `WorkloadQueue`/`Workload`
/// indexing. The `WorkloadQueue` parameter and the `GameIndices::
/// FramebufferPair` index-pair *lookup* are collapsed into this direct
/// `FbPairFields` input; the `first == second` index-pair identity
/// short-circuit is preserved as a separate `first_index`/`second_index`
/// pair (see module doc "Admitted domain" for why this changes nothing
/// observable).
pub fn are_framebuffer_pairs_compatible(
    first_index: FramebufferPairIndex,
    second_index: FramebufferPairIndex,
    first: FbPairFields,
    second: FbPairFields,
) -> bool {
    if first_index == second_index {
        return true;
    }

    if (first.depth_read || first.depth_write)
        && (second.depth_read || second.depth_write)
        && first.depth_image.address != second.depth_image.address
    {
        return false;
    }

    let first_color = first.color_image;
    let second_color = second.color_image;
    if (first_color.address != second_color.address)
        || (first_color.fmt != second_color.fmt)
        || (first_color.siz != second_color.siz)
        || (first_color.width != second_color.width)
    {
        return false;
    }

    true
}

/// `GameFrame::isSceneCompatible`'s decision logic, over caller-supplied
/// `FbPairFields` for the scene's first projection and the candidate
/// projection, plus the already-computed `matrixDifference` results for
/// the view and projection matrices (see module doc "Admitted domain" for
/// why the matrix-difference computation itself is a caller input rather
/// than being computed here). `MatrixDiffTolerance = 1e-6f` is the literal
/// source constant.
pub fn is_scene_compatible(
    first_proj_fb_pair_index: FramebufferPairIndex,
    proj_fb_pair_index: FramebufferPairIndex,
    first_proj_fb_pair: FbPairFields,
    proj_fb_pair: FbPairFields,
    view_matrix_diff: f32,
    proj_matrix_diff: f32,
) -> bool {
    const MATRIX_DIFF_TOLERANCE: f32 = 1e-6;

    if !are_framebuffer_pairs_compatible(
        first_proj_fb_pair_index,
        proj_fb_pair_index,
        first_proj_fb_pair,
        proj_fb_pair,
    ) {
        return false;
    }

    if view_matrix_diff > MATRIX_DIFF_TOLERANCE {
        return false;
    }

    if proj_matrix_diff > MATRIX_DIFF_TOLERANCE {
        return false;
    }

    true
}

#[cfg(test)]
mod tests {
    use super::*;

    fn color(address: u32, fmt: u8, siz: u8, width: u16) -> ColorImageFields {
        ColorImageFields {
            address,
            fmt,
            siz,
            width,
        }
    }

    fn depth(address: u32) -> DepthImageFields {
        DepthImageFields { address }
    }

    fn fb_pair(
        color_image: ColorImageFields,
        depth_image: DepthImageFields,
        depth_read: bool,
        depth_write: bool,
    ) -> FbPairFields {
        FbPairFields {
            color_image,
            depth_image,
            depth_read,
            depth_write,
        }
    }

    fn default_fb_pair() -> FbPairFields {
        fb_pair(color(0x1000, 5, 2, 320), depth(0x2000), false, false)
    }

    fn index(workload_index: u32, fb_pair_index: u32) -> FramebufferPairIndex {
        FramebufferPairIndex {
            workload_index,
            fb_pair_index,
        }
    }

    // --- FramebufferPairIndex equality (operator==) ---

    #[test]
    fn framebuffer_pair_index_equal_when_both_fields_match() {
        assert_eq!(index(1, 2), index(1, 2));
    }

    #[test]
    fn framebuffer_pair_index_unequal_workload_index_differs() {
        assert_ne!(index(1, 2), index(9, 2));
    }

    #[test]
    fn framebuffer_pair_index_unequal_fb_pair_index_differs() {
        assert_ne!(index(1, 2), index(1, 9));
    }

    #[test]
    fn framebuffer_pair_index_unequal_both_fields_differ() {
        assert_ne!(index(1, 2), index(3, 4));
    }

    // --- are_framebuffer_pairs_compatible: identity short-circuit ---

    #[test]
    fn identical_index_returns_true_even_with_mismatched_fields() {
        // first == second short-circuits before any field comparison, so
        // even wildly different FbPairFields must still return true.
        let idx = index(1, 2);
        let a = fb_pair(color(1, 1, 1, 1), depth(1), true, true);
        let b = fb_pair(color(999, 9, 9, 999), depth(999), false, false);
        assert!(are_framebuffer_pairs_compatible(idx, idx, a, b));
    }

    #[test]
    fn identical_index_zero_zero_returns_true() {
        let idx = index(0, 0);
        assert!(are_framebuffer_pairs_compatible(
            idx,
            idx,
            default_fb_pair(),
            default_fb_pair()
        ));
    }

    // --- are_framebuffer_pairs_compatible: depth gate ---

    #[test]
    fn different_index_both_depth_inactive_ignores_depth_address_mismatch() {
        let a = fb_pair(color(1, 1, 1, 1), depth(0xAAAA), false, false);
        let b = fb_pair(color(1, 1, 1, 1), depth(0xBBBB), false, false);
        assert!(are_framebuffer_pairs_compatible(
            index(1, 1),
            index(2, 2),
            a,
            b
        ));
    }

    #[test]
    fn depth_read_on_both_sides_and_mismatched_address_fails() {
        let a = fb_pair(color(1, 1, 1, 1), depth(0xAAAA), true, false);
        let b = fb_pair(color(1, 1, 1, 1), depth(0xBBBB), true, false);
        assert!(!are_framebuffer_pairs_compatible(
            index(1, 1),
            index(2, 2),
            a,
            b
        ));
    }

    #[test]
    fn depth_write_on_both_sides_and_mismatched_address_fails() {
        let a = fb_pair(color(1, 1, 1, 1), depth(0xAAAA), false, true);
        let b = fb_pair(color(1, 1, 1, 1), depth(0xBBBB), false, true);
        assert!(!are_framebuffer_pairs_compatible(
            index(1, 1),
            index(2, 2),
            a,
            b
        ));
    }

    #[test]
    fn depth_read_first_depth_write_second_and_mismatched_address_fails() {
        // Gate is (depthRead||depthWrite) on EACH side independently, not
        // requiring the same flag on both.
        let a = fb_pair(color(1, 1, 1, 1), depth(0xAAAA), true, false);
        let b = fb_pair(color(1, 1, 1, 1), depth(0xBBBB), false, true);
        assert!(!are_framebuffer_pairs_compatible(
            index(1, 1),
            index(2, 2),
            a,
            b
        ));
    }

    #[test]
    fn depth_active_only_on_first_side_ignores_depth_address_mismatch() {
        // Gate requires BOTH sides to have depthRead||depthWrite -- if only
        // one side is depth-active, the depth-address check is skipped.
        let a = fb_pair(color(1, 1, 1, 1), depth(0xAAAA), true, false);
        let b = fb_pair(color(1, 1, 1, 1), depth(0xBBBB), false, false);
        assert!(are_framebuffer_pairs_compatible(
            index(1, 1),
            index(2, 2),
            a,
            b
        ));
    }

    #[test]
    fn depth_active_only_on_second_side_ignores_depth_address_mismatch() {
        let a = fb_pair(color(1, 1, 1, 1), depth(0xAAAA), false, false);
        let b = fb_pair(color(1, 1, 1, 1), depth(0xBBBB), true, false);
        assert!(are_framebuffer_pairs_compatible(
            index(1, 1),
            index(2, 2),
            a,
            b
        ));
    }

    #[test]
    fn depth_active_both_sides_matching_address_passes_depth_gate() {
        let a = fb_pair(color(1, 1, 1, 1), depth(0xAAAA), true, true);
        let b = fb_pair(color(1, 1, 1, 1), depth(0xAAAA), true, true);
        assert!(are_framebuffer_pairs_compatible(
            index(1, 1),
            index(2, 2),
            a,
            b
        ));
    }

    // --- are_framebuffer_pairs_compatible: color-image fields ---

    #[test]
    fn color_address_mismatch_fails() {
        let a = fb_pair(color(0x1000, 5, 2, 320), depth(0), false, false);
        let b = fb_pair(color(0x2000, 5, 2, 320), depth(0), false, false);
        assert!(!are_framebuffer_pairs_compatible(
            index(1, 1),
            index(2, 2),
            a,
            b
        ));
    }

    #[test]
    fn color_fmt_mismatch_fails() {
        let a = fb_pair(color(0x1000, 5, 2, 320), depth(0), false, false);
        let b = fb_pair(color(0x1000, 6, 2, 320), depth(0), false, false);
        assert!(!are_framebuffer_pairs_compatible(
            index(1, 1),
            index(2, 2),
            a,
            b
        ));
    }

    #[test]
    fn color_siz_mismatch_fails() {
        let a = fb_pair(color(0x1000, 5, 2, 320), depth(0), false, false);
        let b = fb_pair(color(0x1000, 5, 3, 320), depth(0), false, false);
        assert!(!are_framebuffer_pairs_compatible(
            index(1, 1),
            index(2, 2),
            a,
            b
        ));
    }

    #[test]
    fn color_width_mismatch_fails() {
        let a = fb_pair(color(0x1000, 5, 2, 320), depth(0), false, false);
        let b = fb_pair(color(0x1000, 5, 2, 640), depth(0), false, false);
        assert!(!are_framebuffer_pairs_compatible(
            index(1, 1),
            index(2, 2),
            a,
            b
        ));
    }

    #[test]
    fn color_all_fields_matching_passes() {
        let a = fb_pair(color(0x1000, 5, 2, 320), depth(0), false, false);
        let b = fb_pair(color(0x1000, 5, 2, 320), depth(0), false, false);
        assert!(are_framebuffer_pairs_compatible(
            index(1, 1),
            index(2, 2),
            a,
            b
        ));
    }

    #[test]
    fn color_width_max_u16_matches() {
        let a = fb_pair(color(0x1000, 5, 2, u16::MAX), depth(0), false, false);
        let b = fb_pair(color(0x1000, 5, 2, u16::MAX), depth(0), false, false);
        assert!(are_framebuffer_pairs_compatible(
            index(1, 1),
            index(2, 2),
            a,
            b
        ));
    }

    #[test]
    fn color_address_max_u32_matches() {
        let a = fb_pair(color(u32::MAX, 5, 2, 320), depth(0), false, false);
        let b = fb_pair(color(u32::MAX, 5, 2, 320), depth(0), false, false);
        assert!(are_framebuffer_pairs_compatible(
            index(1, 1),
            index(2, 2),
            a,
            b
        ));
    }

    #[test]
    fn color_fmt_max_u8_mismatch_fails() {
        let a = fb_pair(color(0x1000, u8::MAX, 2, 320), depth(0), false, false);
        let b = fb_pair(color(0x1000, u8::MAX - 1, 2, 320), depth(0), false, false);
        assert!(!are_framebuffer_pairs_compatible(
            index(1, 1),
            index(2, 2),
            a,
            b
        ));
    }

    #[test]
    fn multiple_color_field_mismatches_still_fails_once() {
        let a = fb_pair(color(0x1000, 5, 2, 320), depth(0), false, false);
        let b = fb_pair(color(0x2000, 6, 3, 640), depth(0), false, false);
        assert!(!are_framebuffer_pairs_compatible(
            index(1, 1),
            index(2, 2),
            a,
            b
        ));
    }

    #[test]
    fn zero_valued_color_image_fields_on_both_sides_match() {
        let a = fb_pair(color(0, 0, 0, 0), depth(0), false, false);
        let b = fb_pair(color(0, 0, 0, 0), depth(0), false, false);
        assert!(are_framebuffer_pairs_compatible(
            index(1, 1),
            index(2, 2),
            a,
            b
        ));
    }

    // --- is_scene_compatible: fb-pair-first short-circuit ---

    #[test]
    fn incompatible_fb_pairs_fails_regardless_of_matrix_diffs() {
        let a = fb_pair(color(0x1000, 5, 2, 320), depth(0), false, false);
        let b = fb_pair(color(0x2000, 5, 2, 320), depth(0), false, false);
        // Both matrix diffs are 0.0 (perfectly compatible), but the color
        // image mismatch must still fail the whole predicate.
        assert!(!is_scene_compatible(
            index(1, 1),
            index(2, 2),
            a,
            b,
            0.0,
            0.0
        ));
    }

    #[test]
    fn compatible_fb_pairs_and_zero_matrix_diffs_passes() {
        let a = default_fb_pair();
        let b = default_fb_pair();
        assert!(is_scene_compatible(
            index(1, 1),
            index(2, 2),
            a,
            b,
            0.0,
            0.0
        ));
    }

    #[test]
    fn identical_index_bypasses_fb_pair_field_check_in_scene_compatible_too() {
        let idx = index(5, 5);
        let a = fb_pair(color(1, 1, 1, 1), depth(1), true, true);
        let b = fb_pair(color(999, 9, 9, 999), depth(999), false, false);
        assert!(is_scene_compatible(idx, idx, a, b, 0.0, 0.0));
    }

    // --- is_scene_compatible: view matrix tolerance ---

    #[test]
    fn view_matrix_diff_at_exactly_tolerance_passes() {
        let a = default_fb_pair();
        let b = default_fb_pair();
        assert!(is_scene_compatible(
            index(1, 1),
            index(2, 2),
            a,
            b,
            1e-6,
            0.0
        ));
    }

    #[test]
    fn view_matrix_diff_just_above_tolerance_fails() {
        let a = default_fb_pair();
        let b = default_fb_pair();
        assert!(!is_scene_compatible(
            index(1, 1),
            index(2, 2),
            a,
            b,
            1e-6 + f32::EPSILON,
            0.0
        ));
    }

    #[test]
    fn view_matrix_diff_just_below_tolerance_passes() {
        let a = default_fb_pair();
        let b = default_fb_pair();
        assert!(is_scene_compatible(
            index(1, 1),
            index(2, 2),
            a,
            b,
            1e-6 - 1e-9,
            0.0
        ));
    }

    #[test]
    fn view_matrix_diff_large_fails_before_proj_diff_is_checked() {
        let a = default_fb_pair();
        let b = default_fb_pair();
        // proj_matrix_diff is also over tolerance, but the point is that
        // view is checked (and fails) first -- both being over tolerance
        // still yields a single false, consistent either way.
        assert!(!is_scene_compatible(
            index(1, 1),
            index(2, 2),
            a,
            b,
            100.0,
            100.0
        ));
    }

    // --- is_scene_compatible: proj matrix tolerance ---

    #[test]
    fn proj_matrix_diff_at_exactly_tolerance_passes() {
        let a = default_fb_pair();
        let b = default_fb_pair();
        assert!(is_scene_compatible(
            index(1, 1),
            index(2, 2),
            a,
            b,
            0.0,
            1e-6
        ));
    }

    #[test]
    fn proj_matrix_diff_just_above_tolerance_fails() {
        let a = default_fb_pair();
        let b = default_fb_pair();
        assert!(!is_scene_compatible(
            index(1, 1),
            index(2, 2),
            a,
            b,
            0.0,
            1e-6 + f32::EPSILON
        ));
    }

    #[test]
    fn proj_matrix_diff_just_below_tolerance_passes() {
        let a = default_fb_pair();
        let b = default_fb_pair();
        assert!(is_scene_compatible(
            index(1, 1),
            index(2, 2),
            a,
            b,
            0.0,
            1e-6 - 1e-9
        ));
    }

    #[test]
    fn view_diff_ok_but_proj_diff_over_tolerance_fails() {
        let a = default_fb_pair();
        let b = default_fb_pair();
        assert!(!is_scene_compatible(
            index(1, 1),
            index(2, 2),
            a,
            b,
            0.0,
            10.0
        ));
    }

    #[test]
    fn view_diff_over_tolerance_but_proj_diff_ok_fails() {
        let a = default_fb_pair();
        let b = default_fb_pair();
        assert!(!is_scene_compatible(
            index(1, 1),
            index(2, 2),
            a,
            b,
            10.0,
            0.0
        ));
    }

    // --- is_scene_compatible: negative and NaN matrix diffs ---

    #[test]
    fn negative_matrix_diffs_pass_the_greater_than_check() {
        // matrixDifference is presumably non-negative in practice (it is a
        // magnitude), but the ported comparison is a plain f32 `>`, so a
        // negative input is well-defined and passes (not > tolerance).
        let a = default_fb_pair();
        let b = default_fb_pair();
        assert!(is_scene_compatible(
            index(1, 1),
            index(2, 2),
            a,
            b,
            -1.0,
            -1.0
        ));
    }

    #[test]
    fn nan_view_matrix_diff_fails_the_greater_than_check() {
        // NaN > tolerance is false in IEEE-754, so the `if diff >
        // tolerance { return false }` branch is NOT taken for NaN -- this
        // falls through to the proj check, matching plain f32 comparison
        // semantics with no special-casing added.
        let a = default_fb_pair();
        let b = default_fb_pair();
        assert!(is_scene_compatible(
            index(1, 1),
            index(2, 2),
            a,
            b,
            f32::NAN,
            0.0
        ));
    }

    #[test]
    fn nan_proj_matrix_diff_also_fails_the_greater_than_check() {
        let a = default_fb_pair();
        let b = default_fb_pair();
        assert!(is_scene_compatible(
            index(1, 1),
            index(2, 2),
            a,
            b,
            0.0,
            f32::NAN
        ));
    }

    // --- FbPairFields / index struct field-level Debug/Clone sanity ---

    #[test]
    fn fb_pair_fields_is_copy_and_clone() {
        let a = default_fb_pair();
        let b = a;
        assert_eq!(a, b);
    }

    #[test]
    fn framebuffer_pair_index_is_copy_and_clone() {
        let a = index(3, 4);
        let b = a;
        assert_eq!(a, b);
    }
}
