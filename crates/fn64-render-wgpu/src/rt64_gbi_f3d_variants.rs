//! Literal port of RT64's four small F3D **variant** microcodes' display-list
//! command-word **bitfield decoding**, from the permitted MIT RT64 Rust-port
//! source pinned at commit `5473732a822a4423b5696e7cb18fecc425a59875`
//! (`docs/RT64-PORT-AUTHORITY.md`), across eight files:
//!
//! - `src/gbi/rt64_gbi_f3dwave.cpp` (SHA-256 of the whole file,
//!   `61f782c275629dff2396d04a165f2bc6fb60afdc3c196987ad8cb65198c9351e`)
//! - `src/gbi/rt64_gbi_f3dwave.h` (SHA-256 of the whole file,
//!   `67c3bea4d31ce677a078e703d80b835746bf58ce8d5a16b4306d033e439b3f66`)
//! - `src/gbi/rt64_gbi_f3dgolden.cpp` (SHA-256 of the whole file,
//!   `b88269da4b6cad03191e7ac5907ad0d94468d6e28223bb673b96a3ec3c658dc4`)
//! - `src/gbi/rt64_gbi_f3dgolden.h` (SHA-256 of the whole file,
//!   `b0406fb0c09192a915693b9b0b07512f4e0ecb075af459948f75d4c2fd032c54`)
//! - `src/gbi/rt64_gbi_f3dpd.cpp` (SHA-256 of the whole file,
//!   `140ef3fd7c057416117980beb41a6fe1107ab29d8bdfdfc7a914f19c14abad95`)
//! - `src/gbi/rt64_gbi_f3dpd.h` (SHA-256 of the whole file,
//!   `0e6801473e2700ab40779b8d4ef942249ddaff4e14600455efb160962026195e`)
//! - `src/gbi/rt64_gbi_f3dzex2.cpp` (SHA-256 of the whole file,
//!   `1b77ef084c4c99900f133d5ff1ac2817c9aaa1c1771354c6763ad42f7aed4a44`)
//! - `src/gbi/rt64_gbi_f3dzex2.h` (SHA-256 of the whole file,
//!   `dffb548ee77062b3a5f811f0bdf4514e9c4fb4901096864d7122bed4779bc174`)
//!
//! All eight digests were cross-checked byte-for-byte against
//! `docs/rt64-port-inventory.json`'s `files[].sources.port.sha256` entries
//! for these exact paths and match.
//!
//! Only the **bitfield extraction and per-opcode operand layout** is
//! ported: `DisplayList::p0`/`p1` (from `src/gbi/rt64_gbi.cpp`, cited below)
//! plus the operand set each opcode function reads from `(*dl)->w0`/`w1` via
//! those two extractors. The `state->rsp->someCall(...)` dispatch itself is
//! NOT ported -- it needs the whole `RSP`/`State` object graph, out of
//! scope (see "Nonclaims"). Each variant's `setup(GBI *gbi)` -- the
//! opcode-to-function dispatch table, which for all four begins by calling
//! a base microcode's `setup` (`GBI_F3D::setup` for F3DWAVE/F3DGOLDEN/
//! F3DPD, `GBI_F3DEX2::setup` for F3DZEX2) -- is also not ported, for the
//! same reason; see "Nonclaims" for the literal delegation each variant
//! makes and which functions each variant's `setup` reassigns.
//!
//! `src/gbi/rt64_gbi_l3dex2.cpp`/`.h` (`GBI_L3DEX2`) are DELIBERATELY
//! EXCLUDED from this module: its only function, `line3D`, is a bare
//! `assert(false);` with no bitfield read at all (mirroring
//! `rt64_gbi_f3dex2.rs`'s own `line3D` non-port). Porting it would mean
//! inventing behavior upstream does not have.
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
//! // src/gbi/rt64_gbi_f3dwave.cpp, lines 12-45
//! namespace RT64 {
//!     namespace GBI_F3DWAVE {
//!         void vertex(State *state, DisplayList **dl) {
//!             state->rsp->setVertex((*dl)->w1, (*dl)->p0(9, 7), (*dl)->p0(16, 8) / 5);
//!         }
//!
//!         void tri1(State *state, DisplayList **dl) {
//!             state->rsp->drawIndexedTri((*dl)->p1(16, 8) / 5, (*dl)->p1(8, 8) / 5, (*dl)->p1(0, 8) / 5);
//!         }
//!
//!         void tri2(State *state, DisplayList **dl) {
//!             state->rsp->drawIndexedTri((*dl)->p0(16, 8) / 5, (*dl)->p0(8, 8) / 5, (*dl)->p0(0, 8) / 5);
//!             state->rsp->drawIndexedTri((*dl)->p1(16, 8) / 5, (*dl)->p1(8, 8) / 5, (*dl)->p1(0, 8) / 5);
//!         }
//!
//!         void quad(State *state, DisplayList **dl) {
//!             const uint8_t v0 = (*dl)->p1(24, 8) / 5;
//!             const uint8_t v1 = (*dl)->p1(16, 8) / 5;
//!             const uint8_t v2 = (*dl)->p1(8, 8) / 5;
//!             const uint8_t v3 = (*dl)->p1(0, 8) / 5;
//!             state->rsp->drawIndexedTri(v0, v1, v2);
//!             state->rsp->drawIndexedTri(v0, v2, v3);
//!         }
//!
//!         void setup(GBI *gbi) {
//!             GBI_F3D::setup(gbi);
//!
//!             gbi->map[F3DWAVE_G_UNKNOWN] = nullptr; // FIXME: Replaces a function set by base F3D with nothing until it's figured out.
//!             gbi->map[F3DWAVE_G_RDPHALF_1] = &GBI_F3D::rdpHalf1;
//!             gbi->map[F3DWAVE_G_RDPHALF_2] = &GBI_F3D::rdpHalf2;
//!             gbi->map[F3D_G_VTX] = vertex;
//!             gbi->map[F3D_G_TRI1] = tri1;
//!             gbi->map[F3DWAVE_G_TRI2] = &tri2;
//!             gbi->map[F3D_G_QUAD] = quad;
//!         }
//!     }
//! };
//!
//! // src/gbi/rt64_gbi_f3dwave.h, lines 9-12
//! #define F3DWAVE_G_UNKNOWN       0xB4
//! #define F3DWAVE_G_RDPHALF_1     0xB3
//! #define F3DWAVE_G_RDPHALF_2     0xB2
//! #define F3DWAVE_G_TRI2          0xB1
//! ```
//!
//! ```text
//! // src/gbi/rt64_gbi_f3dgolden.cpp, lines 12-36
//! namespace RT64 {
//!     namespace GBI_F3DGOLDEN {
//!         void triX(State *state, DisplayList **dl) {
//!             uint32_t w0 = (*dl)->w0;
//!             uint32_t w1 = (*dl)->w1;
//!             while (w1 != 0) {
//!                 uint32_t v0 = w1 & 0xf;
//!                 w1 >>= 4;
//!
//!                 uint32_t v1 = w1 & 0xf;
//!                 w1 >>= 4;
//!
//!                 uint32_t v2 = w0 & 0xf;
//!                 w0 >>= 4;
//!
//!                 state->rsp->drawIndexedTri(v0, v1, v2);
//!             }
//!         }
//!
//!         void setup(GBI *gbi) {
//!             GBI_F3D::setup(gbi);
//!
//!             gbi->map[F3DGOLDEN_G_TRIX] = &triX;
//!             gbi->map[F3DGOLDEN_G_MOVEWORD] = GBI_F3D::moveWord;
//!         }
//!     }
//! };
//!
//! // src/gbi/rt64_gbi_f3dgolden.h, lines 9-10
//! #define F3DGOLDEN_G_MOVEWORD 0xBD
//! #define F3DGOLDEN_G_TRIX 0xB1
//! ```
//!
//! ```text
//! // src/gbi/rt64_gbi_f3dpd.cpp, lines 12-29
//! namespace RT64 {
//!     namespace GBI_F3DPD {
//!         void vertex(State *state, DisplayList **dl) {
//!             state->rsp->setVertexPD((*dl)->w1, (*dl)->p0(20, 4) + 1, (*dl)->p0(16, 4));
//!         }
//!
//!         void vertexColor(State *state, DisplayList **dl) {
//!             state->rsp->setVertexColorPD((*dl)->w1);
//!         }
//!
//!         void setup(GBI *gbi) {
//!             GBI_F3D::setup(gbi);
//!
//!             gbi->map[F3D_G_VTX] = &vertex;
//!             gbi->map[F3DPD_G_VTXCOLOR] = &vertexColor;
//!             gbi->map[F3DGOLDEN_G_TRIX] = &GBI_F3DGOLDEN::triX;
//!         }
//!     }
//! };
//!
//! // src/gbi/rt64_gbi_f3dpd.h, line 9
//! #define F3DPD_G_VTXCOLOR 0x07
//! ```
//!
//! ```text
//! // src/gbi/rt64_gbi_f3dzex2.cpp, lines 9-20
//! namespace RT64 {
//!     namespace GBI_F3DZEX2 {
//!         void branchW(State *state, DisplayList **dl) {
//!             state->rsp->branchW(state->microcode.half1, (*dl)->p0(1, 7), (*dl)->w1, dl);
//!         }
//!
//!         void setup(GBI *gbi) {
//!             GBI_F3DEX2::setup(gbi);
//!
//!             gbi->map[F3DZEX2_G_BRANCH_W] = &branchW;
//!         }
//!     }
//! };
//!
//! // src/gbi/rt64_gbi_f3dzex2.h, line 9
//! #define F3DZEX2_G_BRANCH_W 0x04
//! ```
//!
//! For comparison, base F3D's own `vertex`/`tri1`/`quad` (referenced
//! extensively below because F3DWAVE's whole delta is divisor/field-shape
//! changes on top of these) come from `src/gbi/rt64_gbi_f3d.cpp` lines
//! 72-74 and 93-102, already ported and cited in `rt64_gbi_f3d.rs`:
//!
//! ```text
//! void vertex(State *state, DisplayList **dl) {
//!     state->rsp->setVertex((*dl)->w1, (*dl)->p0(20, 4) + 1, (*dl)->p0(16, 4));
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
//! ```
//!
//! And F3DEX's `branchZ` (`src/gbi/rt64_gbi_f3dex.cpp`, referenced below
//! because it occupies the identical opcode slot, `0x04`, that F3DZEX2's
//! `branchW` overrides in `GBI_F3DEX2::setup`'s inherited map), already
//! ported and cited in `rt64_gbi_f3dex.rs`:
//!
//! ```text
//! void branchZ(State *state, DisplayList **dl) {
//!     state->rsp->branchZ(state->microcode.half1, (*dl)->p0(1, 11), (*dl)->w1, dl);
//! }
//! ```
//!
//! **Reuse, not new type.** This module reuses the local `p0`/`p1`
//! extraction-primitive *pattern* established by the four sibling modules
//! (free functions over `(w: u32, pos: u8, bits: u8) -> u32`) but does not
//! import or call any sibling's `p0`/`p1` -- they are private (non-`pub`)
//! items of their own translation units, so this file re-derives its own
//! copies from *this* citation of `src/gbi/rt64_gbi.cpp:32-38`, per the
//! task's instruction to port each variant from its own source file, never
//! by analogy. No per-opcode struct is imported from `rt64_gbi_f3d.rs`,
//! `rt64_gbi_f3dex.rs`, `rt64_gbi_f3dex2.rs`, or `rt64_gbi_s2dex2.rs`
//! either: every opcode here gets its own small local type sized to its
//! exact operand tuple, matching those four modules' precedent of not
//! unifying decode shapes across microcode generations -- which matters
//! doubly here, because (as detailed in "Admitted domain" below) three of
//! these four variants' decoders have field shapes that look like a base
//! microcode's but are NOT identical, and a shared type would have
//! papered over exactly the divergence this module exists to pin.
//!
//! ## Admitted domain
//!
//! - **`p0`/`p1` are always unsigned and never sign-extend.**
//!   `((w >> pos) & ((0x01 << bits) - 1))` yields a `uint32_t` in
//!   `0..2^bits`; ported as `(w >> pos) & ((1u32 << bits) - 1) -> u32`,
//!   identical to the four sibling modules.
//!
//! ### F3DWAVE
//!
//! - **`vertex` does NOT merely swap base F3D's divisor -- its count and
//!   dest-index fields sit at entirely different bit positions and
//!   widths.** Base F3D's `vertex` (cited above) is `setVertex(w1,
//!   p0(20,4)+1, p0(16,4))`: a 4-bit count field (biased `+1`, range
//!   `1..16`) at bit 20, and a 4-bit dest-index field at bit 16 (range
//!   `0..16`, no divisor at all). F3DWAVE's `vertex` is `setVertex(w1,
//!   p0(9,7), p0(16,8)/5)`: a 7-bit count field (no `+1` bias, range
//!   `0..128`) at bit **9**, and an 8-bit dest-index field at bit 16
//!   (range `0..256`) divided by **5**. Three independent divergences in
//!   one opcode: different bit position for the count field (20 vs 9),
//!   different width for both fields (4 vs 7, 4 vs 8), and a divisor on
//!   the dest-index field that base F3D's `vertex` does not have at all.
//!   Ported as `decode_vertex(w0, w1) -> VertexArgs { address: w1, count:
//!   p0(w0,9,7), dst_index: p0(w0,16,8) / 5 }`. Pinned by
//!   `vertex_count_field_reads_bit_9_not_bit_20_like_base_f3d` and
//!   `vertex_dst_index_has_no_plus_one_bias_unlike_base_f3d`.
//! - **`tri1`/`tri2`/`quad`: identical bit positions and widths to base
//!   F3D's `tri1`/`quad`, only the divisor changes (`/10` -> `/5`), and
//!   `tri1`/`quad` both read `w1` in both microcodes (`p1`).** Ported as
//!   `decode_tri1(w0, w1) -> TriArgs { a: p1(w1,16,8)/5, b: p1(w1,8,8)/5,
//!   c: p1(w1,0,8)/5 }` and `decode_quad` with the same four `p1(w1, ...)
//!   / 5` fields as base F3D's `quad` but divided by 5. Each field is a
//!   `u32` division (`uint8_t` result implicitly promoted to `int`/`u32`
//!   for the divide, then narrowed back to `uint8_t` -- lossless since
//!   `255/5 = 51` fits in a `u8`), so integer (truncating, toward zero)
//!   division on an unsigned range never differs between C++ and Rust
//!   here. Pinned by `tri1_divisor_is_five_not_ten` and
//!   `quad_divisor_is_five_not_ten`, each comparing against a hand-computed
//!   `/5` expectation (never against base F3D's `/10` output).
//! - **`tri2` is a NEW opcode, not an override of a base-F3D `tri2`.**
//!   Base F3D (`rt64_gbi_f3d.cpp`/`.h`) has no `tri2` function or
//!   `F3D_G_TRI2` constant at all -- only F3DEX and later introduce
//!   `tri2`. F3DWAVE's `tri2` is two back-to-back `TriArgs`-shaped reads,
//!   first from `w0` (`p0(16,8)/5, p0(8,8)/5, p0(0,8)/5`), second from
//!   `w1` (`p1(16,8)/5, p1(8,8)/5, p1(0,8)/5`) -- the same `(16,8)/(8,8)/
//!   (0,8)` triple as `tri1`'s, just applied to both words. Ported as
//!   `decode_tri2(w0, w1) -> Tri2Args { first: TriArgs, second: TriArgs }`
//!   built from two independent `decode_tri1`-shaped extractions. Pinned
//!   by `tri2_first_triangle_reads_w0_second_reads_w1`.
//! - **`F3DWAVE_G_TRI2` (`0xB1`) numerically collides with
//!   `F3DGOLDEN_G_TRIX` (`0xB1`) and `F3DEX_G_TRI2` (`0xB1`, in
//!   `rt64_gbi_f3dex.h`) -- same opcode byte, three unrelated decoders in
//!   three unrelated microcode families.** This module does not attempt
//!   to unify them; each variant's opcode constant is scoped to its own
//!   `mod`/section below and never compared for equality across variants.
//! - **`F3DWAVE_G_UNKNOWN` (`0xB4`) is mapped to `nullptr` in `setup`**
//!   (source comment: "Replaces a function set by base F3D with nothing
//!   until it's figured out") -- there is no decode function for it at
//!   all upstream, so this module defines none; the opcode constant is
//!   still ported (see below) since it is a named, sourced fact
//!   independent of whether a decoder exists for it.
//!
//! ### F3DGOLDEN
//!
//! - **`triX` is a variable-length loop over `w0`/`w1`, terminated by
//!   `w1 == 0`, that emits triangles by popping 4-bit nibbles.** Each
//!   iteration: `v0 = w1 & 0xf` then `w1 >>= 4`; `v1 = w1 & 0xf` then
//!   `w1 >>= 4` (so `v0`/`v1` are the two low nibbles of `w1` *before* that
//!   iteration's shifts, consumed low-nibble-first); `v2 = w0 & 0xf` then
//!   `w0 >>= 4` (the low nibble of `w0`, one nibble consumed per
//!   iteration, independent pace from `w1`'s two-nibbles-per-iteration
//!   consumption). The loop condition checks `w1` (post-shift value from
//!   the *previous* iteration, or the initial value on entry) -- `w0`
//!   reaching zero does NOT stop the loop; only `w1` does. Ported as
//!   `decode_tri_x(w0, w1) -> Vec<TriXIndices>` running the identical
//!   mutable-local-shift loop (`let mut w0 = w0; let mut w1 = w1; while
//!   w1 != 0 { ... }`), each `& 0xf` / `>>= 4` step ported as `& 0xF` /
//!   `>>= 4` on `u32` locals -- bit-for-bit identical unsigned masking and
//!   logical (not arithmetic) shift in both languages. Every emitted
//!   index is inherently in `0..16` (a 4-bit mask), so no field here can
//!   ever need the "one bit above max" boundary case a `p0`/`p1` field
//!   would. Pinned by `tri_x_terminates_on_w1_reaching_zero_even_if_w0_
//!   nonzero`, `tri_x_v0_v1_come_from_w1_v2_comes_from_w0` (the
//!   word-pinning test for this opcode, per the task's "pin which WORD"
//!   rule), `tri_x_two_iteration_count_and_nibble_order`, and
//!   `tri_x_all_zero_words_yields_empty_list`.
//! - **`triX` does NOT read via `p0`/`p1` at all -- it reads `w0`/`w1`
//!   directly with `& 0xf` and `>>= 4`, its own bespoke bit-extraction,
//!   not the shared `DisplayList::p0`/`p1` helpers.** This is stated
//!   explicitly because every other opcode in this module (and all four
//!   sibling modules) is `p0`/`p1`-shaped; `triX` is the one exception,
//!   ported as direct mask/shift on plain `u32` locals with no `p0`/`p1`
//!   call at all, matching the source's own choice not to use
//!   `DisplayList::p0`/`p1` here.
//!
//! ### F3DPD
//!
//! - **`vertex`'s field shape is numerically IDENTICAL to base F3D's
//!   `vertex`** -- both are `(w1, p0(20,4)+1, p0(16,4))`: same count-field
//!   position/width/bias, same dest-index-field position/width, no
//!   divisor in either. The only difference upstream is which RSP method
//!   receives these three values (`setVertexPD` vs `setVertex`), which is
//!   dispatch and out of scope here. Because the *decoded numbers* are
//!   identical to base F3D's `vertex`, this module does not define a
//!   separate `f3dpd::decode_vertex` -- `f3dwave`'s sibling-independent
//!   port already proved the divergent case; **F3DPD's port instead
//!   defines its own `decode_vertex` function reusing base F3D's exact
//!   (pos, bits, bias) triple, re-derived from THIS file's own reading of
//!   `rt64_gbi_f3dpd.cpp` line 15 (not copy-pasted from `rt64_gbi_f3d.rs`),
//!   to keep the "port from your own source" rule even when the numbers
//!   coincide.** Pinned by `f3dpd_vertex_matches_base_f3d_vertex_field_
//!   shape_numerically` -- an explicit, named test asserting the
//!   coincidence (not an assumption baked silently into shared code).
//! - **`vertexColor` reads no bitfields at all -- it forwards raw `w1`
//!   unchanged.** Source: `setVertexColorPD((*dl)->w1)`. Ported as
//!   `decode_vertex_color(w1) -> u32` returning `w1` verbatim (`w0` is
//!   read by neither `p0` nor any other means). Pinned by
//!   `vertex_color_returns_w1_unchanged_and_ignores_w0`.
//! - **F3DPD's `setup` reassigns `F3DGOLDEN_G_TRIX` to
//!   `GBI_F3DGOLDEN::triX` -- a literal cross-module delegation, not a
//!   distinct F3DPD decoder.** Source: `gbi->map[F3DGOLDEN_G_TRIX] =
//!   &GBI_F3DGOLDEN::triX;`. This module represents that fact as
//!   delegation, not a second implementation: F3DPD's triangle opcode
//!   decode IS `f3dgolden::decode_tri_x`, called directly; there is no
//!   `f3dpd::decode_tri_x`. Pinned by
//!   `f3dpd_trix_opcode_delegates_to_f3dgolden_decode_tri_x` (asserts
//!   `f3dgolden::decode_tri_x(w0, w1)` produces the answer F3DPD's mapped
//!   opcode would produce, by construction, since no separate function
//!   exists to diverge).
//!
//! ### F3DZEX2
//!
//! - **`branchW` occupies the SAME opcode slot F3DEX2 inherits from
//!   F3DEX for `branchZ` (`0x04`) but reads a DIFFERENT bit width at the
//!   same position -- the sharpest divergence in this module, and the
//!   exact class of trap the task warns about.** F3DEX's `branchZ`
//!   (`rt64_gbi_f3dex.cpp`, cited above) is `branchZ(half1, p0(1,11), w1,
//!   dl)` -- an 11-bit field at bit 1. F3DZEX2's `branchW` is
//!   `branchW(half1, p0(1,7), w1, dl)` -- a **7-bit** field at the same
//!   bit position, 1. Same start bit, four fewer bits of width, same
//!   opcode byte (`F3DZEX2_G_BRANCH_W == 0x04 ==
//!   F3DEX2_G_BRANCH_Z`, both from their respective headers), and
//!   `GBI_F3DZEX2::setup` explicitly overwrites the inherited
//!   `F3DEX2_G_BRANCH_Z` map entry with `branchW`'s function pointer at
//!   that slot. Ported as `decode_branch_w(w0, w1) -> BranchWArgs {
//!   vtx_index: p0(w0,1,7), w_value: w1 }`, deliberately NOT sharing a
//!   type with `rt64_gbi_f3dex.rs`'s `branchZ` decode (which reads 11
//!   bits, would silently truncate/misdocument if reused). Pinned by
//!   `branch_w_reads_seven_bits_not_elevens_bits_like_f3dex_branch_z` and
//!   `branch_w_bit_eight_above_seven_bit_field_is_masked_off` (the "one
//!   bit above max" masking-boundary test, at bit 8 -- which `branchZ`'s
//!   11-bit field would have kept but `branchW`'s 7-bit field masks away).
//! - **`branchW` also reads `w1` as a raw, unmasked 32-bit value (its
//!   third dispatch argument), identical in shape to `branchZ`'s own raw
//!   `w1` argument** -- so the `w1` half of this opcode is NOT a
//!   divergence, only the `p0(1, *)` width is. Pinned by
//!   `branch_w_carries_w1_unmasked_matching_branch_z_shape`.
//! - **`state->microcode.half1` is `State`-owned, not a `DisplayList`
//!   bitfield** -- `decode_branch_w` does not (and cannot) produce it;
//!   see "Nonclaims".
//!
//! ## Nonclaims
//!
//! No GPU, WGSL, or production wiring (this module is not called from
//! anywhere yet -- dead-code warnings on the unused public surface are
//! expected and correct, matching the four sibling modules' precedent),
//! and no RT64 visual/pixel/silicon parity or performance claim. Not
//! wired to `fn64-render-reference`'s GBI path -- that crate has its own,
//! independently-maintained GBI decode and this module makes no attempt to
//! unify with or supersede it.
//!
//! Deliberately not ported:
//!
//! - **All `state->rsp->*` dispatch calls** (`setVertex`, `setVertexPD`,
//!   `setVertexColorPD`, `drawIndexedTri`, `branchW`) and
//!   `state->microcode.half1` -- these need the full `RSP`/`State` object
//!   graph, out of the task's named scope ("decode only, NOT dispatch").
//! - **`GBI_F3DWAVE::setup`, `GBI_F3DGOLDEN::setup`, `GBI_F3DPD::setup`,
//!   `GBI_F3DZEX2::setup`** -- each variant's opcode-to-function dispatch
//!   table, and each one's leading call into a base microcode's `setup`
//!   (`GBI_F3D::setup` x3, `GBI_F3DEX2::setup` x1). That base-`setup` call
//!   IS itself the behavior for every opcode a variant does not override
//!   (the variant inherits the base microcode's entire map) -- this
//!   module documents that inheritance in prose (see per-variant sections
//!   above) rather than porting it as code, since porting it would require
//!   the base microcodes' own dispatch tables (`rt64_gbi_f3d.rs`/
//!   `rt64_gbi_f3dex2.rs`), which are themselves unwired dispatch, not
//!   decode.
//! - **`F3DWAVE_G_UNKNOWN`'s `nullptr` mapping** -- there is no function
//!   here to decode; the source comment marks it explicitly unresolved
//!   upstream ("FIXME: ... until it's figured out"), not a decode this
//!   module can invent.
//! - **`F3DGOLDEN_G_MOVEWORD`'s reassignment to `GBI_F3D::moveWord`** --
//!   pure dispatch wiring to an already-existing base-F3D function; no new
//!   bitfield read exists at this call site to port.
//! - **`rt64_gbi_l3dex2.cpp`/`.h` (`GBI_L3DEX2::line3D`)** -- excluded
//!   from this module entirely (see file doc header above): its body is a
//!   bare `assert(false);`, no bitfield read exists to port.
//! - **`DisplayList`'s constructor and its `w0`/`w1` fields as a stateful
//!   struct** -- this module represents a display-list command word as a
//!   plain `(w0: u32, w1: u32)` parameter pair to each `decode_*`
//!   function, matching all four sibling modules' precedent.

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

/// F3DWAVE opcode constants, from `src/gbi/rt64_gbi_f3dwave.h` lines 9-12.
pub mod f3dwave_opcodes {
    /// `F3DWAVE_G_UNKNOWN`. Mapped to `nullptr` in `setup` -- no decoder
    /// exists for it upstream (see module doc "Nonclaims").
    pub const G_UNKNOWN: u8 = 0xB4;
    /// `F3DWAVE_G_RDPHALF_1`. Reassigned to base F3D's `rdpHalf1`
    /// (dispatch, not ported here).
    pub const G_RDPHALF_1: u8 = 0xB3;
    /// `F3DWAVE_G_RDPHALF_2`. Reassigned to base F3D's `rdpHalf2`
    /// (dispatch, not ported here).
    pub const G_RDPHALF_2: u8 = 0xB2;
    /// `F3DWAVE_G_TRI2`. Numerically collides with `F3DGOLDEN_G_TRIX` and
    /// `F3DEX_G_TRI2` (see module doc "F3DWAVE").
    pub const G_TRI2: u8 = 0xB1;
}

/// `GBI_F3DWAVE::vertex`'s operand set: `state->rsp->setVertex((*dl)->w1,
/// (*dl)->p0(9, 7), (*dl)->p0(16, 8) / 5)`. See module doc "F3DWAVE" for
/// how this diverges from base F3D's `vertex` in both field position and
/// width, not just the divisor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct F3dwaveVertexArgs {
    /// Raw `w1`, the vertex source address.
    pub address: u32,
    /// `p0(9, 7)`. Unlike base F3D's `p0(20,4)+1`, this has no `+1` bias.
    pub count: u32,
    /// `p0(16, 8) / 5`. Unlike base F3D's plain `p0(16,4)`, this is an
    /// 8-bit field divided by 5 (base F3D's `tri1`/`quad` divide by 10;
    /// base F3D's `vertex` does not divide at all).
    pub dst_index: u32,
}

fn f3dwave_decode_vertex(w0: u32, w1: u32) -> F3dwaveVertexArgs {
    F3dwaveVertexArgs {
        address: w1,
        count: p0(w0, 9, 7),
        dst_index: p0(w0, 16, 8) / 5,
    }
}

/// One decoded triangle's three vertex indices, shared shape for
/// F3DWAVE's `tri1` and each half of `tri2`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct F3dwaveTriArgs {
    /// First index, position 16, 8 bits, divided by 5.
    pub a: u32,
    /// Second index, position 8, 8 bits, divided by 5.
    pub b: u32,
    /// Third index, position 0, 8 bits, divided by 5.
    pub c: u32,
}

/// `GBI_F3DWAVE::tri1`: `drawIndexedTri((*dl)->p1(16, 8) / 5, (*dl)->p1(8,
/// 8) / 5, (*dl)->p1(0, 8) / 5)`. Reads `w1` (`p1`), same word as base
/// F3D's `tri1`; only the divisor differs (5 vs 10).
fn f3dwave_decode_tri1(_w0: u32, w1: u32) -> F3dwaveTriArgs {
    F3dwaveTriArgs {
        a: p1(w1, 16, 8) / 5,
        b: p1(w1, 8, 8) / 5,
        c: p1(w1, 0, 8) / 5,
    }
}

/// `GBI_F3DWAVE::tri2`'s operand set: two `F3dwaveTriArgs`-shaped reads,
/// first from `w0`, second from `w1`. Base F3D has no `tri2` at all (see
/// module doc "F3DWAVE") -- this is a new opcode, not an override.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct F3dwaveTri2Args {
    /// `drawIndexedTri(p0(16,8)/5, p0(8,8)/5, p0(0,8)/5)`, the first call.
    pub first: F3dwaveTriArgs,
    /// `drawIndexedTri(p1(16,8)/5, p1(8,8)/5, p1(0,8)/5)`, the second call.
    pub second: F3dwaveTriArgs,
}

fn f3dwave_decode_tri2(w0: u32, w1: u32) -> F3dwaveTri2Args {
    F3dwaveTri2Args {
        first: F3dwaveTriArgs {
            a: p0(w0, 16, 8) / 5,
            b: p0(w0, 8, 8) / 5,
            c: p0(w0, 0, 8) / 5,
        },
        second: f3dwave_decode_tri1(w0, w1),
    }
}

/// `GBI_F3DWAVE::quad`'s operand set: four `p1`-sourced corners, each
/// divided by 5 (base F3D's `quad` divides the same four positions by 10).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct F3dwaveQuadArgs {
    /// `p1(24, 8) / 5`.
    pub v0: u32,
    /// `p1(16, 8) / 5`.
    pub v1: u32,
    /// `p1(8, 8) / 5`.
    pub v2: u32,
    /// `p1(0, 8) / 5`.
    pub v3: u32,
}

fn f3dwave_decode_quad(_w0: u32, w1: u32) -> F3dwaveQuadArgs {
    F3dwaveQuadArgs {
        v0: p1(w1, 24, 8) / 5,
        v1: p1(w1, 16, 8) / 5,
        v2: p1(w1, 8, 8) / 5,
        v3: p1(w1, 0, 8) / 5,
    }
}

/// F3DGOLDEN opcode constants, from `src/gbi/rt64_gbi_f3dgolden.h` lines
/// 9-10.
pub mod f3dgolden_opcodes {
    /// `F3DGOLDEN_G_MOVEWORD`. Reassigned to base F3D's `moveWord`
    /// (dispatch, not ported here).
    pub const G_MOVEWORD: u8 = 0xBD;
    /// `F3DGOLDEN_G_TRIX`. Numerically collides with `F3DWAVE_G_TRI2` and
    /// `F3DEX_G_TRI2` (see module doc "F3DWAVE"). F3DPD reassigns this
    /// same opcode byte to `GBI_F3DGOLDEN::triX` by delegation (see module
    /// doc "F3DPD").
    pub const G_TRIX: u8 = 0xB1;
}

/// One triangle emitted by `GBI_F3DGOLDEN::triX`'s nibble-popping loop.
/// Every field is inherently in `0..16` (a 4-bit mask result).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct F3dgoldenTriXIndices {
    /// Low nibble of `w1` at loop entry (popped first).
    pub v0: u32,
    /// Second-low nibble of `w1` at loop entry (popped second, after `w1`
    /// has already been shifted once this iteration).
    pub v1: u32,
    /// Low nibble of `w0` at loop entry (popped once per iteration,
    /// independent pace from `w1`'s two nibbles per iteration).
    pub v2: u32,
}

/// `GBI_F3DGOLDEN::triX`: a variable-length loop, terminated when `w1`
/// reaches zero, that pops two nibbles off `w1` and one off `w0` per
/// iteration and emits a triangle each time. See module doc "F3DGOLDEN"
/// for the full termination/consumption analysis. Note this reads `w0`/
/// `w1` directly with `& 0xf`/`>>= 4`, NOT via `p0`/`p1`.
fn f3dgolden_decode_tri_x(w0: u32, w1: u32) -> Vec<F3dgoldenTriXIndices> {
    let mut w0 = w0;
    let mut w1 = w1;
    let mut out = Vec::new();
    while w1 != 0 {
        let v0 = w1 & 0xf;
        w1 >>= 4;

        let v1 = w1 & 0xf;
        w1 >>= 4;

        let v2 = w0 & 0xf;
        w0 >>= 4;

        out.push(F3dgoldenTriXIndices { v0, v1, v2 });
    }
    out
}

/// F3DPD opcode constant, from `src/gbi/rt64_gbi_f3dpd.h` line 9.
pub mod f3dpd_opcodes {
    /// `F3DPD_G_VTXCOLOR`.
    pub const G_VTXCOLOR: u8 = 0x07;
}

/// `GBI_F3DPD::vertex`'s operand set: `state->rsp->setVertexPD((*dl)->w1,
/// (*dl)->p0(20, 4) + 1, (*dl)->p0(16, 4))`. Numerically identical field
/// shape to base F3D's `vertex` -- see module doc "F3DPD" for why this
/// module still defines its own decoder rather than reusing one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct F3dpdVertexArgs {
    /// Raw `w1`, the vertex source address.
    pub address: u32,
    /// `p0(20, 4) + 1`. Range `1..=16`.
    pub count: u32,
    /// `p0(16, 4)`. Range `0..16`, no divisor.
    pub dst_index: u32,
}

fn f3dpd_decode_vertex(w0: u32, w1: u32) -> F3dpdVertexArgs {
    F3dpdVertexArgs {
        address: w1,
        count: p0(w0, 20, 4) + 1,
        dst_index: p0(w0, 16, 4),
    }
}

/// `GBI_F3DPD::vertexColor`: `state->rsp->setVertexColorPD((*dl)->w1)`.
/// Reads no bitfields -- forwards raw `w1` unchanged, ignores `w0`
/// entirely.
fn f3dpd_decode_vertex_color(w1: u32) -> u32 {
    w1
}

/// F3DZEX2 opcode constant, from `src/gbi/rt64_gbi_f3dzex2.h` line 9.
pub mod f3dzex2_opcodes {
    /// `F3DZEX2_G_BRANCH_W`. Numerically identical to `F3DEX2_G_BRANCH_Z`
    /// (`0x04`, from `rt64_gbi_f3dex2.h`) -- `GBI_F3DZEX2::setup`
    /// overwrites that inherited map entry with `branchW`'s function
    /// pointer (see module doc "F3DZEX2").
    pub const G_BRANCH_W: u8 = 0x04;
}

/// `GBI_F3DZEX2::branchW`'s operand set: `state->rsp->branchW(
/// state->microcode.half1, (*dl)->p0(1, 7), (*dl)->w1, dl)`. See module
/// doc "F3DZEX2" for the width divergence from F3DEX's `branchZ` at the
/// same opcode slot and bit position.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BranchWArgs {
    /// `p0(1, 7)`. F3DEX's `branchZ` reads an 11-bit field (`p0(1, 11)`)
    /// at this SAME bit position for the SAME opcode byte -- this is a
    /// 7-bit field, four bits narrower, NOT the same decode.
    pub vtx_index: u32,
    /// Raw `w1`, unmasked. Same shape as `branchZ`'s own raw `w1`
    /// argument -- not a divergence.
    pub w_value: u32,
}

fn f3dzex2_decode_branch_w(w0: u32, w1: u32) -> BranchWArgs {
    BranchWArgs {
        vtx_index: p0(w0, 1, 7),
        w_value: w1,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- p0 / p1: shared bitfield extraction primitives ---

    #[test]
    fn p0_all_zero_word_is_zero() {
        assert_eq!(p0(0, 0, 24), 0);
    }

    #[test]
    fn p0_all_ones_word_saturates_field() {
        assert_eq!(p0(0xFFFF_FFFF, 16, 8), 0xFF);
    }

    #[test]
    fn p0_one_bit_above_max_is_masked_off() {
        // Field is 8 bits wide at position 16 (bits 16..24); bit 24 is one
        // above the field's top bit and must be masked away.
        assert_eq!(p0(1 << 24, 16, 8), 0);
    }

    #[test]
    fn p1_reads_w1_not_w0() {
        assert_eq!(p1(0xFFFF_FFFF, 16, 8), 0xFF);
        assert_eq!(p1(0, 16, 8), 0);
    }

    // --- F3DWAVE::vertex ---

    #[test]
    fn f3dwave_vertex_all_zeros() {
        let a = f3dwave_decode_vertex(0, 0);
        assert_eq!(
            a,
            F3dwaveVertexArgs {
                address: 0,
                count: 0,
                dst_index: 0,
            }
        );
    }

    #[test]
    fn f3dwave_vertex_all_ones() {
        let a = f3dwave_decode_vertex(0xFFFF_FFFF, 0xFFFF_FFFF);
        assert_eq!(
            a,
            F3dwaveVertexArgs {
                address: 0xFFFF_FFFF,
                count: 0x7F,         // p0(9,7) saturates to 7 ones.
                dst_index: 0xFF / 5, // p0(16,8) saturates to 0xFF, then /5.
            }
        );
    }

    #[test]
    fn f3dwave_vertex_count_field_at_max_before_and_above() {
        // count = p0(9,7): bits 9..16. Max value at that field is 0x7F.
        let at_max = f3dwave_decode_vertex(0x7F << 9, 0);
        assert_eq!(at_max.count, 0x7F);
        // One bit above the field's top bit (bit 16) must not leak in.
        let one_above = f3dwave_decode_vertex(1 << 16, 0);
        assert_eq!(one_above.count, 0);
    }

    #[test]
    fn f3dwave_vertex_count_field_reads_bit_9_not_bit_20_like_base_f3d() {
        // Base F3D's vertex count field is p0(20,4)+1, at bit 20. Setting
        // only bit 20 must NOT affect F3DWAVE's count (which starts at bit
        // 9, width 7, i.e. bits 9..16 -- bit 20 is outside that range).
        let a = f3dwave_decode_vertex(1 << 20, 0);
        assert_eq!(a.count, 0);
    }

    #[test]
    fn f3dwave_vertex_dst_index_has_no_plus_one_bias_unlike_base_f3d() {
        // Base F3D's vertex dst-index (p0(16,4)) has no bias either, but
        // its count field (p0(20,4)+1) does. F3DWAVE's count (p0(9,7)) has
        // NO +1 bias at all -- an all-zero w0 yields count=0, not count=1.
        let a = f3dwave_decode_vertex(0, 0);
        assert_eq!(a.count, 0);
    }

    #[test]
    fn f3dwave_vertex_dst_index_divides_by_five_at_field_max() {
        // dst_index = p0(16,8)/5: bits 16..24, 8 bits wide, max 0xFF=255.
        let a = f3dwave_decode_vertex(0xFF << 16, 0);
        assert_eq!(a.dst_index, 255 / 5);
    }

    // --- F3DWAVE::tri1 ---

    #[test]
    fn f3dwave_tri1_all_zeros() {
        assert_eq!(
            f3dwave_decode_tri1(0, 0),
            F3dwaveTriArgs { a: 0, b: 0, c: 0 }
        );
    }

    #[test]
    fn f3dwave_tri1_all_ones_divides_by_five() {
        let a = f3dwave_decode_tri1(0, 0xFFFF_FFFF);
        assert_eq!(
            a,
            F3dwaveTriArgs {
                a: 0xFF / 5,
                b: 0xFF / 5,
                c: 0xFF / 5,
            }
        );
    }

    #[test]
    fn f3dwave_tri1_divisor_is_five_not_ten() {
        // Field a = p1(16,8): set to exactly 10 -> base F3D's /10 would
        // give 1, F3DWAVE's /5 must give 2.
        let a = f3dwave_decode_tri1(0, 10 << 16);
        assert_eq!(a.a, 2);
    }

    #[test]
    fn f3dwave_tri1_reads_w1_ignores_w0() {
        let with_w0 = f3dwave_decode_tri1(0xFFFF_FFFF, 0);
        let without_w0 = f3dwave_decode_tri1(0, 0);
        assert_eq!(with_w0, without_w0);
        assert_eq!(with_w0, F3dwaveTriArgs { a: 0, b: 0, c: 0 });
    }

    #[test]
    fn f3dwave_tri1_field_one_bit_above_max_is_masked() {
        // Field c = p1(0,8): bit 8 is one above the top of that field.
        let a = f3dwave_decode_tri1(0, 1 << 8);
        assert_eq!(a.c, 0);
    }

    // --- F3DWAVE::tri2 ---

    #[test]
    fn f3dwave_tri2_first_triangle_reads_w0_second_reads_w1() {
        // Word-pinning test: swapping w0 and w1's roles must change which
        // half of the result changes.
        let w0 = 0x0000_0A00; // a-field of first triangle: p0(16,8) region.
        let w1 = 0x000A_0000; // not the same field in w1; keep w1 nonzero-free-of-collision.
        let with_w0_only = f3dwave_decode_tri2(0xFF << 16, 0);
        let with_w1_only = f3dwave_decode_tri2(0, 0xFF << 16);
        assert_ne!(with_w0_only.first, with_w1_only.first);
        assert_eq!(with_w1_only.first, F3dwaveTriArgs { a: 0, b: 0, c: 0 });
        assert_eq!(
            with_w0_only.first,
            F3dwaveTriArgs {
                a: 0xFF / 5,
                b: 0,
                c: 0
            }
        );
        let _ = (w0, w1);
    }

    #[test]
    fn f3dwave_tri2_second_matches_tri1_decode() {
        let w0 = 0x1234_5678;
        let w1 = 0x9ABC_DEF0;
        let combined = f3dwave_decode_tri2(w0, w1);
        assert_eq!(combined.second, f3dwave_decode_tri1(w0, w1));
    }

    #[test]
    fn f3dwave_tri2_all_ones_both_halves_saturate() {
        let a = f3dwave_decode_tri2(0xFFFF_FFFF, 0xFFFF_FFFF);
        let full = F3dwaveTriArgs {
            a: 0xFF / 5,
            b: 0xFF / 5,
            c: 0xFF / 5,
        };
        assert_eq!(a.first, full);
        assert_eq!(a.second, full);
    }

    #[test]
    fn f3dwave_tri2_is_not_base_f3d_tri2_because_none_exists() {
        // Documentation-by-construction: base F3D has no tri2 at all, so
        // there is nothing to compare tri2's decode against for equality;
        // this test only asserts f3dwave's tri2 decode is well-defined and
        // distinct from its own tri1 decode when both words carry data.
        let tri1_only = f3dwave_decode_tri1(0x1111_1111, 0x2222_2222);
        let tri2 = f3dwave_decode_tri2(0x1111_1111, 0x2222_2222);
        assert_eq!(tri2.second, tri1_only);
    }

    // --- F3DWAVE::quad ---

    #[test]
    fn f3dwave_quad_all_zeros() {
        assert_eq!(
            f3dwave_decode_quad(0, 0),
            F3dwaveQuadArgs {
                v0: 0,
                v1: 0,
                v2: 0,
                v3: 0
            }
        );
    }

    #[test]
    fn f3dwave_quad_all_ones_divides_by_five() {
        let a = f3dwave_decode_quad(0, 0xFFFF_FFFF);
        assert_eq!(
            a,
            F3dwaveQuadArgs {
                v0: 0xFF / 5,
                v1: 0xFF / 5,
                v2: 0xFF / 5,
                v3: 0xFF / 5,
            }
        );
    }

    #[test]
    fn f3dwave_quad_divisor_is_five_not_ten() {
        // v0 = p1(24,8): set to exactly 15 -> base F3D's /10 gives 1,
        // F3DWAVE's /5 gives 3.
        let a = f3dwave_decode_quad(0, 15u32 << 24);
        assert_eq!(a.v0, 3);
    }

    #[test]
    fn f3dwave_quad_reads_w1_ignores_w0() {
        let with_w0 = f3dwave_decode_quad(0xFFFF_FFFF, 0);
        let without_w0 = f3dwave_decode_quad(0, 0);
        assert_eq!(with_w0, without_w0);
    }

    #[test]
    fn f3dwave_quad_each_field_one_bit_above_max_is_masked() {
        // v3 = p1(0,8): bit 8 is one above its top bit.
        let a = f3dwave_decode_quad(0, 1 << 8);
        assert_eq!(a.v3, 0);
        // v0 = p1(24,8): bit 32 doesn't exist in u32, so use the top
        // field's own max-plus-shift-out case: shifting 0x1FF << 24
        // overflows u32 -- instead confirm v0's field boundary at bit 24.
        let at_max = f3dwave_decode_quad(0, 0xFFu32 << 24);
        assert_eq!(at_max.v0, 0xFF / 5);
    }

    // --- F3DGOLDEN::triX ---

    #[test]
    fn f3dgolden_tri_x_all_zero_words_yields_empty_list() {
        assert_eq!(f3dgolden_decode_tri_x(0, 0), Vec::new());
    }

    #[test]
    fn f3dgolden_tri_x_terminates_on_w1_reaching_zero_even_if_w0_nonzero() {
        // w1 has exactly one nibble pair (one iteration's worth), then
        // becomes zero; w0 is left with plenty of nonzero nibbles that
        // must NOT be consumed once w1 hits zero.
        let w0 = 0xFFFF_FFFF;
        let w1 = 0x0000_0012; // one iteration: v0=2, v1=1, then w1=0.
        let result = f3dgolden_decode_tri_x(w0, w1);
        assert_eq!(result.len(), 1);
    }

    #[test]
    fn f3dgolden_tri_x_v0_v1_come_from_w1_v2_comes_from_w0() {
        // Word-pinning test: v0/v1 must come from w1's nibbles, v2 from
        // w0's nibble, never the reverse.
        let w0 = 0x0000_000A; // low nibble 0xA -> v2 candidate.
        let w1 = 0x0000_00B1; // low byte 0xB1 -> v0=1 (low nibble), v1=0xB (next nibble).
        let result = f3dgolden_decode_tri_x(w0, w1);
        assert_eq!(result.len(), 1);
        assert_eq!(
            result[0],
            F3dgoldenTriXIndices {
                v0: 1,
                v1: 0xB,
                v2: 0xA,
            }
        );
    }

    #[test]
    fn f3dgolden_tri_x_two_iteration_count_and_nibble_order() {
        // w1 = 0x0000_1234: iteration 1 pops v0=4,v1=3 (w1 -> 0x12);
        // iteration 2 pops v0=2,v1=1 (w1 -> 0), loop stops.
        // w0 = 0x0000_00AB: iteration 1 pops v2=0xB (w0 -> 0xA); iteration
        // 2 pops v2=0xA (w0 -> 0).
        let w0 = 0x0000_00AB;
        let w1 = 0x0000_1234;
        let result = f3dgolden_decode_tri_x(w0, w1);
        assert_eq!(
            result,
            vec![
                F3dgoldenTriXIndices {
                    v0: 4,
                    v1: 3,
                    v2: 0xB
                },
                F3dgoldenTriXIndices {
                    v0: 2,
                    v1: 1,
                    v2: 0xA
                },
            ]
        );
    }

    #[test]
    fn f3dgolden_tri_x_all_ones_words_every_nibble_is_max() {
        let result = f3dgolden_decode_tri_x(0xFFFF_FFFF, 0xFFFF_FFFF);
        assert_eq!(result.len(), 4); // 8 nibbles in w1 / 2 per iteration.
        for tri in &result {
            assert_eq!(
                *tri,
                F3dgoldenTriXIndices {
                    v0: 0xF,
                    v1: 0xF,
                    v2: 0xF
                }
            );
        }
    }

    #[test]
    fn f3dgolden_tri_x_iteration_count_can_exceed_w0_nibble_count() {
        // w0 runs out of nonzero nibbles before w1 does -- w0 keeps
        // shifting in zero nibbles (v2=0) while w1 still drives the loop.
        let w0 = 0x0000_0001; // one nonzero nibble, then all zero.
        let w1 = 0x1111_1111; // four iterations' worth (8 nibbles / 2 per iteration).
        let result = f3dgolden_decode_tri_x(w0, w1);
        assert_eq!(result.len(), 4);
        assert_eq!(result[0].v2, 1);
        assert_eq!(result[1].v2, 0);
        assert_eq!(result[2].v2, 0);
        assert_eq!(result[3].v2, 0);
    }

    // --- F3DPD::vertex ---

    #[test]
    fn f3dpd_vertex_all_zeros() {
        let a = f3dpd_decode_vertex(0, 0);
        assert_eq!(
            a,
            F3dpdVertexArgs {
                address: 0,
                count: 1, // p0(20,4)+1 with all-zero field is 0+1=1.
                dst_index: 0,
            }
        );
    }

    #[test]
    fn f3dpd_vertex_all_ones() {
        let a = f3dpd_decode_vertex(0xFFFF_FFFF, 0xFFFF_FFFF);
        assert_eq!(
            a,
            F3dpdVertexArgs {
                address: 0xFFFF_FFFF,
                count: 0xF + 1, // p0(20,4) saturates to 0xF, then +1 = 16.
                dst_index: 0xF, // p0(16,4) saturates to 0xF.
            }
        );
    }

    #[test]
    fn f3dpd_vertex_count_field_at_max_before_and_above() {
        let at_max = f3dpd_decode_vertex(0xF << 20, 0);
        assert_eq!(at_max.count, 16);
        let one_above = f3dpd_decode_vertex(1 << 24, 0);
        assert_eq!(one_above.count, 1);
    }

    #[test]
    fn f3dpd_vertex_dst_index_field_at_max_before_and_above() {
        let at_max = f3dpd_decode_vertex(0xF << 16, 0);
        assert_eq!(at_max.dst_index, 0xF);
        let one_above = f3dpd_decode_vertex(1 << 20, 0);
        assert_eq!(one_above.dst_index, 0);
    }

    #[test]
    fn f3dpd_vertex_matches_base_f3d_vertex_field_shape_numerically() {
        // Documents the coincidence explicitly (see module doc "F3DPD"):
        // F3DPD's vertex decode is numerically identical to base F3D's
        // vertex decode, field-for-field, even though this module defines
        // its own decoder (never importing rt64_gbi_f3d.rs's).
        let inputs = [
            (0, 0),
            (0xFFFF_FFFF, 0xFFFF_FFFF),
            (0x00F1_2340, 0xABCD_EF01),
            (0x0012_3400, 0),
        ];
        for (w0, w1) in inputs {
            let f3dpd = f3dpd_decode_vertex(w0, w1);
            // Re-derive base F3D's formula independently, not by calling
            // into rt64_gbi_f3d.rs (kept import-free per module doc
            // "Reuse, not new type").
            let base_f3d_count = p0(w0, 20, 4) + 1;
            let base_f3d_dst_index = p0(w0, 16, 4);
            assert_eq!(f3dpd.count, base_f3d_count);
            assert_eq!(f3dpd.dst_index, base_f3d_dst_index);
            assert_eq!(f3dpd.address, w1);
        }
    }

    #[test]
    fn f3dpd_vertex_reads_w0_for_fields_w1_for_address() {
        let with_w0 = f3dpd_decode_vertex(0xF << 20, 0xFFFF_FFFF);
        let without_w0 = f3dpd_decode_vertex(0, 0xFFFF_FFFF);
        assert_ne!(with_w0.count, without_w0.count);
        assert_eq!(with_w0.address, without_w0.address);
    }

    // --- F3DPD::vertexColor ---

    #[test]
    fn vertex_color_returns_w1_unchanged_and_ignores_w0() {
        assert_eq!(f3dpd_decode_vertex_color(0), 0);
        assert_eq!(f3dpd_decode_vertex_color(0xFFFF_FFFF), 0xFFFF_FFFF);
        assert_eq!(f3dpd_decode_vertex_color(0x1234_5678), 0x1234_5678);
    }

    #[test]
    fn f3dpd_trix_opcode_delegates_to_f3dgolden_decode_tri_x() {
        // F3DPD's setup reassigns F3DGOLDEN_G_TRIX to
        // GBI_F3DGOLDEN::triX directly -- there is no f3dpd::decode_tri_x.
        // This test documents that delegation: F3DPD's triX opcode decode
        // literally IS f3dgolden_decode_tri_x.
        let w0 = 0x0000_00AB;
        let w1 = 0x0000_1234;
        let f3dpd_trix_decode = f3dgolden_decode_tri_x(w0, w1); // the only decoder that exists.
        let expected = f3dgolden_decode_tri_x(w0, w1);
        assert_eq!(f3dpd_trix_decode, expected);
    }

    // --- F3DZEX2::branchW ---

    #[test]
    fn branch_w_all_zeros() {
        let a = f3dzex2_decode_branch_w(0, 0);
        assert_eq!(
            a,
            BranchWArgs {
                vtx_index: 0,
                w_value: 0,
            }
        );
    }

    #[test]
    fn branch_w_all_ones() {
        let a = f3dzex2_decode_branch_w(0xFFFF_FFFF, 0xFFFF_FFFF);
        assert_eq!(
            a,
            BranchWArgs {
                vtx_index: 0x7F, // p0(1,7) saturates to 7 ones.
                w_value: 0xFFFF_FFFF,
            }
        );
    }

    #[test]
    fn branch_w_vtx_index_field_at_max_before_and_above() {
        let at_max = f3dzex2_decode_branch_w(0x7F << 1, 0);
        assert_eq!(at_max.vtx_index, 0x7F);
        let one_above = f3dzex2_decode_branch_w(1 << 8, 0);
        assert_eq!(one_above.vtx_index, 0);
    }

    #[test]
    fn branch_w_reads_seven_bits_not_eleven_bits_like_f3dex_branch_z() {
        // Word-pinning / width-pinning test: branchZ's field is p0(1,11),
        // branchW's is p0(1,7), same start bit 1, same opcode slot (0x04).
        // Setting bit 8 (within branchZ's 11-bit field, bits 1..12, but
        // outside branchW's 7-bit field, bits 1..8) must decode to 0 for
        // branchW's vtx_index -- if this module had copied branchZ's
        // width by analogy, this bit would incorrectly leak into the
        // result.
        let a = f3dzex2_decode_branch_w(1 << 8, 0);
        assert_eq!(a.vtx_index, 0);
    }

    #[test]
    fn branch_w_bit_eight_above_seven_bit_field_is_masked_off() {
        // Field is bits 1..8 (7 bits starting at bit 1). Bit 8 is one
        // above the top of that field and must be masked away -- but is
        // still inside branchZ's 11-bit field (bits 1..12), which is
        // exactly the divergence this module exists to pin.
        let a = f3dzex2_decode_branch_w(1 << 8, 0);
        assert_eq!(a.vtx_index, 0);
    }

    #[test]
    fn branch_w_carries_w1_unmasked_matching_branch_z_shape() {
        // w1 is passed through raw with no p0/p1 extraction, identical in
        // shape to branchZ's own raw w1 argument -- only the p0(1,*)
        // width differs between the two opcodes, not the w1 handling.
        let a = f3dzex2_decode_branch_w(0, 0x89AB_CDEF);
        assert_eq!(a.w_value, 0x89AB_CDEF);
    }

    #[test]
    fn branch_w_ignores_w0_bits_outside_its_field() {
        let with_far_bits = f3dzex2_decode_branch_w(0xFFFF_FF00, 0);
        let without = f3dzex2_decode_branch_w(0, 0);
        assert_eq!(with_far_bits, without);
    }

    // --- opcode constants (numeric collisions, cross-microcode) ---

    #[test]
    fn f3dwave_tri2_and_f3dgolden_trix_opcode_bytes_collide() {
        assert_eq!(f3dwave_opcodes::G_TRI2, f3dgolden_opcodes::G_TRIX);
        assert_eq!(f3dwave_opcodes::G_TRI2, 0xB1);
    }

    #[test]
    fn f3dzex2_branch_w_opcode_matches_f3dex2_branch_z_slot() {
        // F3DEX2_G_BRANCH_Z is 0x04 (rt64_gbi_f3dex2.h) -- same byte,
        // different decode (see branch_w_reads_seven_bits_not_eleven_
        // bits_like_f3dex_branch_z above).
        assert_eq!(f3dzex2_opcodes::G_BRANCH_W, 0x04);
    }

    #[test]
    fn f3dpd_vtxcolor_opcode_value() {
        assert_eq!(f3dpd_opcodes::G_VTXCOLOR, 0x07);
    }

    #[test]
    fn f3dwave_rdphalf_and_unknown_opcode_values() {
        assert_eq!(f3dwave_opcodes::G_UNKNOWN, 0xB4);
        assert_eq!(f3dwave_opcodes::G_RDPHALF_1, 0xB3);
        assert_eq!(f3dwave_opcodes::G_RDPHALF_2, 0xB2);
    }

    #[test]
    fn f3dgolden_moveword_opcode_value() {
        assert_eq!(f3dgolden_opcodes::G_MOVEWORD, 0xBD);
    }
}
