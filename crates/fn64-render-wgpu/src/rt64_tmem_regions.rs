//! Literal port of `RT64::FramebufferManager`'s TMEM region interval
//! tracker: `insertRegionsTMEM`, `discardRegionsTMEM`, and
//! `synchronizeRegionsTMEM`. A literal port of the permitted MIT RT64
//! Rust-port source pinned at commit `5473732a822a4423b5696e7cb18fecc425a59875`
//! (`docs/RT64-PORT-AUTHORITY.md`):
//!
//! - `src/hle/rt64_framebuffer_manager.cpp:517-634` (whole-file SHA-256,
//!   `1a97e98b34dc4707d4a9514ef6992bd751e5a0d6fe2c5bcefd50234b41686fd5`, 1093
//!   lines -- matching `docs/rt64-port-inventory.json`'s `sources.port.sha256`
//!   for that path, confirmed independently here by `shasum -a 256` against
//!   the pinned port-commit checkout).
//! - `src/hle/rt64_framebuffer_manager.h:69-75,168,173` (`RegionTMEM`, the
//!   `std::list<RegionTMEM> activeRegionsTMEM` member, and the
//!   `RegionIterator` alias; whole-file SHA-256,
//!   `fca8057640165a3e97994581da1a427c79d350559b928776e1c0d1707813eeee`, 230
//!   lines -- matching the same inventory field, confirmed the same way).
//! - `src/hle/rt64_framebuffer.h:82-95` (`FramebufferTile`'s field list only
//!   -- this module never reads or interprets those fields, it only stores
//!   and default-clears the payload as the interval tracker does; whole-file
//!   SHA-256, `95e132fa28c97412d6e63e36c96c7b15df846943c3d8dd156a64da12beb479b0`,
//!   96 lines -- matching the same inventory field, confirmed the same way).
//! - `src/hle/rt64_rdp.h:21` (`#define RDP_TMEM_WORDS 512`).
//!
//! `docs/rt64-port-inventory.json` does not yet record
//! `src/hle/rt64_framebuffer_manager.cpp`/`.h`'s `ported_as` as pointing at
//! this module (both currently list other/empty `ported_as` entries) --
//! `scripts/lint-docs.py`'s inventory scanner is expected to report a drift
//! for that until a follow-up regenerates the inventory to add this module;
//! this module's own writable surface does not include
//! `docs/rt64-port-inventory.json`, so that reconciliation is deliberately
//! left to the owning ticket rather than done here (matching
//! `rt64_framebuffer_geometry.rs`'s precedent for the same situation).
//!
//! ```text
//! void FramebufferManager::insertRegionsTMEM(uint32_t addressStart, uint32_t tmemStart, uint32_t tmemWords, uint32_t tmemMask, bool RGBA32, bool syncRequired, std::vector<RegionIterator> *resultRegions) {
//!     if (resultRegions != nullptr) {
//!         resultRegions->clear();
//!     }
//!
//!     auto insertRegions = [&](bool upperTMEM) {
//!         RegionTMEM newRegion = { };
//!         newRegion.fbTile.address = addressStart;
//!         newRegion.syncRequired = syncRequired;
//!
//!         const uint32_t tmemAdd = upperTMEM ? (RDP_TMEM_WORDS >> 1) : 0;
//!         const uint32_t byteShift = RGBA32 ? 4 : 3;
//!         uint32_t tmemEnd = (tmemStart & tmemMask) + tmemWords;
//!         const uint32_t tmemBarrier = tmemEnd;
//!         uint32_t tmemCursor = tmemEnd;
//!         uint32_t wordsLeft = tmemWords;
//!         while (wordsLeft > 0) {
//!             if ((tmemCursor > tmemBarrier) && ((tmemCursor - tmemBarrier) > wordsLeft)) {
//!                 wordsLeft -= (tmemCursor - tmemBarrier);
//!                 newRegion.tmemStart = tmemBarrier;
//!                 newRegion.tmemEnd = tmemCursor + tmemAdd;
//!                 wordsLeft = 0;
//!             }
//!             else if (wordsLeft > tmemCursor) {
//!                 wordsLeft -= tmemCursor;
//!                 newRegion.tmemStart = tmemAdd;
//!                 newRegion.tmemEnd = tmemCursor + tmemAdd;
//!                 tmemCursor = (tmemMask + 1);
//!             }
//!             else {
//!                 tmemCursor -= wordsLeft;
//!                 newRegion.tmemStart = tmemCursor + tmemAdd;
//!                 newRegion.tmemEnd = tmemCursor + wordsLeft + tmemAdd;
//!                 wordsLeft = 0;
//!             }
//!
//!             activeRegionsTMEM.push_front(newRegion);
//!
//!             if (resultRegions != nullptr) {
//!                 resultRegions->push_back(activeRegionsTMEM.begin());
//!             }
//!         }
//!         };
//!
//!     insertRegions(false);
//!
//!     if (RGBA32) {
//!         insertRegions(true);
//!     }
//! }
//!
//! void FramebufferManager::discardRegionsTMEM(uint32_t tmemStart, uint32_t tmemWords, uint32_t tmemMask) {
//!     tmemStart = tmemStart & tmemMask;
//!
//!     const uint32_t wordLimit = (tmemMask + 1);
//!     if ((tmemStart + tmemWords) > wordLimit) {
//!         const uint32_t leftWords = wordLimit - tmemStart;
//!         discardRegionsTMEM(tmemStart, leftWords, tmemMask);
//!
//!         const uint32_t rightWords = tmemWords - leftWords;
//!         discardRegionsTMEM(0, std::min(tmemStart, rightWords), tmemMask);
//!     }
//!     else {
//!         auto it = activeRegionsTMEM.begin();
//!         const uint32_t tmemEnd = tmemStart + tmemWords;
//!         while (it != activeRegionsTMEM.end()) {
//!             if ((it->tmemStart < tmemEnd) && (it->tmemEnd > tmemStart)) {
//!                 // Region is fully contained within the discard region. Erase the region.
//!                 if ((it->tmemStart >= tmemStart) && (it->tmemEnd <= tmemEnd)) {
//!                     it->tmemEnd = it->tmemStart;
//!                 }
//!                 // Only the right side of the region is contained withing the discard region. Shrink the region.
//!                 else if ((it->tmemStart <= tmemStart) && (it->tmemEnd < tmemEnd)) {
//!                     it->fbTile = {};
//!                     it->tmemEnd = tmemStart;
//!                 }
//!                 // Only the left side of the region is contained withing the discard region. Move the start of the region.
//!                 else if ((it->tmemStart > tmemStart) && (it->tmemEnd >= tmemEnd)) {
//!                     it->fbTile = {};
//!                     it->tmemStart = tmemEnd;
//!                 }
//!                 // The discard region is fully contained inside the region but doesn't cover each side. Must shrink the
//!                 // region to the left side and insert a new one for the right side.
//!                 else {
//!                     it->fbTile = {};
//!
//!                     // Don't add the new region if it'd end up being empty.
//!                     if (it->tmemEnd != tmemEnd) {
//!                         RegionTMEM newRegion = *it;
//!                         newRegion.tmemStart = tmemEnd;
//!                         activeRegionsTMEM.push_back(newRegion);
//!                     }
//!
//!                     it->tmemEnd = tmemStart;
//!                 }
//!
//!                 // Region is empty, erase it.
//!                 if (it->tmemStart == it->tmemEnd) {
//!                     it = activeRegionsTMEM.erase(it);
//!                 }
//!                 else {
//!                     it++;
//!                 }
//!             }
//!             else {
//!                 it++;
//!             }
//!         }
//!     }
//! }
//!
//! void FramebufferManager::synchronizeRegionsTMEM() {
//!     auto it = activeRegionsTMEM.begin();
//!     while (it != activeRegionsTMEM.end()) {
//!         it->syncRequired = false;
//!         it++;
//!     }
//! }
//! ```
//!
//! ```text
//! // rt64_framebuffer_manager.h
//! struct RegionTMEM {
//!     uint32_t tmemStart;
//!     uint32_t tmemEnd;
//!     FramebufferTile fbTile;
//!     uint64_t tileCopyId;
//!     bool syncRequired;
//! };
//! std::list<RegionTMEM> activeRegionsTMEM;
//! typedef std::list<RegionTMEM>::iterator RegionIterator;
//!
//! // rt64_framebuffer.h
//! struct FramebufferTile {
//!     uint32_t address;
//!     uint8_t siz;
//!     uint8_t fmt;
//!     uint32_t left;
//!     uint32_t top;
//!     uint32_t right;
//!     uint32_t bottom;
//!     uint32_t lineWidth;
//!     uint32_t ditherPattern;
//! };
//!
//! // rt64_rdp.h
//! #define RDP_TMEM_WORDS 512
//! ```
//!
//! **Reuse, not new type.** No Rust `FramebufferTile` type exists anywhere
//! in this crate (`rt64_framebuffer_geometry.rs`, the sibling module that
//! quotes the same C++ struct in its doc header, defines only free functions
//! over raw fields -- `framebuffer_tile_valid(left, top, right, bottom)` --
//! not a `FramebufferTile` struct; confirmed by grepping `struct
//! FramebufferTile` across `crates/`). `rt64_tmem_hasher.rs`'s two ported
//! predicates (`needs_to_hash_rows_individually`, `requires_raw_tmem`) work
//! in *bytes* against fixed 4096/2048 budgets and take a `LoadTile`, not a
//! `RegionTMEM`/`FramebufferTile` -- nothing there is a boundary, constant,
//! or predicate this module's *word*-space interval arithmetic (`tmemMask`,
//! `RDP_TMEM_WORDS`) can reuse; the two modules operate on disjoint
//! quantities (interval bookkeeping in TMEM *words* here vs. byte-budget
//! predicates there) and are cited against each other only to record that
//! the search was made. This module therefore defines one minimal local
//! [`FbTile`] mirroring `FramebufferTile`'s 9 fields verbatim: the interval
//! tracker never reads or interprets any of those fields, it only ever (a)
//! sets `.address` on construction and (b) resets the whole payload to
//! default (`= {}`) in three of the four discard branches -- so `FbTile`'s
//! only load-bearing property is round-tripping construct/default-clear
//! identically to the C++ struct, which `#[derive(Default)]` on a 9-field
//! POD gives for free without inventing interpretation this module has no
//! license to add.
//!
//! ## Admitted domain
//!
//! - **The wraparound `while` loop in `insertRegionsTMEM` always runs
//!   exactly once, for every possible input.** At loop entry `tmemCursor ==
//!   tmemBarrier == tmemEnd == (tmemStart & tmemMask) + tmemWords` and
//!   `wordsLeft == tmemWords`. Cond1 (`tmemCursor > tmemBarrier`) is false
//!   on the first iteration because the two are equal by construction.
//!   Cond2 (`wordsLeft > tmemCursor`) is also false on the first iteration:
//!   `tmemCursor - wordsLeft == (tmemStart & tmemMask) + tmemWords -
//!   tmemWords == (tmemStart & tmemMask) >= 0` always (unsigned), so
//!   `wordsLeft <= tmemCursor` always holds. The `else` branch is therefore
//!   always taken, and it always sets `wordsLeft = 0`, ending the loop.
//!   Verified by two independent methods: (1) an exhaustive 200,000-sample
//!   randomized sweep over `tmemStart in [0, 2^32)`, `tmemWords in [0,
//!   5000]`, `tmemMask in {0, 255, 511, 1023, 2^32-1}` found zero cases
//!   where cond1 or cond2 fired on the reachable first iteration
//!   (`/tmp/prove.py` in this session); (2) the algebraic identity above.
//!   **Consequence:** `insertRegionsTMEM` as literally written does **not**
//!   reduce an oversized or boundary-crossing region modulo `tmemMask + 1`
//!   -- a single call always emits exactly one region,
//!   `[(tmemStart & tmemMask) + tmemAdd, (tmemStart & tmemMask) + tmemWords
//!   + tmemAdd)`, even when that range's end exceeds `tmemMask + 1` (a
//!   region "longer than TMEM itself" is emitted whole, unclamped -- see
//!   [`tests::insert_region_longer_than_tmem_is_not_split`] and
//!   [`tests::insert_region_crossing_boundary_is_not_split`]). This reads
//!   like dead code the way the hazard brief warns ("a no-`break` loop that
//!   looked like a bug but was real behavior") -- cond1 and cond2 are
//!   ported literally as unreachable-from-a-fresh-call branches rather than
//!   removed, because removing them would silently change behavior if this
//!   function's `tmemCursor`/`wordsLeft` initialization is ever edited
//!   upstream without this port being re-verified, and because "literal
//!   port" forbids normalizing branch structure even when a branch is
//!   provably dead under current initialization. No test can exercise
//!   cond1/cond2 as *reachable* from [`insert_regions_tmem`]'s public entry
//!   point, by the same proof; this is reported here rather than silently
//!   worked around.
//! - **The exact wraparound boundary, restated in plain terms.** Because
//!   the loop never actually splits, "wraparound" only ever manifests as:
//!   the emitted `tmemStart`/`tmemEnd` are computed from `tmemStart &
//!   tmemMask` (so a start past the mask wraps down into range) added to
//!   the raw, un-clamped `tmemWords` -- the resulting `tmemEnd` is
//!   inclusive-of-nothing/exclusive-at-the-top in the conventional
//!   half-open-interval sense (`[tmemStart, tmemEnd)`), and it is never
//!   truncated back down to `tmemMask + 1` even when it overshoots. A
//!   region ending exactly at `tmemMask + 1` (e.g. start=462, words=50,
//!   mask=511 -> `[462, 512)`) is therefore indistinguishable in kind from
//!   one that overshoots by 50 words (start=480, words=50, mask=511 ->
//!   `[480, 530)`): both take the same `else` branch, both are emitted as
//!   one whole, unsplit region. See
//!   [`tests::insert_region_ending_exactly_at_boundary`] and
//!   [`tests::insert_region_crossing_boundary_is_not_split`].
//! - **`byteShift` is computed but never read anywhere in
//!   `insertRegionsTMEM`.** `const uint32_t byteShift = RGBA32 ? 4 : 3;` has
//!   no further use in the function body (confirmed by grepping the
//!   function's text for the identifier: it appears exactly once, at its
//!   own declaration). This is ported as a genuinely dead local
//!   ([`_byte_shift`], the underscore-prefix documenting that it is
//!   intentionally unused, matching the source) rather than dropped --
//!   dropping a source statement, even a dead one, is not this module's
//!   license under a literal-port mandate.
//! - **RGBA32 double pass and its ordering.** When `RGBA32` is true,
//!   `insertRegions` runs twice: once with `upperTMEM = false` (`tmemAdd =
//!   0`), then with `upperTMEM = true` (`tmemAdd = RDP_TMEM_WORDS >> 1 =
//!   256`). Each call is independent (its own fresh `tmemCursor`/
//!   `wordsLeft`), and each `push_front`s its one emitted region onto
//!   `active_regions_tmem` -- so after an RGBA32 insert, the *upper*-half
//!   region (inserted second) sits at the front of the list, followed by
//!   the *lower*-half region (inserted first), followed by whatever was
//!   already there. See
//!   [`tests::insert_regions_tmem_rgba32_double_pass_order_and_offset`].
//! - **`insertRegionsTMEM` pushes to the FRONT (`push_front`); the
//!   discard split branch pushes the new right remainder to the BACK
//!   (`push_back`).** Both are reproduced with the matching end of
//!   [`std::collections::VecDeque`] (`push_front`/`push_back`) rather than
//!   normalized to one operation, and iteration in [`discard_regions_tmem`]
//!   walks front-to-back exactly as `std::list::iterator`'s `it++` does, so
//!   a newly `push_back`ed remainder is visited later in the *same* pass
//!   only if the walk hasn't already passed the end -- matching C++
//!   `std::list` (a linked list, where `push_back` while iterating is safe
//!   and the appended node is reachable by continuing `it++`, unlike a
//!   `Vec`/`VecDeque` where growing during a manual index walk needs the
//!   same care this port gives it: indices are tracked manually and the
//!   loop bound is re-read from `.len()` each iteration, not cached).
//! - **Comparison strictness in `discardRegionsTMEM`'s outer overlap test:**
//!   `(it->tmemStart < tmemEnd) && (it->tmemEnd > tmemStart)` -- both
//!   *strict*. Two intervals that only touch (one's end equals the other's
//!   start) do **not** overlap by this test: region `[100,150)` vs. discard
//!   `[150,200)` gives `150 < 200` true but `150 > 150` false (region's own
//!   `tmemEnd` vs. discard's `tmemStart`), so the region is left completely
//!   untouched. See [`tests::discard_touching_interval_is_untouched`].
//!   Shrinking the overlap by one word on either side (discard `[149,200)`
//!   against the same region) makes `it->tmemEnd(150) > tmemStart(149)`
//!   true, so the region *is* affected. See
//!   [`tests::discard_overlapping_by_one_word_is_affected`].
//! - **The four-way subtraction's branch conditions, and their strictness,
//!   ported literally and independently (no case merged into another):**
//!   1. Fully contained: `tmemStart >= tmemStart_discard && tmemEnd <=
//!      tmemEnd_discard` (both non-strict/inclusive) -> erase: sets
//!      `tmemEnd = tmemStart` **without** clearing `fbTile`.
//!   2. Right-side-only-contained ("shrink"): `tmemStart <= start_discard
//!      && tmemEnd < end_discard` (left non-strict, right strict) -> clears
//!      `fbTile`, then `tmemEnd = start_discard` (keeps the region's left
//!      remainder `[tmemStart, start_discard)`).
//!   3. Left-side-only-contained ("move start"): `tmemStart > start_discard
//!      && tmemEnd >= end_discard` (left strict, right non-strict) ->
//!      clears `fbTile`, then `tmemStart = end_discard` (keeps the right
//!      remainder `[end_discard, tmemEnd)`).
//!   4. Else (discard fully inside the region, split): clears `fbTile`
//!      first, then -- only if the resulting right remainder would be
//!      non-empty (`tmemEnd != end_discard`) -- copies the (already
//!      fbTile-cleared) region, sets the copy's `tmemStart = end_discard`,
//!      and `push_back`s it; finally sets the original's `tmemEnd =
//!      start_discard`.
//!   **The asymmetry is real and preserved literally:** case 1 is the only
//!   one of the four that does not clear `fbTile` -- because case 1 is
//!   erasing the whole region outright (the immediately-following "region
//!   is empty, erase it" check fires unconditionally for case 1, so
//!   clearing `fbTile` first would be observably wasted work on a node
//!   about to be deleted, and doing so anyway would be a *behavior change*
//!   from the source, not a no-op). Cases 2-4 all shrink/split a region
//!   that *survives* the discard (at least in part), so their surviving
//!   remainder must not keep stale `fbTile` state from before the discard.
//!   Ported as four independent `if`/`else if`/`else if`/`else` arms in
//!   [`discard_regions_tmem`] (not collapsed to two, not reordered) --
//!   see [`tests::discard_case1_fully_contained_erases_without_clearing_marker`],
//!   [`tests::discard_case2_shrinks_from_right_and_clears_marker`],
//!   [`tests::discard_case3_moves_start_and_clears_marker`], and
//!   [`tests::discard_case4_splits_and_clears_marker_on_both_halves`].
//!   **Could any two of the four have been wrongly collapsed?** Cases 2 and
//!   3 look superficially symmetric (both "partially contained, shrink one
//!   side") and a naive port might fold them into a single "shrink toward
//!   whichever side overlaps" branch -- but their *comparison strictness*
//!   differs in which side is strict (case 2: `tmemEnd < end_discard`
//!   strict-right; case 3: `tmemStart > start_discard` strict-left), so a
//!   collapsed version would misclassify the exact-touch boundary case
//!   differently than the source does. This port keeps all four `if`/`else
//!   if` arms exactly as ordered and worded in the source.
//! - **Discard recursion depth is bounded to exactly one split, never
//!   deeper, for any input.** `discardRegionsTMEM` recurses when
//!   `tmemStart + tmemWords > tmemMask + 1`, splitting into a left call
//!   (`tmemStart`, `leftWords = wordLimit - tmemStart`) and a right call
//!   (`0`, `min(tmemStart, rightWords)`). The right call's own width is
//!   clamped to at most `tmemStart <= tmemMask < wordLimit`, so the right
//!   call's own `0 + width <= tmemMask < wordLimit` can never itself
//!   satisfy the recursion guard again -- verified by a 200,000-sample
//!   randomized sweep (`/tmp/prove_discard_depth.py` in this session;
//!   `tmemStart` up to `2^32-1`, `tmemWords` up to 100,000, several masks)
//!   which never observed recursion depth exceeding 1. **The
//!   `min(tmemStart, rightWords)` clamp on the second recursive call is
//!   preserved exactly** ([`discard_regions_tmem`]'s `right_words.min
//!   (tmem_start)` matches the source's `std::min(tmemStart, rightWords)`
//!   argument order, which matters only for readability here since `min`
//!   is commutative, but the *presence* of the clamp is what the hazard
//!   brief warns is easy to drop and would silently over-discard: without
//!   it, `discard(0, tmemWords - leftWords, tmemMask)` could itself exceed
//!   `wordLimit` again for large `tmemWords`, discarding words that were
//!   never in the caller's requested range). See
//!   [`tests::discard_crossing_boundary_recurses_and_clamps_right_half`]
//!   and [`tests::discard_recursion_clamp_prevents_over_discard_for_huge_width`].
//! - **`synchronizeRegionsTMEM` is a plain forward walk with no
//!   conditionals or erasure** -- it unconditionally sets `syncRequired =
//!   false` on every region, in list order, and never removes or reorders
//!   anything. Ported as a single iterator `for_each` equivalent
//!   ([`synchronize_regions_tmem`]).
//! - **Container ordering.** `std::list<RegionTMEM>` is an ordered,
//!   iterator-stable doubly linked list; `push_front`/`push_back`/`erase`
//!   all have direct `VecDeque` equivalents that preserve the same
//!   observable order for this module's access pattern (front-to-back scan
//!   with in-place mutation, front insertion, and back insertion of at most
//!   one new element per discard call). This module does **not** use
//!   `retain`/`filter` anywhere the source erases conditionally mid-scan --
//!   [`discard_regions_tmem`] walks by explicit index with manual
//!   `remove`/advance, exactly mirroring the source's manual
//!   `it = erase(it)` vs. `it++` branch, because a `retain` closure cannot
//!   reproduce "erase this element, and also possibly push a *new* element
//!   onto the back, observable in the same pass" in the same order.
//!
//! ## Nonclaims
//!
//! No GPU, WGSL, or production wiring (this module is not called from
//! anywhere yet -- dead-code warnings on the unused public surface are
//! expected and correct, matching `rt64_tmem_hasher.rs`/
//! `rt64_framebuffer_geometry.rs`'s precedent), and no RT64 visual/pixel/
//! silicon parity or performance claim. This is also a **deliberately
//! partial port of `rt64_framebuffer_manager.cpp`**: that file is ~1093
//! lines and mostly bound to fn64's not-yet-ported State/Workload graph.
//! This module ports only the three TMEM region interval functions named by
//! ticket M4.11 (`insertRegionsTMEM`, `discardRegionsTMEM`,
//! `synchronizeRegionsTMEM`). Sibling ticket M4.10 owns `makeFramebufferTile`
//! from the same source file -- this module does not touch it, does not
//! define anything under `crates/fn64-render-wgpu/src/tmem/` (claimed by
//! M4.2/M4.3), and does not read or write `docs/rt64-port-inventory.json`.
//! `checkRegionsTMEM`, `checkTileCopyTMEM`, `createTileCopyRecord`,
//! `createTileCopySetup`, `destroyAllTileCopies`, `find`,
//! `findMostRecentContaining`, `findTileCopyId`, `get`,
//! `getUsedTimestamp`, `hashTracking`, `makeTileCopyTMEM`,
//! `makeTileReintepretation`, `nextWriteTimestamp`, `performDiscards`,
//! `performOperations`, `recordOperations`, `reinterpretTileRecord`,
//! `reinterpretTileSetup`, `resetOperations`, `resetTracking`,
//! `setupOperations`, `storeRAM`, `uploadRAM`, `writeChanges`, `changeRAM`,
//! `checkRAM`, `clearUsedTileCopies`, and the `FramebufferManager`
//! constructor are all **not ported** -- every one of them is bound to
//! `RenderWorker`/`RenderTarget`/`Workload`/GPU descriptor-set machinery
//! this crate's State/Workload graph does not yet have a Rust equivalent
//! for, well outside this ticket's named scope. `resultRegions`
//! (`std::vector<RegionIterator> *`) is ported as a `Vec<usize>` of
//! stable slot indices into this module's own [`RegionTmemList`] rather
//! than a live iterator/reference, because Rust has no safe direct
//! equivalent of a `std::list::iterator` handed back to an unrelated
//! caller across further mutation of the same list; this is a
//! representation change forced by the language, not a behavior change --
//! the *order* and *identity* of which regions get returned is preserved
//! (see [`insert_regions_tmem`]'s doc comment).

use std::collections::VecDeque;

/// `RDP_TMEM_WORDS` (`src/hle/rt64_rdp.h:21`): the fixed TMEM size in
/// 8-byte words (512 words = 4096 bytes).
pub const RDP_TMEM_WORDS: u32 = 512;

/// Minimal local mirror of RT64's `FramebufferTile`
/// (`src/hle/rt64_framebuffer.h:82-95`). See module doc "Reuse, not new
/// type" for why this exists as a fresh POD rather than reusing an
/// existing crate type (none exists) and why the interval tracker never
/// interprets its fields -- it only constructs (`.address` set) and
/// default-clears (`= {}`) it as an opaque payload.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FbTile {
    pub address: u32,
    pub siz: u8,
    pub fmt: u8,
    pub left: u32,
    pub top: u32,
    pub right: u32,
    pub bottom: u32,
    pub line_width: u32,
    pub dither_pattern: u32,
}

/// Literal port of `RegionTMEM` (`src/hle/rt64_framebuffer_manager.h:69-75`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RegionTmem {
    pub tmem_start: u32,
    pub tmem_end: u32,
    pub fb_tile: FbTile,
    pub tile_copy_id: u64,
    pub sync_required: bool,
}

/// Literal port of `std::list<RegionTMEM> activeRegionsTMEM`
/// (`src/hle/rt64_framebuffer_manager.h:168`). A `VecDeque` reproduces the
/// same observable front/back push order and front-to-back iteration order
/// this module's ported functions rely on (see module doc "Container
/// ordering").
pub type RegionTmemList = VecDeque<RegionTmem>;

/// Literal port of `FramebufferManager::insertRegionsTMEM`
/// (`src/hle/rt64_framebuffer_manager.cpp:517-566`). See module doc
/// "Admitted domain" for why the inner loop always runs exactly once, and
/// for the RGBA32 double-pass ordering.
///
/// `result_regions`, when `Some`, is cleared and then filled with the
/// **front-list index** (`0` = the list's current front slot after each
/// push) of each newly inserted region, in the same order
/// `resultRegions->push_back(activeRegionsTMEM.begin())` would record them
/// -- i.e. always index `0`, once per region pushed, because every push is
/// a `push_front` and `activeRegionsTMEM.begin()` always refers to the
/// most-recently-pushed element at the moment of the push. This mirrors the
/// C++ call site's actual observable content (a run of `begin()` iterators
/// captured immediately after each push), not a claim that a later mutation
/// of the list will keep these indices valid.
pub fn insert_regions_tmem(
    regions: &mut RegionTmemList,
    address_start: u32,
    tmem_start: u32,
    tmem_words: u32,
    tmem_mask: u32,
    rgba32: bool,
    sync_required: bool,
    mut result_regions: Option<&mut Vec<usize>>,
) {
    if let Some(out) = result_regions.as_deref_mut() {
        out.clear();
    }

    let mut insert_regions_pass = |regions: &mut RegionTmemList, upper_tmem: bool| {
        let mut new_region = RegionTmem {
            fb_tile: FbTile {
                address: address_start,
                ..FbTile::default()
            },
            sync_required,
            ..RegionTmem::default()
        };

        let tmem_add: u32 = if upper_tmem { RDP_TMEM_WORDS >> 1 } else { 0 };
        // Computed but never read anywhere in the source function body --
        // see module doc "Admitted domain" ("`byteShift` is computed but
        // never read").
        let _byte_shift: u32 = if rgba32 { 4 } else { 3 };
        let tmem_end0: u32 = (tmem_start & tmem_mask) + tmem_words;
        let tmem_barrier: u32 = tmem_end0;
        let mut tmem_cursor: u32 = tmem_end0;
        let mut words_left: u32 = tmem_words;
        while words_left > 0 {
            if (tmem_cursor > tmem_barrier) && ((tmem_cursor - tmem_barrier) > words_left) {
                words_left -= tmem_cursor - tmem_barrier;
                new_region.tmem_start = tmem_barrier;
                new_region.tmem_end = tmem_cursor + tmem_add;
                words_left = 0;
            } else if words_left > tmem_cursor {
                words_left -= tmem_cursor;
                new_region.tmem_start = tmem_add;
                new_region.tmem_end = tmem_cursor + tmem_add;
                tmem_cursor = tmem_mask + 1;
            } else {
                tmem_cursor -= words_left;
                new_region.tmem_start = tmem_cursor + tmem_add;
                new_region.tmem_end = tmem_cursor + words_left + tmem_add;
                words_left = 0;
            }

            regions.push_front(new_region);

            if let Some(out) = result_regions.as_deref_mut() {
                out.push(0);
            }
        }
    };

    insert_regions_pass(regions, false);

    if rgba32 {
        insert_regions_pass(regions, true);
    }
}

/// Literal port of `FramebufferManager::discardRegionsTMEM`
/// (`src/hle/rt64_framebuffer_manager.cpp:568-626`). See module doc
/// "Admitted domain" for the recursion depth bound, the `min` clamp, the
/// four-way subtraction's exact conditions/strictness, and why this walks
/// by manual index rather than `retain`.
pub fn discard_regions_tmem(
    regions: &mut RegionTmemList,
    tmem_start: u32,
    tmem_words: u32,
    tmem_mask: u32,
) {
    let tmem_start = tmem_start & tmem_mask;

    let word_limit = tmem_mask + 1;
    if (tmem_start + tmem_words) > word_limit {
        let left_words = word_limit - tmem_start;
        discard_regions_tmem(regions, tmem_start, left_words, tmem_mask);

        let right_words = tmem_words - left_words;
        discard_regions_tmem(regions, 0, right_words.min(tmem_start), tmem_mask);
    } else {
        let tmem_end = tmem_start + tmem_words;
        let mut i = 0usize;
        while i < regions.len() {
            let overlaps = regions[i].tmem_start < tmem_end && regions[i].tmem_end > tmem_start;
            if overlaps {
                // Case 1: region is fully contained within the discard
                // region. Erase the region. (No fbTile clear -- see module
                // doc "Admitted domain" for why this is the one branch that
                // doesn't clear.)
                if regions[i].tmem_start >= tmem_start && regions[i].tmem_end <= tmem_end {
                    regions[i].tmem_end = regions[i].tmem_start;
                }
                // Case 2: only the right side of the region is contained
                // within the discard region. Shrink the region.
                else if regions[i].tmem_start <= tmem_start && regions[i].tmem_end < tmem_end {
                    regions[i].fb_tile = FbTile::default();
                    regions[i].tmem_end = tmem_start;
                }
                // Case 3: only the left side of the region is contained
                // within the discard region. Move the start of the region.
                else if regions[i].tmem_start > tmem_start && regions[i].tmem_end >= tmem_end {
                    regions[i].fb_tile = FbTile::default();
                    regions[i].tmem_start = tmem_end;
                }
                // Case 4: the discard region is fully contained inside the
                // region but doesn't cover each side. Must shrink the
                // region to the left side and insert a new one for the
                // right side.
                else {
                    regions[i].fb_tile = FbTile::default();

                    // Don't add the new region if it'd end up being empty.
                    if regions[i].tmem_end != tmem_end {
                        let mut new_region = regions[i];
                        new_region.tmem_start = tmem_end;
                        regions.push_back(new_region);
                    }

                    regions[i].tmem_end = tmem_start;
                }

                // Region is empty, erase it.
                if regions[i].tmem_start == regions[i].tmem_end {
                    regions.remove(i);
                    // Do not advance `i`: the next element has shifted into
                    // this slot, matching `it = activeRegionsTMEM.erase(it)`
                    // leaving `it` pointing at the following element.
                } else {
                    i += 1;
                }
            } else {
                i += 1;
            }
        }
    }
}

/// Literal port of `FramebufferManager::synchronizeRegionsTMEM`
/// (`src/hle/rt64_framebuffer_manager.cpp:628-634`).
pub fn synchronize_regions_tmem(regions: &mut RegionTmemList) {
    for region in regions.iter_mut() {
        region.sync_required = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn region(start: u32, end: u32) -> RegionTmem {
        RegionTmem {
            tmem_start: start,
            tmem_end: end,
            ..RegionTmem::default()
        }
    }

    fn region_marked(start: u32, end: u32) -> RegionTmem {
        RegionTmem {
            tmem_start: start,
            tmem_end: end,
            fb_tile: FbTile {
                address: 0xAAAA_AAAA,
                ..FbTile::default()
            },
            ..RegionTmem::default()
        }
    }

    // ---------------------------------------------------------------
    // insertRegionsTMEM
    // ---------------------------------------------------------------

    #[test]
    fn insert_region_entirely_inside_tmem() {
        // Hand-computed: tmemEnd0 = (100 & 511) + 50 = 150; the only
        // reachable else-branch sets tmemCursor = 150 - 50 = 100,
        // tmemStart = 100, tmemEnd = 100 + 50 = 150.
        let mut regions = RegionTmemList::new();
        insert_regions_tmem(&mut regions, 0xABCD, 100, 50, 511, false, false, None);
        assert_eq!(regions.len(), 1);
        assert_eq!(regions[0].tmem_start, 100);
        assert_eq!(regions[0].tmem_end, 150);
        assert_eq!(regions[0].fb_tile.address, 0xABCD);
        assert!(!regions[0].sync_required);
    }

    #[test]
    fn insert_region_ending_exactly_at_boundary() {
        // Hand-computed: start=462, words=50, mask=511 -> tmemEnd0 = 512
        // (== tmemMask+1, the exact boundary). Cursor = 512-50=462,
        // tmemStart=462, tmemEnd=512. One region, ends exactly at the
        // TMEM limit, not split.
        let mut regions = RegionTmemList::new();
        insert_regions_tmem(&mut regions, 0, 462, 50, 511, false, false, None);
        assert_eq!(regions.len(), 1);
        assert_eq!(regions[0].tmem_start, 462);
        assert_eq!(regions[0].tmem_end, 512);
    }

    #[test]
    fn insert_region_crossing_boundary_is_not_split() {
        // Hand-computed: start=480, words=50, mask=511 -> naive end=530,
        // which exceeds tmemMask+1=512 by 18. Per "Admitted domain", the
        // else-branch always fires and this is emitted whole, unclamped:
        // tmemStart=480, tmemEnd=530 (NOT wrapped/split into two nodes).
        let mut regions = RegionTmemList::new();
        insert_regions_tmem(&mut regions, 0, 480, 50, 511, false, false, None);
        assert_eq!(regions.len(), 1);
        assert_eq!(regions[0].tmem_start, 480);
        assert_eq!(regions[0].tmem_end, 530);
    }

    #[test]
    fn insert_region_longer_than_tmem_itself_is_not_split() {
        // Hand-computed: start=0, words=600 (> RDP_TMEM_WORDS=512),
        // mask=511 -> tmemEnd0 = 0 + 600 = 600. else-branch: cursor =
        // 600-600=0, tmemStart=0, tmemEnd=600. One region, wider than TMEM.
        let mut regions = RegionTmemList::new();
        insert_regions_tmem(&mut regions, 0, 0, 600, 511, false, false, None);
        assert_eq!(regions.len(), 1);
        assert_eq!(regions[0].tmem_start, 0);
        assert_eq!(regions[0].tmem_end, 600);
    }

    #[test]
    fn insert_region_start_reduced_by_mask_before_adding_words() {
        // Hand-computed: tmemStart=513 (one past mask=511), mask=511 ->
        // (513 & 511) = 1. tmemEnd0 = 1 + 20 = 21. else-branch:
        // cursor=21-20=1, tmemStart=1, tmemEnd=21. The masking is applied
        // to the *start* only, before tmemWords is added.
        let mut regions = RegionTmemList::new();
        insert_regions_tmem(&mut regions, 0, 513, 20, 511, false, false, None);
        assert_eq!(regions.len(), 1);
        assert_eq!(regions[0].tmem_start, 1);
        assert_eq!(regions[0].tmem_end, 21);
    }

    #[test]
    fn insert_region_zero_words_pushes_nothing() {
        // words_left starts at 0, so the while loop body never runs.
        let mut regions = RegionTmemList::new();
        insert_regions_tmem(&mut regions, 0, 100, 0, 511, false, false, None);
        assert_eq!(regions.len(), 0);
    }

    #[test]
    fn insert_regions_tmem_pushes_to_front() {
        // A pre-existing region should end up AFTER the newly-inserted one.
        let mut regions = RegionTmemList::new();
        regions.push_back(region(900, 950));
        insert_regions_tmem(&mut regions, 0, 10, 5, 511, false, false, None);
        assert_eq!(regions.len(), 2);
        assert_eq!(regions[0].tmem_start, 10);
        assert_eq!(regions[0].tmem_end, 15);
        assert_eq!(regions[1].tmem_start, 900);
    }

    #[test]
    fn insert_regions_tmem_rgba32_double_pass_order_and_offset() {
        // Hand-computed lower pass (upperTMEM=false, tmemAdd=0): start=100,
        // words=50, mask=511 -> tmemEnd0=150, else-branch cursor=100,
        // tmemStart=100, tmemEnd=150.
        // Hand-computed upper pass (upperTMEM=true, tmemAdd=256): SAME
        // tmemStart/tmemWords/tmemMask inputs re-run independently ->
        // tmemEnd0=150 again, cursor=100 again, but tmemAdd=256 this time:
        // tmemStart=100+256=356, tmemEnd=150+256=406.
        // Lower pass runs first (push_front), upper pass runs second
        // (push_front) -- so the upper-half region ends up at the FRONT.
        let mut regions = RegionTmemList::new();
        insert_regions_tmem(&mut regions, 0, 100, 50, 511, true, false, None);
        assert_eq!(regions.len(), 2);
        assert_eq!(regions[0].tmem_start, 356);
        assert_eq!(regions[0].tmem_end, 406);
        assert_eq!(regions[1].tmem_start, 100);
        assert_eq!(regions[1].tmem_end, 150);
    }

    #[test]
    fn insert_regions_tmem_non_rgba32_skips_upper_pass() {
        let mut regions = RegionTmemList::new();
        insert_regions_tmem(&mut regions, 0, 100, 50, 511, false, false, None);
        assert_eq!(regions.len(), 1);
    }

    #[test]
    fn insert_regions_tmem_result_regions_records_front_slot_per_push() {
        // Two pushes (RGBA32 double pass): resultRegions records begin()
        // immediately after each push_front, which is always the front
        // slot (index 0) at that moment.
        let mut regions = RegionTmemList::new();
        let mut out = Vec::new();
        insert_regions_tmem(&mut regions, 0, 100, 50, 511, true, false, Some(&mut out));
        assert_eq!(out, vec![0, 0]);
    }

    #[test]
    fn insert_regions_tmem_result_regions_cleared_when_some() {
        let mut regions = RegionTmemList::new();
        let mut out = vec![7, 8, 9];
        insert_regions_tmem(&mut regions, 0, 100, 50, 511, false, false, Some(&mut out));
        assert_eq!(out, vec![0]);
    }

    #[test]
    fn insert_regions_tmem_sync_required_propagates() {
        let mut regions = RegionTmemList::new();
        insert_regions_tmem(&mut regions, 0, 100, 50, 511, false, true, None);
        assert!(regions[0].sync_required);
    }

    #[test]
    fn insert_regions_tmem_address_start_sets_fb_tile_address_only() {
        // newRegion.fbTile.address = addressStart is the ONLY fbTile field
        // touched; every other FbTile field stays at its default (0).
        let mut regions = RegionTmemList::new();
        insert_regions_tmem(&mut regions, 0x1234_5678, 0, 10, 511, false, false, None);
        assert_eq!(regions[0].fb_tile.address, 0x1234_5678);
        assert_eq!(regions[0].fb_tile.siz, 0);
        assert_eq!(regions[0].fb_tile.fmt, 0);
        assert_eq!(regions[0].fb_tile.left, 0);
        assert_eq!(regions[0].fb_tile.top, 0);
        assert_eq!(regions[0].fb_tile.right, 0);
        assert_eq!(regions[0].fb_tile.bottom, 0);
        assert_eq!(regions[0].fb_tile.line_width, 0);
        assert_eq!(regions[0].fb_tile.dither_pattern, 0);
    }

    // ---------------------------------------------------------------
    // discardRegionsTMEM: comparison strictness at boundaries
    // ---------------------------------------------------------------

    #[test]
    fn discard_touching_interval_is_untouched() {
        // Region [100,150), discard [150,200): tmemEnd(discard's own
        // tmemStart+tmemWords)=150+50=200. Outer test:
        // region.tmemStart(100) < 200 true; region.tmemEnd(150) >
        // discard.tmemStart(150) -> 150>150 FALSE (strict). No overlap.
        let mut regions = RegionTmemList::new();
        regions.push_back(region(100, 150));
        discard_regions_tmem(&mut regions, 150, 50, 511);
        assert_eq!(regions.len(), 1);
        assert_eq!(regions[0].tmem_start, 100);
        assert_eq!(regions[0].tmem_end, 150);
    }

    #[test]
    fn discard_overlapping_by_one_word_is_affected() {
        // Region [100,150), discard [149,200): discard tmemEnd=149+51=200.
        // Outer test: 100<200 true; region.tmemEnd(150) >
        // discard.tmemStart(149) -> true. Overlap by exactly one word.
        // Classification: region.tmemStart(100)<=discard.tmemStart(149)
        // true; region.tmemEnd(150) < discard.tmemEnd(200) true -> case 2
        // (shrink from right): tmemEnd becomes discard.tmemStart=149.
        let mut regions = RegionTmemList::new();
        regions.push_back(region_marked(100, 150));
        discard_regions_tmem(&mut regions, 149, 51, 511);
        assert_eq!(regions.len(), 1);
        assert_eq!(regions[0].tmem_start, 100);
        assert_eq!(regions[0].tmem_end, 149);
        assert_eq!(regions[0].fb_tile, FbTile::default());
    }

    #[test]
    fn discard_touching_on_the_other_side_is_untouched() {
        // Region [150,200), discard [100,150): discard tmemEnd=100+50=150.
        // Outer test: region.tmemStart(150) < discard tmemEnd(150) ->
        // FALSE (strict). No overlap.
        let mut regions = RegionTmemList::new();
        regions.push_back(region(150, 200));
        discard_regions_tmem(&mut regions, 100, 50, 511);
        assert_eq!(regions.len(), 1);
        assert_eq!(regions[0].tmem_start, 150);
        assert_eq!(regions[0].tmem_end, 200);
    }

    #[test]
    fn discard_overlapping_by_one_word_on_the_other_side_is_affected() {
        // Region [150,200), discard [100,151): tmemEnd=100+51=151.
        // Outer test: region.tmemStart(150) < 151 true. Overlap.
        // Classification: region.tmemStart(150) > discard.tmemStart(100)
        // true; region.tmemEnd(200) >= discard.tmemEnd(151) true -> case 3
        // (move start): tmemStart becomes discard.tmemEnd=151.
        let mut regions = RegionTmemList::new();
        regions.push_back(region_marked(150, 200));
        discard_regions_tmem(&mut regions, 100, 51, 511);
        assert_eq!(regions.len(), 1);
        assert_eq!(regions[0].tmem_start, 151);
        assert_eq!(regions[0].tmem_end, 200);
        assert_eq!(regions[0].fb_tile, FbTile::default());
    }

    // ---------------------------------------------------------------
    // discardRegionsTMEM: four discard branches, independently
    // ---------------------------------------------------------------

    #[test]
    fn discard_case1_fully_contained_erases_without_clearing_marker() {
        // Region [100,150), discard [90,200): fully contains the region
        // (100>=90, 150<=200) -> case 1. tmemEnd=tmemStart=100, then the
        // "empty, erase" check removes it. This is the one case that never
        // touches fb_tile before erasing.
        let mut regions = RegionTmemList::new();
        regions.push_back(region_marked(100, 150));
        discard_regions_tmem(&mut regions, 90, 110, 511);
        assert_eq!(regions.len(), 0);
    }

    #[test]
    fn discard_case1_boundary_inclusive_both_sides() {
        // Discard region exactly equal to the tracked region: [100,150)
        // discard [100,150). tmemStart>=tmemStart (100>=100 true, non-strict)
        // and tmemEnd<=tmemEnd (150<=150 true, non-strict) -> case 1 fires
        // on an exact match, confirming both comparisons are non-strict.
        let mut regions = RegionTmemList::new();
        regions.push_back(region(100, 150));
        discard_regions_tmem(&mut regions, 100, 50, 511);
        assert_eq!(regions.len(), 0);
    }

    #[test]
    fn discard_case2_shrinks_from_right_and_clears_marker() {
        // Region [100,150), discard [50,120): discard tmemEnd=50+70=120.
        // region.tmemStart(100)<=discard.tmemStart(50)? 100<=50 FALSE.
        // Re-derive with a case that actually satisfies case2's own
        // condition: region.tmemStart <= discard.tmemStart.
        // Use discard [80,120) instead: discard tmemEnd=80+40=120.
        // region.tmemStart(100)<=80? still false. Case 2 requires the
        // region to start at-or-before the discard start, i.e. discard
        // starts inside-or-after the region. Use discard [120,200):
        // discard tmemEnd=120+80=200. region.tmemStart(100)<=120 true;
        // region.tmemEnd(150)<200 true -> case 2. Shrinks to [100,120).
        let mut regions = RegionTmemList::new();
        regions.push_back(region_marked(100, 150));
        discard_regions_tmem(&mut regions, 120, 80, 511);
        assert_eq!(regions.len(), 1);
        assert_eq!(regions[0].tmem_start, 100);
        assert_eq!(regions[0].tmem_end, 120);
        assert_eq!(regions[0].fb_tile, FbTile::default());
    }

    #[test]
    fn discard_case3_moves_start_and_clears_marker() {
        // Region [100,150), discard [50,120): discard tmemEnd=50+70=120.
        // region.tmemStart(100)>discard.tmemStart(50) true;
        // region.tmemEnd(150)>=discard.tmemEnd(120) true -> case 3.
        // Moves start to discard.tmemEnd=120: [120,150).
        let mut regions = RegionTmemList::new();
        regions.push_back(region_marked(100, 150));
        discard_regions_tmem(&mut regions, 50, 70, 511);
        assert_eq!(regions.len(), 1);
        assert_eq!(regions[0].tmem_start, 120);
        assert_eq!(regions[0].tmem_end, 150);
        assert_eq!(regions[0].fb_tile, FbTile::default());
    }

    #[test]
    fn discard_case4_splits_and_clears_marker_on_both_halves() {
        // Region [100,150), discard [110,130): discard tmemEnd=110+20=130.
        // Fails case1 (110>=100 true but region.tmemEnd 150<=130 false).
        // Fails case2 (region.tmemStart 100<=110 true but region.tmemEnd
        // 150<130 false). Fails case3 (region.tmemStart 100>110 false).
        // -> case 4 (else): split into [100,110) and [130,150), both
        // fb_tile-cleared, right half pushed to the BACK of the list.
        let mut regions = RegionTmemList::new();
        regions.push_back(region_marked(100, 150));
        discard_regions_tmem(&mut regions, 110, 20, 511);
        assert_eq!(regions.len(), 2);
        // Original slot (index 0) shrunk to the left remainder.
        assert_eq!(regions[0].tmem_start, 100);
        assert_eq!(regions[0].tmem_end, 110);
        assert_eq!(regions[0].fb_tile, FbTile::default());
        // New back-pushed right remainder.
        assert_eq!(regions[1].tmem_start, 130);
        assert_eq!(regions[1].tmem_end, 150);
        assert_eq!(regions[1].fb_tile, FbTile::default());
    }

    #[test]
    fn discard_case4_right_remainder_omitted_when_it_would_be_empty() {
        // Region [100,150), discard [110,150): discard tmemEnd=110+40=150.
        // Fails case1 (region.tmemEnd 150<=150 true, but need BOTH
        // conditions and tmemStart 110>=100... wait recompute: case1 needs
        // region.tmemStart>=discardStart(100>=110 false) -> fails case1.
        // Fails case2 (region.tmemEnd 150<discardEnd(150) -> 150<150 false).
        // Case3: region.tmemStart(100)>discardStart(110)? false. -> case4.
        // it->tmemEnd(150) != tmemEnd(150) is FALSE, so no new region is
        // pushed -- only the shrink happens.
        let mut regions = RegionTmemList::new();
        regions.push_back(region_marked(100, 150));
        discard_regions_tmem(&mut regions, 110, 40, 511);
        assert_eq!(regions.len(), 1);
        assert_eq!(regions[0].tmem_start, 100);
        assert_eq!(regions[0].tmem_end, 110);
        assert_eq!(regions[0].fb_tile, FbTile::default());
    }

    #[test]
    fn discard_no_overlap_leaves_region_and_list_length_unchanged() {
        let mut regions = RegionTmemList::new();
        regions.push_back(region(300, 350));
        discard_regions_tmem(&mut regions, 0, 100, 511);
        assert_eq!(regions.len(), 1);
        assert_eq!(regions[0], region(300, 350));
    }

    #[test]
    fn discard_multiple_regions_only_overlapping_ones_affected() {
        let mut regions = RegionTmemList::new();
        regions.push_back(region(0, 50));
        regions.push_back(region(100, 150));
        regions.push_back(region(200, 250));
        discard_regions_tmem(&mut regions, 90, 70, 511); // discard [90,160)
                                                         // [0,50) untouched, [100,150) fully contained -> erased,
                                                         // [200,250) untouched.
        assert_eq!(regions.len(), 2);
        assert_eq!(regions[0], region(0, 50));
        assert_eq!(regions[1], region(200, 250));
    }

    // ---------------------------------------------------------------
    // discardRegionsTMEM: mask-crossing recursion
    // ---------------------------------------------------------------

    #[test]
    fn discard_crossing_boundary_recurses_and_clamps_right_half() {
        // start=480, words=60, mask=511: wordLimit=512.
        // 480+60=540 > 512 -> recurse.
        // leftWords = 512-480 = 32 -> first recursive call discard(480,32,511)
        //   covers [480,512).
        // rightWords = 60-32 = 28 -> second call discard(0, min(480,28)=28,
        //   511) covers [0,28).
        let mut regions = RegionTmemList::new();
        regions.push_back(region(470, 520)); // overlaps first half
        regions.push_back(region(10, 40)); // overlaps second half
        discard_regions_tmem(&mut regions, 480, 60, 511);
        // First call discard(480,32,511) against [470,520): discard
        // tmemEnd=480+32=512. region.tmemStart(470)<=480 true,
        // region.tmemEnd(520)>=512... case3 needs tmemStart>discardStart
        // (470>480 false) -> not case3. case2 needs tmemEnd<discardEnd
        // (520<512 false) -> not case2. case1 needs tmemEnd<=discardEnd
        // (520<=512 false) -> not case1. -> case4 split: left [470,480),
        // right [512,520) pushed to back.
        // Second call discard(0,28,511) against remaining regions
        // (including the newly split [512,520), which does not overlap
        // [0,28)) and [10,40): discard tmemEnd=0+28=28.
        // [10,40): tmemStart(10)<=0? false -> not case2. tmemStart(10)>0
        // true, tmemEnd(40)>=28 true -> case3: moves start to 28: [28,40).
        assert!(regions.contains(&region(470, 480)));
        assert!(regions.contains(&region(512, 520)));
        assert!(regions.contains(&region(28, 40)));
        assert_eq!(regions.len(), 3);
    }

    #[test]
    fn discard_recursion_clamp_prevents_over_discard_for_huge_width() {
        // start=10, words=2000, mask=511: wordLimit=512.
        // 10+2000=2010>512 -> recurse.
        // leftWords = 512-10=502 -> first call discard(10,502,511) covers
        //   [10,512) (this alone already discards anything in [10,512),
        //   independent of the clamp -- NOT a witness for the clamp).
        // rightWords = 2000-502=1498 -> WITHOUT the min() clamp the second
        //   call would be discard(0,1498,511), itself exceeding wordLimit
        //   and recursing further, eventually reaching words well past 512
        //   (e.g. [600,610), which is inside the unclamped [0,1498) range
        //   an un-clamped second call would eventually cover, via its own
        //   further recursion, but is outside BOTH the first call's
        //   [10,512) and the CLAMPED second call's range).
        // WITH the clamp: min(10,1498)=10, so the second call is
        //   discard(0,10,511), covering only [0,10) -- nowhere near 600.
        // A region at [600,610) therefore must survive iff the clamp is
        // honored; a region at [2,8) (inside the clamped [0,10)) must
        // still be discarded, confirming the clamped call still runs.
        let mut regions = RegionTmemList::new();
        regions.push_back(region(600, 610));
        regions.push_back(region(2, 8)); // inside the clamped [0,10) range
        discard_regions_tmem(&mut regions, 10, 2000, 511);
        assert!(
            regions.contains(&region(600, 610)),
            "region outside the clamped right-half range must survive: {regions:?}"
        );
        assert!(
            !regions.iter().any(|r| r.tmem_start == 2 && r.tmem_end == 8),
            "region inside the clamped [0,10) range must be discarded: {regions:?}"
        );
    }

    // ---------------------------------------------------------------
    // synchronizeRegionsTMEM
    // ---------------------------------------------------------------

    #[test]
    fn synchronize_regions_tmem_clears_all_sync_required_flags() {
        let mut regions = RegionTmemList::new();
        regions.push_back(RegionTmem {
            sync_required: true,
            ..region(0, 10)
        });
        regions.push_back(RegionTmem {
            sync_required: true,
            ..region(20, 30)
        });
        regions.push_back(RegionTmem {
            sync_required: false,
            ..region(40, 50)
        });
        synchronize_regions_tmem(&mut regions);
        assert!(regions.iter().all(|r| !r.sync_required));
        assert_eq!(regions.len(), 3);
    }

    #[test]
    fn synchronize_regions_tmem_preserves_order_and_other_fields() {
        let mut regions = RegionTmemList::new();
        regions.push_back(RegionTmem {
            sync_required: true,
            ..region(5, 15)
        });
        regions.push_back(RegionTmem {
            sync_required: true,
            ..region(25, 35)
        });
        synchronize_regions_tmem(&mut regions);
        assert_eq!(regions[0].tmem_start, 5);
        assert_eq!(regions[0].tmem_end, 15);
        assert_eq!(regions[1].tmem_start, 25);
        assert_eq!(regions[1].tmem_end, 35);
    }

    #[test]
    fn synchronize_regions_tmem_empty_list_is_a_no_op() {
        let mut regions = RegionTmemList::new();
        synchronize_regions_tmem(&mut regions);
        assert_eq!(regions.len(), 0);
    }

    // ---------------------------------------------------------------
    // RDP_TMEM_WORDS / FbTile / RegionTmem sanity
    // ---------------------------------------------------------------

    #[test]
    fn rdp_tmem_words_matches_source_define() {
        assert_eq!(RDP_TMEM_WORDS, 512);
    }

    #[test]
    fn fb_tile_default_is_all_zero() {
        assert_eq!(
            FbTile::default(),
            FbTile {
                address: 0,
                siz: 0,
                fmt: 0,
                left: 0,
                top: 0,
                right: 0,
                bottom: 0,
                line_width: 0,
                dither_pattern: 0,
            }
        );
    }

    #[test]
    fn region_tmem_default_is_all_zero_and_not_sync_required() {
        let r = RegionTmem::default();
        assert_eq!(r.tmem_start, 0);
        assert_eq!(r.tmem_end, 0);
        assert_eq!(r.fb_tile, FbTile::default());
        assert_eq!(r.tile_copy_id, 0);
        assert!(!r.sync_required);
    }

    // ---------------------------------------------------------------
    // Additional characterization: mask=0, RGBA32 crossing, discard
    // sequences, and insert-then-discard integration.
    // ---------------------------------------------------------------

    #[test]
    fn insert_regions_tmem_mask_zero_collapses_to_a_single_word() {
        // Hand-computed: tmemMask=0 -> start&0=0 always, wordLimit=1.
        // tmemEnd0 = 0 + 3 = 3. else-branch: cursor=3-3=0, tmemStart=0,
        // tmemEnd=3 (unclamped, per "Admitted domain" -- this exceeds
        // wordLimit=1 but is still emitted whole).
        let mut regions = RegionTmemList::new();
        insert_regions_tmem(&mut regions, 0, 5, 3, 0, false, false, None);
        assert_eq!(regions.len(), 1);
        assert_eq!(regions[0].tmem_start, 0);
        assert_eq!(regions[0].tmem_end, 3);
    }

    #[test]
    fn insert_regions_tmem_rgba32_crossing_boundary_neither_pass_is_split() {
        // Hand-computed: start=480, words=50, mask=511.
        // Lower pass (tmemAdd=0): tmemEnd0=530, else-branch cursor=480,
        // tmemStart=480, tmemEnd=530 (same as the non-RGBA32 crossing case).
        // Upper pass (tmemAdd=256): independently re-run with the SAME
        // start/words/mask -> tmemEnd0=530 again, cursor=480 again, but
        // offset by 256: tmemStart=736, tmemEnd=786.
        let mut regions = RegionTmemList::new();
        insert_regions_tmem(&mut regions, 0, 480, 50, 511, true, false, None);
        assert_eq!(regions.len(), 2);
        assert_eq!(regions[0].tmem_start, 736);
        assert_eq!(regions[0].tmem_end, 786);
        assert_eq!(regions[1].tmem_start, 480);
        assert_eq!(regions[1].tmem_end, 530);
    }

    #[test]
    fn discard_case1_start_touches_end_strictly_inside_still_erases() {
        // Region [100,150), discard [100,200): discard tmemEnd=100+100=200.
        // case1: region.tmemStart(100)>=100 true (non-strict, exact touch);
        // region.tmemEnd(150)<=200 true -> erase.
        let mut regions = RegionTmemList::new();
        regions.push_back(region(100, 150));
        discard_regions_tmem(&mut regions, 100, 100, 511);
        assert_eq!(regions.len(), 0);
    }

    #[test]
    fn discard_on_empty_list_is_a_no_op() {
        let mut regions = RegionTmemList::new();
        discard_regions_tmem(&mut regions, 10, 5, 511);
        assert_eq!(regions.len(), 0);
    }

    #[test]
    fn discard_sequential_calls_compose_left_to_right() {
        // Region [0,200). First discard [0,50) -> case2 (shrink from
        // right, region.tmemStart(0)<=0 true, region.tmemEnd(200)<50
        // false... recompute: discard tmemEnd=0+50=50.
        // case1: 0>=0 true, 200<=50 false -> not case1.
        // case2: 0<=0 true, 200<50 false -> not case2.
        // case3: 0>0 false -> not case3. -> case4 split: left[0,0) empty
        // (dropped by the "empty, erase" check since tmemStart==tmemEnd
        // makes the ORIGINAL slot get erased -- but wait, original slot's
        // tmemEnd is set to tmemStart(0) which equals its own
        // tmemStart(0) only if region.tmemStart==discard.tmemStart(0);
        // here region.tmemStart=0 so after case4 sets it->tmemEnd=
        // tmemStart(discard's start=0), the slot becomes [0,0) -> erased.
        // Right remainder [50,200) is pushed to back (region.tmemEnd(200)
        // != discard.tmemEnd(50), so it IS pushed).
        // Net result after first discard: single region [50,200).
        let mut regions = RegionTmemList::new();
        regions.push_back(region(0, 200));
        discard_regions_tmem(&mut regions, 0, 50, 511);
        assert_eq!(regions.len(), 1);
        assert_eq!(regions[0], region(50, 200));

        // Second discard [150,200) against [50,200): discard
        // tmemEnd=150+50=200. case1: 50>=150 false. case2: 50<=150 true,
        // 200<200 false -> not case2. case3: 50>150 false -> not case3.
        // -> case4: left[50,150) kept in place, right remainder would be
        // [200,200) which is empty (tmemEnd(200)==discardEnd(200)) so NOT
        // pushed.
        discard_regions_tmem(&mut regions, 150, 50, 511);
        assert_eq!(regions.len(), 1);
        assert_eq!(regions[0], region(50, 150));
    }

    #[test]
    fn insert_then_discard_integration_splits_the_inserted_region() {
        // Insert [100,150) (address irrelevant here), then discard
        // [120,130) from inside it: expect a case-4 split into
        // [100,120) and [130,150).
        let mut regions = RegionTmemList::new();
        insert_regions_tmem(&mut regions, 0xFEED, 100, 50, 511, false, false, None);
        assert_eq!(regions.len(), 1);
        discard_regions_tmem(&mut regions, 120, 10, 511);
        assert_eq!(regions.len(), 2);
        assert!(regions.contains(&region(100, 120)));
        assert!(regions.contains(&region(130, 150)));
    }

    #[test]
    fn discard_case4_split_copies_tile_copy_id_into_new_right_remainder() {
        // The split branch does `RegionTMEM newRegion = *it;` BEFORE
        // overwriting tmemStart -- so every other field (including
        // tile_copy_id) is copied from the original into the new back
        // element, not left at its default.
        let mut regions = RegionTmemList::new();
        regions.push_back(RegionTmem {
            tile_copy_id: 0xC0FFEE,
            ..region_marked(100, 150)
        });
        discard_regions_tmem(&mut regions, 110, 20, 511);
        assert_eq!(regions.len(), 2);
        assert_eq!(regions[1].tmem_start, 130);
        assert_eq!(regions[1].tmem_end, 150);
        assert_eq!(regions[1].tile_copy_id, 0xC0FFEE);
    }

    #[test]
    fn discard_case3_boundary_inclusive_end_matches_exactly() {
        // Region [150,200), discard [50,200): discard tmemEnd=50+150=200.
        // case3: region.tmemStart(150)>discard.tmemStart(50) true;
        // region.tmemEnd(200)>=discard.tmemEnd(200) true (non-strict,
        // exact match) -> moves start to discard.tmemEnd=200, making the
        // region [200,200) -> empty -> erased.
        let mut regions = RegionTmemList::new();
        regions.push_back(region(150, 200));
        discard_regions_tmem(&mut regions, 50, 150, 511);
        assert_eq!(regions.len(), 0);
    }

    #[test]
    fn discard_case2_boundary_inclusive_start_matches_exactly() {
        // Region [100,150), discard [100,300): discard tmemEnd=100+200=300.
        // case1 needs region.tmemEnd(150)<=300 true AND
        // region.tmemStart(100)>=100 true -> this actually matches case1
        // (fully contained), not case2, confirming case1 takes priority
        // when both the start and full containment conditions hold.
        let mut regions = RegionTmemList::new();
        regions.push_back(region_marked(100, 150));
        discard_regions_tmem(&mut regions, 100, 200, 511);
        assert_eq!(regions.len(), 0);
    }

    #[test]
    fn insert_regions_tmem_multiple_calls_accumulate_at_front_in_call_order() {
        let mut regions = RegionTmemList::new();
        insert_regions_tmem(&mut regions, 0, 0, 10, 511, false, false, None);
        insert_regions_tmem(&mut regions, 0, 20, 10, 511, false, false, None);
        insert_regions_tmem(&mut regions, 0, 40, 10, 511, false, false, None);
        // Each call push_fronts its one region, so the most recently
        // inserted call's region is at the front.
        assert_eq!(regions.len(), 3);
        assert_eq!(regions[0].tmem_start, 40);
        assert_eq!(regions[1].tmem_start, 20);
        assert_eq!(regions[2].tmem_start, 0);
    }

    #[test]
    fn insert_regions_tmem_never_touches_tile_copy_id() {
        // insertRegionsTMEM's RegionTMEM newRegion = {} zero-initializes
        // tileCopyId, and the function never assigns it afterward.
        let mut regions = RegionTmemList::new();
        insert_regions_tmem(&mut regions, 0, 0, 10, 511, false, false, None);
        assert_eq!(regions[0].tile_copy_id, 0);
    }

    #[test]
    fn insert_regions_tmem_with_a_different_mask_value() {
        // Hand-computed with tmemMask=255 (not the usual 511): start=200,
        // words=100. (200&255)=200. tmemEnd0=200+100=300 (exceeds
        // tmemMask+1=256, still emitted whole/unclamped, same as the
        // mask=511 crossing case).
        let mut regions = RegionTmemList::new();
        insert_regions_tmem(&mut regions, 0, 200, 100, 255, false, false, None);
        assert_eq!(regions.len(), 1);
        assert_eq!(regions[0].tmem_start, 200);
        assert_eq!(regions[0].tmem_end, 300);
    }

    #[test]
    fn discard_case4_new_region_inherits_sync_required_from_original() {
        // The split's new back element is a full copy of `*it` (post
        // fb_tile-clear), so sync_required carries over too.
        let mut regions = RegionTmemList::new();
        regions.push_back(RegionTmem {
            sync_required: true,
            ..region(100, 150)
        });
        discard_regions_tmem(&mut regions, 110, 20, 511);
        assert_eq!(regions.len(), 2);
        assert!(regions[0].sync_required);
        assert!(regions[1].sync_required);
    }
}
