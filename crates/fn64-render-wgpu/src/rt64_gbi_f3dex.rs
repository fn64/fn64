//! Literal port of RT64's F3DEX display-list command-word **bitfield
//! decoding**, from the permitted MIT RT64 Rust-port source pinned at
//! commit `5473732a822a4423b5696e7cb18fecc425a59875`
//! (`docs/RT64-PORT-AUTHORITY.md`), `src/gbi/rt64_gbi_f3dex.cpp` (SHA-256 of
//! the whole file,
//! `94a5c60fd4bfacf8ad065e3c8de77d1a4c623ec8fb7f1b6237a85fef00aa6522`).
//!
//! Only the **bitfield extraction and per-opcode operand layout** is
//! ported: `DisplayList::p0`/`p1` (from `src/gbi/rt64_display_list.h` and
//! `src/gbi/rt64_gbi.cpp`, cited below) plus the operand set each
//! opcode function reads from `(*dl)->w0`/`w1` via those two extractors.
//! The `state->rsp->someCall(...)` dispatch itself is NOT ported -- it
//! needs the whole `RSP`/`State` object graph, which is out of scope (see
//! "Nonclaims"). `GBI_F3DEX::setup` (the opcode-to-function dispatch table
//! and the `gbi->constants` map) is also not ported: it wires function
//! pointers and enum constants from other GBI files (`GBI_F3D`,
//! `GBI_EXTENDED`, `GBI_RDP`) that are entirely outside this module's
//! decode-only scope.
//!
//! ```text
//! // src/gbi/rt64_display_list.h, lines 8-15
//! struct DisplayList {
//!     uint32_t w0;
//!     uint32_t w1;
//!
//!     DisplayList();
//!     uint32_t p0(uint8_t pos, uint8_t bits) const;
//!     uint32_t p1(uint8_t pos, uint8_t bits) const;
//! };
//!
//! // src/gbi/rt64_gbi.cpp, lines 32-38
//! uint32_t DisplayList::p0(uint8_t pos, uint8_t bits) const {
//!     return ((w0 >> pos) & ((0x01 << bits) - 1));
//! }
//!
//! uint32_t DisplayList::p1(uint8_t pos, uint8_t bits) const {
//!     return ((w1 >> pos) & ((0x01 << bits) - 1));
//! }
//!
//! // src/gbi/rt64_gbi_f3dex.cpp, lines 15-51
//! namespace RT64 {
//!     namespace GBI_F3DEX {
//!         void vertex(State *state, DisplayList **dl) {
//!             state->rsp->setVertex((*dl)->w1, (*dl)->p0(10, 6), (*dl)->p0(17, 7));
//!         }
//!
//!         void modifyVertex(State *state, DisplayList **dl) {
//!             state->rsp->modifyVertex((*dl)->p0(1, 15), (*dl)->p0(16, 8), (*dl)->w1);
//!         }
//!
//!         void tri1(State *state, DisplayList **dl) {
//!             state->rsp->drawIndexedTri((*dl)->p1(17, 7), (*dl)->p1(9, 7), (*dl)->p1(1, 7));
//!         }
//!
//!         void tri2(State *state, DisplayList **dl) {
//!             state->rsp->drawIndexedTri((*dl)->p0(17, 7), (*dl)->p0(9, 7), (*dl)->p0(1, 7));
//!             state->rsp->drawIndexedTri((*dl)->p1(17, 7), (*dl)->p1(9, 7), (*dl)->p1(1, 7));
//!         }
//!
//!         void quad(State *state, DisplayList **dl) {
//!             uint8_t a = (*dl)->p1(25, 7);
//!             uint8_t b = (*dl)->p1(17, 7);
//!             uint8_t c = (*dl)->p1(9, 7);
//!             uint8_t d = (*dl)->p1(1, 7);
//!             state->rsp->drawIndexedTri(a, b, c);
//!             state->rsp->drawIndexedTri(a, c, d);
//!         }
//!
//!         void cullDl(State *state, DisplayList **dl) {
//!             // TODO
//!         }
//!
//!         void branchZ(State *state, DisplayList **dl) {
//!             state->rsp->branchZ(state->microcode.half1, (*dl)->p0(1, 11), (*dl)->w1, dl);
//!         }
//!
//!         void loadUCode(State *state, DisplayList **dl) {
//!             state->ext.interpreter->loadUCodeGBI((*dl)->w1, state->microcode.half1, false);
//!         }
//!     }
//! };
//! ```
//!
//! **Reuse, not new type.** `crates/fn64-render-wgpu/src/` has no existing
//! GBI/display-list-command type (grepped for `struct.*Vertex`,
//! `struct.*Tri`, `DisplayList` across the crate: the only hits are
//! `raw_dpc`'s render-target triangle/vertex types, which are GPU draw-call
//! shapes, not display-list *decode* structs). `crates/fn64-render-reference/
//! src/gbi/` does have a full GBI implementation (`entries.rs`,
//! `stream.rs`, `geometry.rs`, ...), but it targets **F3DEX2**'s bit
//! layout, which differs from classic F3DEX's here -- e.g. F3DEX2's
//! `G_MODIFYVTX`/`G_BRANCH_Z` use a `v*5`/`v*2`-encoded cache-index scheme
//! (`stream.rs` around `G_MODIFYVTX`/`G_BRANCH_Z`), not the plain
//! `p0(1,15)`/`p0(1,11)` shift-mask fields this (F3DEX-generation) source
//! uses. Per the task's explicit instruction, this module is not wired to
//! `fn64-render-reference` regardless; the per-opcode structs below are
//! new, small, local types (`VertexArgs`, `ModifyVertexArgs`, `Tri1Args`,
//! `Tri2Args`, `QuadArgs`, `BranchZArgs`) sized exactly to each function's
//! operand tuple, with no attempt to unify them with F3DEX2's differently-
//! shaped decode.
//!
//! ## Admitted domain
//!
//! - **`p0`/`p1` are always unsigned, never sign-extend.** `((w >> pos) &
//!   ((0x01 << bits) - 1))` produces a `uint32_t` in the source; masking
//!   with `(1 << bits) - 1` yields a value in `0..2^bits`, with no sign
//!   bit ever considered. This is ported as `(w >> pos) & ((1u32 <<
//!   bits) - 1)` returning `u32` in every case, including `branchZ`'s
//!   `p0(1, 11)` -- the 11-bit field there is **not** sign-extended by
//!   `p0` itself; if the (unported) `state->rsp->branchZ` call site
//!   reinterprets it as signed, that happens after this module's scope
//!   ends. This module makes no claim about that reinterpretation.
//! - **Extraction source: `w0` vs `w1` is per-call, not per-opcode.**
//!   `p0` always reads `w0`, `p1` always reads `w1` -- both are const
//!   methods on the same `DisplayList`, so within a single opcode
//!   different operands can (and do, in `tri2`) mix `p0` and would-be
//!   `p1` calls at the *same* bit positions read from *different* words.
//!   `tri2` calls `p0(17,7)/p0(9,7)/p0(1,7)` for its first triangle and
//!   `p1(17,7)/p1(9,7)/p1(1,7)` for its second -- same three (pos, bits)
//!   pairs, but read from `w0` then `w1`. This module keeps that
//!   `w0`/`w1` split explicit in `decode_tri2`'s two calls to a shared
//!   `decode_tri_indices(word, ...)` helper, rather than merging w0/w1
//!   into one 64-bit value (the source never does that -- `w0` and `w1`
//!   stay two independent 32-bit fields throughout).
//! - **Shift width is always `u8` in the source (`pos`, `bits` are both
//!   `uint8_t`), and `bits` never exceeds 15 across every call site in
//!   this file** (`modifyVertex`'s `p0(1, 15)` is the widest). No call
//!   shifts `0x01` by 32 or more, so there is no risk of C++
//!   shift-amount UB (`0x01 << bits` past bit width) or of an all-ones
//!   `u32` mask overflowing when the port computes `(1u32 << bits) - 1`
//!   in Rust -- `bits <= 15` keeps `1u32 << bits` comfortably inside
//!   `u32` for every literal call in this file, and this module does not
//!   attempt to support a hypothetical `bits >= 32` caller since none
//!   exists upstream. The `p0`/`p1` primitive-level characterization
//!   tests below (not tied to any specific opcode) probe up to `bits =
//!   31`, not `32`: `1u32 << 32` is a shift-by-width-of-type overflow in
//!   Rust (a debug-build panic, matching C++'s `0x01 << 32` being shift
//!   UB for the same reason -- both languages consider shifting by the
//!   full bit width of the operand's type invalid), so `bits = 32` is
//!   outside the domain either language's `<<` operator actually
//!   supports, and this module does not claim behavior there.
//! - **`0x01` in C++ is `int` (signed), so `0x01 << bits` is signed-`int`
//!   left shift before the final `& (w >> pos)` promotes it back to
//!   `uint32_t` for the `&`.** On every real toolchain `int` is 32-bit
//!   two's complement and `bits <= 15` here, so `0x01 << bits` never sets
//!   the sign bit and the signed/unsigned distinction is invisible for
//!   this file's actual call sites -- this port uses `1u32 << bits`
//!   throughout (unsigned from the start), which is bit-for-bit identical
//!   to the C++ result at every `bits` value this file actually passes,
//!   and simpler to state as `u32`-only arithmetic. No case in this file
//!   exercises the (only theoretically different) signed-overflow edge.
//! - **`quad`'s `uint8_t a/b/c/d` truncation is a syntactic no-op at these
//!   widths, but is ported explicitly anyway.** Each of `p1(25,7)`,
//!   `p1(17,7)`, `p1(9,7)`, `p1(1,7)` returns a value in `0..128` (7-bit
//!   field), which fits in `uint8_t`/`u8` losslessly -- so the C++
//!   `uint8_t a = (*dl)->p1(25, 7);` narrowing conversion never actually
//!   discards bits for any input. This module still narrows explicitly
//!   (`as u8`) rather than leaving the decoded quad fields as `u32`, to
//!   preserve the source's declared operand *type*, not just its numeric
//!   *value* -- `decode_quad` returns a `QuadArgs` with `u8` fields,
//!   matching `uint8_t a, b, c, d`. `vertex`/`modifyVertex`/`tri1`/`tri2`/
//!   `branchZ`'s fields stay `u32` (or the field's exact source type),
//!   matching the source, which never narrows those.
//! - **`quad`'s `a = p1(25, 7)`: `25 + 7 == 32`, exactly the top of the
//!   32-bit word -- there are no bits above this field to mask away**,
//!   so an all-ones `w1` and a "one bit above the field" probe are the
//!   same test for `a` specifically (there is no 33rd bit). Every other
//!   field in this file (`vertex`'s `p0(17,7)` -> bits 17..24 of 32,
//!   `modifyVertex`'s `p0(16,8)` -> bits 16..24, etc.) does have bits
//!   above it, and those get an explicit "one bit above the field must
//!   not leak in" characterization test.
//! - **`vertex`'s two fields (`p0(10,6)` and `p0(17,7)`) and
//!   `modifyVertex`'s two fields (`p0(1,15)` and `p0(16,8)`) are
//!   adjacent-but-non-overlapping** (`10+6=16` vs `17`; `1+15=16` vs
//!   `16`) -- ported with no cross-field masking beyond each field's own
//!   `(pos, bits)`, exactly mirroring the source's independent `p0(...)`
//!   calls per operand.
//! - **`loadUCode`'s only `DisplayList`-sourced operand is `w1`** (raw,
//!   un-shifted, un-masked) -- `state->microcode.half1` is a `State`
//!   field, not decoded from `dl`, and is out of this module's scope
//!   (see "Nonclaims"). `decode_load_ucode` therefore returns just the
//!   raw `w1` word.
//! - **`cullDl` reads no bitfields at all** -- the upstream body is a bare
//!   `// TODO` comment with no statements. This module ports that fact
//!   literally: there is no `decode_cull_dl` function, and no invented
//!   operand layout. A `#[test]` asserting this file has no such decoder
//!   would be testing an absence, so instead the module doc and the
//!   opcode roster below state it explicitly.
//!
//! ## Nonclaims
//!
//! No GPU, WGSL, or production wiring (this module is not called from
//! anywhere yet -- dead-code warnings on the unused public surface are
//! expected and correct, matching `rt64_common.rs`'s and `rt64_math.rs`'s
//! precedent), and no RT64 visual/pixel/silicon parity or performance
//! claim. Not wired to `fn64-render-reference`'s GBI path (see "Reuse, not
//! new type" -- that crate's F3DEX2 decode has a different bit layout for
//! several of these opcodes and is a separate, independently-maintained
//! implementation, not a shared dependency of this port).
//!
//! Deliberately not ported from `rt64_gbi_f3dex.cpp`:
//!
//! - **All `state->rsp->*` and `state->ext.interpreter->*` dispatch
//!   calls** -- `setVertex`, `modifyVertex`, `drawIndexedTri`, `branchZ`,
//!   `loadUCodeGBI`. These need the full `RSP`/`State`/`Interpreter`
//!   object graph, which does not exist in this crate and is out of the
//!   task's named scope ("decode only, NOT dispatch").
//! - **`cullDl`** -- upstream is a `// TODO` stub with an empty body (no
//!   bitfield reads, no dispatch call). Ported as an explicit absence
//!   (see "Admitted domain"), not invented behavior.
//! - **`GBI_F3DEX::setup(GBI *gbi)`** -- the opcode-to-function-pointer
//!   dispatch table and the `gbi->constants` map (`G_MTX_MODELVIEW`,
//!   `G_TEXTURE_ENABLE`, etc.). This wires together functions from other
//!   GBI translation units (`GBI_F3D`, `GBI_EXTENDED`, `GBI_RDP`) that are
//!   entirely outside this file's own opcode functions, and is
//!   dispatch/wiring, not bitfield decode.
//! - **`DisplayList`'s constructor and its `w0`/`w1` fields as a stateful
//!   struct** -- this module represents a display-list command word as a
//!   plain `(w0: u32, w1: u32)` parameter pair to each `decode_*`
//!   function, not as an owned struct with methods, since nothing here
//!   needs `DisplayList`'s identity or its other (unported) fields/
//!   methods -- `p0`/`p1` are ported as free functions over `(w0, w1)`,
//!   per the task's explicit instruction.

/// `DisplayList::p0(pos, bits)`: extracts `bits` bits from `w0` starting at
/// bit `pos`. Always unsigned, never sign-extends (see module doc).
fn p0(w0: u32, pos: u8, bits: u8) -> u32 {
    (w0 >> pos) & ((1u32 << bits) - 1)
}

/// `DisplayList::p1(pos, bits)`: extracts `bits` bits from `w1` starting at
/// bit `pos`. Always unsigned, never sign-extends (see module doc).
fn p1(w1: u32, pos: u8, bits: u8) -> u32 {
    (w1 >> pos) & ((1u32 << bits) - 1)
}

/// `GBI_F3DEX::vertex`'s operand set: `state->rsp->setVertex((*dl)->w1,
/// (*dl)->p0(10, 6), (*dl)->p0(17, 7))`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct VertexArgs {
    /// Raw `w1` (source RDRAM address, per the source's naming convention).
    addr: u32,
    /// `p0(10, 6)`.
    field_10_6: u32,
    /// `p0(17, 7)`.
    field_17_7: u32,
}

fn decode_vertex(w0: u32, w1: u32) -> VertexArgs {
    VertexArgs {
        addr: w1,
        field_10_6: p0(w0, 10, 6),
        field_17_7: p0(w0, 17, 7),
    }
}

/// `GBI_F3DEX::modifyVertex`'s operand set: `state->rsp->modifyVertex(
/// (*dl)->p0(1, 15), (*dl)->p0(16, 8), (*dl)->w1)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ModifyVertexArgs {
    /// `p0(1, 15)`.
    field_1_15: u32,
    /// `p0(16, 8)`.
    field_16_8: u32,
    /// Raw `w1` (new value, per the source's argument order).
    value: u32,
}

fn decode_modify_vertex(w0: u32, w1: u32) -> ModifyVertexArgs {
    ModifyVertexArgs {
        field_1_15: p0(w0, 1, 15),
        field_16_8: p0(w0, 16, 8),
        value: w1,
    }
}

/// One `drawIndexedTri(a, b, c)` triangle's three vertex-cache indices, each
/// a `p0`/`p1`-extracted 7-bit field.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct TriIndices {
    a: u32,
    b: u32,
    c: u32,
}

/// Shared (pos, bits) shape used by `tri1`'s single triangle, both of
/// `tri2`'s triangles, and `quad`'s two derived triangles: fields at bit
/// 17, 9, and 1, each 7 bits wide.
fn decode_tri_indices(word: u32) -> TriIndices {
    TriIndices {
        a: (word >> 17) & ((1u32 << 7) - 1),
        b: (word >> 9) & ((1u32 << 7) - 1),
        c: (word >> 1) & ((1u32 << 7) - 1),
    }
}

/// `GBI_F3DEX::tri1`'s operand set: `state->rsp->drawIndexedTri(
/// (*dl)->p1(17, 7), (*dl)->p1(9, 7), (*dl)->p1(1, 7))`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Tri1Args {
    tri: TriIndices,
}

fn decode_tri1(_w0: u32, w1: u32) -> Tri1Args {
    Tri1Args {
        tri: decode_tri_indices(w1),
    }
}

/// `GBI_F3DEX::tri2`'s operand set: two `drawIndexedTri` calls, the first
/// from `w0`'s `p0(17,7)/p0(9,7)/p0(1,7)`, the second from `w1`'s
/// `p1(17,7)/p1(9,7)/p1(1,7)` -- same (pos, bits) triple, different words
/// (see module doc "Admitted domain").
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Tri2Args {
    first: TriIndices,
    second: TriIndices,
}

fn decode_tri2(w0: u32, w1: u32) -> Tri2Args {
    Tri2Args {
        first: decode_tri_indices(w0),
        second: decode_tri_indices(w1),
    }
}

/// `GBI_F3DEX::quad`'s operand set: `uint8_t a/b/c/d` from `w1`'s
/// `p1(25,7)/p1(17,7)/p1(9,7)/p1(1,7)`, narrowed to `u8` exactly as the
/// source narrows to `uint8_t` (see module doc "Admitted domain" -- a
/// syntactic no-op at these widths, ported explicitly anyway to preserve
/// the source's declared operand type).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct QuadArgs {
    a: u8,
    b: u8,
    c: u8,
    d: u8,
}

fn decode_quad(_w0: u32, w1: u32) -> QuadArgs {
    QuadArgs {
        a: p1(w1, 25, 7) as u8,
        b: p1(w1, 17, 7) as u8,
        c: p1(w1, 9, 7) as u8,
        d: p1(w1, 1, 7) as u8,
    }
}

/// `GBI_F3DEX::branchZ`'s `DisplayList`-sourced operand set:
/// `state->rsp->branchZ(state->microcode.half1, (*dl)->p0(1, 11),
/// (*dl)->w1, dl)`. `state->microcode.half1` and `dl` (the `DisplayList
/// **` pointer itself) are not `DisplayList` bitfields and are out of
/// scope (see "Nonclaims").
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct BranchZArgs {
    /// `p0(1, 11)`. Unsigned per `p0`'s definition -- see module doc for
    /// why this is not sign-extended by this module.
    field_1_11: u32,
    /// Raw `w1` (depth-compare value, per the source's argument order).
    depth: u32,
}

fn decode_branch_z(w0: u32, w1: u32) -> BranchZArgs {
    BranchZArgs {
        field_1_11: p0(w0, 1, 11),
        depth: w1,
    }
}

/// `GBI_F3DEX::loadUCode`'s only `DisplayList`-sourced operand:
/// `state->ext.interpreter->loadUCodeGBI((*dl)->w1, state->microcode.half1,
/// false)`. `state->microcode.half1` and the literal `false` are not
/// `DisplayList` bitfields and are out of scope (see "Nonclaims").
fn decode_load_ucode(_w0: u32, w1: u32) -> u32 {
    w1
}

// `cullDl` has no decoder: the upstream function body is a bare `// TODO`
// with no bitfield reads (see module doc "Admitted domain" and
// "Nonclaims"). No `decode_cull_dl` exists in this module.

#[cfg(test)]
mod tests {
    use super::*;

    // --- p0 / p1: shared bitfield extraction primitives ---

    #[test]
    fn p0_all_zero_word_is_zero() {
        // bits=31, not 32: this file's widest real call site is bits=15
        // (modifyVertex's p0(1,15)); bits=32 would require `1u32 << 32`,
        // which is a shift-by-width overflow no call site in the source
        // ever exercises (see module doc "Admitted domain" -- bits<=15).
        assert_eq!(p0(0, 0, 31), 0);
    }

    #[test]
    fn p0_reads_low_bit_at_pos_zero() {
        assert_eq!(p0(0b1, 0, 1), 1);
    }

    #[test]
    fn p0_ignores_w1_entirely() {
        // p0 must read only w0, never w1 -- an all-ones w0 word, masked to
        // 31 bits (see p0_all_zero_word_is_zero for why not 32).
        assert_eq!(p0(0xFFFF_FFFF, 0, 31), 0x7FFF_FFFF);
    }

    #[test]
    fn p0_masks_off_bits_above_the_field() {
        // pos=0, bits=4: value 0xFF has bits 4..8 set above the field --
        // those must be masked away, leaving only 0xF.
        assert_eq!(p0(0xFF, 0, 4), 0xF);
    }

    #[test]
    fn p0_extracts_field_at_nonzero_shift() {
        // bits 8..12 of 0x0000_0F00 = 0xF.
        assert_eq!(p0(0x0000_0F00, 8, 4), 0xF);
    }

    #[test]
    fn p0_field_boundary_max_value_that_fits() {
        // 7-bit field, all ones: max value 0x7F, positioned at bit 1.
        let w0 = 0x7F << 1;
        assert_eq!(p0(w0, 1, 7), 0x7F);
    }

    #[test]
    fn p0_field_boundary_one_bit_above_does_not_leak_in() {
        // Same as above, plus bit 8 (one above the 7-bit field ending at
        // bit 7) set -- p0(1,7) must still read exactly 0x7F.
        let w0 = (0x7F << 1) | (1 << 8);
        assert_eq!(p0(w0, 1, 7), 0x7F);
    }

    #[test]
    fn p0_all_ones_word_widest_field_in_file() {
        // Widest (pos, bits) used anywhere in this file is (1, 15)
        // (modifyVertex). All-ones input must yield the full 15-bit mask.
        assert_eq!(p0(0xFFFF_FFFF, 1, 15), 0x7FFF);
    }

    #[test]
    fn p1_all_zero_word_is_zero() {
        // bits=31, not 32 -- see p0_all_zero_word_is_zero.
        assert_eq!(p1(0, 0, 31), 0);
    }

    #[test]
    fn p1_reads_w1_not_w0() {
        assert_eq!(p1(0b1010, 1, 3), 0b101);
    }

    #[test]
    fn p1_masks_off_bits_above_the_field() {
        assert_eq!(p1(0xFF, 0, 4), 0xF);
    }

    #[test]
    fn p1_field_boundary_max_value_that_fits() {
        let w1 = 0x7F << 9;
        assert_eq!(p1(w1, 9, 7), 0x7F);
    }

    #[test]
    fn p1_field_boundary_one_bit_above_does_not_leak_in() {
        let w1 = (0x7F << 9) | (1 << 16);
        assert_eq!(p1(w1, 9, 7), 0x7F);
    }

    // --- vertex ---

    #[test]
    fn vertex_all_zero_words_decode_to_all_zero_fields() {
        let a = decode_vertex(0, 0);
        assert_eq!(
            a,
            VertexArgs {
                addr: 0,
                field_10_6: 0,
                field_17_7: 0
            }
        );
    }

    #[test]
    fn vertex_w1_passes_through_raw_unmasked() {
        let a = decode_vertex(0, 0xDEAD_BEEF);
        assert_eq!(a.addr, 0xDEAD_BEEF);
    }

    #[test]
    fn vertex_field_10_6_max_value_boundary() {
        // 6-bit field at bit 10: max value 0x3F.
        let w0 = 0x3F << 10;
        let a = decode_vertex(w0, 0);
        assert_eq!(a.field_10_6, 0x3F);
        assert_eq!(a.field_17_7, 0);
    }

    #[test]
    fn vertex_field_17_7_max_value_boundary() {
        // 7-bit field at bit 17: max value 0x7F.
        let w0 = 0x7F << 17;
        let a = decode_vertex(w0, 0);
        assert_eq!(a.field_17_7, 0x7F);
        assert_eq!(a.field_10_6, 0);
    }

    #[test]
    fn vertex_adjacent_fields_do_not_bleed_into_each_other() {
        // field_10_6 occupies bits 10..16, field_17_7 occupies bits
        // 17..24 -- bit 16 belongs to neither field and must not appear
        // in either decoded value.
        let w0 = 1 << 16;
        let a = decode_vertex(w0, 0);
        assert_eq!(a.field_10_6, 0);
        assert_eq!(a.field_17_7, 0);
    }

    #[test]
    fn vertex_all_ones_w0_saturates_both_fields() {
        let a = decode_vertex(0xFFFF_FFFF, 0xFFFF_FFFF);
        assert_eq!(a.field_10_6, 0x3F);
        assert_eq!(a.field_17_7, 0x7F);
        assert_eq!(a.addr, 0xFFFF_FFFF);
    }

    // --- modifyVertex ---

    #[test]
    fn modify_vertex_all_zero_words_decode_to_all_zero_fields() {
        let a = decode_modify_vertex(0, 0);
        assert_eq!(
            a,
            ModifyVertexArgs {
                field_1_15: 0,
                field_16_8: 0,
                value: 0
            }
        );
    }

    #[test]
    fn modify_vertex_w1_passes_through_raw_as_value() {
        let a = decode_modify_vertex(0, 0x1234_5678);
        assert_eq!(a.value, 0x1234_5678);
    }

    #[test]
    fn modify_vertex_field_1_15_max_value_boundary() {
        // 15-bit field at bit 1: max value 0x7FFF.
        let w0 = 0x7FFF << 1;
        let a = decode_modify_vertex(w0, 0);
        assert_eq!(a.field_1_15, 0x7FFF);
        assert_eq!(a.field_16_8, 0);
    }

    #[test]
    fn modify_vertex_field_1_15_one_bit_above_does_not_leak_in() {
        // Bit 16 belongs to field_16_8, not field_1_15.
        let w0 = (0x7FFF << 1) | (1 << 16);
        let a = decode_modify_vertex(w0, 0);
        assert_eq!(a.field_1_15, 0x7FFF);
    }

    #[test]
    fn modify_vertex_field_16_8_max_value_boundary() {
        // 8-bit field at bit 16: max value 0xFF.
        let w0 = 0xFFu32 << 16;
        let a = decode_modify_vertex(w0, 0);
        assert_eq!(a.field_16_8, 0xFF);
        assert_eq!(a.field_1_15, 0);
    }

    #[test]
    fn modify_vertex_field_16_8_one_bit_above_does_not_leak_in() {
        // Bit 24 is one above field_16_8's top (bits 16..24).
        let w0 = (0xFFu32 << 16) | (1 << 24);
        let a = decode_modify_vertex(w0, 0);
        assert_eq!(a.field_16_8, 0xFF);
    }

    #[test]
    fn modify_vertex_bit_zero_belongs_to_neither_field() {
        // field_1_15 starts at bit 1, so bit 0 must not appear in it.
        let a = decode_modify_vertex(1, 0);
        assert_eq!(a.field_1_15, 0);
    }

    #[test]
    fn modify_vertex_all_ones_saturates_both_fields_and_value() {
        let a = decode_modify_vertex(0xFFFF_FFFF, 0xFFFF_FFFF);
        assert_eq!(a.field_1_15, 0x7FFF);
        assert_eq!(a.field_16_8, 0xFF);
        assert_eq!(a.value, 0xFFFF_FFFF);
    }

    // --- tri1 ---

    #[test]
    fn tri1_all_zero_w1_decodes_to_all_zero_indices() {
        let a = decode_tri1(0xFFFF_FFFF, 0);
        assert_eq!(a.tri, TriIndices { a: 0, b: 0, c: 0 });
    }

    #[test]
    fn tri1_ignores_w0_entirely() {
        // w0 must have zero effect on tri1's decode.
        let with_w0 = decode_tri1(0xFFFF_FFFF, 0x1234_5678);
        let without_w0 = decode_tri1(0, 0x1234_5678);
        assert_eq!(with_w0, without_w0);
    }

    #[test]
    fn tri1_field_a_max_value_boundary() {
        // a = p1(17, 7): max value 0x7F at bit 17.
        let w1 = 0x7Fu32 << 17;
        let a = decode_tri1(0, w1);
        assert_eq!(a.tri.a, 0x7F);
        assert_eq!(a.tri.b, 0);
        assert_eq!(a.tri.c, 0);
    }

    #[test]
    fn tri1_field_b_max_value_boundary() {
        // b = p1(9, 7): max value 0x7F at bit 9.
        let w1 = 0x7Fu32 << 9;
        let a = decode_tri1(0, w1);
        assert_eq!(a.tri.b, 0x7F);
        assert_eq!(a.tri.a, 0);
        assert_eq!(a.tri.c, 0);
    }

    #[test]
    fn tri1_field_c_max_value_boundary() {
        // c = p1(1, 7): max value 0x7F at bit 1.
        let w1 = 0x7Fu32 << 1;
        let a = decode_tri1(0, w1);
        assert_eq!(a.tri.c, 0x7F);
        assert_eq!(a.tri.a, 0);
        assert_eq!(a.tri.b, 0);
    }

    #[test]
    fn tri1_all_ones_w1_saturates_all_three_indices() {
        let a = decode_tri1(0, 0xFFFF_FFFF);
        assert_eq!(
            a.tri,
            TriIndices {
                a: 0x7F,
                b: 0x7F,
                c: 0x7F
            }
        );
    }

    #[test]
    fn tri1_bit_zero_belongs_to_no_field() {
        // c starts at bit 1, so bit 0 alone must decode to all-zero.
        let a = decode_tri1(0, 1);
        assert_eq!(a.tri, TriIndices { a: 0, b: 0, c: 0 });
    }

    #[test]
    fn tri1_bit_8_belongs_to_no_field() {
        // Between c's top (bit 8) and b's bottom (bit 9): a gap bit.
        let a = decode_tri1(0, 1 << 8);
        assert_eq!(a.tri, TriIndices { a: 0, b: 0, c: 0 });
    }

    #[test]
    fn tri1_bit_16_belongs_to_no_field() {
        // Between b's top (bit 16) and a's bottom (bit 17): a gap bit.
        let a = decode_tri1(0, 1 << 16);
        assert_eq!(a.tri, TriIndices { a: 0, b: 0, c: 0 });
    }

    // --- tri2 ---

    #[test]
    fn tri2_all_zero_words_decode_to_all_zero_triangles() {
        let a = decode_tri2(0, 0);
        assert_eq!(
            a,
            Tri2Args {
                first: TriIndices { a: 0, b: 0, c: 0 },
                second: TriIndices { a: 0, b: 0, c: 0 },
            }
        );
    }

    #[test]
    fn tri2_first_triangle_reads_w0_second_reads_w1_independently() {
        // Same (pos, bits) shape on both words, but distinct values --
        // must not cross-contaminate.
        let w0 = 0x7Fu32 << 17; // first.a = 0x7F, rest 0.
        let w1 = 0x7Fu32 << 1; // second.c = 0x7F, rest 0.
        let a = decode_tri2(w0, w1);
        assert_eq!(
            a.first,
            TriIndices {
                a: 0x7F,
                b: 0,
                c: 0
            }
        );
        assert_eq!(
            a.second,
            TriIndices {
                a: 0,
                b: 0,
                c: 0x7F
            }
        );
    }

    #[test]
    fn tri2_matches_two_independent_tri1_decodes_on_w0_and_w1() {
        // tri2's field layout is identical to tri1's, just applied twice
        // (once per word) -- decode_tri_indices(w0) == what tri1 would
        // produce if it read w0 instead of w1.
        let w0 = 0x1234_5678;
        let w1 = 0x9ABC_DEF0;
        let combined = decode_tri2(w0, w1);
        assert_eq!(combined.first, decode_tri_indices(w0));
        assert_eq!(combined.second, decode_tri_indices(w1));
    }

    #[test]
    fn tri2_all_ones_both_words_saturates_every_index() {
        let a = decode_tri2(0xFFFF_FFFF, 0xFFFF_FFFF);
        let full = TriIndices {
            a: 0x7F,
            b: 0x7F,
            c: 0x7F,
        };
        assert_eq!(a.first, full);
        assert_eq!(a.second, full);
    }

    // --- quad ---

    #[test]
    fn quad_all_zero_w1_decodes_to_all_zero_indices() {
        let a = decode_quad(0xFFFF_FFFF, 0);
        assert_eq!(
            a,
            QuadArgs {
                a: 0,
                b: 0,
                c: 0,
                d: 0
            }
        );
    }

    #[test]
    fn quad_ignores_w0_entirely() {
        let with_w0 = decode_quad(0xFFFF_FFFF, 0x1234_5678);
        let without_w0 = decode_quad(0, 0x1234_5678);
        assert_eq!(with_w0, without_w0);
    }

    #[test]
    fn quad_field_a_occupies_the_top_seven_bits_no_bits_above_to_mask() {
        // a = p1(25, 7): 25 + 7 = 32, exactly the word's top -- all-ones w1
        // and "one bit above" are the same probe here (see module doc).
        let w1 = 0x7Fu32 << 25;
        let a = decode_quad(0, w1);
        assert_eq!(a.a, 0x7F);
        assert_eq!(a.b, 0);
        assert_eq!(a.c, 0);
        assert_eq!(a.d, 0);
    }

    #[test]
    fn quad_field_b_max_value_boundary() {
        let w1 = 0x7Fu32 << 17;
        let a = decode_quad(0, w1);
        assert_eq!(a.b, 0x7F);
        assert_eq!(a.a, 0);
        assert_eq!(a.c, 0);
        assert_eq!(a.d, 0);
    }

    #[test]
    fn quad_field_c_max_value_boundary() {
        let w1 = 0x7Fu32 << 9;
        let a = decode_quad(0, w1);
        assert_eq!(a.c, 0x7F);
        assert_eq!(a.a, 0);
        assert_eq!(a.b, 0);
        assert_eq!(a.d, 0);
    }

    #[test]
    fn quad_field_d_max_value_boundary() {
        let w1 = 0x7Fu32 << 1;
        let a = decode_quad(0, w1);
        assert_eq!(a.d, 0x7F);
        assert_eq!(a.a, 0);
        assert_eq!(a.b, 0);
        assert_eq!(a.c, 0);
    }

    #[test]
    fn quad_field_b_one_bit_above_does_not_leak_in() {
        // Bit 24 is one above b's top (bits 17..24).
        let w1 = (0x7Fu32 << 17) | (1 << 24);
        let a = decode_quad(0, w1);
        assert_eq!(a.b, 0x7F);
    }

    #[test]
    fn quad_all_ones_w1_saturates_all_four_fields_as_u8() {
        let a = decode_quad(0, 0xFFFF_FFFF);
        assert_eq!(
            a,
            QuadArgs {
                a: 0x7F,
                b: 0x7F,
                c: 0x7F,
                d: 0x7F
            }
        );
    }

    #[test]
    fn quad_fields_are_narrowed_to_u8_type_matching_source_uint8_t() {
        // Compile-time-shaped assertion: field type is u8, not u32 -- a
        // value that would need more than 8 bits (impossible for a 7-bit
        // field, but checked structurally) is not representable.
        let a = decode_quad(0, 0xFFFF_FFFF);
        let _: u8 = a.a;
        let _: u8 = a.b;
        let _: u8 = a.c;
        let _: u8 = a.d;
    }

    // --- branchZ ---

    #[test]
    fn branch_z_all_zero_words_decode_to_all_zero_fields() {
        let a = decode_branch_z(0, 0);
        assert_eq!(
            a,
            BranchZArgs {
                field_1_11: 0,
                depth: 0
            }
        );
    }

    #[test]
    fn branch_z_w1_passes_through_raw_as_depth() {
        let a = decode_branch_z(0, 0xCAFE_BABE);
        assert_eq!(a.depth, 0xCAFE_BABE);
    }

    #[test]
    fn branch_z_field_1_11_max_value_boundary() {
        // 11-bit field at bit 1: max value 0x7FF.
        let w0 = 0x7FFu32 << 1;
        let a = decode_branch_z(w0, 0);
        assert_eq!(a.field_1_11, 0x7FF);
    }

    #[test]
    fn branch_z_field_1_11_one_bit_above_does_not_leak_in() {
        // Bit 12 is one above the field's top (bits 1..12).
        let w0 = (0x7FFu32 << 1) | (1 << 12);
        let a = decode_branch_z(w0, 0);
        assert_eq!(a.field_1_11, 0x7FF);
    }

    #[test]
    fn branch_z_bit_zero_belongs_to_no_field() {
        // Field starts at bit 1, so bit 0 alone must decode to zero.
        let a = decode_branch_z(1, 0);
        assert_eq!(a.field_1_11, 0);
    }

    #[test]
    fn branch_z_all_ones_saturates_field_and_depth() {
        let a = decode_branch_z(0xFFFF_FFFF, 0xFFFF_FFFF);
        assert_eq!(a.field_1_11, 0x7FF);
        assert_eq!(a.depth, 0xFFFF_FFFF);
    }

    #[test]
    fn branch_z_field_is_never_sign_extended_even_with_top_bit_of_field_set() {
        // Field's own top bit (bit 11, the 11th bit of the 11-bit field,
        // i.e. bit index 11 counting from bit 1..12 -- concretely bit 11
        // of w0) set alone must NOT produce a value that looks
        // sign-extended into the u32's upper bits: p0 always masks to
        // exactly `bits` width and returns an unsigned u32.
        let w0 = 1u32 << 11; // top bit of the 11-bit field at pos=1.
        let a = decode_branch_z(w0, 0);
        assert_eq!(a.field_1_11, 1 << 10); // relative bit 10 within the field.
        assert!(a.field_1_11 <= 0x7FF, "must stay within the 11-bit range");
    }

    // --- loadUCode ---

    #[test]
    fn load_ucode_returns_w1_raw() {
        assert_eq!(decode_load_ucode(0, 0x0BAD_F00D), 0x0BAD_F00D);
    }

    #[test]
    fn load_ucode_ignores_w0_entirely() {
        let with_w0 = decode_load_ucode(0xFFFF_FFFF, 0x1234_5678);
        let without_w0 = decode_load_ucode(0, 0x1234_5678);
        assert_eq!(with_w0, without_w0);
    }

    #[test]
    fn load_ucode_all_zero_words_is_zero() {
        assert_eq!(decode_load_ucode(0xFFFF_FFFF, 0), 0);
    }

    #[test]
    fn load_ucode_all_ones_w1_passes_through_unmasked() {
        assert_eq!(decode_load_ucode(0, 0xFFFF_FFFF), 0xFFFF_FFFF);
    }
}
