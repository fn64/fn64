//! `DrawCallKey`/`DrawCallMask` comparison and `PresetDrawCall::matches()`: a
//! literal port of the permitted MIT RT64 source pinned at commit
//! `5473732a822a4423b5696e7cb18fecc425a59875` (`docs/RT64-PORT-AUTHORITY.md`),
//! `src/preset/rt64_preset_draw_call.h` (SHA-256 of the whole file,
//! `41184b01d3992d49f8df340fc4ceda2edcb4d9b86f199489fa9ddade549f5a52`,
//! 78 lines) + `src/preset/rt64_preset_draw_call.cpp` (SHA-256 of the whole
//! file, `c0a432a5a5debbb198c8f1d8b5514901b81ff584122a87697afc21adb6d615a7`,
//! 486 lines). Both digests were computed independently here with
//! `shasum -a 256` against the pinned checkout at
//! `src/preset/rt64_preset_draw_call.{h,cpp}` and cross-checked verbatim
//! against `docs/rt64-port-inventory.json`'s
//! `files[path="src/preset/rt64_preset_draw_call.{h,cpp}"].sources.port.sha256`,
//! which records the identical two digests. This module ports only the
//! struct layout (`DrawCallKey`, `DrawCallMask`) and the comparison/matching
//! functions (`DrawCallKey::operator==`, `DrawCallKey::fromDrawCall`,
//! `DrawCallMask::defaultAll`, `PresetDrawCall::matches`) out of the `.h`'s
//! 78 lines and the `.cpp`'s first ~188 lines (through the end of
//! `matches()`); everything after that in the `.cpp` --
//! `PresetDrawCallLibrary` (JSON I/O, a material cache keyed by
//! `DrawCallKey::hash()`) and the wall-to-wall-ImGui
//! `PresetDrawCallLibraryInspector` -- is not touched, matching the M8.9
//! ticket's exclusions.
//!
//! ```text
//! // rt64_preset_draw_call.h (struct declarations, lines 19-47)
//! struct DrawCallKey {
//!     uint64_t tmemHashes[8];
//!     interop::ColorCombiner colorCombiner;
//!     interop::OtherMode otherMode;
//!     uint32_t geometryMode;
//!
//!     uint64_t hash() const;
//!     static DrawCallKey fromDrawCall(const DrawCall &call, const DrawData &drawData, const TextureCache &textureCache);
//!     bool operator==(const DrawCallKey &k) const;
//! };
//!
//! struct DrawCallMask {
//!     uint32_t otherModeL;
//!     uint32_t otherModeH;
//!     uint32_t geometryMode;
//!     uint64_t attribute;
//!
//!     static DrawCallMask defaultAll();
//! };
//!
//! struct PresetDrawCall : public PresetBase {
//!     DrawCallKey key;
//!     DrawCallMask mask;
//!     std::string materialPresetName;
//!
//!     bool matches(const DrawCallKey &otherKey) const;
//!     virtual bool readJson(const json &jsonObj) override;
//!     virtual bool writeJson(json &jsonObj) const override;
//! };
//!
//! // rt64_preset_draw_call.cpp (lines 56-188)
//! DrawCallKey DrawCallKey::fromDrawCall(const DrawCall &call, const DrawData &drawData, const TextureCache &textureCache) {
//!     DrawCallKey key;
//!     memset(key.tmemHashes, 0, sizeof(key.tmemHashes));
//!     key.colorCombiner = call.colorCombiner;
//!     key.otherMode = call.otherMode;
//!     key.geometryMode = call.geometryMode;
//!
//!     for (uint32_t t = 0; t < call.tileCount; t++) {
//!         const auto &callTile = drawData.callTiles[call.tileIndex + t];
//!         const uint64_t tmemHash = callTile.tmemHashOrID;
//!         if (!callTile.tileCopyUsed && (tmemHash > 0)) {
//!             key.tmemHashes[t] = tmemHash;
//!         }
//!     }
//!
//!     return key;
//! }
//!
//! bool DrawCallKey::operator==(const DrawCallKey &k) const {
//!     return memcmp(this, &k, sizeof(DrawCallKey));
//! }
//!
//! // DrawCallMask
//!
//! DrawCallMask DrawCallMask::defaultAll() {
//!     DrawCallMask m;
//!     m.otherModeL = 0xFFFFFFFFU;
//!     m.otherModeH = 0xFFFFFFFFU;
//!     m.geometryMode = 0xFFFFFFFFU;
//!     m.attribute = 0xFFFFFFFFFFFFFFFFULL;
//!     return m;
//! }
//!
//! // PresetDrawCall
//!
//! bool PresetDrawCall::matches(const DrawCallKey &otherKey) const {
//!     if (mask.attribute & (1ULL << static_cast<uint32_t>(DrawAttribute::Texture))) {
//!         auto keyIncludesHashesOf = [](const DrawCallKey &A, const DrawCallKey &B) {
//!             const int HashCount = int(std::size(A.tmemHashes));
//!             for (int i = 0; i < HashCount; i++) {
//!                 if (A.tmemHashes[i] != 0) {
//!                     bool found = false;
//!                     for (int j = 0; (j < HashCount) && !found && (B.tmemHashes[j] != 0); j++) {
//!                         found = (A.tmemHashes[i] == B.tmemHashes[j]);
//!                     }
//!
//!                     if (!found) {
//!                         return false;
//!                     }
//!                 }
//!             }
//!
//!             return true;
//!         };
//!
//!         if (!keyIncludesHashesOf(key, otherKey) || !keyIncludesHashesOf(otherKey, key)) {
//!             return false;
//!         }
//!     }
//!
//!     if (mask.attribute & (1ULL << static_cast<uint32_t>(DrawAttribute::Combine))) {
//!         if ((key.colorCombiner.L != otherKey.colorCombiner.L) && (key.colorCombiner.H != otherKey.colorCombiner.H)) {
//!             return false;
//!         }
//!     }
//!
//!     if (mask.attribute & (1ULL << static_cast<uint32_t>(DrawAttribute::OtherMode))) {
//!         if ((key.otherMode.L & mask.otherModeL) != (otherKey.otherMode.L & mask.otherModeL)) {
//!             return false;
//!         }
//!
//!         if ((key.otherMode.H & mask.otherModeH) != (otherKey.otherMode.H & mask.otherModeH)) {
//!             return false;
//!         }
//!     }
//!
//!     if (mask.attribute & (1ULL << static_cast<uint32_t>(DrawAttribute::GeometryMode))) {
//!         if ((key.geometryMode & mask.geometryMode) != (otherKey.geometryMode & mask.geometryMode)) {
//!             return false;
//!         }
//!     }
//!
//!     return true;
//! }
//! ```
//!
//! The `DrawAttribute` bit values (`src/hle/rt64_draw_call.h`, not re-cited
//! here since only the four values `matches()` reads are used: `Combine = 7`,
//! `Texture = 8`, `OtherMode = 9`, `GeometryMode = 11`) are reproduced as
//! plain `u64` shift amounts below rather than as a ported enum, since the
//! enum itself (`DrawAttribute::Count = 28` values, most unrelated to
//! matching) is out of this ticket's scope.
//!
//! **Reuse, not new type**: this module reuses no existing fn64 type --
//! `DrawCallKey`/`DrawCallMask`/`PresetDrawCall` (renamed
//! `RawDrawCallSample` for the un-ported `fromDrawCall` input, see below)
//! have no prior Rust equivalent anywhere in this workspace.
//!
//! `fromDrawCall`'s C++ signature takes a live `DrawCall`, `DrawData`, and
//! `TextureCache` -- none of which exist in `fn64-render-wgpu`, and pulling
//! them in would be exactly the kind of production-wiring decision this
//! ticket excludes. Its actual behavior only samples four fields off
//! `DrawCall` (`colorCombiner`, `otherMode`, `geometryMode`, `tileCount`,
//! `tileIndex`) and two fields off each `DrawCallTile` it walks
//! (`tmemHashOrID`, `tileCopyUsed`) -- see `src/hle/rt64_draw_call.h` lines
//! 138-155 for `DrawCallTile`'s field shapes, read only to confirm these two
//! fields' types (`uint64_t`, `bool`); no other content of that file is
//! ported. `from_raw_sample` below is `fromDrawCall` ported against a
//! minimal local struct (`RawDrawCallSample`) holding exactly those sampled
//! fields, as the ticket instructs, not the live engine types.
//!
//! ## Admitted domain
//!
//! - **`DrawCallKey::eq` (named `eq`, not `PartialEq::eq`, to keep the
//!   inversion below impossible to invoke by accident through `==`)**:
//!   ports `operator==`'s literal body, `return memcmp(this, &k,
//!   sizeof(DrawCallKey));`. `memcmp` returns 0 when the compared bytes are
//!   identical and nonzero otherwise; C++ implicitly converts that `int` to
//!   `bool` via "nonzero is true". The result: this operator, despite its
//!   name, returns `true` when the two keys DIFFER and `false` when they are
//!   byte-identical -- inverted from what `==` conventionally means. This is
//!   preserved exactly, not "fixed" to real equality; a characterization
//!   test locks in the inversion by name.
//! - **`DrawCallMask::default_all`**: all four fields set to their type's
//!   all-ones value (`u32::MAX` for the three `u32` fields, `u64::MAX` for
//!   `attribute`).
//! - **`PresetDrawCall::matches` mask semantics**: `mask.attribute` is a
//!   bitmask of `DrawAttribute` values (`Combine`, `Texture`, `OtherMode`,
//!   `GeometryMode`, tested via `1u64 << attribute_value`). When an
//!   attribute's bit is **clear**, that entire attribute group is skipped --
//!   not compared at all, i.e. **ignored**, not "must match zero" and not a
//!   wildcard-token concept (there is no third value; absence from the mask
//!   is itself the ignore signal). When an attribute's bit is **set**, the
//!   attribute's specific comparison runs and can independently return
//!   `false` from the whole function.
//! - **Texture comparison** (bit `Texture = 8`): both directions of a
//!   nonzero-hash subset check must hold (`keyIncludesHashesOf(key,
//!   otherKey) && keyIncludesHashesOf(otherKey, key)`), making the pair a
//!   full set-equality check over each key's nonzero `tmemHashes` entries --
//!   with one asymmetric quirk: the inner scan of `B.tmemHashes` stops at
//!   `B`'s **first zero entry** (`(j < HashCount) && !found && (B.tmemHashes[j]
//!   != 0)`), so a zero "hole" before the end of `B`'s array blocks matching
//!   against any nonzero hash placed after that hole, even though `A`'s scan
//!   does not require its own hashes to be contiguous. The inner loop has no
//!   `break`; it terminates purely through its three-way loop condition
//!   (`j < HashCount`, `!found`, `B.tmemHashes[j] != 0`), which is preserved
//!   as a `while` loop with the identical three-way condition rather than a
//!   `for` with a `break`, to keep the "no break" shape visible.
//! - **Combine comparison** (bit `Combine = 7`): `(key.colorCombiner.L !=
//!   otherKey.colorCombiner.L) && (key.colorCombiner.H !=
//!   otherKey.colorCombiner.H)` -- an `&&`, not `||`. This means a mismatch
//!   is only detected when **both** `L` and `H` differ; if exactly one of
//!   the two differs, `matches()` does NOT reject on this attribute. This
//!   reads as a bug relative to the other three fields' all-`||`-shaped
//!   rejection, but it is what the pinned source does, and hazard #4
//!   (asymmetric branches must not be normalized) requires porting it
//!   exactly as written, not "corrected" to `||`.
//! - **OtherMode comparison** (bit `OtherMode = 9`): two independent
//!   sub-checks, `L` then `H`, each masked with the matching
//!   `DrawCallMask` field before comparison (`(key.otherMode.L &
//!   mask.otherModeL) != (otherKey.otherMode.L & mask.otherModeL)`) and each
//!   independently able to return `false`. Within this masked comparison, a
//!   *bit* cleared in `mask.otherModeL`/`mask.otherModeH` is masked out of
//!   both operands before the `!=`, so that bit can never cause a mismatch
//!   regardless of its value in either key -- distinct from the
//!   attribute-level mask semantics above, which skip the whole field.
//! - **GeometryMode comparison** (bit `GeometryMode = 11`): one masked
//!   check, same bit-level masked-equality shape as OtherMode.
//! - **Short-circuit order**: Texture, then Combine, then OtherMode-L, then
//!   OtherMode-H, then GeometryMode, then (if nothing rejected) `true`. Any
//!   attribute whose bit is set and whose comparison fails returns `false`
//!   immediately without evaluating later attributes -- preserved exactly
//!   via early `return false` on each branch, in this order.
//! - **`from_raw_sample`** (`fromDrawCall`): copies `color_combiner`,
//!   `other_mode`, `geometry_mode` straight from the input sample; starts
//!   `tmem_hashes` fully zeroed; then for each of the sample's tiles (in
//!   index order, `t = 0..tile_count`), writes `tmem_hashes[t] =
//!   tile.tmem_hash_or_id` only when `!tile.tile_copy_used &&
//!   tile.tmem_hash_or_id > 0`; tiles that fail that condition leave that
//!   slot at its zeroed default. Slots beyond `tile_count` (up to the fixed
//!   8) also stay zero. A `tile_count` greater than 8 is out of the ported
//!   domain (the C++ writes into a fixed `uint64_t tmemHashes[8]` with no
//!   bounds check, i.e. undefined behavior on overflow); this port makes
//!   that case a loud panic instead of silently reproducing UB, and this is
//!   flagged as a deliberate deviation, not a claim about the original's
//!   behavior at that boundary.
//!
//! ## Nonclaims
//!
//! No GPU, no wgpu resource, no bind group, no shader. No production
//! wiring: this module is not `pub use`d from the crate root and nothing
//! in `fn64-render-wgpu` calls it. No parity or performance claim -- this is
//! a CPU-only characterization of the comparison/matching logic, not a
//! validated match against real RT64 runtime output. Explicitly NOT ported,
//! per the M8.9 ticket:
//! - `DrawCallKey::hash()` (`XXH3_64bits(this, sizeof(DrawCallKey))`) --
//!   pulling in `xxHash` is a dependency decision the ticket calls out as a
//!   standing reject for a mechanical port card; `PresetDrawCallLibrary`'s
//!   hash-keyed material cache (`findMaterialsInCache`, `cachedKeyMap`,
//!   `cachedKeyMaterialMap`) is downstream of `hash()` and is not ported for
//!   the same reason.
//! - `PresetDrawCallLibraryInspector` and all of `PresetDrawCallLibrary`'s
//!   and `PresetDrawCall`'s JSON I/O (`to_json`/`from_json`/`readJson`/
//!   `writeJson`) -- wall-to-wall ImGui and `nlohmann::json`, the excluded
//!   "inspector half" per the ticket.
//! - `PresetBase` (the parent type `PresetDrawCall` inherits `enabled`
//!   from in the original) is not ported; this module's `PresetDrawCall`
//!   analog carries only `key`, `mask`, and skips `materialPresetName`
//!   (a plain `String` with no comparison semantics of its own) and
//!   `enabled` (never read by `matches()` itself in the pinned source).

/// Bit position of `DrawAttribute::Combine` in `DrawCallMask::attribute`,
/// per `src/hle/rt64_draw_call.h`'s `DrawAttribute` enum (`Combine = 7`).
const ATTR_COMBINE: u32 = 7;
/// Bit position of `DrawAttribute::Texture` (`Texture = 8`).
const ATTR_TEXTURE: u32 = 8;
/// Bit position of `DrawAttribute::OtherMode` (`OtherMode = 9`).
const ATTR_OTHER_MODE: u32 = 9;
/// Bit position of `DrawAttribute::GeometryMode` (`GeometryMode = 11`).
const ATTR_GEOMETRY_MODE: u32 = 11;

/// Literal port of `DrawCallKey` (`rt64_preset_draw_call.h` lines 19-28),
/// minus `hash()` (excluded, see module doc). `interop::ColorCombiner` and
/// `interop::OtherMode` (`shared/rt64_color_combiner.h`,
/// `shared/rt64_other_mode.h`) are each a plain `{ uint L; uint H; }` pair;
/// their fields are inlined here as `color_combiner_l`/`color_combiner_h`
/// and `other_mode_l`/`other_mode_h` rather than nesting two 2-field structs,
/// since nothing else of either type is ported.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct DrawCallKey {
    pub tmem_hashes: [u64; 8],
    pub color_combiner_l: u32,
    pub color_combiner_h: u32,
    pub other_mode_l: u32,
    pub other_mode_h: u32,
    pub geometry_mode: u32,
}

impl DrawCallKey {
    /// Literal port of `DrawCallKey::operator==` (`rt64_preset_draw_call.cpp`
    /// lines 74-76): `return memcmp(this, &k, sizeof(DrawCallKey));`.
    /// `memcmp` is nonzero when the two byte images DIFFER, and C++ treats
    /// nonzero as `true` -- so this returns `true` when `self` and `k` are
    /// NOT byte-identical, and `false` when they are. This is the inverse of
    /// what `==` conventionally means; the name `eq` (not `PartialEq::eq`)
    /// is deliberate so this can never be reached through Rust's `==`
    /// operator by accident. Two keys are compared byte-for-byte across
    /// every field in declaration order (`tmem_hashes`, then the four
    /// combiner/other-mode words, then `geometry_mode`), matching
    /// `memcmp`'s whole-struct scope.
    pub fn eq(&self, k: &DrawCallKey) -> bool {
        self != k
    }
}

/// Literal port of `DrawCallMask` (`rt64_preset_draw_call.h` lines 30-37).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct DrawCallMask {
    pub other_mode_l: u32,
    pub other_mode_h: u32,
    pub geometry_mode: u32,
    pub attribute: u64,
}

impl DrawCallMask {
    /// Literal port of `DrawCallMask::defaultAll` (`rt64_preset_draw_call.cpp`
    /// lines 96-103): every field set to its type's all-ones value.
    pub fn default_all() -> DrawCallMask {
        DrawCallMask {
            other_mode_l: 0xFFFF_FFFFu32,
            other_mode_h: 0xFFFF_FFFFu32,
            geometry_mode: 0xFFFF_FFFFu32,
            attribute: 0xFFFF_FFFF_FFFF_FFFFu64,
        }
    }
}

/// The subset of `PresetDrawCall` (`rt64_preset_draw_call.h` lines 39-47)
/// this ticket ports: `key` and `mask`. `materialPresetName` and the
/// inherited `PresetBase::enabled` are excluded -- see module doc's
/// Nonclaims.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct PresetDrawCall {
    pub key: DrawCallKey,
    pub mask: DrawCallMask,
}

impl PresetDrawCall {
    /// Literal port of `PresetDrawCall::matches` (`rt64_preset_draw_call.cpp`
    /// lines 140-188). See module doc's "Admitted domain" for the full
    /// semantics of each branch; the short-circuit order here is Texture,
    /// Combine, OtherMode-L, OtherMode-H, GeometryMode, matching the source
    /// exactly.
    pub fn matches(&self, other_key: &DrawCallKey) -> bool {
        if self.mask.attribute & (1u64 << ATTR_TEXTURE) != 0 {
            if !key_includes_hashes_of(&self.key, other_key)
                || !key_includes_hashes_of(other_key, &self.key)
            {
                return false;
            }
        }

        if self.mask.attribute & (1u64 << ATTR_COMBINE) != 0
            && self.key.color_combiner_l != other_key.color_combiner_l
            && self.key.color_combiner_h != other_key.color_combiner_h
        {
            return false;
        }

        if self.mask.attribute & (1u64 << ATTR_OTHER_MODE) != 0 {
            if (self.key.other_mode_l & self.mask.other_mode_l)
                != (other_key.other_mode_l & self.mask.other_mode_l)
            {
                return false;
            }

            if (self.key.other_mode_h & self.mask.other_mode_h)
                != (other_key.other_mode_h & self.mask.other_mode_h)
            {
                return false;
            }
        }

        if self.mask.attribute & (1u64 << ATTR_GEOMETRY_MODE) != 0
            && (self.key.geometry_mode & self.mask.geometry_mode)
                != (other_key.geometry_mode & self.mask.geometry_mode)
        {
            return false;
        }

        true
    }
}

/// Literal port of the `keyIncludesHashesOf` lambda local to
/// `PresetDrawCall::matches` (`rt64_preset_draw_call.cpp` lines 142-158).
/// For every nonzero hash in `a.tmem_hashes`, that value must appear
/// somewhere in `b.tmem_hashes`'s *leading nonzero run* -- the inner scan
/// stops at `b`'s first zero entry, so a zero "hole" before the end of `b`
/// blocks matching against any nonzero hash placed after that hole. `a`'s
/// own hashes are scanned in full regardless of holes. Ported as a `while`
/// with the identical three-way loop condition from the source
/// (`(j < HashCount) && !found && (B.tmemHashes[j] != 0)`), preserving the
/// original's lack of an explicit `break`.
fn key_includes_hashes_of(a: &DrawCallKey, b: &DrawCallKey) -> bool {
    let hash_count = a.tmem_hashes.len();
    for i in 0..hash_count {
        if a.tmem_hashes[i] != 0 {
            let mut found = false;
            let mut j = 0usize;
            while j < hash_count && !found && b.tmem_hashes[j] != 0 {
                found = a.tmem_hashes[i] == b.tmem_hashes[j];
                j += 1;
            }

            if !found {
                return false;
            }
        }
    }

    true
}

/// Minimal local input for `from_raw_sample`, holding only the fields
/// `DrawCallKey::fromDrawCall` actually samples off a live `DrawCall` --
/// `colorCombiner`, `otherMode`, `geometryMode` -- plus the per-tile fields
/// it reads off each `DrawCallTile` it walks (`tmemHashOrID`,
/// `tileCopyUsed`), flattened into a `Vec` in tile-index order rather than
/// the source's `(tileIndex, tileCount)` slice into a shared `DrawData`
/// array. This is a local stand-in, not a port of `DrawCall`/`DrawData`/
/// `TextureCache` themselves -- see module doc.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RawDrawCallSample {
    pub color_combiner_l: u32,
    pub color_combiner_h: u32,
    pub other_mode_l: u32,
    pub other_mode_h: u32,
    pub geometry_mode: u32,
    pub tiles: Vec<RawDrawCallTileSample>,
}

/// One `DrawCallTile`'s worth of input to `from_raw_sample`: exactly the two
/// fields `fromDrawCall` reads (`tmemHashOrID`, `tileCopyUsed`).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct RawDrawCallTileSample {
    pub tmem_hash_or_id: u64,
    pub tile_copy_used: bool,
}

/// Literal port of `DrawCallKey::fromDrawCall` (`rt64_preset_draw_call.cpp`
/// lines 56-72), against `RawDrawCallSample` instead of the live
/// `DrawCall`/`DrawData`/`TextureCache` triple -- see module doc. Panics if
/// `sample.tiles.len()` exceeds 8: the source writes into a fixed
/// `tmemHashes[8]` with no bounds check (`key.tmemHashes[t] = tmemHash` for
/// `t` up to `call.tileCount`), which is undefined behavior on overflow in
/// C++; this port makes that case a loud, defined panic instead, which is a
/// deliberate deviation from (not a claim about) the original.
pub fn from_raw_sample(sample: &RawDrawCallSample) -> DrawCallKey {
    assert!(
        sample.tiles.len() <= 8,
        "DrawCallKey::fromDrawCall port: tile count exceeds the fixed 8-slot tmemHashes array \
         (undefined behavior in the original C++; treated as a loud panic here instead)"
    );

    let mut tmem_hashes = [0u64; 8];
    for (t, tile) in sample.tiles.iter().enumerate() {
        let tmem_hash = tile.tmem_hash_or_id;
        if !tile.tile_copy_used && tmem_hash > 0 {
            tmem_hashes[t] = tmem_hash;
        }
    }

    DrawCallKey {
        tmem_hashes,
        color_combiner_l: sample.color_combiner_l,
        color_combiner_h: sample.color_combiner_h,
        other_mode_l: sample.other_mode_l,
        other_mode_h: sample.other_mode_h,
        geometry_mode: sample.geometry_mode,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn zero_key() -> DrawCallKey {
        DrawCallKey {
            tmem_hashes: [0; 8],
            color_combiner_l: 0,
            color_combiner_h: 0,
            other_mode_l: 0,
            other_mode_h: 0,
            geometry_mode: 0,
        }
    }

    fn sample_key() -> DrawCallKey {
        DrawCallKey {
            tmem_hashes: [1, 2, 3, 4, 5, 6, 7, 8],
            color_combiner_l: 0x1111_1111,
            color_combiner_h: 0x2222_2222,
            other_mode_l: 0x3333_3333,
            other_mode_h: 0x4444_4444,
            geometry_mode: 0x5555_5555,
        }
    }

    fn mask_all() -> DrawCallMask {
        DrawCallMask::default_all()
    }

    fn mask_none() -> DrawCallMask {
        DrawCallMask {
            other_mode_l: 0,
            other_mode_h: 0,
            geometry_mode: 0,
            attribute: 0,
        }
    }

    // -----------------------------------------------------------------
    // DrawCallMask::default_all
    // -----------------------------------------------------------------

    #[test]
    fn default_all_sets_every_field_to_all_ones() {
        let m = DrawCallMask::default_all();
        assert_eq!(m.other_mode_l, 0xFFFF_FFFF);
        assert_eq!(m.other_mode_h, 0xFFFF_FFFF);
        assert_eq!(m.geometry_mode, 0xFFFF_FFFF);
        assert_eq!(m.attribute, 0xFFFF_FFFF_FFFF_FFFF);
    }

    // -----------------------------------------------------------------
    // DrawCallKey::eq -- the memcmp inversion
    // -----------------------------------------------------------------

    #[test]
    fn eq_returns_false_for_byte_identical_keys() {
        // memcmp == 0 for identical bytes; DrawCallKey::operator== returns
        // that 0 as a bool, i.e. false. Identical keys report NOT equal
        // under this operator's name.
        let a = sample_key();
        let b = sample_key();
        assert!(!a.eq(&b));
    }

    #[test]
    fn eq_returns_true_for_keys_differing_in_tmem_hashes() {
        let a = sample_key();
        let mut b = sample_key();
        b.tmem_hashes[0] = 999;
        assert!(a.eq(&b));
    }

    #[test]
    fn eq_returns_true_for_keys_differing_in_color_combiner_l() {
        let a = sample_key();
        let mut b = sample_key();
        b.color_combiner_l ^= 1;
        assert!(a.eq(&b));
    }

    #[test]
    fn eq_returns_true_for_keys_differing_in_color_combiner_h() {
        let a = sample_key();
        let mut b = sample_key();
        b.color_combiner_h ^= 1;
        assert!(a.eq(&b));
    }

    #[test]
    fn eq_returns_true_for_keys_differing_in_other_mode_l() {
        let a = sample_key();
        let mut b = sample_key();
        b.other_mode_l ^= 1;
        assert!(a.eq(&b));
    }

    #[test]
    fn eq_returns_true_for_keys_differing_in_other_mode_h() {
        let a = sample_key();
        let mut b = sample_key();
        b.other_mode_h ^= 1;
        assert!(a.eq(&b));
    }

    #[test]
    fn eq_returns_true_for_keys_differing_in_geometry_mode() {
        let a = sample_key();
        let mut b = sample_key();
        b.geometry_mode ^= 1;
        assert!(a.eq(&b));
    }

    #[test]
    fn eq_is_symmetric_under_the_inversion() {
        let a = sample_key();
        let mut b = sample_key();
        b.geometry_mode ^= 1;
        assert_eq!(a.eq(&b), b.eq(&a));
    }

    // -----------------------------------------------------------------
    // matches() -- exact match, all-set mask
    // -----------------------------------------------------------------

    #[test]
    fn matches_accepts_identical_keys_under_full_mask() {
        let preset = PresetDrawCall {
            key: sample_key(),
            mask: mask_all(),
        };
        assert!(preset.matches(&sample_key()));
    }

    // -----------------------------------------------------------------
    // matches() -- all-clear mask (every attribute ignored)
    // -----------------------------------------------------------------

    #[test]
    fn matches_accepts_any_key_under_empty_mask() {
        let preset = PresetDrawCall {
            key: sample_key(),
            mask: mask_none(),
        };
        // Every field differs, but no attribute bit is set, so every
        // comparison is skipped and matches() falls through to true.
        let mut other = sample_key();
        other.tmem_hashes = [9, 9, 9, 9, 9, 9, 9, 9];
        other.color_combiner_l = 0;
        other.color_combiner_h = 0;
        other.other_mode_l = 0;
        other.other_mode_h = 0;
        other.geometry_mode = 0;
        assert!(preset.matches(&other));
    }

    // -----------------------------------------------------------------
    // matches() -- Texture attribute (single-field mask)
    // -----------------------------------------------------------------

    fn mask_texture_only() -> DrawCallMask {
        DrawCallMask {
            other_mode_l: 0,
            other_mode_h: 0,
            geometry_mode: 0,
            attribute: 1u64 << ATTR_TEXTURE,
        }
    }

    #[test]
    fn matches_texture_only_mask_ignores_other_fields() {
        let preset = PresetDrawCall {
            key: sample_key(),
            mask: mask_texture_only(),
        };
        let mut other = sample_key();
        other.color_combiner_l = 0xDEAD_BEEF;
        other.other_mode_l = 0xDEAD_BEEF;
        other.geometry_mode = 0xDEAD_BEEF;
        // tmem_hashes are identical (same nonzero set both directions).
        assert!(preset.matches(&other));
    }

    #[test]
    fn matches_texture_rejects_when_self_has_a_hash_other_lacks() {
        let preset = PresetDrawCall {
            key: sample_key(),
            mask: mask_texture_only(),
        };
        let mut other = sample_key();
        other.tmem_hashes = [1, 2, 3, 4, 5, 6, 7, 0]; // drop the 8
        assert!(!preset.matches(&other));
    }

    #[test]
    fn matches_texture_rejects_when_other_has_a_hash_self_lacks() {
        let mut key = sample_key();
        key.tmem_hashes = [1, 2, 3, 4, 5, 6, 7, 0]; // drop the 8
        let preset = PresetDrawCall {
            key,
            mask: mask_texture_only(),
        };
        assert!(!preset.matches(&sample_key()));
    }

    #[test]
    fn matches_texture_is_order_independent_within_nonzero_set() {
        let preset = PresetDrawCall {
            key: sample_key(),
            mask: mask_texture_only(),
        };
        // Same nonzero set, different order: no zero holes, so this must
        // still match (the subset check doesn't require positional order).
        let other = DrawCallKey {
            tmem_hashes: [8, 7, 6, 5, 4, 3, 2, 1],
            ..sample_key()
        };
        assert!(preset.matches(&other));
    }

    #[test]
    fn matches_texture_zero_hole_blocks_matching_hashes_placed_after_it() {
        // b's scan stops at its first zero. Put a's distinguishing hash
        // value only reachable in b after a hole, and confirm rejection.
        let key = DrawCallKey {
            tmem_hashes: [1, 2, 0, 0, 0, 0, 0, 0],
            ..sample_key()
        };
        let preset = PresetDrawCall {
            key,
            mask: mask_texture_only(),
        };
        // other has 1 before the hole (found) but 2 only after a hole at
        // index 1, so scanning other for a's "2" stops at other's index-1
        // zero before ever reaching the trailing 2.
        let other = DrawCallKey {
            tmem_hashes: [1, 0, 2, 0, 0, 0, 0, 0],
            ..sample_key()
        };
        assert!(!preset.matches(&other));
    }

    #[test]
    fn matches_texture_all_zero_hashes_trivially_match() {
        let preset = PresetDrawCall {
            key: zero_key(),
            mask: mask_texture_only(),
        };
        assert!(preset.matches(&zero_key()));
    }

    // -----------------------------------------------------------------
    // matches() -- Combine attribute (single-field mask), the && quirk
    // -----------------------------------------------------------------

    fn mask_combine_only() -> DrawCallMask {
        DrawCallMask {
            other_mode_l: 0,
            other_mode_h: 0,
            geometry_mode: 0,
            attribute: 1u64 << ATTR_COMBINE,
        }
    }

    #[test]
    fn matches_combine_only_mask_ignores_other_fields() {
        let preset = PresetDrawCall {
            key: sample_key(),
            mask: mask_combine_only(),
        };
        let mut other = sample_key();
        other.tmem_hashes = [0; 8];
        other.other_mode_l = 0xDEAD_BEEF;
        other.geometry_mode = 0xDEAD_BEEF;
        assert!(preset.matches(&other));
    }

    #[test]
    fn matches_combine_accepts_when_only_l_differs() {
        // The && bug: a mismatch is only detected when BOTH L and H
        // differ, so an L-only difference does not reject.
        let preset = PresetDrawCall {
            key: sample_key(),
            mask: mask_combine_only(),
        };
        let mut other = sample_key();
        other.color_combiner_l ^= 0xFF;
        assert!(preset.matches(&other));
    }

    #[test]
    fn matches_combine_accepts_when_only_h_differs() {
        let preset = PresetDrawCall {
            key: sample_key(),
            mask: mask_combine_only(),
        };
        let mut other = sample_key();
        other.color_combiner_h ^= 0xFF;
        assert!(preset.matches(&other));
    }

    #[test]
    fn matches_combine_rejects_when_both_l_and_h_differ() {
        let preset = PresetDrawCall {
            key: sample_key(),
            mask: mask_combine_only(),
        };
        let mut other = sample_key();
        other.color_combiner_l ^= 0xFF;
        other.color_combiner_h ^= 0xFF;
        assert!(!preset.matches(&other));
    }

    #[test]
    fn matches_combine_accepts_identical_l_and_h() {
        let preset = PresetDrawCall {
            key: sample_key(),
            mask: mask_combine_only(),
        };
        assert!(preset.matches(&sample_key()));
    }

    // -----------------------------------------------------------------
    // matches() -- OtherMode attribute (single-field mask), bit masking
    // -----------------------------------------------------------------

    fn mask_other_mode_only(other_mode_l: u32, other_mode_h: u32) -> DrawCallMask {
        DrawCallMask {
            other_mode_l,
            other_mode_h,
            geometry_mode: 0,
            attribute: 1u64 << ATTR_OTHER_MODE,
        }
    }

    #[test]
    fn matches_other_mode_only_mask_ignores_other_fields() {
        let preset = PresetDrawCall {
            key: sample_key(),
            mask: mask_other_mode_only(0xFFFF_FFFF, 0xFFFF_FFFF),
        };
        let mut other = sample_key();
        other.tmem_hashes = [0; 8];
        other.color_combiner_l = 0xDEAD_BEEF;
        other.geometry_mode = 0xDEAD_BEEF;
        assert!(preset.matches(&other));
    }

    #[test]
    fn matches_other_mode_full_mask_rejects_l_mismatch() {
        let preset = PresetDrawCall {
            key: sample_key(),
            mask: mask_other_mode_only(0xFFFF_FFFF, 0xFFFF_FFFF),
        };
        let mut other = sample_key();
        other.other_mode_l ^= 1;
        assert!(!preset.matches(&other));
    }

    #[test]
    fn matches_other_mode_full_mask_rejects_h_mismatch() {
        let preset = PresetDrawCall {
            key: sample_key(),
            mask: mask_other_mode_only(0xFFFF_FFFF, 0xFFFF_FFFF),
        };
        let mut other = sample_key();
        other.other_mode_h ^= 1;
        assert!(!preset.matches(&other));
    }

    #[test]
    fn matches_other_mode_bit_masked_out_of_l_is_ignored() {
        // mask.otherModeL clears bit 0: differing only in bit 0 must not
        // cause rejection, because that bit is masked out of both operands
        // before comparison.
        let preset = PresetDrawCall {
            key: sample_key(),
            mask: mask_other_mode_only(0xFFFF_FFFE, 0xFFFF_FFFF),
        };
        let mut other = sample_key();
        other.other_mode_l ^= 1;
        assert!(preset.matches(&other));
    }

    #[test]
    fn matches_other_mode_bit_masked_out_of_h_is_ignored() {
        let preset = PresetDrawCall {
            key: sample_key(),
            mask: mask_other_mode_only(0xFFFF_FFFF, 0xFFFF_FFFE),
        };
        let mut other = sample_key();
        other.other_mode_h ^= 1;
        assert!(preset.matches(&other));
    }

    #[test]
    fn matches_other_mode_bit_still_in_mask_still_rejects() {
        // Confirms the masked-out test above isn't vacuous: with the same
        // mask, a mismatch in a bit that's still set in the mask rejects.
        let preset = PresetDrawCall {
            key: sample_key(),
            mask: mask_other_mode_only(0xFFFF_FFFE, 0xFFFF_FFFF),
        };
        let mut other = sample_key();
        other.other_mode_l ^= 2; // bit 1, still set in the mask
        assert!(!preset.matches(&other));
    }

    #[test]
    fn matches_other_mode_all_clear_field_mask_ignores_any_l_h_difference() {
        let preset = PresetDrawCall {
            key: sample_key(),
            mask: mask_other_mode_only(0, 0),
        };
        let mut other = sample_key();
        other.other_mode_l = 0xFFFF_FFFF;
        other.other_mode_h = 0xFFFF_FFFF;
        assert!(preset.matches(&other));
    }

    // -----------------------------------------------------------------
    // matches() -- GeometryMode attribute (single-field mask), bit masking
    // -----------------------------------------------------------------

    fn mask_geometry_mode_only(geometry_mode: u32) -> DrawCallMask {
        DrawCallMask {
            other_mode_l: 0,
            other_mode_h: 0,
            geometry_mode,
            attribute: 1u64 << ATTR_GEOMETRY_MODE,
        }
    }

    #[test]
    fn matches_geometry_mode_only_mask_ignores_other_fields() {
        let preset = PresetDrawCall {
            key: sample_key(),
            mask: mask_geometry_mode_only(0xFFFF_FFFF),
        };
        let mut other = sample_key();
        other.tmem_hashes = [0; 8];
        other.color_combiner_l = 0xDEAD_BEEF;
        other.other_mode_l = 0xDEAD_BEEF;
        assert!(preset.matches(&other));
    }

    #[test]
    fn matches_geometry_mode_full_mask_rejects_mismatch() {
        let preset = PresetDrawCall {
            key: sample_key(),
            mask: mask_geometry_mode_only(0xFFFF_FFFF),
        };
        let mut other = sample_key();
        other.geometry_mode ^= 1;
        assert!(!preset.matches(&other));
    }

    #[test]
    fn matches_geometry_mode_masked_bit_is_ignored() {
        let preset = PresetDrawCall {
            key: sample_key(),
            mask: mask_geometry_mode_only(0xFFFF_FFFE),
        };
        let mut other = sample_key();
        other.geometry_mode ^= 1;
        assert!(preset.matches(&other));
    }

    #[test]
    fn matches_geometry_mode_unmasked_bit_still_rejects() {
        let preset = PresetDrawCall {
            key: sample_key(),
            mask: mask_geometry_mode_only(0xFFFF_FFFE),
        };
        let mut other = sample_key();
        other.geometry_mode ^= 2;
        assert!(!preset.matches(&other));
    }

    #[test]
    fn matches_geometry_mode_all_clear_field_mask_ignores_any_difference() {
        let preset = PresetDrawCall {
            key: sample_key(),
            mask: mask_geometry_mode_only(0),
        };
        let mut other = sample_key();
        other.geometry_mode = 0xFFFF_FFFF;
        assert!(preset.matches(&other));
    }

    // -----------------------------------------------------------------
    // matches() -- short-circuit order
    // -----------------------------------------------------------------

    #[test]
    fn matches_short_circuits_on_texture_before_combine() {
        // Both Texture and Combine bits set; Texture is checked first and
        // fails, so matches() must return false without needing Combine to
        // also fail (it doesn't, here -- combine fields are identical).
        let mask = DrawCallMask {
            other_mode_l: 0,
            other_mode_h: 0,
            geometry_mode: 0,
            attribute: (1u64 << ATTR_TEXTURE) | (1u64 << ATTR_COMBINE),
        };
        let preset = PresetDrawCall {
            key: sample_key(),
            mask,
        };
        let mut other = sample_key();
        other.tmem_hashes = [0; 8]; // texture mismatch
                                    // combine fields left identical
        assert!(!preset.matches(&other));
    }

    #[test]
    fn matches_short_circuits_on_combine_before_other_mode() {
        let mask = DrawCallMask {
            other_mode_l: 0xFFFF_FFFF,
            other_mode_h: 0xFFFF_FFFF,
            geometry_mode: 0,
            attribute: (1u64 << ATTR_COMBINE) | (1u64 << ATTR_OTHER_MODE),
        };
        let preset = PresetDrawCall {
            key: sample_key(),
            mask,
        };
        let mut other = sample_key();
        other.color_combiner_l ^= 0xFF;
        other.color_combiner_h ^= 0xFF; // both differ: combine rejects
                                        // other_mode fields left identical
        assert!(!preset.matches(&other));
    }

    #[test]
    fn matches_short_circuits_on_other_mode_l_before_h() {
        // OtherMode-L fails; OtherMode-H is never reached (if it were, this
        // would still reject since both fail here, so this test only
        // documents order, not distinguishes behavior -- see the next test
        // for a case that would behave differently under the wrong order).
        let mask = mask_other_mode_only(0xFFFF_FFFF, 0xFFFF_FFFF);
        let preset = PresetDrawCall {
            key: sample_key(),
            mask,
        };
        let mut other = sample_key();
        other.other_mode_l ^= 1;
        assert!(!preset.matches(&other));
    }

    #[test]
    fn matches_falls_through_all_attributes_to_true_when_none_fail() {
        let preset = PresetDrawCall {
            key: sample_key(),
            mask: mask_all(),
        };
        assert!(preset.matches(&sample_key()));
    }

    #[test]
    fn matches_geometry_mode_is_the_last_check_and_still_rejects_alone() {
        // Every other attribute matches; only GeometryMode differs. This
        // exercises the final branch after falling through Texture,
        // Combine, and OtherMode without early return.
        let mask = mask_all();
        let preset = PresetDrawCall {
            key: sample_key(),
            mask,
        };
        let mut other = sample_key();
        other.geometry_mode ^= 1;
        assert!(!preset.matches(&other));
    }

    // -----------------------------------------------------------------
    // from_raw_sample (DrawCallKey::fromDrawCall)
    // -----------------------------------------------------------------

    fn tile(tmem_hash_or_id: u64, tile_copy_used: bool) -> RawDrawCallTileSample {
        RawDrawCallTileSample {
            tmem_hash_or_id,
            tile_copy_used,
        }
    }

    #[test]
    fn from_raw_sample_copies_scalar_fields_directly() {
        let sample = RawDrawCallSample {
            color_combiner_l: 0x1234,
            color_combiner_h: 0x5678,
            other_mode_l: 0x9ABC,
            other_mode_h: 0xDEF0,
            geometry_mode: 0x1111,
            tiles: vec![],
        };
        let key = from_raw_sample(&sample);
        assert_eq!(key.color_combiner_l, 0x1234);
        assert_eq!(key.color_combiner_h, 0x5678);
        assert_eq!(key.other_mode_l, 0x9ABC);
        assert_eq!(key.other_mode_h, 0xDEF0);
        assert_eq!(key.geometry_mode, 0x1111);
    }

    #[test]
    fn from_raw_sample_with_no_tiles_leaves_all_hashes_zero() {
        let sample = RawDrawCallSample {
            color_combiner_l: 0,
            color_combiner_h: 0,
            other_mode_l: 0,
            other_mode_h: 0,
            geometry_mode: 0,
            tiles: vec![],
        };
        let key = from_raw_sample(&sample);
        assert_eq!(key.tmem_hashes, [0u64; 8]);
    }

    #[test]
    fn from_raw_sample_records_nonzero_uncopied_tile_hashes_by_index() {
        let sample = RawDrawCallSample {
            color_combiner_l: 0,
            color_combiner_h: 0,
            other_mode_l: 0,
            other_mode_h: 0,
            geometry_mode: 0,
            tiles: vec![tile(111, false), tile(222, false)],
        };
        let key = from_raw_sample(&sample);
        assert_eq!(key.tmem_hashes, [111, 222, 0, 0, 0, 0, 0, 0]);
    }

    #[test]
    fn from_raw_sample_skips_tile_copy_used_hashes() {
        // tileCopyUsed suppresses recording the hash even though it's
        // nonzero: the slot stays at its zeroed default.
        let sample = RawDrawCallSample {
            color_combiner_l: 0,
            color_combiner_h: 0,
            other_mode_l: 0,
            other_mode_h: 0,
            geometry_mode: 0,
            tiles: vec![tile(111, true), tile(222, false)],
        };
        let key = from_raw_sample(&sample);
        assert_eq!(key.tmem_hashes, [0, 222, 0, 0, 0, 0, 0, 0]);
    }

    #[test]
    fn from_raw_sample_skips_zero_hash_tiles() {
        // tmemHash > 0 is required even when tileCopyUsed is false.
        let sample = RawDrawCallSample {
            color_combiner_l: 0,
            color_combiner_h: 0,
            other_mode_l: 0,
            other_mode_h: 0,
            geometry_mode: 0,
            tiles: vec![tile(0, false), tile(222, false)],
        };
        let key = from_raw_sample(&sample);
        assert_eq!(key.tmem_hashes, [0, 222, 0, 0, 0, 0, 0, 0]);
    }

    #[test]
    fn from_raw_sample_fills_all_eight_slots_in_index_order() {
        let sample = RawDrawCallSample {
            color_combiner_l: 0,
            color_combiner_h: 0,
            other_mode_l: 0,
            other_mode_h: 0,
            geometry_mode: 0,
            tiles: (1..=8u64).map(|h| tile(h, false)).collect(),
        };
        let key = from_raw_sample(&sample);
        assert_eq!(key.tmem_hashes, [1, 2, 3, 4, 5, 6, 7, 8]);
    }

    #[test]
    #[should_panic(expected = "tile count exceeds the fixed 8-slot")]
    fn from_raw_sample_panics_on_more_than_eight_tiles() {
        let sample = RawDrawCallSample {
            color_combiner_l: 0,
            color_combiner_h: 0,
            other_mode_l: 0,
            other_mode_h: 0,
            geometry_mode: 0,
            tiles: (1..=9u64).map(|h| tile(h, false)).collect(),
        };
        let _ = from_raw_sample(&sample);
    }

    #[test]
    fn from_raw_sample_key_round_trips_through_matches_under_full_mask() {
        let sample = RawDrawCallSample {
            color_combiner_l: 0x1111_1111,
            color_combiner_h: 0x2222_2222,
            other_mode_l: 0x3333_3333,
            other_mode_h: 0x4444_4444,
            geometry_mode: 0x5555_5555,
            tiles: vec![tile(1, false), tile(2, false)],
        };
        let key = from_raw_sample(&sample);
        let preset = PresetDrawCall {
            key,
            mask: mask_all(),
        };
        assert!(preset.matches(&from_raw_sample(&sample)));
    }
}
