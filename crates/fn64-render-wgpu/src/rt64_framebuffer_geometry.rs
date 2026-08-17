//! Literal port of `RT64::Framebuffer`'s pure geometry cluster and the
//! `Framebuffer::copyNativeToRAM` RDRAM word-swap (including its `i ^ 3`
//! sub-word tail), plus `RT64::NativeTarget::getNativeSize` and
//! `RT64::FramebufferTile::valid`, a literal port of the permitted MIT RT64
//! Rust-port source pinned at commit `5473732a822a4423b5696e7cb18fecc425a59875`
//! (`docs/RT64-PORT-AUTHORITY.md`):
//!
//! - `src/hle/rt64_framebuffer.h:32-95` (whole-file SHA-256,
//!   `95e132fa28c97412d6e63e36c96c7b15df846943c3d8dd156a64da12beb479b0`, 96
//!   lines -- matching `docs/rt64-port-inventory.json`'s `sources.port.sha256`
//!   for that path, confirmed independently here by `shasum -a 256` against
//!   the pinned port-commit checkout).
//! - `src/hle/rt64_framebuffer.cpp:53-63,65-72,137-191,195-201` (whole-file
//!   SHA-256, `ce68b459aa3fe82967954a395f4990b1a8c10098f4033bad156857dd1863b36d`,
//!   202 lines -- matching the same inventory field, confirmed the same way).
//! - `src/render/rt64_native_target.cpp:58-61` (whole-file SHA-256,
//!   `c08a1d105c111eef16668253c4843aae9e4a61ee49f61e45e825168d24d66a51`, 372
//!   lines -- matching the same inventory field, confirmed the same way).
//!
//! `docs/rt64-port-inventory.json` does not yet record any of these three
//! paths' `ported_as` as pointing at this module (all three currently list
//! `"ported_as": []`) -- `scripts/lint-docs.py`'s inventory scanner is
//! expected to report a drift for that until a follow-up regenerates the
//! inventory to add this module; this module's own writable surface does not
//! include `docs/rt64-port-inventory.json`, so that reconciliation is
//! deliberately left to the owning ticket rather than done here.
//!
//! ```text
//! // rt64_framebuffer.h
//! struct Framebuffer {
//!     enum class Type {
//!         None,
//!         Color,
//!         Depth
//!     };
//!
//!     uint32_t addressStart;
//!     uint32_t addressEnd;
//!     uint8_t siz;
//!     uint32_t width;
//!     uint32_t height;
//!     uint32_t maxHeight;
//!     uint32_t readHeight;
//!     NativeTarget nativeTarget;
//!     std::vector<uint8_t> nativeSwappedRAM;
//!     FixedRect lastWriteRect;
//!     uint8_t lastWriteFmt;
//!     Type lastWriteType;
//!     uint64_t lastWriteTimestamp;
//!     uint32_t modifiedBytes;
//!     uint32_t RAMBytes;
//!     uint64_t RAMHash;
//!     std::array<uint32_t, 4> ditherPatterns;
//!     TileCopyCache tileCopyCache;
//!     bool widthChanged;
//!     bool sizChanged;
//!     bool rdramChanged;
//!     bool interpolationEnabled;
//!     bool everUsedAsDepth;
//!
//!     Framebuffer();
//!     ~Framebuffer();
//!     uint32_t imageRowBytes(uint32_t rowWidth) const;
//!     bool contains(uint32_t start, uint32_t end) const;
//!     bool overlaps(uint32_t start, uint32_t end) const;
//!     void discardLastWrite();
//!     bool isLastWriteDifferent(Framebuffer::Type newType) const;
//!     ... // (RHI/RenderWorker methods excluded -- see Nonclaims)
//!     void copyNativeToRAM(uint8_t *dst, uint32_t dstRowWidth, uint32_t dstRowStart, uint32_t dstRowEnd);
//!     void clearChanged();
//!     void addDitherPatterns(const std::array<uint32_t, 4> &extraPatterns);
//!     uint32_t bestDitherPattern() const;
//! };
//!
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
//!
//!     bool valid() const;
//!     uint64_t hash() const;
//! };
//!
//! // rt64_framebuffer.cpp
//! uint32_t Framebuffer::imageRowBytes(uint32_t rowWidth) const {
//!     return rowWidth << siz >> 1;
//! }
//!
//! bool Framebuffer::contains(uint32_t start, uint32_t end) const {
//!     return (start >= addressStart) && (end <= addressEnd);
//! }
//!
//! bool Framebuffer::overlaps(uint32_t start, uint32_t end) const {
//!     return (addressStart < end) && (addressEnd > start);
//! }
//!
//! void Framebuffer::discardLastWrite() {
//!     lastWriteType = Type::None;
//!     lastWriteRect.reset();
//! }
//!
//! bool Framebuffer::isLastWriteDifferent(Framebuffer::Type newType) const {
//!     return (lastWriteType != Type::None) && (lastWriteType != newType);
//! }
//!
//! void Framebuffer::copyNativeToRAM(uint8_t *dst, uint32_t dstRowWidth, uint32_t dstRowStart, uint32_t dstRowEnd) {
//!     assert(dst != nullptr);
//!     assert(dstRowStart < height);
//!     assert(dstRowEnd <= height);
//!
//!     // Copy native target to RDRAM.
//!     uint8_t *dstBytes = dst + dstRowStart * imageRowBytes(dstRowWidth);
//!     uint32_t *dstWords = reinterpret_cast<uint32_t *>(dstBytes);
//!     uint32_t bytesToSwap = (dstRowEnd - dstRowStart) * imageRowBytes(dstRowWidth);
//!     uint32_t dstFirstWord = dstWords[0];
//!     nativeTarget.copyToRAM(dstRowStart, dstRowEnd, dstRowWidth, siz, dstBytes);
//!
//!     // Write back to RDRAM by swapping every word.
//!     if (bytesToSwap >= sizeof(uint32_t)) {
//!         uint32_t wordsToSwap = (bytesToSwap) / sizeof(uint32_t);
//!         while (wordsToSwap > 0) {
//!             *dstWords = _byteswap_ulong(*dstWords);
//!             wordsToSwap--;
//!             dstWords++;
//!         }
//!     }
//!     // Special case when the total amount of bytes is smaller than a word.
//!     else {
//!         uint8_t *dstFirstWordU8 = reinterpret_cast<uint8_t *>(&dstFirstWord);
//!         for (uint32_t i = 0; i < bytesToSwap; i++) {
//!             dstFirstWordU8[i ^ 3] = dstBytes[i];
//!         }
//!
//!         dstWords[0] = dstFirstWord;
//!     }
//! }
//!
//! void Framebuffer::clearChanged() {
//!     widthChanged = false;
//!     sizChanged = false;
//!     rdramChanged = false;
//! }
//!
//! void Framebuffer::addDitherPatterns(const std::array<uint32_t, 4> &extraPatterns) {
//!     for (uint32_t i = 0; i < ditherPatterns.size(); i++) {
//!         ditherPatterns[i] += extraPatterns[i];
//!     }
//! }
//!
//! uint32_t Framebuffer::bestDitherPattern() const {
//!     return std::max_element(ditherPatterns.begin(), ditherPatterns.end()) - ditherPatterns.begin();
//! }
//!
//! // FramebufferTile
//!
//! bool FramebufferTile::valid() const {
//!     return (bottom > top) && (right > left);
//! }
//!
//! // rt64_native_target.cpp
//! uint32_t NativeTarget::getNativeSize(uint32_t width, uint32_t height, uint8_t siz) {
//!     const uint32_t rowSize = width << siz >> 1;
//!     return rowSize * height;
//! }
//! ```
//!
//! **Reuse, not new type.** `discard_last_write` reuses
//! [`crate::rt64_common::FixedRect`] directly for `lastWriteRect` -- no new
//! rect type, and no edit to `rt64_common.rs`. `FixedRect::reset()` is
//! already ported there with the exact same sentinel semantics
//! (`ulx`/`uly` = `i32::MAX`, `lrx`/`lry` = `i32::MIN`) this module's
//! `discard_last_write` calls into.
//!
//! ## Admitted domain
//!
//! - **The `i ^ 3` sub-word tail (hazard: get its boundary exactly right).**
//!   `copyNativeToRAM`'s `else` branch only executes when `bytesToSwap < 4`
//!   (the `if` branch handles `bytesToSwap >= 4` entirely, including
//!   non-multiple-of-4 counts -- see the next bullet). Within the tail,
//!   `dstFirstWordU8` is a **byte view of the word captured *before***
//!   `nativeTarget.copyToRAM` overwrote `dstBytes` (`uint32_t dstFirstWord =
//!   dstWords[0]` happens strictly before the `copyToRAM` call in the
//!   source). The loop then overwrites `dstFirstWordU8[i ^ 3]` with
//!   `dstBytes[i]` (the **post-copy** byte at logical position `i`) for `i`
//!   in `0..bytesToSwap`, and finally writes the whole reassembled word back
//!   to `dstWords[0]`. This module ports that exactly as
//!   [`copy_native_to_ram_tail_swap`]: it takes the pre-copy first word as
//!   `orig_first_word: [u8; 4]` and the post-copy leading bytes as a slice,
//!   and returns the reassembled 4-byte word -- it does not silently reduce
//!   this to a `memcpy` or a full word byteswap, both of which would be
//!   wrong (the `i ^ 3` indexing does *not* correspond to `u32::swap_bytes`
//!   except at exactly `bytes_to_swap == 4`, which this tail never reaches).
//!   Hand-derived boundary table (byte position `i` -> destination index
//!   `i ^ 3`, independently re-derived, not read off any implementation):
//!   `0 -> 3`, `1 -> 2`, `2 -> 1`, `3 -> 0` (never reached by this tail, since
//!   the tail only runs for `bytes_to_swap < 4`). So for `bytes_to_swap = 1`
//!   only byte 3 of the word changes (from the pre-copy original); for
//!   `bytes_to_swap = 2`, bytes 3 and 2 change; for `bytes_to_swap = 3`,
//!   bytes 3, 2, and 1 change and only byte 0 (the pre-copy original's high
//!   byte) survives untouched. See `tail_swap_one_byte_touches_only_byte_index_three`,
//!   `tail_swap_two_bytes_touches_indices_three_and_two`, and
//!   `tail_swap_three_bytes_preserves_only_original_byte_zero` below, each
//!   asserting a hand-computed expected word, not a value captured from this
//!   module's own implementation.
//! - **Non-multiple-of-4 `bytesToSwap` in the `>= 4` branch is a real,
//!   upstream silent-truncation frontier -- reported here, not silently
//!   patched.** `wordsToSwap = bytesToSwap / sizeof(uint32_t)` is C++
//!   integer division; for `bytesToSwap` in `{5, 6, 7}` this yields
//!   `wordsToSwap = 1` (`5/4 = 6/4 = 7/4 = 1` in truncating integer
//!   division), so only the first 4 bytes get word-swapped and the
//!   remaining 1-3 trailing bytes are **not touched by either branch**: the
//!   `else` (tail) branch is only reachable when `bytesToSwap < 4`, so it
//!   never fires for `bytesToSwap` in `{5, 6, 7}`. This module's
//!   [`copy_native_to_ram_word_swap`] preserves that division exactly
//!   (`bytes_to_swap / 4`, not `bytes_to_swap.div_ceil(4)` or any rounding
//!   variant) and returns only the count of whole words actually swapped, so
//!   a caller can observe the same leftover-byte gap RT64 itself has; it does
//!   not invent a fix. This is a genuine divide-related frontier named by
//!   this port's hazard list, not a divide-by-zero (the divisor `4` is a
//!   compile-time constant, never zero).
//! - **`imageRowBytes`/`getNativeSize`'s `rowWidth << siz >> 1` is preserved
//!   as left-shift-then-right-shift, never rewritten as
//!   `rowWidth * (1 << siz) / 2` or any algebraically-equivalent-looking
//!   division.** `siz` is `Framebuffer::siz` / `NativeTarget::getNativeSize`'s
//!   third parameter -- RDP's `G_IM_SIZ_*` **shift-amount** encoding (`0` =
//!   4-bit, `1` = 8-bit, `2` = 16-bit, `3` = 32-bit pixels; RT64 uses it as a
//!   raw bit-shift, not a byte or pixel *count*), never the `65536.0`,
//!   `65535.0`, or `1023.0` scale constants named in this port's hazard list
//!   -- none of those three appear anywhere in this cluster's source; this
//!   admitted-domain note exists precisely to record that they were checked
//!   for and are absent, not assumed absent. Operand order matters at
//!   `siz == 0`: `(rowWidth << 0) >> 1 == rowWidth >> 1` truncates an odd
//!   `rowWidth` down (e.g. `rowWidth = 1` at `siz = 0` yields `0`, tested in
//!   `image_row_bytes_siz0_one_pixel_truncates_to_zero_bytes` below) --
//!   preserved exactly, not rounded up.
//! - **`getNativeSize`'s `rowSize * height` multiplication can overflow
//!   `u32` -- reported, not guarded.** RT64's C++ `uint32_t` multiply wraps
//!   silently on overflow (unsigned overflow is well-defined wraparound in
//!   C++, unlike signed overflow). This port's [`get_native_size`] uses
//!   plain `*`, which is Rust's own debug-overflow-checks/release-wraparound
//!   convention (panics in a debug build, wraps in release) -- the same
//!   admitted-domain choice `rt64_common.rs`'s `scaled` makes for its `<< 2`
//!   (see that module's doc), not a guard this module invents. No test
//!   exercises the overflow case itself (a debug-build overflow panic is a
//!   build-profile-dependent property, out of this port's characterization
//!   scope, matching `rt64_common.rs`'s precedent for `debug_assert!`-gated
//!   paths).
//! - **`addDitherPatterns`'s `ditherPatterns[i] += extraPatterns[i]` can also
//!   overflow `u32` -- same admitted choice, same reasoning, not tested for
//!   the overflow case itself** (only the accumulate-in-place behavior at
//!   non-overflowing magnitudes is characterized).
//! - **`bestDitherPattern` returns the index of the first maximum, matching
//!   `std::max_element`'s documented "first largest element" tie-breaking**
//!   (`std::max_element` compares with `<` and keeps the first element for
//!   which no later element is strictly greater) -- not the count of the
//!   maximum value, and not the last index on a tie. `best_dither_pattern`
//!   below scans left-to-right and only replaces the current best on a
//!   *strictly greater* value, matching that tie-breaking rule exactly; see
//!   `best_dither_pattern_ties_prefer_the_first_index` below, asserting a
//!   hand-picked index, not a captured one.
//! - **`bestDitherPattern` on an all-zero `ditherPatterns` (the freshly
//!   constructed state, per `Framebuffer::Framebuffer()`'s
//!   `ditherPatterns.fill(0)`) returns index `0`** -- the first element is
//!   already the max of an all-equal array, so this is not a special case in
//!   either the C++ or this port, just the ordinary first-max-wins rule
//!   applied to a degenerate input; asserted explicitly in
//!   `best_dither_pattern_all_zero_returns_index_zero` since it is the
//!   state every real `Framebuffer` starts in.
//! - **No divide-by-zero frontier exists in this cluster.** The only integer
//!   divisions ported here are `bytes_to_swap / 4` (`4` is
//!   `size_of::<u32>()`, a compile-time constant) and the bit-shift-based
//!   `imageRowBytes`/`getNativeSize` (`>> 1`, also a compile-time shift
//!   amount, not a runtime divisor) -- neither has a caller-controlled
//!   divisor, so there is nothing to report under the "divide-by-zero"
//!   hazard beyond confirming its absence.
//! - **No private-helper visibility gap was hit.** Every symbol this cluster
//!   needs (`FixedRect` and its `pub fn reset`) is already `pub` on
//!   `rt64_common.rs`'s public surface; nothing here needed reaching into a
//!   private helper or silently re-deriving one.
//!
//! ## Nonclaims
//!
//! No GPU, WGSL, or production wiring (this module is not called from
//! anywhere yet; dead-code warnings on the unused public surface are
//! expected and correct), and no RT64 visual/pixel/silicon parity or
//! performance claim. Deliberately not ported from this cluster:
//!
//! - `Framebuffer::copyRAMToNativeAndChanges`'s *other* word-swap loop (the
//!   `src -> nativeSwappedRAM` direction, `_byteswap_ulong(*srcWords)` over
//!   exactly `nativeSize / sizeof(uint32_t)` words with **no tail case at
//!   all**) is a distinct call site from `copyNativeToRAM`'s swap-back and is
//!   not part of this ticket's named cluster (`imageRowBytes`,
//!   `contains`/`overlaps`, `discardLastWrite`, `isLastWriteDifferent`,
//!   `clearChanged`, `addDitherPatterns`, `bestDitherPattern`,
//!   `FramebufferTile::valid`, `NativeTarget::getNativeSize`, and "both
//!   byte-swap loops" naming only `copyNativeToRAM`'s two branches) -- it
//!   also drives `nativeTarget.copyFromRAM`, which is RHI plumbing this
//!   ticket's reject list excludes.
//! - `Framebuffer::copyRAMToNativeAndChanges`, `readChangeFromBytes`,
//!   `readChangeFromStorage`, `copyRenderTargetToNative` (RenderWorker/RHI
//!   methods -- explicitly excluded by this ticket).
//! - `NativeTarget::copyFromRAM`/`copyToRAM`/`copyToNative`/`getBufferFormat`
//!   and all other `NativeTarget` methods except `getNativeSize` (RHI
//!   buffer/descriptor/barrier plumbing -- explicitly excluded).
//! - `FramebufferTile::hash()` (`XXH3_64bits(this, sizeof(FramebufferTile))`)
//!   -- explicitly excluded by this ticket: it hashes raw struct bytes via
//!   XXH3, and XXH3 is a standing triage reject for this port program.
//! - `Framebuffer`'s constructor/destructor, `TileCopyCache::update`, and
//!   every non-listed field (`nativeTarget`, `nativeSwappedRAM`,
//!   `tileCopyCache`, `RAMHash`, timestamps) -- out of this cluster's named
//!   scope; `Framebuffer` here is represented only as the minimal set of
//!   owned fields each ported method actually reads or writes (`address_start`/
//!   `address_end` for `contains`/`overlaps`; `siz` for `image_row_bytes`;
//!   `last_write_type`/`last_write_rect` for `discard_last_write`/
//!   `is_last_write_different`; `width_changed`/`siz_changed`/`rdram_changed`
//!   for `clear_changed`; `dither_patterns` for `add_dither_patterns`/
//!   `best_dither_pattern`), not a full mirror of the C++ struct's ~20
//!   fields.
//! - The `#ifdef DUMP_RAW_RDRAM` debug-dump block in `copyNativeToRAM`
//!   (compiled out by default, `NDEBUG`-adjacent developer tooling with no
//!   portable behavior to characterize).

/// Owned subset of `RT64::Framebuffer`'s fields this module's methods read
/// or write -- see module doc "Nonclaims" for why this is not a full mirror
/// of the C++ struct.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FramebufferGeometry {
    pub address_start: u32,
    pub address_end: u32,
    pub siz: u8,
    pub last_write_type: FramebufferType,
    pub last_write_rect: crate::rt64_common::FixedRect,
    pub width_changed: bool,
    pub siz_changed: bool,
    pub rdram_changed: bool,
    pub dither_patterns: [u32; 4],
}

/// `Framebuffer::Type` (`rt64_framebuffer.h:33-37`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FramebufferType {
    None,
    Color,
    Depth,
}

impl FramebufferGeometry {
    /// `imageRowBytes(rowWidth)` (`rt64_framebuffer.cpp:53-55`):
    /// `rowWidth << siz >> 1`. Left-shift by `siz`, then right-shift by 1 --
    /// preserved in that exact order (see module doc "Admitted domain").
    pub fn image_row_bytes(&self, row_width: u32) -> u32 {
        (row_width << self.siz) >> 1
    }

    /// `contains(start, end)` (`rt64_framebuffer.cpp:57-59`): `start` and
    /// `end` both fall within `[addressStart, addressEnd]` inclusive.
    pub fn contains(&self, start: u32, end: u32) -> bool {
        (start >= self.address_start) && (end <= self.address_end)
    }

    /// `overlaps(start, end)` (`rt64_framebuffer.cpp:61-63`): strict-interval
    /// overlap test.
    pub fn overlaps(&self, start: u32, end: u32) -> bool {
        (self.address_start < end) && (self.address_end > start)
    }

    /// `discardLastWrite()` (`rt64_framebuffer.cpp:65-68`): resets the last
    /// write's type to `None` and its rect to `FixedRect`'s null sentinel
    /// (see module doc "Reuse, not new type").
    pub fn discard_last_write(&mut self) {
        self.last_write_type = FramebufferType::None;
        self.last_write_rect.reset();
    }

    /// `isLastWriteDifferent(newType)` (`rt64_framebuffer.cpp:70-72`): true
    /// iff there *was* a last write (type is not `None`) and its type
    /// differs from `new_type`.
    pub fn is_last_write_different(&self, new_type: FramebufferType) -> bool {
        (self.last_write_type != FramebufferType::None) && (self.last_write_type != new_type)
    }

    /// `clearChanged()` (`rt64_framebuffer.cpp:177-181`): clears all three
    /// change-tracking flags.
    pub fn clear_changed(&mut self) {
        self.width_changed = false;
        self.siz_changed = false;
        self.rdram_changed = false;
    }

    /// `addDitherPatterns(extraPatterns)` (`rt64_framebuffer.cpp:183-187`):
    /// element-wise in-place accumulate. See module doc "Admitted domain"
    /// for the unguarded-overflow note.
    pub fn add_dither_patterns(&mut self, extra_patterns: &[u32; 4]) {
        for i in 0..self.dither_patterns.len() {
            self.dither_patterns[i] += extra_patterns[i];
        }
    }

    /// `bestDitherPattern()` (`rt64_framebuffer.cpp:189-191`): index of the
    /// first maximum element, matching `std::max_element`'s tie-breaking
    /// (see module doc "Admitted domain").
    pub fn best_dither_pattern(&self) -> u32 {
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

/// `FramebufferTile::valid()` (`rt64_framebuffer.cpp:195-197`): a rectangle
/// with strictly positive width and height. Takes the four bounds directly
/// rather than a full `FramebufferTile` mirror (see module doc "Nonclaims").
pub fn framebuffer_tile_valid(left: u32, top: u32, right: u32, bottom: u32) -> bool {
    (bottom > top) && (right > left)
}

/// `NativeTarget::getNativeSize(width, height, siz)`
/// (`rt64_native_target.cpp:58-61`): `(width << siz >> 1) * height`. Same
/// row-bytes shift-then-shift as [`FramebufferGeometry::image_row_bytes`],
/// then multiplied by `height` -- preserved as `row_size * height`, not
/// `width * height << siz >> 1` or any reassociated form (see module doc
/// "Admitted domain" for the unguarded-overflow note on this multiply).
pub fn get_native_size(width: u32, height: u32, siz: u8) -> u32 {
    let row_size = (width << siz) >> 1;
    row_size * height
}

/// `copyNativeToRAM`'s full-word swap-back loop
/// (`rt64_framebuffer.cpp:150-157`), the `bytesToSwap >= sizeof(uint32_t)`
/// branch: reverses the byte order of each complete 4-byte word in `buf`, in
/// place, for exactly `bytes_to_swap / 4` words (C++ integer division,
/// preserved exactly -- see module doc "Admitted domain" for the
/// non-multiple-of-4 leftover-byte frontier this creates). `buf` must be at
/// least `bytes_to_swap` bytes long; only the first `(bytes_to_swap / 4) * 4`
/// bytes of it are touched. Returns the number of whole words swapped.
pub fn copy_native_to_ram_word_swap(buf: &mut [u8], bytes_to_swap: u32) -> u32 {
    let words_to_swap = bytes_to_swap / 4;
    for w in 0..words_to_swap as usize {
        let base = w * 4;
        buf.swap(base, base + 3);
        buf.swap(base + 1, base + 2);
    }
    words_to_swap
}

/// `copyNativeToRAM`'s sub-word swap-back tail (`rt64_framebuffer.cpp:159-166`),
/// the `else` branch reached only when `bytes_to_swap < sizeof(uint32_t)`
/// (`4`): reassembles the word captured *before* `nativeTarget.copyToRAM`
/// overwrote the destination (`orig_first_word`) with `bytes_to_swap` bytes
/// from the *post-copy* buffer (`post_copy_leading_bytes`), each placed at
/// `i ^ 3` -- see module doc "Admitted domain" for the exact `i -> i^3`
/// boundary table and why this is not a `u32::swap_bytes`/`memcpy`. Panics
/// if `bytes_to_swap >= 4` (the source's own `if`/`else` never reaches this
/// path at `bytes_to_swap >= 4`, and `post_copy_leading_bytes` must supply at
/// least `bytes_to_swap` bytes) or if `post_copy_leading_bytes` is shorter
/// than `bytes_to_swap`.
pub fn copy_native_to_ram_tail_swap(
    orig_first_word: [u8; 4],
    post_copy_leading_bytes: &[u8],
    bytes_to_swap: u32,
) -> [u8; 4] {
    assert!(
        bytes_to_swap < 4,
        "copy_native_to_ram_tail_swap is only reachable for bytes_to_swap < 4, matching RT64's \
         `else` branch of the `bytesToSwap >= sizeof(uint32_t)` check -- got {bytes_to_swap}"
    );
    assert!(
        post_copy_leading_bytes.len() >= bytes_to_swap as usize,
        "post_copy_leading_bytes too short: need at least {bytes_to_swap} bytes, got {}",
        post_copy_leading_bytes.len()
    );

    let mut buf = orig_first_word;
    for i in 0..bytes_to_swap as usize {
        buf[i ^ 3] = post_copy_leading_bytes[i];
    }
    buf
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rt64_common::FixedRect;

    fn geometry(address_start: u32, address_end: u32) -> FramebufferGeometry {
        FramebufferGeometry {
            address_start,
            address_end,
            siz: 2,
            last_write_type: FramebufferType::None,
            last_write_rect: FixedRect::new(),
            width_changed: false,
            siz_changed: false,
            rdram_changed: false,
            dither_patterns: [0; 4],
        }
    }

    // --- image_row_bytes: all four siz values, zero/one-pixel edges ---

    #[test]
    fn image_row_bytes_siz0_zero_width_is_zero() {
        let g = geometry(0, 0);
        assert_eq!(g.image_row_bytes(0), 0);
    }

    #[test]
    fn image_row_bytes_siz0_one_pixel_truncates_to_zero_bytes() {
        // (1 << 0) >> 1 = 1 >> 1 = 0 -- integer right-shift truncates.
        let mut g = geometry(0, 0);
        g.siz = 0;
        assert_eq!(g.image_row_bytes(1), 0);
    }

    #[test]
    fn image_row_bytes_siz0_two_pixels_is_one_byte() {
        let mut g = geometry(0, 0);
        g.siz = 0;
        assert_eq!(g.image_row_bytes(2), 1);
    }

    #[test]
    fn image_row_bytes_siz0_320_width() {
        // (320 << 0) >> 1 = 160.
        let mut g = geometry(0, 0);
        g.siz = 0;
        assert_eq!(g.image_row_bytes(320), 160);
    }

    #[test]
    fn image_row_bytes_siz1_one_pixel_is_one_byte() {
        // (1 << 1) >> 1 = 2 >> 1 = 1.
        let mut g = geometry(0, 0);
        g.siz = 1;
        assert_eq!(g.image_row_bytes(1), 1);
    }

    #[test]
    fn image_row_bytes_siz1_zero_width_is_zero() {
        let mut g = geometry(0, 0);
        g.siz = 1;
        assert_eq!(g.image_row_bytes(0), 0);
    }

    #[test]
    fn image_row_bytes_siz1_320_width() {
        // (320 << 1) >> 1 = 640 >> 1 = 320.
        let mut g = geometry(0, 0);
        g.siz = 1;
        assert_eq!(g.image_row_bytes(320), 320);
    }

    #[test]
    fn image_row_bytes_siz2_one_pixel_is_two_bytes() {
        // (1 << 2) >> 1 = 4 >> 1 = 2.
        let mut g = geometry(0, 0);
        g.siz = 2;
        assert_eq!(g.image_row_bytes(1), 2);
    }

    #[test]
    fn image_row_bytes_siz2_320_width() {
        // (320 << 2) >> 1 = 1280 >> 1 = 640.
        let mut g = geometry(0, 0);
        g.siz = 2;
        assert_eq!(g.image_row_bytes(320), 640);
    }

    #[test]
    fn image_row_bytes_siz3_one_pixel_is_four_bytes() {
        // (1 << 3) >> 1 = 8 >> 1 = 4.
        let mut g = geometry(0, 0);
        g.siz = 3;
        assert_eq!(g.image_row_bytes(1), 4);
    }

    #[test]
    fn image_row_bytes_siz3_320_width() {
        // (320 << 3) >> 1 = 2560 >> 1 = 1280.
        let mut g = geometry(0, 0);
        g.siz = 3;
        assert_eq!(g.image_row_bytes(320), 1280);
    }

    #[test]
    fn image_row_bytes_operand_order_left_shift_before_right_shift() {
        // A wrong "rowWidth * (1<<siz) / 2" reassociation would agree with
        // the source at even rowWidth but diverge at odd rowWidth for
        // siz==0 -- both formulas actually agree here (1*1/2==0==1>>1), so
        // this test instead pins the *exact* shift-then-shift result at an
        // odd width and siz==1, where a naive "rowWidth >> (1 - siz)" or
        // similar rewrite would diverge: (5 << 1) >> 1 = 10 >> 1 = 5.
        let mut g = geometry(0, 0);
        g.siz = 1;
        assert_eq!(g.image_row_bytes(5), 5);
    }

    // --- contains ---

    #[test]
    fn contains_fully_inside_range() {
        let g = geometry(100, 200);
        assert!(g.contains(120, 180));
    }

    #[test]
    fn contains_exact_boundary_start_is_inclusive() {
        let g = geometry(100, 200);
        assert!(g.contains(100, 150));
    }

    #[test]
    fn contains_exact_boundary_end_is_inclusive() {
        let g = geometry(100, 200);
        assert!(g.contains(150, 200));
    }

    #[test]
    fn contains_exact_full_range_is_true() {
        let g = geometry(100, 200);
        assert!(g.contains(100, 200));
    }

    #[test]
    fn contains_start_one_below_lower_bound_is_false() {
        let g = geometry(100, 200);
        assert!(!g.contains(99, 150));
    }

    #[test]
    fn contains_end_one_above_upper_bound_is_false() {
        let g = geometry(100, 200);
        assert!(!g.contains(150, 201));
    }

    #[test]
    fn contains_zero_length_range_at_start_is_true() {
        let g = geometry(100, 200);
        assert!(g.contains(100, 100));
    }

    // --- overlaps ---

    #[test]
    fn overlaps_true_for_partial_overlap() {
        let g = geometry(100, 200);
        assert!(g.overlaps(150, 250));
        assert!(g.overlaps(50, 150));
    }

    #[test]
    fn overlaps_true_when_query_fully_contains_range() {
        let g = geometry(100, 200);
        assert!(g.overlaps(0, 1000));
    }

    #[test]
    fn overlaps_touching_at_end_boundary_is_false() {
        // addressStart < end (100 < 200 true) but addressEnd > start
        // (200 > 200 false) -- strict inequality on both sides means
        // exactly-touching ranges do NOT overlap.
        let g = geometry(100, 200);
        assert!(!g.overlaps(200, 300));
    }

    #[test]
    fn overlaps_touching_at_start_boundary_is_false() {
        // addressStart < end (100 < 100 false).
        let g = geometry(100, 200);
        assert!(!g.overlaps(0, 100));
    }

    #[test]
    fn overlaps_one_unit_past_touching_boundary_is_true() {
        let g = geometry(100, 200);
        assert!(g.overlaps(199, 300));
        assert!(g.overlaps(0, 101));
    }

    #[test]
    fn overlaps_disjoint_range_is_false() {
        let g = geometry(100, 200);
        assert!(!g.overlaps(300, 400));
        assert!(!g.overlaps(0, 50));
    }

    #[test]
    fn overlaps_zero_length_query_never_overlaps() {
        // start == end means addressStart < end AND addressEnd > start can
        // both hold only if the point falls strictly inside (100,200), e.g.
        // (150,150): addressStart(100) < 150 true, addressEnd(200) > 150
        // true -- so a zero-length range strictly inside DOES overlap.
        let g = geometry(100, 200);
        assert!(g.overlaps(150, 150));
        // But a zero-length range at the exact boundary does not.
        assert!(!g.overlaps(100, 100));
        assert!(!g.overlaps(200, 200));
    }

    // --- discard_last_write / is_last_write_different ---

    #[test]
    fn discard_last_write_resets_type_to_none() {
        let mut g = geometry(0, 0);
        g.last_write_type = FramebufferType::Color;
        g.discard_last_write();
        assert_eq!(g.last_write_type, FramebufferType::None);
    }

    #[test]
    fn discard_last_write_resets_rect_to_null_sentinel() {
        let mut g = geometry(0, 0);
        g.last_write_rect = FixedRect::with_bounds(1, 2, 3, 4);
        g.discard_last_write();
        assert!(g.last_write_rect.is_null());
        assert_eq!(g.last_write_rect, FixedRect::new());
    }

    #[test]
    fn is_last_write_different_false_when_last_write_is_none() {
        let g = geometry(0, 0);
        assert!(!g.is_last_write_different(FramebufferType::Color));
        assert!(!g.is_last_write_different(FramebufferType::Depth));
    }

    #[test]
    fn is_last_write_different_false_when_types_match() {
        let mut g = geometry(0, 0);
        g.last_write_type = FramebufferType::Color;
        assert!(!g.is_last_write_different(FramebufferType::Color));
    }

    #[test]
    fn is_last_write_different_true_when_types_differ() {
        let mut g = geometry(0, 0);
        g.last_write_type = FramebufferType::Color;
        assert!(g.is_last_write_different(FramebufferType::Depth));
    }

    // --- clear_changed ---

    #[test]
    fn clear_changed_resets_all_three_flags() {
        let mut g = geometry(0, 0);
        g.width_changed = true;
        g.siz_changed = true;
        g.rdram_changed = true;
        g.clear_changed();
        assert!(!g.width_changed);
        assert!(!g.siz_changed);
        assert!(!g.rdram_changed);
    }

    #[test]
    fn clear_changed_is_a_no_op_when_already_clear() {
        let mut g = geometry(0, 0);
        g.clear_changed();
        assert!(!g.width_changed);
        assert!(!g.siz_changed);
        assert!(!g.rdram_changed);
    }

    // --- add_dither_patterns ---

    #[test]
    fn add_dither_patterns_accumulates_elementwise() {
        let mut g = geometry(0, 0);
        g.dither_patterns = [1, 2, 3, 4];
        g.add_dither_patterns(&[10, 20, 30, 40]);
        assert_eq!(g.dither_patterns, [11, 22, 33, 44]);
    }

    #[test]
    fn add_dither_patterns_zero_extra_is_a_no_op() {
        let mut g = geometry(0, 0);
        g.dither_patterns = [5, 6, 7, 8];
        g.add_dither_patterns(&[0, 0, 0, 0]);
        assert_eq!(g.dither_patterns, [5, 6, 7, 8]);
    }

    #[test]
    fn add_dither_patterns_repeated_calls_keep_accumulating() {
        let mut g = geometry(0, 0);
        g.add_dither_patterns(&[1, 1, 1, 1]);
        g.add_dither_patterns(&[1, 1, 1, 1]);
        g.add_dither_patterns(&[1, 1, 1, 1]);
        assert_eq!(g.dither_patterns, [3, 3, 3, 3]);
    }

    // --- best_dither_pattern ---

    #[test]
    fn best_dither_pattern_all_zero_returns_index_zero() {
        let g = geometry(0, 0);
        assert_eq!(g.best_dither_pattern(), 0);
    }

    #[test]
    fn best_dither_pattern_unique_max_at_each_index() {
        let mut g = geometry(0, 0);
        g.dither_patterns = [1, 9, 2, 3];
        assert_eq!(g.best_dither_pattern(), 1);

        g.dither_patterns = [9, 1, 2, 3];
        assert_eq!(g.best_dither_pattern(), 0);

        g.dither_patterns = [1, 2, 9, 3];
        assert_eq!(g.best_dither_pattern(), 2);

        g.dither_patterns = [1, 2, 3, 9];
        assert_eq!(g.best_dither_pattern(), 3);
    }

    #[test]
    fn best_dither_pattern_ties_prefer_the_first_index() {
        // std::max_element keeps the first element when no later element is
        // strictly greater -- a tie at indices 1 and 3 must resolve to 1.
        let mut g = geometry(0, 0);
        g.dither_patterns = [0, 9, 0, 9];
        assert_eq!(g.best_dither_pattern(), 1);
    }

    #[test]
    fn best_dither_pattern_all_equal_nonzero_returns_index_zero() {
        let mut g = geometry(0, 0);
        g.dither_patterns = [7, 7, 7, 7];
        assert_eq!(g.best_dither_pattern(), 0);
    }

    // --- framebuffer_tile_valid ---

    #[test]
    fn framebuffer_tile_valid_positive_area_is_valid() {
        assert!(framebuffer_tile_valid(0, 0, 10, 10));
    }

    #[test]
    fn framebuffer_tile_valid_zero_width_is_invalid() {
        // right == left: right > left is false.
        assert!(!framebuffer_tile_valid(5, 0, 5, 10));
    }

    #[test]
    fn framebuffer_tile_valid_zero_height_is_invalid() {
        assert!(!framebuffer_tile_valid(0, 5, 10, 5));
    }

    #[test]
    fn framebuffer_tile_valid_inverted_bounds_is_invalid() {
        assert!(!framebuffer_tile_valid(10, 10, 0, 0));
    }

    #[test]
    fn framebuffer_tile_valid_one_pixel_tile_is_valid() {
        // right=left+1, bottom=top+1: strictly greater on both axes.
        assert!(framebuffer_tile_valid(0, 0, 1, 1));
    }

    #[test]
    fn framebuffer_tile_valid_zero_width_and_height_is_invalid() {
        assert!(!framebuffer_tile_valid(3, 3, 3, 3));
    }

    // --- get_native_size ---

    #[test]
    fn get_native_size_zero_height_is_zero() {
        assert_eq!(get_native_size(320, 0, 2), 0);
    }

    #[test]
    fn get_native_size_zero_width_is_zero() {
        assert_eq!(get_native_size(0, 240, 2), 0);
    }

    #[test]
    fn get_native_size_one_pixel_dimensions_siz2() {
        // rowSize = (1<<2)>>1 = 2; * 1 height = 2.
        assert_eq!(get_native_size(1, 1, 2), 2);
    }

    #[test]
    fn get_native_size_matches_row_bytes_times_height_siz0() {
        // rowSize = (320<<0)>>1 = 160; * 240 = 38400.
        assert_eq!(get_native_size(320, 240, 0), 38_400);
    }

    #[test]
    fn get_native_size_matches_row_bytes_times_height_siz2() {
        // rowSize = (320<<2)>>1 = 640; * 240 = 153600.
        assert_eq!(get_native_size(320, 240, 2), 153_600);
    }

    #[test]
    fn get_native_size_matches_row_bytes_times_height_siz3() {
        // rowSize = (320<<3)>>1 = 1280; * 240 = 307200.
        assert_eq!(get_native_size(320, 240, 3), 307_200);
    }

    #[test]
    fn get_native_size_consistent_with_image_row_bytes_helper() {
        let mut g = geometry(0, 0);
        g.siz = 2;
        let width = 640u32;
        let height = 480u32;
        assert_eq!(
            get_native_size(width, height, g.siz),
            g.image_row_bytes(width) * height
        );
    }

    // --- copy_native_to_ram_word_swap ---

    #[test]
    fn word_swap_single_word_reverses_bytes() {
        let mut buf = [0xAAu8, 0xBB, 0xCC, 0xDD];
        let words = copy_native_to_ram_word_swap(&mut buf, 4);
        assert_eq!(words, 1);
        assert_eq!(buf, [0xDD, 0xCC, 0xBB, 0xAA]);
    }

    #[test]
    fn word_swap_two_words_reverses_each_independently() {
        let mut buf = [0x01u8, 0x02, 0x03, 0x04, 0x11, 0x22, 0x33, 0x44];
        let words = copy_native_to_ram_word_swap(&mut buf, 8);
        assert_eq!(words, 2);
        assert_eq!(buf, [0x04, 0x03, 0x02, 0x01, 0x44, 0x33, 0x22, 0x11]);
    }

    #[test]
    fn word_swap_zero_bytes_is_a_no_op() {
        let mut buf = [0xAAu8, 0xBB, 0xCC, 0xDD];
        let words = copy_native_to_ram_word_swap(&mut buf, 0);
        assert_eq!(words, 0);
        assert_eq!(buf, [0xAA, 0xBB, 0xCC, 0xDD]);
    }

    #[test]
    fn word_swap_non_multiple_of_four_truncates_via_integer_division() {
        // bytes_to_swap=7: 7/4=1 word swapped (integer division), matching
        // RT64's own C++ truncation -- the trailing 3 bytes are untouched by
        // this function (they're also never reached by the tail helper,
        // since the source's else-branch only runs for bytesToSwap < 4).
        let mut buf = [0x01u8, 0x02, 0x03, 0x04, 0x99, 0x99, 0x99];
        let words = copy_native_to_ram_word_swap(&mut buf, 7);
        assert_eq!(words, 1);
        assert_eq!(buf, [0x04, 0x03, 0x02, 0x01, 0x99, 0x99, 0x99]);
    }

    #[test]
    fn word_swap_matches_u32_swap_bytes_at_exactly_one_word() {
        let value: u32 = 0x1234_5678;
        let mut buf = value.to_le_bytes();
        copy_native_to_ram_word_swap(&mut buf, 4);
        assert_eq!(u32::from_le_bytes(buf), value.swap_bytes());
    }

    // --- copy_native_to_ram_tail_swap: the i^3 tail, pinned exactly ---

    #[test]
    fn tail_swap_zero_bytes_leaves_original_word_untouched() {
        let orig = [0xAAu8, 0xBB, 0xCC, 0xDD];
        let post = [0x11u8, 0x22, 0x33, 0x44];
        let result = copy_native_to_ram_tail_swap(orig, &post, 0);
        assert_eq!(result, orig);
    }

    #[test]
    fn tail_swap_one_byte_touches_only_byte_index_three() {
        // i=0 -> dest index 0^3=3. Only buf[3] changes; buf[0..3] retain the
        // pre-copy original bytes.
        let orig = [0xAAu8, 0xBB, 0xCC, 0xDD];
        let post = [0x11u8, 0x22, 0x33, 0x44];
        let result = copy_native_to_ram_tail_swap(orig, &post, 1);
        assert_eq!(result, [0xAA, 0xBB, 0xCC, 0x11]);
    }

    #[test]
    fn tail_swap_two_bytes_touches_indices_three_and_two() {
        // i=0 -> dest 3 = post[0]; i=1 -> dest 1^3=2 = post[1].
        let orig = [0xAAu8, 0xBB, 0xCC, 0xDD];
        let post = [0x11u8, 0x22, 0x33, 0x44];
        let result = copy_native_to_ram_tail_swap(orig, &post, 2);
        assert_eq!(result, [0xAA, 0xBB, 0x22, 0x11]);
    }

    #[test]
    fn tail_swap_three_bytes_preserves_only_original_byte_zero() {
        // i=0 -> dest 3 = post[0]; i=1 -> dest 2 = post[1]; i=2 -> dest
        // 2^3=1 = post[2]. Only buf[0] (orig's own high byte) survives.
        let orig = [0xAAu8, 0xBB, 0xCC, 0xDD];
        let post = [0x11u8, 0x22, 0x33, 0x44];
        let result = copy_native_to_ram_tail_swap(orig, &post, 3);
        assert_eq!(result, [0xAA, 0x33, 0x22, 0x11]);
    }

    #[test]
    fn tail_swap_reads_post_copy_bytes_not_original_bytes() {
        // Confirms the tail reads dstBytes (post-copy content), not the
        // saved dstFirstWord, for the bytes it does touch -- give orig and
        // post fully disjoint values to catch an accidental swap of which
        // buffer is the source of truth.
        let orig = [0x00u8, 0x00, 0x00, 0x00];
        let post = [0xFFu8, 0xEE, 0xDD, 0xCC];
        let result = copy_native_to_ram_tail_swap(orig, &post, 1);
        assert_eq!(result, [0x00, 0x00, 0x00, 0xFF]);
    }

    #[test]
    fn tail_swap_preserves_untouched_original_bytes_exactly() {
        // At bytes_to_swap=2, orig[0] and orig[1] must survive unchanged
        // (only indices 3 and 2 are written).
        let orig = [0x77u8, 0x88, 0x99, 0xAA];
        let post = [0x00u8, 0x00, 0x00, 0x00];
        let result = copy_native_to_ram_tail_swap(orig, &post, 2);
        assert_eq!(result[0], 0x77);
        assert_eq!(result[1], 0x88);
    }

    #[test]
    #[should_panic(expected = "only reachable for bytes_to_swap < 4")]
    fn tail_swap_panics_at_bytes_to_swap_four() {
        // The source's own if/else means this path is never taken at
        // bytesToSwap >= 4 -- pinned as a loud panic, not silently handled.
        let orig = [0u8; 4];
        let post = [0u8; 4];
        let _ = copy_native_to_ram_tail_swap(orig, &post, 4);
    }

    #[test]
    #[should_panic(expected = "too short")]
    fn tail_swap_panics_when_post_copy_slice_too_short() {
        let orig = [0u8; 4];
        let post = [0u8; 1];
        let _ = copy_native_to_ram_tail_swap(orig, &post, 2);
    }

    // --- combined word-swap + tail: full copy_native_to_ram shape at each
    //     residual byte count, cross-checked against an independent
    //     from-scratch byte-position simulation (not this module's own code).

    fn independent_expected_word(orig: [u8; 4], post: &[u8], bytes_to_swap: u32) -> [u8; 4] {
        // Re-derive the C++ semantics from the doc comment alone, using a
        // different code shape (explicit match) than the module's loop.
        let mut out = orig;
        match bytes_to_swap {
            0 => {}
            1 => out[3] = post[0],
            2 => {
                out[3] = post[0];
                out[2] = post[1];
            }
            3 => {
                out[3] = post[0];
                out[2] = post[1];
                out[1] = post[2];
            }
            _ => panic!("only defined for bytes_to_swap in 0..=3"),
        }
        out
    }

    #[test]
    fn tail_swap_matches_independent_simulation_at_every_residual_count() {
        let orig = [0x10u8, 0x20, 0x30, 0x40];
        let post = [0x91u8, 0x92, 0x93, 0x94];
        for n in 0..4u32 {
            let expected = independent_expected_word(orig, &post, n);
            let actual = copy_native_to_ram_tail_swap(orig, &post, n);
            assert_eq!(actual, expected, "bytes_to_swap={n}");
        }
    }
}
