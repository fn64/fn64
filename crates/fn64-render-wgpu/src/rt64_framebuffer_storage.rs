//! Literal port of `RT64::FramebufferStorage`: the append-only RDRAM arena
//! with its `(rdramUsed * 3) / 2` growth policy and `get()`'s
//! last-qualifying-handle-wins lookup, a literal port of the permitted MIT
//! RT64 Rust-port source pinned at commit
//! `5473732a822a4423b5696e7cb18fecc425a59875` (`docs/RT64-PORT-AUTHORITY.md`):
//!
//! - `src/hle/rt64_framebuffer_storage.h` (whole file, lines 1-35; whole-file
//!   SHA-256 `767cc9eec63e1684cfa419d8bd3f0a3c8cb7ce5834515b7cf2f0d63be79ad0e6`
//!   -- matching `docs/rt64-port-inventory.json`'s `sources.port.sha256` for
//!   that path, confirmed independently here by `shasum -a 256` against the
//!   pinned port-commit checkout).
//! - `src/hle/rt64_framebuffer_storage.cpp` (whole file, lines 1-61;
//!   whole-file SHA-256
//!   `a4c4d3e5dd390cd7889a316afb881a8ba344272b444f4421326bbcfd59597910` --
//!   matching the same inventory field, confirmed the same way).
//!
//! `docs/rt64-port-inventory.json` does not yet record either path's
//! `ported_as` as pointing at this module (both currently list `"ported_as":
//! []`) -- `scripts/lint-docs.py`'s inventory scanner is expected to report a
//! drift for that until a follow-up regenerates the inventory to add this
//! module; this module's own writable surface does not include
//! `docs/rt64-port-inventory.json`, so that reconciliation is deliberately
//! left to the owning ticket rather than done here.
//!
//! ```text
//! // rt64_framebuffer_storage.h:17-34
//! struct FramebufferStorage {
//!     struct Handle {
//!         uint32_t fbPairIndex;
//!         uint32_t address;
//!         uint32_t rdramIndex;
//!         uint32_t size;
//!     };
//!
//!     uint32_t rdramUsed;
//!     std::vector<uint8_t> rdramData;
//!     std::vector<Handle> handleVector;
//!
//!     FramebufferStorage();
//!     void reset();
//!     void store(uint32_t fbPairIndex, uint32_t address, const uint8_t *data, uint32_t size);
//!     const Handle *get(uint32_t maxFbPairIndex, uint32_t address) const;
//!     const uint8_t *getRDRAM(const Handle &handle) const;
//! };
//!
//! // rt64_framebuffer_storage.cpp:13-20
//! FramebufferStorage::FramebufferStorage() {
//!     reset();
//! }
//!
//! void FramebufferStorage::reset() {
//!     rdramUsed = 0;
//!     handleVector.clear();
//! }
//!
//! // rt64_framebuffer_storage.cpp:22-38
//! void FramebufferStorage::store(uint32_t fbPairIndex, uint32_t address, const uint8_t *data, uint32_t size) {
//!     uint32_t dstIndex = rdramUsed;
//!     rdramUsed += size;
//!     if (rdramUsed > rdramData.size()) {
//!         const uint32_t newSize = (rdramUsed * 3) / 2;
//!         rdramData.resize(newSize, 0);
//!     }
//!
//!     memcpy(rdramData.data() + dstIndex, data, size);
//!
//!     Handle handle;
//!     handle.fbPairIndex = fbPairIndex;
//!     handle.address = address;
//!     handle.rdramIndex = dstIndex;
//!     handle.size = size;
//!     handleVector.emplace_back(handle);
//! }
//!
//! // rt64_framebuffer_storage.cpp:40-55
//! const FramebufferStorage::Handle *FramebufferStorage::get(uint32_t maxFbPairIndex, uint32_t address) const {
//!     const FramebufferStorage::Handle *maxHandle = nullptr;
//!     for (const auto &handle : handleVector) {
//!         if (handle.fbPairIndex > maxFbPairIndex) {
//!             continue;
//!         }
//!
//!         if (handle.address != address) {
//!             continue;
//!         }
//!
//!         maxHandle = &handle;
//!     }
//!
//!     return maxHandle;
//! }
//!
//! // rt64_framebuffer_storage.cpp:57-60
//! const uint8_t *FramebufferStorage::getRDRAM(const Handle &handle) const {
//!     assert((handle.rdramIndex + handle.size) <= rdramData.size());
//!     return rdramData.data() + handle.rdramIndex;
//! }
//! ```
//!
//! **Reuse, not new type.** `Handle` and `FramebufferStorage` have no
//! existing analogue anywhere in `fn64-render-wgpu`: `rt64_texture_map_lru.rs`
//! ports a different RT64 file's slot allocator (`TextureMap`, LIFO free-list
//! reuse over indices with per-slot generation, `src/render/rt64_texture_cache.*`)
//! and `rt64_framebuffer_geometry.rs` ports the sibling `rt64_framebuffer.cpp`'s
//! pure-geometry cluster -- neither has an append-only byte arena or a
//! last-match-wins handle table, so this module is a new, minimal, owned
//! struct rather than a variant of either.
//!
//! ## Admitted domain
//!
//! - **Growth formula's exact form and truncation (hazard: multiply-then-divide
//!   vs. `n + n/2`, and which value it's computed from).** The source computes
//!   `newSize = (rdramUsed * 3) / 2` -- multiply by 3 first, then integer-divide
//!   by 2, **not** `rdramUsed + rdramUsed / 2` (these differ for odd `rdramUsed`:
//!   e.g. `rdramUsed = 5` gives `(5*3)/2 = 15/2 = 7` under the ported form but
//!   `5 + 5/2 = 5 + 2 = 7` under the alternate form too by coincidence at this
//!   value -- the two forms are *not* generally equal for all odd inputs
//!   despite agreeing here; e.g. `rdramUsed = 7`: `(7*3)/2 = 21/2 = 10` vs.
//!   `7 + 7/2 = 7 + 3 = 10`, still equal; the forms actually agree for all
//!   `n >= 0` in exact (non-overflowing) integer arithmetic since
//!   `floor(3n/2) == n + floor(n/2)` is an algebraic identity -- they diverge
//!   only once wraparound enters, see the overflow bullet below). This port's
//!   [`FramebufferStorage::grown_capacity`] preserves the source's literal
//!   `(rdram_used * 3) / 2` operation order (wrapping multiply, then
//!   truncating divide) rather than the algebraically-equivalent additive
//!   form, per this port's rule to preserve arithmetic order even when a
//!   result happens to coincide -- and it is computed from `rdramUsed`
//!   **after** `rdramUsed += size` has already run (the post-advance total
//!   requirement, not the pre-store size or the request size alone).
//!   `odd_capacity_growth_uses_multiply_by_three_then_divide_by_two_truncating`
//!   below pins an odd `rdramUsed` (`rdramUsed = 7`) against the
//!   hand-computed `(7*3)/2 = 10` (not `7*1.5 = 10.5` rounded any other way).
//! - **Grow-threshold comparison strictness: `>`, not `>=`.** `if (rdramUsed >
//!   rdramData.size())` only grows when the post-advance total *exceeds*
//!   current capacity; a store that lands exactly on the existing capacity
//!   (`rdramUsed == rdramData.size()`) does **not** trigger a resize. This
//!   port's [`FramebufferStorage::store`] uses `>` for the same check, tested
//!   at both sides of the threshold: `store_that_exactly_fills_capacity_does_not_grow`
//!   (capacity stays put) and `store_that_is_one_byte_over_capacity_grows`
//!   (capacity grows) below.
//! - **`resize` never shrinks and zero-fills new capacity.** `rdramData.resize(newSize,
//!   0)` is only ever reached inside the `rdramUsed > rdramData.size()`
//!   branch, so `newSize` is always greater than the current length at the
//!   call site; `std::vector::resize` with a fill value zero-initializes the
//!   newly added elements and leaves existing elements untouched. This port's
//!   `Vec::resize(new_size, 0)` has the identical behavior (grow-only at this
//!   call site, zero-fill of the new tail, no truncation since it is never
//!   called with a smaller size here).
//! - **`get`'s last-handle-wins semantic is "last qualifying entry in
//!   insertion order overwrites the running result," not first-match, not
//!   maximum-value, not shadow/invalidate of the earlier handle's storage.**
//!   The loop has no early exit: it visits every handle in `handleVector`
//!   (append order == call order of `store`), and on each one that satisfies
//!   both `fbPairIndex <= maxFbPairIndex` (via the inverted `if (...>
//!   maxFbPairIndex) continue;` guard) and `address == address`, it
//!   unconditionally overwrites `maxHandle` with a pointer to *that* handle --
//!   so after the loop, `maxHandle` points at the handle with the greatest
//!   index in `handleVector` among those that qualify (equivalently, the most
//!   recently `store`d one, since `store` only appends). A strictly earlier
//!   handle at the same address is not removed, invalidated, or shadowed in
//!   the vector -- it is simply not the one returned once a later qualifying
//!   entry exists. This port's [`FramebufferStorage::get`] reproduces the
//!   identical full-scan-no-early-exit loop rather than reversing iteration
//!   order to "return the first reverse match" (behaviorally identical here
//!   only because reversing plus early-exit would find the same element, but
//!   the ported form matches the source's literal control flow instead of
//!   relying on that equivalence). `get_returns_the_later_of_two_handles_sharing_one_address`
//!   below stores two handles at the same address with the later one having a
//!   smaller `fbPairIndex` than the first, then confirms `get` still returns
//!   the later-stored (higher vector index) handle, and
//!   `get_with_a_stale_handle_still_reachable_finds_the_newer_one_when_both_qualify`
//!   exercises the exact "stale handle used after a newer one exists" case
//!   named by this port's hazard list: both handles remain valid entries in
//!   `handle_vector` (neither is ever removed by a later `store`), and `get`
//!   is shown to prefer the newer one whenever both qualify under the same
//!   `max_fb_pair_index` bound.
//! - **`fbPairIndex` bound is inclusive (`<=`), via De Morgan on the `continue`
//!   guard.** `if (handle.fbPairIndex > maxFbPairIndex) continue;` skips
//!   exactly the handles whose index is strictly greater than the bound, so a
//!   handle with `fbPairIndex == maxFbPairIndex` is retained as a candidate.
//!   This port's `get` uses `handle.fb_pair_index <= max_fb_pair_index` as the
//!   positive-form equivalent (inclusive upper bound), tested by
//!   `get_bound_is_inclusive_of_max_fb_pair_index` below at the exact
//!   boundary value.
//! - **`reset` clears the handle table and zeroes the used counter but
//!   deliberately retains the allocated `rdramData` buffer** (both its
//!   capacity and its byte contents -- `reset` never calls `rdramData.clear()`
//!   or `.resize(0, ...)`, only `rdramUsed = 0; handleVector.clear();`). After
//!   `reset`, the old bytes are still physically present in `rdram_data` but
//!   unreachable through the public API (no handle references them, and the
//!   next `store` will overwrite from offset 0 without zeroing ahead of the
//!   write cursor first). This port's [`FramebufferStorage::reset`] does not
//!   touch `self.rdram_data`, matching that retention, and
//!   `reset_clears_handles_and_used_but_retains_the_rdram_buffer_bytes` below
//!   asserts the buffer's prior length and byte content both survive a
//!   `reset` call, and that a `get` for any previously stored handle returns
//!   `None` afterward (its `Handle` was in `handle_vector`, now cleared) even
//!   though the underlying bytes are still sitting in `rdram_data`.
//! - **Integer overflow in `rdramUsed += size` and `rdramUsed * 3` is
//!   unsigned-wraparound behavior in the source, not a guarded error.** C++
//!   `uint32_t` arithmetic is defined modulo 2^32 (unlike signed overflow,
//!   which is undefined behavior); both `rdramUsed += size` in `store` and the
//!   `rdramUsed * 3` inside `grownCapacity`'s formula can wrap silently for
//!   sufficiently large inputs, and the source adds no overflow check before
//!   either operation. This port uses `wrapping_add`/`wrapping_mul`
//!   (`wrapping_mul` then plain truncating `/2`, matching C++'s truncating
//!   unsigned division) to reproduce that same modulo-2^32 wraparound exactly
//!   -- not Rust's default checked/panicking arithmetic and not a saturating
//!   or guarded variant, since either would silently diverge from the C++
//!   behavior on overflowing inputs. This is documented, not exercised by a
//!   dedicated overflow test: reaching `u32::MAX`-adjacent `rdramUsed` values
//!   would require gigabytes of prior `store` calls to build up, which is out
//!   of this port's characterization scope (the same "document, don't
//!   contrive an unreachable-scale test" precedent `rt64_framebuffer_geometry.rs`
//!   sets for its own `u32` multiply overflow bullet).
//! - **No private-helper visibility gap was hit.** `FramebufferStorage`'s
//!   entire public surface (`reset`, `store`, `get`, `getRDRAM`) and its two
//!   data members (`rdramData`, `handleVector`) plus the nested `Handle`
//!   struct are all that this cluster needs; there is no private helper
//!   method anywhere in the 96 combined lines of `.h`+`.cpp` to reach into.
//! - **No divide-by-zero frontier exists in this module.** The only integer
//!   division ported here is `(rdramUsed * 3) / 2`, whose divisor `2` is a
//!   compile-time constant, never a runtime or caller-controlled value.
//!
//! ## Nonclaims
//!
//! No GPU, RHI, or production wiring (this module is not called from
//! anywhere yet and is not registered on any public crate surface beyond its
//! own `mod` declaration; dead-code warnings on its unused public surface are
//! expected and correct), and no RT64 visual/pixel/silicon parity or
//! performance claim. `getRDRAM`'s C++ `assert` is release-mode-stripped
//! (`NDEBUG`) bounds checking, not a production guarantee; this port mirrors
//! that with `debug_assert!` rather than inventing a `Result`-returning or
//! panicking-in-release API the source does not have.

/// Literal port of `RT64::FramebufferStorage::Handle`
/// (`rt64_framebuffer_storage.h:18-23`): a fixed-size record describing one
/// `store`d span of RDRAM bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Handle {
    pub fb_pair_index: u32,
    pub address: u32,
    pub rdram_index: u32,
    pub size: u32,
}

/// Literal port of `RT64::FramebufferStorage`
/// (`rt64_framebuffer_storage.h:17-34`, `rt64_framebuffer_storage.cpp:13-60`):
/// an append-only byte arena (`rdram_data`) addressed through an append-only
/// handle table (`handle_vector`). See the module doc "Admitted domain" for
/// the exact growth formula, comparison strictness, and last-handle-wins
/// lookup semantics this preserves.
#[derive(Debug, Clone)]
pub struct FramebufferStorage {
    pub rdram_used: u32,
    pub rdram_data: Vec<u8>,
    pub handle_vector: Vec<Handle>,
}

impl Default for FramebufferStorage {
    fn default() -> Self {
        Self::new()
    }
}

impl FramebufferStorage {
    /// `rt64_framebuffer_storage.cpp:13-15`: `FramebufferStorage::FramebufferStorage()`
    /// delegates its entire body to `reset()`.
    pub fn new() -> Self {
        let mut storage = FramebufferStorage {
            rdram_used: 0,
            rdram_data: Vec::new(),
            handle_vector: Vec::new(),
        };
        storage.reset();
        storage
    }

    /// `rt64_framebuffer_storage.cpp:17-20`: zeroes `rdramUsed` and clears
    /// `handleVector`. `rdramData` is deliberately left untouched -- see the
    /// module doc's "Admitted domain" bullet on retained buffer bytes.
    pub fn reset(&mut self) {
        self.rdram_used = 0;
        self.handle_vector.clear();
    }

    /// `rt64_framebuffer_storage.cpp:26`: the exact `(rdramUsed * 3) / 2`
    /// growth formula, isolated as its own function purely for testability of
    /// the odd-capacity truncation case -- the source inlines this
    /// expression directly inside `store`'s `if` body rather than naming it,
    /// so this is a literal-port helper, not an upstream symbol.
    fn grown_capacity(rdram_used: u32) -> u32 {
        rdram_used.wrapping_mul(3) / 2
    }

    /// `rt64_framebuffer_storage.cpp:22-38`: `FramebufferStorage::store`.
    /// Copies `data` (must be at least `size` bytes) into the arena at the
    /// current write cursor, growing the arena first if the post-advance
    /// total exceeds current capacity, then appends a `Handle` describing the
    /// span just written.
    pub fn store(&mut self, fb_pair_index: u32, address: u32, data: &[u8], size: u32) {
        let dst_index = self.rdram_used;
        self.rdram_used = self.rdram_used.wrapping_add(size);
        if self.rdram_used > self.rdram_data.len() as u32 {
            let new_size = Self::grown_capacity(self.rdram_used);
            self.rdram_data.resize(new_size as usize, 0);
        }

        let dst_start = dst_index as usize;
        let copy_len = size as usize;
        self.rdram_data[dst_start..dst_start + copy_len].copy_from_slice(&data[..copy_len]);

        self.handle_vector.push(Handle {
            fb_pair_index,
            address,
            rdram_index: dst_index,
            size,
        });
    }

    /// `rt64_framebuffer_storage.cpp:40-55`: `FramebufferStorage::get`. Full
    /// no-early-exit scan; returns the last handle in `handle_vector` (i.e.
    /// the most recently `store`d one) whose `fb_pair_index <=
    /// max_fb_pair_index` and whose `address` matches exactly. See the
    /// module doc's "Admitted domain" for why this is not a first-match or
    /// max-value lookup.
    pub fn get(&self, max_fb_pair_index: u32, address: u32) -> Option<&Handle> {
        let mut max_handle: Option<&Handle> = None;
        for handle in &self.handle_vector {
            if handle.fb_pair_index > max_fb_pair_index {
                continue;
            }

            if handle.address != address {
                continue;
            }

            max_handle = Some(handle);
        }

        max_handle
    }

    /// `rt64_framebuffer_storage.cpp:57-60`: `FramebufferStorage::getRDRAM`.
    /// The C++ `assert` is release-stripped bounds checking, ported as
    /// `debug_assert!` per this module's "Nonclaims".
    pub fn get_rdram(&self, handle: &Handle) -> &[u8] {
        debug_assert!(
            (handle.rdram_index as u64 + handle.size as u64) <= self.rdram_data.len() as u64
        );
        let start = handle.rdram_index as usize;
        let end = start + handle.size as usize;
        &self.rdram_data[start..end]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // -- construction / reset --------------------------------------------

    #[test]
    fn new_storage_has_zero_used_and_empty_data_and_handles() {
        let storage = FramebufferStorage::new();
        assert_eq!(storage.rdram_used, 0);
        assert!(storage.rdram_data.is_empty());
        assert!(storage.handle_vector.is_empty());
    }

    #[test]
    fn default_matches_new() {
        let storage = FramebufferStorage::default();
        assert_eq!(storage.rdram_used, 0);
        assert!(storage.rdram_data.is_empty());
        assert!(storage.handle_vector.is_empty());
    }

    #[test]
    fn reset_on_a_fresh_storage_stays_empty() {
        let mut storage = FramebufferStorage::new();
        storage.reset();
        assert_eq!(storage.rdram_used, 0);
        assert!(storage.rdram_data.is_empty());
        assert!(storage.handle_vector.is_empty());
    }

    #[test]
    fn reset_clears_handles_and_used_but_retains_the_rdram_buffer_bytes() {
        let mut storage = FramebufferStorage::new();
        storage.store(0, 0x1000, &[0xAA, 0xBB, 0xCC, 0xDD], 4);
        let data_before = storage.rdram_data.clone();
        let len_before = storage.rdram_data.len();
        assert!(len_before > 0);

        storage.reset();

        assert_eq!(storage.rdram_used, 0);
        assert!(storage.handle_vector.is_empty());
        // The buffer itself -- capacity and bytes -- must survive reset.
        assert_eq!(storage.rdram_data.len(), len_before);
        assert_eq!(storage.rdram_data, data_before);
        // No handle references those bytes anymore.
        assert!(storage.get(u32::MAX, 0x1000).is_none());
    }

    #[test]
    fn reset_is_idempotent_across_repeated_calls() {
        let mut storage = FramebufferStorage::new();
        storage.store(0, 0x2000, &[1, 2, 3], 3);
        storage.reset();
        storage.reset();
        storage.reset();
        assert_eq!(storage.rdram_used, 0);
        assert!(storage.handle_vector.is_empty());
    }

    // -- store: growth formula (exact multiply-then-divide order) --------

    #[test]
    fn grown_capacity_matches_hand_computed_multiply_by_three_divide_by_two() {
        // Hand-computed, not captured from any implementation run:
        // (4*3)/2 = 12/2 = 6
        assert_eq!(FramebufferStorage::grown_capacity(4), 6);
        // (10*3)/2 = 30/2 = 15
        assert_eq!(FramebufferStorage::grown_capacity(10), 15);
    }

    #[test]
    fn odd_capacity_growth_uses_multiply_by_three_then_divide_by_two_truncating() {
        // rdram_used = 7 (odd): (7*3)/2 = 21/2 = 10 in truncating integer
        // division (not 10.5 rounded up to 11, and not 7 + 7/2 evaluated any
        // differently -- both forms agree here, but this port preserves the
        // *multiply-then-divide* operation order regardless).
        assert_eq!(FramebufferStorage::grown_capacity(7), 10);
    }

    #[test]
    fn odd_capacity_growth_five_truncates_down_not_up() {
        // Hand-computed: (5*3)/2 = 15/2 = 7 (truncating, not 7.5 -> 8).
        assert_eq!(FramebufferStorage::grown_capacity(5), 7);
    }

    #[test]
    fn odd_capacity_growth_one_truncates_to_one() {
        // Hand-computed: (1*3)/2 = 3/2 = 1 (truncating, not 1.5 -> 2).
        assert_eq!(FramebufferStorage::grown_capacity(1), 1);
    }

    #[test]
    fn empty_arena_first_store_grows_from_zero() {
        let mut storage = FramebufferStorage::new();
        // rdram_used becomes 4 (0 + 4), 4 > 0 (current capacity), so grow to
        // (4*3)/2 = 6.
        storage.store(0, 0x1000, &[1, 2, 3, 4], 4);
        assert_eq!(storage.rdram_used, 4);
        assert_eq!(storage.rdram_data.len(), 6);
        assert_eq!(&storage.rdram_data[0..4], &[1, 2, 3, 4]);
        // The grown tail is zero-filled.
        assert_eq!(&storage.rdram_data[4..6], &[0, 0]);
    }

    #[test]
    fn first_allocation_records_a_handle_at_rdram_index_zero() {
        let mut storage = FramebufferStorage::new();
        storage.store(3, 0xABCD, &[9, 9], 2);
        assert_eq!(storage.handle_vector.len(), 1);
        let handle = storage.handle_vector[0];
        assert_eq!(handle.fb_pair_index, 3);
        assert_eq!(handle.address, 0xABCD);
        assert_eq!(handle.rdram_index, 0);
        assert_eq!(handle.size, 2);
    }

    // -- store: grow-threshold comparison strictness (`>`, not `>=`) -----

    #[test]
    fn store_that_exactly_fills_capacity_does_not_grow() {
        let mut storage = FramebufferStorage::new();
        // First store: rdram_used = 4 > 0, grows to (4*3)/2 = 6.
        storage.store(0, 0x100, &[1, 2, 3, 4], 4);
        assert_eq!(storage.rdram_data.len(), 6);

        // Second store of exactly 2 bytes: rdram_used becomes 4 + 2 = 6.
        // 6 > 6 is false, so capacity must NOT grow.
        storage.store(0, 0x200, &[5, 6], 2);
        assert_eq!(storage.rdram_used, 6);
        assert_eq!(storage.rdram_data.len(), 6);
        assert_eq!(&storage.rdram_data[0..6], &[1, 2, 3, 4, 5, 6]);
    }

    #[test]
    fn store_that_is_one_byte_over_capacity_grows() {
        let mut storage = FramebufferStorage::new();
        // First store: rdram_used = 4 > 0, grows to (4*3)/2 = 6.
        storage.store(0, 0x100, &[1, 2, 3, 4], 4);
        assert_eq!(storage.rdram_data.len(), 6);

        // Second store of 3 bytes: rdram_used becomes 4 + 3 = 7.
        // 7 > 6 is true, so capacity must grow to (7*3)/2 = 21/2 = 10.
        storage.store(0, 0x200, &[5, 6, 7], 3);
        assert_eq!(storage.rdram_used, 7);
        assert_eq!(storage.rdram_data.len(), 10);
        assert_eq!(&storage.rdram_data[0..7], &[1, 2, 3, 4, 5, 6, 7]);
        assert_eq!(&storage.rdram_data[7..10], &[0, 0, 0]);
    }

    #[test]
    fn store_that_is_well_under_capacity_does_not_grow() {
        let mut storage = FramebufferStorage::new();
        storage.store(0, 0x100, &[1; 10], 10);
        // rdram_used = 10 > 0, grows to (10*3)/2 = 15.
        assert_eq!(storage.rdram_data.len(), 15);
        let capacity_after_first = storage.rdram_data.len();

        // Second store of 1 byte: rdram_used = 11, 11 > 15 is false.
        storage.store(0, 0x200, &[2], 1);
        assert_eq!(storage.rdram_used, 11);
        assert_eq!(storage.rdram_data.len(), capacity_after_first);
    }

    // -- store: repeated allocation across several growths ----------------

    #[test]
    fn repeated_allocation_across_several_growths_matches_hand_computed_sequence() {
        let mut storage = FramebufferStorage::new();

        // Store 1: size 4. used=4, 4>0 -> grow to (4*3)/2=6.
        storage.store(0, 0xA, &[1, 2, 3, 4], 4);
        assert_eq!(storage.rdram_used, 4);
        assert_eq!(storage.rdram_data.len(), 6);

        // Store 2: size 5. used=4+5=9, 9>6 -> grow to (9*3)/2=27/2=13.
        storage.store(0, 0xB, &[5, 6, 7, 8, 9], 5);
        assert_eq!(storage.rdram_used, 9);
        assert_eq!(storage.rdram_data.len(), 13);

        // Store 3: size 4. used=9+4=13, 13>13 is false -> no grow.
        storage.store(0, 0xC, &[10, 11, 12, 13], 4);
        assert_eq!(storage.rdram_used, 13);
        assert_eq!(storage.rdram_data.len(), 13);

        // Store 4: size 1. used=13+1=14, 14>13 -> grow to (14*3)/2=42/2=21.
        storage.store(0, 0xD, &[14], 1);
        assert_eq!(storage.rdram_used, 14);
        assert_eq!(storage.rdram_data.len(), 21);

        // All four spans' bytes must still be intact at their recorded
        // rdram_index offsets.
        assert_eq!(storage.handle_vector.len(), 4);
        assert_eq!(storage.rdram_data[0..4], [1, 2, 3, 4]);
        assert_eq!(storage.rdram_data[4..9], [5, 6, 7, 8, 9]);
        assert_eq!(storage.rdram_data[9..13], [10, 11, 12, 13]);
        assert_eq!(storage.rdram_data[13..14], [14]);
    }

    #[test]
    fn each_store_captures_dst_index_before_advancing_used() {
        let mut storage = FramebufferStorage::new();
        storage.store(0, 0x1, &[1, 2], 2);
        storage.store(0, 0x2, &[3, 4, 5], 3);
        storage.store(0, 0x3, &[6], 1);

        assert_eq!(storage.handle_vector[0].rdram_index, 0);
        assert_eq!(storage.handle_vector[1].rdram_index, 2);
        assert_eq!(storage.handle_vector[2].rdram_index, 5);
    }

    #[test]
    fn zero_size_store_advances_nothing_and_still_records_a_handle() {
        let mut storage = FramebufferStorage::new();
        storage.store(0, 0x9, &[], 0);
        assert_eq!(storage.rdram_used, 0);
        assert_eq!(storage.rdram_data.len(), 0);
        assert_eq!(storage.handle_vector.len(), 1);
        assert_eq!(storage.handle_vector[0].size, 0);
        assert_eq!(storage.handle_vector[0].rdram_index, 0);
    }

    // -- get: empty arena --------------------------------------------------

    #[test]
    fn get_on_empty_arena_returns_none() {
        let storage = FramebufferStorage::new();
        assert!(storage.get(u32::MAX, 0x1000).is_none());
        assert!(storage.get(0, 0).is_none());
    }

    // -- get: last-handle-wins semantics ------------------------------------

    #[test]
    fn get_returns_the_later_of_two_handles_sharing_one_address() {
        let mut storage = FramebufferStorage::new();
        storage.store(5, 0x4000, &[1], 1);
        storage.store(2, 0x4000, &[2], 1); // later store, smaller fbPairIndex
                                           // Both qualify at max_fb_pair_index = 5; the later-stored one
                                           // (index 1 in handle_vector) must win, not the earlier one at
                                           // index 0, despite the earlier one having the larger fb_pair_index.
        let found = storage.get(5, 0x4000).expect("expected a match");
        assert_eq!(found.fb_pair_index, 2);
        assert_eq!(found.rdram_index, 1);
    }

    #[test]
    fn get_with_a_stale_handle_still_reachable_finds_the_newer_one_when_both_qualify() {
        let mut storage = FramebufferStorage::new();
        // "Stale handle" here means: a handle stored earlier that remains
        // present in handle_vector (store never removes prior entries) but
        // is no longer what get() returns once a newer qualifying handle
        // exists at the same address.
        storage.store(0, 0x8000, &[0xAA], 1);
        let stale = storage.handle_vector[0];
        storage.store(1, 0x8000, &[0xBB], 1);

        // The stale handle is still a live element of handle_vector...
        assert_eq!(storage.handle_vector[0], stale);
        assert_eq!(storage.handle_vector.len(), 2);

        // ...but get() prefers the newer one.
        let found = storage.get(10, 0x8000).expect("expected a match");
        assert_ne!(*found, stale);
        assert_eq!(found.fb_pair_index, 1);
        assert_eq!(found.rdram_index, 1);
    }

    #[test]
    fn get_bound_is_inclusive_of_max_fb_pair_index() {
        let mut storage = FramebufferStorage::new();
        storage.store(7, 0x5000, &[1], 1);
        // fb_pair_index == max_fb_pair_index must be retained (`<=`, not `<`).
        let found = storage.get(7, 0x5000).expect("boundary value must match");
        assert_eq!(found.fb_pair_index, 7);
    }

    #[test]
    fn get_excludes_handles_strictly_above_the_bound() {
        let mut storage = FramebufferStorage::new();
        storage.store(8, 0x5000, &[1], 1);
        assert!(storage.get(7, 0x5000).is_none());
    }

    #[test]
    fn get_ignores_handles_at_a_different_address() {
        let mut storage = FramebufferStorage::new();
        storage.store(0, 0x1111, &[1], 1);
        assert!(storage.get(0, 0x2222).is_none());
    }

    #[test]
    fn get_with_lower_bound_finds_an_earlier_qualifying_handle_when_the_later_one_is_excluded() {
        let mut storage = FramebufferStorage::new();
        storage.store(1, 0x9000, &[1], 1);
        storage.store(9, 0x9000, &[2], 1);
        // max_fb_pair_index = 3 excludes the second handle (fb_pair_index 9)
        // but not the first (fb_pair_index 1).
        let found = storage.get(3, 0x9000).expect("expected the earlier handle");
        assert_eq!(found.fb_pair_index, 1);
        assert_eq!(found.rdram_index, 0);
    }

    #[test]
    fn get_scans_past_a_non_matching_address_in_the_middle() {
        let mut storage = FramebufferStorage::new();
        storage.store(0, 0x1000, &[1], 1);
        storage.store(0, 0x2000, &[2], 1); // different address, must be skipped
        storage.store(0, 0x1000, &[3], 1);
        let found = storage.get(0, 0x1000).expect("expected a match");
        assert_eq!(found.rdram_index, 2);
    }

    #[test]
    fn get_returns_last_of_three_handles_sharing_address_and_bound() {
        let mut storage = FramebufferStorage::new();
        storage.store(0, 0x3000, &[1], 1);
        storage.store(0, 0x3000, &[2], 1);
        storage.store(0, 0x3000, &[3], 1);
        let found = storage.get(0, 0x3000).expect("expected a match");
        assert_eq!(found.rdram_index, 2);
        assert_eq!(storage.get_rdram(found), &[3]);
    }

    // -- getRDRAM ------------------------------------------------------------

    #[test]
    fn get_rdram_returns_the_exact_stored_bytes() {
        let mut storage = FramebufferStorage::new();
        storage.store(0, 0x1000, &[0x11, 0x22, 0x33], 3);
        let handle = storage.handle_vector[0];
        assert_eq!(storage.get_rdram(&handle), &[0x11, 0x22, 0x33]);
    }

    #[test]
    fn get_rdram_after_growth_still_finds_the_first_spans_original_bytes() {
        let mut storage = FramebufferStorage::new();
        storage.store(0, 0x1000, &[1, 2, 3, 4], 4);
        let first_handle = storage.handle_vector[0];
        // Trigger a growth with a second store.
        storage.store(0, 0x2000, &[5, 6, 7], 3);
        // First span's bytes must be unaffected by the resize/copy.
        assert_eq!(storage.get_rdram(&first_handle), &[1, 2, 3, 4]);
    }

    #[test]
    fn get_rdram_zero_size_handle_returns_empty_slice() {
        let mut storage = FramebufferStorage::new();
        storage.store(0, 0x1000, &[], 0);
        let handle = storage.handle_vector[0];
        assert_eq!(storage.get_rdram(&handle), &[] as &[u8]);
    }

    // -- multi-growth + get interplay across a realistic sequence --------

    #[test]
    fn sequence_of_stores_resets_and_gets_matches_hand_traced_expectations() {
        let mut storage = FramebufferStorage::new();

        storage.store(0, 0x100, &[1, 2], 2); // used=2, grow (2*3)/2=3
        assert_eq!(storage.rdram_data.len(), 3);

        storage.store(1, 0x200, &[3], 1); // used=3, 3>3 false, no grow
        assert_eq!(storage.rdram_data.len(), 3);

        storage.store(2, 0x100, &[9, 9], 2); // used=5, 5>3 grow (5*3)/2=7
        assert_eq!(storage.rdram_data.len(), 7);
        assert_eq!(storage.rdram_used, 5);

        // get(): two handles at 0x100 (fb_pair_index 0 and 2); the later
        // (index 2, rdram_index 3) must win for any bound >= 2.
        let found = storage.get(2, 0x100).expect("expected a match");
        assert_eq!(found.rdram_index, 3);
        assert_eq!(storage.get_rdram(found), &[9, 9]);

        // For a bound that excludes the later handle, the earlier one wins.
        let found_early = storage.get(1, 0x100).expect("expected the earlier handle");
        assert_eq!(found_early.rdram_index, 0);

        storage.reset();
        assert!(storage.get(u32::MAX, 0x100).is_none());
        assert_eq!(storage.rdram_used, 0);
        // Buffer bytes still physically present despite being unreachable.
        assert_eq!(storage.rdram_data.len(), 7);

        // A fresh store after reset writes from offset 0 again and can reuse
        // the retained capacity without growing (used=2, 2>7 is false).
        storage.store(0, 0x300, &[7, 7], 2);
        assert_eq!(storage.rdram_data.len(), 7);
        assert_eq!(storage.handle_vector.len(), 1);
        assert_eq!(storage.handle_vector[0].rdram_index, 0);
    }

    // -- additional odd-capacity growth truncation coverage ---------------

    #[test]
    fn odd_capacity_growth_three_truncates_down_not_up() {
        // Hand-computed: (3*3)/2 = 9/2 = 4 (truncating, not 4.5 -> 5).
        assert_eq!(FramebufferStorage::grown_capacity(3), 4);
    }

    #[test]
    fn odd_capacity_growth_nine_truncates_down_not_up() {
        // Hand-computed: (9*3)/2 = 27/2 = 13 (truncating, not 13.5 -> 14).
        assert_eq!(FramebufferStorage::grown_capacity(9), 13);
    }

    #[test]
    fn odd_capacity_growth_eleven_truncates_down_not_up() {
        // Hand-computed: (11*3)/2 = 33/2 = 16 (truncating, not 16.5 -> 17).
        assert_eq!(FramebufferStorage::grown_capacity(11), 16);
    }

    #[test]
    fn even_capacity_growth_is_exact_with_no_truncation() {
        // Hand-computed: (8*3)/2 = 24/2 = 12 exactly (no truncation occurs
        // for even inputs, since 3*even is always even).
        assert_eq!(FramebufferStorage::grown_capacity(8), 12);
    }

    #[test]
    fn grown_capacity_of_two_matches_hand_computed_value() {
        // Hand-computed: (2*3)/2 = 6/2 = 3.
        assert_eq!(FramebufferStorage::grown_capacity(2), 3);
    }

    // -- additional store / growth-threshold coverage ----------------------

    #[test]
    fn store_size_one_from_empty_arena_grows_to_one() {
        let mut storage = FramebufferStorage::new();
        // rdram_used = 1 > 0, grow to (1*3)/2 = 1.
        storage.store(0, 0x10, &[0x42], 1);
        assert_eq!(storage.rdram_used, 1);
        assert_eq!(storage.rdram_data.len(), 1);
        assert_eq!(storage.rdram_data[0], 0x42);
    }

    #[test]
    fn three_stores_each_landing_exactly_on_capacity_boundary() {
        let mut storage = FramebufferStorage::new();
        // Store 1: size 2. used=2, grow to (2*3)/2=3.
        storage.store(0, 0x1, &[1, 2], 2);
        assert_eq!(storage.rdram_data.len(), 3);
        // Store 2: size 1. used=3, 3>3 false -> no grow (exact fit).
        storage.store(0, 0x2, &[3], 1);
        assert_eq!(storage.rdram_data.len(), 3);
        assert_eq!(storage.rdram_used, 3);
        // Store 3: size 1. used=4, 4>3 true -> grow to (4*3)/2=6.
        storage.store(0, 0x3, &[4], 1);
        assert_eq!(storage.rdram_data.len(), 6);
        assert_eq!(storage.rdram_used, 4);
        assert_eq!(&storage.rdram_data[0..4], &[1, 2, 3, 4]);
    }

    #[test]
    fn many_small_stores_growth_sequence_matches_hand_trace() {
        let mut storage = FramebufferStorage::new();
        let expected_capacities = [
            // (store size, expected rdram_used after, expected capacity after)
            (1u32, 1u32, 1u32), // used=1>0 -> (1*3)/2=1
            (1, 2, 3),          // used=2>1 -> (2*3)/2=3
            (1, 3, 3),          // used=3>3 false -> stays 3
            (1, 4, 6),          // used=4>3 -> (4*3)/2=6
            (1, 5, 6),          // used=5>6 false -> stays 6
            (1, 6, 6),          // used=6>6 false -> stays 6
            (1, 7, 10),         // used=7>6 -> (7*3)/2=10
        ];
        for (i, (size, expected_used, expected_cap)) in expected_capacities.iter().enumerate() {
            storage.store(0, 0x1000 + i as u32, &[i as u8], *size);
            assert_eq!(storage.rdram_used, *expected_used, "at step {i}");
            assert_eq!(
                storage.rdram_data.len(),
                *expected_cap as usize,
                "at step {i}"
            );
        }
    }

    // -- additional get() coverage ------------------------------------------

    #[test]
    fn get_zero_max_fb_pair_index_matches_only_fb_pair_index_zero() {
        let mut storage = FramebufferStorage::new();
        storage.store(0, 0x7000, &[1], 1);
        let found = storage.get(0, 0x7000).expect("fb_pair_index 0 <= 0");
        assert_eq!(found.fb_pair_index, 0);
    }

    #[test]
    fn get_with_max_bound_matches_every_fb_pair_index() {
        let mut storage = FramebufferStorage::new();
        storage.store(u32::MAX, 0x7100, &[1], 1);
        let found = storage
            .get(u32::MAX, 0x7100)
            .expect("u32::MAX bound must include u32::MAX fb_pair_index");
        assert_eq!(found.fb_pair_index, u32::MAX);
    }

    #[test]
    fn get_after_reset_and_restore_only_sees_post_reset_handles() {
        let mut storage = FramebufferStorage::new();
        storage.store(0, 0x8100, &[1], 1);
        storage.reset();
        storage.store(0, 0x8100, &[2], 1);
        let found = storage
            .get(0, 0x8100)
            .expect("expected the post-reset handle");
        assert_eq!(storage.get_rdram(found), &[2]);
        assert_eq!(storage.handle_vector.len(), 1);
    }

    #[test]
    fn get_interleaved_addresses_and_bounds_matches_hand_traced_expectation() {
        let mut storage = FramebufferStorage::new();
        storage.store(0, 0xA, &[10], 1); // idx 0
        storage.store(1, 0xB, &[11], 1); // idx 1
        storage.store(2, 0xA, &[12], 1); // idx 2
        storage.store(3, 0xB, &[13], 1); // idx 3
        storage.store(4, 0xA, &[14], 1); // idx 4

        // For address 0xA, bound 4: candidates are idx 0 (fb=0), idx 2 (fb=2),
        // idx 4 (fb=4); last is idx 4.
        let found_a = storage.get(4, 0xA).unwrap();
        assert_eq!(found_a.rdram_index, 4);

        // For address 0xA, bound 3: candidates idx 0, idx 2 (fb=2<=3); idx 4
        // excluded (fb=4>3); last qualifying is idx 2.
        let found_a_bounded = storage.get(3, 0xA).unwrap();
        assert_eq!(found_a_bounded.rdram_index, 2);

        // For address 0xB, bound 3: candidates idx 1 (fb=1), idx 3 (fb=3);
        // last is idx 3.
        let found_b = storage.get(3, 0xB).unwrap();
        assert_eq!(found_b.rdram_index, 3);

        // For address 0xB, bound 0: no candidate qualifies (fb=1 > 0).
        assert!(storage.get(0, 0xB).is_none());
    }

    #[test]
    fn get_all_handles_excluded_by_address_returns_none_even_with_max_bound() {
        let mut storage = FramebufferStorage::new();
        storage.store(0, 0x1, &[1], 1);
        storage.store(1, 0x2, &[2], 1);
        storage.store(2, 0x3, &[3], 1);
        assert!(storage.get(u32::MAX, 0x999).is_none());
    }

    // -- Handle / FramebufferStorage struct-level equality and Debug -------

    #[test]
    fn handle_equality_compares_all_four_fields() {
        let a = Handle {
            fb_pair_index: 1,
            address: 2,
            rdram_index: 3,
            size: 4,
        };
        let b = Handle {
            fb_pair_index: 1,
            address: 2,
            rdram_index: 3,
            size: 4,
        };
        let c = Handle {
            fb_pair_index: 1,
            address: 2,
            rdram_index: 3,
            size: 5,
        };
        assert_eq!(a, b);
        assert_ne!(a, c);
    }

    #[test]
    fn handle_is_copy() {
        let h = Handle {
            fb_pair_index: 1,
            address: 2,
            rdram_index: 3,
            size: 4,
        };
        let h2 = h;
        // Both usable after copy -- proves Handle: Copy.
        assert_eq!(h, h2);
    }

    #[test]
    fn debug_formatting_does_not_panic_for_storage_and_handle() {
        let mut storage = FramebufferStorage::new();
        storage.store(0, 0x1, &[1], 1);
        let handle = storage.handle_vector[0];
        let _ = format!("{storage:?}");
        let _ = format!("{handle:?}");
    }

    #[test]
    fn clone_of_storage_is_independent_of_the_original() {
        let mut storage = FramebufferStorage::new();
        storage.store(0, 0x1, &[9], 1);
        let mut cloned = storage.clone();
        cloned.store(1, 0x2, &[8], 1);
        assert_eq!(storage.handle_vector.len(), 1);
        assert_eq!(cloned.handle_vector.len(), 2);
    }

    // -- multi-byte payloads spanning several handles -----------------------

    #[test]
    fn multi_byte_payloads_preserve_exact_bytes_across_growths() {
        let mut storage = FramebufferStorage::new();
        let payload_a: Vec<u8> = (0..20).collect();
        let payload_b: Vec<u8> = (100..120).collect();
        storage.store(0, 0x100, &payload_a, 20);
        storage.store(1, 0x200, &payload_b, 20);

        let handle_a = storage.get(0, 0x100).unwrap();
        assert_eq!(storage.get_rdram(handle_a), payload_a.as_slice());
        let handle_b = storage.get(1, 0x200).unwrap();
        assert_eq!(storage.get_rdram(handle_b), payload_b.as_slice());
    }

    #[test]
    fn store_with_data_slice_longer_than_size_only_copies_size_bytes() {
        // The C++ signature takes (data pointer, size) independently, so a
        // caller-provided buffer longer than `size` must only contribute its
        // first `size` bytes -- memcpy(dst, data, size) never reads past size.
        let mut storage = FramebufferStorage::new();
        let data = [1u8, 2, 3, 4, 5, 6, 7, 8];
        storage.store(0, 0x1, &data, 3);
        assert_eq!(storage.rdram_used, 3);
        assert_eq!(&storage.rdram_data[0..3], &[1, 2, 3]);
    }
}
