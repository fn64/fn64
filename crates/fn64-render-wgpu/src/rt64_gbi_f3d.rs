//! Literal port of RT64's F3D (base microcode) display-list command-word
//! bitfield *decoding*, a literal port of the permitted MIT RT64 Rust-port
//! source pinned at commit `5473732a822a4423b5696e7cb18fecc425a59875`
//! (`docs/RT64-PORT-AUTHORITY.md`), `src/gbi/rt64_gbi_f3d.cpp` (SHA-256 of
//! the whole file, `db0319bfa8c53c12119a5ae292fe735c97c2ff06076451f77e6f6f8e2a327825`):
//!
//! ```text
//! // src/gbi/rt64_display_list.h (struct + method declarations)
//! struct DisplayList {
//!     uint32_t w0;
//!     uint32_t w1;
//!
//!     DisplayList();
//!     uint32_t p0(uint8_t pos, uint8_t bits) const;
//!     uint32_t p1(uint8_t pos, uint8_t bits) const;
//! };
//!
//! // src/gbi/rt64_gbi.cpp:32-38 (extractor definitions)
//! uint32_t DisplayList::p0(uint8_t pos, uint8_t bits) const {
//!     return ((w0 >> pos) & ((0x01 << bits) - 1));
//! }
//!
//! uint32_t DisplayList::p1(uint8_t pos, uint8_t bits) const {
//!     return ((w1 >> pos) & ((0x01 << bits) - 1));
//! }
//!
//! // src/gbi/rt64_gbi_f3d.cpp (this port's source, in full -- every opcode
//! // function's bitfield-relevant lines; see "Nonclaims" for the
//! // state->rsp/state dispatch lines this port deliberately omits)
//! void matrix(State *state, DisplayList **dl) {
//!     state->rsp->matrix((*dl)->w1, (*dl)->p0(16, 8));
//! }
//!
//! void popMatrix(State *state, DisplayList **dl) {
//!     if ((*dl)->w1 == 0) {
//!         state->rsp->popMatrix(1);
//!     }
//! }
//!
//! void moveMem(State *state, DisplayList **dl) {
//!     switch ((*dl)->p0(16, 8)) {
//!     case F3D_G_MV_VIEWPORT: /* w1 */ break;
//!     case F3D_G_MV_MATRIX_1: /* w1, *dl = *dl + 3 */ break;
//!     case F3D_G_MV_L0..F3D_G_MV_L7: /* w1 */ break;
//!     case F3D_G_MV_LOOKATX: case F3D_G_MV_LOOKATY: /* w1 */ break;
//!     default: assert(false && "Unimplemented move mem."); break;
//!     }
//! }
//!
//! void vertex(State *state, DisplayList **dl) {
//!     state->rsp->setVertex((*dl)->w1, (*dl)->p0(20, 4) + 1, (*dl)->p0(16, 4));
//! }
//!
//! void runDl(State *state, DisplayList **dl) {
//!     if ((*dl)->p0(16, 1) == 0) {
//!         state->pushReturnAddress(*dl);
//!     }
//!     // ... state->rsp->fromSegmentedMasked((*dl)->w1) and RDRAM jump (out of scope)
//! }
//!
//! void endDl(State *state, DisplayList **dl) {
//!     *dl = state->popReturnAddress();
//! }
//!
//! void sprite2DBase(State *state, DisplayList **dl) {
//!     // TODO
//! }
//!
//! void tri1(State *state, DisplayList **dl) {
//!     state->rsp->drawIndexedTri((*dl)->p1(16, 8) / 10, (*dl)->p1(8, 8) / 10, (*dl)->p1(0, 8) / 10);
//! }
//!
//! void quad(State *state, DisplayList **dl) {
//!     const uint8_t v0 = (*dl)->p1(24, 8) / 10;
//!     const uint8_t v1 = (*dl)->p1(16, 8) / 10;
//!     const uint8_t v2 = (*dl)->p1(8, 8) / 10;
//!     const uint8_t v3 = (*dl)->p1(0, 8) / 10;
//!     state->rsp->drawIndexedTri(v0, v1, v2);
//!     state->rsp->drawIndexedTri(v0, v2, v3);
//! }
//!
//! void cullDl(State *state, DisplayList **dl) {
//!     // TODO
//! }
//!
//! void moveWord(State *state, DisplayList **dl) {
//!     uint8_t type = (*dl)->p0(0, 8);
//!     switch (type) {
//!     case G_MW_MATRIX:
//!         assert(false);
//!         // TODO
//!         break;
//!     case G_MW_NUMLIGHT:
//!         state->rsp->setLightCount((((*dl)->w1 - 0x80000000) >> 5) - 1);
//!         break;
//!     case G_MW_CLIP:
//!         state->rsp->setClipRatioEdge(((*dl)->p0(8, 16) - G_MWO_CLIP_RNX) / 8, int16_t((*dl)->w1 & 0xFFFFU));
//!         break;
//!     case G_MW_SEGMENT:
//!         state->rsp->setSegment((*dl)->p0(10, 4), (*dl)->w1);
//!         break;
//!     case G_MW_FOG:
//!         state->rsp->setFog((int16_t)((*dl)->p1(16, 16)), (int16_t)((*dl)->p1(0, 16)));
//!         break;
//!     case G_MW_LIGHTCOL:
//!         state->rsp->setLightColor((*dl)->p0(8, 16) / 32, (*dl)->w1);
//!         break;
//!     case F3D_G_MW_POINTS:
//!         state->rsp->modifyVertex((*dl)->p0(8, 16) / 40, (*dl)->p0(8, 16) % 40, (*dl)->w1);
//!         break;
//!     case G_MW_PERSPNORM:
//!         // TODO
//!         break;
//!     default:
//!         break;
//!     }
//! }
//!
//! void texture(State *state, DisplayList **dl) {
//!     uint8_t tile = (*dl)->p0(8, 3);
//!     uint8_t level = (*dl)->p0(11, 3);
//!     uint8_t on = (*dl)->p0(0, 8);
//!     uint16_t sc = (*dl)->p1(16, 16);
//!     uint16_t tc = (*dl)->p1(0, 16);
//!     state->rsp->setTexture(tile, level, on, sc, tc);
//! }
//!
//! void setOtherModeH(State *state, DisplayList **dl) {
//!     state->rsp->setOtherModeH((*dl)->p0(0, 8), (*dl)->p0(8, 8), (*dl)->w1);
//! }
//!
//! void setOtherModeL(State *state, DisplayList **dl) {
//!     state->rsp->setOtherModeL((*dl)->p0(0, 8), (*dl)->p0(8, 8), (*dl)->w1);
//! }
//!
//! void setGeometryMode(State *state, DisplayList **dl) {
//!     state->rsp->setGeometryMode((*dl)->w1);
//! }
//!
//! void clearGeometryMode(State *state, DisplayList **dl) {
//!     state->rsp->clearGeometryMode((*dl)->w1);
//! }
//!
//! void rdpHalf1(State *state, DisplayList **dl) {
//!     state->microcode.half1 = (*dl)->w1;
//! }
//!
//! void rdpHalf2(State *state, DisplayList **dl) {
//!     state->microcode.half2 = (*dl)->w1;
//! }
//!
//! void setColorImage(State *state, DisplayList **dl) {
//!     const uint8_t fmt = (*dl)->p0(21, 3);
//!     const uint8_t siz = (*dl)->p0(19, 2);
//!     const uint16_t width = (*dl)->p0(0, 12) + 1;
//!     const uint32_t address = (*dl)->w1;
//!     state->rsp->setColorImage(fmt, siz, width, address);
//! }
//!
//! void setDepthImage(State *state, DisplayList **dl) {
//!     const uint32_t address = (*dl)->w1;
//!     state->rsp->setDepthImage(address);
//! }
//!
//! void setTextureImage(State *state, DisplayList **dl) {
//!     const uint8_t fmt = (*dl)->p0(21, 3);
//!     const uint8_t siz = (*dl)->p0(19, 2);
//!     const uint16_t width = (*dl)->p0(0, 12) + 1;
//!     const uint32_t address = (*dl)->w1;
//!     state->rsp->setTextureImage(fmt, siz, width, address);
//! }
//! ```
//!
//! **Reuse, not new type.** `crates/fn64-render-reference/src/gbi/` has a
//! full GBI implementation, but its `SUPPORTED` ucode list
//! (`geometry.rs:744`) is `&[UcodeId::F3dex2]` only -- it never decodes F3D
//! base-microcode command words at all, so there is no existing type whose
//! bitfield layout this port could reuse. F3D's `moveWord`/`texture`/
//! `setColorImage` field widths and shift positions also differ from
//! F3DEX2's (e.g. F3D's `texture` opcode packs `on` at `p0(0,8)` with no
//! separate `w1`-only path F3DEX2 uses for some fields), so even a same-
//! named decode elsewhere in `fn64-render-reference` would not be a literal
//! match. This module defines its own small typed-struct-per-opcode
//! decode, matching `rt64_common.rs`'s and `rt64_math.rs`'s established
//! precedent of a fresh, unwired, characterization-first module per source
//! file rather than wiring into the existing GBI interpreter.
//!
//! ## Admitted domain
//!
//! - **`p0`/`p1` are pure bit-twiddles with no sign extension of their
//!   own.** `((w >> pos) & ((1 << bits) - 1))` always yields an unsigned
//!   value truncated to `bits` width; C++'s `0x01 << bits` is `int` (32-bit
//!   signed) arithmetic promoted before the subtract-one, but for every
//!   `bits` value this file actually uses (1, 2, 3, 4, 8, 12, 16) the shift
//!   result is always positive and fits comfortably in `i32`/`u32`, so
//!   there is no overflow/UB to characterize. Ported as `fn p0(w0: u32,
//!   pos: u8, bits: u8) -> u32 { (w0 >> pos) & ((1u32 << bits) - 1) }` (and
//!   `p1` identically over `w1`) -- literal, not idiomatized (no
//!   `u32::MASK`/`bit_field` helpers).
//! - **`w0` vs `w1`**: `p0` always reads the *first* command word (opcode +
//!   packed small fields), `p1` always reads the *second* (usually a raw
//!   address, a signed 16.16 pair, or two 16-bit halves). This port keeps
//!   that split explicit: every decode function takes `(w0: u32, w1: u32)`
//!   and calls `p0`/`p1` exactly where the C++ does, never swapping which
//!   word backs which field.
//! - **Sign extension is opcode-specific, not extractor-specific.** `p0`/
//!   `p1` themselves return `uint32_t` (zero-extended within the field
//!   width); the *caller* decides whether a field represents a signed
//!   quantity and applies a C-style narrowing cast. This file has three
//!   such sites, all inside `moveWord`'s `G_MW_CLIP`/`G_MW_FOG` arms:
//!   - `G_MW_CLIP`: `int16_t((*dl)->w1 & 0xFFFFU)` -- mask to the low 16
//!     bits of the *raw* `w1` (not through `p1`), then reinterpret those 16
//!     bits as signed. Ported as `((w1 & 0xFFFF) as u16) as i16`: masking
//!     to `u16` first (truncation), then a bit-preserving `as i16`
//!     reinterpret (matching C++'s `int16_t(unsigned_value)`, which is
//!     implementation-defined pre-C++20 and standardized 2's-complement
//!     reinterpret from C++20 onward -- the only behavior real toolchains
//!     implement, same precedent as `rt64_common.rs`'s `fixedToFloat`).
//!   - `G_MW_FOG`: `(int16_t)((*dl)->p1(16, 16))` and `(int16_t)((*dl)->p1(0,
//!     16))` -- `p1(_, 16)` already zero-extends a 16-bit field into a
//!     `u32`, and the C-style cast then truncates to the low 16 bits and
//!     reinterprets as signed (a `u32 -> i16` narrowing cast is defined by
//!     truncating to the destination width first, per C++ integer-
//!     conversion rules, then reinterpreting). Ported as `(p1(w1, 16, 16)
//!     as u16) as i16` and `(p1(w1, 0, 16) as u16) as i16` -- the explicit
//!     `as u16` intermediate makes the truncate-then-reinterpret order
//!     visible in Rust the way the single C-style cast hides it in C++.
//!   - `G_MW_NUMLIGHT`'s `((*dl)->w1 - 0x80000000) >> 5) - 1` performs the
//!     subtraction and shift in `uint32_t` arithmetic (`w1` is already
//!     `uint32_t`, and `0x80000000` is an `unsigned int` literal since it
//!     does not fit in `int`) -- this is *not* a sign-extension site: the
//!     subtraction wraps modulo 2^32 (defined for unsigned types in C++),
//!     and the `>> 5` is a logical (unsigned) right shift. Ported as
//!     `w1.wrapping_sub(0x8000_0000) >> 5) as i32 - 1` -- `wrapping_sub`
//!     makes the intentional unsigned wraparound explicit (matching this
//!     module's and `rt64_common.rs`'s convention of never leaving a wrap
//!     path to Rust's debug-mode overflow panic when the C++ semantics are
//!     *specified* wraparound, as opposed to UB), then the final `- 1` is
//!     done in `i32` since the C++ result of this whole subexpression
//!     feeds `setLightCount` (out of scope, but the natural result type of
//!     "count minus one" is signed here, matching `rt64_gbi_f3dex.cpp`'s
//!     sibling convention of returning counts as `i32`, per this task's
//!     brief -- noted, not shared).
//! - **No other field in this file is ever cast to a signed type.**
//!   `matrix`'s `w1` (matrix address), `vertex`'s `w1` (vertex-array
//!   address), `moveMem`'s `w1` payloads, `runDl`'s `w1` (segmented
//!   address), `setColorImage`/`setTextureImage`/`setDepthImage`'s `w1`
//!   (framebuffer/texture addresses), `setOtherModeH`/`L`'s `w1` (mode
//!   bits), `setGeometryMode`/`clearGeometryMode`'s `w1` (mode-flag bits),
//!   and `texture`'s `sc`/`tc` (`uint16_t`, unsigned texture-coordinate
//!   scale factors) are all addresses or bitmask/unsigned-scale payloads in
//!   the upstream C++ -- ported as `u32`/`u16` with no sign cast, matching
//!   the source exactly.
//! - **`tri1`/`quad`'s `/ 10` on `p1(_, 8)` (an 8-bit *unsigned* field,
//!   0..=255) is C++ unsigned integer division** (`p1` returns `uint32_t`,
//!   division by the `int` literal `10` promotes `10` to `unsigned` per
//!   the usual arithmetic conversions, so this is unsigned-by-unsigned
//!   division, always truncating toward zero for non-negative operands --
//!   no rounding-direction ambiguity). Ported as plain `u32 / 10` before
//!   narrowing to `u8` via `as u8` (the C++ result is then implicitly
//!   narrowed to `uint8_t` at the `drawIndexedTri` call site or the local
//!   `const uint8_t` declaration -- since `p1(_, 8)` is at most 255, `/ 10`
//!   is at most 25, which always fits in `u8` with no truncation-loss to
//!   characterize).
//! - **`vertex`'s `p0(20, 4) + 1`**: a 4-bit unsigned field (0..=15) plus
//!   one, so the result ranges 1..=16 and always fits comfortably in
//!   `u32`/`u8` -- no overflow to characterize. Ported as plain `+ 1`.
//! - **`setColorImage`/`setTextureImage`'s `p0(0, 12) + 1`**: a 12-bit
//!   unsigned field (0..=4095) plus one, ranging 1..=4096 -- does not fit
//!   in `u8` but fits in `u16` (the C++ declares this `const uint16_t
//!   width`), so this port uses `u16` for the result, matching the source
//!   type exactly (not `u32`, which would silently widen the claimed
//!   range).
//! - **`moveWord`'s `F3D_G_MW_POINTS` arm computes `p0(8, 16) / 40` and
//!   `p0(8, 16) % 40` from the *same* 16-bit field** (an index/offset pair
//!   into an N64-microcode point-modification list) -- both are unsigned
//!   division/modulo on a `uint32_t` in range 0..=65535, so no sign or
//!   overflow subtlety; ported as plain `/` and `%`.
//! - **`moveMem`'s `F3D_G_MV_MATRIX_1` arm advances the display-list
//!   cursor by 3 words (`*dl = *dl + 3`)** -- this is a `DisplayList*`
//!   pointer-arithmetic side effect on the *caller's* iteration state, not
//!   a bitfield decode; captured here only as a documented fact (see
//!   `MoveMemDecoded::extra_words_consumed`), not executed, since actually
//!   walking a display list is out of this module's pure-decode scope.
//! - **C++ integer promotion**: every `p0`/`p1` call's `pos`/`bits`
//!   arguments are `uint8_t` literals that get promoted to `int` for the
//!   shift, then the `uint32_t` return implicitly narrows any `int`-typed
//!   subexpression back to unsigned -- this port's `u8` parameters and
//!   `u32` shift/mask arithmetic reproduce the same effective widths
//!   without needing Rust's (much stricter, no-implicit-promotion) integer
//!   rules to diverge from C++'s observable behavior anywhere in this file.
//!
//! ## Nonclaims
//!
//! No GPU, WGSL, or production wiring (this module is not called from
//! anywhere yet; dead-code warnings on the unused public surface are
//! expected and correct, matching `rt64_common.rs`'s and `rt64_math.rs`'s
//! precedent), and no RT64 visual/pixel/silicon parity or performance
//! claim. Not wired to `fn64-render-reference`'s GBI interpreter (see
//! "Reuse, not new type").
//!
//! **Dispatch deliberately not ported** (every `state->rsp->*`/`state->*`
//! call, per the task's DECODE-only scope): `matrix`'s `state->rsp->matrix`
//! call; `popMatrix`'s `state->rsp->popMatrix(1)`; every `moveMem` arm's
//! `state->rsp->set*`/`forceMatrix` call and its needed default-case
//! `assert(false && "Unimplemented move mem.")` (this port's
//! [`decode_move_mem`] returns `None` for an unrecognized selector instead
//! -- see below); `vertex`'s `state->rsp->setVertex` call; `runDl`'s
//! `state->pushReturnAddress`, `state->rsp->fromSegmentedMasked`, and the
//! RDRAM pointer resolution/jump (`state->fromRDRAM`, the `reinterpret_cast`,
//! and the `*dl = ... - 1` cursor rewrite); `endDl`'s
//! `state->popReturnAddress()`; `tri1`/`quad`'s `state->rsp->drawIndexedTri`
//! calls; every `moveWord` arm's `state->rsp->set*`/`modifyVertex` call;
//! `texture`'s `state->rsp->setTexture` call; `setOtherModeH`/`L`'s
//! `state->rsp->setOtherModeH`/`L` calls; `setGeometryMode`/
//! `clearGeometryMode`'s `state->rsp->*GeometryMode` calls; `rdpHalf1`/
//! `rdpHalf2`'s `state->microcode.half1`/`half2` field writes;
//! `setColorImage`/`setDepthImage`/`setTextureImage`'s
//! `state->rsp->set*Image` calls; `reset`'s `state->rsp->setLookAtVectors`/
//! `setFog` calls; and the entire `setup(GBI *gbi)` function (opcode-table
//! registration plus the `F3DENUM` constant table) -- all of these need the
//! whole `State`/`RSP`/`GBI` object graph, which is out of scope per the
//! task brief.
//!
//! **Opcodes ported** (bitfield decode only, as pure `(w0, w1) -> struct`
//! functions): `matrix` (-> [`decode_matrix`]), `popMatrix` (->
//! [`decode_pop_matrix`]), `moveMem` (-> [`decode_move_mem`],
//! [`MoveMemTarget`]), `vertex` (-> [`decode_vertex`]), `runDl` (->
//! [`decode_run_dl`]), `tri1` (-> [`decode_tri1`]), `quad` (->
//! [`decode_quad`]), `moveWord` (-> [`decode_move_word`],
//! [`MoveWordDecoded`]), `texture` (-> [`decode_texture`]),
//! `setOtherModeH`/`setOtherModeL` (-> [`decode_set_other_mode`], shared
//! layout), `setGeometryMode`/`clearGeometryMode` (->
//! [`decode_geometry_mode_word`], shared trivial layout),
//! `rdpHalf1`/`rdpHalf2` (-> [`decode_rdp_half`], shared trivial layout),
//! `setColorImage`/`setTextureImage` (-> [`decode_image_word`], shared
//! layout), `setDepthImage` (-> [`decode_depth_image`]).
//!
//! **Opcodes NOT ported** (no bitfield decode exists to characterize):
//! `endDl` (no fields -- pure control-flow pop), `sprite2DBase` (upstream
//! `// TODO` stub, empty body, no fields to decode -- porting behavior here
//! would mean inventing behavior upstream does not have), `cullDl`
//! (upstream `// TODO` stub, same reasoning), `reset` (no `DisplayList`
//! fields at all -- takes only `State*`), and `setup` (opcode-table wiring,
//! not decode).
//!
//! **`moveWord`'s `G_MW_MATRIX` and `G_MW_PERSPNORM` arms are upstream
//! stubs** (`G_MW_MATRIX` is `assert(false); // TODO`; `G_MW_PERSPNORM` is
//! a bare `// TODO` with no body) -- [`decode_move_word`] returns
//! [`MoveWordDecoded::Unimplemented`] for both, matching upstream's
//! "recognized selector, no implemented behavior" state exactly, not
//! inventing a payload decode upstream never wrote. The `default: break;`
//! arm (any `type` byte not matching a known `G_MW_*`/`F3D_G_MW_POINTS`
//! constant) is ported as [`MoveWordDecoded::Unrecognized`], distinct from
//! the two named-but-unimplemented TODO cases.
//!
//! **`moveMem`'s `default: assert(false && "Unimplemented move mem.")`**
//! is a debug-only crash-on-unknown-selector guard; this port's
//! [`decode_move_mem`] returns `None` instead of panicking, since a pure
//! decode function characterizing "which selector byte was this" should
//! not itself crash on an out-of-domain input -- the crash is dispatch
//! behavior (a debug assertion is not a bitfield-decode fact), consistent
//! with this module's DECODE-only scope.
//!
//! `F3D_G_SPNOOP` (mapped to `GBI_EXTENDED::noOpHook` in `setup`) and
//! `G_RDPNOOP` (mapped to `GBI_RDP::noOp`) are not F3D-file-local decode
//! functions at all (they live in `rt64_gbi_extended.cpp`/
//! `rt64_gbi_rdp.cpp`) -- out of this file's scope entirely, not merely
//! deferred.

/// `DisplayList::p0(pos, bits)`: `(w0 >> pos) & ((1 << bits) - 1)`.
fn p0(w0: u32, pos: u8, bits: u8) -> u32 {
    (w0 >> pos) & ((1u32 << bits) - 1)
}

/// `DisplayList::p1(pos, bits)`: `(w1 >> pos) & ((1 << bits) - 1)`.
fn p1(w1: u32, pos: u8, bits: u8) -> u32 {
    (w1 >> pos) & ((1u32 << bits) - 1)
}

/// `matrix`'s decoded operands: `w1` is the matrix-data segmented address,
/// `params` is the 8-bit flag byte at `p0(16, 8)` (push/load/mul,
/// projection/modelview -- flag *meaning* is dispatch, not decode, so this
/// stays a raw `u8`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MatrixDecoded {
    pub address: u32,
    pub params: u8,
}

pub fn decode_matrix(w0: u32, w1: u32) -> MatrixDecoded {
    MatrixDecoded {
        address: w1,
        params: p0(w0, 16, 8) as u8,
    }
}

/// `popMatrix`: `w1 == 0` gates whether a pop happens at all (the C++ only
/// calls `popMatrix(1)` inside the `if`). No count field is decoded from
/// the command word -- the `1` is a literal in the dispatch call, out of
/// this module's scope.
pub fn decode_pop_matrix(_w0: u32, w1: u32) -> bool {
    w1 == 0
}

/// `moveMem`'s selector byte at `p0(16, 8)`, recognized values only (see
/// `rt64_gbi_f3d.h`'s `F3D_G_MV_*` constants).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MoveMemTarget {
    Viewport,
    Matrix1,
    Light(u8),
    LookAtX,
    LookAtY,
}

/// `moveMem`'s decoded operands: the recognized target (`None` for the
/// `default: assert(false)` arm -- see module doc "Nonclaims"), the raw
/// `w1` payload, and whether this arm advances the display-list cursor by
/// 3 extra words (`F3D_G_MV_MATRIX_1` only).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MoveMemDecoded {
    pub target: Option<MoveMemTarget>,
    pub payload: u32,
    pub extra_words_consumed: u32,
}

pub fn decode_move_mem(w0: u32, w1: u32) -> MoveMemDecoded {
    use MoveMemTarget::*;
    let selector = p0(w0, 16, 8);
    let (target, extra_words_consumed) = match selector {
        0x80 => (Some(Viewport), 0), // F3D_G_MV_VIEWPORT
        0x9e => (Some(Matrix1), 3),  // F3D_G_MV_MATRIX_1
        0x86 => (Some(Light(0)), 0), // F3D_G_MV_L0
        0x88 => (Some(Light(1)), 0), // F3D_G_MV_L1
        0x8a => (Some(Light(2)), 0), // F3D_G_MV_L2
        0x8c => (Some(Light(3)), 0), // F3D_G_MV_L3
        0x8e => (Some(Light(4)), 0), // F3D_G_MV_L4
        0x90 => (Some(Light(5)), 0), // F3D_G_MV_L5
        0x92 => (Some(Light(6)), 0), // F3D_G_MV_L6
        0x94 => (Some(Light(7)), 0), // F3D_G_MV_L7
        0x84 => (Some(LookAtX), 0),  // F3D_G_MV_LOOKATX
        0x82 => (Some(LookAtY), 0),  // F3D_G_MV_LOOKATY
        _ => (None, 0),
    };
    MoveMemDecoded {
        target,
        payload: w1,
        extra_words_consumed,
    }
}

/// `vertex`'s decoded operands: `w1` is the vertex-array segmented
/// address, `count = p0(20, 4) + 1` (1..=16), `start = p0(16, 4)` (0..=15).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct VertexDecoded {
    pub address: u32,
    pub count: u32,
    pub start: u32,
}

pub fn decode_vertex(w0: u32, w1: u32) -> VertexDecoded {
    VertexDecoded {
        address: w1,
        count: p0(w0, 20, 4) + 1,
        start: p0(w0, 16, 4),
    }
}

/// `runDl`'s decoded operands: `push_return = p0(16, 1) == 0` (a branchless
/// helper -- upstream branches directly on `p0(16, 1) == 0`, ported here as
/// a bool field so callers see the same condition upstream tests), and the
/// raw `w1` segmented address (segment resolution/RDRAM jump are dispatch,
/// out of scope).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RunDlDecoded {
    pub push_return_address: bool,
    pub segmented_address: u32,
}

pub fn decode_run_dl(w0: u32, w1: u32) -> RunDlDecoded {
    RunDlDecoded {
        push_return_address: p0(w0, 16, 1) == 0,
        segmented_address: w1,
    }
}

/// `tri1`'s three vertex-buffer indices, each `p1(_, 8) / 10`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Tri1Decoded {
    pub v0: u8,
    pub v1: u8,
    pub v2: u8,
}

pub fn decode_tri1(_w0: u32, w1: u32) -> Tri1Decoded {
    Tri1Decoded {
        v0: (p1(w1, 16, 8) / 10) as u8,
        v1: (p1(w1, 8, 8) / 10) as u8,
        v2: (p1(w1, 0, 8) / 10) as u8,
    }
}

/// `quad`'s four vertex-buffer indices, each `p1(_, 8) / 10`. Dispatch
/// draws two triangles (`v0,v1,v2` then `v0,v2,v3`) from these four
/// indices -- out of scope, this struct stops at the decoded indices.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct QuadDecoded {
    pub v0: u8,
    pub v1: u8,
    pub v2: u8,
    pub v3: u8,
}

pub fn decode_quad(_w0: u32, w1: u32) -> QuadDecoded {
    QuadDecoded {
        v0: (p1(w1, 24, 8) / 10) as u8,
        v1: (p1(w1, 16, 8) / 10) as u8,
        v2: (p1(w1, 8, 8) / 10) as u8,
        v3: (p1(w1, 0, 8) / 10) as u8,
    }
}

/// `moveWord`'s decoded arm, keyed by `type = p0(0, 8)`. See module doc
/// "Admitted domain" for the `G_MW_CLIP`/`G_MW_FOG`/`G_MW_NUMLIGHT`
/// sign/wrap handling.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MoveWordDecoded {
    /// `G_MW_MATRIX`: upstream `assert(false); // TODO` -- recognized
    /// selector, no implemented payload.
    Unimplemented,
    /// `G_MW_NUMLIGHT`: `count_minus_one = ((w1 - 0x80000000) >> 5) - 1`,
    /// unsigned wraparound subtraction then logical shift, then a final
    /// signed `- 1`.
    NumLight { count_minus_one: i32 },
    /// `G_MW_CLIP`: `edge_index = (p0(8, 16) - G_MWO_CLIP_RNX) / 8`,
    /// `value = int16_t(w1 & 0xFFFF)`.
    Clip { edge_index: u32, value: i16 },
    /// `G_MW_SEGMENT`: `segment = p0(10, 4)`, `address = w1`.
    Segment { segment: u32, address: u32 },
    /// `G_MW_FOG`: `mul = int16_t(p1(16, 16))`, `offset = int16_t(p1(0,
    /// 16))`.
    Fog { mul: i16, offset: i16 },
    /// `G_MW_LIGHTCOL`: `light_index = p0(8, 16) / 32`, `color = w1`.
    LightColor { light_index: u32, color: u32 },
    /// `F3D_G_MW_POINTS`: `vertex_index = p0(8, 16) / 40`, `offset =
    /// p0(8, 16) % 40`, `value = w1`.
    Points {
        vertex_index: u32,
        offset: u32,
        value: u32,
    },
    /// `G_MW_PERSPNORM`: upstream bare `// TODO` -- recognized selector, no
    /// implemented payload.
    PerspNorm,
    /// `default: break;` -- an unrecognized `type` byte.
    Unrecognized { raw_type: u8 },
}

pub fn decode_move_word(w0: u32, w1: u32) -> MoveWordDecoded {
    let ty = p0(w0, 0, 8) as u8;
    match ty {
        0x00 => MoveWordDecoded::Unimplemented, // G_MW_MATRIX
        0x02 => {
            // G_MW_NUMLIGHT
            let shifted = w1.wrapping_sub(0x8000_0000) >> 5;
            MoveWordDecoded::NumLight {
                count_minus_one: shifted as i32 - 1,
            }
        }
        0x04 => {
            // G_MW_CLIP; G_MWO_CLIP_RNX = 0x04
            let edge_index = (p0(w0, 8, 16) - 0x04) / 8;
            let value = ((w1 & 0xFFFF) as u16) as i16;
            MoveWordDecoded::Clip { edge_index, value }
        }
        0x06 => {
            // G_MW_SEGMENT
            MoveWordDecoded::Segment {
                segment: p0(w0, 10, 4),
                address: w1,
            }
        }
        0x08 => {
            // G_MW_FOG
            let mul = (p1(w1, 16, 16) as u16) as i16;
            let offset = (p1(w1, 0, 16) as u16) as i16;
            MoveWordDecoded::Fog { mul, offset }
        }
        0x0a => {
            // G_MW_LIGHTCOL
            MoveWordDecoded::LightColor {
                light_index: p0(w0, 8, 16) / 32,
                color: w1,
            }
        }
        0x0c => {
            // F3D_G_MW_POINTS
            let field = p0(w0, 8, 16);
            MoveWordDecoded::Points {
                vertex_index: field / 40,
                offset: field % 40,
                value: w1,
            }
        }
        0x0e => MoveWordDecoded::PerspNorm, // G_MW_PERSPNORM
        other => MoveWordDecoded::Unrecognized { raw_type: other },
    }
}

/// `texture`'s decoded operands: `tile = p0(8, 3)`, `level = p0(11, 3)`,
/// `on = p0(0, 8)`, `sc = p1(16, 16)`, `tc = p1(0, 16)` (both `sc`/`tc` are
/// unsigned `uint16_t` texture-coordinate scale factors in the source).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TextureDecoded {
    pub tile: u8,
    pub level: u8,
    pub on: u8,
    pub sc: u16,
    pub tc: u16,
}

pub fn decode_texture(w0: u32, w1: u32) -> TextureDecoded {
    TextureDecoded {
        tile: p0(w0, 8, 3) as u8,
        level: p0(w0, 11, 3) as u8,
        on: p0(w0, 0, 8) as u8,
        sc: p1(w1, 16, 16) as u16,
        tc: p1(w1, 0, 16) as u16,
    }
}

/// `setOtherModeH`/`setOtherModeL`'s shared decoded operands: both opcode
/// functions call `state->rsp->setOtherMode{H,L}((*dl)->p0(0, 8),
/// (*dl)->p0(8, 8), (*dl)->w1)` with the identical field layout -- ported
/// as one decode function used by both call sites (a genuine shared
/// layout within *this* file, not a cross-file abstraction -- see task
/// note on not sharing with a sibling f3dex module).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SetOtherModeDecoded {
    pub shift: u8,
    pub length: u8,
    pub data: u32,
}

pub fn decode_set_other_mode(w0: u32, w1: u32) -> SetOtherModeDecoded {
    SetOtherModeDecoded {
        shift: p0(w0, 0, 8) as u8,
        length: p0(w0, 8, 8) as u8,
        data: w1,
    }
}

/// `setGeometryMode`/`clearGeometryMode`: both pass only `w1` through
/// unchanged (the geometry-mode bitmask itself). Trivial, but named
/// per-opcode per the task's "enumerate every function" instruction.
pub fn decode_geometry_mode_word(_w0: u32, w1: u32) -> u32 {
    w1
}

/// `rdpHalf1`/`rdpHalf2`: both pass only `w1` through unchanged (stored
/// into `state->microcode.half1`/`half2`, out of scope).
pub fn decode_rdp_half(_w0: u32, w1: u32) -> u32 {
    w1
}

/// `setColorImage`/`setTextureImage`'s shared decoded operands: both
/// opcode functions extract `fmt = p0(21, 3)`, `siz = p0(19, 2)`, `width =
/// p0(0, 12) + 1`, `address = w1` with the identical layout.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ImageWordDecoded {
    pub fmt: u8,
    pub siz: u8,
    pub width: u16,
    pub address: u32,
}

pub fn decode_image_word(w0: u32, w1: u32) -> ImageWordDecoded {
    ImageWordDecoded {
        fmt: p0(w0, 21, 3) as u8,
        siz: p0(w0, 19, 2) as u8,
        width: (p0(w0, 0, 12) + 1) as u16,
        address: w1,
    }
}

/// `setDepthImage`: passes only `w1` through unchanged (the depth-buffer
/// address; no `fmt`/`siz`/`width` fields, unlike `setColorImage`/
/// `setTextureImage`).
pub fn decode_depth_image(_w0: u32, w1: u32) -> u32 {
    w1
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- p0 / p1 raw extractor behavior ---

    #[test]
    fn p0_extracts_low_bits_at_zero_position() {
        assert_eq!(p0(0b1111_1111, 0, 4), 0b1111);
    }

    #[test]
    fn p0_masks_out_bits_above_the_field_width() {
        // One bit above an 8-bit field at position 0 must not leak in.
        assert_eq!(p0(0x1_FF, 0, 8), 0xFF);
    }

    #[test]
    fn p0_shifts_before_masking() {
        // Field at pos=8, width=4 covering bits [8..12); 0x0F00 has bits
        // 8-11 set to 0xF, all other bits zero.
        assert_eq!(p0(0x0000_0F00, 8, 4), 0xF);
    }

    #[test]
    fn p0_all_zero_word_is_zero() {
        // bits=16 (the widest field width this file actually uses) rather
        // than 32 -- p0/p1 are never called with bits=32 in the source, and
        // `1u32 << 32` is itself a shift-overflow panic in Rust, unrelated
        // to any behavior this port characterizes.
        assert_eq!(p0(0, 0, 16), 0);
    }

    #[test]
    fn p0_all_ones_word_full_sixteen_bit_field() {
        assert_eq!(p0(u32::MAX, 0, 16), 0xFFFF);
    }

    #[test]
    fn p0_reads_from_w0_only_not_w1() {
        // p0 has no w1 parameter -- confirmed structurally, but exercise a
        // representative extraction to lock behavior against w0 alone.
        assert_eq!(p0(0xABCD_0000, 16, 16), 0xABCD);
    }

    #[test]
    fn p1_reads_from_w1_independently_of_w0() {
        assert_eq!(p1(0xDEAD_0000, 16, 16), 0xDEAD);
    }

    #[test]
    fn p1_one_bit_field_extracts_single_bit() {
        assert_eq!(p1(0b10, 1, 1), 1);
        assert_eq!(p1(0b10, 0, 1), 0);
    }

    // --- decode_matrix ---

    #[test]
    fn decode_matrix_all_zero() {
        let d = decode_matrix(0, 0);
        assert_eq!(d.address, 0);
        assert_eq!(d.params, 0);
    }

    #[test]
    fn decode_matrix_params_field_max_value() {
        // p0(16, 8) all-ones = 0xFF, positioned at bits [16..24).
        let d = decode_matrix(0x00FF_0000, 0xDEAD_BEEF);
        assert_eq!(d.params, 0xFF);
        assert_eq!(d.address, 0xDEAD_BEEF);
    }

    #[test]
    fn decode_matrix_one_bit_above_params_field_is_masked_out() {
        // Bit 24 (just above the 8-bit field at [16..24)) must not appear.
        let d = decode_matrix(0x0100_0000, 0);
        assert_eq!(d.params, 0);
    }

    // --- decode_pop_matrix ---

    #[test]
    fn decode_pop_matrix_zero_w1_is_true() {
        assert!(decode_pop_matrix(0xFFFF_FFFF, 0));
    }

    #[test]
    fn decode_pop_matrix_nonzero_w1_is_false() {
        assert!(!decode_pop_matrix(0, 1));
        assert!(!decode_pop_matrix(0, u32::MAX));
    }

    // --- decode_move_mem ---

    #[test]
    fn decode_move_mem_viewport_selector() {
        let d = decode_move_mem(0x0080_0000, 0x1234);
        assert_eq!(d.target, Some(MoveMemTarget::Viewport));
        assert_eq!(d.payload, 0x1234);
        assert_eq!(d.extra_words_consumed, 0);
    }

    #[test]
    fn decode_move_mem_matrix1_selector_reports_extra_words() {
        let d = decode_move_mem(0x009e_0000, 0xCAFE);
        assert_eq!(d.target, Some(MoveMemTarget::Matrix1));
        assert_eq!(d.extra_words_consumed, 3);
    }

    #[test]
    fn decode_move_mem_all_eight_light_selectors() {
        let expected = [
            (0x86u32, 0u8),
            (0x88, 1),
            (0x8a, 2),
            (0x8c, 3),
            (0x8e, 4),
            (0x90, 5),
            (0x92, 6),
            (0x94, 7),
        ];
        for (selector, index) in expected {
            let w0 = selector << 16;
            let d = decode_move_mem(w0, 0);
            assert_eq!(
                d.target,
                Some(MoveMemTarget::Light(index)),
                "selector={selector:#x}"
            );
        }
    }

    #[test]
    fn decode_move_mem_lookat_x_and_y_selectors() {
        let dx = decode_move_mem(0x0084_0000, 0);
        assert_eq!(dx.target, Some(MoveMemTarget::LookAtX));
        let dy = decode_move_mem(0x0082_0000, 0);
        assert_eq!(dy.target, Some(MoveMemTarget::LookAtY));
    }

    #[test]
    fn decode_move_mem_unrecognized_selector_is_none() {
        let d = decode_move_mem(0x00FF_0000, 0);
        assert_eq!(d.target, None);
        assert_eq!(d.extra_words_consumed, 0);
    }

    // --- decode_vertex ---

    #[test]
    fn decode_vertex_all_zero_count_is_one_start_is_zero() {
        let d = decode_vertex(0, 0);
        assert_eq!(d.count, 1);
        assert_eq!(d.start, 0);
        assert_eq!(d.address, 0);
    }

    #[test]
    fn decode_vertex_count_field_max_value_yields_sixteen() {
        // p0(20, 4) all-ones = 0xF -> count = 16.
        let d = decode_vertex(0x00F0_0000, 0);
        assert_eq!(d.count, 16);
    }

    #[test]
    fn decode_vertex_start_field_max_value() {
        // p0(16, 4) all-ones = 0xF at bits [16..20).
        let d = decode_vertex(0x000F_0000, 0);
        assert_eq!(d.start, 0xF);
    }

    #[test]
    fn decode_vertex_bit_above_count_field_is_masked() {
        // Bit 24 sits just above the count field's [20..24) range.
        let d = decode_vertex(0x0100_0000, 0);
        assert_eq!(d.count, 1);
    }

    #[test]
    fn decode_vertex_address_is_raw_w1() {
        let d = decode_vertex(0, 0xAABB_CCDD);
        assert_eq!(d.address, 0xAABB_CCDD);
    }

    // --- decode_run_dl ---

    #[test]
    fn decode_run_dl_flag_bit_zero_pushes_return_address() {
        let d = decode_run_dl(0, 0);
        assert!(d.push_return_address);
    }

    #[test]
    fn decode_run_dl_flag_bit_set_does_not_push() {
        // p0(16, 1) reads bit 16.
        let d = decode_run_dl(0x0001_0000, 0);
        assert!(!d.push_return_address);
    }

    #[test]
    fn decode_run_dl_segmented_address_is_raw_w1() {
        let d = decode_run_dl(0, 0x0800_1234);
        assert_eq!(d.segmented_address, 0x0800_1234);
    }

    // --- decode_tri1 ---

    #[test]
    fn decode_tri1_all_zero() {
        let d = decode_tri1(0, 0);
        assert_eq!((d.v0, d.v1, d.v2), (0, 0, 0));
    }

    #[test]
    fn decode_tri1_field_max_value_divides_by_ten() {
        // p1(_, 8) all-ones = 255; 255 / 10 = 25.
        let d = decode_tri1(0, 0x00FF_FFFF);
        assert_eq!((d.v0, d.v1, d.v2), (25, 25, 25));
    }

    #[test]
    fn decode_tri1_distinct_fields_do_not_bleed_into_each_other() {
        // v0 at [16..24)=0x0A(->1), v1 at [8..16)=0x14(->2), v2 at
        // [0..8)=0x1E(->3).
        let w1 = (0x0Au32 << 16) | (0x14 << 8) | 0x1E;
        let d = decode_tri1(0, w1);
        assert_eq!(d.v0, 1);
        assert_eq!(d.v1, 2);
        assert_eq!(d.v2, 3);
    }

    #[test]
    fn decode_tri1_bit_above_v2_field_does_not_affect_v2() {
        // Byte value 20 placed at bits [8..16) (v1's field) must not leak
        // into v2's field ([0..8)): v1 = 20/10 = 2, v2 stays 0.
        let d = decode_tri1(0, 20 << 8);
        assert_eq!(d.v2, 0);
        assert_eq!(d.v1, 2);
    }

    // --- decode_quad ---

    #[test]
    fn decode_quad_all_zero() {
        let d = decode_quad(0, 0);
        assert_eq!((d.v0, d.v1, d.v2, d.v3), (0, 0, 0, 0));
    }

    #[test]
    fn decode_quad_all_ones_word_divides_each_byte_by_ten() {
        let d = decode_quad(0, u32::MAX);
        assert_eq!((d.v0, d.v1, d.v2, d.v3), (25, 25, 25, 25));
    }

    #[test]
    fn decode_quad_four_distinct_byte_fields() {
        let w1 = (10u32 << 24) | (20 << 16) | (30 << 8) | 40;
        let d = decode_quad(0, w1);
        assert_eq!(d.v0, 1);
        assert_eq!(d.v1, 2);
        assert_eq!(d.v2, 3);
        assert_eq!(d.v3, 4);
    }

    // --- decode_move_word: G_MW_MATRIX / G_MW_PERSPNORM stubs ---

    #[test]
    fn decode_move_word_g_mw_matrix_is_unimplemented() {
        assert_eq!(
            decode_move_word(0x00, 0xDEAD_BEEF),
            MoveWordDecoded::Unimplemented
        );
    }

    #[test]
    fn decode_move_word_g_mw_perspnorm_is_perspnorm_variant() {
        assert_eq!(decode_move_word(0x0e, 0), MoveWordDecoded::PerspNorm);
    }

    #[test]
    fn decode_move_word_unrecognized_type_reports_raw_byte() {
        let d = decode_move_word(0x7F, 0);
        assert_eq!(d, MoveWordDecoded::Unrecognized { raw_type: 0x7F });
    }

    #[test]
    fn decode_move_word_type_byte_is_masked_to_eight_bits() {
        // p0(0, 8) with a stray bit 8 set must still read type=0x02
        // (G_MW_NUMLIGHT), ignoring bit 8.
        let d = decode_move_word(0x0000_0102, 0x8000_0020);
        assert_eq!(d, MoveWordDecoded::NumLight { count_minus_one: 0 });
    }

    // --- decode_move_word: G_MW_NUMLIGHT ---

    #[test]
    fn decode_move_word_numlight_at_base_offset_is_zero() {
        // w1 = 0x80000000 exactly: (0x80000000 - 0x80000000) >> 5 = 0; -1 = -1.
        let d = decode_move_word(0x02, 0x8000_0000);
        assert_eq!(
            d,
            MoveWordDecoded::NumLight {
                count_minus_one: -1
            }
        );
    }

    #[test]
    fn decode_move_word_numlight_one_light() {
        // (0x80000000 + 32 - 0x80000000) >> 5 = 1; -1 = 0.
        let d = decode_move_word(0x02, 0x8000_0020);
        assert_eq!(d, MoveWordDecoded::NumLight { count_minus_one: 0 });
    }

    #[test]
    fn decode_move_word_numlight_eight_lights() {
        // 8 * 32 = 256 = 0x100.
        let d = decode_move_word(0x02, 0x8000_0000 + 0x100);
        assert_eq!(d, MoveWordDecoded::NumLight { count_minus_one: 7 });
    }

    #[test]
    fn decode_move_word_numlight_w1_below_base_wraps_unsigned() {
        // w1 = 0: (0 - 0x80000000) wraps to 0x80000000 (unsigned), >> 5 =
        // 0x04000000, - 1 = 0x03FFFFFF as i32.
        let d = decode_move_word(0x02, 0);
        let expected: i32 = ((0u32.wrapping_sub(0x8000_0000)) >> 5) as i32 - 1;
        assert_eq!(
            d,
            MoveWordDecoded::NumLight {
                count_minus_one: expected
            }
        );
        assert_eq!(expected, 0x03FF_FFFF);
    }

    // --- decode_move_word: G_MW_CLIP ---

    #[test]
    fn decode_move_word_clip_all_zero() {
        // edge_index = (0 - 4) / 8 in u32 wraps hugely -- but the domain of
        // legal p0(8,16) values starts at G_MWO_CLIP_RNX=4, so use that as
        // the "zero" case instead. type=G_MW_CLIP=0x04 at bits [0..8),
        // p0(8,16)=4 at bits [8..24).
        let w0 = (4u32 << 8) | 0x04;
        let d = decode_move_word(w0, 0);
        assert_eq!(
            d,
            MoveWordDecoded::Clip {
                edge_index: 0,
                value: 0
            }
        );
    }

    #[test]
    fn decode_move_word_clip_edge_index_divides_offset_by_eight() {
        // p0(8,16) = 4 + 8*3 = 28 -> edge_index = 3. type=0x04 at [0..8).
        let w0 = (28u32 << 8) | 0x04;
        let d = decode_move_word(w0, 0);
        assert_eq!(
            d,
            MoveWordDecoded::Clip {
                edge_index: 3,
                value: 0
            }
        );
    }

    #[test]
    fn decode_move_word_clip_value_sign_extends_negative() {
        // w1 low 16 bits = 0xFFFF -> int16_t(-1). type=0x04, p0(8,16)=4.
        let w0 = (4u32 << 8) | 0x04;
        let d = decode_move_word(w0, 0x0000_FFFF);
        assert_eq!(
            d,
            MoveWordDecoded::Clip {
                edge_index: 0,
                value: -1
            }
        );
    }

    #[test]
    fn decode_move_word_clip_value_positive_boundary() {
        // w1 low 16 bits = 0x7FFF -> int16_t(32767), the largest positive.
        let w0 = (4u32 << 8) | 0x04;
        let d = decode_move_word(w0, 0x0000_7FFF);
        assert_eq!(
            d,
            MoveWordDecoded::Clip {
                edge_index: 0,
                value: 32767
            }
        );
    }

    #[test]
    fn decode_move_word_clip_value_negative_boundary() {
        // w1 low 16 bits = 0x8000 -> int16_t(-32768), the most negative.
        let w0 = (4u32 << 8) | 0x04;
        let d = decode_move_word(w0, 0x0000_8000);
        assert_eq!(
            d,
            MoveWordDecoded::Clip {
                edge_index: 0,
                value: -32768
            }
        );
    }

    #[test]
    fn decode_move_word_clip_value_ignores_high_bits_of_w1() {
        // High 16 bits of w1 must not leak into the masked-to-0xFFFF value.
        let w0 = (4u32 << 8) | 0x04;
        let d = decode_move_word(w0, 0xFFFF_0001);
        assert_eq!(
            d,
            MoveWordDecoded::Clip {
                edge_index: 0,
                value: 1
            }
        );
    }

    // --- decode_move_word: G_MW_SEGMENT ---

    #[test]
    fn decode_move_word_segment_all_zero() {
        let d = decode_move_word(0x06, 0);
        assert_eq!(
            d,
            MoveWordDecoded::Segment {
                segment: 0,
                address: 0
            }
        );
    }

    #[test]
    fn decode_move_word_segment_field_max_value() {
        // p0(10, 4) all-ones = 0xF at bits [10..14).
        let w0 = (0x0Fu32 << 10) | 0x06;
        let d = decode_move_word(w0, 0xABCD_EF01);
        assert_eq!(
            d,
            MoveWordDecoded::Segment {
                segment: 0xF,
                address: 0xABCD_EF01
            }
        );
    }

    #[test]
    fn decode_move_word_segment_bit_above_field_is_masked() {
        // Bit 14 sits just above the segment field's [10..14) range.
        let w0 = (1u32 << 14) | 0x06;
        let d = decode_move_word(w0, 0);
        assert_eq!(
            d,
            MoveWordDecoded::Segment {
                segment: 0,
                address: 0
            }
        );
    }

    // --- decode_move_word: G_MW_FOG ---

    #[test]
    fn decode_move_word_fog_all_zero() {
        let d = decode_move_word(0x08, 0);
        assert_eq!(d, MoveWordDecoded::Fog { mul: 0, offset: 0 });
    }

    #[test]
    fn decode_move_word_fog_negative_mul_and_offset() {
        let w1 = (0xFFFFu32 << 16) | 0x8000;
        let d = decode_move_word(0x08, w1);
        assert_eq!(
            d,
            MoveWordDecoded::Fog {
                mul: -1,
                offset: -32768
            }
        );
    }

    #[test]
    fn decode_move_word_fog_positive_boundary_both_fields() {
        let w1 = (0x7FFFu32 << 16) | 0x7FFF;
        let d = decode_move_word(0x08, w1);
        assert_eq!(
            d,
            MoveWordDecoded::Fog {
                mul: 32767,
                offset: 32767
            }
        );
    }

    #[test]
    fn decode_move_word_fog_mul_and_offset_are_independent_fields() {
        let w1 = (100u32 << 16) | 0xFFFF; // mul=100, offset=-1
        let d = decode_move_word(0x08, w1);
        assert_eq!(
            d,
            MoveWordDecoded::Fog {
                mul: 100,
                offset: -1
            }
        );
    }

    // --- decode_move_word: G_MW_LIGHTCOL ---

    #[test]
    fn decode_move_word_lightcol_all_zero() {
        let d = decode_move_word(0x0a, 0);
        assert_eq!(
            d,
            MoveWordDecoded::LightColor {
                light_index: 0,
                color: 0
            }
        );
    }

    #[test]
    fn decode_move_word_lightcol_divides_offset_by_thirty_two() {
        // p0(8,16) = 32*2 = 64.
        let w0 = (64u32 << 8) | 0x0a;
        let d = decode_move_word(w0, 0x00FF_00FF);
        assert_eq!(
            d,
            MoveWordDecoded::LightColor {
                light_index: 2,
                color: 0x00FF_00FF
            }
        );
    }

    // --- decode_move_word: F3D_G_MW_POINTS ---

    #[test]
    fn decode_move_word_points_all_zero() {
        let d = decode_move_word(0x0c, 0);
        assert_eq!(
            d,
            MoveWordDecoded::Points {
                vertex_index: 0,
                offset: 0,
                value: 0
            }
        );
    }

    #[test]
    fn decode_move_word_points_splits_field_by_div_and_mod_forty() {
        // p0(8,16) = 40*3 + 7 = 127.
        let w0 = (127u32 << 8) | 0x0c;
        let d = decode_move_word(w0, 0xCAFE_BABE);
        assert_eq!(
            d,
            MoveWordDecoded::Points {
                vertex_index: 3,
                offset: 7,
                value: 0xCAFE_BABE
            }
        );
    }

    #[test]
    fn decode_move_word_points_field_max_value() {
        // p0(8,16) all-ones = 0xFFFF = 65535 -> 65535/40=1638, 65535%40=15.
        let w0 = (0xFFFFu32 << 8) | 0x0c;
        let d = decode_move_word(w0, 0);
        assert_eq!(
            d,
            MoveWordDecoded::Points {
                vertex_index: 1638,
                offset: 15,
                value: 0
            }
        );
    }

    // --- decode_texture ---

    #[test]
    fn decode_texture_all_zero() {
        let d = decode_texture(0, 0);
        assert_eq!(d.tile, 0);
        assert_eq!(d.level, 0);
        assert_eq!(d.on, 0);
        assert_eq!(d.sc, 0);
        assert_eq!(d.tc, 0);
    }

    #[test]
    fn decode_texture_tile_field_max_value_three_bits() {
        // p0(8,3) all-ones = 0x7 at bits [8..11).
        let d = decode_texture(0x07 << 8, 0);
        assert_eq!(d.tile, 7);
    }

    #[test]
    fn decode_texture_level_field_max_value_three_bits() {
        // p0(11,3) all-ones = 0x7 at bits [11..14).
        let d = decode_texture(0x07 << 11, 0);
        assert_eq!(d.level, 7);
    }

    #[test]
    fn decode_texture_tile_and_level_fields_are_adjacent_but_independent() {
        // tile occupies bits 8-10, level occupies bits 11-13 -- setting only
        // tile's top bit (bit 10) must not affect level.
        let d = decode_texture(1 << 10, 0);
        assert_eq!(d.tile, 4);
        assert_eq!(d.level, 0);
    }

    #[test]
    fn decode_texture_on_field_full_eight_bits() {
        let d = decode_texture(0xFF, 0);
        assert_eq!(d.on, 0xFF);
    }

    #[test]
    fn decode_texture_sc_and_tc_are_independent_sixteen_bit_fields() {
        let w1 = (0x1234u32 << 16) | 0x5678;
        let d = decode_texture(0, w1);
        assert_eq!(d.sc, 0x1234);
        assert_eq!(d.tc, 0x5678);
    }

    #[test]
    fn decode_texture_sc_max_value_does_not_leak_into_tc() {
        let d = decode_texture(0, 0xFFFF_0000);
        assert_eq!(d.sc, 0xFFFF);
        assert_eq!(d.tc, 0);
    }

    // --- decode_set_other_mode (shared by setOtherModeH / setOtherModeL) ---

    #[test]
    fn decode_set_other_mode_all_zero() {
        let d = decode_set_other_mode(0, 0);
        assert_eq!(d.shift, 0);
        assert_eq!(d.length, 0);
        assert_eq!(d.data, 0);
    }

    #[test]
    fn decode_set_other_mode_shift_and_length_are_independent_byte_fields() {
        let w0 = (0xAAu32 << 8) | 0x55;
        let d = decode_set_other_mode(w0, 0xDEAD_BEEF);
        assert_eq!(d.shift, 0x55);
        assert_eq!(d.length, 0xAA);
        assert_eq!(d.data, 0xDEAD_BEEF);
    }

    #[test]
    fn decode_set_other_mode_shift_field_max_value() {
        let d = decode_set_other_mode(0xFF, 0);
        assert_eq!(d.shift, 0xFF);
    }

    // --- decode_geometry_mode_word / decode_rdp_half / decode_depth_image ---

    #[test]
    fn decode_geometry_mode_word_passes_w1_through_unchanged() {
        assert_eq!(
            decode_geometry_mode_word(0xFFFF_FFFF, 0x1234_5678),
            0x1234_5678
        );
    }

    #[test]
    fn decode_geometry_mode_word_ignores_w0_entirely() {
        assert_eq!(decode_geometry_mode_word(0, 42), 42);
        assert_eq!(decode_geometry_mode_word(u32::MAX, 42), 42);
    }

    #[test]
    fn decode_rdp_half_passes_w1_through_unchanged() {
        assert_eq!(decode_rdp_half(0xABCD, 0x1111_2222), 0x1111_2222);
    }

    #[test]
    fn decode_depth_image_passes_w1_through_unchanged() {
        assert_eq!(decode_depth_image(0xABCD, 0x0300_0000), 0x0300_0000);
    }

    // --- decode_image_word (shared by setColorImage / setTextureImage) ---

    #[test]
    fn decode_image_word_all_zero_width_is_one() {
        let d = decode_image_word(0, 0);
        assert_eq!(d.fmt, 0);
        assert_eq!(d.siz, 0);
        assert_eq!(d.width, 1);
        assert_eq!(d.address, 0);
    }

    #[test]
    fn decode_image_word_fmt_field_max_value_three_bits() {
        // p0(21,3) all-ones = 0x7 at bits [21..24).
        let d = decode_image_word(0x07 << 21, 0);
        assert_eq!(d.fmt, 7);
    }

    #[test]
    fn decode_image_word_siz_field_max_value_two_bits() {
        // p0(19,2) all-ones = 0x3 at bits [19..21).
        let d = decode_image_word(0x03 << 19, 0);
        assert_eq!(d.siz, 3);
    }

    #[test]
    fn decode_image_word_fmt_and_siz_fields_are_adjacent_but_independent() {
        // fmt occupies bits 21-23, siz occupies bits 19-20 -- setting only
        // fmt's low bit (bit 21) must not affect siz.
        let d = decode_image_word(1 << 21, 0);
        assert_eq!(d.fmt, 1);
        assert_eq!(d.siz, 0);
    }

    #[test]
    fn decode_image_word_width_field_max_value_wraps_to_four_thousand_ninety_six() {
        // p0(0,12) all-ones = 0xFFF = 4095, + 1 = 4096.
        let d = decode_image_word(0x0FFF, 0);
        assert_eq!(d.width, 4096);
    }

    #[test]
    fn decode_image_word_width_bit_above_field_is_masked() {
        // Bit 12 sits just above the 12-bit width field at [0..12).
        let d = decode_image_word(1 << 12, 0);
        assert_eq!(d.width, 1);
    }

    #[test]
    fn decode_image_word_address_is_raw_w1() {
        let d = decode_image_word(0, 0x0400_1000);
        assert_eq!(d.address, 0x0400_1000);
    }

    #[test]
    fn decode_image_word_all_fields_set_simultaneously_do_not_bleed() {
        let w0 = (0x5u32 << 21) | (0x2 << 19) | 0x123;
        let d = decode_image_word(w0, 0x8000_0000);
        assert_eq!(d.fmt, 5);
        assert_eq!(d.siz, 2);
        assert_eq!(d.width, 0x124);
        assert_eq!(d.address, 0x8000_0000);
    }
}
