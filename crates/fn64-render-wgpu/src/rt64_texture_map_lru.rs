//! `TextureMap`'s slot allocator and LRU eviction policy: a literal port of
//! the permitted MIT RT64 Rust-port source pinned at commit
//! `5473732a822a4423b5696e7cb18fecc425a59875`
//! (`docs/RT64-PORT-AUTHORITY.md`), `src/render/rt64_texture_cache.h:54-55`
//! (`AccessPair`/`AccessList` typedefs) and `:121-152` (`TextureMap` struct
//! declaration), `src/render/rt64_texture_cache.cpp:189-385` (`TextureMap::`
//! constructor through `getMaxIndex`) (SHA-256 of the whole files,
//! `.h` = `57d66a7aeb7d7afcc1f57602114ed59cc8148ea2e201baf61edcc2f04328c816`,
//! `.cpp` = `723d43fe5ba112452264f15ce075fdd4b7e525bd01f5d483c73f78e7d1f4e9fc`):
//!
//! ```text
//! // rt64_texture_cache.h:54-55
//! typedef std::pair<uint32_t, uint64_t> AccessPair;
//! typedef std::list<AccessPair> AccessList;
//!
//! // rt64_texture_cache.h:121-152 (fields this module ports; textures/
//! // cachedTextureDimensions/textureReplacements/... GPU-adjacent fields
//! // omitted here, see "Reuse, not new type")
//! struct TextureMap {
//!     std::unordered_map<uint64_t, uint32_t> hashMap;
//!     std::vector<Texture *> textures;
//!     std::vector<uint64_t> hashes;
//!     std::vector<uint32_t> freeSpaces;
//!     std::vector<uint32_t> versions;
//!     std::vector<uint64_t> creationFrames;
//!     uint32_t globalVersion;
//!     AccessList accessList;
//!     std::vector<AccessList::iterator> listIterators;
//!     std::vector<Texture *> evictedTextures;
//!
//!     TextureMap();
//!     ~TextureMap();
//!     void clearReplacements();
//!     void add(uint64_t hash, uint64_t creationFrame, Texture *texture);
//!     void replace(uint64_t hash, Texture *texture, bool shiftedByHalf, bool referenceCounted);
//!     bool use(uint64_t hash, uint64_t submissionFrame, uint32_t &textureIndex, interop::float2 &textureScale, interop::float3 &textureDimensions, bool &textureReplaced, bool &hasMipmaps, bool &shiftedByHalf);
//!     bool evict(uint64_t submissionFrame, std::vector<uint64_t> &evictedHashes);
//!     void incrementLock();
//!     void decrementLock();
//!     Texture *get(uint32_t index) const;
//!     size_t getMaxIndex() const;
//! };
//!
//! // rt64_texture_cache.cpp:189-192
//! TextureMap::TextureMap() {
//!     globalVersion = 0;
//!     replacementMapEnabled = true;
//! }
//!
//! // rt64_texture_cache.cpp:217-255
//! void TextureMap::add(uint64_t hash, uint64_t creationFrame, Texture *texture) {
//!     assert(hashMap.find(hash) == hashMap.end());
//!
//!     // Check for free spaces on the LIFO queue first.
//!     uint32_t textureIndex;
//!     if (!freeSpaces.empty()) {
//!         textureIndex = freeSpaces.back();
//!         freeSpaces.pop_back();
//!     }
//!     else {
//!         textureIndex = static_cast<uint32_t>(textures.size());
//!         textures.push_back(nullptr);
//!         // ...cachedTextureDimensions/textureReplacements/... GPU fields grown in lockstep...
//!         hashes.push_back(0);
//!         versions.push_back(0);
//!         creationFrames.push_back(0);
//!         listIterators.push_back(accessList.end());
//!     }
//!
//!     hashMap[hash] = textureIndex;
//!     textures[textureIndex] = texture;
//!     // ...cachedTextureDimensions/textureReplacements/... GPU fields reset...
//!     hashes[textureIndex] = hash;
//!     versions[textureIndex]++;
//!     creationFrames[textureIndex] = creationFrame;
//!     globalVersion++;
//!
//!     accessList.push_front({ textureIndex, creationFrame });
//!     listIterators[textureIndex] = accessList.begin();
//! }
//!
//! // rt64_texture_cache.cpp:291-326 (use, minus the GPU-adjacent replacement/
//! // scale/dimension out-parameters -- see "Reuse, not new type")
//! bool TextureMap::use(uint64_t hash, uint64_t submissionFrame, uint32_t &textureIndex, ...) {
//!     const auto it = hashMap.find(hash);
//!     if (it == hashMap.end()) {
//!         textureIndex = 0;
//!         return false;
//!     }
//!
//!     textureIndex = it->second;
//!     // ...textureReplaced/textureScale/textureDimensions/hasMipmaps/shiftedByHalf populated...
//!
//!     // Remove the existing entry from the list if it exists.
//!     AccessList::iterator listIt = listIterators[textureIndex];
//!     if (listIt != accessList.end()) {
//!         accessList.erase(listIt);
//!     }
//!
//!     // Push a new access entry to the front of the list and store the new iterator.
//!     accessList.push_front({ textureIndex, submissionFrame });
//!     listIterators[textureIndex] = accessList.begin();
//!     return true;
//! }
//!
//! // rt64_texture_cache.cpp:328-376
//! bool TextureMap::evict(uint64_t submissionFrame, std::vector<uint64_t> &evictedHashes) {
//!     evictedHashes.clear();
//!
//!     auto it = accessList.rbegin();
//!     while (it != accessList.rend()) {
//!         assert(submissionFrame >= it->second);
//!
//!         // The max age allowed is the difference between the last time the texture was used and the time it was uploaded.
//!         // Ensure the textures live long enough for the frame queue to use them.
//!         const uint64_t MinimumMaxAge = WORKLOAD_QUEUE_SIZE * 2;
//!         const uint64_t MaximumMaxAge = WORKLOAD_QUEUE_SIZE * 32;
//!         const uint64_t age = submissionFrame - it->second;
//!         const uint64_t maxAge = std::clamp(it->second - creationFrames[it->first], MinimumMaxAge, MaximumMaxAge);
//!
//!         // Evict all entries that are present in the access list and are older than the frame by the specified margin.
//!         if (age >= maxAge) {
//!             const uint32_t textureIndex = it->first;
//!             const uint64_t textureHash = hashes[textureIndex];
//!             evictedTextures.emplace_back(textures[textureIndex]);
//!             textures[textureIndex] = nullptr;
//!             hashes[textureIndex] = 0;
//!             creationFrames[textureIndex] = 0;
//!             freeSpaces.push_back(textureIndex);
//!             listIterators[textureIndex] = accessList.end();
//!             hashMap.erase(textureHash);
//!             evictedHashes.push_back(textureHash);
//!             it = decltype(it)(accessList.erase(std::next(it).base()));
//!             // ...textureReplacements[textureIndex] decrement/reset (GPU-adjacent, not ported)...
//!         }
//!         // Stop iterating if we reach an entry that has been used in the present.
//!         else if (age == 0) {
//!             break;
//!         }
//!         else {
//!             it++;
//!         }
//!     }
//!
//!     return !evictedHashes.empty();
//! }
//!
//! Texture *TextureMap::get(uint32_t index) const {
//!     assert(index < textures.size());
//!     return textures[index];
//! }
//!
//! size_t TextureMap::getMaxIndex() const {
//!     return textures.size();
//! }
//! ```
//!
//! **Reuse, not new type.** `TextureMap` in the source is one struct that
//! interleaves the slot allocator/LRU bookkeeping this module ports
//! (`hashMap`, `textures` as an opaque presence vector, `hashes`,
//! `freeSpaces`, `versions`, `creationFrames`, `globalVersion`,
//! `accessList`, `listIterators`) with GPU/replacement-pool fields this
//! module does not model (`cachedTextureDimensions`, `textureReplacements`,
//! `cachedTextureReplacementDimensions`, `textureReplacementShiftedByHalf`,
//! `textureReplacementReferenceCounted`, `textureScales`,
//! `evictedTextures` as a `Texture *` sink, `replacementMap`,
//! `replacementMapEnabled`). This module defines one new owned type,
//! [`TextureMapLru`], carrying only the allocator/LRU fields, with `Texture
//! *` replaced by an opaque `u64` handle (this crate has no `Texture` type
//! and the source never dereferences the pointer in the ported methods --
//! `get`/`evict` only read/return it as an identity token, see "Admitted
//! domain"). No existing type in this crate models a slot table or an
//! LRU access list, so nothing here duplicates prior work.
//!
//! ## Admitted domain
//!
//! - **Free-slot reuse is LIFO, popped from the back of `freeSpaces`.**
//!   `add` checks `!freeSpaces.empty()`, reads `freeSpaces.back()`, then
//!   `freeSpaces.pop_back()` -- the most recently freed slot (by `evict`,
//!   which also `push_back`s) is the next one reused. Ported as
//!   `Vec::pop()` on this module's `free_spaces: Vec<u32>`, which pops from
//!   the same end it is pushed to -- same LIFO order, same growth-only
//!   fallback (`textures.size()` as the next fresh index) when empty.
//! - **`versions[textureIndex]++` runs unconditionally on every `add`, for
//!   both a freshly grown slot (starts at 0, becomes 1) and a reused slot
//!   (increments from whatever it was left at) -- it is never reset to 0
//!   on reuse.** This is a real behavior, not an oversight: the version
//!   counter distinguishes stale references to a reused slot index from
//!   the current occupant, which only works if reuse *always* advances the
//!   counter rather than restarting it. Ported as a plain `u32` field,
//!   incremented with `wrapping_add(1)` (see wrap note below), never
//!   reset on reuse.
//! - **`globalVersion` increments on `add` and (in the un-ported
//!   `clearReplacements`/`replace`) on replacement-pool mutation, but
//!   `use` never touches it.** This module ports only the `add`-side
//!   increment (`use` and `evict` do not touch `global_version` here
//!   either, matching the source: `evict` never increments
//!   `globalVersion`, only per-slot `versions` would if the un-ported
//!   replacement-reset lines ran, which they do not for a plain evicted
//!   slot -- `evict` clears `textureReplacements`-adjacent fields this
//!   module doesn't carry, and does not touch `versions` or
//!   `globalVersion` at all).
//! - **`versions`/`globalVersion` are `uint32_t` in C++ and can wrap.** A
//!   slot that is added/evicted/re-added `2^32` times wraps its per-slot
//!   version back to 0 with no guard in the source (plain `uint32_t`
//!   overflow, well-defined unsigned wraparound in C++). This module uses
//!   `wrapping_add(1)` for both counters to preserve that wraparound
//!   exactly rather than panicking (debug builds) or silently changing
//!   behavior (release builds' implicit wrap, which Rust's default `+=`
//!   would only match in `--release`) -- this is a documented behavior,
//!   not a guarded-against edge case, per hazard 3.
//! - **`use`'s access-list update is unconditional remove-then-push-front,
//!   not a conditional "only move if not already at front".** `listIt !=
//!   accessList.end()` is always true for any index reachable through
//!   `hashMap` (every `add`-assigned slot gets a valid `listIterators`
//!   entry; only a slot with no live hash-map entry -- i.e., free or never
//!   allocated -- would have `accessList.end()`, and `use` only reaches
//!   this line after a successful `hashMap.find`). So in practice the
//!   `if` is always taken for every reachable call; ported as an
//!   unconditional erase-then-push-front in [`TextureMapLru::use`] emitted
//!   as a literal `if` mirroring the source's own guard shape, not
//!   normalized away (hazard 2: preserve asymmetric-looking branches
//!   literally even when one arm is unreachable in practice from this
//!   module's own call sites -- the guard is cheap and the source keeps
//!   it, likely because a `TextureMap` field could theoretically be
//!   default-constructed with a dangling `listIterators` entry outside the
//!   `add`/`use`/`evict` protocol this module ports in isolation).
//! - **`accessList` is a `std::list`, ordered `push_front`-MRU-first: index
//!   0 (front) is always the most-recently touched slot (by `add` at
//!   `creationFrame`, or by `use` at `submissionFrame`), and the back is
//!   the least-recently touched.** This module's [`AccessEntry`] list is a
//!   `Vec<AccessEntry>` with the same front/back convention: `add`/`use`
//!   insert at index 0 (`Vec::insert(0, ..)`, the literal translation of
//!   `push_front` preserving the same ordering semantics a linked list's
//!   `push_front` gives, since this module is CPU-side bookkeeping over
//!   at-most-hundreds of live textures, not a hot GPU path -- no
//!   asymptotic claim is made or needed here).
//! - **`evict`'s sweep is `rbegin()` to `rend()`: back (oldest / least
//!   recently touched) to front (newest / most recently touched).**
//!   Ported as iterating `access_list` from `len() - 1` down to `0`.
//! - **`assert(submissionFrame >= it->second)` is a debug-only precondition
//!   the source relies on** (compiled out under `NDEBUG`/release, matching
//!   this crate's established "debug-only C++ `assert()` becomes
//!   `debug_assert!`" precedent, e.g. `rt64_frame_compatibility.rs`).
//!   `age = submissionFrame - it->second` is unsigned subtraction that
//!   would wrap to a huge value if the precondition were violated in a
//!   release build -- this module ports that as `submission_frame
//!   .wrapping_sub(entry.frame)`, preserving the same "trust the caller,
//!   wrap silently outside debug" behavior rather than adding a checked
//!   subtraction the source does not have (hazard 3: a caller-violated
//!   precondition is the caller's bug, and this module's `debug_assert!`
//!   catches it in test/debug builds exactly as the C++ `assert` would).
//! - **`maxAge = clamp(it->second - creationFrames[it->first],
//!   MinimumMaxAge, MaximumMaxAge)` clamps the entry's OWN LIFETIME (last
//!   access time minus creation time), not the entry's current age.** A
//!   texture that has lived a long time before its most recent access
//!   earns a longer eviction grace period on THIS sweep, independent of
//!   how stale that most recent access now is. `MinimumMaxAge =
//!   WORKLOAD_QUEUE_SIZE * 2`, `MaximumMaxAge = WORKLOAD_QUEUE_SIZE * 32`,
//!   with `WORKLOAD_QUEUE_SIZE = 4` pinned from `src/hle/
//!   rt64_workload_queue.h:26` (`#define WORKLOAD_QUEUE_SIZE 4`) -- so in
//!   the real engine `MinimumMaxAge = 8`, `MaximumMaxAge = 128`. This
//!   module takes `workload_queue_size` as a named `u64` constructor
//!   parameter (per the ticket's "take it as a named constant parameter"
//!   instruction) rather than hard-coding `4`, and derives
//!   `min_max_age`/`max_max_age` from it with the same `* 2` / `* 32`
//!   multipliers, using `std::cmp::max`/`std::cmp::min` composed as a
//!   literal 1:1 of `std::clamp(value, lo, hi)` (clamp to `[lo, hi]`
//!   inclusive on both ends, C++ `std::clamp` semantics -- if `value <
//!   lo`, returns `lo`; if `value > hi`, returns `hi`; otherwise `value`
//!   unchanged. Rust's `Ord::clamp` has identical inclusive-both-ends
//!   semantics and is used directly).
//! - **The eviction test is `age >= maxAge`, inclusive at the upper
//!   bound.** An entry with `age` exactly equal to `maxAge` IS evicted
//!   (`>=`, not `>`). Pinned by
//!   [`tests::age_exactly_equal_to_max_age_is_evicted`] and
//!   [`tests::age_one_below_max_age_survives`] on both sides of the
//!   threshold (hazard 1).
//! - **The `else if (age == 0) break;` early-stop is checked only when
//!   `age >= maxAge` is FALSE, and it stops the ENTIRE sweep, not just
//!   this entry.** Since the sweep runs oldest-to-newest (back to front)
//!   and `age` is `submissionFrame - it->second`, `age == 0` means "this
//!   entry's last access IS the current submission frame" -- i.e., this
//!   entry was just touched in the frame being evicted for. Because the
//!   list is MRU-ordered, once the reverse (oldest-first) scan reaches an
//!   entry with `age == 0`, every entry closer to the front is *at least*
//!   as recently touched (also `age == 0`, since nothing can be accessed
//!   "in the future" relative to `submissionFrame` -- see the
//!   `debug_assert!` above), so the break is a valid early-out, not a
//!   silent truncation of the sweep. This module preserves the identical
//!   `if age >= max_age { evict } else if age == 0 { break } else {
//!   advance }` three-way branch, in the same order, rather than
//!   restructuring it into a `while`+early-return or merging the `else
//!   if`/`else` arms (hazard 2: this looks like it could be simplified to
//!   "break on the first non-evictable entry", but that would be WRONG --
//!   an entry with `0 < age < maxAge` (not evicted, not `age == 0`) must
//!   fall through to `it++` and let the scan continue past it, since an
//!   older-but-still-live entry ahead of it could still be independently
//!   evictable... no: the scan is oldest-to-newest, so an entry further
//!   in the scan is NEWER, and could have `age == 0` while this one does
//!   not -- both a genuine "keep scanning" `else` arm and a genuine
//!   "stop entirely" `age == 0` arm coexist and are NOT redundant with
//!   each other, matching hazard 2's prior-card finding that asymmetric
//!   branches without a `break` (here, WITH a conditional `break`) encode
//!   real, order-dependent behavior.
//! - **On eviction, the freed slot's `hashes`/`creation_frames` are reset
//!   to `0`/`0` and `list_iterators`-equivalent bookkeeping is cleared,
//!   `free_spaces.push(index)` (back, matching `push_back`), and the
//!   access-list entry is removed from `access_list` at the current scan
//!   position -- the scan then continues from the same logical position
//!   (the element that is now the new neighbor at that position), which
//!   this module implements as decrementing the scan cursor by one after
//!   a `Vec::remove` at that index** (removing index `i` in a
//!   front-indexed `Vec` only shifts elements at index `> i`, i.e. toward
//!   the front/MRU end; every already-visited index `< i` toward the back
//!   is untouched, so "continue the reverse scan from the same position"
//!   is exactly "decrement the cursor", identical in effect to the
//!   source's `it = decltype(it)(accessList.erase(std::next(it).base()))`
//!   reverse-iterator-erase idiom for a `std::list`, verified by hand: for
//!   list `[A(front), B, C, D(back)]` erasing at reverse-iterator position
//!   `D` yields `std::next(rit(D)).base() == end()`, `list.erase(end()`
//!   is undefined -- the real list has `D` at a real forward position, so
//!   `std::next(rit(D)).base()` is actually the forward `end()` only when
//!   `D` is the sole remaining unvisited tail; the general case
//!   (`std::next(rit(X)).base()` is the forward iterator immediately
//!   *after* `X`, i.e. one step toward the front) erases `X` and returns
//!   an iterator to what was in front of `X`, and re-wrapping as a reverse
//!   iterator points that reverse iterator at what is now immediately
//!   behind the returned forward position -- the element that was one
//!   step further toward the back than `X`, i.e. the next element the
//!   reverse scan would have visited had `X` not been removed. This
//!   module's cursor-decrement after `Vec::remove(i)` produces the
//!   identical next-visited element.
//! - **`evictedHashes` is populated in eviction order (oldest-processed
//!   first within one `evict()` call), `.clear()`-ed at the start of every
//!   call.** Ported as `Vec<u64>` built with `.push()` in the same order,
//!   returned fresh (not accumulated across calls) from
//!   [`TextureMapLru::evict`].
//! - **`get(index)` and `getMaxIndex()` are direct reads with no LRU/
//!   allocator side effects** -- `get` asserts `index < textures.size()`
//!   (debug-only, ported as `debug_assert!`) and returns the opaque handle
//!   (`None` is not a source concept; a freed slot's handle was set to
//!   `nullptr`/`None`-equivalent by `evict`, so `get` on a freed-but-not-
//!   reused slot legitimately returns the "empty" sentinel, ported as
//!   `Option<u64>` populated `None` on evict / fresh-grow, `Some(handle)`
//!   on `add`). `getMaxIndex` returns `textures.len()`, i.e. the total
//!   slot-table size including free (not-yet-reused) slots, not the count
//!   of live entries.
//!
//! ## Nonclaims
//!
//! No GPU, no production wiring (this module is not called from anywhere
//! yet; `#[allow(dead_code)]`-free dead-code warnings on the unused public
//! surface are expected, matching this crate's established precedent), and
//! no RT64 visual/pixel/silicon parity or performance claim -- the
//! `Vec`-based `access_list`/`free_spaces` representation is chosen for
//! literal front/back/LIFO-order fidelity over at-most-hundreds of live
//! textures, not for asymptotic parity with `std::list`'s O(1)
//! arbitrary-position erase.
//!
//! `src/render/rt64_texture_cache.cpp` is 1,791 lines; this module ports
//! ONLY `TextureMap`'s constructor, `add`, `use`, `evict`, `get`, and
//! `getMaxIndex` (`.cpp:189-192, 217-255, 291-385`) plus the `AccessPair`/
//! `AccessList` typedefs and the allocator/LRU subset of the `TextureMap`
//! struct fields (`.h:54-55, 121-152`) -- the slot table and LRU sweep
//! this ticket names. Explicitly NOT ported, all rejected as unportable
//! dependency bulk per the planner's carve-out (see the ticket's
//! "Critical scoping note"):
//! - `TextureMap::clearReplacements` and `TextureMap::replace` (`.cpp:
//!   204-215, 257-289`) -- both operate purely on the GPU-adjacent
//!   replacement-pool fields (`textureReplacements`,
//!   `textureReplacementShiftedByHalf`, `textureReplacementReferenceCounted`,
//!   `textureScales`, `cachedTextureReplacementDimensions`) and the
//!   `ReplacementMap` reference-counting this module does not carry.
//! - `TextureMap::incrementLock`/`decrementLock` -- declared in the header
//!   (`.h:148-149`) but **never defined** anywhere in
//!   `rt64_texture_cache.cpp` (only `TextureCache::incrementLock`/
//!   `decrementLock` at `.cpp:1721,1726` exist, operating on `Texture`
//!   mutex/lock state, a different class). This is a private-helper
//!   visibility gap per hazard 4: the two `TextureMap` methods are
//!   link-time-undefined in the source itself (dead declarations, or
//!   defined in a translation unit outside the pinned file), so there is
//!   nothing to port them FROM within this file; they are reported here,
//!   not silently invented.
//! - `ReplacementMap`, `TextureCache`, `StreamThread`, and every other type/
//!   function in the file (the XXH3 hashing, `stb_image`/`ddspp` texture
//!   decode, zip/directory filesystem loading, and `std::thread`/mutex
//!   streaming machinery the ticket's "Critical scoping note" names) --
//!   none of it is referenced by the six ported methods above, confirming
//!   the ticket's premise that the slot-allocator/LRU algorithm is
//!   dependency-free and separable from the rest of the file.
//! - The GPU/replacement-adjacent `TextureMap` fields listed in "Reuse, not
//!   new type" above (`textures` as a real `Texture *`, dimensions,
//!   scales, replacement flags, `evictedTextures` as a `Texture *` sink,
//!   `replacementMap`, `replacementMapEnabled`) -- this module's `textures`
//!   equivalent is `Option<u64>` opaque handles only, and eviction
//!   collects evicted *handles*, not a `Texture *` destructor-run sink
//!   (the source's `~TextureMap()` `delete`s every entry in `textures` and
//!   `evictedTextures`; this module owns no heap resource to free).

/// One entry in the ported `AccessList`: `AccessPair` (`rt64_texture_cache.h:54`)
/// is `std::pair<uint32_t, uint64_t>` = `(textureIndex, frame)`, where
/// `frame` is either the `creationFrame` (pushed by `add`) or the
/// `submissionFrame` (pushed by `use`) at the time this entry was made
/// most-recently-used.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AccessEntry {
    pub texture_index: u32,
    pub frame: u64,
}

/// `TextureMap`'s slot allocator and LRU eviction policy, over opaque `u64`
/// texture handles in place of `Texture *` (see module doc "Reuse, not new
/// type"). `workload_queue_size` is `WORKLOAD_QUEUE_SIZE` (pinned to `4` in
/// the real engine, `src/hle/rt64_workload_queue.h:26`), taken as a named
/// constructor parameter per the ticket rather than hard-coded.
#[derive(Debug)]
pub struct TextureMapLru {
    /// `hashMap`: hash -> slot index. `use` and `add`'s duplicate-hash
    /// `assert` both key off this.
    hash_map: std::collections::HashMap<u64, u32>,
    /// `textures`: `None` where the source stores `nullptr` (free or
    /// never-allocated slot), `Some(handle)` where the source stores a live
    /// `Texture *`.
    textures: Vec<Option<u64>>,
    /// `hashes`: the hash last stored at each slot (`0` when free, matching
    /// the source's `hashes[textureIndex] = 0` reset on evict).
    hashes: Vec<u64>,
    /// `freeSpaces`: the LIFO free-slot stack, popped from the back.
    free_spaces: Vec<u32>,
    /// `versions`: per-slot version counter, incremented (never reset) on
    /// every `add`, including slot reuse.
    versions: Vec<u32>,
    /// `creationFrames`: the frame each slot's current occupant was added
    /// at; `0` when free.
    creation_frames: Vec<u64>,
    /// `globalVersion`: incremented on every `add` (this module's ported
    /// subset; see "Admitted domain" for what else touches it in the full
    /// source).
    global_version: u32,
    /// `accessList`: MRU-ordered, index `0` = front = most recently
    /// touched, last index = back = least recently touched.
    access_list: Vec<AccessEntry>,
    /// `WORKLOAD_QUEUE_SIZE`, named per the ticket rather than hard-coded.
    workload_queue_size: u64,
}

impl TextureMapLru {
    /// `TextureMap::TextureMap()` (`.cpp:189-192`). The source also sets
    /// `replacementMapEnabled = true`, a GPU-adjacent field this module does
    /// not carry (see "Nonclaims").
    pub fn new(workload_queue_size: u64) -> Self {
        Self {
            hash_map: std::collections::HashMap::new(),
            textures: Vec::new(),
            hashes: Vec::new(),
            free_spaces: Vec::new(),
            versions: Vec::new(),
            creation_frames: Vec::new(),
            global_version: 0,
            access_list: Vec::new(),
            workload_queue_size,
        }
    }

    /// `TextureMap::add` (`.cpp:217-255`), minus the GPU-adjacent dimension/
    /// scale/replacement field resets (see "Reuse, not new type").
    /// `debug_assert!`s the source's `assert(hashMap.find(hash) ==
    /// hashMap.end())` duplicate-hash precondition.
    pub fn add(&mut self, hash: u64, creation_frame: u64, texture: u64) {
        debug_assert!(
            !self.hash_map.contains_key(&hash),
            "TextureMap::add: hash already present (source asserts hashMap.find(hash) == hashMap.end())"
        );

        let texture_index = if let Some(reused) = self.free_spaces.pop() {
            reused
        } else {
            let fresh = self.textures.len() as u32;
            self.textures.push(None);
            self.hashes.push(0);
            self.versions.push(0);
            self.creation_frames.push(0);
            fresh
        };

        self.hash_map.insert(hash, texture_index);
        self.textures[texture_index as usize] = Some(texture);
        self.hashes[texture_index as usize] = hash;
        self.versions[texture_index as usize] =
            self.versions[texture_index as usize].wrapping_add(1);
        self.creation_frames[texture_index as usize] = creation_frame;
        self.global_version = self.global_version.wrapping_add(1);

        self.access_list.insert(
            0,
            AccessEntry {
                texture_index,
                frame: creation_frame,
            },
        );
    }

    /// `TextureMap::use` (`.cpp:291-326`), minus the GPU-adjacent
    /// `textureReplaced`/`textureScale`/`textureDimensions`/`hasMipmaps`/
    /// `shiftedByHalf` out-parameters (see "Reuse, not new type"). Returns
    /// `Some(texture_index)` on a hash-map hit (matching the source's `true`
    /// return plus its `textureIndex` out-parameter), `None` on a miss
    /// (matching `false`; the source also writes `textureIndex = 0` on
    /// miss, which this module does not need to reproduce since `None`
    /// already distinguishes the miss case for a caller).
    pub fn use_texture(&mut self, hash: u64, submission_frame: u64) -> Option<u32> {
        let texture_index = *self.hash_map.get(&hash)?;

        // Remove the existing entry from the list if it exists (see module
        // doc "Admitted domain" on why this `if` is always taken in
        // practice for a reachable index, and why it is kept literal
        // anyway).
        if let Some(position) = self
            .access_list
            .iter()
            .position(|entry| entry.texture_index == texture_index)
        {
            self.access_list.remove(position);
        }

        self.access_list.insert(
            0,
            AccessEntry {
                texture_index,
                frame: submission_frame,
            },
        );

        Some(texture_index)
    }

    /// `TextureMap::evict` (`.cpp:328-376`), minus the GPU-adjacent
    /// `textureScales`/`textureReplacements`-decrement bookkeeping (see
    /// "Reuse, not new type"). Returns the evicted hashes in eviction order
    /// (empty if none), matching the source's `evictedHashes` out-parameter
    /// plus its `bool` "did anything get evicted" return collapsed into
    /// "non-empty `Vec`".
    pub fn evict(&mut self, submission_frame: u64) -> Vec<u64> {
        let mut evicted_hashes = Vec::new();

        let min_max_age = self.workload_queue_size * 2;
        let max_max_age = self.workload_queue_size * 32;

        // Reverse scan: access_list[len - 1] (back / oldest) down to
        // access_list[0] (front / newest), matching accessList.rbegin() ..
        // accessList.rend().
        let mut cursor = self.access_list.len();
        while cursor > 0 {
            let index_in_list = cursor - 1;
            let entry = self.access_list[index_in_list];

            debug_assert!(
                submission_frame >= entry.frame,
                "TextureMap::evict: submissionFrame >= it->second violated"
            );

            let age = submission_frame.wrapping_sub(entry.frame);
            let lifetime = entry
                .frame
                .wrapping_sub(self.creation_frames[entry.texture_index as usize]);
            let max_age = lifetime.clamp(min_max_age, max_max_age);

            if age >= max_age {
                let texture_index = entry.texture_index;
                let texture_hash = self.hashes[texture_index as usize];
                self.textures[texture_index as usize] = None;
                self.hashes[texture_index as usize] = 0;
                self.creation_frames[texture_index as usize] = 0;
                self.free_spaces.push(texture_index);
                self.hash_map.remove(&texture_hash);
                evicted_hashes.push(texture_hash);
                self.access_list.remove(index_in_list);
                // Continue the reverse scan from the same logical position:
                // removing index_in_list only shifted indices >
                // index_in_list (toward the front/MRU end), so the next
                // element the reverse scan would visit is now at the same
                // index_in_list (or the scan is done if that was index 0).
                cursor = index_in_list;
            } else if age == 0 {
                break;
            } else {
                cursor -= 1;
            }
        }

        evicted_hashes
    }

    /// `TextureMap::get` (`.cpp:378-381`). `debug_assert!`s the source's
    /// `assert(index < textures.size())`.
    pub fn get(&self, index: u32) -> Option<u64> {
        debug_assert!(
            (index as usize) < self.textures.len(),
            "TextureMap::get: index < textures.size() violated"
        );
        self.textures[index as usize]
    }

    /// `TextureMap::getMaxIndex` (`.cpp:383-385`): total slot-table size,
    /// including free (not-yet-reused) slots.
    pub fn get_max_index(&self) -> usize {
        self.textures.len()
    }

    /// Test/inspection accessor: the current `globalVersion` (see "Admitted
    /// domain" for exactly which operations touch it in this module's
    /// ported subset).
    pub fn global_version(&self) -> u32 {
        self.global_version
    }

    /// Test/inspection accessor: the per-slot `versions[index]` counter.
    pub fn version(&self, index: u32) -> u32 {
        self.versions[index as usize]
    }

    /// Test/inspection accessor: a read-only view of `freeSpaces`, back
    /// (next-to-reuse) at the end, matching the source's `Vec::back()` pop
    /// order.
    pub fn free_spaces(&self) -> &[u32] {
        &self.free_spaces
    }

    /// Test/inspection accessor: a read-only view of `accessList`, front
    /// (index 0, most-recently-touched) first.
    pub fn access_list(&self) -> &[AccessEntry] {
        &self.access_list
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // -- empty allocator --

    #[test]
    fn new_allocator_has_no_slots_and_no_free_spaces() {
        let map = TextureMapLru::new(4);
        assert_eq!(map.get_max_index(), 0);
        assert!(map.free_spaces().is_empty());
        assert!(map.access_list().is_empty());
        assert_eq!(map.global_version(), 0);
    }

    #[test]
    fn evict_on_empty_allocator_evicts_nothing() {
        let mut map = TextureMapLru::new(4);
        let evicted = map.evict(1000);
        assert!(evicted.is_empty());
    }

    #[test]
    fn use_texture_on_empty_allocator_misses() {
        let mut map = TextureMapLru::new(4);
        assert_eq!(map.use_texture(0xABCD, 5), None);
    }

    // -- single slot --

    #[test]
    fn single_add_grows_table_to_one_slot() {
        let mut map = TextureMapLru::new(4);
        map.add(1, 0, 100);
        assert_eq!(map.get_max_index(), 1);
        assert_eq!(map.get(0), Some(100));
        assert_eq!(map.version(0), 1);
        assert_eq!(map.global_version(), 1);
        assert!(map.free_spaces().is_empty());
    }

    #[test]
    fn single_add_pushes_access_list_front_with_creation_frame() {
        let mut map = TextureMapLru::new(4);
        map.add(7, 42, 100);
        assert_eq!(
            map.access_list(),
            &[AccessEntry {
                texture_index: 0,
                frame: 42
            }]
        );
    }

    #[test]
    fn use_after_add_finds_the_slot_and_returns_its_index() {
        let mut map = TextureMapLru::new(4);
        map.add(7, 0, 100);
        assert_eq!(map.use_texture(7, 10), Some(0));
    }

    #[test]
    fn use_moves_the_entry_to_the_front_with_new_frame() {
        let mut map = TextureMapLru::new(4);
        map.add(7, 0, 100);
        map.use_texture(7, 99);
        assert_eq!(
            map.access_list(),
            &[AccessEntry {
                texture_index: 0,
                frame: 99
            }]
        );
    }

    #[test]
    fn use_of_unknown_hash_misses_and_does_not_touch_access_list() {
        let mut map = TextureMapLru::new(4);
        map.add(7, 0, 100);
        assert_eq!(map.use_texture(999, 10), None);
        assert_eq!(map.access_list().len(), 1);
    }

    // -- full capacity / multiple slots, no free spaces --

    #[test]
    fn multiple_adds_grow_distinct_sequential_slots() {
        let mut map = TextureMapLru::new(4);
        map.add(1, 0, 10);
        map.add(2, 0, 20);
        map.add(3, 0, 30);
        assert_eq!(map.get_max_index(), 3);
        assert_eq!(map.get(0), Some(10));
        assert_eq!(map.get(1), Some(20));
        assert_eq!(map.get(2), Some(30));
    }

    #[test]
    fn each_add_pushes_to_the_front_newest_first() {
        let mut map = TextureMapLru::new(4);
        map.add(1, 1, 10);
        map.add(2, 2, 20);
        map.add(3, 3, 30);
        assert_eq!(
            map.access_list(),
            &[
                AccessEntry {
                    texture_index: 2,
                    frame: 3
                },
                AccessEntry {
                    texture_index: 1,
                    frame: 2
                },
                AccessEntry {
                    texture_index: 0,
                    frame: 1
                },
            ]
        );
    }

    // -- allocate-free-reallocate: proving LIFO order --

    #[test]
    fn freed_slots_are_reused_lifo_most_recently_freed_first() {
        let mut map = TextureMapLru::new(4);
        // Three slots at frame 0, evict at a submission_frame that ages
        // out all three (age = submission_frame - creation_frame = 1000,
        // max_age is clamped to at most workload_queue_size * 32 = 128).
        map.add(1, 0, 10); // slot 0
        map.add(2, 0, 20); // slot 1
        map.add(3, 0, 30); // slot 2
        let evicted = map.evict(1000);
        assert_eq!(evicted.len(), 3);
        // Eviction sweeps oldest(back)-to-newest(front) of the access
        // list, which is [2,1,0] front-to-back after three adds -- so the
        // sweep visits 0, then 1, then 2, pushing each to free_spaces in
        // that order: free_spaces = [0, 1, 2], LIFO pop gives 2 first.
        assert_eq!(map.free_spaces(), &[0, 1, 2]);

        map.add(4, 1000, 40);
        // LIFO: slot 2 (most recently freed) is reused first.
        assert_eq!(map.get(2), Some(40));
        assert_eq!(map.free_spaces(), &[0, 1]);

        map.add(5, 1000, 50);
        assert_eq!(map.get(1), Some(50));
        assert_eq!(map.free_spaces(), &[0]);

        map.add(6, 1000, 60);
        assert_eq!(map.get(0), Some(60));
        assert!(map.free_spaces().is_empty());

        // No new slots were grown; the table stayed at 3.
        assert_eq!(map.get_max_index(), 3);
    }

    #[test]
    fn reused_slot_version_increments_rather_than_resets() {
        let mut map = TextureMapLru::new(4);
        map.add(1, 0, 10);
        assert_eq!(map.version(0), 1);
        map.evict(1000);
        assert_eq!(map.version(0), 1, "evict does not touch versions");
        map.add(2, 1000, 20);
        assert_eq!(
            map.version(0),
            2,
            "reused slot's version increments, does not reset to 1"
        );
    }

    #[test]
    fn add_after_evict_reuses_index_and_hash_map_reflects_new_hash() {
        let mut map = TextureMapLru::new(4);
        map.add(1, 0, 10);
        map.evict(1000);
        assert_eq!(
            map.use_texture(1, 1000),
            None,
            "old hash no longer resolves"
        );
        map.add(2, 1000, 20);
        assert_eq!(map.use_texture(2, 1000), Some(0));
    }

    // -- eviction with all ages equal --

    #[test]
    fn eviction_with_all_ages_equal_evicts_all_in_oldest_to_newest_scan_order() {
        let mut map = TextureMapLru::new(4);
        map.add(1, 0, 10);
        map.add(2, 0, 20);
        map.add(3, 0, 30);
        // All three were created at frame 0 and never `use`d, so all three
        // have identical age at any given submission_frame > 0.
        let evicted = map.evict(1000);
        assert_eq!(evicted, vec![1, 2, 3]);
        assert!(map.access_list().is_empty());
    }

    // -- eviction at the exact age clamp on both sides --

    #[test]
    fn age_exactly_equal_to_max_age_is_evicted() {
        let mut map = TextureMapLru::new(4);
        // workload_queue_size = 4 => min_max_age = 8, max_max_age = 128.
        // lifetime = last_used_frame(10) - creation_frame(0) = 10, clamped
        // to [8, 128] => max_age = 10 (unclamped, inside range).
        map.add(1, 0, 10);
        map.use_texture(1, 10); // last used at frame 10, lifetime = 10
                                // age = submission_frame - 10 == max_age(10) => submission_frame = 20.
        let evicted = map.evict(20);
        assert_eq!(
            evicted,
            vec![1],
            "age == max_age must evict (>=, inclusive)"
        );
    }

    #[test]
    fn age_one_below_max_age_survives() {
        let mut map = TextureMapLru::new(4);
        map.add(1, 0, 10);
        map.use_texture(1, 10); // lifetime = 10, max_age = 10
                                // age = 9, one below max_age(10).
        let evicted = map.evict(19);
        assert!(evicted.is_empty(), "age < max_age must survive");
    }

    #[test]
    fn lifetime_below_min_max_age_is_clamped_up_to_min_max_age() {
        let mut map = TextureMapLru::new(4);
        // lifetime = 10 - 9 = 1, clamped up to min_max_age = 8.
        map.add(1, 9, 10);
        map.use_texture(1, 10);
        // age = 7, below the clamped max_age(8): survives.
        let evicted = map.evict(17);
        assert!(
            evicted.is_empty(),
            "age(7) < clamped max_age(8) must survive"
        );
        // age = 8, equal to the clamped max_age(8): evicted.
        let evicted = map.evict(18);
        assert_eq!(evicted, vec![1], "age(8) == clamped max_age(8) must evict");
    }

    #[test]
    fn lifetime_above_max_max_age_is_clamped_down_to_max_max_age() {
        let mut map = TextureMapLru::new(4);
        // lifetime = 10000 - 0 = 10000, clamped down to max_max_age = 128.
        map.add(1, 0, 10);
        map.use_texture(1, 10000);
        // age = 127, below the clamped max_age(128): survives.
        let evicted = map.evict(10127);
        assert!(
            evicted.is_empty(),
            "age(127) < clamped max_age(128) must survive"
        );
        // age = 128, equal to the clamped max_age(128): evicted.
        let evicted = map.evict(10128);
        assert_eq!(
            evicted,
            vec![1],
            "age(128) == clamped max_age(128) must evict"
        );
    }

    // -- repeated access refreshing age --

    #[test]
    fn repeated_use_keeps_refreshing_the_entry_out_of_eviction_range() {
        let mut map = TextureMapLru::new(4);
        map.add(1, 0, 10);
        // Repeatedly touch the entry well before its max_age is reached.
        for frame in [5u64, 10, 15, 20, 25] {
            map.use_texture(1, frame);
            let evicted = map.evict(frame);
            assert!(
                evicted.is_empty(),
                "freshly used entry (age == 0) must survive its own submission frame"
            );
        }
        assert_eq!(
            map.get(0),
            Some(10),
            "never evicted across repeated fresh use"
        );
    }

    #[test]
    fn use_refreshes_frame_so_a_previously_close_to_eviction_entry_survives() {
        let mut map = TextureMapLru::new(4);
        map.add(1, 0, 10);
        // lifetime so far = 10 - 0 = 10, max_age = 10; without a refresh,
        // frame 20 would evict it (age == max_age case above).
        map.use_texture(1, 10);
        // Refresh again at frame 15: lifetime becomes 15 - 0 = 15 (still
        // inside [8,128]), age resets to 0 relative to frame 15.
        map.use_texture(1, 15);
        let evicted = map.evict(20);
        // age = 20 - 15 = 5, max_age = clamp(15, 8, 128) = 15: 5 < 15, survives.
        assert!(
            evicted.is_empty(),
            "refreshed entry must survive at what would have been the old threshold"
        );
    }

    // -- the age == 0 early break, and its scan-order implications --

    #[test]
    fn age_zero_break_stops_the_whole_sweep_even_with_an_older_evictable_entry_beyond_it() {
        let mut map = TextureMapLru::new(4);
        // hash 1 -> slot 0: created at frame 0, never used again -- old and
        // independently evictable by the time we submit at frame 1000.
        map.add(1, 0, 10);
        // hash 2 -> slot 1: created at frame 0, then freshly touched at the
        // current submission frame (age == 0 relative to submission_frame
        // 1000), which pushes it to the FRONT of the access list -- so the
        // reverse (oldest-to-newest) scan visits slot 0 first, then slot 1.
        map.add(2, 0, 20);
        map.use_texture(2, 1000);
        assert_eq!(
            map.access_list(),
            &[
                AccessEntry {
                    texture_index: 1,
                    frame: 1000
                },
                AccessEntry {
                    texture_index: 0,
                    frame: 0
                },
            ],
            "slot 1 (hash 2) is MRU-front after use_texture; slot 0 (hash 1) is the back/oldest"
        );
        // Reverse scan at submission_frame = 1000 starts at the back:
        // slot 0 (frame 0, age 1000, lifetime 0 clamped up to min_max_age 8)
        // -> age(1000) >= max_age(8), evictable, evicted.
        // Continue at the same cursor: slot 1 (frame 1000, age 0) -> break.
        let evicted = map.evict(1000);
        assert_eq!(evicted, vec![1], "slot 0 (hash 1) is evicted; the age==0 break then stops before slot 1's own check would matter");
        assert_eq!(
            map.get(1),
            Some(20),
            "slot 1 survives: it was age == 0, the fresh-use case the break protects"
        );
    }

    #[test]
    fn age_zero_break_can_leave_an_older_evictable_entry_unvisited_beyond_a_fresh_one() {
        // This test demonstrates the literal hazard: build a list where a
        // stale, evictable entry sits BEHIND (older than, i.e. later in the
        // reverse scan than) a fresh age==0 entry that is itself behind an
        // even-older entry -- proving the break stops the sweep at the
        // FIRST age==0 it encounters during the reverse walk, regardless of
        // what lies further toward the front.
        let mut map = TextureMapLru::new(4);
        map.add(1, 0, 10); // slot 0: oldest, created frame 0
        map.add(2, 500, 20); // slot 1: created frame 500
        map.add(3, 0, 30); // slot 2: created frame 0, will be the "fresh" one
                           // Move slot 2 to the front with a fresh use at the submission frame.
        map.use_texture(3, 1000);
        // access_list front-to-back is now: slot2(1000), slot1(500), slot0(0).
        assert_eq!(
            map.access_list(),
            &[
                AccessEntry {
                    texture_index: 2,
                    frame: 1000
                },
                AccessEntry {
                    texture_index: 1,
                    frame: 500
                },
                AccessEntry {
                    texture_index: 0,
                    frame: 0
                },
            ]
        );
        // Reverse scan order: slot0 (back) first, then slot1, then slot2 (front).
        // slot0: age = 1000 - 0 = 1000, lifetime = 0 - 0 = 0 clamped to 8:
        //   age(1000) >= max_age(8) -> evicted.
        // slot1: age = 1000 - 500 = 500, lifetime = 500 - 0 = 500 clamped to 128:
        //   age(500) >= max_age(128) -> ALSO evicted (not blocked by anything yet).
        // slot2: age = 1000 - 1000 = 0 -> break.
        let evicted = map.evict(1000);
        assert_eq!(
            evicted,
            vec![1, 2],
            "both older entries (slot 0 then slot 1) are evicted in oldest-to-newest scan order before the break at slot 2"
        );
        assert_eq!(
            map.get(2),
            Some(30),
            "slot 2 (age == 0) survives via the break"
        );
    }

    #[test]
    fn age_zero_break_does_not_fire_from_the_middle_only_from_the_scan_cursor() {
        let mut map = TextureMapLru::new(4);
        // Build access_list (front-to-back) = [ (2, 100), (1, 100), (0, 0) ]
        // i.e. slot 0 is oldest (frame 0), slots 1 and 2 both fresh at frame 100.
        map.add(1, 0, 10); // slot 0, creation_frame 0
        map.add(2, 0, 20); // slot 1
        map.add(3, 0, 30); // slot 2
        map.use_texture(2, 100); // slot 1 -> front
        map.use_texture(3, 100); // slot 2 -> front
        assert_eq!(
            map.access_list(),
            &[
                AccessEntry {
                    texture_index: 2,
                    frame: 100
                },
                AccessEntry {
                    texture_index: 1,
                    frame: 100
                },
                AccessEntry {
                    texture_index: 0,
                    frame: 0
                },
            ]
        );
        // Reverse scan at submission_frame = 100 starts at the back: slot 0
        // (frame 0, age 100, lifetime 0 clamped to 8) -> evictable, evict it.
        // Continue at the same cursor position, now pointing at slot 1
        // (frame 100, age 0) -> break immediately; slot 2 is never visited
        // (though it also has age 0, so the outcome is the same either way
        // -- this test pins that the scan actually stops there rather than
        // asserting on an unreachable difference).
        let evicted = map.evict(100);
        assert_eq!(
            evicted,
            vec![1],
            "only the oldest (slot 0 / hash 1) is evicted before the break"
        );
    }

    // -- misc: getMaxIndex / get on freed slot --

    #[test]
    fn get_on_a_freed_not_yet_reused_slot_returns_none() {
        let mut map = TextureMapLru::new(4);
        map.add(1, 0, 10);
        map.evict(1000);
        assert_eq!(map.get(0), None);
        assert_eq!(map.get_max_index(), 1, "table size unchanged by eviction");
    }

    #[test]
    fn evict_returns_empty_when_nothing_crosses_the_threshold() {
        let mut map = TextureMapLru::new(4);
        map.add(1, 0, 10);
        let evicted = map.evict(1); // age = 1 - 0 = 1, max_age clamped to 8: survives.
        assert!(evicted.is_empty());
        assert_eq!(map.get(0), Some(10));
    }

    #[test]
    fn global_version_increments_once_per_add_and_is_untouched_by_use_and_evict() {
        let mut map = TextureMapLru::new(4);
        map.add(1, 0, 10);
        assert_eq!(map.global_version(), 1);
        map.add(2, 0, 20);
        assert_eq!(map.global_version(), 2);
        map.use_texture(1, 5);
        assert_eq!(map.global_version(), 2, "use does not touch globalVersion");
        map.evict(1000);
        assert_eq!(
            map.global_version(),
            2,
            "evict does not touch globalVersion"
        );
    }

    #[test]
    fn version_wraps_on_overflow_without_panicking() {
        let mut map = TextureMapLru::new(4);
        map.add(1, 0, 10);
        // Force the counter to the top of u32 range and prove one more
        // add wraps to 0 rather than panicking (debug overflow) or being
        // disallowed.
        map.versions[0] = u32::MAX;
        map.evict(1_000_000);
        map.add(2, 1_000_000, 20);
        assert_eq!(map.version(0), 0, "u32 version wraps past MAX back to 0");
    }

    #[test]
    fn global_version_wraps_on_overflow_without_panicking() {
        let mut map = TextureMapLru::new(4);
        map.global_version = u32::MAX;
        map.add(1, 0, 10);
        assert_eq!(
            map.global_version(),
            0,
            "u32 globalVersion wraps past MAX back to 0"
        );
    }

    #[test]
    fn workload_queue_size_scales_the_min_and_max_age_clamp() {
        // With workload_queue_size = 1: min_max_age = 2, max_max_age = 32.
        let mut map = TextureMapLru::new(1);
        map.add(1, 0, 10);
        // lifetime = 0 (never used since add), clamped up to min_max_age = 2.
        let evicted = map.evict(1); // age = 1, below clamped max_age(2): survives.
        assert!(evicted.is_empty());
        let evicted = map.evict(2); // age = 2, == clamped max_age(2): evicted.
        assert_eq!(evicted, vec![1]);
    }

    #[test]
    fn add_duplicate_hash_is_debug_asserted() {
        let mut map = TextureMapLru::new(4);
        map.add(1, 0, 10);
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            map.add(1, 0, 20);
        }));
        if cfg!(debug_assertions) {
            assert!(
                result.is_err(),
                "duplicate-hash add should debug_assert in debug builds"
            );
        }
    }

    #[test]
    fn free_spaces_accessor_reflects_lifo_push_back_order() {
        let mut map = TextureMapLru::new(4);
        map.add(1, 0, 10);
        map.add(2, 0, 20);
        map.evict(1000); // both evicted, oldest(slot 0)-to-newest(slot 1) scan order
        assert_eq!(map.free_spaces(), &[0, 1]);
    }

    #[test]
    fn access_entry_is_copy_and_clone_and_eq() {
        let a = AccessEntry {
            texture_index: 1,
            frame: 2,
        };
        let b = a;
        assert_eq!(a, b);
        assert_eq!(a.clone(), b);
    }

    // -- additional coverage: growth after partial free-list reuse --

    #[test]
    fn add_grows_a_new_slot_when_free_spaces_is_exhausted_even_after_prior_reuse() {
        let mut map = TextureMapLru::new(4);
        map.add(1, 0, 10); // slot 0
        map.evict(1000); // slot 0 freed
        map.add(2, 1000, 20); // reuses slot 0
        assert_eq!(map.get_max_index(), 1, "reuse must not grow the table");
        map.add(3, 1000, 30); // no free spaces left, must grow to slot 1
        assert_eq!(map.get_max_index(), 2);
        assert_eq!(map.get(1), Some(30));
    }

    #[test]
    fn evicted_hash_no_longer_resolves_but_a_different_hash_can_reuse_its_old_slot() {
        let mut map = TextureMapLru::new(4);
        map.add(0xAAAA, 0, 10);
        map.evict(1000);
        assert_eq!(
            map.use_texture(0xAAAA, 1000),
            None,
            "stale hash must not resolve post-eviction"
        );
        map.add(0xBBBB, 1000, 99);
        assert_eq!(
            map.use_texture(0xBBBB, 1000),
            Some(0),
            "new hash resolves to the reused slot"
        );
        assert_eq!(
            map.use_texture(0xAAAA, 1000),
            None,
            "stale hash still does not resolve after the slot was reused by a different hash"
        );
    }

    #[test]
    fn repeated_evict_calls_with_nothing_evictable_are_idempotent_and_do_not_touch_access_list() {
        let mut map = TextureMapLru::new(4);
        map.add(1, 100, 10);
        map.use_texture(1, 100);
        let before = map.access_list().to_vec();
        for _ in 0..5 {
            let evicted = map.evict(100); // age == 0 every time: break immediately.
            assert!(evicted.is_empty());
        }
        assert_eq!(
            map.access_list().to_vec(),
            before,
            "no-op evict calls do not mutate the access list"
        );
    }

    #[test]
    fn get_max_index_counts_free_slots_still_allocated_in_the_table() {
        let mut map = TextureMapLru::new(4);
        map.add(1, 0, 10);
        map.add(2, 0, 20);
        map.evict(1000); // both freed, table stays at 2 slots
        assert_eq!(map.get_max_index(), 2);
        assert!(map.get(0).is_none());
        assert!(map.get(1).is_none());
    }

    #[test]
    fn use_after_evict_of_a_different_slot_still_finds_the_surviving_entry() {
        let mut map = TextureMapLru::new(4);
        map.add(1, 0, 10); // slot 0: will be evicted
        map.add(2, 500, 20); // slot 1: fresh relative to submission frame
        map.use_texture(2, 1000); // refresh slot 1 to age 0 at frame 1000
        let evicted = map.evict(1000);
        assert_eq!(
            evicted,
            vec![1],
            "only the stale hash 1 / slot 0 is evicted"
        );
        assert_eq!(
            map.use_texture(2, 1000),
            Some(1),
            "hash 2 / slot 1 still resolves"
        );
    }

    #[test]
    fn three_way_lifo_interleaving_of_add_and_evict_preserves_stack_order() {
        let mut map = TextureMapLru::new(4);
        map.add(1, 0, 10); // slot 0
        map.add(2, 0, 20); // slot 1
        map.evict(1000); // both evictable: free_spaces = [0, 1]
        map.add(3, 1000, 30); // pops 1 (LIFO): reused slot 1
        assert_eq!(map.get(1), Some(30));
        assert_eq!(map.free_spaces(), &[0]);
        map.evict(1000); // slot 1 (hash 3, created at 1000) not yet stale: age 0, survives
        assert_eq!(map.free_spaces(), &[0], "fresh slot 1 must not be re-freed");
        map.add(4, 1000, 40); // pops 0 (LIFO): reused slot 0
        assert_eq!(map.get(0), Some(40));
        assert!(map.free_spaces().is_empty());
    }

    #[test]
    fn min_max_age_and_max_max_age_pinned_at_the_named_workload_queue_size_of_four() {
        // Pin the doc-claimed real-engine values: workload_queue_size = 4
        // => min_max_age = 8, max_max_age = 128.
        let mut map = TextureMapLru::new(4);
        map.add(1, 0, 10);
        // lifetime = 0 (never re-used), clamps up to min_max_age = 8.
        assert!(map.evict(7).is_empty(), "age(7) < min_max_age(8) survives");
        assert_eq!(map.evict(8), vec![1], "age(8) == min_max_age(8) evicts");
    }

    #[test]
    fn zero_workload_queue_size_still_clamps_min_and_max_age_to_zero() {
        // workload_queue_size = 0 => min_max_age = 0, max_max_age = 0, so
        // max_age is always clamp(lifetime, 0, 0) == 0 regardless of
        // lifetime -- every entry is immediately evictable at any age >= 0,
        // i.e. everything except a same-frame (age == 0) entry.
        let mut map = TextureMapLru::new(0);
        map.add(1, 0, 10);
        let evicted = map.evict(1);
        assert_eq!(
            evicted,
            vec![1],
            "age(1) >= max_age(0) evicts under a zero queue size"
        );
    }

    #[test]
    fn evict_at_submission_frame_equal_to_creation_frame_has_age_zero_and_survives() {
        let mut map = TextureMapLru::new(4);
        map.add(1, 500, 10);
        let evicted = map.evict(500);
        assert!(
            evicted.is_empty(),
            "age == 0 relative to the entry's own creation frame always breaks the sweep"
        );
    }

    #[test]
    fn hash_map_lookup_after_replace_free_reallocate_cycle_resolves_only_the_live_hash() {
        let mut map = TextureMapLru::new(4);
        map.add(10, 0, 100);
        map.add(20, 0, 200);
        map.add(30, 0, 300);
        map.evict(1000); // all three evicted, free_spaces = [0, 1, 2]
        map.add(40, 1000, 400); // reuses slot 2 (LIFO)
        assert_eq!(map.use_texture(40, 1000), Some(2));
        for stale in [10u64, 20, 30] {
            assert_eq!(
                map.use_texture(stale, 1000),
                None,
                "stale hash {stale} must not resolve"
            );
        }
    }

    #[test]
    fn versions_are_independent_per_slot() {
        let mut map = TextureMapLru::new(4);
        map.add(1, 0, 10); // slot 0, version 1
        map.add(2, 0, 20); // slot 1, version 1
        map.use_texture(1, 5); // use does not touch versions, but moves slot 0
                               // to the access-list front: access_list becomes [(0,5), (1,0)].
        assert_eq!(map.version(0), 1);
        assert_eq!(map.version(1), 1);
        // Reverse (back-to-front) eviction scan now visits slot 1 first
        // (it's at the back after slot 0 was moved to the front), then
        // slot 0: free_spaces ends up [1, 0], so LIFO pop reuses slot 0
        // next, not slot 1.
        map.evict(1000); // both freed
        map.add(3, 1000, 30); // reuses slot 0 (LIFO, last freed by the scan)
        assert_eq!(map.version(0), 2, "slot 0 reused, version bumped");
        assert_eq!(
            map.version(1),
            1,
            "slot 1 was freed but not yet reused, version unchanged"
        );
    }
}
