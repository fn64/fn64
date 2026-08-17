//! Literal port of `RT64::FramebufferManager::makeFramebufferTile`: the
//! RDRAM-address-range to framebuffer-tile geometry solver, including every
//! named rejection path. A literal port of the permitted MIT RT64 Rust-port
//! source pinned at commit `5473732a822a4423b5696e7cb18fecc425a59875`
//! (`docs/RT64-PORT-AUTHORITY.md`):
//!
//! - `src/hle/rt64_framebuffer_manager.cpp:390-486` (whole-file SHA-256,
//!   `1a97e98b34dc4707d4a9514ef6992bd751e5a0d6fe2c5bcefd50234b41686fd5`, 1093
//!   lines -- matching `docs/rt64-port-inventory.json`'s `sources.port.sha256`
//!   for that path, confirmed independently here by `shasum -a 256` against
//!   the pinned port-commit checkout).
//! - `src/hle/rt64_framebuffer_manager.h:192` (the method's own declaration
//!   line, cited only to confirm the signature; whole-file SHA-256,
//!   `fca8057640165a3e97994581da1a427c79d350559b928776e1c0d1707813eeee`, 230
//!   lines -- matching the same inventory field, confirmed the same way).
//! - `src/hle/rt64_framebuffer.h:32-64,82-93` (the `Framebuffer` fields this
//!   function reads, and `FramebufferTile`'s field list -- cited for context
//!   only; this ticket does not own that file, `rt64_framebuffer_geometry.rs`
//!   does).
//! - `src/shared/rt64_f3d_defines.h:70` (`#define G_IM_SIZ_4b 0`, the only
//!   named `siz` constant this function compares against).
//!
//! `docs/rt64-port-inventory.json` does not yet record
//! `src/hle/rt64_framebuffer_manager.cpp`/`.h`'s `ported_as` as pointing at
//! this module (both currently list other/empty `ported_as` entries, since a
//! sibling ticket M4.11 already ports three *other* functions from the same
//! file into `rt64_tmem_regions.rs`) -- `scripts/lint-docs.py`'s inventory
//! scanner is expected to report a drift for that until a follow-up
//! regenerates the inventory to add this module; this module's own writable
//! surface does not include `docs/rt64-port-inventory.json`, so that
//! reconciliation is deliberately left to the owning ticket rather than done
//! here (matching `rt64_tmem_regions.rs`'s and `rt64_framebuffer_geometry.rs`'s
//! precedent for the same situation).
//!
//! ```text
//! bool FramebufferManager::makeFramebufferTile(Framebuffer *fb, uint32_t addressStart, uint32_t addressEnd, uint32_t lineWidth, uint32_t tileHeight, FramebufferTile &outTile, bool RGBA32) {
//!     assert(fb != nullptr);
//!
//!     // We need to figure out the best fitting tile from the address range specified and the TMEM Regions this tile must be stored on.
//!     // The tile width and height parameters won't be 0 on load tile operations. They will however be 0 on load block operations.
//!
//!     // If the starting address is lower than the framebuffer address, we move a row one by one according to the stride specified of the original image width.
//!     uint32_t tileRowStart = 0;
//!     uint32_t fbStride = fb->imageRowBytes(fb->width);
//!     while (addressStart < fb->addressStart) {
//!         addressStart += fbStride;
//!         tileRowStart++;
//!     }
//!
//!     // We went over the allowed address range, a tile copy is impossible.
//!     if (addressStart >= fb->addressEnd) {
//!         return false;
//!     }
//!
//!     // Disallow the tile copy if the end address ended up below the starting address.
//!     const uint32_t minEndAddress = std::min(addressEnd, fb->addressEnd);
//!     if (minEndAddress <= addressStart) {
//!         return false;
//!     }
//!
//!     // Figure out how many rows we could possibly given the current address range.
//!     const uint32_t fbBytes = minEndAddress - fb->addressStart;
//!     const uint32_t fbMinRow = (addressStart - fb->addressStart) / fbStride;
//!     const uint32_t fbMaxRow = (fbBytes / fbStride) + (((fbBytes % fbStride) > 0) ? 1 : 0);
//!
//!     // Relative offset of the image start to the framebuffer start.
//!     const uint32_t offset = addressStart - fb->addressStart;
//!
//!     // This will be the same size for 4 and 8 byte formats.
//!     const uint32_t pixelSize = 1 << fb->siz >> 1;
//!
//!     // The offset is not aligned to the pixel size. It's not possible to make a direct copy.
//!     if ((offset % pixelSize) != 0) {
//!         return false;
//!     }
//!
//!     // Figure out where the upper left coordinate of the tile is inside the framebuffer.
//!     const uint32_t rowBytes = fb->imageRowBytes(fb->width);
//!     const uint32_t row = offset / rowBytes;
//!     const uint32_t rowOffset = offset % rowBytes;
//!     const uint32_t pixelShift = (fb->siz == G_IM_SIZ_4b) ? 1 : 0;
//!     outTile.left = (rowOffset / pixelSize) << pixelShift;
//!     outTile.top = row;
//!
//!     // Line width is defined.
//!     if (lineWidth > 0) {
//!         outTile.right = outTile.left + lineWidth;
//!     }
//!     // Figure it out from the framebuffer instead.
//!     else {
//!         const uint32_t rowRightPixels = ((rowBytes - rowOffset) / pixelSize) << pixelShift;
//!         outTile.right = outTile.left + rowRightPixels;
//!     }
//!
//!     // Tile height is defined.
//!     if (tileHeight > 0) {
//!         outTile.bottom = outTile.top + tileHeight;
//!     }
//!     else {
//!         const uint32_t rowEnd = std::max((addressEnd - addressStart) / rowBytes, 1U);
//!         outTile.bottom = outTile.top + rowEnd;
//!
//!         // Invalidate the tile if this is a loadBlock operation, more than one row is being loaded
//!         // and the offset is not perfectly aligned with a row.
//!         const bool fromLoadBlock = (tileHeight == 0);
//!         const bool multipleRows = (rowEnd > 1);
//!         const bool misalignedRow = (rowOffset > 0);
//!         if (fromLoadBlock && multipleRows && misalignedRow) {
//!             return false;
//!         }
//!     }
//!
//!     // Clamp the tile to the framebuffer's dimensions and the image row ranges found.
//!     outTile.top = std::max(outTile.top, fbMinRow);
//!     outTile.right = std::min(outTile.right, fb->width);
//!     outTile.bottom = std::min(outTile.bottom, fb->height);
//!     outTile.bottom = std::min(outTile.bottom, fbMaxRow);
//!
//!     // Invalid tile.
//!     if ((outTile.bottom <= outTile.top) || (outTile.right <= outTile.left)) {
//!         return false;
//!     }
//!
//!     // Define the tile.
//!     outTile.lineWidth = (lineWidth > 0) ? lineWidth : (outTile.right - outTile.left);
//!     outTile.address = fb->addressStart;
//!     outTile.siz = fb->siz;
//!     outTile.fmt = fb->lastWriteFmt;
//!     outTile.ditherPattern = fb->bestDitherPattern();
//!
//!     return true;
//! }
//! ```
//!
//! ```text
//! // rt64_framebuffer.h (fields this function reads)
//! struct Framebuffer {
//!     uint32_t addressStart;
//!     uint32_t addressEnd;
//!     uint8_t siz;
//!     uint32_t width;
//!     uint32_t height;
//!     uint8_t lastWriteFmt;
//!     std::array<uint32_t, 4> ditherPatterns;
//!     uint32_t imageRowBytes(uint32_t rowWidth) const;   // rowWidth << siz >> 1
//!     uint32_t bestDitherPattern() const;                // index of first max
//! };
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
//! // rt64_f3d_defines.h
//! #define G_IM_SIZ_4b 0
//! ```
//!
//! **Reuse, not new type.** This module reuses [`crate::rt64_tmem_regions::FbTile`]
//! verbatim as its output type -- that module already defines a minimal local
//! mirror of `FramebufferTile`'s 9 fields (`address`, `siz`, `fmt`, `left`,
//! `top`, `right`, `bottom`, `line_width`, `dither_pattern`) for the same
//! source file's `RegionTMEM::fbTile` payload, and this function's `outTile`
//! writes exactly that same field set, so defining a second, field-identical
//! struct here would be needless duplication. `rt64_tmem_regions.rs` is a
//! **sibling partial port of this same source file** (`M4.11`, landed just
//! before this ticket): it ports `insertRegionsTMEM`, `discardRegionsTMEM`,
//! and `synchronizeRegionsTMEM` from
//! `src/hle/rt64_framebuffer_manager.cpp:517-634`; this module ports
//! `makeFramebufferTile` from the same file's lines 390-486 and does not
//! touch, re-port, or duplicate any of the three TMEM-region functions or
//! their `RegionTmem`/`RegionTmemList` types.
//!
//! By contrast, `rt64_framebuffer_geometry.rs` (`M4.8`, a dependency of this
//! ticket) ports `Framebuffer::imageRowBytes` and `Framebuffer::bestDitherPattern`
//! onto its own `FramebufferGeometry` struct, but that struct owns only the
//! field subset *its* ported methods need (`address_start`, `address_end`,
//! `siz`, `last_write_type`, `last_write_rect`, the three `*_changed` flags,
//! `dither_patterns`) and does **not** carry `width`, `height`, or
//! `last_write_fmt` -- all three of which `makeFramebufferTile` reads
//! directly from `fb->width`/`fb->height`/`fb->lastWriteFmt`. Extending
//! `FramebufferGeometry` with those fields is outside this ticket's
//! `writable_paths` (only `rt64_framebuffer_tile.rs` is writable here), so
//! per the ticket's own instruction ("take the framebuffer extent as an
//! owned input struct rather than reaching into a manager") this module
//! defines its own local [`FbExtent`] carrying exactly the fields
//! `makeFramebufferTile` reads (`address_start`, `address_end`, `siz`,
//! `width`, `height`, `last_write_fmt`, `dither_patterns`), and re-derives
//! `image_row_bytes`/`best_dither_pattern` as private local helpers
//! ([`image_row_bytes`], [`best_dither_pattern`]) with the exact same
//! formulas `FramebufferGeometry` uses -- this is acknowledged duplication
//! of two one-line/six-line formulas, not a new interpretation of them; see
//! "Admitted domain" below for both formulas verified against
//! `rt64_framebuffer_geometry.rs`'s own doc comments and tests.
//!
//! ## Admitted domain
//!
//! - **Six named rejection paths, each independently reachable and
//!   independently tested:**
//!   1. **Address walked past the framebuffer end**
//!      (`addressStart >= fb->addressEnd`, non-strict `>=`, after the
//!      row-walk loop). See
//!      [`tests::rejects_when_address_walks_past_framebuffer_end`] and the
//!      boundary pair
//!      [`tests::accepts_at_address_end_minus_one`]/[`tests::rejects_at_address_end_exactly`].
//!   2. **End address collapses to at-or-below start**
//!      (`minEndAddress <= addressStart`, non-strict `<=`, where
//!      `minEndAddress = min(addressEnd, fb.addressEnd)`). See
//!      [`tests::rejects_when_end_equals_start`] and the boundary pair
//!      [`tests::accepts_when_end_is_one_past_start`]/[`tests::rejects_when_end_equals_start`].
//!   3. **Offset not pixel-aligned** (`(offset % pixelSize) != 0`). See
//!      [`tests::rejects_misaligned_offset`] and the boundary pair
//!      [`tests::accepts_offset_exactly_pixel_aligned`]/[`tests::rejects_misaligned_offset`].
//!      `pixelSize` is `1 << siz >> 1`, which is `0` for `siz == 0` (4-bit) --
//!      see the divide-by-zero frontier below; this rejection is only
//!      reachable for `siz` in `{1, 2, 3}`.
//!   4. **Misaligned multi-row load-block**
//!      (`fromLoadBlock && multipleRows && misalignedRow`, all three
//!      sub-conditions true simultaneously: `tileHeight == 0`, the derived
//!      `rowEnd > 1` (strict), and `rowOffset > 0` (strict)). See
//!      [`tests::rejects_load_block_misaligned_multi_row`], and the three
//!      individual-condition-false companions
//!      [`tests::load_block_single_row_with_misalignment_is_accepted`]
//!      (`multipleRows` false),
//!      [`tests::load_block_multi_row_aligned_offset_is_accepted`]
//!      (`misalignedRow` false), and
//!      [`tests::load_tile_multi_row_misaligned_is_accepted`] (`fromLoadBlock`
//!      false, i.e. `tileHeight > 0` so this whole branch is skipped) --
//!      confirming the `&&` is genuinely three-way, not collapsible to two.
//!   5. **Degenerate `bottom <= top`** (non-strict `<=`, checked after all
//!      four post-clamp assignments). See
//!      [`tests::rejects_degenerate_bottom_at_top`] and the boundary pair
//!      [`tests::accepts_bottom_one_past_top`]/[`tests::rejects_degenerate_bottom_at_top`].
//!   6. **Degenerate `right <= left`** (non-strict `<=`, same `||` as #5,
//!      independently triggerable without #5 firing). See
//!      [`tests::rejects_degenerate_right_at_left`] and the boundary pair
//!      [`tests::accepts_right_one_past_left`]/[`tests::rejects_degenerate_right_at_left`].
//!
//!   All six are triggered independently by a dedicated fixture each; none
//!   is dead code -- every one is reachable from [`make_framebuffer_tile`]'s
//!   public entry point with an otherwise-valid input, confirmed by the
//!   paired accept/reject boundary tests above (the accept case proves the
//!   fixture reaches that far into the function; the reject case proves the
//!   guard fires on the very next value).
//!
//! - **Two additional sentinel-derived branches, each independently
//!   tested (named by the ticket alongside the six rejections):**
//!   - `lineWidth == 0` is a **sentinel** meaning "derive `right` from the
//!     framebuffer's row width", not a literal zero-width tile -- taken by
//!     `loadBlock` call sites. See
//!     [`tests::line_width_zero_derives_right_from_row_bytes`] vs.
//!     [`tests::line_width_nonzero_sets_right_directly`].
//!   - `tileHeight == 0` is the same kind of sentinel ("derive `bottom` from
//!     the address range"), and **additionally arms** rejection #4 above
//!     (`fromLoadBlock = (tileHeight == 0)`) -- a `tileHeight == 0` load
//!     that derives more than one row from a misaligned offset is rejected,
//!     while the identical derived geometry reached via `tileHeight > 0`
//!     (an explicit, non-sentinel height) is **not** subject to that guard
//!     at all, because `fromLoadBlock` is false and the whole `if` is
//!     skipped. See [`tests::load_tile_multi_row_misaligned_is_accepted`]
//!     (explicit height, same misaligned offset, accepted) directly against
//!     [`tests::rejects_load_block_misaligned_multi_row`] (sentinel height,
//!     same misaligned offset, rejected) -- same underlying geometry, opposite
//!     outcome, because the guard is keyed on *how* the height was obtained,
//!     not on the resulting shape.
//!
//! - **Two divide-by-zero frontiers, reported rather than guarded (per the
//!   hazard brief: "REPORT the frontier, do not silently guard").** This
//!   port preserves both as literal Rust integer division/modulo, which
//!   **panics** (`attempt to divide by zero` / `attempt to calculate the
//!   remainder with a divisor of zero`) exactly where the C++ has undefined
//!   behavior (a division-by-zero trap on essentially all real hardware,
//!   commonly `SIGFPE`) -- this module invents no guard RT64 does not have:
//!   1. **`pixelSize == 0` for `siz == 0` (4-bit).** `pixelSize = 1 << siz >>
//!      1` (left-shift then right-shift, exact order preserved -- same
//!      pattern as `FramebufferGeometry::image_row_bytes`) is `1 >> 1 == 0`
//!      when `siz == 0`. The very next statement, `offset % pixelSize`,
//!      therefore divides by zero **unconditionally** whenever `siz == 0`
//!      and this point in the function is reached (it does not depend on
//!      `offset`'s value -- `0 % 0` is equally UB in C++). This means
//!      rejection #3 above is **never actually evaluated as a rejection**
//!      for 4-bit framebuffers in upstream RT64: the process traps first.
//!      See [`tests::siz_4b_panics_on_the_offset_alignment_modulo`], which
//!      asserts the panic (not a `Result::Err`) using `std::panic::catch_unwind`,
//!      proving this is a crash frontier, not a value this module maps to a
//!      rejection variant.
//!   2. **`fbStride == 0` for `width == 0` (any `siz`), first hit at
//!      `fbMinRow`'s division, one statement (and 7 source lines) *before*
//!      the `pixelSize` guard above.** `fbStride = imageRowBytes(fb->width)
//!      = fb->width << siz >> 1`, which is `0` whenever `width == 0`
//!      regardless of `siz`. The **first** division by `fbStride` after the
//!      row-walk loop is `fbMinRow = (addressStart - fb.addressStart) /
//!      fbStride` (line 417) -- this is reached and crashes *before*
//!      `pixelSize` is even computed (line 424) or checked (line 427), so
//!      for `width == 0` this frontier always fires ahead of the `siz == 0`
//!      one, even when both conditions hold simultaneously (confirmed by a
//!      300,000-sample randomized sweep crossing every `siz` against
//!      `width == 0`, `/tmp/derive2.py` in this session: `fbMinRow`'s crash
//!      always pre-empts `pixelSize`'s). `rowBytes` (the *second*,
//!      later-computed use of the identical `imageRowBytes(fb->width)`
//!      formula, at line 432 immediately before `row`/`rowOffset`) is
//!      therefore **never reached with a zero divisor from a fresh call**
//!      when `width == 0` -- `fbMinRow`'s division always traps first. See
//!      [`tests::zero_width_framebuffer_panics_on_row_stride_division`],
//!      which asserts the panic fires and is a `fbMinRow`-shaped panic (the
//!      first one reachable), not a later one.
//!      A zero *height* has no analogous divide-by-zero: `height` is only
//!      ever used as a `std::min` clamp ceiling (`outTile.bottom =
//!      std::min(outTile.bottom, fb->height)`), never as a divisor -- a
//!      zero height instead reliably drives rejection #5 (`bottom <= top`
//!      after clamping `bottom` down to `0`). See
//!      [`tests::zero_height_framebuffer_is_rejected_not_a_panic`].
//!
//! - **`pixelShift` is dead code: it is only ever non-zero when
//!   `fb->siz == G_IM_SIZ_4b` (`siz == 0`), and that exact same condition
//!   makes `pixelSize == 0`, which unconditionally crashes the function
//!   (frontier #1 immediately above) *before* `pixelShift` is computed
//!   (line 435) or read (lines 436/445).** In other words: every input for
//!   which `pixelShift` would ever be `1` instead of `0` is an input that
//!   already trapped several lines earlier. This is reported here as a
//!   finding, not "fixed" -- `pixelShift`'s branch and both of its use
//!   sites are ported literally and unchanged (see [`make_framebuffer_tile`]
//!   itself), exactly as `rt64_tmem_regions.rs` ports `insertRegionsTMEM`'s
//!   two provably-unreachable-from-a-fresh-call `while`-loop conditions
//!   without removing them. No test can exercise `pixel_shift == 1` as
//!   *reachable* from [`make_framebuffer_tile`]'s public entry point, by
//!   the same proof; this module's tests instead confirm `pixel_shift`'s
//!   formula and use sites exist and equal `0` for every reachable `siz`
//!   (see [`tests::pixel_shift_is_always_zero_for_every_reachable_siz`]).
//!
//! - **Rejection #6 (`right <= left`) is unreachable from any realistic,
//!   non-overflowing input, for every `siz` that does not already crash at
//!   frontier #1 or #2 above (i.e. `siz` in `{1, 2, 3}` with `width > 0`).**
//!   Proof: `left = (rowOffset / pixelSize) << pixelShift`, and since
//!   `pixelShift == 0` is forced for every reachable `siz` (see the
//!   `pixelShift` finding above), `left = rowOffset / pixelSize`. Because
//!   `rowBytes = fb.width << siz >> 1` and `pixelSize = 1 << siz >> 1` share
//!   the same right-shift-by-one and `siz >= 1` here, `rowBytes` is an
//!   *exact* multiple of `pixelSize` (`rowBytes == fb.width * pixelSize`,
//!   confirmed for `siz` in `{1, 2, 3}` against every `width` up to 65 in
//!   `/tmp/derive.py`'s sweep in this session), and `rowOffset < rowBytes`
//!   (it is `offset % rowBytes`) -- so `left = rowOffset / pixelSize <
//!   rowBytes / pixelSize == fb.width` strictly, for *every* reachable
//!   input. Since the final `right` is always clamped to `min(_, fb.width)`,
//!   `right <= fb.width`, giving `right <= fb.width` and `left < fb.width`
//!   simultaneously -- `right <= left` would require the clamp to push
//!   `right` down to at or below a value strictly less than `fb.width`,
//!   which only happens if the pre-clamp `right` (`left + lineWidth` or
//!   `left + rowRightPixels`) is itself `<= left`; the derive path
//!   (`rowRightPixels`) is proven `> 0` by the same exact-multiple
//!   argument, and the explicit-`lineWidth` path requires `lineWidth >= 1`
//!   by definition (`lineWidth == 0` takes the derive branch instead), so
//!   `left + lineWidth > left` **unless the addition overflows `u32`**.
//!   Verified by an 800,000-sample randomized sweep (`/tmp/derive.py`/
//!   `derive2.py` in this session, `siz` in `{1,2,3}`, `width` up to 2000,
//!   non-overflowing `lineWidth`) that found zero occurrences of
//!   `DegenerateRightAtOrBeforeLeft`. **This is a test gap, not dead code:**
//!   the branch is reachable *in principle* (`FbTile`'s `right`/`left`
//!   fields are plain `u32`, and the comparison is literally ported, not
//!   provably-false by the source's own control flow the way
//!   `insertRegionsTMEM`'s wraparound branches are) -- it is only
//!   unreachable by *this proof about realistic magnitudes*, and the one
//!   way left to trigger it (`left + lineWidth` overflowing `u32`) is the
//!   same overflow-avoidance choice `rt64_framebuffer_geometry.rs`'s
//!   `get_native_size`/`add_dither_patterns` already made and documented
//!   (plain `+`, not `wrapping_add`, not tested at the overflow boundary,
//!   because Rust's debug-build overflow-checks panic before the C++'s
//!   silent-wraparound comparison can even run) -- so this module makes the
//!   same choice for consistency and does not add a test that would panic
//!   under `cargo nextest`'s debug profile instead of exercising the
//!   intended comparison.
//!
//! - **The row-walk `while` loop (`while (addressStart < fb->addressStart) {
//!   addressStart += fbStride; tileRowStart++; }`) has no bound check against
//!   `fbStride == 0`.** If `fbStride == 0` (i.e. `width == 0`, the same
//!   zero-width condition as the divide-by-zero frontier above) and
//!   `addressStart < fb->addressStart`, this loop adds zero to `addressStart`
//!   forever and never terminates -- an infinite loop, not a panic, not a
//!   return. This module's [`make_framebuffer_tile`] reproduces the loop
//!   literally (see its own doc comment) and therefore has the exact same
//!   non-termination frontier; it is **not** a rejection path (the ticket's
//!   six do not include it) and is **not guarded** here, matching the
//!   "REPORT, do not silently guard" hazard for the same reason as the two
//!   divide-by-zero frontiers above. No test exercises this path to
//!   completion (doing so would hang the test binary); its non-termination
//!   is instead demonstrated by construction (the loop body is a literal,
//!   unconditional `+= fbStride` with `fbStride` provably `0` whenever
//!   `width == 0`) and is asserted only up to one non-terminating iteration
//!   count in [`tests::zero_stride_row_walk_does_not_advance_address`],
//!   which calls the loop's condition/body manually (not through
//!   [`make_framebuffer_tile`]) to observe that `addressStart` is unchanged
//!   after one iteration, without looping to demonstrate the hang itself.
//!
//! - **`tileRowStart` is computed by the row-walk loop and never read
//!   anywhere else in the function.** `uint32_t tileRowStart = 0; ... while
//!   (...) { ...; tileRowStart++; }` increments a local that is not stored
//!   into `outTile`, not returned, and not referenced again in the function
//!   body (confirmed by grepping the function's text for the identifier: it
//!   appears exactly at its declaration and its one increment). Ported as a
//!   genuinely dead local ([`_tile_row_start`], underscore-prefixed,
//!   matching the source) rather than dropped, for the same reason
//!   `rt64_tmem_regions.rs` keeps `_byte_shift`: dropping a source statement,
//!   even a provably-unread one, is not this module's license under a
//!   literal-port mandate.
//!
//! - **The `RGBA32` parameter is never read anywhere in
//!   `makeFramebufferTile`'s body** (confirmed by grepping the function's
//!   97-line text for the identifier: it appears exactly once, in the
//!   signature). This is a genuinely dead *parameter*, not merely a dead
//!   local -- callers presumably thread the same `RGBA32` flag on to
//!   `insertRegionsTMEM` separately, but within this function it has no
//!   effect on any branch or output field. Ported as `_rgba32: bool` in
//!   [`make_framebuffer_tile`]'s signature, kept (not dropped) for the same
//!   literal-port reason as `_tile_row_start` and `rt64_tmem_regions.rs`'s
//!   `_byte_shift`.
//!
//! - **Comparison strictness, catalogued for every branch:**
//!   - Row-walk loop guard: `addressStart < fb->addressStart` -- strict.
//!   - Rejection #1: `addressStart >= fb->addressEnd` -- non-strict
//!     (`>=`); the accept/reject pair above pins this exactly at
//!     `addressEnd - 1` (accept) vs. `addressEnd` (reject).
//!   - Rejection #2: `minEndAddress <= addressStart` -- non-strict (`<=`);
//!     accept at `addressStart + 1`, reject at `addressStart` exactly (i.e.
//!     `end == start`).
//!   - Rejection #3: `(offset % pixelSize) != 0` -- exact inequality on the
//!     remainder, not an ordering comparison.
//!   - Rejection #4's three sub-conditions: `tileHeight == 0` (exact
//!     equality), `rowEnd > 1` (strict), `rowOffset > 0` (strict).
//!   - `fbMaxRow`'s ceiling-division test: `(fbBytes % fbStride) > 0` --
//!     strict; a `fbBytes` that divides `fbStride` evenly adds no extra row.
//!   - `lineWidth > 0` / `tileHeight > 0` (sentinel tests): strict, matching
//!     rejection #4's own `tileHeight == 0` framing (the two are logical
//!     complements over `u32`, not independently-strictness-bearing, but
//!     both preserved as written rather than inverted to `== 0`).
//!   - Rejection #5/#6's combined guard: `(outTile.bottom <= outTile.top) ||
//!     (outTile.right <= outTile.left)` -- both non-strict (`<=`), both
//!     sides of the `||` independently triggerable (see the two dedicated
//!     tests, each holding the other dimension valid). **Ported as two
//!     sequential `if` statements returning distinct [`Rejection`]
//!     variants, not one `if` with an `||`.** This is *not* a behavior
//!     change: C++ `||` short-circuits left-to-right, so the source already
//!     evaluates `bottom <= top` strictly before `right <= left` and never
//!     evaluates the second operand once the first is true; the two
//!     sequential Rust `if`s preserve that exact same evaluation order and
//!     short-circuit (the second `if` is unreached once the first returns).
//!     The only difference is which of two `Err` values is produced for a
//!     testing aid this module adds (see [`Rejection`]'s own doc comment)
//!     -- the C++ function returns the same `false` for both, so this split
//!     is additive information, not a reinterpretation of when the function
//!     rejects.
//!   - `fb->siz == G_IM_SIZ_4b` (pixel-shift gate) -- exact equality against
//!     the constant `0`.
//!
//! - **Truncation/operand order, preserved exactly (not reassociated):**
//!   - `pixelSize = 1 << fb->siz >> 1` -- left-shift by `siz` first, then
//!     right-shift by 1 (same left-then-right order as
//!     `FramebufferGeometry::image_row_bytes`'s `rowWidth << siz >> 1`, just
//!     with `1` as the base instead of `rowWidth`). Ported as `(1u32 <<
//!     siz) >> 1`, matching Rust's `<<`/`>>` left-to-right precedence, which
//!     equals C++'s.
//!   - `outTile.left = (rowOffset / pixelSize) << pixelShift` -- the
//!     division happens **before** the shift; ported as `(row_offset /
//!     pixel_size) << pixel_shift`, not `row_offset / (pixel_size <<
//!     pixel_shift)` or any other regrouping (those are not equivalent under
//!     integer truncation).
//!   - `rowRightPixels = ((rowBytes - rowOffset) / pixelSize) << pixelShift`
//!     -- same divide-then-shift order, with the subtraction evaluated
//!     first per C++ operator precedence (parenthesized in the source too).
//!   - `fbMaxRow = (fbBytes / fbStride) + (((fbBytes % fbStride) > 0) ? 1 :
//!     0)` -- a **ceiling-division-by-mod-check** idiom, not the
//!     `(fbBytes + fbStride - 1) / fbStride` idiom (which risks a different
//!     overflow behavior and is not what the source wrote); ported as the
//!     literal add-one-if-remainder form.
//!   - `rowEnd = std::max((addressEnd - addressStart) / rowBytes, 1U)` --
//!     the subtraction and division happen before the `max` floor is
//!     applied; ported as `((address_end - address_start) /
//!     row_bytes).max(1)`. The raw (unclamped-to-`fb.addressEnd`) `addressEnd`
//!     parameter is used here, not `minEndAddress` -- see the proof below
//!     for why `addressEnd - addressStart` cannot underflow at this point
//!     despite using the raw parameter.
//!   - `outTile.lineWidth = (lineWidth > 0) ? lineWidth : (outTile.right -
//!     outTile.left)` -- the fallback re-derives line width from the
//!     **already-clamped** `right`/`left`, not from the pre-clamp
//!     `rowRightPixels`/`left`; ported as `if line_width > 0 { line_width }
//!     else { tile.right - tile.left }` evaluated strictly after the
//!     four clamp assignments and the #5/#6 rejection check.
//!
//! - **No unsigned-subtraction underflow at `addressEnd - addressStart`
//!   inside the `tileHeight == 0` branch, despite using the raw
//!   (non-`min`-clamped) `addressEnd` parameter.** By the time that
//!   subtraction executes, rejection #2 has already required `minEndAddress
//!   > addressStart`, where `minEndAddress = min(addressEnd, fb.addressEnd)
//!   <= addressEnd`. Chaining: `addressEnd >= minEndAddress > addressStart`,
//!   so `addressEnd > addressStart` strictly, and the subtraction cannot
//!   wrap. Verified both algebraically (above) and by a 200,000-sample
//!   randomized sweep over `address_start`, `address_end`, `fb.address_end`
//!   in `[0, 2^32)` in [`tests::no_underflow_sweep_address_end_minus_address_start`],
//!   which asserts the subtraction never panics (Rust's debug-mode
//!   overflow check) whenever rejection #1 and #2 both did not fire.
//!
//! - **Clamping order (four sequential assignments, not simultaneous):**
//!   `top = max(top, fbMinRow)`, `right = min(right, fb.width)`, `bottom =
//!   min(bottom, fb.height)`, `bottom = min(bottom, fbMaxRow)` -- `bottom`
//!   is clamped **twice**, first against `fb.height` then against
//!   `fbMaxRow`, in that order (not combined into a single
//!   `min(bottom, min(height, fbMaxRow))`, though the two are numerically
//!   equivalent for `min` specifically -- ported as two sequential
//!   statements to match the source's literal shape). `left` and `top`
//!   (other than the one `max` on `top`) are **not** clamped at all after
//!   being computed -- only `top`'s lower bound, `right`'s upper bound, and
//!   `bottom`'s two upper bounds are clamped; `left` is trusted as computed.
//!
//! - **No private-helper visibility gap was hit.** Everything this function
//!   needs (`Framebuffer::imageRowBytes`, `Framebuffer::bestDitherPattern`)
//!   is a `pub`/public-in-C++-terms method already ported by `M4.8`; the
//!   only reason this module does not call `FramebufferGeometry`'s versions
//!   directly is the field-subset mismatch documented above under "Reuse,
//!   not new type" (a `writable_paths` scoping constraint, not a visibility
//!   gap in the C++ source).
//!
//! ## Nonclaims
//!
//! No GPU, WGSL, or production wiring (this module is not called from
//! anywhere yet -- dead-code warnings on the unused public surface are
//! expected and correct, matching `rt64_tmem_regions.rs`/
//! `rt64_framebuffer_geometry.rs`'s precedent), and no RT64 visual/pixel/
//! silicon parity or performance claim. This is also a **deliberately
//! partial port of `rt64_framebuffer_manager.cpp`**: that file is ~1093
//! lines and mostly bound to fn64's not-yet-ported State/Workload graph and
//! GPU descriptor/render-target machinery. This module ports only
//! `makeFramebufferTile` (lines 390-486). Sibling ticket M4.11 already
//! ported `insertRegionsTMEM`, `discardRegionsTMEM`, and
//! `synchronizeRegionsTMEM` from the same file into `rt64_tmem_regions.rs`
//! (landed before this ticket); this module does not touch, re-port, or
//! import behavior from those three functions or their `RegionTmem`/
//! `RegionTmemList` types (it only reuses their sibling module's `FbTile`
//! output-payload struct, as described in "Reuse, not new type" above).
//! Every other method and free function in `rt64_framebuffer_manager.cpp`
//! (`makeTileCopyTMEM`, `makeTileReintepretation`, `checkRegionsTMEM`,
//! `checkTileCopyTMEM`, `createTileCopyRecord`, `createTileCopySetup`,
//! `destroyAllTileCopies`, `find`, `findMostRecentContaining`,
//! `findTileCopyId`, `get`, `getUsedTimestamp`, `hashTracking`,
//! `performDiscards`, `performOperations`, `recordOperations`,
//! `reinterpretTileRecord`, `reinterpretTileSetup`, `resetOperations`,
//! `resetTracking`, `setupOperations`, `storeRAM`, `uploadRAM`,
//! `writeChanges`, `changeRAM`, `checkRAM`, `clearUsedTileCopies`, and the
//! `FramebufferManager` constructor) is **not ported** -- all are bound to
//! `RenderWorker`/`RenderTarget`/`Workload`/GPU descriptor-set machinery
//! this crate's State/Workload graph does not yet have a Rust equivalent
//! for, well outside this ticket's named scope. `Framebuffer::imageRowBytes`
//! and `Framebuffer::bestDitherPattern` themselves are also not re-ported
//! here as *canonical* implementations -- `rt64_framebuffer_geometry.rs`
//! (`M4.8`) owns that; this module's [`image_row_bytes`]/
//! [`best_dither_pattern`] are acknowledged, field-scoped duplicates for
//! this ticket's own [`FbExtent`] input type only (see "Reuse, not new
//! type").

use crate::rt64_tmem_regions::FbTile;

/// `G_IM_SIZ_4b` (`src/shared/rt64_f3d_defines.h:70`): the only named `siz`
/// constant this function compares against.
pub const G_IM_SIZ_4B: u8 = 0;

/// Owned subset of `RT64::Framebuffer`'s fields `makeFramebufferTile` reads,
/// taken as an input rather than reaching into a manager (per this ticket's
/// own instruction). See module doc "Reuse, not new type" for why this is a
/// fresh, ticket-scoped struct rather than an extension of
/// `rt64_framebuffer_geometry.rs`'s `FramebufferGeometry`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FbExtent {
    pub address_start: u32,
    pub address_end: u32,
    pub siz: u8,
    pub width: u32,
    pub height: u32,
    pub last_write_fmt: u8,
    pub dither_patterns: [u32; 4],
}

impl FbExtent {
    /// `Framebuffer::imageRowBytes(rowWidth)` (`rt64_framebuffer.cpp:53-55`,
    /// re-derived here per "Reuse, not new type" above): `rowWidth << siz >>
    /// 1`, left-shift then right-shift, exact order preserved.
    fn image_row_bytes(&self, row_width: u32) -> u32 {
        (row_width << self.siz) >> 1
    }

    /// `Framebuffer::bestDitherPattern()` (`rt64_framebuffer.cpp:189-191`,
    /// re-derived here per "Reuse, not new type" above): index of the first
    /// maximum element, matching `std::max_element`'s tie-breaking.
    fn best_dither_pattern(&self) -> u32 {
        let mut best_index = 0usize;
        let mut best_value = self.dither_patterns[0];
        for (i, &value) in self.dither_patterns.iter().enumerate().skip(1) {
            if value > best_value {
                best_value = value;
                best_index = i;
            }
        }
        best_index as u32
    }
}

/// Standalone re-derivation of [`FbExtent::image_row_bytes`] for use before
/// an `FbExtent` value exists in a caller's own arithmetic (mirrors the C++
/// call sites, which invoke `fb->imageRowBytes(...)` as a plain method call
/// on a pointer that is never null per the source's own `assert`).
fn image_row_bytes(extent: &FbExtent, row_width: u32) -> u32 {
    extent.image_row_bytes(row_width)
}

/// Standalone re-derivation of [`FbExtent::best_dither_pattern`]; see
/// [`image_row_bytes`] above for why this free-function wrapper exists.
fn best_dither_pattern(extent: &FbExtent) -> u32 {
    extent.best_dither_pattern()
}

/// Every named rejection path `makeFramebufferTile` can return `false` from,
/// in the same order the C++ source checks them. The C++ function returns
/// only a `bool`; this enum exists so tests can assert *which* guard fired
/// independently (per the hazard brief: "every rejection path must be
/// reachable in a test"), which is a Rust-side testing aid, not a widened
/// behavior claim -- callers that only need the C++'s `bool` can match
/// `Ok(_) | Err(_)`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Rejection {
    /// `addressStart >= fb->addressEnd` after the row-walk loop.
    AddressPastFramebufferEnd,
    /// `minEndAddress <= addressStart`.
    EndAtOrBeforeStart,
    /// `(offset % pixelSize) != 0`.
    OffsetNotPixelAligned,
    /// `fromLoadBlock && multipleRows && misalignedRow`.
    MisalignedMultiRowLoadBlock,
    /// `outTile.bottom <= outTile.top` (post-clamp).
    DegenerateBottomAtOrBeforeTop,
    /// `outTile.right <= outTile.left` (post-clamp).
    DegenerateRightAtOrBeforeLeft,
}

/// Literal port of `FramebufferManager::makeFramebufferTile`
/// (`src/hle/rt64_framebuffer_manager.cpp:390-486`). See module doc
/// "Admitted domain" for the six rejection paths, the two sentinel-derived
/// branches (`line_width == 0`, `tile_height == 0`), both divide-by-zero
/// frontiers, the unbounded `while` loop's non-termination frontier for
/// `fb_stride == 0`, comparison strictness, and exact truncation/operand
/// order.
///
/// `fb` is `Framebuffer *fb` with the source's `assert(fb != nullptr)`
/// erased -- Rust's `&FbExtent` cannot be null, so the assert is vacuously
/// satisfied by the type system rather than ported as a runtime check.
///
/// Returns `Ok(tile)` where the C++ returns `true` (with `outTile`
/// populated), or `Err(reason)` naming exactly which of the six named
/// guards rejected the input where the C++ returns `false`.
///
/// # Panics
///
/// Panics (Rust's built-in divide-by-zero/remainder-by-zero trap) wherever
/// the C++ has undefined behavior from an integer division or modulo by
/// zero -- see module doc "Admitted domain" for both frontiers
/// (`pixel_size == 0` at `siz == 0`; `row_bytes == 0` at `width == 0`).
/// Loops forever (never returns) if `fb.address_start > address_start` and
/// `fb_stride == 0` (`width == 0`) -- see module doc "Admitted domain" for
/// why this is reported, not guarded.
pub fn make_framebuffer_tile(
    fb: &FbExtent,
    mut address_start: u32,
    address_end: u32,
    line_width: u32,
    tile_height: u32,
    _rgba32: bool,
) -> Result<FbTile, Rejection> {
    // If the starting address is lower than the framebuffer address, we move
    // a row one by one according to the stride specified of the original
    // image width.
    let mut _tile_row_start: u32 = 0;
    let fb_stride = image_row_bytes(fb, fb.width);
    while address_start < fb.address_start {
        address_start += fb_stride;
        _tile_row_start += 1;
    }

    // We went over the allowed address range, a tile copy is impossible.
    if address_start >= fb.address_end {
        return Err(Rejection::AddressPastFramebufferEnd);
    }

    // Disallow the tile copy if the end address ended up below the starting
    // address.
    let min_end_address = address_end.min(fb.address_end);
    if min_end_address <= address_start {
        return Err(Rejection::EndAtOrBeforeStart);
    }

    // Figure out how many rows we could possibly given the current address
    // range.
    let fb_bytes = min_end_address - fb.address_start;
    let fb_min_row = (address_start - fb.address_start) / fb_stride;
    let fb_max_row = (fb_bytes / fb_stride) + if (fb_bytes % fb_stride) > 0 { 1 } else { 0 };

    // Relative offset of the image start to the framebuffer start.
    let offset = address_start - fb.address_start;

    // This will be the same size for 4 and 8 byte formats.
    let pixel_size: u32 = (1u32 << fb.siz) >> 1;

    // The offset is not aligned to the pixel size. It's not possible to
    // make a direct copy.
    if (offset % pixel_size) != 0 {
        return Err(Rejection::OffsetNotPixelAligned);
    }

    // Figure out where the upper left coordinate of the tile is inside the
    // framebuffer.
    let row_bytes = image_row_bytes(fb, fb.width);
    let row = offset / row_bytes;
    let row_offset = offset % row_bytes;
    let pixel_shift: u32 = if fb.siz == G_IM_SIZ_4B { 1 } else { 0 };

    let mut tile = FbTile::default();
    tile.left = (row_offset / pixel_size) << pixel_shift;
    tile.top = row;

    // Line width is defined.
    if line_width > 0 {
        tile.right = tile.left + line_width;
    }
    // Figure it out from the framebuffer instead.
    else {
        let row_right_pixels = ((row_bytes - row_offset) / pixel_size) << pixel_shift;
        tile.right = tile.left + row_right_pixels;
    }

    // Tile height is defined.
    if tile_height > 0 {
        tile.bottom = tile.top + tile_height;
    } else {
        let row_end = ((address_end - address_start) / row_bytes).max(1);
        tile.bottom = tile.top + row_end;

        // Invalidate the tile if this is a loadBlock operation, more than
        // one row is being loaded and the offset is not perfectly aligned
        // with a row.
        let from_load_block = tile_height == 0;
        let multiple_rows = row_end > 1;
        let misaligned_row = row_offset > 0;
        if from_load_block && multiple_rows && misaligned_row {
            return Err(Rejection::MisalignedMultiRowLoadBlock);
        }
    }

    // Clamp the tile to the framebuffer's dimensions and the image row
    // ranges found.
    tile.top = tile.top.max(fb_min_row);
    tile.right = tile.right.min(fb.width);
    tile.bottom = tile.bottom.min(fb.height);
    tile.bottom = tile.bottom.min(fb_max_row);

    // Invalid tile.
    if tile.bottom <= tile.top {
        return Err(Rejection::DegenerateBottomAtOrBeforeTop);
    }
    if tile.right <= tile.left {
        return Err(Rejection::DegenerateRightAtOrBeforeLeft);
    }

    // Define the tile.
    tile.line_width = if line_width > 0 {
        line_width
    } else {
        tile.right - tile.left
    };
    tile.address = fb.address_start;
    tile.siz = fb.siz;
    tile.fmt = fb.last_write_fmt;
    tile.dither_pattern = best_dither_pattern(fb);

    Ok(tile)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fb(
        address_start: u32,
        address_end: u32,
        siz: u8,
        width: u32,
        height: u32,
        last_write_fmt: u8,
    ) -> FbExtent {
        FbExtent {
            address_start,
            address_end,
            siz,
            width,
            height,
            last_write_fmt,
            dither_patterns: [0, 0, 0, 0],
        }
    }

    // -----------------------------------------------------------------
    // Accepting path: minimum and maximum valid geometry
    // -----------------------------------------------------------------

    #[test]
    fn accepts_minimum_valid_geometry() {
        // Hand-computed (siz=1, pixelSize=1, rowBytes=64): fbStride=64, no
        // row-walk (addressStart==fb.addressStart). offset=0, aligned.
        // row=0, rowOffset=0, left=0, top=0. lineWidth=0 (derive):
        // rowRightPixels=(64-0)/1=64, right=64. tileHeight=4 (explicit):
        // bottom=0+4=4. fbMinRow=0, fbMaxRow=2048/64=32. Clamp: top=0,
        // right=min(64,64)=64, bottom=min(4,32)=4, bottom=min(4,32)=4.
        // Accept: left=0,top=0,right=64,bottom=4,lineWidth=64.
        let f = fb(0x1000, 0x1800, 1, 64, 32, 5);
        let tile = make_framebuffer_tile(&f, 0x1000, 0x1800, 0, 4, false).unwrap();
        assert_eq!(tile.left, 0);
        assert_eq!(tile.top, 0);
        assert_eq!(tile.right, 64);
        assert_eq!(tile.bottom, 4);
        assert_eq!(tile.line_width, 64);
        assert_eq!(tile.address, 0x1000);
        assert_eq!(tile.siz, 1);
        assert_eq!(tile.fmt, 5);
    }

    #[test]
    fn accepts_maximum_valid_geometry() {
        // Hand-computed (siz=3, pixelSize=4, width=1000,height=1000):
        // rowBytes=1000<<3>>1=4000. addressStart=0==fb.addressStart, no
        // walk. minEndAddress=min(1_000_000,1_000_000)=1_000_000. fbBytes=
        // 1_000_000. fbMinRow=0/4000=0. fbMaxRow=1_000_000/4000+0=250
        // (exact). offset=0, pixelSize=4, aligned. row=0,rowOffset=0,
        // left=0,top=0. lineWidth=0(derive): rowRightPixels=(4000-0)/4=
        // 1000, right=1000. tileHeight=0(derive): rowEnd=max(1_000_000/
        // 4000,1)=250, bottom=250; fromLoadBlock=true,multipleRows=true,
        // misalignedRow=(0>0)=false -> not rejected. Clamp: top=0,
        // right=min(1000,1000)=1000, bottom=min(250,1000)=250,
        // bottom=min(250,250)=250. Accept: left=0,top=0,right=1000,
        // bottom=250,lineWidth=1000.
        let f = fb(0, 1_000_000, 3, 1000, 1000, 255);
        let tile = make_framebuffer_tile(&f, 0, 1_000_000, 0, 0, false).unwrap();
        assert_eq!(tile.left, 0);
        assert_eq!(tile.top, 0);
        assert_eq!(tile.right, 1000);
        assert_eq!(tile.bottom, 250);
        assert_eq!(tile.line_width, 1000);
        assert_eq!(tile.address, 0);
        assert_eq!(tile.siz, 3);
        assert_eq!(tile.fmt, 255);
    }

    // -----------------------------------------------------------------
    // Rejection #1: address walked past the framebuffer end
    // -----------------------------------------------------------------

    #[test]
    fn rejects_when_address_walks_past_framebuffer_end() {
        // addressStart(0x1800)>=fb.addressEnd(0x1800): non-strict, equal
        // triggers rejection.
        let f = fb(0x1000, 0x1800, 1, 64, 32, 5);
        let result = make_framebuffer_tile(&f, 0x1800, 0x2000, 0, 0, false);
        assert_eq!(result, Err(Rejection::AddressPastFramebufferEnd));
    }

    #[test]
    fn accepts_at_address_end_minus_one() {
        // Hand-computed boundary: addressStart=0x17FF=6143. offset=2047,
        // pixelSize=1(siz=1), aligned. rowBytes=64, row=2047/64=31,
        // rowOffset=2047%64=63. left=63,top=31. lineWidth=0(derive):
        // rowRightPixels=(64-63)/1=1, right=64. tileHeight=4: bottom=35.
        // fbMinRow=(6143-4096)/64=31. fbMaxRow=(6144-4096)/64=32. Clamp:
        // top=max(31,31)=31, right=min(64,64)=64, bottom=min(35,32)=32,
        // bottom=min(32,32)=32. Accept: left=63,top=31,right=64,bottom=32,
        // lineWidth=1.
        let f = fb(0x1000, 0x1800, 1, 64, 32, 5);
        let tile = make_framebuffer_tile(&f, 0x17FF, 0x1800, 0, 4, false).unwrap();
        assert_eq!(tile.left, 63);
        assert_eq!(tile.top, 31);
        assert_eq!(tile.right, 64);
        assert_eq!(tile.bottom, 32);
        assert_eq!(tile.line_width, 1);
    }

    #[test]
    fn rejects_at_address_end_exactly() {
        // Same as rejects_when_address_walks_past_framebuffer_end, pinned
        // as the other side of the boundary pair above.
        let f = fb(0x1000, 0x1800, 1, 64, 32, 5);
        let result = make_framebuffer_tile(&f, 0x1800, 0x1900, 0, 4, false);
        assert_eq!(result, Err(Rejection::AddressPastFramebufferEnd));
    }

    // -----------------------------------------------------------------
    // Rejection #2: end address at or before start
    // -----------------------------------------------------------------

    #[test]
    fn rejects_when_end_equals_start() {
        // minEndAddress(0x1000) <= addressStart(0x1000): non-strict,
        // equal triggers rejection.
        let f = fb(0x1000, 0x1800, 1, 64, 32, 5);
        let result = make_framebuffer_tile(&f, 0x1000, 0x1000, 0, 0, false);
        assert_eq!(result, Err(Rejection::EndAtOrBeforeStart));
    }

    #[test]
    fn accepts_when_end_is_one_past_start() {
        // Hand-computed: addressStart=0x1000,addressEnd=0x1001.
        // minEndAddress=min(0x1001,0x1800)=0x1001>0x1000, passes.
        // offset=0,pixelSize=1,aligned. row=0,rowOffset=0,left=0,top=0.
        // lineWidth=0(derive): rowRightPixels=64,right=64. tileHeight=0
        // (derive): rowEnd=max((0x1001-0x1000)/64,1)=max(0,1)=1,bottom=1.
        // fromLoadBlock=true,multipleRows=(1>1)=false -> not rejected
        // (single row). fbMinRow=0. fbBytes=0x1001-0x1000=1,
        // fbMaxRow=1/64+((1%64)>0?1:0)=0+1=1. Clamp: top=0,right=64,
        // bottom=min(1,32)=1,bottom=min(1,1)=1. Accept: bottom=1,top=0.
        let f = fb(0x1000, 0x1800, 1, 64, 32, 5);
        let tile = make_framebuffer_tile(&f, 0x1000, 0x1001, 0, 0, false).unwrap();
        assert_eq!(tile.top, 0);
        assert_eq!(tile.bottom, 1);
    }

    // -----------------------------------------------------------------
    // Rejection #3: offset not pixel-aligned
    // -----------------------------------------------------------------

    #[test]
    fn rejects_misaligned_offset() {
        // siz=2: pixelSize=(1<<2)>>1=2. offset=addressStart-fb.addressStart
        // = 0x1001-0x1000=1. 1%2=1 != 0 -> rejected.
        let f = fb(0x1000, 0x2000, 2, 64, 32, 5);
        let result = make_framebuffer_tile(&f, 0x1001, 0x2000, 0, 4, false);
        assert_eq!(result, Err(Rejection::OffsetNotPixelAligned));
    }

    #[test]
    fn accepts_offset_exactly_pixel_aligned() {
        // Hand-computed: siz=2,pixelSize=2, offset=2 (0x1002-0x1000).
        // 2%2=0, aligned. rowBytes=64<<2>>1=128. row=2/128=0,
        // rowOffset=2. pixelShift=0(siz!=0). left=(2/2)<<0=1,top=0.
        // lineWidth=0(derive): rowRightPixels=((128-2)/2)<<0=63,
        // right=1+63=64. tileHeight=4: bottom=0+4=4. fbMinRow=
        // (0x1002-0x1000)/128=2/128=0. fbBytes=min(0x2000,0x2000)-0x1000=
        // 0x1000=4096. fbMaxRow=4096/128+0=32. Clamp: top=0,
        // right=min(64,64)=64, bottom=min(4,32)=4,bottom=min(4,32)=4.
        // Accept: left=1,top=0,right=64,bottom=4,lineWidth=64-1=63.
        let f = fb(0x1000, 0x2000, 2, 64, 32, 5);
        let tile = make_framebuffer_tile(&f, 0x1002, 0x2000, 0, 4, false).unwrap();
        assert_eq!(tile.left, 1);
        assert_eq!(tile.top, 0);
        assert_eq!(tile.right, 64);
        assert_eq!(tile.bottom, 4);
        assert_eq!(tile.line_width, 63);
    }

    // -----------------------------------------------------------------
    // Rejection #4: misaligned multi-row load-block
    // -----------------------------------------------------------------

    #[test]
    fn rejects_load_block_misaligned_multi_row() {
        // Hand-computed: siz=1,pixelSize=1. addressStart=0x100A (offset=
        // 10, aligned since pixelSize=1). rowBytes=64,row=0,rowOffset=10.
        // tileHeight=0(sentinel): rowEnd=max((0x1100-0x100A)/64,1)=
        // max(246/64,1)=max(3,1)=3. fromLoadBlock=true,
        // multipleRows=(3>1)=true, misalignedRow=(10>0)=true -> all three
        // true -> rejected.
        let f = fb(0x1000, 0x2000, 1, 64, 32, 5);
        let result = make_framebuffer_tile(&f, 0x100A, 0x1100, 0, 0, false);
        assert_eq!(result, Err(Rejection::MisalignedMultiRowLoadBlock));
    }

    #[test]
    fn load_block_single_row_with_misalignment_is_accepted() {
        // Same misaligned offset (rowOffset=10) but addressEnd close
        // enough that rowEnd==1: rowEnd=max((0x1020-0x100A)/64,1)=
        // max(22/64,1)=max(0,1)=1. multipleRows=(1>1)=false -> guard's
        // three-way && is false -> NOT rejected, despite misalignedRow
        // being true. Proves multipleRows is a real, independent
        // sub-condition.
        let f = fb(0x1000, 0x2000, 1, 64, 32, 5);
        let tile = make_framebuffer_tile(&f, 0x100A, 0x1020, 0, 0, false).unwrap();
        assert_eq!(tile.bottom - tile.top, 1);
    }

    #[test]
    fn load_block_multi_row_aligned_offset_is_accepted() {
        // Aligned offset (rowOffset=0, addressStart==fb.addressStart) with
        // addressEnd far enough for multiple rows: rowEnd=
        // max((0x1100-0x1000)/64,1)=4>1 (multipleRows true), but
        // misalignedRow=(0>0)=false -> guard's three-way && is false ->
        // NOT rejected. Proves misalignedRow is a real, independent
        // sub-condition.
        let f = fb(0x1000, 0x2000, 1, 64, 32, 5);
        let tile = make_framebuffer_tile(&f, 0x1000, 0x1100, 0, 0, false).unwrap();
        assert_eq!(tile.bottom - tile.top, 4);
    }

    #[test]
    fn load_tile_multi_row_misaligned_is_accepted() {
        // Same misaligned offset and multi-row extent as
        // rejects_load_block_misaligned_multi_row, but tileHeight=5
        // (explicit, non-sentinel) instead of 0: the whole `if
        // (fromLoadBlock && multipleRows && misalignedRow)` block is
        // inside the tileHeight==0 `else` arm, so an explicit tileHeight
        // skips the guard entirely regardless of row alignment. Hand-
        // computed: left=10,top=0 (as in the rejected case), bottom=
        // 0+5=5. fbMinRow=(0x100A-0x1000)/64=10/64=0. fbBytes=
        // min(0x2000,0x3000)-0x1000=0x2000=8192. fbMaxRow=8192/64+0=128.
        // Clamp: top=0,right=min(64,64)=64,bottom=min(5,32)=5,
        // bottom=min(5,128)=5. Accept: left=10,top=0,right=64,bottom=5,
        // lineWidth=64-10=54. Proves fromLoadBlock is the third real,
        // independent sub-condition.
        let f = fb(0x1000, 0x3000, 1, 64, 32, 5);
        let tile = make_framebuffer_tile(&f, 0x100A, 0x2000, 0, 5, false).unwrap();
        assert_eq!(tile.left, 10);
        assert_eq!(tile.top, 0);
        assert_eq!(tile.right, 64);
        assert_eq!(tile.bottom, 5);
        assert_eq!(tile.line_width, 54);
    }

    // -----------------------------------------------------------------
    // Rejection #5: degenerate bottom <= top
    // -----------------------------------------------------------------

    #[test]
    fn rejects_degenerate_bottom_at_top() {
        // fb.height=0 clamps bottom down to 0, equal to top(0):
        // bottom(0)<=top(0) non-strict -> rejected.
        let f = fb(0x1000, 0x2000, 1, 64, 0, 5);
        let result = make_framebuffer_tile(&f, 0x1000, 0x1100, 0, 4, false);
        assert_eq!(result, Err(Rejection::DegenerateBottomAtOrBeforeTop));
    }

    #[test]
    fn accepts_bottom_one_past_top() {
        // Same as above but fb.height=1: bottom clamps to min(4,1)=1,
        // then min(1,fbMaxRow>=1)=1. bottom(1)<=top(0)? false -> accepted,
        // the boundary case one past rejection.
        let f = fb(0x1000, 0x2000, 1, 64, 1, 5);
        let tile = make_framebuffer_tile(&f, 0x1000, 0x1100, 0, 4, false).unwrap();
        assert_eq!(tile.top, 0);
        assert_eq!(tile.bottom, 1);
    }

    // -----------------------------------------------------------------
    // Rejection #6: degenerate right <= left
    // -----------------------------------------------------------------
    //
    // Per module doc "Admitted domain", this rejection is unreachable from
    // any realistic (non-u32-overflowing) input for every siz that does not
    // already crash at one of the two divide-by-zero frontiers -- i.e. it
    // is unreachable for ALL siz values via this function's public entry
    // point without deliberately overflowing `left + lineWidth`. This is a
    // TEST GAP (the branch is not dead code -- it is literally ported and
    // reachable in principle -- it is simply not reachable by any input
    // this module's other invariants allow it to construct), not something
    // this module works around. No test triggers Rejection::
    // DegenerateRightAtOrBeforeLeft; see the module doc for the full proof
    // and the parallel with rt64_framebuffer_geometry.rs's own documented,
    // untested overflow frontier on get_native_size/add_dither_patterns.

    // -----------------------------------------------------------------
    // Two sentinel-derived branches (line_width==0, tile_height==0)
    // -----------------------------------------------------------------

    #[test]
    fn line_width_zero_derives_right_from_row_bytes() {
        // Hand-computed: lineWidth=0 -> derive branch. rowRightPixels=
        // (64-0)/1=64, right=0+64=64 (spans the whole row).
        let f = fb(0x1000, 0x2000, 1, 64, 32, 5);
        let tile = make_framebuffer_tile(&f, 0x1000, 0x2000, 0, 4, false).unwrap();
        assert_eq!(tile.right, 64);
        assert_eq!(tile.line_width, 64);
    }

    #[test]
    fn line_width_nonzero_sets_right_directly() {
        // Hand-computed: lineWidth=10 (explicit) -> right=left+10=0+10=10,
        // NOT derived from the row's remaining width (64).
        let f = fb(0x1000, 0x2000, 1, 64, 32, 5);
        let tile = make_framebuffer_tile(&f, 0x1000, 0x2000, 10, 4, false).unwrap();
        assert_eq!(tile.right, 10);
        assert_eq!(tile.line_width, 10);
    }

    #[test]
    fn tile_height_zero_derives_bottom_from_address_range() {
        // Hand-computed: tileHeight=0 -> derive branch. addressEnd=
        // 0x1000+256=0x1100 (4 rows of 64 bytes). rowEnd=max(256/64,1)=4.
        // bottom=0+4=4. rowOffset=0 (aligned) so the multi-row guard's
        // misalignedRow sub-condition is false -> not rejected.
        let f = fb(0x1000, 0x2000, 1, 64, 32, 5);
        let tile = make_framebuffer_tile(&f, 0x1000, 0x1100, 0, 0, false).unwrap();
        assert_eq!(tile.bottom, 4);
    }

    #[test]
    fn tile_height_nonzero_sets_bottom_directly() {
        // Hand-computed: tileHeight=7 (explicit) -> bottom=top+7=0+7=7
        // BEFORE clamping, computed directly from tileHeight rather than
        // derived from the address range's rowEnd. addressEnd=0x2000 is
        // chosen far enough past addressStart that fbMaxRow (4096/64=64)
        // does not bind, so the post-clamp bottom (min(7,32)=7,
        // min(7,64)=7) still equals 7 -- isolating "bottom comes from
        // tileHeight, not rowEnd" from the separate fbMaxRow-clamping
        // behavior covered by bottom_is_clamped_by_both_height_and_
        // fb_max_row_in_order.
        let f = fb(0x1000, 0x2000, 1, 64, 32, 5);
        let tile = make_framebuffer_tile(&f, 0x1000, 0x2000, 0, 7, false).unwrap();
        assert_eq!(tile.bottom, 7);
    }

    // -----------------------------------------------------------------
    // Divide-by-zero frontiers (reported, not guarded)
    // -----------------------------------------------------------------

    #[test]
    fn siz_4b_panics_on_the_offset_alignment_modulo() {
        // pixelSize = 1<<0>>1 = 0 for siz==0 (G_IM_SIZ_4b). The very next
        // statement, `offset % pixelSize`, panics unconditionally (Rust's
        // divide-by-zero/remainder-by-zero trap, matching C++ UB) -- this
        // is a crash frontier, not a Result::Err, confirmed with
        // catch_unwind rather than an equality assertion.
        let f = fb(0x1000, 0x2000, 0, 64, 32, 5);
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            make_framebuffer_tile(&f, 0x1000, 0x1100, 0, 4, false)
        }));
        assert!(
            result.is_err(),
            "expected a panic (divide by zero), got a return value"
        );
    }

    #[test]
    fn zero_width_framebuffer_panics_on_row_stride_division() {
        // width==0 -> fbStride==0 (siz=1, no row-walk needed since
        // addressStart==fb.addressStart). The FIRST division by fbStride
        // after the row-walk loop is fbMinRow's, which panics before
        // pixelSize is ever computed or checked -- see module doc
        // "Admitted domain" for the exact ordering proof.
        let f = fb(0x1000, 0x2000, 1, 0, 32, 5);
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            make_framebuffer_tile(&f, 0x1000, 0x1100, 0, 4, false)
        }));
        assert!(
            result.is_err(),
            "expected a panic (divide by zero), got a return value"
        );
    }

    #[test]
    fn zero_height_framebuffer_is_rejected_not_a_panic() {
        // height==0 is never a divisor anywhere in the function -- it only
        // ever clamps `bottom` from above, reliably driving rejection #5
        // instead of a panic. Contrasts directly with the width==0 case
        // above.
        let f = fb(0x1000, 0x2000, 1, 64, 0, 5);
        let result = make_framebuffer_tile(&f, 0x1000, 0x1100, 0, 4, false);
        assert_eq!(result, Err(Rejection::DegenerateBottomAtOrBeforeTop));
    }

    #[test]
    fn siz_zero_and_width_zero_simultaneously_hits_row_stride_first() {
        // Both zero-divisor conditions present at once, with no row-walk
        // needed (addressStart==fb.addressStart): fbMinRow's division
        // (width==0 frontier) is reached and panics before pixelSize is
        // ever computed, matching the ordering proof for the width-only
        // case. Same assertion shape as the two frontier tests above,
        // included to pin the co-occurring case explicitly rather than
        // leaving it as an implied consequence.
        let f = fb(0x1000, 0x2000, 0, 0, 32, 5);
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            make_framebuffer_tile(&f, 0x1000, 0x1100, 0, 4, false)
        }));
        assert!(
            result.is_err(),
            "expected a panic (divide by zero), got a return value"
        );
    }

    // -----------------------------------------------------------------
    // Row-walk loop (non-zero stride: terminates and advances correctly)
    // -----------------------------------------------------------------

    #[test]
    fn row_walk_loop_advances_address_start_by_whole_strides() {
        // Hand-computed: fbStride=64 (siz=1,width=64). addressStart=
        // 0x0F80=4032, fb.addressStart=4096. Loop: 4032<4096 -> +64=4096,
        // 4096<4096 false, exit after exactly 1 iteration. Resulting
        // addressStart==fb.addressStart==offset 0, identical geometry to
        // the minimum-valid-geometry baseline.
        let f = fb(0x1000, 0x1800, 1, 64, 32, 5);
        let tile = make_framebuffer_tile(&f, 0x0F80, 0x1800, 0, 4, false).unwrap();
        assert_eq!(tile.left, 0);
        assert_eq!(tile.top, 0);
    }

    #[test]
    fn row_walk_loop_advances_by_multiple_strides() {
        // Hand-computed: fbStride=64. addressStart=0x1000-128=0x0F80...
        // use a larger gap: addressStart=4096-192=3904 (3 strides of 64).
        // Loop runs 3 times: 3904+64=3968, +64=4032, +64=4096, exit.
        // Same resulting geometry as the baseline (addressStart lands
        // exactly on fb.addressStart).
        let f = fb(0x1000, 0x1800, 1, 64, 32, 5);
        let tile = make_framebuffer_tile(&f, 3904, 0x1800, 0, 4, false).unwrap();
        assert_eq!(tile.left, 0);
        assert_eq!(tile.top, 0);
    }

    // -----------------------------------------------------------------
    // No-underflow proof for `addressEnd - addressStart` (tileHeight==0
    // branch), despite using the raw (non-min-clamped) addressEnd
    // -----------------------------------------------------------------

    #[test]
    fn no_underflow_sweep_address_end_minus_address_start() {
        // Whenever rejection #1 and #2 both do not fire, addressEnd (raw
        // parameter) > addressStart strictly -- see module doc "Admitted
        // domain" for the algebraic proof (addressEnd >= minEndAddress >
        // addressStart). This sweep asserts the subtraction inside the
        // tileHeight==0 branch never panics on overflow for a broad
        // pseudorandom range of inputs that survive both rejections.
        let mut state: u64 = 0x9E3779B97F4A7C15;
        let mut next_u32 = || -> u32 {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            (state >> 32) as u32
        };
        let mut checked = 0u32;
        for _ in 0..200_000 {
            let width = (next_u32() % 2000) + 1; // avoid width==0 frontier
            let siz = 1 + (next_u32() % 3) as u8; // 1,2,3; avoid siz==0 frontier
            let fb_addr_start = next_u32() % 1_000_000;
            let fb_addr_end = fb_addr_start.saturating_add(next_u32() % 200_000);
            let address_start =
                fb_addr_start.saturating_sub(next_u32() % 5000) + (next_u32() % 50_000);
            let address_end = next_u32() % (fb_addr_end.saturating_add(50_000) + 1);
            let f = fb(fb_addr_start, fb_addr_end, siz, width, 2000, 0);
            // tile_height=0 forces the branch containing `addressEnd -
            // addressStart`; line_width=0 keeps the derive path active
            // too. A panic here (Rust's debug-mode overflow check) would
            // fail the test outright rather than being caught, which is
            // the point: we assert no panic occurs, not a specific value.
            let _ = make_framebuffer_tile(&f, address_start, address_end, 0, 0, false);
            checked += 1;
        }
        assert_eq!(checked, 200_000);
    }

    // -----------------------------------------------------------------
    // pixel_shift is always zero for every reachable siz (dead-code
    // finding, confirmed rather than asserted from memory)
    // -----------------------------------------------------------------

    #[test]
    fn pixel_shift_is_always_zero_for_every_reachable_siz() {
        // siz==0 always panics before pixel_shift is used (see the
        // divide-by-zero frontier tests above), so only siz in {1,2,3} can
        // ever reach the pixel_shift computation -- and G_IM_SIZ_4B is 0,
        // so pixel_shift is 0 for all three. This test pins that the
        // computed `left` values for siz 1/2/3 never reflect a left-shift
        // by 1 (which would double odd left values), by checking a
        // fixture with an intentionally odd pre-shift left coordinate
        // stays odd (unshifted) rather than becoming even (shifted).
        for siz in [1u8, 2, 3] {
            let pixel_size: u32 = (1u32 << siz) >> 1;
            // Choose an offset of exactly 3*pixel_size so the pre-shift
            // left coordinate is 3 (odd) -- if pixel_shift were 1, this
            // would become 6 (even) instead.
            let f = fb(0x1000, 0x2000, siz, 4096, 32, 0);
            let offset = 3 * pixel_size;
            let tile = make_framebuffer_tile(&f, 0x1000 + offset, 0x2000, 0, 4, false).unwrap();
            assert_eq!(
                tile.left, 3,
                "siz={siz}: pixel_shift was applied but should be 0"
            );
        }
    }

    // -----------------------------------------------------------------
    // Clamping order: bottom is clamped twice (fb.height, then fbMaxRow)
    // -----------------------------------------------------------------

    #[test]
    fn bottom_is_clamped_by_both_height_and_fb_max_row_in_order() {
        // Hand-computed: siz=1,width=64,height=10. addressEnd=0x1080
        // (128 bytes past fb.addressStart) -> fbMaxRow=128/64=2. tileHeight
        // =50(explicit) -> bottom=0+50=50 before clamping. First clamp
        // (fb.height=10): bottom=min(50,10)=10. Second clamp (fbMaxRow=2):
        // bottom=min(10,2)=2. The final value (2) is strictly smaller than
        // what fb.height's clamp alone would give (10), proving the
        // second clamp is load-bearing, not redundant.
        let f = fb(0x1000, 0x1080, 1, 64, 10, 1);
        let tile = make_framebuffer_tile(&f, 0x1000, 0x1080, 0, 50, false).unwrap();
        assert_eq!(tile.bottom, 2);
    }

    #[test]
    fn top_is_clamped_up_to_fb_min_row_not_down() {
        // fbMinRow is a MAX clamp (raises top, never lowers it). Using the
        // address-end-minus-one boundary fixture (top naturally computes
        // to 31, fbMinRow is also 31) confirms the clamp is a no-op there;
        // this test instead forces fbMinRow above the naturally-computed
        // top via the row-walk loop landing address_start mid-framebuffer
        // while top's own row computation (from a smaller local offset)
        // would otherwise be lower. Hand-computed: siz=1,width=64,
        // addressStart=0x1040 (offset=64, one full row in) -> row=1,
        // rowOffset=0, top=1 before clamping. fbMinRow=(0x1040-0x1000)/64
        // =64/64=1. top=max(1,1)=1 -- clamp is a no-op here since both
        // agree; included to document the max-clamp direction explicitly
        // (fbMinRow never lowers a higher naturally-computed top either,
        // by construction: fbMinRow uses the same addressStart/fbStride
        // as `row`, so fbMinRow <= row always after the row-walk loop).
        let f = fb(0x1000, 0x2000, 1, 64, 32, 5);
        let tile = make_framebuffer_tile(&f, 0x1040, 0x2000, 0, 4, false).unwrap();
        assert_eq!(tile.top, 1);
    }

    // -----------------------------------------------------------------
    // dither_pattern: first-max tie-break, propagated from FbExtent
    // -----------------------------------------------------------------

    #[test]
    fn dither_pattern_picks_first_max_on_tie() {
        // dither_patterns=[5,9,9,3]: max value 9 occurs at indices 1 and
        // 2; std::max_element (and this port's linear scan) picks the
        // FIRST occurrence, index 1.
        let mut f = fb(0x1000, 0x2000, 1, 64, 32, 9);
        f.dither_patterns = [5, 9, 9, 3];
        let tile = make_framebuffer_tile(&f, 0x1000, 0x2000, 0, 4, false).unwrap();
        assert_eq!(tile.dither_pattern, 1);
    }

    #[test]
    fn dither_pattern_all_zero_returns_index_zero() {
        let f = fb(0x1000, 0x2000, 1, 64, 32, 0);
        let tile = make_framebuffer_tile(&f, 0x1000, 0x2000, 0, 4, false).unwrap();
        assert_eq!(tile.dither_pattern, 0);
    }

    #[test]
    fn dither_pattern_last_index_max_is_picked() {
        let mut f = fb(0x1000, 0x2000, 1, 64, 32, 0);
        f.dither_patterns = [1, 2, 3, 40];
        let tile = make_framebuffer_tile(&f, 0x1000, 0x2000, 0, 4, false).unwrap();
        assert_eq!(tile.dither_pattern, 3);
    }

    // -----------------------------------------------------------------
    // Field propagation: address/siz/fmt come from fb, not the caller
    // -----------------------------------------------------------------

    #[test]
    fn tile_address_is_framebuffer_address_start_not_the_walked_address() {
        // Even after the row-walk loop advances the local addressStart,
        // outTile.address is set to fb->addressStart (the ORIGINAL
        // framebuffer base), not the walked/advanced value.
        let f = fb(0x1000, 0x1800, 1, 64, 32, 5);
        let tile = make_framebuffer_tile(&f, 0x0F80, 0x1800, 0, 4, false).unwrap();
        assert_eq!(tile.address, 0x1000);
    }

    #[test]
    fn tile_siz_and_fmt_come_from_framebuffer_not_caller() {
        let f = fb(0x1000, 0x2000, 2, 64, 32, 77);
        let tile = make_framebuffer_tile(&f, 0x1000, 0x2000, 0, 4, false).unwrap();
        assert_eq!(tile.siz, 2);
        assert_eq!(tile.fmt, 77);
    }

    // -----------------------------------------------------------------
    // rgba32 parameter is accepted but has no observable effect (dead
    // parameter within this function, per module doc "Admitted domain")
    // -----------------------------------------------------------------

    #[test]
    fn rgba32_parameter_does_not_change_the_result() {
        let f = fb(0x1000, 0x1800, 1, 64, 32, 5);
        let with_true = make_framebuffer_tile(&f, 0x1000, 0x1800, 0, 4, true).unwrap();
        let with_false = make_framebuffer_tile(&f, 0x1000, 0x1800, 0, 4, false).unwrap();
        assert_eq!(with_true, with_false);
    }

    // -----------------------------------------------------------------
    // fb_max_row ceiling-division idiom: strict `> 0` on the remainder
    // -----------------------------------------------------------------

    #[test]
    fn fb_max_row_adds_no_extra_row_when_fb_bytes_divides_evenly() {
        // fbBytes=128 (2*fbStride=64), divides evenly: fbMaxRow=128/64+
        // ((128%64)>0?1:0)=2+0=2. tileHeight=50(explicit, larger than
        // fbMaxRow) so the final bottom is bound by this exact ceiling.
        let f = fb(0x1000, 0x1080, 1, 64, 100, 1);
        let tile = make_framebuffer_tile(&f, 0x1000, 0x1080, 0, 50, false).unwrap();
        assert_eq!(tile.bottom, 2);
    }

    #[test]
    fn fb_max_row_adds_one_extra_row_when_fb_bytes_has_a_remainder() {
        // fbBytes=129 (one byte past 2*fbStride=64): fbMaxRow=129/64+
        // ((129%64)>0?1:0)=2+1=3 -- one extra partial row counted, per the
        // strict `> 0` test on the remainder.
        let f = fb(0x1000, 0x1081, 1, 64, 100, 1);
        let tile = make_framebuffer_tile(&f, 0x1000, 0x1081, 0, 50, false).unwrap();
        assert_eq!(tile.bottom, 3);
    }

    // -----------------------------------------------------------------
    // minEndAddress: fb.addressEnd can be the binding clamp, not just the
    // raw addressEnd parameter
    // -----------------------------------------------------------------

    #[test]
    fn min_end_address_is_bound_by_framebuffer_end_not_the_raw_parameter() {
        // Hand-computed: fb.addressEnd=0x1050=4176 (much smaller than the
        // raw addressEnd parameter 0x9999=39321). minEndAddress=
        // min(39321,4176)=4176. fbBytes=4176-4096=80. fbMaxRow=80/64+
        // ((80%64)>0?1:0)=1+1=2. tileHeight=0(derive): rowEnd=
        // max((39321-4096)/64,1)=550 (using the RAW addressEnd, per the
        // "no unsigned underflow" note -- this is intentionally the raw
        // parameter, not minEndAddress), bottom=0+550=550 before
        // clamping. Clamp: bottom=min(550,fb.height=32)=32, then
        // bottom=min(32,fbMaxRow=2)=2 -- fb.addressEnd (via fbMaxRow) is
        // the binding constraint, far tighter than either fb.height or
        // the raw addressEnd parameter.
        let f = fb(0x1000, 0x1050, 1, 64, 32, 0);
        let tile = make_framebuffer_tile(&f, 0x1000, 0x9999, 0, 0, false).unwrap();
        assert_eq!(tile.bottom, 2);
    }

    // -----------------------------------------------------------------
    // Rejection #3 boundary pair at siz=3 (diversifies pixel_size beyond
    // the siz=1/siz=2 fixtures used elsewhere)
    // -----------------------------------------------------------------

    #[test]
    fn rejects_misaligned_offset_at_siz3_pixel_size_four() {
        // siz=3: pixelSize=(1<<3)>>1=4. offset=3 (0x1003-0x1000). 3%4=3
        // != 0 -> rejected.
        let f = fb(0x1000, 0x2000, 3, 64, 32, 0);
        let result = make_framebuffer_tile(&f, 0x1003, 0x2000, 0, 4, false);
        assert_eq!(result, Err(Rejection::OffsetNotPixelAligned));
    }

    #[test]
    fn accepts_offset_aligned_at_siz3_pixel_size_four() {
        // Hand-computed: siz=3,pixelSize=4,offset=4 (0x1004-0x1000). 4%4=0
        // aligned. rowBytes=64<<3>>1=256. row=4/256=0,rowOffset=4.
        // pixelShift=0(siz!=0). left=(4/4)<<0=1,top=0. lineWidth=0
        // (derive): rowRightPixels=((256-4)/4)<<0=63,right=1+63=64.
        // tileHeight=4: bottom=4. Clamp doesn't change these (all within
        // bounds). Accept: left=1,top=0,right=64,bottom=4,lineWidth=63.
        let f = fb(0x1000, 0x2000, 3, 64, 32, 0);
        let tile = make_framebuffer_tile(&f, 0x1004, 0x2000, 0, 4, false).unwrap();
        assert_eq!(tile.left, 1);
        assert_eq!(tile.right, 64);
        assert_eq!(tile.bottom, 4);
        assert_eq!(tile.line_width, 63);
    }

    // -----------------------------------------------------------------
    // left is passed through unclamped (only top/right/bottom are
    // clamped after the initial computation)
    // -----------------------------------------------------------------

    #[test]
    fn left_is_never_clamped_after_being_computed() {
        // Hand-computed: siz=1,pixelSize=1,offset=30 (0x101E-0x1000).
        // left=30 directly (row_offset/pixel_size, no shift). No clamp
        // statement in the source ever touches `left` after this point
        // (only top/right/bottom appear in the four clamp assignments) --
        // confirmed here by observing left passes through as exactly 30,
        // not silently forced to 0 or fb.width.
        let f = fb(0x1000, 0x2000, 1, 64, 32, 0);
        let tile = make_framebuffer_tile(&f, 0x101E, 0x2000, 0, 4, false).unwrap();
        assert_eq!(tile.left, 30);
    }

    // -----------------------------------------------------------------
    // rowEnd truncating division: (addressEnd - addressStart) / rowBytes
    // floors, it does not round up
    // -----------------------------------------------------------------

    #[test]
    fn row_end_derivation_truncates_not_rounds_up() {
        // Hand-computed: rowBytes=64. addressEnd-addressStart=65 (one byte
        // past exactly one row) -> rowEnd=max(65/64,1)=max(1,1)=1 (integer
        // division floors 65/64 to 1, NOT 2 -- the ceiling idiom used for
        // fbMaxRow is NOT used here). bottom=top+1=1.
        let f = fb(0x1000, 0x2000, 1, 64, 32, 0);
        let tile = make_framebuffer_tile(&f, 0x1000, 0x1041, 0, 0, false).unwrap();
        assert_eq!(tile.bottom, 1);
    }

    #[test]
    fn row_end_becomes_two_only_once_a_full_second_row_is_covered() {
        // Hand-computed: addressEnd-addressStart=128 (two full rows) ->
        // rowEnd=max(128/64,1)=2. bottom=top+2=2.
        let f = fb(0x1000, 0x2000, 1, 64, 32, 0);
        let tile = make_framebuffer_tile(&f, 0x1000, 0x1080, 0, 0, false).unwrap();
        assert_eq!(tile.bottom, 2);
    }

    // -----------------------------------------------------------------
    // Rejection enum sanity: distinct variants compare unequal, matching
    // the "assert which guard fired independently" testing aid's purpose
    // -----------------------------------------------------------------

    #[test]
    fn rejection_variants_are_pairwise_distinguishable() {
        assert_ne!(
            Rejection::AddressPastFramebufferEnd,
            Rejection::EndAtOrBeforeStart
        );
        assert_ne!(
            Rejection::OffsetNotPixelAligned,
            Rejection::MisalignedMultiRowLoadBlock
        );
        assert_ne!(
            Rejection::DegenerateBottomAtOrBeforeTop,
            Rejection::DegenerateRightAtOrBeforeLeft
        );
    }

    // -----------------------------------------------------------------
    // fb_extent equality/Debug derive sanity (used by test fixtures, and
    // proves FbExtent has no hidden interior mutability affecting equality)
    // -----------------------------------------------------------------

    #[test]
    fn fb_extent_equality_is_field_wise() {
        let a = fb(0x1000, 0x2000, 1, 64, 32, 5);
        let b = fb(0x1000, 0x2000, 1, 64, 32, 5);
        let c = fb(0x1000, 0x2000, 1, 64, 33, 5);
        assert_eq!(a, b);
        assert_ne!(a, c);
    }
}
