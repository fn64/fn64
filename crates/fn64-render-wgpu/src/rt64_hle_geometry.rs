//! Literal port of the pure integer arithmetic and pure predicates carved out
//! of RT64's HLE geometry/state cluster -- the framebuffer-change pool's
//! 32-alignment and reuse test, `DrawAttribute`'s gapped bit numbering with
//! `DrawStatus`' bit operations, `DrawCall::identityRectScale`,
//! `DrawCallTile::validTexcoords`, `Projection::usesViewport`, and
//! `FramebufferPair`'s dither-pattern index, emptiness and projection-identity
//! tests -- plus `TextureManager::dumpTexture`'s RDRAM/palette extent
//! arithmetic. A literal port of the permitted MIT RT64 Rust-port source
//! pinned at commit `5473732a822a4423b5696e7cb18fecc425a59875`
//! (`docs/RT64-PORT-AUTHORITY.md`).
//!
//! ## Cited sources and digests
//!
//! Every digest below is the SHA-256 of the whole file, computed independently
//! here with `shasum -a 256` against the pinned port-commit checkout at
//! `/private/tmp/fn64-rt64-port-source`, and cross-checked verbatim against
//! `docs/rt64-port-inventory.json`'s
//! `files[path=...].sources.port.sha256`. **All six matched; no mismatch.**
//! (The inventory records `port_delta: "unchanged"` for all six, so
//! `sources.oracle.sha256` records the identical digest for each and the
//! oracle and port trees agree on these files byte for byte.)
//!
//! | Source | SHA-256 | Inventory lines | Drift |
//! |---|---|---|---|
//! | `src/hle/rt64_framebuffer_changes.cpp` | `03e6b0874535a04e0fe9f8f5d04d123297c9029bf7e0b2158b16fdf253bc262d` | 107 | partial (~15/107) |
//! | `src/hle/rt64_draw_call.h` | `32311f22cc3919f0983501b3dd63ab4d9f269d26095bb511c00a84886e43e2c2` | 161 | partial (~30/161) |
//! | `src/hle/rt64_draw_call.cpp` | `c9f8cccfbf3b34bc5ddefcb527b09f30762a8072f5ee767e812c4b5162d543dd` | 91 | full |
//! | `src/hle/rt64_rdp_tmem.cpp` | `bcf1d32e6f78894901c87a63ed28419d44e27675916756c565dc3baa9335bc4a` | 197 | partial (~30/197) |
//! | `src/hle/rt64_framebuffer_pair.cpp` | `93bd0d4322fa43bf424f464a799be99a0c75f52ac214e1ad27a30e1448c9ce3d` | 104 | partial (~20/104) |
//! | `src/hle/rt64_projection.cpp` | `526d7de56dd07a515a71fc0c511372360d4ec436da36c6f1d58c491596723b7f` | 45 | partial (~9/45) |
//!
//! Per-file drift disclosure, stated precisely:
//!
//! - **`rt64_draw_call.cpp` -- full.** All six of its function bodies
//!   (`attributeName`, `identityRectScale`, and the five `DrawStatus`
//!   methods plus two constructors) are ported. Nothing in the file is left
//!   unported.
//! - **`rt64_draw_call.h` -- partial.** Ported: the `DrawAttribute` enum's 22
//!   enumerator values (lines 23-46) and `DrawCallTile::validTexcoords` (lines
//!   157-159). Cited-but-not-ported: the `DrawCall` (lines 70-123),
//!   `DrawStatus` (125-136) and `DrawCallTile` (138-160) *field layouts*, the
//!   `DrawExtendedType`/`DrawVertexTestZ`/`DrawExtendedData`/
//!   `DrawExtendedFlags` declarations (48-68). See "Refused" below.
//! - **`rt64_framebuffer_changes.cpp` -- partial.** Ported:
//!   `FramebufferChangePool::use`'s 32-alignment formula (lines 37-40) and its
//!   compatible-buffer search predicate (lines 43-55). Cited-but-not-ported:
//!   the `RenderTexture`/`RenderWorker`/descriptor-set construction (65-87),
//!   the `std::map` pool bookkeeping (57-63, 92-106), the constructors and
//!   `reset` (12-34).
//! - **`rt64_rdp_tmem.cpp` -- partial.** Ported: `dumpTexture`'s RDRAM extent
//!   arithmetic (lines 88-111) and its palette extent arithmetic (135-144).
//!   Cited-but-not-ported: everything file/JSON/hash-set related --
//!   `uploadEmpty`, `uploadTMEM`, `uploadTexture`, `removeHashes`, the
//!   `std::ofstream` writes, the `snprintf` naming, the `json` serialization,
//!   and the `TLUT` enum mapping (176-185).
//! - **`rt64_framebuffer_pair.cpp` -- partial.** Ported: `addGameCall`'s
//!   dither-pattern index extraction (line 43), `inProjection` (47-56), and
//!   `isEmpty` (68-70). Cited-but-not-ported: `reset` (12-31),
//!   `changeProjection` (58-66), `earlyPresentCandidate` (72-103), and
//!   `addGameCall`'s `Vec`-mutating remainder. See "Refused" below.
//! - **`rt64_projection.cpp` -- partial.** Ported: `usesViewport` (lines
//!   36-44). Cited-but-not-ported: `reset` (10-18), `addGameCall` (20-28),
//!   `addPointLight` (30-34).
//!
//! `docs/rt64-port-inventory.json` does not yet record any of these six paths'
//! `ported_as` as pointing at this module (all currently list
//! `"ported_as": []`) -- `scripts/lint-docs.py`'s inventory scanner is
//! expected to report a `ported_as` drift for each until a follow-up
//! regenerates the inventory. This card's writable surface does not include
//! `docs/rt64-port-inventory.json` (sibling lanes are running concurrently and
//! a regeneration would clobber their entries), so that reconciliation is
//! deliberately left to the owning ticket. Note also that the inventory's
//! whole-file digest marks a source `ported` at **file** granularity: five of
//! the six sources here are genuine partial ports, so the burndown will
//! over-credit them. That over-credit is disclosed, not claimed.
//!
//! ## Verbatim source
//!
//! ```text
//! // rt64_framebuffer_changes.cpp:36-55
//! FramebufferChange &FramebufferChangePool::use(RenderWorker *renderWorker, FramebufferChange::Type type, uint32_t width, uint32_t height, bool usesHDR) {
//!     // To increase the chances of reusing buffers, we extend the width and height to a multiple of 32.
//!     const uint32_t Alignment = 32;
//!     uint32_t alignedWidth = ((width / Alignment) + ((width % Alignment) ? 1 : 0)) * Alignment;
//!     uint32_t alignedHeight = ((height / Alignment) + ((height % Alignment) ? 1 : 0)) * Alignment;
//!
//!     // Find a compatible changes buffer to use.
//!     for (auto &changes : changesMap) {
//!         if (changes.second.used) {
//!             continue;
//!         }
//!
//!         if ((changes.second.type == type) &&
//!             (changes.second.width == alignedWidth) &&
//!             (changes.second.height == alignedHeight))
//!         {
//!             changes.second.used = true;
//!             return changes.second;
//!         }
//!     }
//!     // ...
//! }
//!
//! // rt64_draw_call.h:20-46
//! // The values of the attributes must be preserved as they are for draw call filters that were already saved using these constants.
//! // Any missing gaps are purely intentional for the sake of keeping backwards compatibility.
//!
//! enum class DrawAttribute : uint32_t {
//!     Zero = 0,
//!     UID = 1,
//!     Tris = 2,
//!     Scissor = 5,
//!     Combine = 7,
//!     Texture = 8,
//!     OtherMode = 9,
//!     GeometryMode = 11,
//!     PrimColor = 12,
//!     EnvColor = 13,
//!     FogColor = 14,
//!     FillColor = 15,
//!     BlendColor = 16,
//!     Lights = 18,
//!     FramebufferPair = 21,
//!     PrimDepth = 22,
//!     Convert = 23,
//!     Key = 24,
//!     ObjRenderMode = 25,
//!     ExtendedType = 26,
//!     ExtendedFlags = 27,
//!     Count = 28
//! };
//!
//! // rt64_draw_call.h:157-159
//! bool validTexcoords() const {
//!     return (minTexcoord.x <= maxTexcoord.x) && (minTexcoord.y <= maxTexcoord.y);
//! }
//!
//! // rt64_draw_call.cpp:13-90
//! std::string DrawCall::attributeName(DrawAttribute a) {
//!     switch (a) {
//!     case DrawAttribute::Zero:
//!         return "Zero";
//!     case DrawAttribute::UID:
//!         return "UID";
//!     case DrawAttribute::Tris:
//!         return "Tris";
//!     case DrawAttribute::Scissor:
//!         return "Scissor";
//!     case DrawAttribute::Combine:
//!         return "Combine";
//!     case DrawAttribute::Texture:
//!         return "Texture";
//!     case DrawAttribute::OtherMode:
//!         return "OtherMode";
//!     case DrawAttribute::GeometryMode:
//!         return "GeometryMode";
//!     case DrawAttribute::PrimColor:
//!         return "PrimColor";
//!     case DrawAttribute::EnvColor:
//!         return "EnvColor";
//!     case DrawAttribute::FogColor:
//!         return "FogColor";
//!     case DrawAttribute::FillColor:
//!         return "FillColor";
//!     case DrawAttribute::BlendColor:
//!         return "BlendColor";
//!     case DrawAttribute::Lights:
//!         return "Lights";
//!     case DrawAttribute::ExtendedType:
//!         return "Extended";
//!     default:
//!         return "Unknown";
//!     }
//! }
//!
//! bool DrawCall::identityRectScale() const {
//!     const int16_t IdentityScale = 1024;
//!     return ((rectDsdx == IdentityScale) || (rectDsdx == -IdentityScale)) && ((rectDtdy == IdentityScale) || (rectDtdy == -IdentityScale));
//! }
//!
//! DrawStatus::DrawStatus() {
//!     reset();
//! }
//!
//! DrawStatus::DrawStatus(uint32_t v) {
//!     changed = v;
//! }
//!
//! void DrawStatus::reset() {
//!     clearChanges();
//! }
//!
//! void DrawStatus::clearChanges() {
//!     changed = 0;
//! }
//!
//! void DrawStatus::clearChange(DrawAttribute attribute) {
//!     assert(attribute < DrawAttribute::Count);
//!     changed &= ~(1U << static_cast<uint32_t>(attribute));
//! }
//!
//! void DrawStatus::setChanged(DrawAttribute attribute) {
//!     assert(attribute < DrawAttribute::Count);
//!     changed |= (1U << static_cast<uint32_t>(attribute));
//! }
//!
//! bool DrawStatus::isChanged(DrawAttribute attribute) const {
//!     assert(attribute < DrawAttribute::Count);
//!     return (changed & (1U << static_cast<uint32_t>(attribute))) != 0;
//! }
//!
//! bool DrawStatus::isChanged() const {
//!     return changed != 0;
//! }
//!
//! // rt64_projection.cpp:36-44
//! bool Projection::usesViewport() const {
//!     switch (type) {
//!     case Type::Perspective:
//!     case Type::Orthographic:
//!         return true;
//!     default:
//!         return false;
//!     }
//! }
//!
//! // rt64_framebuffer_pair.cpp:33-70
//! void FramebufferPair::addGameCall(const GameCall &gameCall) {
//!     assert(projectionCount > 0);
//!     auto &proj = projections[projectionCount - 1];
//!     proj.addGameCall(gameCall);
//!     depthRead = depthRead || gameCall.callDesc.otherMode.zCmp();
//!     depthWrite = depthWrite || gameCall.callDesc.otherMode.zUpd();
//!     fillRectOnly = fillRectOnly && (proj.type == Projection::Type::Rectangle) && (gameCall.callDesc.otherMode.cycleType() == G_CYC_FILL);
//!     gameCallCount++;
//!
//!     // Track what type of color dither this call used.
//!     uint32_t ditherIndex = (gameCall.callDesc.otherMode.rgbDither() >> G_MDSFT_RGBDITHER) & 0x3;
//!     ditherPatterns[ditherIndex]++;
//! }
//!
//! bool FramebufferPair::inProjection(uint32_t transformsIndex, Projection::Type type) const {
//!     if (projectionCount > 0) {
//!         const Projection &lastProj = projections[projectionCount - 1];
//!         if ((lastProj.transformsIndex == transformsIndex) && (lastProj.type == type)) {
//!             return true;
//!         }
//!     }
//!
//!     return false;
//! }
//!
//! bool FramebufferPair::isEmpty() const {
//!     return (gameCallCount == 0) && startFbOperations.empty() && endFbOperations.empty();
//! }
//!
//! // rt64_rdp_tmem.cpp:88-111 (dumpTexture's RDRAM extent arithmetic)
//! const LoadOperation &loadOp = state->rdp->rice.lastLoadOpByTMEM[loadTile.tmem];
//! uint32_t rdramStart = loadOp.texture.address;
//! uint32_t rdramCount = 0;
//! uint32_t commonBytesOffset = (loadOp.tile.uls >> 2) << loadOp.texture.siz >> 1;
//! uint32_t commonBytesPerRow = loadOp.texture.width << loadOp.texture.siz >> 1;
//! if (loadOp.type == LoadOperation::Type::Block) {
//!     uint32_t wordCount = ((loadOp.tile.lrs - loadOp.tile.uls) >> (4 - loadOp.tile.siz)) + 1;
//!     rdramStart = loadOp.texture.address + commonBytesOffset + commonBytesPerRow * loadOp.tile.ult;
//!     rdramCount = (wordCount << 3);
//!
//!     // Increase the amount of RDRAM dumped by textures that require padding when using load block.
//!     commonBytesPerRow = std::max(commonBytesPerRow, uint32_t(loadTile.line) << 3U);
//! }
//! else if (loadOp.type == LoadOperation::Type::Tile) {
//!     uint32_t rowCount = 1 + ((loadOp.tile.lrt >> 2) - (loadOp.tile.ult >> 2));
//!     uint32_t tileWidth = ((loadOp.tile.lrs >> 2) - (loadOp.tile.uls >> 2));
//!     uint32_t wordsPerRow = (tileWidth >> (4 - loadOp.tile.siz)) + 1;
//!     rdramStart = loadOp.texture.address + commonBytesOffset + commonBytesPerRow * (loadOp.tile.ult >> 2);
//!     rdramCount = rowCount * commonBytesPerRow;
//! }
//!
//! // Dump more RDRAM if necessary if it doesn't cover what the tile could possibly sample.
//! uint32_t loadTileBpr = width << loadTile.siz >> 1;
//! rdramCount = std::max(rdramCount, std::max(loadTileBpr, commonBytesPerRow) * height);
//!
//! // rt64_rdp_tmem.cpp:135-144 (dumpTexture's palette extent arithmetic)
//! if (tlut > 0) {
//!     const bool CI4 = (loadTile.siz == G_IM_SIZ_4b);
//!     const int32_t paletteTMEM = (RDP_TMEM_WORDS >> 1) + (CI4 ? (loadTile.palette << 4) : 0);
//!     const LoadOperation &paletteLoadOp = state->rdp->rice.lastLoadOpByTMEM[paletteTMEM];
//!     uint32_t paletteBytesOffset = (paletteLoadOp.tile.uls >> 2) << paletteLoadOp.texture.siz >> 1;
//!     uint32_t paletteBytesPerRow = paletteLoadOp.texture.width << paletteLoadOp.texture.siz >> 1;
//!     const uint32_t rowCount = 1 + ((paletteLoadOp.tile.lrt >> 2) - (paletteLoadOp.tile.ult >> 2));
//!     const uint32_t wordsPerRow = ((paletteLoadOp.tile.lrs >> 2) - (paletteLoadOp.tile.uls >> 2)) + 1;
//!     uint32_t paletteRdramStart = paletteLoadOp.texture.address + paletteBytesOffset + paletteBytesPerRow * (paletteLoadOp.tile.ult >> 2);
//!     uint32_t paletteRdramCount = (rowCount - 1) * paletteBytesPerRow + (wordsPerRow << 3);
//!     // ...
//! }
//! ```
//!
//! ## Reuse, not new type
//!
//! A duplication pass over `fn64-render-wgpu` found substantial overlapping
//! ground. Everything already owned is **reused or refused**, never
//! re-derived:
//!
//! - **[`crate::rt64_tmem_regions::RDP_TMEM_WORDS`]** (`= 512`, cited there to
//!   `src/hle/rt64_rdp.h:21`) is *imported*, not redeclared, for
//!   `paletteTMEM`'s `(RDP_TMEM_WORDS >> 1)` upper-half base. This module
//!   makes no independent claim about that constant's value.
//! - **`FixedRect` is already owned by [`crate::rt64_common`]**, which ports
//!   the whole type from `src/common/rt64_common.{h,cpp}` including `reset`,
//!   `merge`, `isNull`, `isEmpty` and `fullyInside`. Every `FixedRect`
//!   operation appearing in the cited files (`Projection::reset`'s
//!   `scissorRect.reset()`, `Projection::addGameCall`'s
//!   `isNull()`/`merge()`, `FramebufferPair::reset`'s three `reset()`s,
//!   `earlyPresentCandidate`'s `fullyInside`) is therefore *that module's*,
//!   and this one neither redefines nor re-tests any of them.
//! - **`OtherMode`'s `zCmp`/`zUpd`/`cycleType`/`rgbDither` accessors are
//!   already owned by [`crate::state::OtherMode`]** (ported from
//!   `src/shared/rt64_other_mode.h`). `addGameCall`'s `depthRead`/
//!   `depthWrite`/`fillRectOnly` accumulation is nothing but `||`/`&&` over
//!   those accessors' results plus a `Projection::Type` test, so it carries no
//!   arithmetic this module could add; it is refused as an accumulation over
//!   already-ported predicates. The one part of `addGameCall` that *is* real
//!   arithmetic -- the dither-pattern index -- is ported here, and it is
//!   ported as a function of the **raw `H` word**, deliberately not of
//!   `state::OtherMode::rgb_dither()`; see the disagreement note below.
//! - **`Blender::usesVisualizeCoverageCycle` is already owned by
//!   [`crate::rt64_blender_analysis`]**, which ports both overloads from
//!   `src/shared/rt64_blender.h`. `FramebufferPair::earlyPresentCandidate` is
//!   a loop whose only non-trivial test is that predicate composed with
//!   `FixedRect::fullyInside` -- both already owned -- so it is **refused**
//!   rather than re-derived. A caller wanting the full C++ behavior composes
//!   `rt64_blender_analysis::uses_visualize_coverage_cycle` with
//!   `rt64_common`'s `fully_inside` over its own projection storage.
//! - **`GameIndices::FramebufferPair` and six `FramebufferPair` scalar fields
//!   are already owned by [`crate::rt64_frame_compatibility`]**
//!   (`ColorImageFields`, `DepthImageFields`, `FbPairFields`,
//!   `FramebufferPairIndex`). That module took `colorImage.{address,fmt,siz,
//!   width}`, `depthImage.address`, and `depthRead`/`depthWrite` -- exactly
//!   the fields its two predicates read -- and its Nonclaims explicitly refuse
//!   the wider `Workload`/`WorkloadQueue` graph. This module therefore does
//!   **not** redeclare any image-descriptor struct, and does not re-port
//!   `depthRead`/`depthWrite`. The three `FramebufferPair` facts ported here
//!   (`ditherPatterns` indexing, `isEmpty`, `inProjection`) are disjoint from
//!   that module's six fields, so the two modules do not overlap.
//! - **`RgbDither` as a decoded enum is already owned by
//!   [`crate::rgb_dither`]**; this module deliberately does not introduce a
//!   second dither enum. [`dither_pattern_index`] returns the bare `u32`
//!   index into RT64's own four-slot `ditherPatterns` histogram, which is a
//!   different thing from a decoded dither *mode*.
//! - **fn64's [`crate::tmem`] is not this source's concern.** A sibling
//!   assessment concluded that `rt64_rdp_tmem.cpp` is "a texture-cache
//!   upload/dump manager, a different concern from TMEM byte layout", and
//!   that holds under inspection: `crate::tmem` owns physical TMEM lanes,
//!   transfer plans, texel decode and TLUT resolution, while this file's
//!   ported fraction computes *how many RDRAM bytes a debug dump should
//!   write*. No function, constant or predicate is shared between them. The
//!   arithmetic ported here reads `LoadOperation`/`LoadTile` fields and
//!   produces byte extents; it performs no TMEM read, no validity check and
//!   no texel decode.
//!
//! No new vector type is introduced: every value ported here is an integer,
//! a boolean or a C-like enum, so `AGENTS.md`'s "One vector type per port"
//! rule (`fn64_render_ir::Vec3`/`Vec4` for `float3`/`float4`) has no subject
//! in this module -- there is no `float3` or `float4` anywhere in the ported
//! fraction.
//!
//! ## A bit-level disagreement worth recording
//!
//! RT64's `OtherMode::rgbDither()` (`src/shared/rt64_other_mode.h:78-79`)
//! returns `H & (3U << G_MDSFT_RGBDITHER)` -- the two dither bits **left in
//! place at bit 6**, not shifted down. `FramebufferPair::addGameCall` then
//! writes `(gameCall.callDesc.otherMode.rgbDither() >> G_MDSFT_RGBDITHER) &
//! 0x3`, shifting them back down and re-masking. The `& 0x3` is redundant
//! there (the accessor already masked to two bits), but the `>> 6` is load-
//! bearing: without it the index would be `{0, 64, 128, 192}` and the
//! `std::array<uint32_t, 4>` subscript would be out of bounds for three of
//! four dither modes.
//!
//! fn64's [`crate::state::OtherMode::rgb_dither`] instead computes
//! `(self.high >> 6) & 0x3` and maps the result to an `RgbDither` enum --
//! that is, fn64's accessor **already performs the shift RT64's accessor
//! omits**. The two accessors are therefore *not* interchangeable at the bit
//! level: `RT64::rgbDither()` and `fn64::rgb_dither()` return values differing
//! by a factor of 64 for every non-zero dither mode.
//!
//! This is a naming collision, not a defect in either codebase: each accessor
//! is self-consistent with its own call sites. But it is exactly the kind of
//! trap that would silently produce a wrong histogram index if the port
//! naively substituted fn64's accessor into RT64's expression -- the double
//! shift would yield `0` for all four modes. [`dither_pattern_index`] is
//! therefore defined over the **raw `H` other-mode word**, reproducing RT64's
//! full `(H & (3 << 6)) >> 6 & 0x3` chain literally, including the redundant
//! trailing mask, so no accessor-substitution error is possible. Its tests
//! pin all four modes *and* pin that surrounding `H` bits cannot perturb the
//! index.
//!
//! ## Admitted domain
//!
//! - **The 32-alignment formula is a round-up-to-multiple-of-32 in `u32`,
//!   and the source's form and the idiomatic `(w + 31) / 32 * 32` are
//!   *exhaustively* equivalent over the full `u32` domain.** That was checked
//!   two independent ways: algebraically (`(w/32) + (w%32 != 0)` is
//!   `ceil(w/32)` by definition, and so is `(w+31)/32` for `w <= 0xFFFFFFE0`),
//!   and by brute force over `0..=0xFFFFFFFF`'s top page plus `0..200`,
//!   confirming zero divergence -- including at the wrap boundary, where both
//!   forms yield `0` for `w = 0xFFFFFFFF`. The source form is nonetheless the
//!   one written here, verbatim, because it is the authority; the equivalence
//!   is recorded only so that a future reader does not "simplify" it and
//!   assume the simplification was checked. Note both forms wrap to `0` for
//!   `w > 0xFFFFFFE0`; this port reproduces that wrapping with
//!   `wrapping_mul`, matching C++'s defined unsigned-overflow semantics
//!   exactly (this is *not* UB in C++ -- unsigned arithmetic wraps by
//!   standard -- so it is reproduced rather than deviated from).
//! - **`DrawAttribute`'s seven gaps are genuine and load-bearing.** The
//!   header says so in a comment ("Any missing gaps are purely intentional for
//!   the sake of keeping backwards compatibility"), and the gap set was
//!   derived two independent ways: by enumerating the declared values and
//!   subtracting from `0..28` (yielding `{3, 4, 6, 10, 17, 19, 20}`), and by
//!   counting -- 21 declared enumerators against `Count = 28` leaves exactly
//!   7 holes, which matches. Both the *value of each enumerator* and the
//!   *set of absent values* are pinned separately by tests, so neither can be
//!   tidied into a contiguous enum without a test failing.
//! - **`attributeName` deliberately returns `"Unknown"` for six values that
//!   *are* declared.** `FramebufferPair`, `PrimDepth`, `Convert`, `Key`,
//!   `ObjRenderMode` and `ExtendedFlags` all have `switch` cases missing and
//!   fall to `default`. Derived two ways: by listing the 15 `case` labels in
//!   `attributeName` against the 21 declared enumerators (difference of 6),
//!   and by reading each declared name off the enum and checking for a
//!   matching `case`. This is not an oversight to be "fixed" -- the names are
//!   used for saved draw-call filter files, so adding a name would change a
//!   persisted format. Pinned by test.
//! - **`DrawAttribute::ExtendedType` maps to the string `"Extended"`, not
//!   `"ExtendedType"`.** The only enumerator whose name string differs from
//!   its identifier. Pinned by test.
//! - **`1U << static_cast<uint32_t>(attribute)` is well-defined for every
//!   declared attribute.** The largest is `ExtendedFlags = 27`, and `1U << 27`
//!   fits `u32` with four bits to spare; `Count = 28` would still be in range
//!   but is excluded by the `assert(attribute < DrawAttribute::Count)`
//!   precondition. There is no shift-width UB to deviate from. The `assert` is
//!   a debug-only precondition (`NDEBUG`-compiled-out in release), rendered as
//!   `debug_assert!` per `rt64_common.rs`'s established precedent.
//! - **`identityRectScale`'s `IdentityScale = 1024` is unity in S5.10 fixed
//!   point**, derived two ways: as the literal `1024`, and as
//!   `1024 / 1024.0 == 1.0` under the RDP's S5.10 rect-scale encoding. Both
//!   `1024` and `-1024` are representable in `int16_t` (range
//!   `-32768..=32767`), so `-IdentityScale` involves no overflow and no UB.
//!   The predicate accepts *either sign* on each axis independently -- a
//!   mirrored rect still has identity scale -- and requires both axes to
//!   qualify (`&&`). All four sign combinations are pinned.
//! - **`validTexcoords` uses `<=`, not `<`.** A degenerate one-texel span
//!   where `min == max` is *valid*. Both axes must qualify (`&&`). The
//!   coordinates are `interop::int2`, i.e. **signed**, so the comparison is a
//!   signed one and negative texcoords compare correctly; ported as `i32`.
//! - **`usesViewport` is true for exactly `Perspective` and `Orthographic`.**
//!   The C++ `switch` falls through `Perspective` into `Orthographic`'s
//!   `return true` and sends `None`, `Rectangle` and `Triangle` to `default:
//!   return false`. All five variants are pinned individually so the
//!   true-set cannot drift.
//! - **`inProjection` reads only the *last* projection**, at index
//!   `projectionCount - 1`, and returns `false` outright when
//!   `projectionCount == 0` (guarding the underflow that `projectionCount - 1`
//!   would otherwise cause on an unsigned zero). Both fields must match
//!   (`&&`). Ported taking the last projection's `(transformsIndex, type)` as
//!   an `Option`, which represents "no projections" exactly and makes the
//!   underflow unrepresentable rather than merely guarded.
//! - **`isEmpty` is a 3-way `&&`**: zero game calls *and* both operation lists
//!   empty. `startFbDiscards` is deliberately **not** consulted -- a pair with
//!   discards but no calls and no operations is still "empty". That asymmetry
//!   is pinned by test, since it is the kind of omission a reader would
//!   assume is a bug.
//! - **`dumpTexture`'s `commonBytesOffset`/`commonBytesPerRow` are the
//!   standard N64 texels-to-bytes conversion.** `x << siz >> 1` parses as
//!   `((x << siz) >> 1)` (C++ `<<` and `>>` share precedence and associate
//!   left). Derived two ways: as the literal shift chain, and as
//!   `x * 2^siz / 2`, i.e. bytes-per-texel of `{0.5, 1, 2, 4}` for `siz`
//!   `{0, 1, 2, 3}` = `{4b, 8b, 16b, 32b}`. Both agree for every `siz` and a
//!   spread of inputs. The `>> 1` is an integer halving that **truncates**:
//!   for `siz = 0` (4bpp) an odd texel count loses the trailing nibble. That
//!   truncation is reproduced, not rounded away.
//! - **The Block branch's `>> (4 - siz)` divides by texels-per-64-bit-word.**
//!   Derived two ways: as the literal shift (`4-siz` = `{4,3,2,1}` giving
//!   divisors `{16,8,4,2}`), and from first principles (a TMEM word is 64
//!   bits, so it holds `{16, 8, 4, 2}` texels at `{4, 8, 16, 32}` bpp). The
//!   two agree exactly for all four sizes. `wordCount << 3` then converts
//!   words to bytes at 8 bytes per 64-bit word.
//! - **The Block branch uses *raw* `lrs`/`uls`; the Tile branch uses
//!   `>> 2`.** This is a genuine asymmetry in the source, not a transcription
//!   slip: load-block's `uls`/`lrs` are already in texel units, while
//!   load-tile's are in the RDP's 10.2 fixed-point texel coordinates and need
//!   the two-bit shift to become whole texels. Both orderings are pinned by
//!   separate tests so neither can be tidied into the other.
//! - **`std::max` returns its FIRST argument on a false comparison.** Both
//!   `std::max` calls are written as literal ternaries in the source's
//!   argument order: `commonBytesPerRow = std::max(commonBytesPerRow,
//!   uint32_t(loadTile.line) << 3U)` becomes
//!   `if line_bytes > common_bytes_per_row { line_bytes } else
//!   { common_bytes_per_row }`, preserving "first argument wins on a tie",
//!   and the nested `std::max(rdramCount, std::max(loadTileBpr,
//!   commonBytesPerRow) * height)` is expanded the same way, inner first.
//!   For `u32` the tie case is value-indistinguishable, so this is
//!   belt-and-braces rather than observable -- but it is written the
//!   source's way regardless.
//! - **The Block branch's padding bump to `commonBytesPerRow` happens
//!   *after* `rdramStart` is computed with the un-bumped value.** Ordering is
//!   load-bearing: `rdramStart` uses the original row stride, while the
//!   later `max(loadTileBpr, commonBytesPerRow) * height` sees the bumped
//!   one. Preserved exactly, and pinned by a test whose expectation differs
//!   under the two orderings.
//! - **`paletteTMEM = (RDP_TMEM_WORDS >> 1) + (CI4 ? (palette << 4) : 0)`.**
//!   Derived two ways: `512 >> 1 == 256` is the upper-half word base, and
//!   `palette << 4 == palette * 16` is the 16-word stride between CI4
//!   palettes; independently, the maximum CI4 palette index 15 lands at
//!   `256 + 240 = 496 < 512`, confirming all sixteen palettes fit the upper
//!   half exactly. For non-CI4 (`siz != G_IM_SIZ_4b`) the palette offset is
//!   zero and the base is used unchanged. The C++ declares this `int32_t`
//!   despite every input being unsigned; ported as `u32` since the value is
//!   provably in `256..=496` and never negative -- see Nonclaims.
//! - **The palette's byte count uses a *different formula shape* from the
//!   tile's.** The tile branch is `rowCount * commonBytesPerRow`; the palette
//!   is `(rowCount - 1) * paletteBytesPerRow + (wordsPerRow << 3)` -- full
//!   strides for all but the last row, then an exact word-count for the last.
//!   Note also that the palette's `wordsPerRow` is `(lrs>>2) - (uls>>2) + 1`
//!   in *texels*, whereas the tile branch's identically-named local is
//!   `(tileWidth >> (4 - siz)) + 1` in *words*. The two are pinned
//!   separately. `rowCount` is `>= 1` by construction (`1 + delta`), so
//!   `rowCount - 1` does not underflow when `lrt >= ult`; the port takes
//!   `row_count` as a caller-supplied `u32` and documents the precondition
//!   rather than silently saturating.
//!
//! ## Refused, with the deciding evidence
//!
//! Six of the twelve sources this card examined contribute **nothing**
//! portable, and three more contribute only fractions. Named refusals:
//!
//! - **`src/hle/rt64_microcode.h` (14 lines) -- refused, not cited.** Its
//!   entire content is `struct Microcode { uint32_t half1; uint32_t half2; };`.
//!   Field declaration order is not pinnable in safe Rust (field-init
//!   shorthand binds by identifier; see `rt64_shared_params.rs:255`), and this
//!   card makes no `repr(C)`/size/alignment/ABI claim, so a Rust struct here
//!   would assert nothing testable. `fn64-render`'s `microcode_identity.rs`
//!   already owns microcode *identity* from `src/gbi/rt64_gbi.cpp`'s
//!   recognition rows, a different and far more substantive concern.
//! - **`src/hle/rt64_transform_group.h` (28 lines) -- refused, not cited.**
//!   Thirteen default-initialized fields, every default being an already-owned
//!   `G_EX_*` constant from [`crate::rt64_extended_gbi`]
//!   (`G_EX_ID_AUTO`, `G_EX_COMPONENT_AUTO`, `G_EX_COMPONENT_SKIP`,
//!   `G_EX_ORDER_AUTO`, `G_EX_ASPECT_AUTO`, `G_EX_EDIT_NONE`). There is no
//!   arithmetic, no predicate and no derived constant; a port would restate
//!   thirteen imports. `crate::rt64_rigid_body` already consumes those
//!   constants where they drive behavior.
//! - **`src/hle/rt64_game_call.h` (39 lines) -- refused, not cited.** A
//!   four-member aggregate (`DrawCall`, `ShaderDescription`, and three
//!   anonymous sub-structs) plus a `#if SCRIPT_ENABLED` callback pointer. No
//!   behavior; the `SCRIPT_ENABLED` branch is preprocessor plumbing, and
//!   `ShaderDescription` is already owned by
//!   [`crate::rt64_shader_description`].
//! - **`src/hle/rt64_framebuffer_changes.h` (52 lines) -- refused, not
//!   cited.** Two struct declarations whose members are
//!   `std::unique_ptr<RenderTexture>` / `std::unique_ptr<...DescriptorSet>` /
//!   `std::map` -- RHI-bound resource handles with no arithmetic. The `.cpp`'s
//!   one genuine formula is ported; the header's declarations are not.
//! - **`src/hle/rt64_projection.h` (37 lines) -- refused, not cited.** The
//!   `Projection` struct's members are `std::vector<GameCall>`,
//!   `LightManager`, `std::vector<interop::PointLight>` and `FixedRect` --
//!   the HLE object graph, not behavior. Only the `Type` enum is needed to
//!   express `usesViewport`, and it is declared here as the minimal input to
//!   that predicate; the `.cpp`'s `usesViewport` is the cited, ported item.
//! - **`src/hle/rt64_framebuffer_pair.h` (64 lines) -- refused, not cited.**
//!   `rt64_frame_compatibility.rs` already ported the six scalar fields its
//!   predicates read and refused the rest with reasons this card agrees with.
//!   The `FlushReason` enum is a six-value tag with no associated behavior in
//!   any cited file; the `fastPaths` bitfield (`bool clearDepthOnly : 1`) is a
//!   layout construct this card makes no ABI claim about. The `.cpp`'s three
//!   portable facts are ported.
//! - **`FramebufferPair::earlyPresentCandidate` -- refused.** Its only
//!   non-trivial tests are `Blender::usesVisualizeCoverageCycle` (owned by
//!   [`crate::rt64_blender_analysis`]) and `FixedRect::fullyInside` (owned by
//!   [`crate::rt64_common`]). Re-deriving either would duplicate an
//!   adjudicated port; the loop scaffolding around them carries no arithmetic.
//! - **`FramebufferPair::reset` / `changeProjection`, `Projection::reset` /
//!   `addGameCall` / `addPointLight`, `FramebufferChangePool::reset` /
//!   `get` / `release` -- refused.** All are `Vec`/`std::map` mutation and
//!   field zeroing over the un-ported HLE object graph, plus `adjustVector`,
//!   which is defined in the uncited `src/common/rt64_common.h:112`.
//! - **`TextureManager::uploadEmpty` / `uploadTMEM` / `uploadTexture` /
//!   `removeHashes` -- refused.** `std::unordered_set` bookkeeping plus
//!   `XXH3`/`TMEMHasher` calls dispatched to a `TextureCache` this card does
//!   not model. `uploadTMEM`'s only arithmetic-adjacent lines are its two
//!   sanity bounds (`width > 0 && height > 0`, `width > 0x1000 ||
//!   height > 0x1000`), which are literal comparisons against literal
//!   constants with no derivation to check.
//! - **`dumpTexture`'s file I/O, `snprintf` naming and `json` serialization
//!   -- refused.** `std::ofstream`, `std::filesystem::path`, and
//!   `nlohmann::json` are host-I/O concerns with no arithmetic. The
//!   `G_TT_RGBA16`/`G_TT_IA16`/else mapping to `LoadTLUT` is a three-way tag
//!   translation over constants from an uncited header.
//!
//! ## Nonclaims
//!
//! - **No `repr(C)`, size, alignment, padding or ABI claim** is made by any
//!   type here. None carries `repr(C)`.
//! - **Field declaration order is not claimed to be pinned.** Rust's
//!   field-init shorthand binds by identifier, so a reordering of a struct's
//!   fields is not observable through these tests
//!   (`rt64_shared_params.rs:255`). Where declaration *order* matters in the
//!   source it is an ABI concern this card refuses; where *value* order
//!   matters (the `DrawAttribute` enumerators) it is pinned by value, not by
//!   position.
//! - **This module does not model the HLE object graph.** No `Workload`,
//!   `WorkloadQueue`, `GameFrame`, `GameScene`, `GameCall`, `LightManager`,
//!   `TextureCache`, `State`, `RenderWorker` or `std::map` pool is ported.
//!   Every function here takes already-resolved scalars.
//! - **No claim about `LoadOperation`'s or `LoadTile`'s field layout.** The
//!   `dumpTexture` arithmetic is ported as free functions over named `u32`
//!   scalars, deliberately not as methods on a reconstructed
//!   `LoadOperation`/`LoadTile`, so no field set, order or type is asserted
//!   for those uncited types beyond the bit widths of the values actually
//!   consumed.
//! - **`paletteTMEM`'s `int32_t` declaration is deliberately narrowed to
//!   `u32`.** Every operand (`RDP_TMEM_WORDS >> 1`, `palette << 4`) is
//!   non-negative and the sum is provably in `256..=496`, so no negative value
//!   is representable and the signedness carries no information. This is a
//!   type narrowing, not a behavior deviation: no input produces a different
//!   result. It is disclosed because the C++ type is visibly `int32_t`.
//! - **No claim that the `dumpTexture` extents are *correct*** in the sense of
//!   covering exactly the RDRAM a texture samples. They are reproduced as
//!   written. The source itself hedges ("Dump more RDRAM if necessary if it
//!   doesn't cover what the tile could possibly sample"), and the arithmetic
//!   is a debug-dump heuristic, not a hardware contract.
//! - **The Tile branch's `wordsPerRow` local is dead in the source** -- it is
//!   computed at `rt64_rdp_tmem.cpp:104` and never read (`rdramCount` uses
//!   `rowCount * commonBytesPerRow`). It is therefore **not** ported, and no
//!   claim is made about its value. This is recorded so a reader comparing the
//!   verbatim quote against the Rust does not think a line was lost. The
//!   *palette's* identically-named `wordsPerRow` is live and is ported.
//! - **No UB is reproduced and none needed deviating from.** Every shift
//!   width is provably in range for its type (`1U << 27` max; `>> (4 - siz)`
//!   with `siz <= 3` giving shifts of 1..4; `<< siz` with `siz <= 3`), and
//!   C++ unsigned overflow is defined-wrapping rather than UB, so the
//!   `wrapping_mul` in the alignment helper reproduces C++ semantics exactly
//!   rather than deviating from them. There is no DEVIATION-labelled test in
//!   this module.
//! - **No interpolation, lerp or float arithmetic of any kind** appears in the
//!   ported fraction, so the "lerp forms are not interchangeable in f32"
//!   hazard has no subject here. [`crate::rt64_interpolation_helpers`] retains
//!   sole ownership of projection lerp.
//! - **`DrawStatus`' `assert(attribute < DrawAttribute::Count)` is rendered as
//!   `debug_assert!`**, matching `rt64_common.rs`'s precedent for debug-only
//!   C++ `assert()`. In release builds the C++ assert compiles out and the
//!   shift proceeds; the Rust does the same. Since `Count = 28` and `1u32 <<
//!   28` is still in range, no release-mode UB is possible either way.

use crate::rt64_tmem_regions::RDP_TMEM_WORDS;

// -----------------------------------------------------------------------------
// rt64_framebuffer_changes.cpp -- FramebufferChangePool::use
// -----------------------------------------------------------------------------

/// `FramebufferChangePool::use`'s `const uint32_t Alignment = 32`
/// (`src/hle/rt64_framebuffer_changes.cpp:38`): change-pool textures are
/// rounded up to a multiple of this so buffers can be reused across differently
/// sized changes.
pub const FRAMEBUFFER_CHANGE_ALIGNMENT: u32 = 32;

/// `FramebufferChange::Type` (`src/hle/rt64_framebuffer_changes.h:22-25`), the
/// minimal input `FramebufferChangePool::use`'s reuse predicate compares.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum FramebufferChangeType {
    /// `FramebufferChange::Type::Color`.
    Color,
    /// `FramebufferChange::Type::Depth`.
    Depth,
}

/// `FramebufferChangePool::use`'s dimension round-up
/// (`src/hle/rt64_framebuffer_changes.cpp:39-40`):
/// `((v / 32) + ((v % 32) ? 1 : 0)) * 32`.
///
/// Written in the source's literal form rather than the equivalent
/// `(v + 31) / 32 * 32`; the two agree over the whole `u32` domain (verified
/// exhaustively), but the source's form is the authority.
///
/// The final multiply wraps for `v > 0xFFFF_FFE0`, exactly as C++'s defined
/// unsigned-overflow semantics do; `wrapping_mul` reproduces that rather than
/// panicking in debug builds.
#[must_use]
pub fn align_framebuffer_change_dimension(v: u32) -> u32 {
    let alignment = FRAMEBUFFER_CHANGE_ALIGNMENT;
    ((v / alignment) + u32::from(v % alignment != 0)).wrapping_mul(alignment)
}

/// `FramebufferChangePool::use`'s compatible-buffer test
/// (`src/hle/rt64_framebuffer_changes.cpp:44-51`): an entry is reusable when it
/// is not already `used` and its type and *aligned* dimensions all match.
///
/// `candidate_width`/`candidate_height` are the stored entry's already-aligned
/// dimensions; `requested_width`/`requested_height` are the caller's raw
/// request, which this function aligns before comparing, matching the source's
/// order of operations.
#[must_use]
pub fn framebuffer_change_is_reusable(
    candidate_used: bool,
    candidate_type: FramebufferChangeType,
    candidate_width: u32,
    candidate_height: u32,
    requested_type: FramebufferChangeType,
    requested_width: u32,
    requested_height: u32,
) -> bool {
    if candidate_used {
        return false;
    }

    let aligned_width = align_framebuffer_change_dimension(requested_width);
    let aligned_height = align_framebuffer_change_dimension(requested_height);
    (candidate_type == requested_type)
        && (candidate_width == aligned_width)
        && (candidate_height == aligned_height)
}

// -----------------------------------------------------------------------------
// rt64_draw_call.h / rt64_draw_call.cpp -- DrawAttribute, DrawStatus, DrawCall
// -----------------------------------------------------------------------------

/// `DrawAttribute` (`src/hle/rt64_draw_call.h:23-46`).
///
/// The header's own comment governs the numbering: *"The values of the
/// attributes must be preserved as they are for draw call filters that were
/// already saved using these constants. Any missing gaps are purely intentional
/// for the sake of keeping backwards compatibility."* The seven absent values
/// `{3, 4, 6, 10, 17, 19, 20}` are therefore part of the contract, and are
/// pinned by [`tests`] as a set alongside each present value.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(u32)]
pub enum DrawAttribute {
    /// `Zero = 0`.
    Zero = 0,
    /// `UID = 1`.
    Uid = 1,
    /// `Tris = 2`.
    Tris = 2,
    /// `Scissor = 5`.
    Scissor = 5,
    /// `Combine = 7`.
    Combine = 7,
    /// `Texture = 8`.
    Texture = 8,
    /// `OtherMode = 9`.
    OtherMode = 9,
    /// `GeometryMode = 11`.
    GeometryMode = 11,
    /// `PrimColor = 12`.
    PrimColor = 12,
    /// `EnvColor = 13`.
    EnvColor = 13,
    /// `FogColor = 14`.
    FogColor = 14,
    /// `FillColor = 15`.
    FillColor = 15,
    /// `BlendColor = 16`.
    BlendColor = 16,
    /// `Lights = 18`.
    Lights = 18,
    /// `FramebufferPair = 21`.
    FramebufferPair = 21,
    /// `PrimDepth = 22`.
    PrimDepth = 22,
    /// `Convert = 23`.
    Convert = 23,
    /// `Key = 24`.
    Key = 24,
    /// `ObjRenderMode = 25`.
    ObjRenderMode = 25,
    /// `ExtendedType = 26`.
    ExtendedType = 26,
    /// `ExtendedFlags = 27`.
    ExtendedFlags = 27,
}

/// `DrawAttribute::Count = 28` (`src/hle/rt64_draw_call.h:45`).
///
/// Kept out of [`DrawAttribute`] itself: it is a sentinel bound, not an
/// attribute, and `DrawStatus`' three `assert(attribute < DrawAttribute::Count)`
/// preconditions treat it as exclusive.
pub const DRAW_ATTRIBUTE_COUNT: u32 = 28;

impl DrawAttribute {
    /// Every declared `DrawAttribute`, in the header's declaration order.
    pub const ALL: [DrawAttribute; 21] = [
        DrawAttribute::Zero,
        DrawAttribute::Uid,
        DrawAttribute::Tris,
        DrawAttribute::Scissor,
        DrawAttribute::Combine,
        DrawAttribute::Texture,
        DrawAttribute::OtherMode,
        DrawAttribute::GeometryMode,
        DrawAttribute::PrimColor,
        DrawAttribute::EnvColor,
        DrawAttribute::FogColor,
        DrawAttribute::FillColor,
        DrawAttribute::BlendColor,
        DrawAttribute::Lights,
        DrawAttribute::FramebufferPair,
        DrawAttribute::PrimDepth,
        DrawAttribute::Convert,
        DrawAttribute::Key,
        DrawAttribute::ObjRenderMode,
        DrawAttribute::ExtendedType,
        DrawAttribute::ExtendedFlags,
    ];

    /// `static_cast<uint32_t>(attribute)`.
    #[must_use]
    pub const fn value(self) -> u32 {
        self as u32
    }

    /// `DrawCall::attributeName` (`src/hle/rt64_draw_call.cpp:13-48`).
    ///
    /// Six *declared* attributes have no `case` in the source's `switch` and
    /// fall to `default: return "Unknown"`: `FramebufferPair`, `PrimDepth`,
    /// `Convert`, `Key`, `ObjRenderMode` and `ExtendedFlags`. That is
    /// reproduced, not repaired -- these strings name saved draw-call filter
    /// files, so adding a name would change a persisted format.
    ///
    /// Note also that `ExtendedType` maps to `"Extended"`, the one enumerator
    /// whose string differs from its identifier.
    #[must_use]
    pub const fn attribute_name(self) -> &'static str {
        match self {
            DrawAttribute::Zero => "Zero",
            DrawAttribute::Uid => "UID",
            DrawAttribute::Tris => "Tris",
            DrawAttribute::Scissor => "Scissor",
            DrawAttribute::Combine => "Combine",
            DrawAttribute::Texture => "Texture",
            DrawAttribute::OtherMode => "OtherMode",
            DrawAttribute::GeometryMode => "GeometryMode",
            DrawAttribute::PrimColor => "PrimColor",
            DrawAttribute::EnvColor => "EnvColor",
            DrawAttribute::FogColor => "FogColor",
            DrawAttribute::FillColor => "FillColor",
            DrawAttribute::BlendColor => "BlendColor",
            DrawAttribute::Lights => "Lights",
            DrawAttribute::ExtendedType => "Extended",
            _ => "Unknown",
        }
    }
}

/// `DrawStatus` (`src/hle/rt64_draw_call.h:125-136`,
/// `src/hle/rt64_draw_call.cpp:57-90`): a `uint32_t` bitset over
/// [`DrawAttribute`] values.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct DrawStatus {
    /// `uint32_t changed`.
    pub changed: u32,
}

impl DrawStatus {
    /// `DrawStatus::DrawStatus()` (`.cpp:57-59`), which delegates to `reset()`.
    #[must_use]
    pub const fn new() -> Self {
        Self { changed: 0 }
    }

    /// `DrawStatus::DrawStatus(uint32_t v)` (`.cpp:61-63`).
    ///
    /// Note this constructor assigns `v` directly and does **not** call
    /// `reset()`, so a caller can construct a `DrawStatus` with any bit pattern
    /// -- including bits at the seven gap positions, which no `DrawAttribute`
    /// can set or clear.
    #[must_use]
    pub const fn from_bits(v: u32) -> Self {
        Self { changed: v }
    }

    /// `DrawStatus::reset` (`.cpp:65-67`), which calls `clearChanges()`.
    pub fn reset(&mut self) {
        self.clear_changes();
    }

    /// `DrawStatus::clearChanges` (`.cpp:69-71`).
    pub fn clear_changes(&mut self) {
        self.changed = 0;
    }

    /// `DrawStatus::clearChange` (`.cpp:73-76`):
    /// `changed &= ~(1U << static_cast<uint32_t>(attribute))`.
    pub fn clear_change(&mut self, attribute: DrawAttribute) {
        debug_assert!(attribute.value() < DRAW_ATTRIBUTE_COUNT);
        self.changed &= !(1u32 << attribute.value());
    }

    /// `DrawStatus::setChanged` (`.cpp:78-81`):
    /// `changed |= (1U << static_cast<uint32_t>(attribute))`.
    pub fn set_changed(&mut self, attribute: DrawAttribute) {
        debug_assert!(attribute.value() < DRAW_ATTRIBUTE_COUNT);
        self.changed |= 1u32 << attribute.value();
    }

    /// `DrawStatus::isChanged(DrawAttribute)` (`.cpp:83-86`).
    #[must_use]
    pub fn is_attribute_changed(&self, attribute: DrawAttribute) -> bool {
        debug_assert!(attribute.value() < DRAW_ATTRIBUTE_COUNT);
        (self.changed & (1u32 << attribute.value())) != 0
    }

    /// `DrawStatus::isChanged()` (`.cpp:88-90`): `changed != 0`.
    #[must_use]
    pub const fn is_any_changed(&self) -> bool {
        self.changed != 0
    }
}

impl Default for DrawStatus {
    fn default() -> Self {
        Self::new()
    }
}

/// `DrawCall::identityRectScale`'s `const int16_t IdentityScale = 1024`
/// (`src/hle/rt64_draw_call.cpp:51`): unity in the RDP's S5.10 rect-scale
/// encoding (`1024 / 1024.0 == 1.0`).
pub const RECT_IDENTITY_SCALE: i16 = 1024;

/// `DrawCall::identityRectScale` (`src/hle/rt64_draw_call.cpp:50-53`).
///
/// Either sign qualifies on each axis independently -- a mirrored rect still
/// has identity scale -- and both axes must qualify.
#[must_use]
pub fn identity_rect_scale(rect_dsdx: i16, rect_dtdy: i16) -> bool {
    let identity_scale = RECT_IDENTITY_SCALE;
    ((rect_dsdx == identity_scale) || (rect_dsdx == -identity_scale))
        && ((rect_dtdy == identity_scale) || (rect_dtdy == -identity_scale))
}

/// `DrawCallTile::validTexcoords` (`src/hle/rt64_draw_call.h:157-159`).
///
/// `interop::int2` is signed, so these are signed comparisons. The test is
/// `<=`, not `<`: a degenerate one-texel span (`min == max`) is valid.
#[must_use]
pub fn valid_texcoords(min_texcoord: [i32; 2], max_texcoord: [i32; 2]) -> bool {
    (min_texcoord[0] <= max_texcoord[0]) && (min_texcoord[1] <= max_texcoord[1])
}

// -----------------------------------------------------------------------------
// rt64_projection.cpp -- Projection::usesViewport
// -----------------------------------------------------------------------------

/// `Projection::Type` (`src/hle/rt64_projection.h:14-20`), the minimal input to
/// [`ProjectionType::uses_viewport`] and [`framebuffer_pair_in_projection`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ProjectionType {
    /// `Projection::Type::None`.
    None,
    /// `Projection::Type::Perspective`.
    Perspective,
    /// `Projection::Type::Orthographic`.
    Orthographic,
    /// `Projection::Type::Rectangle`.
    Rectangle,
    /// `Projection::Type::Triangle`.
    Triangle,
}

impl ProjectionType {
    /// `Projection::usesViewport` (`src/hle/rt64_projection.cpp:36-44`).
    ///
    /// The C++ `switch` falls through `Perspective` into `Orthographic`'s
    /// `return true`; `None`, `Rectangle` and `Triangle` reach
    /// `default: return false`.
    #[must_use]
    pub const fn uses_viewport(self) -> bool {
        match self {
            ProjectionType::Perspective | ProjectionType::Orthographic => true,
            _ => false,
        }
    }
}

// -----------------------------------------------------------------------------
// rt64_framebuffer_pair.cpp -- dither index, inProjection, isEmpty
// -----------------------------------------------------------------------------

/// `G_MDSFT_RGBDITHER` (`src/shared/rt64_f3d_defines.h:22`), the bit position of
/// the two-bit RGB dither selector inside the other-mode `H` word.
pub const G_MDSFT_RGBDITHER: u32 = 6;

/// `FramebufferPair::ditherPatterns`' slot count
/// (`src/hle/rt64_framebuffer_pair.h:51`: `std::array<uint32_t, 4>`).
pub const DITHER_PATTERN_COUNT: usize = 4;

/// `FramebufferPair::addGameCall`'s dither-pattern index
/// (`src/hle/rt64_framebuffer_pair.cpp:43`):
/// `(otherMode.rgbDither() >> G_MDSFT_RGBDITHER) & 0x3`.
///
/// Defined over the **raw other-mode `H` word**, deliberately not over
/// [`crate::state::OtherMode::rgb_dither`]. RT64's `rgbDither()` returns
/// `H & (3U << 6)` -- the bits left in place -- whereas fn64's `rgb_dither()`
/// already shifts them down before decoding to an enum. Substituting fn64's
/// accessor into RT64's expression would double-shift and yield `0` for all
/// four modes. Reproducing the whole chain from `H` makes that error
/// unrepresentable. See the module docs' "bit-level disagreement" note.
///
/// The trailing `& 0x3` is redundant given the accessor's own mask, and is
/// retained because the source writes it.
///
/// The result is always in `0..4`, so it is always a valid index into the
/// four-slot `ditherPatterns` histogram.
#[must_use]
pub fn dither_pattern_index(other_mode_h: u32) -> u32 {
    let rgb_dither = other_mode_h & (3u32 << G_MDSFT_RGBDITHER);
    (rgb_dither >> G_MDSFT_RGBDITHER) & 0x3
}

/// `FramebufferPair::inProjection` (`src/hle/rt64_framebuffer_pair.cpp:47-56`).
///
/// Only the *last* projection is consulted. `last_projection` is `None` when
/// the source's `projectionCount == 0`, which makes the `projectionCount - 1`
/// unsigned underflow unrepresentable rather than merely guarded.
#[must_use]
pub fn framebuffer_pair_in_projection(
    last_projection: Option<(u32, ProjectionType)>,
    transforms_index: u32,
    projection_type: ProjectionType,
) -> bool {
    if let Some((last_transforms_index, last_type)) = last_projection {
        if (last_transforms_index == transforms_index) && (last_type == projection_type) {
            return true;
        }
    }

    false
}

/// `FramebufferPair::isEmpty` (`src/hle/rt64_framebuffer_pair.cpp:68-70`).
///
/// `startFbDiscards` is deliberately **not** consulted: a pair carrying
/// discards but no game calls and no operations is still empty.
#[must_use]
pub const fn framebuffer_pair_is_empty(
    game_call_count: u32,
    start_fb_operations_len: usize,
    end_fb_operations_len: usize,
) -> bool {
    (game_call_count == 0) && (start_fb_operations_len == 0) && (end_fb_operations_len == 0)
}

// -----------------------------------------------------------------------------
// rt64_rdp_tmem.cpp -- dumpTexture's RDRAM and palette extent arithmetic
// -----------------------------------------------------------------------------

/// `LoadOperation::Type` (as read by `dumpTexture`,
/// `src/hle/rt64_rdp_tmem.cpp:93,101`), the minimal input selecting which
/// extent formula applies.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum LoadOperationType {
    /// `LoadOperation::Type::Block`.
    Block,
    /// `LoadOperation::Type::Tile`.
    Tile,
    /// Any other `LoadOperation::Type`, for which `dumpTexture` takes neither
    /// branch and leaves `rdramStart`/`rdramCount` at their initial
    /// `loadOp.texture.address` / `0`.
    Other,
}

/// `G_IM_SIZ_4b`, the `siz` encoding for 4-bit texels, as compared by
/// `dumpTexture`'s `const bool CI4 = (loadTile.siz == G_IM_SIZ_4b)`
/// (`src/hle/rt64_rdp_tmem.cpp:136`).
pub const G_IM_SIZ_4B: u32 = 0;

/// `dumpTexture`'s `commonBytesOffset` / `commonBytesPerRow` texel-to-byte
/// conversion (`src/hle/rt64_rdp_tmem.cpp:91-92`): `x << siz >> 1`, which
/// parses as `((x << siz) >> 1)`.
///
/// Equivalently `x * 2^siz / 2`, i.e. bytes-per-texel of `{0.5, 1, 2, 4}` for
/// `siz` `{0, 1, 2, 3}` = `{4b, 8b, 16b, 32b}`. The `>> 1` truncates, so an odd
/// texel count at 4bpp loses the trailing nibble; that is reproduced.
#[must_use]
pub fn texels_to_bytes(texels: u32, siz: u32) -> u32 {
    (texels.wrapping_shl(siz)) >> 1
}

/// The RDRAM extent `dumpTexture` computes for one texture
/// (`src/hle/rt64_rdp_tmem.cpp:88-111`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RdramDumpExtent {
    /// `rdramStart`.
    pub start: u32,
    /// `rdramCount`. The source only writes a file when this is `> 0`.
    pub count: u32,
}

/// `dumpTexture`'s RDRAM extent arithmetic
/// (`src/hle/rt64_rdp_tmem.cpp:88-111`).
///
/// Ordering is load-bearing in two places, and both are preserved:
///
/// - the Block branch computes `rdramStart` with the **un-bumped**
///   `commonBytesPerRow`, then bumps it for the later `max`;
/// - the Block branch reads **raw** `lrs`/`uls`, while the Tile branch reads
///   them `>> 2` (load-block coordinates are already whole texels; load-tile
///   coordinates are 10.2 fixed point).
///
/// Both `std::max` calls are written as ternaries in the source's argument
/// order, so the first argument wins a tie.
///
/// The Tile branch's `wordsPerRow` local (source line 104) is dead -- computed
/// and never read -- so it is not reproduced.
#[must_use]
#[allow(clippy::too_many_arguments)]
pub fn dump_texture_rdram_extent(
    load_op_type: LoadOperationType,
    load_op_texture_address: u32,
    load_op_texture_siz: u32,
    load_op_texture_width: u32,
    load_op_tile_uls: u32,
    load_op_tile_lrs: u32,
    load_op_tile_ult: u32,
    load_op_tile_lrt: u32,
    load_op_tile_siz: u32,
    load_tile_line: u32,
    load_tile_siz: u32,
    width: u32,
    height: u32,
) -> RdramDumpExtent {
    let mut rdram_start = load_op_texture_address;
    let mut rdram_count: u32 = 0;
    let common_bytes_offset = texels_to_bytes(load_op_tile_uls >> 2, load_op_texture_siz);
    let mut common_bytes_per_row = texels_to_bytes(load_op_texture_width, load_op_texture_siz);

    match load_op_type {
        LoadOperationType::Block => {
            // Raw `lrs`/`uls` here, not `>> 2`: load-block coordinates are
            // already whole texels.
            let word_count = ((load_op_tile_lrs.wrapping_sub(load_op_tile_uls))
                >> (4 - load_op_tile_siz))
                .wrapping_add(1);
            rdram_start = load_op_texture_address
                .wrapping_add(common_bytes_offset)
                .wrapping_add(common_bytes_per_row.wrapping_mul(load_op_tile_ult));
            rdram_count = word_count.wrapping_shl(3);

            // Increase the amount of RDRAM dumped by textures that require
            // padding when using load block. `std::max` returns its first
            // argument on a false comparison.
            let line_bytes = load_tile_line.wrapping_shl(3);
            common_bytes_per_row = if line_bytes > common_bytes_per_row {
                line_bytes
            } else {
                common_bytes_per_row
            };
        }
        LoadOperationType::Tile => {
            let row_count =
                1u32.wrapping_add((load_op_tile_lrt >> 2).wrapping_sub(load_op_tile_ult >> 2));
            rdram_start = load_op_texture_address
                .wrapping_add(common_bytes_offset)
                .wrapping_add(common_bytes_per_row.wrapping_mul(load_op_tile_ult >> 2));
            rdram_count = row_count.wrapping_mul(common_bytes_per_row);
        }
        LoadOperationType::Other => {}
    }

    // Dump more RDRAM if necessary if it doesn't cover what the tile could
    // possibly sample. Inner `std::max` first, both in source argument order.
    let load_tile_bpr = texels_to_bytes(width, load_tile_siz);
    let inner_max = if common_bytes_per_row > load_tile_bpr {
        common_bytes_per_row
    } else {
        load_tile_bpr
    };
    let candidate = inner_max.wrapping_mul(height);
    rdram_count = if candidate > rdram_count {
        candidate
    } else {
        rdram_count
    };

    RdramDumpExtent {
        start: rdram_start,
        count: rdram_count,
    }
}

/// `dumpTexture`'s `paletteTMEM`
/// (`src/hle/rt64_rdp_tmem.cpp:136-137`):
/// `(RDP_TMEM_WORDS >> 1) + (CI4 ? (palette << 4) : 0)`.
///
/// `RDP_TMEM_WORDS >> 1 == 256` is the upper-half word base, and
/// `palette << 4 == palette * 16` is the stride between CI4 palettes; the
/// maximum CI4 palette index 15 lands at `256 + 240 = 496 < 512`, so all
/// sixteen fit the upper half. For non-CI4 sizes the offset is zero.
///
/// The C++ declares this `int32_t`; it is narrowed to `u32` here because the
/// value is provably in `256..=496` and never negative. See the module docs'
/// Nonclaims.
#[must_use]
pub fn palette_tmem_word(load_tile_siz: u32, load_tile_palette: u32) -> u32 {
    let ci4 = load_tile_siz == G_IM_SIZ_4B;
    (RDP_TMEM_WORDS >> 1) + if ci4 { load_tile_palette << 4 } else { 0 }
}

/// The RDRAM extent `dumpTexture` computes for a palette
/// (`src/hle/rt64_rdp_tmem.cpp:139-144`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PaletteDumpExtent {
    /// `paletteRdramStart`.
    pub start: u32,
    /// `paletteRdramCount`. The source only writes a file when this is `> 0`.
    pub count: u32,
}

/// `dumpTexture`'s palette RDRAM extent arithmetic
/// (`src/hle/rt64_rdp_tmem.cpp:139-144`).
///
/// The byte count uses a **different formula shape** from the tile branch's:
/// `(rowCount - 1) * bytesPerRow + (wordsPerRow << 3)` -- full strides for all
/// but the last row, then an exact word count for the last -- where the tile
/// branch is a plain `rowCount * bytesPerRow`.
///
/// Note also that this `wordsPerRow` is `(lrs>>2) - (uls>>2) + 1` in *texels*,
/// whereas the tile branch's identically-named local is
/// `(tileWidth >> (4 - siz)) + 1` in *words*. They are different quantities
/// sharing a name.
///
/// `rowCount` is `1 + delta`, hence `>= 1` whenever `lrt >= ult`; the
/// `rowCount - 1` subtraction is reproduced with wrapping semantics matching
/// C++'s unsigned arithmetic rather than being saturated.
#[must_use]
pub fn dump_palette_rdram_extent(
    palette_texture_address: u32,
    palette_texture_siz: u32,
    palette_texture_width: u32,
    palette_tile_uls: u32,
    palette_tile_lrs: u32,
    palette_tile_ult: u32,
    palette_tile_lrt: u32,
) -> PaletteDumpExtent {
    let palette_bytes_offset = texels_to_bytes(palette_tile_uls >> 2, palette_texture_siz);
    let palette_bytes_per_row = texels_to_bytes(palette_texture_width, palette_texture_siz);
    let row_count = 1u32.wrapping_add((palette_tile_lrt >> 2).wrapping_sub(palette_tile_ult >> 2));
    let words_per_row =
        ((palette_tile_lrs >> 2).wrapping_sub(palette_tile_uls >> 2)).wrapping_add(1);
    let start = palette_texture_address
        .wrapping_add(palette_bytes_offset)
        .wrapping_add(palette_bytes_per_row.wrapping_mul(palette_tile_ult >> 2));
    let count = row_count
        .wrapping_sub(1)
        .wrapping_mul(palette_bytes_per_row)
        .wrapping_add(words_per_row.wrapping_shl(3));

    PaletteDumpExtent { start, count }
}

#[cfg(test)]
mod tests {
    use super::*;

    // -------------------------------------------------------------------------
    // FramebufferChangePool::use -- 32-alignment and reuse
    // -------------------------------------------------------------------------

    #[test]
    fn alignment_rounds_up_to_the_next_multiple_of_thirty_two() {
        // Hand-derived from ((v/32) + (v%32 ? 1 : 0)) * 32.
        // v=0:  (0 + 0) * 32 = 0     -- zero stays zero, it is already a multiple
        // v=1:  (0 + 1) * 32 = 32
        // v=31: (0 + 1) * 32 = 32
        // v=32: (1 + 0) * 32 = 32    -- exact multiple is NOT bumped
        // v=33: (1 + 1) * 32 = 64
        // v=64: (2 + 0) * 32 = 64
        // v=320:(10 + 0) * 32 = 320  -- a real N64 framebuffer width, already aligned
        // v=240:(7 + 1) * 32 = 256   -- a real N64 framebuffer height, 240 -> 256
        assert_eq!(align_framebuffer_change_dimension(0), 0);
        assert_eq!(align_framebuffer_change_dimension(1), 32);
        assert_eq!(align_framebuffer_change_dimension(31), 32);
        assert_eq!(align_framebuffer_change_dimension(32), 32);
        assert_eq!(align_framebuffer_change_dimension(33), 64);
        assert_eq!(align_framebuffer_change_dimension(64), 64);
        assert_eq!(align_framebuffer_change_dimension(320), 320);
        assert_eq!(align_framebuffer_change_dimension(240), 256);
    }

    #[test]
    fn alignment_is_idempotent_and_never_shrinks_its_input() {
        // Two properties that together pin "round UP to a multiple of 32":
        // aligning an aligned value is a no-op, and the result is >= the input
        // (over the non-wrapping domain).
        for v in 0u32..1024 {
            let a = align_framebuffer_change_dimension(v);
            assert_eq!(a % 32, 0, "v={v} produced non-multiple {a}");
            assert!(a >= v, "v={v} shrank to {a}");
            assert!(a - v < 32, "v={v} overshot to {a}");
            assert_eq!(
                align_framebuffer_change_dimension(a),
                a,
                "not idempotent at {v}"
            );
        }
    }

    #[test]
    fn alignment_wraps_exactly_where_cpp_unsigned_arithmetic_does() {
        // 0xFFFFFFE0 is the largest multiple of 32 in u32: 0xFFFFFFE0 / 32 =
        // 0x07FFFFFF, and 0x07FFFFFF * 32 = 0xFFFFFFE0 exactly.
        assert_eq!(align_framebuffer_change_dimension(0xFFFF_FFE0), 0xFFFF_FFE0);
        // One past it rounds up to 0x100000000, which wraps to 0 in u32 --
        // exactly what C++'s defined unsigned overflow produces.
        assert_eq!(align_framebuffer_change_dimension(0xFFFF_FFE1), 0);
        assert_eq!(align_framebuffer_change_dimension(0xFFFF_FFFF), 0);
    }

    #[test]
    fn reuse_requires_unused_matching_type_and_both_aligned_dimensions() {
        // A stored 320x256 Color entry (256 = aligned 240) matches a request
        // for 320x240 because the request is aligned before comparison.
        assert!(framebuffer_change_is_reusable(
            false,
            FramebufferChangeType::Color,
            320,
            256,
            FramebufferChangeType::Color,
            320,
            240,
        ));

        // Already in use -> never reusable, even on a perfect match.
        assert!(!framebuffer_change_is_reusable(
            true,
            FramebufferChangeType::Color,
            320,
            256,
            FramebufferChangeType::Color,
            320,
            240,
        ));

        // Type mismatch alone rejects.
        assert!(!framebuffer_change_is_reusable(
            false,
            FramebufferChangeType::Depth,
            320,
            256,
            FramebufferChangeType::Color,
            320,
            240,
        ));

        // Width mismatch alone rejects (321 aligns to 352, not 320).
        assert!(!framebuffer_change_is_reusable(
            false,
            FramebufferChangeType::Color,
            320,
            256,
            FramebufferChangeType::Color,
            321,
            240,
        ));

        // Height mismatch alone rejects (257 aligns to 288, not 256).
        assert!(!framebuffer_change_is_reusable(
            false,
            FramebufferChangeType::Color,
            320,
            256,
            FramebufferChangeType::Color,
            320,
            257,
        ));
    }

    #[test]
    fn reuse_compares_against_the_aligned_request_not_the_raw_one() {
        // This is the test that fails if alignment is dropped from the
        // comparison path: a stored 256-high entry must match a raw 240
        // request. If the raw value were compared, 256 != 240 and this
        // would reject.
        assert!(framebuffer_change_is_reusable(
            false,
            FramebufferChangeType::Depth,
            32,
            32,
            FramebufferChangeType::Depth,
            1,
            1,
        ));
        // And conversely a stored entry holding the RAW value must NOT match,
        // since stored entries are always aligned.
        assert!(!framebuffer_change_is_reusable(
            false,
            FramebufferChangeType::Depth,
            1,
            1,
            FramebufferChangeType::Depth,
            1,
            1,
        ));
    }

    // -------------------------------------------------------------------------
    // DrawAttribute -- values and the seven intentional gaps
    // -------------------------------------------------------------------------

    #[test]
    fn every_declared_attribute_holds_its_header_value() {
        // Transcribed one by one from rt64_draw_call.h:24-44.
        assert_eq!(DrawAttribute::Zero.value(), 0);
        assert_eq!(DrawAttribute::Uid.value(), 1);
        assert_eq!(DrawAttribute::Tris.value(), 2);
        assert_eq!(DrawAttribute::Scissor.value(), 5);
        assert_eq!(DrawAttribute::Combine.value(), 7);
        assert_eq!(DrawAttribute::Texture.value(), 8);
        assert_eq!(DrawAttribute::OtherMode.value(), 9);
        assert_eq!(DrawAttribute::GeometryMode.value(), 11);
        assert_eq!(DrawAttribute::PrimColor.value(), 12);
        assert_eq!(DrawAttribute::EnvColor.value(), 13);
        assert_eq!(DrawAttribute::FogColor.value(), 14);
        assert_eq!(DrawAttribute::FillColor.value(), 15);
        assert_eq!(DrawAttribute::BlendColor.value(), 16);
        assert_eq!(DrawAttribute::Lights.value(), 18);
        assert_eq!(DrawAttribute::FramebufferPair.value(), 21);
        assert_eq!(DrawAttribute::PrimDepth.value(), 22);
        assert_eq!(DrawAttribute::Convert.value(), 23);
        assert_eq!(DrawAttribute::Key.value(), 24);
        assert_eq!(DrawAttribute::ObjRenderMode.value(), 25);
        assert_eq!(DrawAttribute::ExtendedType.value(), 26);
        assert_eq!(DrawAttribute::ExtendedFlags.value(), 27);
        assert_eq!(DRAW_ATTRIBUTE_COUNT, 28);
    }

    #[test]
    fn the_seven_gaps_are_pinned_as_a_set_independently_of_the_values() {
        // Derived a second way: subtract the declared value set from 0..28.
        // 21 declared enumerators against Count=28 leaves exactly 7 holes.
        let declared: std::collections::BTreeSet<u32> =
            DrawAttribute::ALL.iter().map(|a| a.value()).collect();
        assert_eq!(
            declared.len(),
            21,
            "ALL must list every declared enumerator once"
        );

        let gaps: Vec<u32> = (0..DRAW_ATTRIBUTE_COUNT)
            .filter(|v| !declared.contains(v))
            .collect();
        assert_eq!(
            gaps,
            vec![3, 4, 6, 10, 17, 19, 20],
            "the intentional backwards-compatibility gaps changed"
        );
        assert_eq!(gaps.len(), (DRAW_ATTRIBUTE_COUNT as usize) - declared.len());

        // Every declared value is strictly below Count.
        for a in DrawAttribute::ALL {
            assert!(a.value() < DRAW_ATTRIBUTE_COUNT, "{a:?} is out of range");
        }
    }

    #[test]
    fn attribute_name_returns_unknown_for_the_six_declared_but_unnamed_values() {
        // The 15 attributes with an explicit `case` in attributeName.
        assert_eq!(DrawAttribute::Zero.attribute_name(), "Zero");
        assert_eq!(DrawAttribute::Uid.attribute_name(), "UID");
        assert_eq!(DrawAttribute::Tris.attribute_name(), "Tris");
        assert_eq!(DrawAttribute::Scissor.attribute_name(), "Scissor");
        assert_eq!(DrawAttribute::Combine.attribute_name(), "Combine");
        assert_eq!(DrawAttribute::Texture.attribute_name(), "Texture");
        assert_eq!(DrawAttribute::OtherMode.attribute_name(), "OtherMode");
        assert_eq!(DrawAttribute::GeometryMode.attribute_name(), "GeometryMode");
        assert_eq!(DrawAttribute::PrimColor.attribute_name(), "PrimColor");
        assert_eq!(DrawAttribute::EnvColor.attribute_name(), "EnvColor");
        assert_eq!(DrawAttribute::FogColor.attribute_name(), "FogColor");
        assert_eq!(DrawAttribute::FillColor.attribute_name(), "FillColor");
        assert_eq!(DrawAttribute::BlendColor.attribute_name(), "BlendColor");
        assert_eq!(DrawAttribute::Lights.attribute_name(), "Lights");

        // The one enumerator whose string differs from its identifier.
        assert_eq!(DrawAttribute::ExtendedType.attribute_name(), "Extended");

        // The six declared attributes that fall to `default: return "Unknown"`.
        // This is source behavior, not an omission to repair.
        for a in [
            DrawAttribute::FramebufferPair,
            DrawAttribute::PrimDepth,
            DrawAttribute::Convert,
            DrawAttribute::Key,
            DrawAttribute::ObjRenderMode,
            DrawAttribute::ExtendedFlags,
        ] {
            assert_eq!(a.attribute_name(), "Unknown", "{a:?} gained a name");
        }

        // Exactly six unnamed, counted independently.
        let unknown = DrawAttribute::ALL
            .iter()
            .filter(|a| a.attribute_name() == "Unknown")
            .count();
        assert_eq!(unknown, 6);
    }

    // -------------------------------------------------------------------------
    // DrawStatus
    // -------------------------------------------------------------------------

    #[test]
    fn draw_status_sets_clears_and_tests_the_attributes_own_bit() {
        let mut s = DrawStatus::new();
        assert_eq!(s.changed, 0);
        assert!(!s.is_any_changed());

        // ExtendedFlags = 27 is the highest declared attribute; 1u32 << 27 =
        // 0x0800_0000, computed independently as 134217728.
        s.set_changed(DrawAttribute::ExtendedFlags);
        assert_eq!(s.changed, 0x0800_0000);
        assert_eq!(s.changed, 134_217_728);
        assert!(s.is_attribute_changed(DrawAttribute::ExtendedFlags));
        assert!(s.is_any_changed());

        // A different attribute's bit is untouched.
        assert!(!s.is_attribute_changed(DrawAttribute::Zero));

        // Zero = 0 sets the low bit, so the two coexist as 0x0800_0001.
        s.set_changed(DrawAttribute::Zero);
        assert_eq!(s.changed, 0x0800_0001);

        // clear_change clears only its own bit.
        s.clear_change(DrawAttribute::ExtendedFlags);
        assert_eq!(s.changed, 0x0000_0001);
        assert!(s.is_attribute_changed(DrawAttribute::Zero));
        assert!(!s.is_attribute_changed(DrawAttribute::ExtendedFlags));

        // reset / clear_changes zero everything.
        s.reset();
        assert_eq!(s.changed, 0);
        assert!(!s.is_any_changed());
    }

    #[test]
    fn draw_status_bits_are_distinct_for_every_declared_attribute() {
        // Setting all 21 attributes must produce 21 distinct set bits, and the
        // seven gap bits must remain clear -- pinning that the bit position is
        // the enum VALUE, not its ordinal position in the declaration.
        let mut s = DrawStatus::new();
        for a in DrawAttribute::ALL {
            s.set_changed(a);
        }
        assert_eq!(s.changed.count_ones(), 21);

        for gap in [3u32, 4, 6, 10, 17, 19, 20] {
            assert_eq!(
                s.changed & (1u32 << gap),
                0,
                "gap bit {gap} was set; bit position is not tracking the enum value"
            );
        }

        // Independently: the expected mask, built from the value list.
        let expected: u32 = DrawAttribute::ALL
            .iter()
            .fold(0, |m, a| m | (1u32 << a.value()));
        assert_eq!(s.changed, expected);
        // And a third way: the full 0..28 mask minus the seven gaps.
        let all_28 = (1u32 << 28) - 1;
        let gap_mask: u32 = [3u32, 4, 6, 10, 17, 19, 20]
            .iter()
            .fold(0, |m, g| m | (1u32 << g));
        assert_eq!(s.changed, all_28 & !gap_mask);
    }

    #[test]
    fn from_bits_assigns_directly_and_does_not_reset() {
        // The uint32_t constructor writes `changed = v` with no masking, so
        // even gap bits survive -- they simply cannot be reached through
        // set_changed / clear_change.
        let s = DrawStatus::from_bits(0xFFFF_FFFF);
        assert_eq!(s.changed, 0xFFFF_FFFF);
        assert!(s.is_any_changed());
        assert!(s.is_attribute_changed(DrawAttribute::Zero));

        // A value with only gap bits set reads as "changed" overall but as
        // unchanged for every declared attribute.
        let gap_only: u32 = [3u32, 4, 6, 10, 17, 19, 20]
            .iter()
            .fold(0, |m, g| m | (1u32 << g));
        let g = DrawStatus::from_bits(gap_only);
        assert!(g.is_any_changed());
        for a in DrawAttribute::ALL {
            assert!(
                !g.is_attribute_changed(a),
                "{a:?} read as changed from gap bits"
            );
        }

        assert_eq!(DrawStatus::default(), DrawStatus::new());
    }

    // -------------------------------------------------------------------------
    // identityRectScale / validTexcoords
    // -------------------------------------------------------------------------

    #[test]
    fn identity_rect_scale_accepts_either_sign_on_each_axis_independently() {
        assert_eq!(RECT_IDENTITY_SCALE, 1024);
        // All four sign combinations qualify -- a mirrored rect still has
        // identity scale.
        assert!(identity_rect_scale(1024, 1024));
        assert!(identity_rect_scale(-1024, 1024));
        assert!(identity_rect_scale(1024, -1024));
        assert!(identity_rect_scale(-1024, -1024));
    }

    #[test]
    fn identity_rect_scale_rejects_any_non_unity_axis() {
        // Both axes must qualify (&&), so a single bad axis rejects.
        assert!(!identity_rect_scale(1023, 1024));
        assert!(!identity_rect_scale(1025, 1024));
        assert!(!identity_rect_scale(1024, 1023));
        assert!(!identity_rect_scale(1024, 0));
        assert!(!identity_rect_scale(0, 0));
        // 512 is half scale, 2048 is double -- neither is identity.
        assert!(!identity_rect_scale(512, 1024));
        assert!(!identity_rect_scale(1024, 2048));
        // -1023 is near-miss on the negative side.
        assert!(!identity_rect_scale(-1023, 1024));
    }

    #[test]
    fn valid_texcoords_admits_degenerate_spans_and_negative_coordinates() {
        // <= not <: a one-texel span is valid.
        assert!(valid_texcoords([0, 0], [0, 0]));
        assert!(valid_texcoords([5, 7], [5, 7]));
        assert!(valid_texcoords([0, 0], [31, 31]));

        // int2 is signed, so negative coordinates compare correctly.
        assert!(valid_texcoords([-10, -10], [-1, -1]));
        assert!(valid_texcoords([-10, -10], [10, 10]));

        // Either axis inverted rejects (&&).
        assert!(!valid_texcoords([1, 0], [0, 0]));
        assert!(!valid_texcoords([0, 1], [0, 0]));
        assert!(!valid_texcoords([1, 1], [0, 0]));
        assert!(!valid_texcoords([0, 0], [-1, 0]));
    }

    // -------------------------------------------------------------------------
    // Projection::usesViewport
    // -------------------------------------------------------------------------

    #[test]
    fn uses_viewport_is_true_for_exactly_perspective_and_orthographic() {
        assert!(ProjectionType::Perspective.uses_viewport());
        assert!(ProjectionType::Orthographic.uses_viewport());
        assert!(!ProjectionType::None.uses_viewport());
        assert!(!ProjectionType::Rectangle.uses_viewport());
        assert!(!ProjectionType::Triangle.uses_viewport());

        // Counted independently: exactly two of the five variants qualify.
        let all = [
            ProjectionType::None,
            ProjectionType::Perspective,
            ProjectionType::Orthographic,
            ProjectionType::Rectangle,
            ProjectionType::Triangle,
        ];
        assert_eq!(all.iter().filter(|t| t.uses_viewport()).count(), 2);
    }

    // -------------------------------------------------------------------------
    // FramebufferPair -- dither index, inProjection, isEmpty
    // -------------------------------------------------------------------------

    #[test]
    fn dither_pattern_index_recovers_all_four_modes_from_the_raw_h_word() {
        // Hand-derived: H bits 7:6 hold the selector. G_CD_MAGICSQ = 0<<6 = 0,
        // G_CD_BAYER = 1<<6 = 64, G_CD_NOISE = 2<<6 = 128,
        // G_CD_DISABLE = 3<<6 = 192.
        assert_eq!(dither_pattern_index(0 << 6), 0);
        assert_eq!(dither_pattern_index(1 << 6), 1);
        assert_eq!(dither_pattern_index(2 << 6), 2);
        assert_eq!(dither_pattern_index(3 << 6), 3);

        // Verified a second way against the literal G_CD_* macro values.
        assert_eq!(dither_pattern_index(0), 0);
        assert_eq!(dither_pattern_index(64), 1);
        assert_eq!(dither_pattern_index(128), 2);
        assert_eq!(dither_pattern_index(192), 3);
    }

    #[test]
    fn dither_pattern_index_ignores_every_other_bit_of_h() {
        // Every H bit outside 7:6 must be masked away. Setting all of them
        // must not perturb the index -- this is what the `& (3 << 6)` buys,
        // and dropping it would let high bits leak into the result.
        let outside = !(3u32 << G_MDSFT_RGBDITHER);
        for mode in 0u32..4 {
            let h = outside | (mode << G_MDSFT_RGBDITHER);
            assert_eq!(dither_pattern_index(h), mode, "h={h:#010x} leaked");
        }
        // And the index is always a valid ditherPatterns subscript.
        for h in [0u32, 1, 0xFFFF_FFFF, 0x8000_0040, 0x0000_00C0] {
            assert!((dither_pattern_index(h) as usize) < DITHER_PATTERN_COUNT);
        }
    }

    #[test]
    fn dither_pattern_index_disagrees_with_an_unshifted_accessor() {
        // The bit-level disagreement recorded in the module docs, pinned.
        // RT64's rgbDither() returns the bits IN PLACE; using that value
        // directly as an index would be out of bounds for three of four modes.
        for mode in 1u32..4 {
            let h = mode << G_MDSFT_RGBDITHER;
            let rt64_accessor_result = h & (3u32 << G_MDSFT_RGBDITHER);
            assert_ne!(
                rt64_accessor_result, mode,
                "RT64's accessor must NOT already be shifted down"
            );
            assert_eq!(rt64_accessor_result, mode * 64);
            assert!(
                (rt64_accessor_result as usize) >= DITHER_PATTERN_COUNT,
                "unshifted value must be an out-of-bounds subscript"
            );
            // The ported chain does the >> 6 that makes it in-bounds.
            assert_eq!(dither_pattern_index(h), mode);
        }
    }

    #[test]
    fn in_projection_matches_only_the_last_projection_on_both_fields() {
        // No projections -> always false (guards the projectionCount-1
        // underflow the C++ has to check for).
        assert!(!framebuffer_pair_in_projection(
            None,
            0,
            ProjectionType::Perspective
        ));

        // Both fields match -> true.
        assert!(framebuffer_pair_in_projection(
            Some((7, ProjectionType::Perspective)),
            7,
            ProjectionType::Perspective
        ));

        // transformsIndex differs -> false.
        assert!(!framebuffer_pair_in_projection(
            Some((7, ProjectionType::Perspective)),
            8,
            ProjectionType::Perspective
        ));

        // type differs -> false.
        assert!(!framebuffer_pair_in_projection(
            Some((7, ProjectionType::Perspective)),
            7,
            ProjectionType::Orthographic
        ));

        // Both differ -> false.
        assert!(!framebuffer_pair_in_projection(
            Some((7, ProjectionType::Rectangle)),
            8,
            ProjectionType::Triangle
        ));
    }

    #[test]
    fn is_empty_requires_zero_calls_and_both_operation_lists_empty() {
        assert!(framebuffer_pair_is_empty(0, 0, 0));

        // Any one of the three disqualifies.
        assert!(!framebuffer_pair_is_empty(1, 0, 0));
        assert!(!framebuffer_pair_is_empty(0, 1, 0));
        assert!(!framebuffer_pair_is_empty(0, 0, 1));
        assert!(!framebuffer_pair_is_empty(1, 1, 1));
    }

    // -------------------------------------------------------------------------
    // dumpTexture -- RDRAM and palette extents
    // -------------------------------------------------------------------------

    #[test]
    fn texels_to_bytes_matches_the_four_bit_depths() {
        // Derived twice: as ((t << siz) >> 1), and as t * 2^siz / 2.
        // siz=0 (4bpp): 0.5 bytes/texel -- and the >>1 TRUNCATES on odd counts.
        assert_eq!(texels_to_bytes(0, 0), 0);
        assert_eq!(texels_to_bytes(1, 0), 0); // 1 texel at 4bpp = half a byte -> 0
        assert_eq!(texels_to_bytes(2, 0), 1);
        assert_eq!(texels_to_bytes(3, 0), 1); // truncation, reproduced
        assert_eq!(texels_to_bytes(320, 0), 160);
        // siz=1 (8bpp): 1 byte/texel.
        assert_eq!(texels_to_bytes(320, 1), 320);
        assert_eq!(texels_to_bytes(1, 1), 1);
        // siz=2 (16bpp): 2 bytes/texel.
        assert_eq!(texels_to_bytes(320, 2), 640);
        // siz=3 (32bpp): 4 bytes/texel.
        assert_eq!(texels_to_bytes(320, 3), 1280);

        // Cross-check the whole table against the multiply/divide form.
        for siz in 0u32..4 {
            for t in [0u32, 1, 2, 3, 7, 64, 320, 1024] {
                assert_eq!(
                    texels_to_bytes(t, siz),
                    (t * (1 << siz)) >> 1,
                    "t={t} siz={siz}"
                );
            }
        }
    }

    #[test]
    fn block_load_extent_uses_raw_coordinates_and_bumps_the_row_stride_after_start() {
        // Hand-derived, 16bpp (siz=2) block load:
        //   commonBytesOffset  = ((uls>>2) << 2) >> 1 = ((0>>2)<<2)>>1 = 0
        //   commonBytesPerRow  = (width << 2) >> 1 = (64 << 2) >> 1 = 128
        //   wordCount = ((lrs - uls) >> (4 - 2)) + 1 = ((63 - 0) >> 2) + 1
        //             = 15 + 1 = 16          <-- RAW lrs/uls, not >>2
        //   rdramStart = 0x1000 + 0 + 128 * ult(0) = 0x1000
        //   rdramCount = 16 << 3 = 128
        //   then commonBytesPerRow = max(128, line(8) << 3 = 64) = 128
        //   loadTileBpr = (width(64) << 2) >> 1 = 128
        //   rdramCount = max(128, max(128, 128) * height(32) = 4096) = 4096
        let e = dump_texture_rdram_extent(
            LoadOperationType::Block,
            0x1000, // texture.address
            2,      // texture.siz (16bpp)
            64,     // texture.width
            0,      // tile.uls
            63,     // tile.lrs
            0,      // tile.ult
            0,      // tile.lrt (unused by Block)
            2,      // tile.siz
            8,      // loadTile.line
            2,      // loadTile.siz
            64,     // width
            32,     // height
        );
        assert_eq!(e.start, 0x1000);
        assert_eq!(e.count, 4096);

        // Now make the line-padding bump observable: line = 64 gives
        // 64 << 3 = 512 > 128, so commonBytesPerRow becomes 512 and
        // rdramCount = max(128, max(128, 512) * 32 = 16384) = 16384.
        // rdramStart must be UNCHANGED at 0x1000, because it was computed
        // before the bump -- that ordering is what this assertion pins.
        let bumped = dump_texture_rdram_extent(
            LoadOperationType::Block,
            0x1000,
            2,
            64,
            0,
            63,
            0,
            0,
            2,
            64, // loadTile.line -- large enough to bump
            2,
            64,
            32,
        );
        assert_eq!(bumped.start, 0x1000, "start must not see the bumped stride");
        assert_eq!(bumped.count, 16384);
    }

    #[test]
    fn block_load_start_offsets_by_the_unbumped_row_stride_times_ult() {
        // Same as above but with ult = 4, so rdramStart picks up
        // commonBytesPerRow * ult. With line=64 the bump would make the
        // stride 512; if the bump leaked into rdramStart it would read
        // 0x1000 + 512*4 = 0x1800 instead of 0x1000 + 128*4 = 0x1200.
        let e = dump_texture_rdram_extent(
            LoadOperationType::Block,
            0x1000,
            2,
            64,
            0,
            63,
            4,
            0,
            2,
            64,
            2,
            64,
            32,
        );
        assert_eq!(e.start, 0x1000 + 128 * 4);
        assert_eq!(e.start, 0x1200);
        assert_ne!(e.start, 0x1000 + 512 * 4);
    }

    #[test]
    fn tile_load_extent_shifts_coordinates_by_two_unlike_the_block_branch() {
        // Hand-derived, 16bpp (siz=2) tile load with 10.2 fixed-point coords:
        //   commonBytesOffset = ((uls>>2) << 2) >> 1 = ((16>>2)<<2)>>1
        //                     = ((4)<<2)>>1 = 16>>1 = 8
        //   commonBytesPerRow = (width(64) << 2) >> 1 = 128
        //   rowCount = 1 + ((lrt>>2) - (ult>>2)) = 1 + ((124>>2) - (8>>2))
        //            = 1 + (31 - 2) = 30
        //   rdramStart = 0x2000 + 8 + 128 * (8>>2 = 2) = 0x2000 + 8 + 256
        //              = 0x2108
        //   rdramCount = 30 * 128 = 3840
        //   loadTileBpr = (width(64) << 2) >> 1 = 128
        //   rdramCount = max(3840, max(128,128) * height(8) = 1024) = 3840
        let e = dump_texture_rdram_extent(
            LoadOperationType::Tile,
            0x2000, // texture.address
            2,      // texture.siz
            64,     // texture.width
            16,     // tile.uls (10.2 -> 4 texels)
            0,      // tile.lrs (unused by the ported Tile arithmetic)
            8,      // tile.ult (10.2 -> 2 rows)
            124,    // tile.lrt (10.2 -> 31 rows)
            2,      // tile.siz
            0,      // loadTile.line (unused by Tile)
            2,      // loadTile.siz
            64,     // width
            8,      // height
        );
        assert_eq!(e.start, 0x2108);
        assert_eq!(e.start, 0x2000 + 8 + 256);
        assert_eq!(e.count, 3840);
        assert_eq!(e.count, 30 * 128);
    }

    #[test]
    fn the_sample_coverage_max_can_raise_the_count_above_the_load_extent() {
        // Tile load whose loaded rows cover less than the tile can sample:
        //   commonBytesOffset = 0, commonBytesPerRow = (8<<1)>>1 = 8
        //   rowCount = 1 + (0 - 0) = 1
        //   rdramStart = 0x100, rdramCount = 1 * 8 = 8
        //   loadTileBpr = (width(64) << 1) >> 1 = 64
        //   inner max(commonBytesPerRow=8, loadTileBpr=64) -> the source writes
        //     std::max(loadTileBpr, commonBytesPerRow) = max(64, 8) = 64
        //   rdramCount = max(8, 64 * height(16) = 1024) = 1024
        let e = dump_texture_rdram_extent(
            LoadOperationType::Tile,
            0x100,
            1,
            8,
            0,
            0,
            0,
            0,
            1,
            0,
            1,
            64,
            16,
        );
        assert_eq!(e.start, 0x100);
        assert_eq!(e.count, 1024);
        assert_ne!(e.count, 8, "the coverage max must have raised the count");
    }

    #[test]
    fn a_non_block_non_tile_load_keeps_the_bare_texture_address_and_only_the_coverage_max() {
        // Neither branch taken: rdramStart stays at texture.address and
        // rdramCount starts at 0, so only the trailing coverage max applies.
        //   commonBytesPerRow = (16 << 1) >> 1 = 16
        //   loadTileBpr = (width(4) << 1) >> 1 = 4
        //   max(loadTileBpr=4, commonBytesPerRow=16) = 16; 16 * height(2) = 32
        //   rdramCount = max(0, 32) = 32
        let e = dump_texture_rdram_extent(
            LoadOperationType::Other,
            0xABCD,
            1,
            16,
            40,
            200,
            12,
            80,
            1,
            99,
            1,
            4,
            2,
        );
        assert_eq!(
            e.start, 0xABCD,
            "start must stay at the bare texture address"
        );
        assert_eq!(e.count, 32);
    }

    #[test]
    fn word_count_shift_divides_by_texels_per_sixty_four_bit_word() {
        // Pins >> (4 - siz) as "texels per 64-bit word", derived independently
        // as 16 >> siz. A block load of exactly one word's worth of texels
        // must yield wordCount = 1, hence rdramCount = 8 bytes before the
        // coverage max (which is suppressed here with height = 0).
        for siz in 0u32..4 {
            let texels_per_word = 16u32 >> siz;
            assert_eq!(texels_per_word, 1u32 << (4 - siz), "siz={siz}");

            // lrs - uls = texels_per_word - 1 -> wordCount = 0 + 1 = 1.
            let e = dump_texture_rdram_extent(
                LoadOperationType::Block,
                0,
                siz,
                0, // texture.width = 0 -> commonBytesPerRow = 0
                0,
                texels_per_word - 1,
                0,
                0,
                siz,
                0,
                siz,
                0, // width = 0 -> loadTileBpr = 0
                0, // height = 0 -> coverage max contributes 0
            );
            assert_eq!(e.count, 8, "siz={siz} should be exactly one 8-byte word");

            // One texel more crosses into a second word.
            let e2 = dump_texture_rdram_extent(
                LoadOperationType::Block,
                0,
                siz,
                0,
                0,
                texels_per_word,
                0,
                0,
                siz,
                0,
                siz,
                0,
                0,
            );
            assert_eq!(e2.count, 16, "siz={siz} should be exactly two 8-byte words");
        }
    }

    #[test]
    fn palette_tmem_word_lands_every_ci4_palette_inside_the_upper_half() {
        // Base is RDP_TMEM_WORDS >> 1 = 512 >> 1 = 256, verified two ways.
        assert_eq!(RDP_TMEM_WORDS, 512);
        assert_eq!(RDP_TMEM_WORDS >> 1, 256);

        // Non-CI4 sizes ignore the palette index entirely.
        for siz in 1u32..4 {
            for pal in 0u32..16 {
                assert_eq!(palette_tmem_word(siz, pal), 256, "siz={siz} pal={pal}");
            }
        }

        // CI4 strides 16 words per palette; all sixteen fit below 512.
        for pal in 0u32..16 {
            let w = palette_tmem_word(G_IM_SIZ_4B, pal);
            assert_eq!(w, 256 + pal * 16);
            assert_eq!(w, (RDP_TMEM_WORDS >> 1) + (pal << 4));
            assert!(w < RDP_TMEM_WORDS, "palette {pal} escaped TMEM at word {w}");
        }
        assert_eq!(palette_tmem_word(G_IM_SIZ_4B, 15), 496);
    }

    #[test]
    fn palette_extent_uses_the_last_row_word_count_not_a_full_stride() {
        // Hand-derived, 16bpp palette (siz=2):
        //   paletteBytesOffset = ((uls>>2) << 2) >> 1 = ((8>>2)<<2)>>1
        //                      = ((2)<<2)>>1 = 8>>1 = 4
        //   paletteBytesPerRow = (width(16) << 2) >> 1 = 32
        //   rowCount    = 1 + ((lrt>>2) - (ult>>2)) = 1 + ((12>>2)-(4>>2))
        //               = 1 + (3 - 1) = 3
        //   wordsPerRow = ((lrs>>2) - (uls>>2)) + 1 = ((20>>2)-(8>>2)) + 1
        //               = (5 - 2) + 1 = 4          <-- TEXELS here, not words
        //   start = 0x300 + 4 + 32 * (4>>2 = 1) = 0x300 + 4 + 32 = 0x324
        //   count = (3 - 1) * 32 + (4 << 3) = 64 + 32 = 96
        let e = dump_palette_rdram_extent(0x300, 2, 16, 8, 20, 4, 12);
        assert_eq!(e.start, 0x324);
        assert_eq!(e.start, 0x300 + 4 + 32);
        assert_eq!(e.count, 96);
        assert_eq!(e.count, 2 * 32 + 4 * 8);

        // A plain rowCount * bytesPerRow would give 3 * 32 = 96 here by
        // coincidence, so pin the shapes apart with a case where they differ:
        // width 32 (bytesPerRow 64), wordsPerRow 1 -> (3-1)*64 + 8 = 136,
        // whereas rowCount * bytesPerRow would be 3 * 64 = 192.
        let d = dump_palette_rdram_extent(0, 2, 32, 0, 0, 4, 12);
        assert_eq!(d.count, 136);
        assert_ne!(d.count, 192, "palette must not use the tile branch's shape");
    }

    #[test]
    fn palette_extent_of_a_single_row_is_exactly_its_word_count() {
        // rowCount = 1 collapses the (rowCount-1) term to zero, leaving only
        // (wordsPerRow << 3): ult == lrt -> 1 + 0 = 1 row.
        //   wordsPerRow = ((32>>2) - (0>>2)) + 1 = 8 + 1 = 9
        //   count = 0 * bytesPerRow + (9 << 3) = 72
        let e = dump_palette_rdram_extent(0x40, 2, 16, 0, 32, 0, 0);
        assert_eq!(e.count, 72);
        assert_eq!(e.count, 9 * 8);
        // start = 0x40 + 0 + 32 * 0 = 0x40
        assert_eq!(e.start, 0x40);
    }
}
