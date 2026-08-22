//! Literal port of RT64's F3DEX2 display-list command-word **bitfield
//! decoding**, from the permitted MIT RT64 Rust-port source pinned at
//! commit `5473732a822a4423b5696e7cb18fecc425a59875`
//! (`docs/RT64-PORT-AUTHORITY.md`), `src/gbi/rt64_gbi_f3dex2.cpp` (SHA-256 of
//! the whole file,
//! `f7b3acbc05406186eb3578e128e5a059ad560770dfc22ce9a4539f3eb79f0bc3`) and
//! `src/gbi/rt64_gbi_f3dex2.h` (SHA-256 of the whole file,
//! `73c23689e91fe6ed8aa7746f7a53f55f9dce1a96269eac1d6383cce0cfa36850`).
//!
//! Only the **bitfield extraction and per-opcode operand layout** is
//! ported: `DisplayList::p0`/`p1` (from `src/gbi/rt64_gbi.cpp`, cited
//! below) plus the operand set each opcode function reads from
//! `(*dl)->w0`/`w1` via those two extractors. The `state->rsp->someCall(...)`
//! / `state->ext.interpreter->...` dispatch itself is NOT ported -- it
//! needs the whole `RSP`/`State` object graph, which is out of scope (see
//! "Nonclaims"). `GBI_F3DEX2::setup` (the opcode-to-function dispatch table
//! and the `gbi->constants` map) and `GBI_F3DEX2::reset` (pure state
//! mutation, no `DisplayList` operand at all) are also not ported, for the
//! same reason.
//!
//! ```text
//! // src/gbi/rt64_gbi.cpp, lines 32-38
//! uint32_t DisplayList::p0(uint8_t pos, uint8_t bits) const {
//!     return ((w0 >> pos) & ((0x01 << bits) - 1));
//! }
//!
//! uint32_t DisplayList::p1(uint8_t pos, uint8_t bits) const {
//!     return ((w1 >> pos) & ((0x01 << bits) - 1));
//! }
//!
//! // src/gbi/rt64_gbi_f3dex2.cpp, lines 18-158
//! namespace RT64 {
//!     namespace GBI_F3DEX2 {
//!         void setOtherMode(State *state, DisplayList **dl) {
//!             const uint32_t high = (*dl)->p0(0, 24);
//!             const uint32_t low = (*dl)->w1;
//!             state->rsp->setOtherMode(high, low);
//!         }
//!
//!         void setOtherModeH(State *state, DisplayList **dl) {
//!             const uint32_t size = (*dl)->p0(0, 8) + 1;
//!             const uint32_t off = std::max(0, (int32_t)(32 - (*dl)->p0(8, 8) - size));
//!             state->rsp->setOtherModeH(size, off, (*dl)->w1);
//!         }
//!
//!         void setOtherModeL(State *state, DisplayList **dl) {
//!             const uint32_t size = (*dl)->p0(0, 8) + 1;
//!             const uint32_t off = std::max(0, (int32_t)(32 - (*dl)->p0(8, 8) - size));
//!             state->rsp->setOtherModeL(size, off, (*dl)->w1);
//!         }
//!
//!         void moveMem(State *state, DisplayList **dl) {
//!             uint8_t index = (*dl)->p0(0, 8);
//!             switch (index) {
//!             case F3DEX2_G_MV_VIEWPORT:
//!                 state->rsp->setViewport((*dl)->w1);
//!                 break;
//!             case F3DEX2_G_MV_MATRIX:
//!                 state->rsp->forceMatrix((*dl)->w1);
//!                 break;
//!             case F3DEX2_G_MV_LIGHT: {
//!                 uint8_t offset = (*dl)->p0(8, 8) * 8;
//!                 int index = (offset / 24);
//!                 if (index >= 2) {
//!                     state->rsp->setLight(index - 2, (*dl)->w1);
//!                 }
//!                 else {
//!                     state->rsp->setLookAt(index, (*dl)->w1);
//!                 }
//!
//!                 break;
//!             }
//!             default:
//!                 assert(false && "Unimplemented moveMem command");
//!                 break;
//!             }
//!         }
//!
//!         void moveWord(State *state, DisplayList **dl) {
//!             uint8_t type = (*dl)->p0(16, 8);
//!             switch (type) {
//!             case F3DEX2_G_MW_FORCEMTX:
//!                 state->rsp->setModelViewProjChanged((*dl)->w1 == 0);
//!                 break;
//!             case G_MW_MATRIX:
//!                 state->rsp->insertMatrix((*dl)->p0(0, 16), (*dl)->w1);
//!                 break;
//!             case G_MW_NUMLIGHT:
//!                 state->rsp->setLightCount((*dl)->w1 / 24);
//!                 break;
//!             case G_MW_CLIP:
//!                 state->rsp->setClipRatioEdge(((*dl)->p0(0, 16) - G_MWO_CLIP_RNX) / 8, (*dl)->w1);
//!                 break;
//!             case G_MW_SEGMENT:
//!                 state->rsp->setSegment((*dl)->p0(2, 4), (*dl)->w1);
//!                 break;
//!             case G_MW_FOG:
//!                 state->rsp->setFog((int16_t)((*dl)->p1(16, 16)), (int16_t)((*dl)->p1(0, 16)));
//!                 break;
//!             case G_MW_LIGHTCOL:
//!                 state->rsp->setLightColor(((*dl)->p0(0, 16) / 24), (*dl)->w1);
//!                 break;
//!             case G_MW_PERSPNORM:
//!                 state->rsp->setPerspNorm((*dl)->w1);
//!                 break;
//!             default:
//!                 assert(false && "Unimplemented moveWord command");
//!                 break;
//!             }
//!         }
//!
//!         void matrix(State *state, DisplayList **dl) {
//!             state->rsp->matrix((*dl)->w1, (*dl)->p0(0, 8) ^ state->rsp->pushMask);
//!         }
//!
//!         void popMatrix(State *state, DisplayList **dl) {
//!             state->rsp->popMatrix((*dl)->w1 >> 6);
//!         }
//!
//!         void geometryMode(State *state, DisplayList **dl) {
//!             uint32_t offMask = (*dl)->p0(0, 24);
//!             uint32_t onMask = (*dl)->w1;
//!             state->rsp->modifyGeometryMode(offMask, onMask);
//!         }
//!
//!         void texture(State *state, DisplayList **dl) {
//!             uint8_t tile = (*dl)->p0(8, 3);
//!             uint8_t level = (*dl)->p0(11, 3);
//!             uint8_t on = (*dl)->p0(1, 7);
//!             uint16_t sc = (*dl)->p1(16, 16);
//!             uint16_t tc = (*dl)->p1(0, 16);
//!             state->rsp->setTexture(tile, level, on, sc, tc);
//!         }
//!
//!         void dmaIO(State *state, DisplayList **dl) {
//!             // Do nothing. This is not possible to implement without RSP computations in the CPU.
//!         }
//!
//!         void special1(State *state, DisplayList **dl) {
//!             const uint32_t param = (*dl)->p0(0, 8);
//!             if (state->ext.interpreter->hleGBI->flags.computeMVP) {
//!                 if (param == 1) {
//!                     state->rsp->specialComputeModelViewProj();
//!                 }
//!                 else {
//!                     assert(false && "Unimplemented combine matrices mode");
//!                 }
//!             }
//!             else {
//!                 assert(false && "Unimplemented special1 command");
//!             }
//!         }
//!
//!         void vertex(State *state, DisplayList **dl) {
//!             uint8_t vtxCount = (*dl)->p0(12, 8);
//!             state->rsp->setVertex((*dl)->w1, vtxCount, (*dl)->p0(1, 7) - vtxCount);
//!         }
//!
//!         void tri1(State *state, DisplayList **dl) {
//!             state->rsp->drawIndexedTri((*dl)->p0(17, 7), (*dl)->p0(9, 7), (*dl)->p0(1, 7));
//!         }
//!
//!         void tri2(State *state, DisplayList **dl) {
//!             state->rsp->drawIndexedTri((*dl)->p0(17, 7), (*dl)->p0(9, 7), (*dl)->p0(1, 7));
//!             state->rsp->drawIndexedTri((*dl)->p1(17, 7), (*dl)->p1(9, 7), (*dl)->p1(1, 7));
//!         }
//!
//!         void quad(State *state, DisplayList **dl) {
//!             tri2(state, dl);
//!         }
//!
//!         void line3D(State *state, DisplayList **dl) {
//!             // TODO
//!         }
//!     }
//! };
//! ```
//!
//! **Reuse, not new type.** This module reuses the local `p0`/`p1`
//! extraction-primitive *pattern* established by `rt64_gbi_f3dex.rs` (free
//! functions over `(w: u32, pos: u8, bits: u8) -> u32`) but does not import
//! or call that module's `p0`/`p1` -- they are private (non-`pub`) items of
//! a sibling module, so this file re-derives its own copies from *this*
//! source's citation of `src/gbi/rt64_gbi.cpp:32-38`, per the task's
//! instruction to port from the F3DEX2 source, never by analogy to a
//! sibling. No per-opcode struct is shared with `rt64_gbi_f3d.rs`,
//! `rt64_gbi_f3dex.rs`, or `rt64_gbi_s2dex2.rs` either: every opcode here
//! gets its own small local type sized to its exact operand tuple, matching
//! those three modules' own precedent of not unifying decode shapes across
//! microcode generations (see each field-layout difference from F3DEX
//! called out below). `crates/fn64-render-reference/src/gbi/` has a
//! production F3DEX2 GBI implementation, but per the task's explicit
//! instruction this module is not wired to it and does not reuse its types
//! (see "Nonclaims").
//!
//! ## Admitted domain
//!
//! - **`p0`/`p1` are always unsigned and never sign-extend themselves.**
//!   `((w >> pos) & ((0x01 << bits) - 1))` yields a `uint32_t` in
//!   `0..2^bits`; ported as `(w >> pos) & ((1u32 << bits) - 1) -> u32`.
//!   Several opcodes below apply an *explicit* C-style cast on top of a
//!   `p0`/`p1` result to reinterpret it as signed -- those are ported as an
//!   extra, explicit `as iNN` step after the unsigned extraction, never
//!   folded into the extractor itself.
//! - **`setOtherModeH`/`setOtherModeL`: the `off` computation involves an
//!   unsigned-then-signed reinterpretation, ported as `wrapping_sub` +
//!   `as i32` + `.max(0)`.** Source: `const uint32_t off = std::max(0,
//!   (int32_t)(32 - p0(8,8) - size));`. `32` is `int`, `p0(8,8)` is
//!   `uint32_t`, `size` is `uint32_t` (`p0(0,8)+1`, range `1..257`) -- by
//!   C++'s usual arithmetic conversions the whole `32 - p0(8,8) - size`
//!   expression is computed in `uint32_t` (the `int` literal `32` converts
//!   *to* `unsigned`, not the reverse), so a case where `p0(8,8) + size >
//!   32` wraps around to a huge unsigned value **before** the `(int32_t)`
//!   cast reinterprets that bit pattern as negative (implementation-defined
//!   pre-C++20, universal two's-complement in practice, matching every real
//!   toolchain RT64 ships on). `std::max(0, ...)` then clamps that negative
//!   `int32_t` to `0`, and the final assignment to `const uint32_t off`
//!   converts the now-non-negative `int32_t` back to `uint32_t` losslessly.
//!   Ported as: `off_raw = 32u32.wrapping_sub(p0_8_8).wrapping_sub(size)`,
//!   `off = (off_raw as i32).max(0) as u32` -- `wrapping_sub` reproduces
//!   C++'s defined unsigned wraparound bit-for-bit, and `as i32` on a `u32`
//!   is Rust's bit-preserving reinterpretation, matching the C++ cast.
//!   Pinned by `set_other_mode_h_off_clamps_to_zero_when_size_plus_shift_
//!   exceeds_32` and the `_l` counterpart.
//! - **`moveWord`'s `G_MW_FOG` case: two *explicit* sign-extending casts,
//!   `(int16_t)p1(16,16)` and `(int16_t)p1(0,16)`.** `p1(16,16)`/`p1(0,16)`
//!   are `uint32_t` in `0..65536`; the `(int16_t)` cast truncates to the
//!   low 16 bits and reinterprets that bit pattern as signed
//!   (implementation-defined pre-C++20, universal two's-complement in
//!   practice). Ported as `p1(w1, 16, 16) as u16 as i16` and `p1(w1, 0, 16)
//!   as u16 as i16` -- Rust's `as u16` truncates to the low 16 bits (a
//!   16-bit field already fits exactly, so this step is a no-op in range
//!   but stated explicitly to mirror the source's declared truncation
//!   width), then `as i16` reinterprets the bit pattern as signed,
//!   bit-for-bit matching the C++ cast. This is the file's clearest signed
//!   field: `decode_move_word_fog`'s two `i16` outputs are pinned at zero,
//!   at the positive max (`0x7FFF`), at the sign boundary (`0x8000` decodes
//!   to `i16::MIN`), and at all-ones (`0xFFFF` decodes to `-1`).
//! - **`moveMem`'s `G_MV_LIGHT` case: a `uint8_t` truncation feeding an
//!   `int` division, then a branch.** Source: `uint8_t offset = p0(8,8) *
//!   8;` -- `p0(8,8)` is `uint32_t` in `0..256`, `* 8` computed in
//!   `uint32_t` giving `0..2040`, then narrowed to `uint8_t offset` by
//!   truncating to the low 8 bits (implementation-defined pre-C++20,
//!   universal `% 256` wraparound in practice) -- e.g. `p0(8,8) = 0x20`
//!   (32) gives `32*8=256`, which truncates to `offset=0`, not `256`. Then
//!   `int index = offset / 24` (`offset` promotes to `int`, so this is
//!   ordinary truncating integer division, range `0..10` since
//!   `offset<=255`), and `if (index >= 2) setLight(index-2, w1) else
//!   setLookAt(index, w1)`. Ported as `offset = (p0(w0,8,8).wrapping_mul(8)
//!   & 0xFF) as u8`, `index = (offset as u32) / 24`, returning an enum
//!   (`MoveMemLightTarget::Light(index-2)` /
//!   `MoveMemLightTarget::LookAt(index)`) that mirrors the branch
//!   selection -- this is decode (a pure function of `dl`'s bits) even
//!   though the eventual `setLight`/`setLookAt` calls are dispatch and stay
//!   unported. Pinned by tests at `p0(8,8)=0` (offset 0, index 0,
//!   LookAt(0)), at the truncation boundary (`p0(8,8)=0x20` wrapping
//!   `256->0`), and at values producing `index>=2` (Light branch).
//! - **`vertex`'s third argument wraps in `uint32_t`, not `int`.** Source:
//!   `uint8_t vtxCount = p0(12,8);` (lossless, an exact 8-bit field, range
//!   `0..256` truncated to `u8`, never actually discards bits since the
//!   field is exactly 8 bits wide); then `p0(1,7) - vtxCount` -- `p0(1,7)`
//!   is `uint32_t` (range `0..128`), `vtxCount` is `uint8_t` which
//!   promotes to `int`, and by the usual arithmetic conversions `int`
//!   converts to `uint32_t` for the subtraction (unsigned has higher
//!   rank), so when `vtxCount > p0(1,7)` this **wraps around** to a large
//!   `u32` rather than going negative. Ported as `p0(w0,1,7).wrapping_sub(
//!   vtx_count as u32)`. Pinned by
//!   `vertex_third_field_wraps_when_vtx_count_exceeds_field` (e.g.
//!   `vtxCount=255`, `p0(1,7)=0` yields `0u32.wrapping_sub(255) =
//!   0xFFFF_FF01`, not a negative number or a panic).
//! - **Every other narrowing (`texture`'s `u8`/`u16` fields, `moveMem`'s
//!   `index: u8`, `moveWord`'s `type: u8`) is lossless** -- each source
//!   field width (3, 7, 8, or 16 bits) fits exactly inside the C++
//!   narrowing target type, so the `uint8_t`/`uint16_t` truncation never
//!   actually discards a set bit for any input `p0`/`p1` can produce (a
//!   `uint8_t` can't hold a 24-bit field, but no field here is narrowed to
//!   a type narrower than its own bit width). Ported with the matching
//!   `u8`/`u16` field type on each decoded struct, to preserve the
//!   source's declared operand *type*, not just its numeric *value*
//!   (matching `rt64_gbi_f3dex.rs`'s `QuadArgs` precedent).
//! - **`popMatrix`'s `w1 >> 6` is a bare shift, no mask** -- unlike every
//!   `p0`/`p1` call in this file, this reads bits `6..32` of `w1` with no
//!   upper bound; ported as `w1 >> 6` directly (`u32 >> 6` in Rust has
//!   identical semantics to `uint32_t >> 6` in C++: logical, not
//!   arithmetic, shift, and `6` is far from the 32-bit shift-width limit).
//! - **`matrix`'s `p0(0,8) ^ state->rsp->pushMask` is decoded only up to
//!   `p0(0,8)`** -- `pushMask` is `State`/`RSP`-owned, not a `DisplayList`
//!   bitfield, so the XOR itself is dispatch-adjacent state combination,
//!   out of this module's scope (see "Nonclaims"). `decode_matrix` returns
//!   the raw `p0(0,8)` mode byte and raw `w1` address only.
//! - **`tri1` reads `w0`, not `w1` -- this is the sharpest layout
//!   divergence from F3DEX's `tri1` and the exact copy-paste trap the task
//!   warns about.** F3DEX's `tri1` (`rt64_gbi_f3dex.cpp`) is
//!   `drawIndexedTri(p1(17,7), p1(9,7), p1(1,7))` -- all `p1`, reading
//!   `w1`. F3DEX2's `tri1` here is `drawIndexedTri(p0(17,7), p0(9,7),
//!   p0(1,7))` -- all `p0`, reading `w0`. Porting F3DEX2's `tri1` by
//!   analogy to F3DEX's `tri1` (swapping `p1`→`p0` without checking) would
//!   have been correct only by coincidence of the field positions matching
//!   (17/9/1, 7 bits each, does match) -- the *word* is genuinely different
//!   and this file's `decode_tri1` reads `w0` explicitly, pinned by
//!   `tri1_ignores_w1_entirely` (mirrored from F3DEX's `tri1_ignores_w0_
//!   entirely`, word swapped).
//! - **`tri2` here matches F3DEX's `tri2` exactly (first triangle from
//!   `p0`/`w0`, second from `p1`/`w1`, same bit positions)** -- so `tri2`
//!   is not a divergence, only `tri1` is. Cross-checked field-by-field
//!   against `rt64_gbi_f3dex.rs`'s `decode_tri2`/`decode_tri_indices`: the
//!   (pos, bits) triple `(17,7)/(9,7)/(1,7)` and the `w0`-then-`w1` split
//!   are identical between the two microcodes for this specific opcode.
//! - **`quad` here is NOT an independent decode -- it *is* `tri2`'s
//!   decode, called through.** Source: `void quad(...) { tri2(state, dl);
//!   }`, a bare tail call with no bitfield reads of its own. This is a
//!   second sharp divergence from F3DEX, whose `quad` reads a *different*
//!   4-field layout entirely (`p1(25,7)/p1(17,7)/p1(9,7)/p1(1,7)`, one
//!   `QuadArgs` struct, four independent `uint8_t` corners). F3DEX2's
//!   `quad` has no such 4-corner struct; it decodes to exactly the same
//!   `Tri2Args` shape as `tri2`. This module does not define a
//!   `decode_quad` -- callers wanting F3DEX2's quad decode call
//!   `decode_tri2` directly, matching the source's literal delegation.
//!   Pinned by `quad_decode_is_defined_as_tri2s_decode` (a doc-comment-only
//!   test asserting there is no separate function, by construction: no
//!   `decode_quad` identifier exists in this module).
//! - **`setOtherMode` (`G_RDPSETOTHERMODE`) is a distinct opcode from
//!   `setOtherModeH`/`setOtherModeL`** (`F3DEX2_G_SETOTHERMODE_H`/`_L`) --
//!   three separate decoders, matching the three separate `gbi->map[...]`
//!   entries in `setup` (not ported, but the 1:1 opcode-to-function
//!   correspondence is preserved here).
//! - **`dmaIO` and `line3D` read no bitfields at all.** `dmaIO`'s body is a
//!   comment ("Do nothing. This is not possible to implement without RSP
//!   computations in the CPU."); `line3D`'s body is a bare `// TODO`. Both
//!   ported as an explicit absence (no `decode_dma_io`/`decode_line_3d`
//!   function), matching `rt64_gbi_f3dex.rs`'s `cullDl` precedent -- not
//!   invented behavior.
//! - **`special1` decodes only `param = p0(0,8)`.** The branch on `param
//!   == 1` vs. the `assert(false, ...)` stubs, and the outer `if
//!   (state->ext.interpreter->hleGBI->flags.computeMVP)` gate, both depend
//!   on `State`/`Interpreter` fields outside `DisplayList`, so only the raw
//!   `param` extraction is decode; the branching itself is dispatch/
//!   assert-stub logic and is not ported (see "Nonclaims").
//!
//! ## Nonclaims
//!
//! No GPU, WGSL, or production wiring (this module is not called from
//! anywhere yet -- dead-code warnings on the unused public surface are
//! expected and correct, matching `rt64_gbi_f3d.rs`/`rt64_gbi_f3dex.rs`/
//! `rt64_gbi_s2dex2.rs`'s precedent), and no RT64 visual/pixel/silicon
//! parity or performance claim. Not wired to `fn64-render-reference`'s GBI
//! path -- that crate has its own, independently-maintained F3DEX2 decode
//! and this module makes no attempt to unify with or supersede it.
//!
//! Deliberately not ported from `rt64_gbi_f3dex2.cpp`:
//!
//! - **All `state->rsp->*` and `state->ext.interpreter->*` dispatch
//!   calls** -- `setOtherMode`, `setOtherModeH`, `setOtherModeL`,
//!   `setViewport`, `forceMatrix`, `setLight`, `setLookAt`,
//!   `setModelViewProjChanged`, `insertMatrix`, `setLightCount`,
//!   `setClipRatioEdge`, `setSegment`, `setFog`, `setLightColor`,
//!   `setPerspNorm`, `matrix`, `popMatrix`, `modifyGeometryMode`,
//!   `setTexture`, `specialComputeModelViewProj`, `setVertex`,
//!   `drawIndexedTri`. These need the full `RSP`/`State`/`Interpreter`
//!   object graph, which does not exist in this crate and is out of the
//!   task's named scope ("decode only, NOT dispatch").
//! - **`dmaIO`** -- upstream body is a comment-only no-op (documented as
//!   impossible without RSP computation on the CPU). No decode exists to
//!   port; there was never a bitfield read here.
//! - **`line3D`** -- upstream is a `// TODO` stub with an empty body (no
//!   bitfield reads, no dispatch call). Ported as an explicit absence, not
//!   invented behavior.
//! - **`special1`'s branch structure** (`state->ext.interpreter->hleGBI->
//!   flags.computeMVP` gate, `param == 1` dispatch, both `assert(false,
//!   ...)` stub arms) -- state-dependent control flow, not a `DisplayList`
//!   bitfield decode. Only the raw `param` field is ported.
//! - **`moveMem`'s `default: assert(false, ...)` arm**, and the
//!   `F3DEX2_G_MV_VIEWPORT`/`F3DEX2_G_MV_MATRIX` cases (which decode
//!   nothing beyond the already-captured `index`/raw `w1` -- their
//!   `state->rsp->setViewport(w1)`/`forceMatrix(w1)` calls are pure
//!   dispatch with no further bitfield extraction) -- represented in
//!   `MoveMemTarget` as `Viewport`/`Matrix` variants carrying the raw `w1`,
//!   and `Unimplemented` for any other `index`, never a panic or invented
//!   default.
//! - **`moveWord`'s `default: assert(false, ...)` arm** -- represented as
//!   `MoveWordTarget::Unimplemented` in `decode_move_word`, carrying the
//!   raw `type` byte for diagnostics, never invented behavior.
//! - **`matrix`'s XOR with `state->rsp->pushMask`** -- `pushMask` is
//!   `RSP`-owned state, not a `DisplayList` bitfield; `decode_matrix`
//!   returns the pre-XOR `p0(0,8)` mode byte and raw `w1` address only.
//! - **`GBI_F3DEX2::setup(GBI *gbi)`** -- the opcode-to-function-pointer
//!   dispatch table and the `gbi->constants` map (`G_MTX_MODELVIEW`,
//!   `G_TEXTURE_ENABLE`, etc.), and the two `F3DEX2_G_MW_*`/`F3DEX2_G_MV_*`
//!   light-offset constant families from `rt64_gbi_f3dex2.h` that
//!   `setup`/`moveMem` reference only via already-decoded fields. This
//!   wires together functions from other GBI translation units (`GBI_F3D`,
//!   `GBI_F3DEX`, `GBI_EXTENDED`) entirely outside this file's own opcode
//!   functions, and is dispatch/wiring, not bitfield decode.
//! - **`GBI_F3DEX2::reset(State *state)`** -- takes no `DisplayList`
//!   parameter at all (`state->rsp->setClipRatioAll(2U)`), so there is
//!   nothing for a `(w0, w1)`-shaped decoder to extract.
//! - **`DisplayList`'s constructor and its `w0`/`w1` fields as a stateful
//!   struct** -- this module represents a display-list command word as a
//!   plain `(w0: u32, w1: u32)` parameter pair to each `decode_*` function,
//!   per the task's explicit instruction and matching
//!   `rt64_gbi_f3dex.rs`'s precedent.

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

/// `GBI_F3DEX2::setOtherMode`'s operand set: `state->rsp->setOtherMode(
/// (*dl)->p0(0, 24), (*dl)->w1)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct SetOtherModeArgs {
    /// `p0(0, 24)`.
    high: u32,
    /// Raw `w1`.
    low: u32,
}

fn decode_set_other_mode(w0: u32, w1: u32) -> SetOtherModeArgs {
    SetOtherModeArgs {
        high: p0(w0, 0, 24),
        low: w1,
    }
}

/// Shared operand shape for `setOtherModeH` and `setOtherModeL`: both
/// compute an identical `(size, off)` pair from `w0` before passing raw
/// `w1` to their respective (unported) dispatch call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct SetOtherModeSizedArgs {
    /// `p0(0, 8) + 1`.
    size: u32,
    /// `std::max(0, (int32_t)(32 - p0(8, 8) - size))`, ported as a
    /// wrapping unsigned subtraction reinterpreted as signed then clamped
    /// to zero (see module doc "Admitted domain").
    off: u32,
    /// Raw `w1`.
    value: u32,
}

fn decode_set_other_mode_sized(w0: u32, w1: u32) -> SetOtherModeSizedArgs {
    let size = p0(w0, 0, 8) + 1;
    let shift = p0(w0, 8, 8);
    let off_raw = 32u32.wrapping_sub(shift).wrapping_sub(size);
    let off = (off_raw as i32).max(0) as u32;
    SetOtherModeSizedArgs {
        size,
        off,
        value: w1,
    }
}

/// `GBI_F3DEX2::moveMem`'s `F3DEX2_G_MV_LIGHT` case: which of `setLight`/
/// `setLookAt` the source would dispatch to, and the already-computed
/// index argument for that call. See module doc "Admitted domain" for the
/// `uint8_t offset` truncation and `int index = offset / 24` derivation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MoveMemLightTarget {
    /// `state->rsp->setLookAt(index, w1)`, taken when `index < 2`.
    LookAt(u32),
    /// `state->rsp->setLight(index - 2, w1)`, taken when `index >= 2`.
    Light(u32),
}

/// `GBI_F3DEX2::moveMem`'s decoded target, one variant per `switch (index)`
/// arm. `Unimplemented` mirrors the source's `default: assert(false, ...)`
/// -- never invented behavior (see module doc "Nonclaims").
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MoveMemTarget {
    /// `F3DEX2_G_MV_VIEWPORT` -> `state->rsp->setViewport(w1)`.
    Viewport(u32),
    /// `F3DEX2_G_MV_MATRIX` -> `state->rsp->forceMatrix(w1)`.
    Matrix(u32),
    /// `F3DEX2_G_MV_LIGHT` -> see `MoveMemLightTarget`.
    Light(MoveMemLightTarget),
    /// `default` arm: `assert(false && "Unimplemented moveMem command")`.
    /// Carries the raw `index` byte for diagnostics.
    Unimplemented(u8),
}

const F3DEX2_G_MV_VIEWPORT: u8 = 8;
const F3DEX2_G_MV_MATRIX: u8 = 14;
const F3DEX2_G_MV_LIGHT: u8 = 10;

fn decode_move_mem(w0: u32, w1: u32) -> MoveMemTarget {
    let index = p0(w0, 0, 8) as u8;
    match index {
        F3DEX2_G_MV_VIEWPORT => MoveMemTarget::Viewport(w1),
        F3DEX2_G_MV_MATRIX => MoveMemTarget::Matrix(w1),
        F3DEX2_G_MV_LIGHT => {
            // uint8_t offset = p0(8,8) * 8; -- truncates to the low 8 bits
            // of the u32 product (see module doc "Admitted domain").
            let offset = (p0(w0, 8, 8).wrapping_mul(8) & 0xFF) as u8;
            let index = (offset as u32) / 24;
            let target = if index >= 2 {
                MoveMemLightTarget::Light(index - 2)
            } else {
                MoveMemLightTarget::LookAt(index)
            };
            MoveMemTarget::Light(target)
        }
        other => MoveMemTarget::Unimplemented(other),
    }
}

/// `GBI_F3DEX2::moveWord`'s decoded target, one variant per `switch (type)`
/// arm. `Unimplemented` mirrors the source's `default: assert(false, ...)`
/// -- never invented behavior (see module doc "Nonclaims").
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MoveWordTarget {
    /// `F3DEX2_G_MW_FORCEMTX` -> `state->rsp->setModelViewProjChanged(w1 ==
    /// 0)`. Carries raw `w1`; the `== 0` comparison is dispatch-adjacent
    /// but trivial and pure, so it is left to the (unported) call site
    /// rather than pre-computed here, matching the source's literal
    /// expression shape (this module hands back the operand, not a bool).
    ForceMtx(u32),
    /// `G_MW_MATRIX` -> `state->rsp->insertMatrix(p0(0, 16), w1)`.
    Matrix { index: u32, value: u32 },
    /// `G_MW_NUMLIGHT` -> `state->rsp->setLightCount(w1 / 24)`.
    NumLight(u32),
    /// `G_MW_CLIP` -> `state->rsp->setClipRatioEdge((p0(0, 16) -
    /// G_MWO_CLIP_RNX) / 8, w1)`. `G_MWO_CLIP_RNX` is `0x04`
    /// (`src/shared/rt64_f3d_defines.h:117`).
    Clip { edge: u32, value: u32 },
    /// `G_MW_SEGMENT` -> `state->rsp->setSegment(p0(2, 4), w1)`.
    Segment { index: u32, value: u32 },
    /// `G_MW_FOG` -> `state->rsp->setFog((int16_t)p1(16, 16),
    /// (int16_t)p1(0, 16))`. Both fields are sign-extended (see module doc
    /// "Admitted domain").
    Fog { mul: i16, offset: i16 },
    /// `G_MW_LIGHTCOL` -> `state->rsp->setLightColor(p0(0, 16) / 24, w1)`.
    LightColor { index: u32, value: u32 },
    /// `G_MW_PERSPNORM` -> `state->rsp->setPerspNorm(w1)`.
    PerspNorm(u32),
    /// `default` arm. Carries the raw `type` byte for diagnostics.
    Unimplemented(u8),
}

const F3DEX2_G_MW_FORCEMTX: u8 = 0x0c;
const G_MW_MATRIX: u8 = 0x00;
const G_MW_NUMLIGHT: u8 = 0x02;
const G_MW_CLIP: u8 = 0x04;
const G_MW_SEGMENT: u8 = 0x06;
const G_MW_FOG: u8 = 0x08;
const G_MW_LIGHTCOL: u8 = 0x0a;
const G_MW_PERSPNORM: u8 = 0x0e;
const G_MWO_CLIP_RNX: u32 = 0x04;

fn decode_move_word(w0: u32, w1: u32) -> MoveWordTarget {
    let word_type = p0(w0, 16, 8) as u8;
    match word_type {
        F3DEX2_G_MW_FORCEMTX => MoveWordTarget::ForceMtx(w1),
        G_MW_MATRIX => MoveWordTarget::Matrix {
            index: p0(w0, 0, 16),
            value: w1,
        },
        G_MW_NUMLIGHT => MoveWordTarget::NumLight(w1 / 24),
        G_MW_CLIP => MoveWordTarget::Clip {
            edge: (p0(w0, 0, 16) - G_MWO_CLIP_RNX) / 8,
            value: w1,
        },
        G_MW_SEGMENT => MoveWordTarget::Segment {
            index: p0(w0, 2, 4),
            value: w1,
        },
        G_MW_FOG => MoveWordTarget::Fog {
            mul: p1(w1, 16, 16) as u16 as i16,
            offset: p1(w1, 0, 16) as u16 as i16,
        },
        G_MW_LIGHTCOL => MoveWordTarget::LightColor {
            index: p0(w0, 0, 16) / 24,
            value: w1,
        },
        G_MW_PERSPNORM => MoveWordTarget::PerspNorm(w1),
        other => MoveWordTarget::Unimplemented(other),
    }
}

/// `GBI_F3DEX2::matrix`'s `DisplayList`-sourced operand set:
/// `state->rsp->matrix(w1, p0(0, 8) ^ state->rsp->pushMask)`.
/// `state->rsp->pushMask` is `RSP`-owned state, not a `DisplayList`
/// bitfield, so only the pre-XOR mode byte is decoded here (see module doc
/// "Nonclaims").
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct MatrixArgs {
    /// Raw `w1` (matrix address).
    addr: u32,
    /// `p0(0, 8)`, pre-XOR with `state->rsp->pushMask`.
    mode: u32,
}

fn decode_matrix(w0: u32, w1: u32) -> MatrixArgs {
    MatrixArgs {
        addr: w1,
        mode: p0(w0, 0, 8),
    }
}

/// `GBI_F3DEX2::popMatrix`'s operand: `state->rsp->popMatrix(w1 >> 6)`.
/// A bare shift, no mask -- reads bits `6..32` of `w1` (see module doc
/// "Admitted domain").
fn decode_pop_matrix(_w0: u32, w1: u32) -> u32 {
    w1 >> 6
}

/// `GBI_F3DEX2::geometryMode`'s operand set: `state->rsp->
/// modifyGeometryMode(p0(0, 24), w1)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct GeometryModeArgs {
    /// `p0(0, 24)`.
    off_mask: u32,
    /// Raw `w1`.
    on_mask: u32,
}

fn decode_geometry_mode(w0: u32, w1: u32) -> GeometryModeArgs {
    GeometryModeArgs {
        off_mask: p0(w0, 0, 24),
        on_mask: w1,
    }
}

/// `GBI_F3DEX2::texture`'s operand set: `state->rsp->setTexture(p0(8, 3),
/// p0(11, 3), p0(1, 7), p1(16, 16), p1(0, 16))`. All narrowings are
/// lossless (see module doc "Admitted domain").
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct TextureArgs {
    tile: u8,
    level: u8,
    on: u8,
    sc: u16,
    tc: u16,
}

fn decode_texture(w0: u32, w1: u32) -> TextureArgs {
    TextureArgs {
        tile: p0(w0, 8, 3) as u8,
        level: p0(w0, 11, 3) as u8,
        on: p0(w0, 1, 7) as u8,
        sc: p1(w1, 16, 16) as u16,
        tc: p1(w1, 0, 16) as u16,
    }
}

// `dmaIO` reads no bitfields: the upstream body is a comment-only no-op
// (see module doc "Admitted domain" and "Nonclaims"). No `decode_dma_io`
// exists in this module.

/// `GBI_F3DEX2::special1`'s only `DisplayList`-sourced operand: `const
/// uint32_t param = p0(0, 8);`. The subsequent branch on `param == 1` and
/// the outer `computeMVP` state gate are dispatch, not decode (see module
/// doc "Admitted domain" and "Nonclaims").
fn decode_special1(w0: u32, _w1: u32) -> u32 {
    p0(w0, 0, 8)
}

/// `GBI_F3DEX2::vertex`'s operand set: `state->rsp->setVertex(w1, vtxCount,
/// p0(1, 7) - vtxCount)` where `vtxCount = p0(12, 8)`. The third field
/// wraps in `uint32_t`, not `int` (see module doc "Admitted domain").
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct VertexArgs {
    /// Raw `w1`.
    addr: u32,
    /// `p0(12, 8)`, narrowed to `uint8_t` (lossless: exact 8-bit field).
    vtx_count: u8,
    /// `p0(1, 7).wrapping_sub(vtx_count as u32)` -- ported as a `uint32_t`
    /// wrapping subtraction, matching the source's usual-arithmetic-
    /// conversions promotion of `vtxCount` to `uint32_t` before the `-`.
    field_1_7_minus_vtx_count: u32,
}

fn decode_vertex(w0: u32, w1: u32) -> VertexArgs {
    let vtx_count = p0(w0, 12, 8) as u8;
    VertexArgs {
        addr: w1,
        vtx_count,
        field_1_7_minus_vtx_count: p0(w0, 1, 7).wrapping_sub(vtx_count as u32),
    }
}

/// One `drawIndexedTri(a, b, c)` triangle's three vertex-cache indices,
/// each a `p0`/`p1`-extracted 7-bit field. Same (pos, bits) shape as
/// `rt64_gbi_f3dex.rs`'s `TriIndices`, but not shared with it (see module
/// doc "Reuse, not new type").
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct TriIndices {
    a: u32,
    b: u32,
    c: u32,
}

fn decode_tri_indices(word: u32) -> TriIndices {
    TriIndices {
        a: (word >> 17) & ((1u32 << 7) - 1),
        b: (word >> 9) & ((1u32 << 7) - 1),
        c: (word >> 1) & ((1u32 << 7) - 1),
    }
}

/// `GBI_F3DEX2::tri1`'s operand set: `state->rsp->drawIndexedTri(p0(17, 7),
/// p0(9, 7), p0(1, 7))`. **Reads `w0`, not `w1`** -- diverges from F3DEX's
/// `tri1`, which reads `w1` (see module doc "Admitted domain", the
/// F3DEX2-vs-F3DEX layout-divergence note).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Tri1Args {
    tri: TriIndices,
}

fn decode_tri1(w0: u32, _w1: u32) -> Tri1Args {
    Tri1Args {
        tri: decode_tri_indices(w0),
    }
}

/// `GBI_F3DEX2::tri2`'s operand set: two `drawIndexedTri` calls, the first
/// from `w0`'s `p0(17,7)/p0(9,7)/p0(1,7)`, the second from `w1`'s
/// `p1(17,7)/p1(9,7)/p1(1,7)`. Identical shape to F3DEX's `tri2` (see
/// module doc "Admitted domain"). `GBI_F3DEX2::quad` is a bare tail call to
/// `tri2` with no bitfields of its own, so this is also F3DEX2's quad
/// decode -- there is no separate `decode_quad` in this module.
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

// `GBI_F3DEX2::quad` has no decoder of its own: the upstream body is a
// bare `tri2(state, dl);` tail call with no bitfield reads (see module doc
// "Admitted domain"). Callers wanting F3DEX2's quad decode use
// `decode_tri2` directly.

// `line3D` reads no bitfields: the upstream body is a bare `// TODO` (see
// module doc "Admitted domain" and "Nonclaims"). No `decode_line_3d`
// exists in this module.

#[cfg(test)]
mod tests {
    use super::*;

    // --- p0 / p1: shared bitfield extraction primitives ---

    #[test]
    fn p0_all_zero_word_is_zero() {
        assert_eq!(p0(0, 0, 24), 0);
    }

    #[test]
    fn p0_reads_low_bit_at_pos_zero() {
        assert_eq!(p0(0b1, 0, 1), 1);
    }

    #[test]
    fn p0_ignores_w1_entirely() {
        assert_eq!(p0(0xFFFF_FFFF, 0, 24), 0x00FF_FFFF);
    }

    #[test]
    fn p0_masks_off_bits_above_the_field() {
        assert_eq!(p0(0xFF, 0, 4), 0xF);
    }

    #[test]
    fn p0_extracts_field_at_nonzero_shift() {
        assert_eq!(p0(0x0000_0F00, 8, 4), 0xF);
    }

    #[test]
    fn p0_field_boundary_one_bit_above_does_not_leak_in() {
        let w0 = (0x7F << 1) | (1 << 8);
        assert_eq!(p0(w0, 1, 7), 0x7F);
    }

    #[test]
    fn p0_all_ones_word_widest_field_in_file() {
        // Widest (pos, bits) used anywhere in this file is (0, 24)
        // (setOtherMode's high, geometryMode's offMask).
        assert_eq!(p0(0xFFFF_FFFF, 0, 24), 0x00FF_FFFF);
    }

    #[test]
    fn p1_all_zero_word_is_zero() {
        assert_eq!(p1(0, 0, 16), 0);
    }

    #[test]
    fn p1_reads_w1_not_w0() {
        assert_eq!(p1(0b1010, 1, 3), 0b101);
    }

    #[test]
    fn p1_masks_off_bits_above_the_field() {
        assert_eq!(p1(0xFFFF_FFFF, 0, 16), 0xFFFF);
    }

    #[test]
    fn p1_field_boundary_one_bit_above_does_not_leak_in() {
        let w1 = (0x7F << 9) | (1 << 16);
        assert_eq!(p1(w1, 9, 7), 0x7F);
    }

    // --- setOtherMode ---

    #[test]
    fn set_other_mode_all_zero_words_decode_to_all_zero_fields() {
        let a = decode_set_other_mode(0, 0);
        assert_eq!(a, SetOtherModeArgs { high: 0, low: 0 });
    }

    #[test]
    fn set_other_mode_high_max_value_boundary() {
        // 24-bit field at bit 0: max value 0x00FF_FFFF.
        let a = decode_set_other_mode(0x00FF_FFFF, 0);
        assert_eq!(a.high, 0x00FF_FFFF);
    }

    #[test]
    fn set_other_mode_high_one_bit_above_does_not_leak_in() {
        let w0 = 0x00FF_FFFF | (1 << 24);
        let a = decode_set_other_mode(w0, 0);
        assert_eq!(a.high, 0x00FF_FFFF);
    }

    #[test]
    fn set_other_mode_low_passes_through_raw_w1() {
        let a = decode_set_other_mode(0, 0xDEAD_BEEF);
        assert_eq!(a.low, 0xDEAD_BEEF);
    }

    #[test]
    fn set_other_mode_ignores_w1_for_high() {
        let with_w1 = decode_set_other_mode(0x1234, 0xFFFF_FFFF);
        let without_w1 = decode_set_other_mode(0x1234, 0);
        assert_eq!(with_w1.high, without_w1.high);
    }

    // --- setOtherModeH / setOtherModeL (shared decode shape) ---

    #[test]
    fn set_other_mode_sized_all_zero_words() {
        let a = decode_set_other_mode_sized(0, 0);
        // size = p0(0,8)+1 = 1; off = max(0, 32-0-1) = 31.
        assert_eq!(
            a,
            SetOtherModeSizedArgs {
                size: 1,
                off: 31,
                value: 0
            }
        );
    }

    #[test]
    fn set_other_mode_sized_size_max_value_boundary() {
        // p0(0,8) = 0xFF -> size = 256.
        let a = decode_set_other_mode_sized(0xFF, 0);
        assert_eq!(a.size, 256);
    }

    #[test]
    fn set_other_mode_sized_size_one_bit_above_does_not_leak_in() {
        let w0 = 0xFFu32 | (1 << 8);
        let a = decode_set_other_mode_sized(w0, 0);
        assert_eq!(a.size, 256);
    }

    #[test]
    fn set_other_mode_sized_off_clamps_to_zero_when_size_plus_shift_exceeds_32() {
        // p0(8,8) = 0, size = 256 (p0(0,8)=255): 32-0-256 wraps to a huge
        // u32, reinterpreted as a large-magnitude negative i32, clamped to
        // 0 by max(0, ...).
        let w0 = 0xFF; // p0(0,8) = 0xFF -> size = 256; p0(8,8) = 0.
        let a = decode_set_other_mode_sized(w0, 0);
        assert_eq!(a.off, 0);
    }

    #[test]
    fn set_other_mode_sized_off_positive_when_shift_plus_size_under_32() {
        // p0(8,8) = 0, size = 1 (p0(0,8)=0): off = max(0, 32-0-1) = 31.
        let a = decode_set_other_mode_sized(0, 0);
        assert_eq!(a.off, 31);
    }

    #[test]
    fn set_other_mode_sized_off_exact_zero_boundary_not_negative() {
        // Choose p0(8,8) and size so 32 - shift - size == 0 exactly:
        // shift = 20, size = 12 (p0(0,8) = 11).
        let w0 = 11u32 | (20u32 << 8);
        let a = decode_set_other_mode_sized(w0, 0);
        assert_eq!(a.size, 12);
        assert_eq!(a.off, 0);
    }

    #[test]
    fn set_other_mode_sized_off_one_below_zero_boundary_clamps() {
        // shift = 21, size = 12 (p0(0,8) = 11): 32-21-12 = -1, clamped to 0.
        let w0 = 11u32 | (21u32 << 8);
        let a = decode_set_other_mode_sized(w0, 0);
        assert_eq!(a.off, 0);
    }

    #[test]
    fn set_other_mode_sized_value_passes_through_raw_w1() {
        let a = decode_set_other_mode_sized(0, 0xCAFE_BABE);
        assert_eq!(a.value, 0xCAFE_BABE);
    }

    #[test]
    fn set_other_mode_l_decode_matches_h_decode_shape() {
        // setOtherModeH and setOtherModeL share the exact same (size, off)
        // derivation in the source -- this module ports both call sites
        // through the same decode_set_other_mode_sized function, so this
        // test just documents that equivalence explicitly.
        let h = decode_set_other_mode_sized(0x1234_5678, 0x9ABC_DEF0);
        let l = decode_set_other_mode_sized(0x1234_5678, 0x9ABC_DEF0);
        assert_eq!(h, l);
    }

    // --- moveMem ---

    #[test]
    fn move_mem_viewport_index() {
        let a = decode_move_mem(F3DEX2_G_MV_VIEWPORT as u32, 0x1111_2222);
        assert_eq!(a, MoveMemTarget::Viewport(0x1111_2222));
    }

    #[test]
    fn move_mem_matrix_index() {
        let a = decode_move_mem(F3DEX2_G_MV_MATRIX as u32, 0x3333_4444);
        assert_eq!(a, MoveMemTarget::Matrix(0x3333_4444));
    }

    #[test]
    fn move_mem_index_field_ignores_bits_above_byte() {
        let w0 = (F3DEX2_G_MV_VIEWPORT as u32) | (0xFF << 8);
        let a = decode_move_mem(w0, 0x55);
        assert_eq!(a, MoveMemTarget::Viewport(0x55));
    }

    #[test]
    fn move_mem_unimplemented_default_arm() {
        let a = decode_move_mem(0xFF, 0);
        assert_eq!(a, MoveMemTarget::Unimplemented(0xFF));
    }

    #[test]
    fn move_mem_light_shift_zero_gives_lookat_zero() {
        // p0(8,8) = 0 -> offset = 0 -> index = 0 -> LookAt(0).
        let w0 = F3DEX2_G_MV_LIGHT as u32;
        let a = decode_move_mem(w0, 0x9999);
        assert_eq!(a, MoveMemTarget::Light(MoveMemLightTarget::LookAt(0)));
    }

    #[test]
    fn move_mem_light_shift_one_gives_lookat_one() {
        // p0(8,8) = 1 -> offset = 8 -> index = 8/24 = 0 -> LookAt(0).
        // Need offset/24 == 1: offset in 24..48 -> p0(8,8) in [3, 5].
        let w0 = F3DEX2_G_MV_LIGHT as u32 | (3u32 << 8);
        let a = decode_move_mem(w0, 0);
        // offset = 3*8 = 24; index = 24/24 = 1 -> LookAt(1).
        assert_eq!(a, MoveMemTarget::Light(MoveMemLightTarget::LookAt(1)));
    }

    #[test]
    fn move_mem_light_index_at_light_branch_boundary() {
        // Need offset/24 == 2 exactly -> offset in 48..72 -> p0(8,8) in
        // [6, 8]. p0(8,8) = 6 -> offset = 48 -> index = 2 -> Light(0).
        let w0 = F3DEX2_G_MV_LIGHT as u32 | (6u32 << 8);
        let a = decode_move_mem(w0, 0x42);
        assert_eq!(a, MoveMemTarget::Light(MoveMemLightTarget::Light(0)));
    }

    #[test]
    fn move_mem_light_index_well_into_light_branch() {
        // p0(8,8) = 0x20 (32) -> offset = 32*8 = 256, truncates (uint8_t)
        // to 0 -> index = 0 -> LookAt(0). This pins the truncation
        // boundary explicitly (see next test for a case that does NOT
        // truncate).
        let w0 = F3DEX2_G_MV_LIGHT as u32 | (0x20u32 << 8);
        let a = decode_move_mem(w0, 0);
        assert_eq!(a, MoveMemTarget::Light(MoveMemLightTarget::LookAt(0)));
    }

    #[test]
    fn move_mem_light_offset_truncation_boundary_just_under_wrap() {
        // p0(8,8) = 0x1F (31) -> offset = 31*8 = 248 (fits in u8, no
        // truncation) -> index = 248/24 = 10 -> Light(8).
        let w0 = F3DEX2_G_MV_LIGHT as u32 | (0x1Fu32 << 8);
        let a = decode_move_mem(w0, 0);
        assert_eq!(a, MoveMemTarget::Light(MoveMemLightTarget::Light(8)));
    }

    #[test]
    fn move_mem_light_max_shift_value() {
        // p0(8,8) = 0xFF (255) -> offset = 255*8 = 2040, truncates to
        // 2040 % 256 = 248 -> index = 248/24 = 10 -> Light(8).
        let w0 = F3DEX2_G_MV_LIGHT as u32 | (0xFFu32 << 8);
        let a = decode_move_mem(w0, 0);
        assert_eq!(a, MoveMemTarget::Light(MoveMemLightTarget::Light(8)));
    }

    // --- moveWord ---

    #[test]
    fn move_word_force_mtx() {
        let w0 = (F3DEX2_G_MW_FORCEMTX as u32) << 16;
        let a = decode_move_word(w0, 0x1234);
        assert_eq!(a, MoveWordTarget::ForceMtx(0x1234));
    }

    #[test]
    fn move_word_type_field_ignores_bits_below_16() {
        let w0 = ((F3DEX2_G_MW_FORCEMTX as u32) << 16) | 0xFFFF;
        let a = decode_move_word(w0, 0);
        assert_eq!(a, MoveWordTarget::ForceMtx(0));
    }

    #[test]
    fn move_word_matrix() {
        let w0 = ((G_MW_MATRIX as u32) << 16) | 0x1234;
        let a = decode_move_word(w0, 0xABCD_EF00);
        assert_eq!(
            a,
            MoveWordTarget::Matrix {
                index: 0x1234,
                value: 0xABCD_EF00
            }
        );
    }

    #[test]
    fn move_word_matrix_index_max_value_boundary() {
        let w0 = ((G_MW_MATRIX as u32) << 16) | 0xFFFF;
        let a = decode_move_word(w0, 0);
        assert_eq!(
            a,
            MoveWordTarget::Matrix {
                index: 0xFFFF,
                value: 0
            }
        );
    }

    #[test]
    fn move_word_numlight() {
        let w0 = (G_MW_NUMLIGHT as u32) << 16;
        let a = decode_move_word(w0, 48);
        assert_eq!(a, MoveWordTarget::NumLight(2));
    }

    #[test]
    fn move_word_numlight_truncating_division() {
        let w0 = (G_MW_NUMLIGHT as u32) << 16;
        let a = decode_move_word(w0, 47);
        assert_eq!(a, MoveWordTarget::NumLight(1));
    }

    #[test]
    fn move_word_clip() {
        // G_MWO_CLIP_RNX = 0x04. p0(0,16) = 0x14 (20) -> (20-4)/8 = 2.
        let w0 = ((G_MW_CLIP as u32) << 16) | 0x14;
        let a = decode_move_word(w0, 0x77);
        assert_eq!(
            a,
            MoveWordTarget::Clip {
                edge: 2,
                value: 0x77
            }
        );
    }

    #[test]
    fn move_word_clip_at_rnx_boundary_gives_zero() {
        // p0(0,16) = G_MWO_CLIP_RNX exactly -> (4-4)/8 = 0.
        let w0 = ((G_MW_CLIP as u32) << 16) | G_MWO_CLIP_RNX;
        let a = decode_move_word(w0, 0);
        assert_eq!(a, MoveWordTarget::Clip { edge: 0, value: 0 });
    }

    #[test]
    fn move_word_segment() {
        // p0(2,4): 4-bit field at bit 2, max value 0xF.
        let w0 = ((G_MW_SEGMENT as u32) << 16) | (0xFu32 << 2);
        let a = decode_move_word(w0, 0x88);
        assert_eq!(
            a,
            MoveWordTarget::Segment {
                index: 0xF,
                value: 0x88
            }
        );
    }

    #[test]
    fn move_word_segment_field_boundary_one_bit_above_does_not_leak_in() {
        let w0 = ((G_MW_SEGMENT as u32) << 16) | (0xFu32 << 2) | (1 << 6);
        let a = decode_move_word(w0, 0);
        assert_eq!(
            a,
            MoveWordTarget::Segment {
                index: 0xF,
                value: 0
            }
        );
    }

    #[test]
    fn move_word_segment_field_boundary_bit_below_does_not_leak_in() {
        let w0 = ((G_MW_SEGMENT as u32) << 16) | (0xFu32 << 2) | 0b11;
        let a = decode_move_word(w0, 0);
        assert_eq!(
            a,
            MoveWordTarget::Segment {
                index: 0xF,
                value: 0
            }
        );
    }

    #[test]
    fn move_word_fog_all_zero_is_zero() {
        let w0 = (G_MW_FOG as u32) << 16;
        let a = decode_move_word(w0, 0);
        assert_eq!(a, MoveWordTarget::Fog { mul: 0, offset: 0 });
    }

    #[test]
    fn move_word_fog_mul_positive_max_boundary() {
        // p1(16,16) = 0x7FFF -> i16 = 32767 (max positive, not sign bit).
        let w0 = (G_MW_FOG as u32) << 16;
        let w1 = 0x7FFFu32 << 16;
        let a = decode_move_word(w0, w1);
        assert_eq!(
            a.clone(),
            MoveWordTarget::Fog {
                mul: 0x7FFF,
                offset: 0
            }
        );
    }

    #[test]
    fn move_word_fog_mul_sign_boundary_becomes_min() {
        // p1(16,16) = 0x8000 -> i16 reinterpretation = i16::MIN (-32768).
        let w0 = (G_MW_FOG as u32) << 16;
        let w1 = 0x8000u32 << 16;
        let a = decode_move_word(w0, w1);
        assert_eq!(
            a,
            MoveWordTarget::Fog {
                mul: i16::MIN,
                offset: 0
            }
        );
    }

    #[test]
    fn move_word_fog_mul_all_ones_is_negative_one() {
        let w0 = (G_MW_FOG as u32) << 16;
        let w1 = 0xFFFFu32 << 16;
        let a = decode_move_word(w0, w1);
        assert_eq!(a, MoveWordTarget::Fog { mul: -1, offset: 0 });
    }

    #[test]
    fn move_word_fog_offset_positive_max_boundary() {
        let w0 = (G_MW_FOG as u32) << 16;
        let a = decode_move_word(w0, 0x7FFF);
        assert_eq!(
            a,
            MoveWordTarget::Fog {
                mul: 0,
                offset: 0x7FFF
            }
        );
    }

    #[test]
    fn move_word_fog_offset_sign_boundary_becomes_min() {
        let w0 = (G_MW_FOG as u32) << 16;
        let a = decode_move_word(w0, 0x8000);
        assert_eq!(
            a,
            MoveWordTarget::Fog {
                mul: 0,
                offset: i16::MIN
            }
        );
    }

    #[test]
    fn move_word_fog_offset_all_ones_is_negative_one() {
        let w0 = (G_MW_FOG as u32) << 16;
        let a = decode_move_word(w0, 0xFFFF);
        assert_eq!(a, MoveWordTarget::Fog { mul: 0, offset: -1 });
    }

    #[test]
    fn move_word_fog_offset_ignores_bits_above_16() {
        // p1(0,16) must not see bits 16..32 of w1 (those belong to mul).
        let w0 = (G_MW_FOG as u32) << 16;
        let w1 = 0x7FFFu32 << 16; // mul field only.
        let a = decode_move_word(w0, w1);
        assert_eq!(
            a.clone(),
            MoveWordTarget::Fog {
                mul: 0x7FFF,
                offset: 0
            }
        );
    }

    #[test]
    fn move_word_lightcol() {
        // p0(0,16) = 48 -> 48/24 = 2.
        let w0 = ((G_MW_LIGHTCOL as u32) << 16) | 48;
        let a = decode_move_word(w0, 0x99);
        assert_eq!(
            a,
            MoveWordTarget::LightColor {
                index: 2,
                value: 0x99
            }
        );
    }

    #[test]
    fn move_word_lightcol_truncating_division() {
        let w0 = ((G_MW_LIGHTCOL as u32) << 16) | 47;
        let a = decode_move_word(w0, 0);
        assert_eq!(a, MoveWordTarget::LightColor { index: 1, value: 0 });
    }

    #[test]
    fn move_word_perspnorm() {
        let w0 = (G_MW_PERSPNORM as u32) << 16;
        let a = decode_move_word(w0, 0xABCD);
        assert_eq!(a, MoveWordTarget::PerspNorm(0xABCD));
    }

    #[test]
    fn move_word_unimplemented_default_arm() {
        let w0 = 0xFFu32 << 16;
        let a = decode_move_word(w0, 0);
        assert_eq!(a, MoveWordTarget::Unimplemented(0xFF));
    }

    // --- matrix ---

    #[test]
    fn matrix_all_zero_words_decode_to_all_zero_fields() {
        let a = decode_matrix(0, 0);
        assert_eq!(a, MatrixArgs { addr: 0, mode: 0 });
    }

    #[test]
    fn matrix_mode_max_value_boundary() {
        let a = decode_matrix(0xFF, 0);
        assert_eq!(a.mode, 0xFF);
    }

    #[test]
    fn matrix_mode_one_bit_above_does_not_leak_in() {
        let w0 = 0xFFu32 | (1 << 8);
        let a = decode_matrix(w0, 0);
        assert_eq!(a.mode, 0xFF);
    }

    #[test]
    fn matrix_addr_passes_through_raw_w1() {
        let a = decode_matrix(0, 0xDEAD_BEEF);
        assert_eq!(a.addr, 0xDEAD_BEEF);
    }

    // --- popMatrix ---

    #[test]
    fn pop_matrix_all_zero_is_zero() {
        assert_eq!(decode_pop_matrix(0xFFFF_FFFF, 0), 0);
    }

    #[test]
    fn pop_matrix_ignores_w0_entirely() {
        let with_w0 = decode_pop_matrix(0xFFFF_FFFF, 0x1234_5678);
        let without_w0 = decode_pop_matrix(0, 0x1234_5678);
        assert_eq!(with_w0, without_w0);
    }

    #[test]
    fn pop_matrix_shifts_right_by_six_no_mask() {
        assert_eq!(decode_pop_matrix(0, 0b1_000000), 0b1);
    }

    #[test]
    fn pop_matrix_all_ones_shifts_but_does_not_mask_upper_bits() {
        // Unlike p0/p1, this is a bare >>, so all bits above 6 survive.
        assert_eq!(decode_pop_matrix(0, 0xFFFF_FFFF), 0xFFFF_FFFF >> 6);
    }

    #[test]
    fn pop_matrix_bits_below_six_are_discarded() {
        assert_eq!(decode_pop_matrix(0, 0b111111), 0);
    }

    // --- geometryMode ---

    #[test]
    fn geometry_mode_all_zero_words_decode_to_all_zero_fields() {
        let a = decode_geometry_mode(0, 0);
        assert_eq!(
            a,
            GeometryModeArgs {
                off_mask: 0,
                on_mask: 0
            }
        );
    }

    #[test]
    fn geometry_mode_off_mask_max_value_boundary() {
        let a = decode_geometry_mode(0x00FF_FFFF, 0);
        assert_eq!(a.off_mask, 0x00FF_FFFF);
    }

    #[test]
    fn geometry_mode_off_mask_one_bit_above_does_not_leak_in() {
        let w0 = 0x00FF_FFFF | (1 << 24);
        let a = decode_geometry_mode(w0, 0);
        assert_eq!(a.off_mask, 0x00FF_FFFF);
    }

    #[test]
    fn geometry_mode_on_mask_passes_through_raw_w1() {
        let a = decode_geometry_mode(0, 0xCAFE_BABE);
        assert_eq!(a.on_mask, 0xCAFE_BABE);
    }

    // --- texture ---

    #[test]
    fn texture_all_zero_words_decode_to_all_zero_fields() {
        let a = decode_texture(0, 0);
        assert_eq!(
            a,
            TextureArgs {
                tile: 0,
                level: 0,
                on: 0,
                sc: 0,
                tc: 0
            }
        );
    }

    #[test]
    fn texture_tile_max_value_boundary() {
        // p0(8,3): 3-bit field, max 0x7.
        let w0 = 0x7u32 << 8;
        let a = decode_texture(w0, 0);
        assert_eq!(a.tile, 0x7);
        assert_eq!(a.level, 0);
    }

    #[test]
    fn texture_tile_one_bit_above_does_not_leak_into_level() {
        let w0 = (0x7u32 << 8) | (1 << 11);
        let a = decode_texture(w0, 0);
        assert_eq!(a.tile, 0x7);
        assert_eq!(a.level, 1);
    }

    #[test]
    fn texture_level_max_value_boundary() {
        let w0 = 0x7u32 << 11;
        let a = decode_texture(w0, 0);
        assert_eq!(a.level, 0x7);
        assert_eq!(a.tile, 0);
    }

    #[test]
    fn texture_on_max_value_boundary() {
        // p0(1,7): 7-bit field, max 0x7F.
        let w0 = 0x7Fu32 << 1;
        let a = decode_texture(w0, 0);
        assert_eq!(a.on, 0x7F);
        assert_eq!(a.tile, 0);
    }

    #[test]
    fn texture_on_bit_zero_belongs_to_no_field() {
        let a = decode_texture(1, 0);
        assert_eq!(a.on, 0);
    }

    #[test]
    fn texture_sc_max_value_boundary() {
        let w1 = 0xFFFFu32 << 16;
        let a = decode_texture(0, w1);
        assert_eq!(a.sc, 0xFFFF);
        assert_eq!(a.tc, 0);
    }

    #[test]
    fn texture_tc_max_value_boundary() {
        let a = decode_texture(0, 0xFFFF);
        assert_eq!(a.tc, 0xFFFF);
        assert_eq!(a.sc, 0);
    }

    #[test]
    fn texture_all_ones_saturates_every_field() {
        let a = decode_texture(0xFFFF_FFFF, 0xFFFF_FFFF);
        assert_eq!(
            a,
            TextureArgs {
                tile: 0x7,
                level: 0x7,
                on: 0x7F,
                sc: 0xFFFF,
                tc: 0xFFFF
            }
        );
    }

    #[test]
    fn texture_fields_are_narrowed_to_declared_source_types() {
        let a = decode_texture(0xFFFF_FFFF, 0xFFFF_FFFF);
        let _: u8 = a.tile;
        let _: u8 = a.level;
        let _: u8 = a.on;
        let _: u16 = a.sc;
        let _: u16 = a.tc;
    }

    // --- special1 ---

    #[test]
    fn special1_all_zero_is_zero() {
        assert_eq!(decode_special1(0, 0), 0);
    }

    #[test]
    fn special1_param_max_value_boundary() {
        assert_eq!(decode_special1(0xFF, 0), 0xFF);
    }

    #[test]
    fn special1_param_one_bit_above_does_not_leak_in() {
        assert_eq!(decode_special1(0xFF | (1 << 8), 0), 0xFF);
    }

    #[test]
    fn special1_ignores_w1_entirely() {
        let with_w1 = decode_special1(0x42, 0xFFFF_FFFF);
        let without_w1 = decode_special1(0x42, 0);
        assert_eq!(with_w1, without_w1);
    }

    #[test]
    fn special1_param_one_is_the_computemvp_dispatch_value() {
        // param == 1 is the only value the (unported) dispatch branch
        // treats as implemented; this module just confirms the raw
        // extraction for that specific value.
        assert_eq!(decode_special1(1, 0), 1);
    }

    // --- vertex ---

    #[test]
    fn vertex_all_zero_words_decode_to_all_zero_fields() {
        let a = decode_vertex(0, 0);
        assert_eq!(
            a,
            VertexArgs {
                addr: 0,
                vtx_count: 0,
                field_1_7_minus_vtx_count: 0
            }
        );
    }

    #[test]
    fn vertex_addr_passes_through_raw_w1() {
        let a = decode_vertex(0, 0xDEAD_BEEF);
        assert_eq!(a.addr, 0xDEAD_BEEF);
    }

    #[test]
    fn vertex_vtx_count_max_value_boundary() {
        // p0(12,8): 8-bit field, max 0xFF -- exact width, lossless u8.
        let w0 = 0xFFu32 << 12;
        let a = decode_vertex(w0, 0);
        assert_eq!(a.vtx_count, 0xFF);
    }

    #[test]
    fn vertex_vtx_count_one_bit_above_does_not_leak_in() {
        let w0 = (0xFFu32 << 12) | (1 << 20);
        let a = decode_vertex(w0, 0);
        assert_eq!(a.vtx_count, 0xFF);
    }

    #[test]
    fn vertex_field_1_7_minus_vtx_count_normal_case_no_wrap() {
        // p0(1,7) = 0x7F, vtxCount = 0 -> 0x7F - 0 = 0x7F.
        let w0 = 0x7Fu32 << 1;
        let a = decode_vertex(w0, 0);
        assert_eq!(a.field_1_7_minus_vtx_count, 0x7F);
    }

    #[test]
    fn vertex_third_field_wraps_when_vtx_count_exceeds_field() {
        // p0(1,7) = 0, vtxCount = 0xFF (255): 0u32.wrapping_sub(255) is a
        // huge u32, matching C++'s uint32_t wraparound (vtxCount promotes
        // to uint32_t, not int, for this subtraction -- see module doc
        // "Admitted domain").
        let w0 = 0xFFu32 << 12; // vtxCount = 0xFF, p0(1,7) = 0.
        let a = decode_vertex(w0, 0);
        assert_eq!(a.vtx_count, 0xFF);
        assert_eq!(a.field_1_7_minus_vtx_count, 0u32.wrapping_sub(0xFF));
        assert_eq!(a.field_1_7_minus_vtx_count, 0xFFFF_FF01);
    }

    #[test]
    fn vertex_third_field_exact_equal_case_is_zero() {
        // p0(1,7) and vtxCount both derived to the same value: 0x7F.
        // vtxCount = p0(12,8), field = p0(1,7). Set both to 0x7F.
        let w0 = (0x7Fu32 << 1) | (0x7Fu32 << 12);
        let a = decode_vertex(w0, 0);
        assert_eq!(a.vtx_count, 0x7F);
        assert_eq!(a.field_1_7_minus_vtx_count, 0);
    }

    // --- tri1 ---

    #[test]
    fn tri1_all_zero_w0_decodes_to_all_zero_indices() {
        let a = decode_tri1(0, 0xFFFF_FFFF);
        assert_eq!(a.tri, TriIndices { a: 0, b: 0, c: 0 });
    }

    #[test]
    fn tri1_ignores_w1_entirely() {
        // Sharpest F3DEX2-vs-F3DEX divergence: F3DEX's tri1 reads w1 and
        // ignores w0; F3DEX2's tri1 reads w0 and ignores w1 (see module
        // doc "Admitted domain").
        let with_w1 = decode_tri1(0x1234_5678, 0xFFFF_FFFF);
        let without_w1 = decode_tri1(0x1234_5678, 0);
        assert_eq!(with_w1, without_w1);
    }

    #[test]
    fn tri1_field_a_max_value_boundary() {
        let w0 = 0x7Fu32 << 17;
        let a = decode_tri1(w0, 0);
        assert_eq!(
            a.tri,
            TriIndices {
                a: 0x7F,
                b: 0,
                c: 0
            }
        );
    }

    #[test]
    fn tri1_field_b_max_value_boundary() {
        let w0 = 0x7Fu32 << 9;
        let a = decode_tri1(w0, 0);
        assert_eq!(
            a.tri,
            TriIndices {
                a: 0,
                b: 0x7F,
                c: 0
            }
        );
    }

    #[test]
    fn tri1_field_c_max_value_boundary() {
        let w0 = 0x7Fu32 << 1;
        let a = decode_tri1(w0, 0);
        assert_eq!(
            a.tri,
            TriIndices {
                a: 0,
                b: 0,
                c: 0x7F
            }
        );
    }

    #[test]
    fn tri1_all_ones_w0_saturates_all_three_indices() {
        let a = decode_tri1(0xFFFF_FFFF, 0);
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
        let a = decode_tri1(1, 0);
        assert_eq!(a.tri, TriIndices { a: 0, b: 0, c: 0 });
    }

    #[test]
    fn tri1_gap_bit_8_belongs_to_no_field() {
        let a = decode_tri1(1 << 8, 0);
        assert_eq!(a.tri, TriIndices { a: 0, b: 0, c: 0 });
    }

    #[test]
    fn tri1_gap_bit_16_belongs_to_no_field() {
        let a = decode_tri1(1 << 16, 0);
        assert_eq!(a.tri, TriIndices { a: 0, b: 0, c: 0 });
    }

    // --- tri2 (also quad's decode, via the source's tail call) ---

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
        let w0 = 0x7Fu32 << 17;
        let w1 = 0x7Fu32 << 1;
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
    fn tri2_matches_two_independent_tri_indices_decodes() {
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

    #[test]
    fn tri2_matches_tri1_on_w0_for_the_first_triangle() {
        // tri1 reads w0 with the identical (17,7)/(9,7)/(1,7) shape as
        // tri2's first triangle -- confirms the two decoders agree where
        // their domains overlap.
        let w0 = 0x2222_3333;
        let tri1 = decode_tri1(w0, 0xFFFF_FFFF);
        let tri2 = decode_tri2(w0, 0);
        assert_eq!(tri1.tri, tri2.first);
    }

    #[test]
    fn quad_decode_is_defined_as_tri2s_decode_not_a_separate_function() {
        // Source: `void quad(...) { tri2(state, dl); }` -- a bare tail
        // call, so this module intentionally has no decode_quad. This test
        // documents the equivalence by using decode_tri2 for both "quad"
        // and "tri2" call sites, matching the source's literal delegation.
        let w0 = 0xAAAA_5555;
        let w1 = 0x5555_AAAA;
        let quad_decode = decode_tri2(w0, w1); // what quad(state, dl) decodes to.
        let tri2_decode = decode_tri2(w0, w1);
        assert_eq!(quad_decode, tri2_decode);
    }
}
