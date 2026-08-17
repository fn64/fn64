//! Literal port of four RDP display-list opcodes' command-word bitfield
//! *decoding* from RT64's `src/gbi/rt64_gbi_rdp.cpp` -- `setScissor`,
//! `setConvert`, `setKeyR`, `setKeyGB` -- a literal port of the permitted
//! MIT RT64 Rust-port source pinned at commit
//! `5473732a822a4423b5696e7cb18fecc425a59875` (`docs/RT64-PORT-AUTHORITY.md`),
//! `src/gbi/rt64_gbi_rdp.cpp` (SHA-256 of the whole file,
//! `9eab7d0b8ba70f816c4cd873a535a50d01b4a0285d2726edf7256809299bae43`) and
//! `src/gbi/rt64_gbi_rdp.h` (SHA-256 of the whole file,
//! `96b9be0e2c37e3a66a9737ec97dc0bab24c6cc5146e93796517490e0c95642c3`):
//!
//! ```text
//! // src/gbi/rt64_display_list.h / src/gbi/rt64_gbi.cpp:32-38 (extractors,
//! // shared with every other GBI decode file in this crate)
//! uint32_t DisplayList::p0(uint8_t pos, uint8_t bits) const {
//!     return ((w0 >> pos) & ((0x01 << bits) - 1));
//! }
//!
//! uint32_t DisplayList::p1(uint8_t pos, uint8_t bits) const {
//!     return ((w1 >> pos) & ((0x01 << bits) - 1));
//! }
//!
//! // src/gbi/rt64_gbi_rdp.cpp:135-169 (this port's source, in full -- every
//! // bitfield-relevant line of the four ported opcode functions; see
//! // "Nonclaims" for the state->rdp dispatch lines this port omits)
//! void setScissor(State *state, DisplayList **dl) {
//!     const uint8_t mode = (*dl)->p1(24, 2);
//!     const int32_t ulx = (*dl)->p0(12, 12);
//!     const int32_t uly = (*dl)->p0(0, 12);
//!     const int32_t lrx = (*dl)->p1(12, 12);
//!     const int32_t lry = (*dl)->p1(0, 12);
//!     state->rdp->setScissor(mode, ulx, uly, lrx, lry);
//! }
//!
//! void setConvert(State *state, DisplayList **dl) {
//!     const int32_t k0 = (*dl)->p0(13, 9);
//!     const int32_t k1 = (*dl)->p0(4, 9);
//!     const int32_t k2 = ((*dl)->p0(0, 4) << 5) | (*dl)->p1(27, 5);
//!     const int32_t k3 = (*dl)->p1(18, 9);
//!     const int32_t k4 = (*dl)->p1(9, 9);
//!     const int32_t k5 = (*dl)->p1(0, 9);
//!     state->rdp->setConvert(k0, k1, k2, k3, k4, k5);
//! }
//!
//! void setKeyR(State *state, DisplayList **dl) {
//!     const uint32_t cR = (*dl)->p1(8, 8);
//!     const uint32_t sR = (*dl)->p1(0, 8);
//!     const uint32_t wR = (*dl)->p1(16, 12);
//!     state->rdp->setKeyR(cR, sR, wR);
//! }
//!
//! void setKeyGB(State *state, DisplayList **dl) {
//!     const uint32_t cG = (*dl)->p1(24, 8);
//!     const uint32_t sG = (*dl)->p1(16, 8);
//!     const uint32_t wG = (*dl)->p0(12, 12);
//!     const uint32_t cB = (*dl)->p1(8, 8);
//!     const uint32_t sB = (*dl)->p1(0, 8);
//!     const uint32_t wB = (*dl)->p0(0, 12);
//!     state->rdp->setKeyGB(cG, sG, wG, cB, sB, wB);
//! }
//! ```
//!
//! **Reuse, not new type.** `crates/fn64-render-reference/src/gbi/` has a
//! full GBI implementation, and its tests exercise `setConvert`/`setKeyR`/
//! `setKeyGB`-shaped RDP state (confirmed by grep -- those hits are the only
//! matches for these opcode names anywhere in the workspace before this
//! file), but that crate's tests operate on its own already-decoded RDP
//! state, not on a `(w0, w1)` command-word decode -- there is no existing
//! type in `fn64-render-wgpu` whose bitfield layout this port could reuse.
//! `setScissor` has no decode anywhere in `crates/fn64-render-wgpu/src/`
//! either; the only prior `set_scissor`-shaped code in this crate is
//! `rt64_extended_gbi.rs`'s `pack_set_scissor`/`pack_set_scissor_align`,
//! which are *packing* (host-side command-word construction) functions for
//! the unrelated extended-GBI opcodes `G_EX_SETSCISSOR_V1`/
//! `G_EX_SETSCISSORALIGN_V1` (`include/rt64_extended_gbi.h`), not a decode
//! of the standard-GBI `G_SETSCISSOR` opcode this file's `setScissor`
//! implements -- different opcode family, different direction (pack vs.
//! decode), not reusable. This module defines its own small typed-struct-
//! per-opcode decode, matching `rt64_gbi_f3d.rs`'s established precedent of
//! a fresh, unwired, characterization-first module per source file rather
//! than wiring into the existing GBI interpreter.
//!
//! ## Admitted domain
//!
//! - **`p0`/`p1` are pure bit-twiddles with no sign extension of their
//!   own**, identical to every other GBI decode file in this crate:
//!   `((w >> pos) & ((1 << bits) - 1))` always yields an unsigned value
//!   truncated to `bits` width. Ported as `fn p0(w0: u32, pos: u8, bits: u8)
//!   -> u32 { (w0 >> pos) & ((1u32 << bits) - 1) }` (and `p1` identically
//!   over `w1`) -- literal, not idiomatized.
//! - **`w0` vs `w1`**: `setScissor` reads `mode`/`lrx`/`lry` from `w1` and
//!   `ulx`/`uly` from `w0` (via `p0`) -- note the asymmetric split, *not*
//!   "all coordinates from one word." `setConvert` reads `k0`/`k1`/the low
//!   nibble of `k2` from `w0` and the rest from `w1`. `setKeyR` reads all
//!   three fields from `w1` alone (`w0` is unused -- the whole first command
//!   word carries only the opcode byte for this command). `setKeyGB` reads
//!   `wG`/`wB` from `w0` and `cG`/`sG`/`cB`/`sB` from `w1`. This port keeps
//!   each split explicit and never swaps which word backs which field.
//! - **None of these four functions contains an explicit sign-extension
//!   cast** (no `(int16_t)`/`(int8_t)`-style narrowing-then-reinterpret,
//!   unlike `rt64_gbi_f3d.rs`'s `moveWord` `G_MW_CLIP`/`G_MW_FOG` arms).
//!   `setScissor`'s `ulx`/`uly`/`lrx`/`lry` and `setConvert`'s `k0..k5` are
//!   declared `int32_t` but initialized directly from `p0`/`p1`'s
//!   `uint32_t` return value -- an implicit `uint32_t -> int32_t`
//!   conversion. Per the C++ standard this conversion is *value-preserving*
//!   (not a reinterpret) whenever the source value fits in the destination
//!   signed type's positive range, which it always does here: every field
//!   these six locals read is at most 12 bits (`setScissor`, range
//!   0..=4095) or 9 bits (`setConvert`'s `k0`/`k1`/`k3`/`k4`/`k5`, range
//!   0..=511; `k2`'s composite `(4-bit << 5) | 5-bit`, also range 0..=511),
//!   far short of `int32_t`'s 31-bit positive range. **So despite the
//!   `int32_t` type label, none of these ten locals can ever hold a
//!   negative value from this decode alone** -- ported as plain `i32` casts
//!   of the `u32` extractor result (`p0(w0, 12, 12) as i32`, etc.), which in
//!   Rust is also value-preserving for values in this range (an `as i32`
//!   cast on a `u32` in 0..=0x7FFF_FFFF never changes the numeric value).
//!   Every characterization test below that probes "one bit above max"
//!   still lands within the field's own width (12 or 9 bits), so it can
//!   never overflow into the sign bit either -- there is no negative-value
//!   or sign-boundary case to test for these four functions, because the
//!   source performs no sign extension. This is a **deliberate scope note**
//!   answering the task brief's request to "work out the exact sign
//!   handling" for `setConvert`'s K coefficients: the RDP hardware /
//!   `state->rdp->setConvert` (out of scope, not ported) may well
//!   *reinterpret* these 9-bit fields as signed two's-complement YUV
//!   coefficients internally (matching real N64 YUV-conversion semantics,
//!   where K0-K5 are documented as signed 9-bit values in range
//!   -256..=255) -- but that reinterpretation, if it happens at all, occurs
//!   inside `RDP::setConvert`'s body, a function this task's DECODE-only
//!   scope explicitly excludes (see Nonclaims). This decode layer's
//!   observable behavior is exhaustively: zero-extend the field, widen to
//!   `i32`, always non-negative. [`decode_set_convert`]'s tests pin exactly
//!   this: the max-value case (all 9 bits set) yields `511i32`, not
//!   `-1i32`.
//! - **`setConvert`'s `k2` bit composition**: `((*dl)->p0(0, 4) << 5) |
//!   (*dl)->p1(27, 5)` combines a 4-bit field from `w0` (shifted left 5)
//!   with a 5-bit field from `w1` (no shift) via bitwise OR, forming one
//!   9-bit value spanning both command words. `p0(0, 4)` is a `uint32_t` in
//!   0..=15; `<< 5` gives a `uint32_t` in 0..=480 (steps of 32) -- C++
//!   promotes both shift operands to `int`/`unsigned int` per the usual
//!   arithmetic conversions, but 15 << 5 = 480 never approaches `int`
//!   overflow, so this is an ordinary unsigned left shift. `p1(27, 5)` is a
//!   `uint32_t` in 0..=31. The `|` of a multiple-of-32 value (0..=480) and a
//!   sub-32 value (0..=31) never overlaps a bit, so the OR is equivalent to
//!   addition here, giving the full 0..=511 range with no bit collision.
//!   Ported as `((p0(w0, 0, 4) << 5) | p1(w1, 27, 5)) as i32` -- literal
//!   shift-then-OR order preserved, `u32` arithmetic throughout before the
//!   final widen to `i32`.
//! - **`setScissor`'s `mode` field** (`p1(24, 2)`, 2 bits, range 0..=3) is
//!   the only field in this file kept as its C++ declared type `uint8_t`
//!   with no sign involved -- ported as `p1(w1, 24, 2) as u8`.
//! - **`setKeyR`/`setKeyGB`'s six fields are all `uint32_t`, no signed type
//!   anywhere** in either function: `setKeyR`'s `cR`/`sR` (8-bit scale/
//!   center fields, 0..=255) and `wR` (12-bit width field, 0..=4095);
//!   `setKeyGB`'s `cG`/`sG`/`cB`/`sB` (8-bit, 0..=255) and `wG`/`wB` (12-bit,
//!   0..=4095, read via `p0` unlike every other field in these two
//!   functions which reads via `p1`). Ported as plain `u32` casts of the
//!   extractor result, matching the source's declared type exactly (not
//!   narrowed to `u8`/`u16`, since the source itself declares `uint32_t`
//!   locals despite every field fitting in fewer bits).
//! - **C++ integer promotion**: as in every other GBI decode file in this
//!   crate, `p0`/`p1`'s `pos`/`bits` arguments are `uint8_t` literals
//!   promoted to `int` for the shift, then the `uint32_t` return implicitly
//!   narrows any `int`-typed subexpression back to unsigned -- this port's
//!   `u8` parameters and `u32` shift/mask arithmetic reproduce the same
//!   effective widths without needing Rust's stricter integer rules to
//!   diverge from C++'s observable behavior anywhere in this file.
//! - **Truncation**: every extracted field here (2, 4, 5, 8, 9, or 12 bits)
//!   is strictly narrower than `u32`, so `p0`/`p1`'s own mask-to-width
//!   truncates any wider garbage in the command word before this port ever
//!   sees it -- the "one bit above max" characterization tests below prove
//!   this masking is preserved bit-for-bit (setting the bit immediately
//!   above a field's top bit must not perturb the decoded value).
//!
//! ## Nonclaims
//!
//! **This is a PARTIAL port of `src/gbi/rt64_gbi_rdp.cpp`.** The whole-file
//! digest cited above will mark the file `ported` in the port-inventory
//! scanner, but the large majority of that file's ~40 opcode functions
//! (`setColorImage`, `setDepthImage`, `setTextureImage`, `setCombine`,
//! `setTile`, `setTileSize`, `loadTile`, `loadBlock`, `loadTLUT`,
//! `setEnvColor`, `setPrimColor`, `setBlendColor`, `setFogColor`,
//! `setFillColor`, `setOtherMode`, `setPrimDepth`, `texrect`/`texrectFlip`,
//! `fillRect`, `syncFull`/`syncLoad`/`syncPipe`/`syncTile`, `triangle`
//! variants, and `setup`) were landed by earlier, separate work --
//! `decodeTriangles` in `crates/fn64-render-wgpu/src/raw_dpc/triangle.rs` +
//! `triangle_vertices.rs`, `texrectLLE` in
//! `crates/fn64-render-wgpu/src/raw_dpc/texture_rectangle.rs`, and other
//! functions this task's ticket did not re-audit -- and some of this file's
//! surface remains unported after this change (in particular every
//! `state->rdp->*` dispatch call this module's Nonclaims section excludes,
//! and any opcode function not named in this doc header's "opcodes ported"
//! list below). This module's contribution is exactly, and only, the four
//! named functions' bitfield decode.
//!
//! No GPU, WGSL, or production wiring (this module is not called from
//! anywhere yet; dead-code warnings on the unused public surface are
//! expected and correct, matching every other GBI decode file in this
//! crate), and no RT64 visual/pixel/silicon parity or performance claim.
//! Not wired to `fn64-render-reference`'s GBI interpreter (see "Reuse, not
//! new type").
//!
//! **Dispatch deliberately not ported** (every `state->rdp->*` call, per
//! the task's DECODE-only scope): `setScissor`'s
//! `state->rdp->setScissor(mode, ulx, uly, lrx, lry)`; `setConvert`'s
//! `state->rdp->setConvert(k0, k1, k2, k3, k4, k5)` (including whatever
//! signed-YUV reinterpretation, clamping, or hardware-specific behavior
//! that function performs internally -- see "Admitted domain" above);
//! `setKeyR`'s `state->rdp->setKeyR(cR, sR, wR)`; `setKeyGB`'s
//! `state->rdp->setKeyGB(cG, sG, wG, cB, sB, wB)`.
//!
//! **Opcodes ported** (bitfield decode only, as pure `(w0, w1) -> struct`
//! functions): `setScissor` (-> [`decode_set_scissor`], [`SetScissorDecoded`]),
//! `setConvert` (-> [`decode_set_convert`], [`SetConvertDecoded`]),
//! `setKeyR` (-> [`decode_set_key_r`], [`SetKeyRDecoded`]), `setKeyGB` (->
//! [`decode_set_key_gb`], [`SetKeyGbDecoded`]).
//!
//! **Every other function in `rt64_gbi_rdp.cpp`/`.h` is out of this
//! module's scope** -- neither newly ported here nor re-audited; see the
//! partial-port disclosure above for what earlier work already landed.

/// `DisplayList::p0(pos, bits)`: `(w0 >> pos) & ((1 << bits) - 1)`.
fn p0(w0: u32, pos: u8, bits: u8) -> u32 {
    (w0 >> pos) & ((1u32 << bits) - 1)
}

/// `DisplayList::p1(pos, bits)`: `(w1 >> pos) & ((1 << bits) - 1)`.
fn p1(w1: u32, pos: u8, bits: u8) -> u32 {
    (w1 >> pos) & ((1u32 << bits) - 1)
}

/// `setScissor`'s decoded operands: `mode = p1(24, 2)`, `ulx = p0(12, 12)`,
/// `uly = p0(0, 12)`, `lrx = p1(12, 12)`, `lry = p1(0, 12)`. The source
/// declares `ulx`/`uly`/`lrx`/`lry` as `int32_t`, but see module doc
/// "Admitted domain" -- these twelve-bit fields can never be negative from
/// this decode alone.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SetScissorDecoded {
    pub mode: u8,
    pub ulx: i32,
    pub uly: i32,
    pub lrx: i32,
    pub lry: i32,
}

pub fn decode_set_scissor(w0: u32, w1: u32) -> SetScissorDecoded {
    SetScissorDecoded {
        mode: p1(w1, 24, 2) as u8,
        ulx: p0(w0, 12, 12) as i32,
        uly: p0(w0, 0, 12) as i32,
        lrx: p1(w1, 12, 12) as i32,
        lry: p1(w1, 0, 12) as i32,
    }
}

/// `setConvert`'s decoded YUV coefficient operands: `k0 = p0(13, 9)`, `k1 =
/// p0(4, 9)`, `k2 = (p0(0, 4) << 5) | p1(27, 5)`, `k3 = p1(18, 9)`, `k4 =
/// p1(9, 9)`, `k5 = p1(0, 9)`. The source declares all six as `int32_t`,
/// but see module doc "Admitted domain" -- none of these nine-bit fields
/// can ever be negative from this decode alone (the RDP's signed-YUV
/// reinterpretation, if any, happens inside the out-of-scope
/// `state->rdp->setConvert` dispatch call, not here).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SetConvertDecoded {
    pub k0: i32,
    pub k1: i32,
    pub k2: i32,
    pub k3: i32,
    pub k4: i32,
    pub k5: i32,
}

pub fn decode_set_convert(w0: u32, w1: u32) -> SetConvertDecoded {
    SetConvertDecoded {
        k0: p0(w0, 13, 9) as i32,
        k1: p0(w0, 4, 9) as i32,
        k2: ((p0(w0, 0, 4) << 5) | p1(w1, 27, 5)) as i32,
        k3: p1(w1, 18, 9) as i32,
        k4: p1(w1, 9, 9) as i32,
        k5: p1(w1, 0, 9) as i32,
    }
}

/// `setKeyR`'s decoded operands: `cR = p1(8, 8)`, `sR = p1(0, 8)`, `wR =
/// p1(16, 12)`. All three are `uint32_t` in the source; `w0` is unused (the
/// command's first word carries only the opcode byte for this command).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SetKeyRDecoded {
    pub c_r: u32,
    pub s_r: u32,
    pub w_r: u32,
}

pub fn decode_set_key_r(_w0: u32, w1: u32) -> SetKeyRDecoded {
    SetKeyRDecoded {
        c_r: p1(w1, 8, 8),
        s_r: p1(w1, 0, 8),
        w_r: p1(w1, 16, 12),
    }
}

/// `setKeyGB`'s decoded operands: `cG = p1(24, 8)`, `sG = p1(16, 8)`, `wG =
/// p0(12, 12)`, `cB = p1(8, 8)`, `sB = p1(0, 8)`, `wB = p0(0, 12)`. All six
/// are `uint32_t` in the source. Note `wG`/`wB` read from `w0` while
/// `cG`/`sG`/`cB`/`sB` read from `w1` -- the only field pair in this file
/// split across both words for a single-word-per-field opcode.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SetKeyGbDecoded {
    pub c_g: u32,
    pub s_g: u32,
    pub w_g: u32,
    pub c_b: u32,
    pub s_b: u32,
    pub w_b: u32,
}

pub fn decode_set_key_gb(w0: u32, w1: u32) -> SetKeyGbDecoded {
    SetKeyGbDecoded {
        c_g: p1(w1, 24, 8),
        s_g: p1(w1, 16, 8),
        w_g: p0(w0, 12, 12),
        c_b: p1(w1, 8, 8),
        s_b: p1(w1, 0, 8),
        w_b: p0(w0, 0, 12),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- p0 / p1 raw extractor behavior ---

    #[test]
    fn p0_all_zero() {
        assert_eq!(p0(0, 0, 12), 0);
    }

    #[test]
    fn p0_extracts_masked_field() {
        assert_eq!(p0(0xFFFF_FFFF, 12, 12), 0xFFF);
    }

    #[test]
    fn p1_all_zero() {
        assert_eq!(p1(0, 0, 12), 0);
    }

    #[test]
    fn p1_extracts_masked_field() {
        assert_eq!(p1(0xFFFF_FFFF, 12, 12), 0xFFF);
    }

    // --- decode_set_scissor ---

    #[test]
    fn decode_set_scissor_all_zero() {
        let d = decode_set_scissor(0, 0);
        assert_eq!(
            d,
            SetScissorDecoded {
                mode: 0,
                ulx: 0,
                uly: 0,
                lrx: 0,
                lry: 0,
            }
        );
    }

    #[test]
    fn decode_set_scissor_all_ones() {
        // w0 = w1 = 0xFFFF_FFFF: every field reads its all-ones value.
        let d = decode_set_scissor(0xFFFF_FFFF, 0xFFFF_FFFF);
        assert_eq!(
            d,
            SetScissorDecoded {
                mode: 0x3,
                ulx: 0xFFF,
                uly: 0xFFF,
                lrx: 0xFFF,
                lry: 0xFFF,
            }
        );
    }

    #[test]
    fn decode_set_scissor_mode_at_max_two_bits() {
        // mode = p1(24, 2): bits 24-25 of w1.
        let d = decode_set_scissor(0, 0x3 << 24);
        assert_eq!(d.mode, 0x3);
    }

    #[test]
    fn decode_set_scissor_mode_one_bit_above_max_is_masked() {
        // bit 26 of w1 is outside mode's 2-bit field -- must not leak in.
        let d = decode_set_scissor(0, 0x1 << 26);
        assert_eq!(d.mode, 0);
    }

    #[test]
    fn decode_set_scissor_ulx_at_max_twelve_bits() {
        // ulx = p0(12, 12): bits 12-23 of w0.
        let d = decode_set_scissor(0xFFF << 12, 0);
        assert_eq!(d.ulx, 0xFFF);
        assert_eq!(d.uly, 0);
    }

    #[test]
    fn decode_set_scissor_ulx_one_bit_above_max_is_masked() {
        // bit 24 of w0 is outside ulx's field -- must not leak in.
        let d = decode_set_scissor(0x1 << 24, 0);
        assert_eq!(d.ulx, 0);
    }

    #[test]
    fn decode_set_scissor_uly_at_max_twelve_bits() {
        // uly = p0(0, 12): bits 0-11 of w0.
        let d = decode_set_scissor(0xFFF, 0);
        assert_eq!(d.uly, 0xFFF);
        assert_eq!(d.ulx, 0);
    }

    #[test]
    fn decode_set_scissor_uly_one_bit_above_max_is_masked() {
        // bit 12 of w0 belongs to ulx, not uly -- must not leak into uly.
        let d = decode_set_scissor(0x1 << 12, 0);
        assert_eq!(d.uly, 0);
        assert_eq!(d.ulx, 1);
    }

    #[test]
    fn decode_set_scissor_lrx_at_max_twelve_bits() {
        // lrx = p1(12, 12): bits 12-23 of w1.
        let d = decode_set_scissor(0, 0xFFF << 12);
        assert_eq!(d.lrx, 0xFFF);
        assert_eq!(d.lry, 0);
    }

    #[test]
    fn decode_set_scissor_lrx_one_bit_above_max_is_masked() {
        let d = decode_set_scissor(0, 0x1 << 24);
        assert_eq!(d.lrx, 0);
    }

    #[test]
    fn decode_set_scissor_lry_at_max_twelve_bits() {
        // lry = p1(0, 12): bits 0-11 of w1.
        let d = decode_set_scissor(0, 0xFFF);
        assert_eq!(d.lry, 0xFFF);
        assert_eq!(d.lrx, 0);
    }

    #[test]
    fn decode_set_scissor_lry_one_bit_above_max_is_masked() {
        let d = decode_set_scissor(0, 0x1 << 12);
        assert_eq!(d.lry, 0);
        assert_eq!(d.lrx, 1);
    }

    #[test]
    fn decode_set_scissor_result_never_negative() {
        // All coordinate fields are declared int32_t in the source but,
        // per module doc "Admitted domain", can never go negative from
        // this decode: every field is 12 bits wide (max 0xFFF = 4095),
        // far short of i32's sign bit.
        let d = decode_set_scissor(0xFFFF_FFFF, 0xFFFF_FFFF);
        assert!(d.ulx >= 0 && d.uly >= 0 && d.lrx >= 0 && d.lry >= 0);
    }

    // --- decode_set_convert ---

    #[test]
    fn decode_set_convert_all_zero() {
        let d = decode_set_convert(0, 0);
        assert_eq!(
            d,
            SetConvertDecoded {
                k0: 0,
                k1: 0,
                k2: 0,
                k3: 0,
                k4: 0,
                k5: 0,
            }
        );
    }

    #[test]
    fn decode_set_convert_all_ones() {
        // Every 9-bit field reads its all-ones value, 0x1FF = 511.
        let d = decode_set_convert(0xFFFF_FFFF, 0xFFFF_FFFF);
        assert_eq!(
            d,
            SetConvertDecoded {
                k0: 0x1FF,
                k1: 0x1FF,
                k2: 0x1FF,
                k3: 0x1FF,
                k4: 0x1FF,
                k5: 0x1FF,
            }
        );
    }

    #[test]
    fn decode_set_convert_all_ones_result_never_negative() {
        // Pinning the "no sign extension in this decode" fact from module
        // doc "Admitted domain": the all-ones case must yield 511, not -1.
        let d = decode_set_convert(0xFFFF_FFFF, 0xFFFF_FFFF);
        assert_eq!(d.k0, 511);
        assert!(d.k0 >= 0 && d.k1 >= 0 && d.k2 >= 0 && d.k3 >= 0 && d.k4 >= 0 && d.k5 >= 0);
    }

    #[test]
    fn decode_set_convert_k0_at_max_nine_bits() {
        // k0 = p0(13, 9): bits 13-21 of w0.
        let d = decode_set_convert(0x1FF << 13, 0);
        assert_eq!(d.k0, 0x1FF);
        assert_eq!(d.k1, 0);
    }

    #[test]
    fn decode_set_convert_k0_one_bit_above_max_is_masked() {
        // bit 22 of w0 is outside k0's field -- must not leak in.
        let d = decode_set_convert(0x1 << 22, 0);
        assert_eq!(d.k0, 0);
    }

    #[test]
    fn decode_set_convert_k1_at_max_nine_bits() {
        // k1 = p0(4, 9): bits 4-12 of w0.
        let d = decode_set_convert(0x1FF << 4, 0);
        assert_eq!(d.k1, 0x1FF);
        assert_eq!(d.k0, 0);
    }

    #[test]
    fn decode_set_convert_k1_one_bit_above_max_is_masked() {
        // bit 13 of w0 belongs to k0, not k1.
        let d = decode_set_convert(0x1 << 13, 0);
        assert_eq!(d.k1, 0);
        assert_eq!(d.k0, 1);
    }

    #[test]
    fn decode_set_convert_k2_low_nibble_from_w0_only() {
        // k2 = (p0(0,4) << 5) | p1(27,5): low 4 bits of w0, shifted left 5.
        let d = decode_set_convert(0xF, 0);
        assert_eq!(d.k2, 0xF << 5);
    }

    #[test]
    fn decode_set_convert_k2_low_nibble_one_bit_above_max_is_masked() {
        // bit 4 of w0 is outside p0(0,4)'s field.
        let d = decode_set_convert(0x1 << 4, 0);
        assert_eq!(d.k2, 0);
    }

    #[test]
    fn decode_set_convert_k2_high_five_bits_from_w1_only() {
        // p1(27, 5): bits 27-31 of w1, OR'd in unshifted.
        let d = decode_set_convert(0, 0x1F << 27);
        assert_eq!(d.k2, 0x1F);
    }

    #[test]
    fn decode_set_convert_k2_high_five_bits_wrap_at_bit_32() {
        // p1(27, 5) at bit 31 (the top bit of w1) is still in-field; there
        // is no bit 32 to test "one above max" against for this half, so
        // this pins the top edge of the 5-bit window instead.
        let d = decode_set_convert(0, 0x1 << 31);
        assert_eq!(d.k2, 0x1 << 4);
    }

    #[test]
    fn decode_set_convert_k2_combines_both_halves_without_bit_collision() {
        // Low nibble all-ones (contributes 0xF << 5 = 0x1E0) OR'd with high
        // five-bits all-ones (contributes 0x1F) = 0x1FF, the full 9-bit
        // range, with no overlapping bit between the two halves.
        let d = decode_set_convert(0xF, 0x1F << 27);
        assert_eq!(d.k2, 0x1FF);
    }

    #[test]
    fn decode_set_convert_k3_at_max_nine_bits() {
        // k3 = p1(18, 9): bits 18-26 of w1.
        let d = decode_set_convert(0, 0x1FF << 18);
        assert_eq!(d.k3, 0x1FF);
        assert_eq!(d.k4, 0);
    }

    #[test]
    fn decode_set_convert_k3_one_bit_above_max_is_masked() {
        // bit 27 of w1 belongs to k2's high half, not k3.
        let d = decode_set_convert(0, 0x1 << 27);
        assert_eq!(d.k3, 0);
        assert_eq!(d.k2, 1);
    }

    #[test]
    fn decode_set_convert_k4_at_max_nine_bits() {
        // k4 = p1(9, 9): bits 9-17 of w1.
        let d = decode_set_convert(0, 0x1FF << 9);
        assert_eq!(d.k4, 0x1FF);
        assert_eq!(d.k5, 0);
    }

    #[test]
    fn decode_set_convert_k4_one_bit_above_max_is_masked() {
        // bit 18 of w1 belongs to k3, not k4.
        let d = decode_set_convert(0, 0x1 << 18);
        assert_eq!(d.k4, 0);
        assert_eq!(d.k3, 1);
    }

    #[test]
    fn decode_set_convert_k5_at_max_nine_bits() {
        // k5 = p1(0, 9): bits 0-8 of w1.
        let d = decode_set_convert(0, 0x1FF);
        assert_eq!(d.k5, 0x1FF);
    }

    #[test]
    fn decode_set_convert_k5_one_bit_above_max_is_masked() {
        // bit 9 of w1 belongs to k4, not k5.
        let d = decode_set_convert(0, 0x1 << 9);
        assert_eq!(d.k5, 0);
        assert_eq!(d.k4, 1);
    }

    // --- decode_set_key_r ---

    #[test]
    fn decode_set_key_r_all_zero() {
        let d = decode_set_key_r(0, 0);
        assert_eq!(
            d,
            SetKeyRDecoded {
                c_r: 0,
                s_r: 0,
                w_r: 0,
            }
        );
    }

    #[test]
    fn decode_set_key_r_all_ones() {
        let d = decode_set_key_r(0xFFFF_FFFF, 0xFFFF_FFFF);
        assert_eq!(
            d,
            SetKeyRDecoded {
                c_r: 0xFF,
                s_r: 0xFF,
                w_r: 0xFFF,
            }
        );
    }

    #[test]
    fn decode_set_key_r_w0_is_unused() {
        // w0 does not feed any field of setKeyR.
        let d = decode_set_key_r(0xFFFF_FFFF, 0);
        assert_eq!(
            d,
            SetKeyRDecoded {
                c_r: 0,
                s_r: 0,
                w_r: 0,
            }
        );
    }

    #[test]
    fn decode_set_key_r_c_r_at_max_eight_bits() {
        // cR = p1(8, 8): bits 8-15 of w1.
        let d = decode_set_key_r(0, 0xFF << 8);
        assert_eq!(d.c_r, 0xFF);
        assert_eq!(d.s_r, 0);
        assert_eq!(d.w_r, 0);
    }

    #[test]
    fn decode_set_key_r_c_r_one_bit_above_max_is_masked() {
        // bit 16 of w1 belongs to w_r, not c_r.
        let d = decode_set_key_r(0, 0x1 << 16);
        assert_eq!(d.c_r, 0);
        assert_eq!(d.w_r, 1);
    }

    #[test]
    fn decode_set_key_r_s_r_at_max_eight_bits() {
        // sR = p1(0, 8): bits 0-7 of w1.
        let d = decode_set_key_r(0, 0xFF);
        assert_eq!(d.s_r, 0xFF);
        assert_eq!(d.c_r, 0);
    }

    #[test]
    fn decode_set_key_r_s_r_one_bit_above_max_is_masked() {
        // bit 8 of w1 belongs to c_r, not s_r.
        let d = decode_set_key_r(0, 0x1 << 8);
        assert_eq!(d.s_r, 0);
        assert_eq!(d.c_r, 1);
    }

    #[test]
    fn decode_set_key_r_w_r_at_max_twelve_bits() {
        // wR = p1(16, 12): bits 16-27 of w1.
        let d = decode_set_key_r(0, 0xFFF << 16);
        assert_eq!(d.w_r, 0xFFF);
        assert_eq!(d.c_r, 0);
    }

    #[test]
    fn decode_set_key_r_w_r_one_bit_above_max_is_masked() {
        // bit 28 of w1 is outside w_r's field -- must not leak in.
        let d = decode_set_key_r(0, 0x1 << 28);
        assert_eq!(d.w_r, 0);
    }

    // --- decode_set_key_gb ---

    #[test]
    fn decode_set_key_gb_all_zero() {
        let d = decode_set_key_gb(0, 0);
        assert_eq!(
            d,
            SetKeyGbDecoded {
                c_g: 0,
                s_g: 0,
                w_g: 0,
                c_b: 0,
                s_b: 0,
                w_b: 0,
            }
        );
    }

    #[test]
    fn decode_set_key_gb_all_ones() {
        let d = decode_set_key_gb(0xFFFF_FFFF, 0xFFFF_FFFF);
        assert_eq!(
            d,
            SetKeyGbDecoded {
                c_g: 0xFF,
                s_g: 0xFF,
                w_g: 0xFFF,
                c_b: 0xFF,
                s_b: 0xFF,
                w_b: 0xFFF,
            }
        );
    }

    #[test]
    fn decode_set_key_gb_c_g_at_max_eight_bits() {
        // cG = p1(24, 8): bits 24-31 of w1.
        let d = decode_set_key_gb(0, 0xFFu32 << 24);
        assert_eq!(d.c_g, 0xFF);
        assert_eq!(d.s_g, 0);
    }

    #[test]
    fn decode_set_key_gb_s_g_at_max_eight_bits() {
        // sG = p1(16, 8): bits 16-23 of w1.
        let d = decode_set_key_gb(0, 0xFF << 16);
        assert_eq!(d.s_g, 0xFF);
        assert_eq!(d.c_g, 0);
    }

    #[test]
    fn decode_set_key_gb_s_g_one_bit_above_max_is_masked() {
        // bit 24 of w1 belongs to c_g, not s_g.
        let d = decode_set_key_gb(0, 0x1 << 24);
        assert_eq!(d.s_g, 0);
        assert_eq!(d.c_g, 1);
    }

    #[test]
    fn decode_set_key_gb_w_g_at_max_twelve_bits() {
        // wG = p0(12, 12): bits 12-23 of w0.
        let d = decode_set_key_gb(0xFFF << 12, 0);
        assert_eq!(d.w_g, 0xFFF);
        assert_eq!(d.w_b, 0);
    }

    #[test]
    fn decode_set_key_gb_w_g_one_bit_above_max_is_masked() {
        // bit 24 of w0 is outside w_g's field.
        let d = decode_set_key_gb(0x1 << 24, 0);
        assert_eq!(d.w_g, 0);
    }

    #[test]
    fn decode_set_key_gb_c_b_at_max_eight_bits() {
        // cB = p1(8, 8): bits 8-15 of w1.
        let d = decode_set_key_gb(0, 0xFF << 8);
        assert_eq!(d.c_b, 0xFF);
        assert_eq!(d.s_b, 0);
    }

    #[test]
    fn decode_set_key_gb_c_b_one_bit_above_max_is_masked() {
        // bit 16 of w1 belongs to s_g, not c_b.
        let d = decode_set_key_gb(0, 0x1 << 16);
        assert_eq!(d.c_b, 0);
        assert_eq!(d.s_g, 1);
    }

    #[test]
    fn decode_set_key_gb_s_b_at_max_eight_bits() {
        // sB = p1(0, 8): bits 0-7 of w1.
        let d = decode_set_key_gb(0, 0xFF);
        assert_eq!(d.s_b, 0xFF);
        assert_eq!(d.c_b, 0);
    }

    #[test]
    fn decode_set_key_gb_s_b_one_bit_above_max_is_masked() {
        // bit 8 of w1 belongs to c_b, not s_b.
        let d = decode_set_key_gb(0, 0x1 << 8);
        assert_eq!(d.s_b, 0);
        assert_eq!(d.c_b, 1);
    }

    #[test]
    fn decode_set_key_gb_w_b_at_max_twelve_bits() {
        // wB = p0(0, 12): bits 0-11 of w0.
        let d = decode_set_key_gb(0xFFF, 0);
        assert_eq!(d.w_b, 0xFFF);
        assert_eq!(d.w_g, 0);
    }

    #[test]
    fn decode_set_key_gb_w_b_one_bit_above_max_is_masked() {
        // bit 12 of w0 belongs to w_g, not w_b.
        let d = decode_set_key_gb(0x1 << 12, 0);
        assert_eq!(d.w_b, 0);
        assert_eq!(d.w_g, 1);
    }

    #[test]
    fn decode_set_key_gb_w0_w1_split_is_independent() {
        // wG/wB come from w0; cG/sG/cB/sB come from w1 -- setting every
        // w1 field to max must not perturb the w0-sourced fields, and
        // vice versa.
        let d = decode_set_key_gb(0xFFFF_FFFF, 0);
        assert_eq!(d.w_g, 0xFFF);
        assert_eq!(d.w_b, 0xFFF);
        assert_eq!(d.c_g, 0);
        assert_eq!(d.s_g, 0);
        assert_eq!(d.c_b, 0);
        assert_eq!(d.s_b, 0);
    }
}
