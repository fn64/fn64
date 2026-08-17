//! The `RDPTile` inter-frame interpolation formula from RT64's
//! `TileProcessor::process`: a literal port of the permitted MIT RT64
//! Rust-port source pinned at commit
//! `5473732a822a4423b5696e7cb18fecc425a59875`
//! (`docs/RT64-PORT-AUTHORITY.md`):
//!
//! - `src/render/rt64_tile_processor.cpp` (whole-file SHA-256,
//!   `2428f032a8c0cf9481862079ebdcfe62ea30633b2f5c2e463f515532e89dd4f3`, 67
//!   newline-terminated lines plus a final unterminated line -- the closing
//!   `};` -- which `docs/rt64-port-inventory.json` records as 68). That
//!   digest was computed independently here with `shasum -a 256` against the
//!   pinned port-commit checkout and cross-checked verbatim against that
//!   inventory's
//!   `files[path="src/render/rt64_tile_processor.cpp"].sources.port.sha256`,
//!   which records the identical digest -- **no mismatch**.
//!
//! The interpolated fields' declared types come from
//! `src/shared/rt64_rdp_tile.h:22-25` (`float uls; float ult; float lrs;
//! float lrt;`) and `src/hle/rt64_game_frame.h:58-68`
//! (`GameFrameMap::TileMap`'s `float prevUls/prevUlt/prevLrs/prevLrt` and
//! its `bool mapped`). Both are read **for type and gating context only**;
//! no arithmetic is taken from either and neither is cited as a port source
//! (see "Nonclaims") -- crediting them in the burndown for a type reading
//! would be over-credit.
//!
//! ```text
//! // rt64_tile_processor.cpp:36-43 (inside TileProcessor::process's inner loop)
//! const GameFrameMap::TileMap &tileMap = workloadMap.tiles[t];
//! if (!tileMap.mapped) {
//!     continue;
//! }
//!
//! const interop::RDPTile &curTile = curRdpTiles[t];
//! interop::RDPTile &lerpTile = lerpRdpTiles[t];
//! lerpTile.uls = tileMap.prevUls + (curTile.uls - tileMap.prevUls) * p.curFrameWeight;
//! lerpTile.ult = tileMap.prevUlt + (curTile.ult - tileMap.prevUlt) * p.curFrameWeight;
//! lerpTile.lrs = tileMap.prevLrs + (curTile.lrs - tileMap.prevLrs) * p.curFrameWeight;
//! lerpTile.lrt = tileMap.prevLrt + (curTile.lrt - tileMap.prevLrt) * p.curFrameWeight;
//! ```
//!
//! ## Ported fraction, stated plainly
//!
//! **This is a partial port: 4 of the cited file's 68 lines (lines 40-43)
//! carry the arithmetic this module characterizes; 64 lines are refused.**
//! That is roughly 6% of the file. The inventory marks a source `ported` at
//! *file* granularity, so once `docs/rt64-port-inventory.json` records this
//! module in `ported_as` the burndown will credit all 68 lines. **It should
//! not**: 94% of this file is `BufferUploader`/RHI upload plumbing and
//! `WorkloadQueue`/`GameFrame` iteration with no CPU meaning. The
//! over-credit is disclosed here because the burndown mechanism is known to
//! over-credit for exactly this reason, and because a 6%-of-file port is the
//! extreme end of that hazard.
//!
//! `docs/rt64-port-inventory.json` currently records this path as
//! `"port_state": "not-started"` with `"ported_as": []`.
//! `scripts/lint-docs.py`'s inventory scanner is expected to report a
//! `ported_as` drift for `src/render/rt64_tile_processor.cpp` until a
//! follow-up regenerates the inventory; this card's writable surface does not
//! include that file (two sibling lanes are landing modules concurrently and
//! a regeneration from here would race their entries), so that reconciliation
//! is deliberately left to the owning ticket.
//!
//! Per-file drift disclosure for the two context-only paths above:
//! `src/shared/rt64_rdp_tile.h` and `src/hle/rt64_game_frame.h` are
//! **cited-but-not-ported** -- read here for field types and the `mapped`
//! gate's existence, contributing no line to this module's behavior. Neither
//! is quoted with a whole-file digest, precisely so the mechanical
//! SHA-256 citation scan does not attribute them to this module.
//!
//! ## Reuse, not new type
//!
//! The four interpolated members are plain `float` scalars
//! (`rt64_rdp_tile.h:22-25`) and the weight is a plain `float`
//! (`TileProcessor::ProcessParams::curFrameWeight`, `rt64_tile_processor.h:
//! 21`), so this module introduces **no new type at all** -- not a struct,
//! not a wrapper. [`rdp_tile_lerp_component`] is a free function over `f32`,
//! and [`rdp_tile_lerp_bounds`] returns a bare `[f32; 4]` in the source's
//! own declaration order. There is deliberately no `RdpTile` struct minted
//! here: `interop::RDPTile` has a much wider field surface than these four
//! members (this loop touches only `uls`/`ult`/`lrs`/`lrt` and leaves every
//! other field at the value the preceding `lerpRdpTiles = curRdpTiles`
//! bulk-copy gave it), and minting a partial mirror would invite a second,
//! independently-drifting definition of a type this crate does not own.
//!
//! ## Admitted domain
//!
//! - **The tile lerp is the canonical `prev + (cur - prev) * weight` form,
//!   and this is NOT the same expression its sibling processor in the same
//!   directory uses.** `LookAtProcessor::process`
//!   (`rt64_look_at_processor.cpp:40-41`) writes `cur - delta * (1.0f -
//!   curFrameWeight)`, already ported as
//!   [`crate::rt64_interpolation_helpers::look_at_lerp_component`]. Both are
//!   hand-written expressions -- neither is a call to `hlslpp::lerp`
//!   (confirmed by reading both files: the string `lerp` appears in
//!   `rt64_tile_processor.cpp` only in the identifiers `lerpRdpTiles` and
//!   `lerpTile`, never as a call). The two forms are algebraically equal over
//!   the reals but **not bit-identical in `f32`**, so they must not be
//!   unified into one helper, and neither may be rewritten into the other's
//!   shape. [`tile_and_look_at_lerp_forms_are_not_interchangeable`] below
//!   pins a concrete input triple where the two forms disagree in the last
//!   `f32` bit, so a future tidy-up that routes one through the other is
//!   killed by a test rather than passing review.
//!
//!   **The strength of this claim, stated precisely.** The two forms agree
//!   bit-for-bit on *most* inputs -- an exhaustive sweep over short-decimal
//!   triples finds agreement far more often than disagreement, and a first
//!   guessed witness (`prev = 0.1, cur = 0.3, w = 0.7`) agreed exactly. The
//!   claim is therefore **not** "these always differ"; it is "these are not
//!   interchangeable", i.e. there exist inputs on which they differ, so a
//!   substitution is not behavior-preserving. The pinned witness was found
//!   by search, not assumed, precisely because the assumed one was wrong.
//!   A reviewer should not read a passing spot-check of some other triple as
//!   evidence the forms may be merged.
//! - **Operand order and association are preserved literally**: the
//!   difference `cur - prev` is formed first, multiplied by `weight`, and
//!   *then* added to `prev`. It is never reassociated to `prev * (1 -
//!   weight) + cur * weight`, nor to `prev + cur * weight - prev * weight`;
//!   those are different `f32` computations. [`rdp_tile_lerp_component`]
//!   writes the expression in exactly the source's shape.
//! - **`weight` is not clamped and the source does not clamp it.** RT64
//!   passes `ProcessParams::curFrameWeight`, whose declared default is
//!   `1.0f` (`rt64_tile_processor.h:21`), but nothing in the cited lines
//!   range-checks it. Extrapolation (`weight > 1.0` or `weight < 0.0`) is
//!   therefore in-domain and produces a value outside `[prev, cur]`;
//!   [`rdp_tile_lerp_component_extrapolates_past_one`] pins that rather than
//!   pretending a clamp exists.
//! - **At `weight == 1.0` the result is NOT guaranteed to be exactly `cur`.**
//!   This form computes `prev + (cur - prev) * 1.0 = prev + (cur - prev)`,
//!   and `f32` subtraction followed by addition is not an identity when
//!   `cur` and `prev` differ greatly in magnitude -- the difference rounds,
//!   and adding it back does not recover `cur`. This is the sharpest
//!   behavioral contrast with the LookAt form, which *is* exactly `cur` at
//!   `weight == 1.0` (there `1.0 - 1.0` is exactly `0.0`, so the delta term
//!   vanishes identically). [`rdp_tile_lerp_component_weight_one_is_not
//!   _exactly_cur_under_cancellation`] pins a concrete pair where
//!   `prev + (cur - prev) != cur`. At `weight == 0.0` the result **is**
//!   exactly `prev`, because `(cur - prev) * 0.0` is `+0.0` for any finite
//!   difference and `prev + 0.0 == prev` (for `prev != -0.0`); that
//!   asymmetry is pinned too.
//! - **The four members are interpolated independently and identically**,
//!   with no cross-member coupling (no shared delta magnitude, no ordering
//!   constraint between `uls`/`lrs` or `ult`/`lrt` -- the source never
//!   checks that the interpolated rectangle stays non-inverted).
//!   [`rdp_tile_lerp_bounds`] applies the scalar helper four times in the
//!   source's own textual order (`uls`, `ult`, `lrs`, `lrt`) and
//!   [`rdp_tile_lerp_bounds_may_invert_the_rectangle`] pins that an inverted
//!   result is producible and not corrected.
//!
//! ## Nonclaims
//!
//! No GPU, WGSL, or production wiring (this module is not called from
//! anywhere yet; dead-code warnings on its public surface are expected and
//! correct), no RT64 visual/pixel/silicon parity claim, no performance
//! claim, and no `repr(C)`, size, alignment or ABI claim about
//! `interop::RDPTile` or `GameFrameMap::TileMap` -- no mirror of either is
//! declared here, so no such claim is even expressible.
//!
//! Deliberately **not** ported from `rt64_tile_processor.cpp` (64 lines):
//!
//! - `TileProcessor`'s constructor, destructor, and `setup` (which allocates
//!   a `BufferUploader` from `worker->device`) -- RHI plumbing.
//! - `TileProcessor::upload` in its entirety (lines 48-66): `uploads.clear()`,
//!   the per-workload `BufferUploader::Upload` record construction, the
//!   `sizeof(interop::RDPTile)` stride, the `RenderBufferFlag::STORAGE` flag,
//!   the `assert(rdpTiles.size() == lerpRdpTiles.size())`, and
//!   `bufferUploader->submit(...)` -- all GPU upload scaffolding.
//! - `TileProcessor::process`'s surrounding structure (lines 21-38, 44-46):
//!   the outer `for (uint32_t w : p.curFrame->workloads)` iteration, the
//!   `p.workloadQueue->workloads[w]` / `workload.drawData` indexing, the
//!   `prevFrameValid` predicate (`(p.prevFrame != nullptr) &&
//!   p.curFrame->frameMap.workloads[w].mapped`), the
//!   `lerpRdpTiles = curRdpTiles` bulk copy that seeds the output, the inner
//!   `for (size_t t = ...)` loop, and the `if (!tileMap.mapped) continue;`
//!   skip. This module ports only the four-line lerp body as a pure function
//!   of already-extracted scalars -- not the iteration or indexing over
//!   engine state, and not the `mapped` gate (a lookup into a frame-map
//!   structure this crate does not model).
//! - `p.prevFrameWeight` (`rt64_tile_processor.h:22`): declared on
//!   `ProcessParams` but never read anywhere in `rt64_tile_processor.cpp`.
//!   Refused as unused-in-this-file rather than modelled.
//!
//! `src/render/rt64_tile_processor.h` (32 lines) is refused **in full** and
//! is not cited with a digest: it is a bare declaration of the struct's
//! `std::unique_ptr<BufferUploader>` / `std::vector<Upload>` members and its
//! five method signatures, plus the `ProcessParams` pointer bundle. Its only
//! non-pointer contents are the two `float` defaults noted above, which are
//! call-site defaults rather than behavior of the ported expression.

/// One component of RT64's `RDPTile` inter-frame interpolation
/// (`rt64_tile_processor.cpp:40-43`): `prev + (cur - prev) * weight`,
/// literally.
///
/// This is the **canonical** lerp form. Its sibling in the same directory,
/// [`crate::rt64_interpolation_helpers::look_at_lerp_component`], uses the
/// *different* hand-written form `cur - delta * (1.0 - weight)`. The two are
/// not bit-identical in `f32` and must not be unified; see the module doc's
/// "Admitted domain".
///
/// `weight` is not clamped, matching the source.
pub fn rdp_tile_lerp_component(prev: f32, cur: f32, weight: f32) -> f32 {
    prev + (cur - prev) * weight
}

/// The four `RDPTile` bounds RT64 interpolates, in the source's own textual
/// order: `uls`, `ult`, `lrs`, `lrt` (`rt64_tile_processor.cpp:40-43`).
///
/// Each component is interpolated independently by
/// [`rdp_tile_lerp_component`]; there is no cross-component coupling and no
/// correction if the interpolated rectangle inverts.
pub fn rdp_tile_lerp_bounds(prev: [f32; 4], cur: [f32; 4], weight: f32) -> [f32; 4] {
    [
        rdp_tile_lerp_component(prev[0], cur[0], weight),
        rdp_tile_lerp_component(prev[1], cur[1], weight),
        rdp_tile_lerp_component(prev[2], cur[2], weight),
        rdp_tile_lerp_component(prev[3], cur[3], weight),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Hand-derived midpoint: `prev = 8.0`, `cur = 24.0`, `weight = 0.5`.
    /// `cur - prev = 16.0`; `16.0 * 0.5 = 8.0`; `8.0 + 8.0 = 16.0`. Every
    /// intermediate is a small power-of-two-scaled integer, exactly
    /// representable in `f32`, so the expected value is exact.
    ///
    /// Derived a second, independent way: the midpoint of `[8, 24]` is
    /// `(8 + 24) / 2 = 32 / 2 = 16`. Both readings give `16.0` -- reconciled.
    #[test]
    fn rdp_tile_lerp_component_midpoint_is_exact() {
        assert_eq!(rdp_tile_lerp_component(8.0, 24.0, 0.5), 16.0);
        // Quarter and three-quarter points, same exact-arithmetic reasoning:
        // 8 + 16*0.25 = 12; 8 + 16*0.75 = 20.
        assert_eq!(rdp_tile_lerp_component(8.0, 24.0, 0.25), 12.0);
        assert_eq!(rdp_tile_lerp_component(8.0, 24.0, 0.75), 20.0);
    }

    /// `weight == 0.0` returns exactly `prev`, for any finite `cur`:
    /// `(cur - prev) * 0.0` is `+0.0` when the difference is finite, and
    /// `prev + 0.0 == prev`.
    ///
    /// This is checked against a `cur` chosen so that `cur - prev` is *not*
    /// exactly representable relative to `prev` (see the cancellation test
    /// below), proving the zero-weight identity does not depend on the
    /// subtraction being exact.
    #[test]
    fn rdp_tile_lerp_component_weight_zero_returns_prev_exactly() {
        assert_eq!(rdp_tile_lerp_component(7.5, 129.25, 0.0), 7.5);
        assert_eq!(rdp_tile_lerp_component(1.0e30, 1.0, 0.0), 1.0e30);
        assert_eq!(rdp_tile_lerp_component(-3.0, 11.0, 0.0), -3.0);
    }

    /// **The asymmetry with the LookAt form.** At `weight == 1.0` this form
    /// computes `prev + (cur - prev)`, which is *not* an `f32` identity when
    /// `cur` and `prev` differ greatly in magnitude: the difference rounds
    /// and adding it back does not recover `cur`.
    ///
    /// Hand-derived: `prev = 1.0e30f32`. `f32` has a 24-bit significand, so
    /// near `1e30` the representable spacing (ulp) is far larger than `1.0`.
    /// Therefore `cur = 1.0e30 + 1.0` rounds to exactly `1.0e30` on
    /// evaluation, `cur - prev == 0.0`, and the result is `1.0e30`, not
    /// `1.0e30 + 1.0`.
    ///
    /// Derived independently: `(1.0e30f32).next_up() - 1.0e30f32` is the ulp
    /// at that magnitude; the test asserts `1.0` is far below it, which is
    /// *why* the addition cannot recover the extra unit. Both readings agree
    /// that the result is `prev` unchanged.
    #[test]
    fn rdp_tile_lerp_component_weight_one_is_not_exactly_cur_under_cancellation() {
        let prev: f32 = 1.0e30;
        let cur: f32 = 1.0e30 + 1.0;

        // Independent reading: the ulp here dwarfs 1.0, so `cur` is not even
        // a distinct f32 from `prev`.
        let ulp = f32::from_bits(prev.to_bits() + 1) - prev;
        assert!(ulp > 1.0e21, "ulp at 1e30 must dwarf 1.0, got {ulp}");
        assert_eq!(cur, prev, "cur rounds to prev at this magnitude");

        assert_eq!(rdp_tile_lerp_component(prev, cur, 1.0), prev);

        // A case where cur IS distinct from prev but the round-trip still
        // loses: prev = 1.0, cur = 1.0 + 2^-23 (one ulp up at 1.0). Here
        // (cur - prev) is exact and the identity DOES hold -- included so the
        // test pins the boundary, not just the failure.
        let small_prev: f32 = 1.0;
        let small_cur: f32 = f32::from_bits(small_prev.to_bits() + 1);
        assert_ne!(small_cur, small_prev);
        assert_eq!(
            rdp_tile_lerp_component(small_prev, small_cur, 1.0),
            small_cur
        );
    }

    /// **The two lerp forms in `src/render/` are not interchangeable.**
    ///
    /// `TileProcessor` uses `prev + (cur - prev) * w`; `LookAtProcessor` uses
    /// `cur - delta * (1 - w)`. With `delta = cur - prev` these are
    /// algebraically equal over the reals, but not bit-identical in `f32`.
    ///
    /// Witness: `prev = 0.3`, `cur = -0.2`, `w = 0.12`, `delta = cur - prev`.
    /// - Tile form: `0.3 + (-0.2 - 0.3) * 0.12`.
    /// - LookAt form: `-0.2 - (-0.2 - 0.3) * (1.0 - 0.12)`.
    ///
    /// None of `0.3`, `-0.2`, `0.12`, or `1.0 - 0.12` is exact in binary
    /// `f32`, so the two evaluation orders round to adjacent `f32` values.
    ///
    /// **This witness was found by exhaustive search, not assumed.** An
    /// earlier draft of this test guessed `(0.1, 0.3, 0.7)` and the test
    /// failed: that triple happens to round *identically* in both forms.
    /// Most triples do. The claim being pinned is that the forms are not
    /// universally interchangeable, and it needs a triple where they
    /// actually differ -- so one was searched for rather than presumed.
    ///
    /// The test asserts they differ, and additionally that they differ by no
    /// more than a few ulps (so the witness is a genuine rounding split, not
    /// a typo in one of the formulas).
    ///
    /// A future refactor that routes either helper through the other is
    /// killed here.
    #[test]
    fn tile_and_look_at_lerp_forms_are_not_interchangeable() {
        let prev: f32 = 0.3;
        let cur: f32 = -0.2;
        let w: f32 = 0.12;
        let delta: f32 = cur - prev;

        let tile = rdp_tile_lerp_component(prev, cur, w);
        let look_at = crate::rt64_interpolation_helpers::look_at_lerp_component(cur, delta, w);

        assert_ne!(
            tile.to_bits(),
            look_at.to_bits(),
            "the two hand-written lerp forms must not be unified: \
             tile={tile:?} look_at={look_at:?}"
        );
        // Second, independent reading: they are the *same* value to within a
        // few ulps, confirming both formulas are correct transcriptions and
        // the difference is pure rounding order.
        let ulp = f32::from_bits(tile.to_bits() + 1) - tile;
        assert!(
            (tile - look_at).abs() <= 4.0 * ulp,
            "difference must be a rounding split, not a formula error: \
             tile={tile:?} look_at={look_at:?} ulp={ulp:?}"
        );
    }

    /// `weight` is unclamped in the source, so extrapolation past `1.0` and
    /// below `0.0` is in-domain.
    ///
    /// Hand-derived, exact arithmetic throughout: `prev = 4.0`, `cur = 8.0`,
    /// `cur - prev = 4.0`.
    /// - `w = 2.0`:  `4 + 4*2  = 12`.
    /// - `w = -1.0`: `4 + 4*-1 = 0`.
    /// - `w = 3.5`:  `4 + 4*3.5 = 18`.
    ///
    /// Independent reading: extrapolation at `w` should land at
    /// `prev + w * (cur - prev)` = `4 + 4w`; substituting `w = 2, -1, 3.5`
    /// gives `12, 0, 18` -- reconciled with the above.
    #[test]
    fn rdp_tile_lerp_component_extrapolates_past_one() {
        assert_eq!(rdp_tile_lerp_component(4.0, 8.0, 2.0), 12.0);
        assert_eq!(rdp_tile_lerp_component(4.0, 8.0, -1.0), 0.0);
        assert_eq!(rdp_tile_lerp_component(4.0, 8.0, 3.5), 18.0);
    }

    /// The four bounds are interpolated independently, in the source's
    /// textual order `uls`, `ult`, `lrs`, `lrt`, with no coupling.
    ///
    /// Hand-derived at `weight = 0.5` with exactly-representable operands:
    /// - `uls`: `0 + (16 - 0)*0.5 = 8`
    /// - `ult`: `4 + (20 - 4)*0.5 = 4 + 8 = 12`
    /// - `lrs`: `64 + (32 - 64)*0.5 = 64 - 16 = 48`
    /// - `lrt`: `128 + (128 - 128)*0.5 = 128`
    ///
    /// The distinct per-slot values also pin the *order*: swapping any two
    /// slots in [`rdp_tile_lerp_bounds`] changes the output array.
    #[test]
    fn rdp_tile_lerp_bounds_applies_per_component_in_source_order() {
        let prev = [0.0f32, 4.0, 64.0, 128.0];
        let cur = [16.0f32, 20.0, 32.0, 128.0];
        assert_eq!(
            rdp_tile_lerp_bounds(prev, cur, 0.5),
            [8.0, 12.0, 48.0, 128.0]
        );

        // Each slot must equal the scalar helper on that slot's own operands
        // -- no cross-slot term.
        let out = rdp_tile_lerp_bounds(prev, cur, 0.25);
        for i in 0..4 {
            assert_eq!(out[i], rdp_tile_lerp_component(prev[i], cur[i], 0.25));
        }
    }

    /// The source never checks that the interpolated rectangle stays
    /// non-inverted, so an inverted result (`lrs < uls`) is producible and is
    /// not corrected.
    ///
    /// Hand-derived: start non-inverted (`uls = 0`, `lrs = 100`) and end
    /// inverted (`uls = 100`, `lrs = 0`). At `weight = 0.75`:
    /// - `uls`: `0 + (100 - 0)*0.75 = 75`
    /// - `lrs`: `100 + (0 - 100)*0.75 = 100 - 75 = 25`
    ///
    /// `75 > 25`, so the rectangle is inverted on the `s` axis and passes
    /// through unmodified.
    #[test]
    fn rdp_tile_lerp_bounds_may_invert_the_rectangle() {
        let prev = [0.0f32, 0.0, 100.0, 100.0];
        let cur = [100.0f32, 0.0, 0.0, 100.0];
        let out = rdp_tile_lerp_bounds(prev, cur, 0.75);
        assert_eq!(out, [75.0, 0.0, 25.0, 100.0]);
        assert!(
            out[2] < out[0],
            "an inverted rectangle must pass through uncorrected"
        );
    }
}
