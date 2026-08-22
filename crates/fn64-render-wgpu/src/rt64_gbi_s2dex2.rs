//! Literal port of RT64's S2DEX2 display-list command-word bitfield
//! decoding, a literal port of the permitted MIT RT64 Rust-port source
//! pinned at commit `5473732a822a4423b5696e7cb18fecc425a59875`
//! (`docs/RT64-PORT-AUTHORITY.md`), `src/gbi/rt64_gbi_s2dex2.cpp` (SHA-256
//! of the whole file, `cf219a097e7a3300954349acb633aca106c3c41ead90dc4c825a7438525e1e02`):
//!
//! Only the bitfield-extraction shape of `moveWord` and `rdpHalf0` is
//! ported as pure functions over `(w0, w1)` -- the `state->rsp->`/`state->`
//! dispatch bodies (`GBI_F3DEX2::moveWord`, `GBI_RDP::texrect`, the
//! `assert(false)` stub branches) are out of scope (see "Nonclaims"). No
//! other function in the 91-line source file performs bitfield decoding:
//! `reset`/`resetFromLoad` write plain struct fields with no bit math, and
//! `setup` only populates opcode-dispatch tables (`gbi->map`/
//! `gbi->constants`) with function pointers and plain integer constants --
//! neither is a decode function over a `DisplayList` command word, so
//! neither is ported here.
//!
//! ```text
//! // src/gbi/rt64_gbi_s2dex2.cpp:16-35
//! void moveWord(State *state, DisplayList **dl) {
//!     switch ((*dl)->p0(16, 8)) {
//!     case G_MW_GENSTAT:
//!         assert(false);
//!         break;
//!     default:
//!         GBI_F3DEX2::moveWord(state, dl);
//!         break;
//!     }
//! }
//!
//! void rdpHalf0(State *state, DisplayList **dl) {
//!     uint8_t nextCode = (*dl + 1)->w0 >> 24;
//!     if (nextCode == S2DEX2_G_SELECT_DL) {
//!         assert(false);
//!     }
//!     else if (nextCode == F3DEX2_G_RDPHALF_1) {
//!         GBI_RDP::texrect(state, dl);
//!     }
//! }
//!
//! // src/gbi/rt64_display_list.h:8-15
//! struct DisplayList {
//!     uint32_t w0;
//!     uint32_t w1;
//!
//!     DisplayList();
//!     uint32_t p0(uint8_t pos, uint8_t bits) const;
//!     uint32_t p1(uint8_t pos, uint8_t bits) const;
//! };
//!
//! // src/gbi/rt64_gbi.cpp:32-38
//! uint32_t DisplayList::p0(uint8_t pos, uint8_t bits) const {
//!     return ((w0 >> pos) & ((0x01 << bits) - 1));
//! }
//!
//! uint32_t DisplayList::p1(uint8_t pos, uint8_t bits) const {
//!     return ((w1 >> pos) & ((0x01 << bits) - 1));
//! }
//!
//! // src/gbi/rt64_gbi_s2dex2.h:9-17 (opcode constants used above)
//! #define S2DEX2_G_RDPHALF_0 0xE4
//! #define S2DEX2_G_SELECT_DL 0x04
//!
//! // src/gbi/rt64_gbi_f3dex2.h:27 (opcode constant used above)
//! #define F3DEX2_G_RDPHALF_1 0xe1
//!
//! // src/shared/rt64_f3d_defines.h:113 (opcode constant used above)
//! #define G_MW_GENSTAT 0x08
//! ```
//!
//! **Reuse, not new type.** `crates/fn64-render-reference/src/gbi/` has a
//! full GBI implementation (including its own S2DEX/S2DEX2 decode and
//! dispatch), but this module is **not** wired to it and reuses none of its
//! types. The task scope is a from-scratch characterization port of
//! `rt64_gbi_s2dex2.cpp`'s own bitfield shape, matching the RT64 C++
//! source's own field extraction (`p0`/`p1`'s `pos`/`bits` shift-mask
//! pair) exactly, as `rt64_common.rs`/`rt64_math.rs` do for their own
//! source files -- not an integration with, or a refactor of, the existing
//! `fn64-render-reference` GBI, whose bitfield helpers (if any) were
//! authored independently and are not verified here to match RT64's
//! `p0(pos, bits)` shift/mask order bit-for-bit. Introducing a dependency
//! on `fn64-render-reference`'s GBI would also cross this crate's
//! characterization-first module boundary (see `rt64_common.rs`,
//! `rt64_math.rs`: every prior RT64 port in this crate is a standalone,
//! unwired module with its own local test-only decode logic).
//!
//! `DisplayList` itself is ported as a plain two-field struct
//! (`Word0Word1`, named to avoid colliding with any future full port of
//! `rt64_display_list.h`) with the two `p0`/`p1` methods, rather than
//! reusing an existing `fn64-render-reference` or `fn64-render-ir` command
//! type, for the same reason: this module characterizes RT64's own
//! extractor, not any existing fn64 command representation.
//!
//! ## Admitted domain
//!
//! - **`p0`/`p1`'s shift-then-mask order is preserved exactly**: `(w >>
//!   pos) & ((0x01 << bits) - 1)` -- shift right by `pos` bits *first*,
//!   then mask to the low `bits` bits of the shifted value. This is not
//!   equivalent to masking first (`(w & (mask << pos)) >> pos`) in terms of
//!   intermediate values, though both give the same final result for a
//!   well-formed `(pos, bits)` pair with `pos + bits <= 32`; the port
//!   preserves the literal shift-then-mask sequence, not the
//!   mathematically-equivalent alternative.
//! - **`w0` vs `w1`**: `p0` reads `w0` (the display-list command word's
//!   high word -- opcode + top-level operands), `p1` reads `w1` (the low
//!   word). `moveWord` calls `p0(16, 8)`: bits `[23:16]` of `w0`. `rdpHalf0`
//!   reads the *next* display-list entry's `w0 >> 24` (bits `[31:24]`, i.e.
//!   the next command's opcode byte) directly, **not** through `p0`/`p1` --
//!   this is a raw shift with no mask, since shifting a `uint32_t` right by
//!   24 already leaves only 8 significant bits, so `& 0xFF` would be
//!   redundant (and the source omits it). This port keeps that same
//!   redundant-mask omission for `rdp_half0_next_opcode`: a plain `w0 >>
//!   24` with no explicit `& 0xFF`, since the top 24 bits are already zero
//!   after the shift.
//! - **`(0x01 << bits) - 1` mask construction**: for `bits = 8` (the only
//!   width `moveWord` uses, via `p0(16, 8)`), this is `(1 << 8) - 1 =
//!   0xFF`, exactly `u8::MAX` -- no edge case at this call site. The port
//!   nonetheless implements `p0`/`p1` generically over `pos`/`bits` (not
//!   hardcoded to width 8) since the source's `DisplayList::p0`/`p1` are
//!   themselves generic, and the task calls for the extractor's own
//!   semantics, not just its one call site.
//! - **Sign-extension: none is present in the ported code.** Every value
//!   extracted here (`p0`/`p1`'s bitfields, and `rdpHalf0`'s next-opcode
//!   byte) is an *unsigned* field compared against unsigned opcode/moveword
//!   constants -- `moveWord`'s switch discriminant and `rdpHalf0`'s
//!   `nextCode` are both compared for bitwise equality against `#define`d
//!   hex opcode constants, never interpreted as a signed magnitude or a
//!   fixed-point coordinate. This file (unlike `GBI_S2DEX::bg1Cyc`'s
//!   `int16_t`/`int32_t` frame/image coordinate math, which is *not* ported
//!   here -- see "Nonclaims") performs no signed fixed-point decoding, so
//!   there is no sign-extension question to resolve for the functions this
//!   module actually covers. This is stated explicitly per the task's
//!   sign-extension instruction, not silently skipped.
//! - **Truncation to narrower ints**: `rdpHalf0`'s `uint8_t nextCode = (*dl
//!   + 1)->w0 >> 24` truncates a `uint32_t` shift result to `uint8_t`. Since
//!   the shift already zeroes the top 24 bits, the truncation is lossless
//!   (the shifted value already fits in 8 bits) -- ported as `((w0 >> 24) &
//!   0xFF) as u8`, with an explicit `& 0xFF` added defensively even though
//!   it is a no-op after `>> 24` on a `u32`, to make the "fits in `u8`"
//!   invariant visible at the Rust call site (Rust's `as u8` cast on a
//!   `u32` truncates silently, unlike C++'s implicit narrowing conversion
//!   which is well-defined here but easy to misread).
//! - **C++ integer promotion**: `p0`/`p1`'s `pos`/`bits` parameters are
//!   `uint8_t` in the C++ signature, but C++'s usual arithmetic
//!   conversions promote both to `int` before the shift/subtract/mask
//!   arithmetic executes (`w0 >> pos` promotes `pos` to `int`; `0x01 <<
//!   bits` promotes `bits` to `int`; the `- 1` is `int` arithmetic). Since
//!   `pos`/`bits` are always small non-negative literals at every call site
//!   in this file (`16`, `8`), the promoted-to-`int` intermediate values
//!   never approach `int`'s range limits, so this port uses `u32` shift
//!   amounts directly (Rust has no implicit integer promotion) with no
//!   observable difference in result -- this is the same admitted-domain
//!   choice `rt64_common.rs` and `rt64_math.rs` make for their own
//!   C++-promoted intermediate arithmetic.
//! - **Open question, resolved conservatively: `p0`/`p1` at `bits = 32` is
//!   outside this port's well-defined domain, and this is a genuine
//!   ambiguity, not a confident guess.** `(0x01 << bits) - 1` with `bits =
//!   32` is `1 << 32` on a (promoted-to-)32-bit type: in C++ this is a
//!   shift-amount-equal-to-the-operand-width, which is **undefined
//!   behavior** per the C++ standard (`[expr.shift]`: the shift amount
//!   must be less than the width of the promoted left operand) -- no real
//!   call site in `rt64_gbi_s2dex2.cpp` ever passes `bits = 32` (the only
//!   call site is `p0(16, 8)`), so upstream never exercises this path.
//!   Rust's `1u32 << 32` is likewise a panic in debug builds ("attempt to
//!   shift left with overflow") and a documented-unspecified
//!   (implementation-defined-by-LLVM, effectively a masked shift) result in
//!   release builds -- neither language gives a portable answer here. This
//!   port does **not** special-case `bits = 32` to silently return
//!   `u32::MAX` (which would be the "intended" full-width-mask behavior a
//!   caller might expect): doing so would invent behavior beyond what the
//!   literal C++ expression specifies, contradicting the literal-port rule.
//!   The characterization tests below exercise the widest *well-defined*
//!   width (`bits = 31`) instead of `32`, and this paragraph records the
//!   `bits = 32` case as an explicit open question for any future caller
//!   that might pass a full 32-bit field width to `p0`/`p1`.
//!
//! ## Nonclaims
//!
//! No GPU, WGSL, or production wiring (this module is not called from
//! anywhere yet, matching `rt64_common.rs`'s and `rt64_math.rs`'s
//! precedent -- dead-code warnings on the unused public surface are
//! expected and correct), and no RT64 visual/pixel/silicon parity or
//! performance claim. Not wired to `fn64-render-reference`'s GBI (see
//! "Reuse, not new type"). Deliberately not ported from
//! `rt64_gbi_s2dex2.cpp`:
//!
//! - **All `state->rsp->`/`state->` dispatch bodies**: `moveWord`'s
//!   `GBI_F3DEX2::moveWord(state, dl)` default-case delegation and its
//!   `G_MW_GENSTAT` `assert(false)` stub branch; `rdpHalf0`'s
//!   `GBI_RDP::texrect(state, dl)` call and its `S2DEX2_G_SELECT_DL`
//!   `assert(false)` stub branch. These require the whole `State` object
//!   graph (RSP/RDP mutable state, RDRAM access, the full opcode-dispatch
//!   table) and are explicitly out of the task's DECODE-only scope. Both
//!   upstream `assert(false)` stubs are *not implemented* in the C++
//!   source itself (they are unconditional aborts, not real behavior) --
//!   this port does not invent behavior for either; the decode functions
//!   below return which branch *would* be taken (an enum discriminant),
//!   leaving the caller (were one ever wired) to decide what an
//!   unimplemented branch means, rather than panicking or guessing.
//! - **`reset`/`resetFromLoad`**: plain field writes on `state->rsp->` (no
//!   bitfield decoding of any display-list command word -- out of the
//!   task's "DECODE only" scope by definition, and would require the `RSP`/
//!   `S2D` state-object graph besides).
//! - **`setup`**: populates `gbi->constants` and `gbi->map` with function
//!   pointers and plain integer constants; contains no display-list
//!   command-word bitfield decoding.
//! - **`DisplayList`'s default constructor** (`w0 = w1 = 0`): trivial
//!   zero-initialization with no decode behavior to characterize;
//!   `Word0Word1` below derives `Default` instead (equivalent behavior, no
//!   named constructor needed for a two-field POD in Rust).
//! - **The full `GBI_S2DEX::` family this file's `setup()` wires into**
//!   (`objRenderMode`, `bg1Cyc`, `bgCopy`, `objLoadTxtr`,
//!   `objLoadTxSprite`, `objLoadTxRect`, `objLoadTxRectR`, and their
//!   `uObjBg`/`uObjScaleBg_t`/`uObjTxSprite`/`uObjTxtr` struct-layout
//!   bitfield decoding in `src/gbi/rt64_gbi_s2dex.cpp`) is a **different
//!   source file** (`rt64_gbi_s2dex2.cpp`'s `setup()` only takes their
//!   *addresses* as function pointers -- it does not define or inline
//!   their bodies) and is out of this task's named scope, which is
//!   `rt64_gbi_s2dex2.cpp` specifically. `rt64_gbi_s2dex.cpp` is already
//!   tracked separately in `docs/RT64-PORT-AUTHORITY.md`'s fn64-source-
//!   overlay table under the `s2dex-object-rect:v3` mechanism at milestone
//!   `M5` -- a distinct, already-scoped effort this module does not
//!   duplicate or preempt.

/// The two 32-bit command words of one RT64 `DisplayList` entry (`w0`/`w1`
/// in the C++ source). Named `Word0Word1` rather than `DisplayList` to
/// avoid colliding with any future full port of `rt64_display_list.h`
/// (see module doc "Reuse, not new type").
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Word0Word1 {
    pub w0: u32,
    pub w1: u32,
}

impl Word0Word1 {
    /// `DisplayList::p0(pos, bits)`: extracts `bits` bits of `w0` starting
    /// at bit `pos`, shift-then-mask (see module doc "Admitted domain").
    pub fn p0(&self, pos: u32, bits: u32) -> u32 {
        (self.w0 >> pos) & ((0x01u32 << bits) - 1)
    }

    /// `DisplayList::p1(pos, bits)`: extracts `bits` bits of `w1` starting
    /// at bit `pos`, shift-then-mask (see module doc "Admitted domain").
    pub fn p1(&self, pos: u32, bits: u32) -> u32 {
        (self.w1 >> pos) & ((0x01u32 << bits) - 1)
    }
}

/// Which branch `GBI_S2DEX2::moveWord`'s `switch ((*dl)->p0(16, 8))` would
/// take. `Genstat` is the `assert(false)` stub (see module doc
/// "Nonclaims" -- upstream does not implement it); `Delegate` is the
/// `default:` branch that calls out to `GBI_F3DEX2::moveWord` (not ported
/// here -- dispatch is out of scope).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MoveWordBranch {
    /// `case G_MW_GENSTAT:` -- upstream stub (`assert(false)`), not
    /// implemented.
    Genstat,
    /// `default:` -- delegates to `GBI_F3DEX2::moveWord` (dispatch, not
    /// ported).
    Delegate,
}

/// `G_MW_GENSTAT` (`src/shared/rt64_f3d_defines.h:113`).
pub const G_MW_GENSTAT: u32 = 0x08;

/// `GBI_S2DEX2::moveWord`'s switch discriminant and branch decision:
/// `(*dl)->p0(16, 8)` compared against `G_MW_GENSTAT`. Returns the moveword
/// index (the raw `p0(16, 8)` value) alongside which branch it selects.
pub fn move_word_decode(dl: Word0Word1) -> (u32, MoveWordBranch) {
    let index = dl.p0(16, 8);
    let branch = if index == G_MW_GENSTAT {
        MoveWordBranch::Genstat
    } else {
        MoveWordBranch::Delegate
    };
    (index, branch)
}

/// Which branch `GBI_S2DEX2::rdpHalf0` would take, keyed on the *next*
/// display-list entry's opcode byte (`(*dl + 1)->w0 >> 24`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RdpHalf0Branch {
    /// `nextCode == S2DEX2_G_SELECT_DL` -- upstream stub (`assert(false)`),
    /// not implemented.
    SelectDl,
    /// `nextCode == F3DEX2_G_RDPHALF_1` -- delegates to
    /// `GBI_RDP::texrect` (dispatch, not ported).
    Texrect,
    /// Neither `if` nor `else if` condition matched -- upstream `rdpHalf0`
    /// falls through and does nothing.
    NoOp,
}

/// `S2DEX2_G_SELECT_DL` (`src/gbi/rt64_gbi_s2dex2.h:13`).
pub const S2DEX2_G_SELECT_DL: u8 = 0x04;

/// `F3DEX2_G_RDPHALF_1` (`src/gbi/rt64_gbi_f3dex2.h:27`).
pub const F3DEX2_G_RDPHALF_1: u8 = 0xe1;

/// `GBI_S2DEX2::rdpHalf0`'s next-opcode extraction and branch decision:
/// `uint8_t nextCode = (*dl + 1)->w0 >> 24`, then compared against
/// `S2DEX2_G_SELECT_DL` and `F3DEX2_G_RDPHALF_1` in that order (see module
/// doc "Admitted domain" for why no `p0`/mask is used here). `next_dl_w0`
/// is the *next* display-list entry's `w0` (`(*dl + 1)->w0`), not the
/// current entry's.
pub fn rdp_half0_decode(next_dl_w0: u32) -> (u8, RdpHalf0Branch) {
    let next_code = ((next_dl_w0 >> 24) & 0xFF) as u8;
    let branch = if next_code == S2DEX2_G_SELECT_DL {
        RdpHalf0Branch::SelectDl
    } else if next_code == F3DEX2_G_RDPHALF_1 {
        RdpHalf0Branch::Texrect
    } else {
        RdpHalf0Branch::NoOp
    };
    (next_code, branch)
}

#[cfg(test)]
mod rt64_gbi_s2dex2_tests {
    use super::*;

    // --- Word0Word1::p0 ---

    #[test]
    fn p0_all_zero_word_yields_zero() {
        let dl = Word0Word1 { w0: 0, w1: 0 };
        assert_eq!(dl.p0(0, 8), 0);
        assert_eq!(dl.p0(16, 8), 0);
    }

    #[test]
    fn p0_all_ones_word_yields_full_mask_for_width() {
        let dl = Word0Word1 {
            w0: 0xFFFF_FFFF,
            w1: 0,
        };
        assert_eq!(dl.p0(16, 8), 0xFF);
        assert_eq!(dl.p0(0, 1), 0x1);
        // bits=31 is the widest well-defined width (see module doc
        // "Admitted domain" -- bits=32 is a shift-amount-equal-to-width
        // overflow in both C++'s promoted-`int` arithmetic and Rust's `u32`
        // shl, so it is deliberately not exercised here).
        assert_eq!(dl.p0(0, 31), 0x7FFF_FFFF);
    }

    #[test]
    fn p0_field_max_value_extracts_exactly() {
        // bits [23:16] = 0xFF, everything else 0.
        let dl = Word0Word1 {
            w0: 0x00FF_0000,
            w1: 0,
        };
        assert_eq!(dl.p0(16, 8), 0xFF);
    }

    #[test]
    fn p0_one_bit_above_field_width_is_masked_out() {
        // bit 24 set (one above the [23:16] field's top), field itself 0.
        let dl = Word0Word1 {
            w0: 0x0100_0000,
            w1: 0,
        };
        assert_eq!(dl.p0(16, 8), 0);
    }

    #[test]
    fn p0_one_bit_below_field_range_is_masked_out() {
        // bit 15 set (one below the [23:16] field's bottom), field itself 0.
        let dl = Word0Word1 {
            w0: 0x0000_8000,
            w1: 0,
        };
        assert_eq!(dl.p0(16, 8), 0);
    }

    #[test]
    fn p0_reads_w0_not_w1() {
        let dl = Word0Word1 {
            w0: 0,
            w1: 0x00FF_0000,
        };
        assert_eq!(dl.p0(16, 8), 0);
    }

    #[test]
    fn p0_arbitrary_bit_pattern_extracts_middle_field() {
        // w0 = 0b...1010_1100... ; extract bits [7:4] = 0b1010 = 0xA.
        let dl = Word0Word1 {
            w0: 0b0000_0000_0000_0000_0000_0000_1010_1100,
            w1: 0,
        };
        assert_eq!(dl.p0(4, 4), 0xA);
    }

    #[test]
    fn p0_width_one_extracts_single_bit() {
        let dl = Word0Word1 {
            w0: 0b0000_0000_0000_0000_0000_0000_0000_0010,
            w1: 0,
        };
        assert_eq!(dl.p0(1, 1), 1);
        assert_eq!(dl.p0(0, 1), 0);
        assert_eq!(dl.p0(2, 1), 0);
    }

    // --- Word0Word1::p1 ---

    #[test]
    fn p1_all_zero_word_yields_zero() {
        let dl = Word0Word1 { w0: 0, w1: 0 };
        assert_eq!(dl.p1(0, 24), 0);
    }

    #[test]
    fn p1_all_ones_word_yields_full_mask_for_width() {
        let dl = Word0Word1 {
            w0: 0,
            w1: 0xFFFF_FFFF,
        };
        assert_eq!(dl.p1(0, 24), 0x00FF_FFFF);
        assert_eq!(dl.p1(24, 8), 0xFF);
    }

    #[test]
    fn p1_reads_w1_not_w0() {
        let dl = Word0Word1 {
            w0: 0x00FF_0000,
            w1: 0,
        };
        assert_eq!(dl.p1(16, 8), 0);
    }

    #[test]
    fn p1_field_max_value_extracts_exactly() {
        let dl = Word0Word1 {
            w0: 0,
            w1: 0x00FF_0000,
        };
        assert_eq!(dl.p1(16, 8), 0xFF);
    }

    #[test]
    fn p1_one_bit_above_field_width_is_masked_out() {
        let dl = Word0Word1 {
            w0: 0,
            w1: 0x0100_0000,
        };
        assert_eq!(dl.p1(16, 8), 0);
    }

    #[test]
    fn p1_widest_well_defined_width_returns_low_31_bits() {
        // bits=31, not 32 -- see p0_all_ones_word_yields_full_mask_for_width.
        let dl = Word0Word1 {
            w0: 0,
            w1: 0x1234_5678,
        };
        assert_eq!(dl.p1(0, 31), 0x1234_5678 & 0x7FFF_FFFF);
    }

    // --- move_word_decode ---

    #[test]
    fn move_word_decode_zero_index_is_delegate_branch() {
        // p0(16, 8) = 0, which is not G_MW_GENSTAT (0x08).
        let dl = Word0Word1 { w0: 0, w1: 0 };
        let (index, branch) = move_word_decode(dl);
        assert_eq!(index, 0);
        assert_eq!(branch, MoveWordBranch::Delegate);
    }

    #[test]
    fn move_word_decode_genstat_index_selects_genstat_branch() {
        // G_MW_GENSTAT = 0x08 placed at bits [23:16].
        let dl = Word0Word1 {
            w0: 0x0008_0000,
            w1: 0,
        };
        let (index, branch) = move_word_decode(dl);
        assert_eq!(index, G_MW_GENSTAT);
        assert_eq!(branch, MoveWordBranch::Genstat);
    }

    #[test]
    fn move_word_decode_max_field_value_is_delegate_branch() {
        // p0(16, 8) = 0xFF, not G_MW_GENSTAT.
        let dl = Word0Word1 {
            w0: 0x00FF_0000,
            w1: 0,
        };
        let (index, branch) = move_word_decode(dl);
        assert_eq!(index, 0xFF);
        assert_eq!(branch, MoveWordBranch::Delegate);
    }

    #[test]
    fn move_word_decode_one_bit_above_field_does_not_leak_into_index() {
        // Bit 24 set (one above the field's top) must not affect p0(16, 8).
        let dl = Word0Word1 {
            w0: 0x0108_0000,
            w1: 0,
        };
        let (index, branch) = move_word_decode(dl);
        assert_eq!(index, G_MW_GENSTAT);
        assert_eq!(branch, MoveWordBranch::Genstat);
    }

    #[test]
    fn move_word_decode_ignores_w1_entirely() {
        let dl = Word0Word1 {
            w0: 0x0008_0000,
            w1: 0xFFFF_FFFF,
        };
        let (index, branch) = move_word_decode(dl);
        assert_eq!(index, G_MW_GENSTAT);
        assert_eq!(branch, MoveWordBranch::Genstat);
    }

    #[test]
    fn move_word_decode_value_adjacent_to_genstat_is_delegate() {
        // 0x07 and 0x09 must not be mistaken for 0x08.
        let below = Word0Word1 {
            w0: 0x0007_0000,
            w1: 0,
        };
        let above = Word0Word1 {
            w0: 0x0009_0000,
            w1: 0,
        };
        assert_eq!(move_word_decode(below).1, MoveWordBranch::Delegate);
        assert_eq!(move_word_decode(above).1, MoveWordBranch::Delegate);
    }

    // --- rdp_half0_decode ---

    #[test]
    fn rdp_half0_decode_zero_word_is_no_op_branch() {
        let (code, branch) = rdp_half0_decode(0);
        assert_eq!(code, 0);
        assert_eq!(branch, RdpHalf0Branch::NoOp);
    }

    #[test]
    fn rdp_half0_decode_all_ones_word_selects_select_dl_masked_to_ff() {
        // (0xFFFFFFFF >> 24) & 0xFF = 0xFF, which matches neither constant
        // (S2DEX2_G_SELECT_DL=0x04, F3DEX2_G_RDPHALF_1=0xe1) -> NoOp.
        let (code, branch) = rdp_half0_decode(0xFFFF_FFFF);
        assert_eq!(code, 0xFF);
        assert_eq!(branch, RdpHalf0Branch::NoOp);
    }

    #[test]
    fn rdp_half0_decode_select_dl_opcode_selects_select_dl_branch() {
        // S2DEX2_G_SELECT_DL = 0x04 placed at bits [31:24].
        let (code, branch) = rdp_half0_decode(0x0400_0000);
        assert_eq!(code, S2DEX2_G_SELECT_DL);
        assert_eq!(branch, RdpHalf0Branch::SelectDl);
    }

    #[test]
    fn rdp_half0_decode_rdphalf1_opcode_selects_texrect_branch() {
        // F3DEX2_G_RDPHALF_1 = 0xe1 placed at bits [31:24].
        let (code, branch) = rdp_half0_decode(0xE100_0000);
        assert_eq!(code, F3DEX2_G_RDPHALF_1);
        assert_eq!(branch, RdpHalf0Branch::Texrect);
    }

    #[test]
    fn rdp_half0_decode_select_dl_is_checked_before_rdphalf1_when_both_could_match() {
        // Both constants are distinct (0x04 vs 0xe1) so they can never both
        // match the same byte; this test documents that select_dl is the
        // first `if` in source order and therefore wins ties in principle.
        let (_, branch) = rdp_half0_decode(0x0400_0000);
        assert_eq!(branch, RdpHalf0Branch::SelectDl);
        assert_ne!(S2DEX2_G_SELECT_DL, F3DEX2_G_RDPHALF_1);
    }

    #[test]
    fn rdp_half0_decode_ignores_low_24_bits_of_next_dl_w0() {
        // Low 24 bits vary but the opcode byte (bits [31:24]) stays fixed
        // at F3DEX2_G_RDPHALF_1 -- branch must not change.
        let (code_a, branch_a) = rdp_half0_decode(0xE100_0000);
        let (code_b, branch_b) = rdp_half0_decode(0xE1FF_FFFF);
        assert_eq!(code_a, code_b);
        assert_eq!(branch_a, branch_b);
        assert_eq!(branch_a, RdpHalf0Branch::Texrect);
    }

    #[test]
    fn rdp_half0_decode_opcode_one_less_than_select_dl_is_no_op() {
        // 0x03 vs S2DEX2_G_SELECT_DL (0x04): boundary just below.
        let (code, branch) = rdp_half0_decode(0x0300_0000);
        assert_eq!(code, 0x03);
        assert_eq!(branch, RdpHalf0Branch::NoOp);
    }

    #[test]
    fn rdp_half0_decode_opcode_one_more_than_select_dl_is_no_op() {
        // 0x05 vs S2DEX2_G_SELECT_DL (0x04): boundary just above.
        let (code, branch) = rdp_half0_decode(0x0500_0000);
        assert_eq!(code, 0x05);
        assert_eq!(branch, RdpHalf0Branch::NoOp);
    }

    #[test]
    fn rdp_half0_decode_opcode_one_less_than_rdphalf1_is_no_op() {
        // 0xe0 vs F3DEX2_G_RDPHALF_1 (0xe1): boundary just below.
        let (code, branch) = rdp_half0_decode(0xE000_0000);
        assert_eq!(code, 0xE0);
        assert_eq!(branch, RdpHalf0Branch::NoOp);
    }

    #[test]
    fn rdp_half0_decode_opcode_one_more_than_rdphalf1_is_no_op() {
        // 0xe2 vs F3DEX2_G_RDPHALF_1 (0xe1): boundary just above.
        let (code, branch) = rdp_half0_decode(0xE200_0000);
        assert_eq!(code, 0xE2);
        assert_eq!(branch, RdpHalf0Branch::NoOp);
    }

    #[test]
    fn rdp_half0_decode_max_byte_value_0xff_is_no_op() {
        let (code, branch) = rdp_half0_decode(0xFF00_0000);
        assert_eq!(code, 0xFF);
        assert_eq!(branch, RdpHalf0Branch::NoOp);
    }

    #[test]
    fn rdp_half0_decode_truncation_drops_bit_32_and_above_conceptually() {
        // u32 has no bits above 31; this test documents that a next_dl_w0
        // with only the opcode byte set and all lower bits at their max
        // still truncates correctly to a single byte via >>24 & 0xFF.
        let (code, _) = rdp_half0_decode(0x04FF_FFFF);
        assert_eq!(code, S2DEX2_G_SELECT_DL);
    }

    // --- constants sanity (guards against silent redefinition drift) ---

    #[test]
    fn constant_g_mw_genstat_matches_source_define() {
        assert_eq!(G_MW_GENSTAT, 0x08);
    }

    #[test]
    fn constant_s2dex2_g_select_dl_matches_source_define() {
        assert_eq!(S2DEX2_G_SELECT_DL, 0x04);
    }

    #[test]
    fn constant_f3dex2_g_rdphalf_1_matches_source_define() {
        assert_eq!(F3DEX2_G_RDPHALF_1, 0xe1);
    }

    // --- Word0Word1 derived traits ---

    #[test]
    fn word0word1_default_is_all_zero() {
        let dl = Word0Word1::default();
        assert_eq!(dl.w0, 0);
        assert_eq!(dl.w1, 0);
    }

    #[test]
    fn word0word1_equality_compares_both_fields() {
        let a = Word0Word1 { w0: 1, w1: 2 };
        let b = Word0Word1 { w0: 1, w1: 2 };
        let c = Word0Word1 { w0: 1, w1: 3 };
        assert_eq!(a, b);
        assert_ne!(a, c);
    }
}
