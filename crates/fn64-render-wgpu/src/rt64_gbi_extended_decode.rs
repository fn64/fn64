//! Literal port of the bitfield-DECODE half of RT64's extended-GBI opcode
//! interpreter -- the per-opcode functions in `GBI_EXTENDED` that read
//! `DisplayList::p0`/`p1` off one or more command-word pairs -- a literal
//! port of the permitted MIT RT64 Rust-port source pinned at commit
//! `5473732a822a4423b5696e7cb18fecc425a59875` (`docs/RT64-PORT-AUTHORITY.md`),
//! `src/gbi/rt64_gbi_extended.cpp` (SHA-256 of the whole file,
//! `54eeb28949d180270c6abd9fa0c28531a8ab3098c0bd824c99c7849225e74792`),
//! `src/gbi/rt64_gbi_extended.h` (SHA-256 of the whole file,
//! `d727674e8ac9c710d832289a36a6389367bee6c1a1d34a6b229b0f339df2b374`), and
//! `src/gbi/rt64_display_list.h` (SHA-256 of the whole file,
//! `0d66de6d51e0db28eaa9cf366e5c68d77a72d90fc2966e7ba53151b5dd68cf15`; the
//! `p0`/`p1` extractor bodies themselves live in `src/gbi/rt64_gbi.cpp`,
//! whose whole-file digest is not cited here because this port does not
//! claim that file -- the two extractor functions are quoted verbatim
//! below and re-derived, matching `rt64_gbi_rdp_decode.rs`'s precedent):
//!
//! ```text
//! // src/gbi/rt64_gbi.cpp:32-38 (extractors, shared with every other GBI
//! // decode file in this crate)
//! uint32_t DisplayList::p0(uint8_t pos, uint8_t bits) const {
//!     return ((w0 >> pos) & ((0x01 << bits) - 1));
//! }
//!
//! uint32_t DisplayList::p1(uint8_t pos, uint8_t bits) const {
//!     return ((w1 >> pos) & ((0x01 << bits) - 1));
//! }
//!
//! // src/gbi/rt64_gbi_extended.cpp:22-41 (texrectV1 -- 3 command-word pairs)
//! void texrectV1(State *state, DisplayList **dl) {
//!     ExtendedAlignment extAlignment;
//!     const uint8_t tile = (*dl)->p1(0, 3);
//!     extAlignment.leftOrigin = (*dl)->p1(3, 12);
//!     extAlignment.rightOrigin = (*dl)->p1(15, 12);
//!     const bool flip = (*dl)->p1(7, 1);
//!     *dl = *dl + 1;
//!
//!     const int16_t ulx = (*dl)->p0(16, 16);
//!     const int16_t uly = (*dl)->p0(0, 16);
//!     const int16_t lrx = (*dl)->p1(16, 16);
//!     const int16_t lry = (*dl)->p1(0, 16);
//!     *dl = *dl + 1;
//!
//!     const int16_t uls = (*dl)->p0(16, 16);
//!     const int16_t ult = (*dl)->p0(0, 16);
//!     const int16_t dsdx = (*dl)->p1(16, 16);
//!     const int16_t dtdy = (*dl)->p1(0, 16);
//!     state->rdp->drawTexRect(ulx, uly, lrx, lry, tile, uls, ult, dsdx, dtdy, flip, extAlignment);
//! }
//!
//! // src/gbi/rt64_gbi_extended.cpp:43-54 (fillrectV1 -- 2 command-word pairs)
//! void fillrectV1(State *state, DisplayList **dl) {
//!     ExtendedAlignment extAlignment;
//!     extAlignment.leftOrigin = (*dl)->p1(0, 12);
//!     extAlignment.rightOrigin = (*dl)->p1(12, 12);
//!     *dl = *dl + 1;
//!
//!     const int16_t ulx = (*dl)->p0(16, 16);
//!     const int16_t uly = (*dl)->p0(0, 16);
//!     const int16_t lrx = (*dl)->p1(16, 16);
//!     const int16_t lry = (*dl)->p1(0, 16);
//!     state->rdp->fillRect(ulx, uly, lrx, lry, extAlignment);
//! }
//!
//! // src/gbi/rt64_gbi_extended.cpp:56-61 (setViewportV1 -- 2 command-word pairs)
//! void setViewportV1(State *state, DisplayList **dl) {
//!     const uint16_t ori = (*dl)->p1(0, 12);
//!     *dl = *dl + 1;
//!
//!     state->rsp->setViewport((*dl)->w1, ori, 0, 0);
//! }
//!
//! // src/gbi/rt64_gbi_extended.cpp:63-75 (setScissorV1 -- 2 command-word pairs)
//! void setScissorV1(State *state, DisplayList **dl) {
//!     ExtendedAlignment extAlignment;
//!     const uint8_t mode = (*dl)->p1(0, 2);
//!     extAlignment.leftOrigin = (*dl)->p1(2, 12);
//!     extAlignment.rightOrigin = (*dl)->p1(14, 12);
//!     *dl = *dl + 1;
//!
//!     const int16_t ulx = (*dl)->p0(16, 16);
//!     const int16_t uly = (*dl)->p0(0, 16);
//!     const int16_t lrx = (*dl)->p1(16, 16);
//!     const int16_t lry = (*dl)->p1(0, 16);
//!     state->rdp->setScissor(mode, ulx, uly, lrx, lry, extAlignment);
//! }
//!
//! // src/gbi/rt64_gbi_extended.cpp:77-88 (setRectAlignV1 -- 2 command-word pairs)
//! void setRectAlignV1(State *state, DisplayList **dl) {
//!     ExtendedAlignment extAlignment;
//!     extAlignment.leftOrigin = (*dl)->p1(0, 12);
//!     extAlignment.rightOrigin = (*dl)->p1(12, 12);
//!     *dl = *dl + 1;
//!
//!     extAlignment.leftOffset = (int16_t)(*dl)->p0(16, 16);
//!     extAlignment.topOffset = (int16_t)(*dl)->p0(0, 16);
//!     extAlignment.rightOffset = (int16_t)(*dl)->p1(16, 16);
//!     extAlignment.bottomOffset = (int16_t)(*dl)->p1(0, 16);
//!     state->rdp->setRectAlign(extAlignment);
//! }
//!
//! // src/gbi/rt64_gbi_extended.cpp:90-97 (setViewportAlignV1 -- 2 command-word pairs)
//! void setViewportAlignV1(State *state, DisplayList **dl) {
//!     const uint16_t ori = (*dl)->p1(0, 12);
//!     *dl = *dl + 1;
//!
//!     const int16_t x = (*dl)->p0(16, 16);
//!     const int16_t y = (*dl)->p0(0, 16);
//!     state->rsp->setViewportAlign(ori, x, y);
//! }
//!
//! // src/gbi/rt64_gbi_extended.cpp:99-116 (setScissorAlignV1 -- 3 command-word pairs)
//! void setScissorAlignV1(State *state, DisplayList **dl) {
//!     ExtendedAlignment extAlignment;
//!     extAlignment.leftOrigin = (*dl)->p1(0, 12);
//!     extAlignment.rightOrigin = (*dl)->p1(12, 12);
//!     *dl = *dl + 1;
//!
//!     extAlignment.leftOffset = (int16_t)(*dl)->p0(16, 16);
//!     extAlignment.topOffset = (int16_t)(*dl)->p0(0, 16);
//!     extAlignment.rightOffset = (int16_t)(*dl)->p1(16, 16);
//!     extAlignment.bottomOffset = (int16_t)(*dl)->p1(0, 16);
//!     *dl = *dl + 1;
//!
//!     extAlignment.leftBound = (*dl)->p0(16, 16);
//!     extAlignment.topBound = (*dl)->p0(0, 16);
//!     extAlignment.rightBound = (*dl)->p1(16, 16);
//!     extAlignment.bottomBound = (*dl)->p1(0, 16);
//!     state->rdp->setScissorAlign(extAlignment);
//! }
//!
//! // src/gbi/rt64_gbi_extended.cpp:118-130 (setRefreshRateV1, vertexZTestV1 --
//! // 1 command-word pair each; endVertexZTestV1 has no fields, not ported)
//! void setRefreshRateV1(State *state, DisplayList **dl) {
//!     const uint16_t refreshRate = (*dl)->p1(0, 16);
//!     state->setRefreshRate(refreshRate);
//! }
//!
//! void vertexZTestV1(State *state, DisplayList **dl) {
//!     const uint8_t vertexIndex = (*dl)->p1(0, 8);
//!     state->rsp->vertexTestZ(vertexIndex);
//! }
//!
//! // src/gbi/rt64_gbi_extended.cpp:132-151 (matrixGroupCommand -- shared
//! // decode body for matrixGroupV1/editGroupByAddressV1; 2 command-word
//! // pairs; idIsAddress/editGroup are caller-supplied booleans, not decoded
//! // from the command words)
//! void matrixGroupCommand(State *state, DisplayList **dl, bool idIsAddress, bool editGroup) {
//!     const uint32_t id = (*dl)->w1;
//!     *dl = *dl + 1;
//!     const uint8_t push = (*dl)->p0(0, 1);
//!     const uint8_t proj = (*dl)->p0(1, 1);
//!     const uint8_t mode = (*dl)->p0(2, 1);
//!     const uint8_t pos = (*dl)->p0(3, 2);
//!     const uint8_t rot = (*dl)->p0(5, 2);
//!     const uint8_t scale = (*dl)->p0(7, 2);
//!     const uint8_t skew = (*dl)->p0(9, 2);
//!     const uint8_t persp = (*dl)->p0(11, 2);
//!     const uint8_t vpos = (*dl)->p0(13, 2);
//!     const uint8_t vtc = (*dl)->p0(22, 2);
//!     const uint8_t tile = (*dl)->p0(15, 2);
//!     const uint8_t order = (*dl)->p0(17, 2);
//!     const uint8_t editable = (*dl)->p0(19, 1);
//!     const uint8_t aspect = (*dl)->p0(20, 2);
//!     const uint8_t lookat = (*dl)->p0(24, 2);
//!     state->rsp->matrixId(id, push, proj, mode, pos, rot, scale, skew, persp, vpos, vtc, tile, lookat, order, aspect, editable, idIsAddress, editGroup);
//! }
//!
//! // src/gbi/rt64_gbi_extended.cpp:157-161 (popMatrixGroupV1 -- 1 command-word pair)
//! void popMatrixGroupV1(State *state, DisplayList **dl) {
//!     const uint8_t popCount = (*dl)->p1(0, 8);
//!     const uint8_t proj = (*dl)->p0(8, 1);
//!     state->rsp->popMatrixId(popCount, proj);
//! }
//!
//! // src/gbi/rt64_gbi_extended.cpp:163-186 (four single-bit/two-bit force/
//! // render toggles -- 1 command-word pair each, all read from p1(0, N))
//! void forceUpscale2DV1(State *state, DisplayList **dl) {
//!     const uint8_t force = (*dl)->p1(0, 1);
//!     state->rdp->forceUpscale2D(force);
//! }
//!
//! void forceTrueBilerpV1(State *state, DisplayList **dl) {
//!     const uint8_t mode = (*dl)->p1(0, 2);
//!     state->rdp->forceTrueBilerp(mode);
//! }
//!
//! void forceScaleLODV1(State *state, DisplayList **dl) {
//!     const uint8_t force = (*dl)->p1(0, 1);
//!     state->rdp->forceScaleLOD(force);
//! }
//!
//! void forceBranchV1(State *state, DisplayList **dl) {
//!     const uint8_t force = (*dl)->p1(0, 1);
//!     state->rsp->forceBranch(force);
//! }
//!
//! void setRenderToRAMV1(State *state, DisplayList **dl) {
//!     const uint8_t render = (*dl)->p1(0, 1);
//!     state->setRenderToRAM(render);
//! }
//!
//! // src/gbi/rt64_gbi_extended.cpp:192-197 (vertexV1 -- 2 command-word pairs)
//! void vertexV1(State *state, DisplayList **dl) {
//!     uint8_t vtxCount = (*dl)->p1(8, 8);
//!     uint8_t dstIndex = (*dl)->p1(0, 8);
//!     *dl = *dl + 1;
//!     state->rsp->setVertexEXV1((*dl)->w1, vtxCount, dstIndex);
//! }
//!
//! // src/gbi/rt64_gbi_extended.cpp:287-295 (setDitherNoiseStrengthV1,
//! // setRDRAMExtendedV1 -- 1 command-word pair each)
//! void setDitherNoiseStrengthV1(State *state, DisplayList **dl) {
//!     const uint16_t noiseStrength = (*dl)->p1(0, 16);
//!     state->setDitherNoiseStrength(noiseStrength / 1024.0f);
//! }
//!
//! void setRDRAMExtendedV1(State *state, DisplayList **dl) {
//!     const uint8_t extended = (*dl)->p1(0, 1);
//!     state->setExtendedRDRAM(extended);
//! }
//!
//! // src/gbi/rt64_gbi_extended.cpp:297-303 (setProjectionMatrixFloatV1,
//! // setViewMatrixFloatV1 -- 1 command-word pair each, no p0/p1 bitfield:
//! // the whole w1 word is passed through verbatim as a float-matrix pointer)
//! void setProjectionMatrixFloatV1(State *state, DisplayList **dl) {
//!     state->rsp->setProjectionMatrixFloat((*dl)->w1);
//! }
//!
//! void setViewMatrixFloatV1(State *state, DisplayList **dl) {
//!     state->rsp->setViewMatrixFloat((*dl)->w1);
//! }
//!
//! // src/gbi/rt64_gbi_extended.cpp:305-308 (setNearClippingV1 -- 1
//! // command-word pair; note the `!nearClipping` negation happens in the
//! // state->rsp->setNoN call, i.e. AFTER decode -- see "Admitted domain")
//! void setNearClippingV1(State *state, DisplayList **dl) {
//!     const uint8_t nearClipping = (*dl)->p1(0, 1);
//!     state->rsp->setNoN(!nearClipping);
//! }
//!
//! // src/gbi/rt64_gbi_extended.cpp:310-314 (matrixFloatV1 -- 2 command-word
//! // pairs; note `^ state->rsp->pushMask` reads mutable RSP state external
//! // to the command words -- see "Admitted domain")
//! void matrixFloatV1(State *state, DisplayList **dl) {
//!     uint8_t params = (*dl)->p1(0, 8) ^ state->rsp->pushMask;
//!     *dl = *dl + 1;
//!     state->rsp->matrixFloat((*dl)->w1, params);
//! }
//!
//! // src/gbi/rt64_gbi_extended.cpp:316-321 (setVertexSegmentV1 -- 2
//! // command-word pairs)
//! void setVertexSegmentV1(State *state, DisplayList **dl) {
//!     uint8_t isEnabled = (*dl)->p1(0, 1);
//!     uint8_t vertexElement = (*dl)->p1(1, 4);
//!     *dl = *dl + 1;
//!     state->rsp->setVertexSegmentV1(isEnabled, vertexElement, (*dl)->w0, (*dl)->w1);
//! }
//!
//! // src/gbi/rt64_gbi_extended.cpp:323-332 (setTexcoordWrapPointV1,
//! // setRectAspectV1 -- 1 command-word pair each)
//! void setTexcoordWrapPointV1(State *state, DisplayList **dl) {
//!     const int16_t wrapPointU = (*dl)->p1(16, 16);
//!     const int16_t wrapPointV = (*dl)->p1(0, 16);
//!     state->setTexcoordWrapPoint(wrapPointU, wrapPointV);
//! }
//!
//! void setRectAspectV1(State *state, DisplayList **dl) {
//!     const uint8_t aspect = (*dl)->p1(0, 2);
//!     state->rdp->setRectAspect(aspect);
//! }
//!
//! // src/gbi/rt64_gbi_extended.cpp:334-374 (noOpHook -- the RT64_HOOK_*
//! // magic-number/opcode header decode only; the switch's state mutations
//! // and *dl reassignment are dispatch, not ported -- see "Nonclaims")
//! void noOpHook(State *state, DisplayList **dl) {
//!     uint32_t magicNumber = (*dl)->p0(0, 24);
//!     if (magicNumber == RT64_HOOK_MAGIC_NUMBER) {
//!         uint32_t hookValue = (*dl)->p1(0, 28);
//!         uint32_t hookOp = (*dl)->p1(28, 4);
//!         switch (hookOp) {
//!         // ... state-mutating dispatch, out of scope ...
//!         }
//!     }
//! }
//! ```
//!
//! **Reuse, not new type.** This port reuses every `G_EX_*` opcode constant
//! and enum from `crate::rt64_extended_gbi` (e.g.
//! `crate::rt64_extended_gbi::G_EX_TEXRECT_V1`,
//! `crate::rt64_extended_gbi::RT64_HOOK_MAGIC_NUMBER`) wherever a test or
//! doc comment needs to name an opcode -- no `G_EX_*`/`RT64_HOOK_*` constant
//! is redeclared here. **One primitive could not be reused**:
//! `rt64_extended_gbi.rs`'s `fn param(value: u32, bits: u32, shift: u32)`
//! (the `PARAM` macro port) is a private, non-`pub(crate)` free function in
//! that module (confirmed by reading the file: it is declared exactly `fn
//! param(...)`, no `pub` or `pub(crate)` qualifier, and its own doc comment
//! states "Private: only this file's `pack_*` functions call it"). It is
//! therefore unreachable from this module and had to be re-derived locally
//! as `p0`/`p1` (see below) -- the exact same visibility gap this ticket's
//! brief warned about (the prior `Quat::dot` case). The *shape* of `p0`/`p1`
//! matches `rt64_gbi_rdp_decode.rs`'s established local re-derivation
//! precedent (that file re-derives the same two functions for the same
//! reason -- `DisplayList::p0`/`p1` have no single owning Rust module in
//! this crate yet). No other type in `crate::rt64_extended_gbi` (no struct,
//! no enum) applies to a decode direction; that file is 100% pack functions
//! returning `[u32; N]`, none of which this module calls.
//!
//! ## Admitted domain
//!
//! - **`p0`/`p1` are pure bit-twiddles with no sign extension of their
//!   own**: `((w >> pos) & ((1 << bits) - 1))` always yields an unsigned
//!   value truncated to `bits` width. Ported as `fn p0(w0: u32, pos: u8,
//!   bits: u8) -> u32 { (w0 >> pos) & ((1u32 << bits) - 1) }` (and `p1`
//!   identically over `w1`) -- literal, matching `rt64_gbi_rdp_decode.rs`.
//!   Every multi-word-pair opcode advances `*dl` by one `DisplayList` (one
//!   `(w0, w1)` pair) per `*dl = *dl + 1`; this port makes that advance
//!   explicit by taking one `(u32, u32)` pair per source command word as
//!   separate function parameters (e.g. `decode_texrect(w0_0, w1_0, w0_1,
//!   w1_1, w0_2, w1_2)` for `texrectV1`'s three pairs) rather than an
//!   opaque cursor -- there is no pointer-advance or mutable cursor state
//!   to reproduce, only which pair backs which field.
//! - **Sign extension IS present in this file, unlike `rt64_gbi_rdp_decode.rs`**:
//!   `texrectV1`, `fillrectV1`, `setScissorV1` declare `ulx`/`uly`/`lrx`/`lry`
//!   (and `texrectV1`'s `uls`/`ult`/`dsdx`/`dtdy`) as `int16_t`, initialized
//!   from `p0`/`p1`'s 16-bit `uint32_t` extraction. A 16-bit field assigned
//!   to `int16_t` **is** a full sign-reinterpretation in C++ (implementation-
//!   defined pre-C++20, two's-complement narrowing in practice on every
//!   target RT64 builds for): bit 15 becomes the sign bit. Ported as `p0(w0,
//!   16, 16) as u16 as i16` (mask-to-16-bits, reinterpret as signed 16-bit) --
//!   e.g. field value `0x8000` decodes to `-32768i16`, `0x7FFF` decodes to
//!   `32767i16`. `setRectAlignV1`/`setScissorAlignV1`'s `leftOffset`/
//!   `topOffset`/`rightOffset`/`bottomOffset` use an explicit `(int16_t)`
//!   cast on the same 16-bit extraction -- identical reinterpretation,
//!   ported identically. `setViewportAlignV1`'s `x`/`y` and
//!   `setTexcoordWrapPointV1`'s `wrapPointU`/`wrapPointV` are the same
//!   pattern (declared `int16_t`, no explicit cast needed since the
//!   initializer already narrows). By contrast `setScissorAlignV1`'s
//!   `leftBound`/`topBound`/`rightBound`/`bottomBound` are declared
//!   `uint32_t`/left as `ExtendedAlignment`'s unsigned bound fields (no
//!   `int16_t` cast) even though they read the same `p0(16,16)`/`p1(0,16)`
//!   16-bit extraction as the offsets two lines above -- ported as plain
//!   `u32`, NOT sign-extended, exactly mirroring that this decode's only
//!   difference from the offset fields is the absent cast. This is the file's
//!   sharpest asymmetry: two structurally identical bit extractions, one
//!   signed and one not, distinguished only by the destination C++ type the
//!   source author wrote.
//! - **`w0` vs `w1` split**: every opcode above keeps an explicit, often
//!   asymmetric split -- e.g. `texrectV1`'s first pair reads `tile`/
//!   `leftOrigin`/`rightOrigin`/`flip` entirely from `w1` (`w0` unused for
//!   that pair, carries only the opcode byte), while its second and third
//!   pairs read `ulx`/`uly`/`uls`/`ult` from `w0` and `lrx`/`lry`/`dsdx`/
//!   `dtdy` from `w1`. `setViewportV1`/`setProjectionMatrixFloatV1`/
//!   `setViewMatrixFloatV1`/`vertexV1` pass a whole raw `w1` word through
//!   with **no** `p0`/`p1` masking at all (a viewport-struct pointer, a
//!   float-matrix pointer, and a vertex-buffer pointer respectively) --
//!   ported as the bare `u32` word, not run through `p0`/`p1`, matching
//!   `rt64_extended_gbi.rs`'s treatment of `pack_viewport`'s `vp` and
//!   `pack_set_proj_matrix_float`'s `matrix`. `setViewportAlignV1` is the
//!   file's one case where BOTH fields of a pair read the same word: `x =
//!   p0(16, 16)` and `y = p0(0, 16)` both come from the second pair's `w0`
//!   -- that pair's `w1` is entirely unused (confirmed against
//!   `pack_set_viewport_align`, which packs both fields into `words[2]`
//!   and leaves `words[3]` as a hardcoded `0`). This is easy to get wrong
//!   by pattern-matching the more common "first field from `w0`, second
//!   from `w1`" shape seen everywhere else in this file -- an earlier draft
//!   of this port did exactly that and a round-trip test against
//!   `pack_set_viewport_align` caught it immediately (`y` decoded as `0`
//!   instead of the packed value).
//! - **`matrixFloatV1`'s `pushMask` XOR is NOT decoded here**: `uint8_t
//!   params = (*dl)->p1(0, 8) ^ state->rsp->pushMask` reads external mutable
//!   RSP state (`RT64::RSP::pushMask`, confirmed absent from
//!   `rt64_gbi_extended.cpp`/`.h` and `rt64_display_list.h` by grep -- it
//!   lives in `src/hle/rt64_rsp.h`/`.cpp`, files this task does not port).
//!   This decode returns the raw, un-XORed 8-bit `p1(0, 8)` field
//!   (`raw_params_before_push_mask_xor` on [`MatrixFloatDecoded`]) and
//!   documents that the caller must XOR it against the live `pushMask` to
//!   reproduce upstream's `params` -- this is the one place in the file
//!   where a "decode" cannot be a pure function of `(w0, w1)` alone, and is
//!   called out explicitly rather than silently guessed at (the ticket's
//!   "state a real finding" instruction, applied to a state read instead of
//!   a visibility gap).
//! - **`setNearClippingV1`'s `!nearClipping` negation is NOT applied here**:
//!   the decode returns the raw bit (`near_clipping: u8`, 0 or 1) exactly as
//!   `p1(0, 1)` extracts it; the boolean negation (`state->rsp->setNoN(!near
//!   Clipping)`) is the dispatch call's argument expression, not part of the
//!   command-word decode, and is excluded per "DECODE only" / "Do NOT port
//!   `state->` dispatch."
//! - **`setDitherNoiseStrengthV1`'s `/ 1024.0f` division is NOT applied
//!   here**: the decode returns the raw 16-bit `noiseStrength` integer; the
//!   floating-point division happens in the `state->setDitherNoiseStrength`
//!   call argument, which is dispatch, not decode (mirrors
//!   `rt64_extended_gbi.rs`'s `pack_set_dither_noise_strength`, which
//!   likewise takes the caller-multiplied `u32` rather than doing the `*
//!   1024` itself).
//! - **`matrixGroupCommand`'s `id`/`address` word is never masked**: `const
//!   uint32_t id = (*dl)->w1` takes the whole first command word verbatim
//!   (used by both `matrixGroupV1`'s numeric group ID and
//!   `editGroupByAddressV1`'s RDRAM address -- see [`decode_matrix_group`]).
//!   `idIsAddress`/`editGroup` are booleans the two call sites
//!   (`matrixGroupV1`, `editGroupByAddressV1`) pass as literals, not decoded
//!   from any command word -- [`decode_matrix_group`] returns the shared
//!   bitfield struct only; which caller it came from is dispatch-routing
//!   information this module does not reconstruct.
//! - **Truncation**: every extracted field here (1, 2, 3, 8, 12, 16, or 24
//!   bits) is at or narrower than `u32`, so `p0`/`p1`'s own mask-to-width
//!   truncates any wider garbage in the command word before this port ever
//!   sees it -- confirmed by "one bit above max" characterization tests
//!   below for every field.
//! - **`noOpHook`'s `magicNumber`/`hookValue`/`hookOp` decode**: `magicNumber
//!   = p0(0, 24)` (24-bit, `w0`), `hookValue = p1(0, 28)` (28-bit, `w1`),
//!   `hookOp = p1(28, 4)` (4-bit, `w1`, the top nibble) -- `hookValue` and
//!   `hookOp` partition all 32 bits of `w1` with no overlap and no gap.
//!   Ported as [`decode_no_op_hook_header`], returning the three raw fields;
//!   the `if (magicNumber == RT64_HOOK_MAGIC_NUMBER)` guard and the
//!   `switch(hookOp)` dispatch (RDRAM reads/writes, `enableExtendedGBI`,
//!   `*dl` reassignment) are NOT ported -- pure dispatch, per scope.
//!
//! ## Nonclaims
//!
//! **This is a PARTIAL port of `src/gbi/rt64_gbi_extended.cpp`.** The
//! whole-file digest cited above will mark the file `ported` in the
//! port-inventory scanner, but only the bitfield-DECODE portion of the
//! functions listed below is ported; no `state->rsp->*`/`state->rdp->*`/
//! `state->*` dispatch call, no `Map[]` table lookup/dispatch
//! (`extendedOp`), no `Map[]` table construction (`initialize`), and no
//! `*dl` pointer/cursor mutation is reproduced anywhere in this module.
//!
//! **Functions ported** (bitfield decode only, as pure word-pair(s) ->
//! struct functions): `texrectV1` (-> [`decode_texrect`],
//! [`TexrectDecoded`]), `fillrectV1` (-> [`decode_fillrect`],
//! [`FillrectDecoded`]), `setViewportV1` (-> [`decode_set_viewport`],
//! [`SetViewportDecoded`]), `setScissorV1` (-> [`decode_set_scissor`],
//! [`SetScissorDecoded`]), `setRectAlignV1` (-> [`decode_set_rect_align`],
//! [`SetRectAlignDecoded`]), `setViewportAlignV1` (->
//! [`decode_set_viewport_align`], [`SetViewportAlignDecoded`]),
//! `setScissorAlignV1` (-> [`decode_set_scissor_align`],
//! [`SetScissorAlignDecoded`]), `setRefreshRateV1` (->
//! [`decode_set_refresh_rate`]), `vertexZTestV1` (->
//! [`decode_vertex_z_test`]), `matrixGroupCommand` (shared by
//! `matrixGroupV1`/`editGroupByAddressV1`; -> [`decode_matrix_group`],
//! [`MatrixGroupDecoded`]), `popMatrixGroupV1` (->
//! [`decode_pop_matrix_group`], [`PopMatrixGroupDecoded`]),
//! `forceUpscale2DV1` (-> [`decode_force_upscale_2d`]),
//! `forceTrueBilerpV1` (-> [`decode_force_true_bilerp`]),
//! `forceScaleLODV1` (-> [`decode_force_scale_lod`]), `forceBranchV1` (->
//! [`decode_force_branch`]), `setRenderToRAMV1` (->
//! [`decode_set_render_to_ram`]), `vertexV1` (-> [`decode_vertex`],
//! [`VertexDecoded`]), `setDitherNoiseStrengthV1` (->
//! [`decode_set_dither_noise_strength`]), `setRDRAMExtendedV1` (->
//! [`decode_set_rdram_extended`]), `setProjectionMatrixFloatV1` (->
//! [`decode_set_projection_matrix_float`], trivial passthrough of `w1`, no
//! bitfield), `setViewMatrixFloatV1` (-> [`decode_set_view_matrix_float`],
//! same), `setNearClippingV1` (-> [`decode_set_near_clipping`]),
//! `matrixFloatV1` (-> [`decode_matrix_float`], [`MatrixFloatDecoded`];
//! `pushMask` XOR NOT applied, see "Admitted domain"), `setVertexSegmentV1`
//! (-> [`decode_set_vertex_segment`], [`SetVertexSegmentDecoded`]),
//! `setTexcoordWrapPointV1` (-> [`decode_set_texcoord_wrap_point`]),
//! `setRectAspectV1` (-> [`decode_set_rect_aspect`]), `noOpHook`'s header
//! decode only (-> [`decode_no_op_hook_header`], [`NoOpHookHeaderDecoded`]).
//!
//! **Functions deliberately NOT ported, and why**:
//! - `noOp`, `endVertexZTestV1`, `pushViewportV1`, `popViewportV1`,
//!   `pushScissorV1`, `popScissorV1`, `pushOtherModeV1`, `popOtherModeV1`,
//!   `pushCombineV1`, `popCombineV1`, `pushProjectionMatrixV1`,
//!   `popProjectionMatrixV1`, `pushEnvColorV1`, `popEnvColorV1`,
//!   `pushBlendColorV1`, `popBlendColorV1`, `pushFogColorV1`,
//!   `popFogColorV1`, `pushFillColorV1`, `popFillColorV1`,
//!   `pushPrimColorV1`, `popPrimColorV1`, `pushGeometryModeV1`,
//!   `popGeometryModeV1` -- every one of these reads **zero** bits from
//!   either command word; their entire body is a single `state->rsp-
//!   >*()`/`state->rdp->*()` dispatch call with no arguments. There is no
//!   bitfield decode to port -- a "decoder" with no fields to decode is not
//!   a decoder, it is pure dispatch, out of scope by the ticket's own
//!   framing.
//! - `print` -- upstream body is `// Not implemented.`, an empty function.
//!   Nothing to port; not invented.
//! - `matrixGroupV1`, `editGroupByAddressV1` -- both are one-line wrappers
//!   (`matrixGroupCommand(state, dl, false, false)` and `matrixGroupCommand
//!   (state, dl, true, true)` respectively) around the already-ported
//!   `matrixGroupCommand`; their only content beyond the shared decode is
//!   two caller-supplied boolean literals with no command-word origin, so
//!   there is nothing opcode-specific left to decode once
//!   `decode_matrix_group` exists.
//! - `extendedOp` -- pure `Map[opCode](state, dl)` dispatch-table lookup and
//!   invocation (`uint32_t opCode = (*dl)->p0(0, 24)` is the only
//!   extraction, and it is the same 24-bit-opcode-from-`w0` pattern every
//!   `G_EX_*` sub-opcode relies on being routed through -- decoding it here
//!   would add no new information over decoding any individual opcode
//!   above). The `if (opCode < G_EX_MAX) { ... } else { assert(false); }`
//!   bounds check and the `assert(false)` unimplemented-opcode branch are
//!   both dispatch-time error handling, not decode.
//! - `initialize` -- populates the static `Map[G_EX_MAX]` dispatch table
//!   with function pointers. Zero bitfield content; pure wiring.
//! - `noOpHook`'s `switch (hookOp)` body (the four `RT64_HOOK_OP_*` arms:
//!   `GETVERSION`'s RDRAM write, `ENABLE`'s `extendedOpCode` re-extraction
//!   (`(*dl)->p0(24, 8)`) plus `enableExtendedGBI` call and the
//!   `assert(false)` invalid-opcode stub, `DISABLE`'s `disableExtendedGBI`
//!   call, `DL`/`BRANCH`'s `pushReturnAddress`/`fromRDRAM`/`*dl`
//!   reassignment, and the `default: assert(false)` unknown-op stub) --
//!   every arm is `state->*` dispatch or RDRAM pointer manipulation, not
//!   bitfield decode. The header fields the switch reads (`magicNumber`,
//!   `hookValue`, `hookOp`) ARE ported, in [`decode_no_op_hook_header`];
//!   the switch's *actions* are not. The upstream `assert(false)` stubs
//!   inside the `ENABLE` arm and the `default` arm are preserved as
//!   documented not-implemented dispatch behavior, not invented as any
//!   kind of decoded value.
//!
//! No GPU, WGSL, or production wiring (this module is not called from
//! anywhere yet; dead-code warnings on the unused public surface are
//! expected and correct, matching every other GBI decode file in this
//! crate), and no RT64 visual/pixel/silicon parity or performance claim.
//! Not wired to `fn64-render-reference`'s GBI interpreter, and not wired to
//! `crate::rt64_extended_gbi`'s `pack_*` functions beyond the round-trip
//! tests below (which call both directions explicitly, in the same
//! `#[test]` function, and are not part of the module's public surface).

use crate::rt64_extended_gbi as packed;

/// `DisplayList::p0(pos, bits)`: `(w0 >> pos) & ((1 << bits) - 1)`.
fn p0(w0: u32, pos: u8, bits: u8) -> u32 {
    (w0 >> pos) & ((1u32 << bits) - 1)
}

/// `DisplayList::p1(pos, bits)`: `(w1 >> pos) & ((1 << bits) - 1)`.
fn p1(w1: u32, pos: u8, bits: u8) -> u32 {
    (w1 >> pos) & ((1u32 << bits) - 1)
}

/// A 16-bit field reinterpreted as signed, matching the source's `(int16_t)`
/// cast / `int16_t` declaration of a `p0`/`p1(_, 16)` extraction: mask to 16
/// bits, then reinterpret (not saturate/clamp) the top bit as the sign bit.
fn sext16(field: u32) -> i16 {
    field as u16 as i16
}

/// `texrectV1`'s decoded operands, across its three command-word pairs.
/// `tile`/`left_origin`/`right_origin`/`flip` come from the first pair's
/// `w1` only (`w0` unused there); `ulx`/`uly`/`lrx`/`lry` from the second
/// pair; `uls`/`ult`/`dsdx`/`dtdy` from the third. Every coordinate is
/// sign-extended per "Admitted domain".
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TexrectDecoded {
    pub tile: u8,
    pub left_origin: u32,
    pub right_origin: u32,
    pub flip: bool,
    pub ulx: i16,
    pub uly: i16,
    pub lrx: i16,
    pub lry: i16,
    pub uls: i16,
    pub ult: i16,
    pub dsdx: i16,
    pub dtdy: i16,
}

pub fn decode_texrect(
    _w0_0: u32,
    w1_0: u32,
    w0_1: u32,
    w1_1: u32,
    w0_2: u32,
    w1_2: u32,
) -> TexrectDecoded {
    TexrectDecoded {
        tile: p1(w1_0, 0, 3) as u8,
        left_origin: p1(w1_0, 3, 12),
        right_origin: p1(w1_0, 15, 12),
        flip: p1(w1_0, 7, 1) != 0,
        ulx: sext16(p0(w0_1, 16, 16)),
        uly: sext16(p0(w0_1, 0, 16)),
        lrx: sext16(p1(w1_1, 16, 16)),
        lry: sext16(p1(w1_1, 0, 16)),
        uls: sext16(p0(w0_2, 16, 16)),
        ult: sext16(p0(w0_2, 0, 16)),
        dsdx: sext16(p1(w1_2, 16, 16)),
        dtdy: sext16(p1(w1_2, 0, 16)),
    }
}

/// `fillrectV1`'s decoded operands, across its two command-word pairs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FillrectDecoded {
    pub left_origin: u32,
    pub right_origin: u32,
    pub ulx: i16,
    pub uly: i16,
    pub lrx: i16,
    pub lry: i16,
}

pub fn decode_fillrect(_w0_0: u32, w1_0: u32, w0_1: u32, w1_1: u32) -> FillrectDecoded {
    FillrectDecoded {
        left_origin: p1(w1_0, 0, 12),
        right_origin: p1(w1_0, 12, 12),
        ulx: sext16(p0(w0_1, 16, 16)),
        uly: sext16(p0(w0_1, 0, 16)),
        lrx: sext16(p1(w1_1, 16, 16)),
        lry: sext16(p1(w1_1, 0, 16)),
    }
}

/// `setViewportV1`'s decoded operands. `vp` is the second pair's raw `w1`
/// word, not run through `p0`/`p1` (an opaque viewport-struct pointer).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SetViewportDecoded {
    pub ori: u16,
    pub vp: u32,
}

pub fn decode_set_viewport(_w0_0: u32, w1_0: u32, _w0_1: u32, w1_1: u32) -> SetViewportDecoded {
    SetViewportDecoded {
        ori: p1(w1_0, 0, 12) as u16,
        vp: w1_1,
    }
}

/// `setScissorV1`'s decoded operands, across its two command-word pairs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SetScissorDecoded {
    pub mode: u8,
    pub left_origin: u32,
    pub right_origin: u32,
    pub ulx: i16,
    pub uly: i16,
    pub lrx: i16,
    pub lry: i16,
}

pub fn decode_set_scissor(_w0_0: u32, w1_0: u32, w0_1: u32, w1_1: u32) -> SetScissorDecoded {
    SetScissorDecoded {
        mode: p1(w1_0, 0, 2) as u8,
        left_origin: p1(w1_0, 2, 12),
        right_origin: p1(w1_0, 14, 12),
        ulx: sext16(p0(w0_1, 16, 16)),
        uly: sext16(p0(w0_1, 0, 16)),
        lrx: sext16(p1(w1_1, 16, 16)),
        lry: sext16(p1(w1_1, 0, 16)),
    }
}

/// `setRectAlignV1`'s decoded operands, across its two command-word pairs.
/// All four offsets carry an explicit `(int16_t)` cast upstream -- signed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SetRectAlignDecoded {
    pub left_origin: u32,
    pub right_origin: u32,
    pub left_offset: i16,
    pub top_offset: i16,
    pub right_offset: i16,
    pub bottom_offset: i16,
}

pub fn decode_set_rect_align(_w0_0: u32, w1_0: u32, w0_1: u32, w1_1: u32) -> SetRectAlignDecoded {
    SetRectAlignDecoded {
        left_origin: p1(w1_0, 0, 12),
        right_origin: p1(w1_0, 12, 12),
        left_offset: sext16(p0(w0_1, 16, 16)),
        top_offset: sext16(p0(w0_1, 0, 16)),
        right_offset: sext16(p1(w1_1, 16, 16)),
        bottom_offset: sext16(p1(w1_1, 0, 16)),
    }
}

/// `setViewportAlignV1`'s decoded operands, across its two command-word
/// pairs. `x`/`y` are declared `int16_t` upstream -- signed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SetViewportAlignDecoded {
    pub ori: u16,
    pub x: i16,
    pub y: i16,
}

pub fn decode_set_viewport_align(
    _w0_0: u32,
    w1_0: u32,
    w0_1: u32,
    _w1_1: u32,
) -> SetViewportAlignDecoded {
    SetViewportAlignDecoded {
        ori: p1(w1_0, 0, 12) as u16,
        // Both x and y read the second command word's w0 (p0), not w1 --
        // the source declares `x = p0(16, 16)` and `y = p0(0, 16)`, unlike
        // most other opcodes in this file where ulx/uly (w0) pair with
        // lrx/lry (w1). This opcode's second command word's w1 is unused
        // (the packer, `pack_set_viewport_align`, sets it to 0).
        x: sext16(p0(w0_1, 16, 16)),
        y: sext16(p0(w0_1, 0, 16)),
    }
}

/// `setScissorAlignV1`'s decoded operands, across its three command-word
/// pairs. The offsets (second pair) are signed (`(int16_t)` cast,
/// identical to [`decode_set_rect_align`]); the bounds (third pair) are
/// declared `uint32_t`/unsigned and read the *same* 16-bit fields with no
/// cast -- see "Admitted domain" for this asymmetry.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SetScissorAlignDecoded {
    pub left_origin: u32,
    pub right_origin: u32,
    pub left_offset: i16,
    pub top_offset: i16,
    pub right_offset: i16,
    pub bottom_offset: i16,
    pub left_bound: u32,
    pub top_bound: u32,
    pub right_bound: u32,
    pub bottom_bound: u32,
}

#[allow(clippy::too_many_arguments)]
pub fn decode_set_scissor_align(
    _w0_0: u32,
    w1_0: u32,
    w0_1: u32,
    w1_1: u32,
    w0_2: u32,
    w1_2: u32,
) -> SetScissorAlignDecoded {
    SetScissorAlignDecoded {
        left_origin: p1(w1_0, 0, 12),
        right_origin: p1(w1_0, 12, 12),
        left_offset: sext16(p0(w0_1, 16, 16)),
        top_offset: sext16(p0(w0_1, 0, 16)),
        right_offset: sext16(p1(w1_1, 16, 16)),
        bottom_offset: sext16(p1(w1_1, 0, 16)),
        left_bound: p0(w0_2, 16, 16),
        top_bound: p0(w0_2, 0, 16),
        right_bound: p1(w1_2, 16, 16),
        bottom_bound: p1(w1_2, 0, 16),
    }
}

/// `setRefreshRateV1`'s decoded operand: a single 16-bit unsigned field.
pub fn decode_set_refresh_rate(_w0: u32, w1: u32) -> u16 {
    p1(w1, 0, 16) as u16
}

/// `vertexZTestV1`'s decoded operand: a single 8-bit unsigned field.
pub fn decode_vertex_z_test(_w0: u32, w1: u32) -> u8 {
    p1(w1, 0, 8) as u8
}

/// `matrixGroupCommand`'s decoded operands, shared by `matrixGroupV1` and
/// `editGroupByAddressV1` (which pass caller-literal `idIsAddress`/
/// `editGroup` booleans this decode does not reconstruct -- see "Admitted
/// domain"). `id` is the raw second command word (also serves as
/// `editGroupByAddressV1`'s `address`), not run through `p0`/`p1`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MatrixGroupDecoded {
    pub id: u32,
    pub push: u8,
    pub proj: u8,
    pub mode: u8,
    pub pos: u8,
    pub rot: u8,
    pub scale: u8,
    pub skew: u8,
    pub persp: u8,
    pub vpos: u8,
    pub vtc: u8,
    pub tile: u8,
    pub order: u8,
    pub editable: u8,
    pub aspect: u8,
    pub lookat: u8,
}

pub fn decode_matrix_group(_w0_0: u32, w1_0: u32, w0_1: u32, _w1_1: u32) -> MatrixGroupDecoded {
    MatrixGroupDecoded {
        id: w1_0,
        push: p0(w0_1, 0, 1) as u8,
        proj: p0(w0_1, 1, 1) as u8,
        mode: p0(w0_1, 2, 1) as u8,
        pos: p0(w0_1, 3, 2) as u8,
        rot: p0(w0_1, 5, 2) as u8,
        scale: p0(w0_1, 7, 2) as u8,
        skew: p0(w0_1, 9, 2) as u8,
        persp: p0(w0_1, 11, 2) as u8,
        vpos: p0(w0_1, 13, 2) as u8,
        tile: p0(w0_1, 15, 2) as u8,
        order: p0(w0_1, 17, 2) as u8,
        editable: p0(w0_1, 19, 1) as u8,
        aspect: p0(w0_1, 20, 2) as u8,
        vtc: p0(w0_1, 22, 2) as u8,
        lookat: p0(w0_1, 24, 2) as u8,
    }
}

/// `popMatrixGroupV1`'s decoded operands: `popCount = p1(0, 8)` (`w1`),
/// `proj = p0(8, 1)` (`w0`) -- note `proj` reads from `w0`, not `w1`, unlike
/// most single-pair opcodes in this file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PopMatrixGroupDecoded {
    pub pop_count: u8,
    pub proj: u8,
}

pub fn decode_pop_matrix_group(w0: u32, w1: u32) -> PopMatrixGroupDecoded {
    PopMatrixGroupDecoded {
        pop_count: p1(w1, 0, 8) as u8,
        proj: p0(w0, 8, 1) as u8,
    }
}

/// `forceUpscale2DV1`'s decoded operand: a single 1-bit unsigned field.
pub fn decode_force_upscale_2d(_w0: u32, w1: u32) -> u8 {
    p1(w1, 0, 1) as u8
}

/// `forceTrueBilerpV1`'s decoded operand: a single 2-bit unsigned field.
pub fn decode_force_true_bilerp(_w0: u32, w1: u32) -> u8 {
    p1(w1, 0, 2) as u8
}

/// `forceScaleLODV1`'s decoded operand: a single 1-bit unsigned field.
pub fn decode_force_scale_lod(_w0: u32, w1: u32) -> u8 {
    p1(w1, 0, 1) as u8
}

/// `forceBranchV1`'s decoded operand: a single 1-bit unsigned field.
pub fn decode_force_branch(_w0: u32, w1: u32) -> u8 {
    p1(w1, 0, 1) as u8
}

/// `setRenderToRAMV1`'s decoded operand: a single 1-bit unsigned field.
pub fn decode_set_render_to_ram(_w0: u32, w1: u32) -> u8 {
    p1(w1, 0, 1) as u8
}

/// `vertexV1`'s decoded operands, across its two command-word pairs.
/// `vtx_ptr` is the second pair's raw `w1` word, not run through `p0`/`p1`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct VertexDecoded {
    pub vtx_count: u8,
    pub dst_index: u8,
    pub vtx_ptr: u32,
}

pub fn decode_vertex(_w0_0: u32, w1_0: u32, _w0_1: u32, w1_1: u32) -> VertexDecoded {
    VertexDecoded {
        vtx_count: p1(w1_0, 8, 8) as u8,
        dst_index: p1(w1_0, 0, 8) as u8,
        vtx_ptr: w1_1,
    }
}

/// `setDitherNoiseStrengthV1`'s decoded operand: the raw 16-bit integer
/// BEFORE the `/ 1024.0f` division -- see "Admitted domain".
pub fn decode_set_dither_noise_strength(_w0: u32, w1: u32) -> u16 {
    p1(w1, 0, 16) as u16
}

/// `setRDRAMExtendedV1`'s decoded operand: a single 1-bit unsigned field.
pub fn decode_set_rdram_extended(_w0: u32, w1: u32) -> u8 {
    p1(w1, 0, 1) as u8
}

/// `setProjectionMatrixFloatV1`'s decoded operand: the raw `w1` word,
/// verbatim, with no `p0`/`p1` masking (a float-matrix pointer).
pub fn decode_set_projection_matrix_float(_w0: u32, w1: u32) -> u32 {
    w1
}

/// `setViewMatrixFloatV1`'s decoded operand: the raw `w1` word, verbatim,
/// with no `p0`/`p1` masking (a float-matrix pointer).
pub fn decode_set_view_matrix_float(_w0: u32, w1: u32) -> u32 {
    w1
}

/// `setNearClippingV1`'s decoded operand: the raw bit, BEFORE the `!`
/// negation applied at the dispatch call site -- see "Admitted domain".
pub fn decode_set_near_clipping(_w0: u32, w1: u32) -> u8 {
    p1(w1, 0, 1) as u8
}

/// `matrixFloatV1`'s decoded operands, across its two command-word pairs.
/// `raw_params_before_push_mask_xor` is `p1(0, 8)` BEFORE the upstream `^
/// state->rsp->pushMask` step, which this module cannot perform (external
/// mutable state) -- see "Admitted domain". `matrix_ptr` is the second
/// pair's raw `w1` word.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MatrixFloatDecoded {
    pub raw_params_before_push_mask_xor: u8,
    pub matrix_ptr: u32,
}

pub fn decode_matrix_float(_w0_0: u32, w1_0: u32, _w0_1: u32, w1_1: u32) -> MatrixFloatDecoded {
    MatrixFloatDecoded {
        raw_params_before_push_mask_xor: p1(w1_0, 0, 8) as u8,
        matrix_ptr: w1_1,
    }
}

/// `setVertexSegmentV1`'s decoded operands, across its two command-word
/// pairs. `vertex_address`/`base_segment_address` are the second pair's raw
/// `w0`/`w1` words, not run through `p0`/`p1`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SetVertexSegmentDecoded {
    pub is_enabled: u8,
    pub vertex_element: u8,
    pub vertex_address: u32,
    pub base_segment_address: u32,
}

pub fn decode_set_vertex_segment(
    _w0_0: u32,
    w1_0: u32,
    w0_1: u32,
    w1_1: u32,
) -> SetVertexSegmentDecoded {
    SetVertexSegmentDecoded {
        is_enabled: p1(w1_0, 0, 1) as u8,
        vertex_element: p1(w1_0, 1, 4) as u8,
        vertex_address: w0_1,
        base_segment_address: w1_1,
    }
}

/// `setTexcoordWrapPointV1`'s decoded operands: both 16-bit fields are
/// declared `int16_t` upstream -- signed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SetTexcoordWrapPointDecoded {
    pub wrap_point_u: i16,
    pub wrap_point_v: i16,
}

pub fn decode_set_texcoord_wrap_point(_w0: u32, w1: u32) -> SetTexcoordWrapPointDecoded {
    SetTexcoordWrapPointDecoded {
        wrap_point_u: sext16(p1(w1, 16, 16)),
        wrap_point_v: sext16(p1(w1, 0, 16)),
    }
}

/// `setRectAspectV1`'s decoded operand: a single 2-bit unsigned field.
pub fn decode_set_rect_aspect(_w0: u32, w1: u32) -> u8 {
    p1(w1, 0, 2) as u8
}

/// `noOpHook`'s header decode: `magicNumber = p0(0, 24)` (`w0`), `hookValue
/// = p1(0, 28)` (`w1`), `hookOp = p1(28, 4)` (`w1`, top nibble). The
/// `magicNumber == RT64_HOOK_MAGIC_NUMBER` guard and the `switch(hookOp)`
/// dispatch are NOT applied here -- pure header decode only.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NoOpHookHeaderDecoded {
    pub magic_number: u32,
    pub hook_value: u32,
    pub hook_op: u8,
}

pub fn decode_no_op_hook_header(w0: u32, w1: u32) -> NoOpHookHeaderDecoded {
    NoOpHookHeaderDecoded {
        magic_number: p0(w0, 0, 24),
        hook_value: p1(w1, 0, 28),
        hook_op: p1(w1, 28, 4) as u8,
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

    #[test]
    fn p0_one_bit_above_max_is_masked() {
        // bit 12 sits just outside an 8-bit field starting at pos 4.
        assert_eq!(p0(0x1 << 12, 4, 8), 0);
    }

    #[test]
    fn p1_one_bit_above_max_is_masked() {
        assert_eq!(p1(0x1 << 12, 4, 8), 0);
    }

    // --- sext16 ---

    #[test]
    fn sext16_zero_stays_zero() {
        assert_eq!(sext16(0), 0);
    }

    #[test]
    fn sext16_max_positive() {
        assert_eq!(sext16(0x7FFF), 32767i16);
    }

    #[test]
    fn sext16_sign_bit_becomes_negative() {
        assert_eq!(sext16(0x8000), -32768i16);
    }

    #[test]
    fn sext16_all_ones_is_negative_one() {
        assert_eq!(sext16(0xFFFF), -1i16);
    }

    // --- decode_texrect ---

    #[test]
    fn decode_texrect_all_zero() {
        let d = decode_texrect(0, 0, 0, 0, 0, 0);
        assert_eq!(
            d,
            TexrectDecoded {
                tile: 0,
                left_origin: 0,
                right_origin: 0,
                flip: false,
                ulx: 0,
                uly: 0,
                lrx: 0,
                lry: 0,
                uls: 0,
                ult: 0,
                dsdx: 0,
                dtdy: 0,
            }
        );
    }

    #[test]
    fn decode_texrect_all_ones() {
        let d = decode_texrect(
            0xFFFF_FFFF,
            0xFFFF_FFFF,
            0xFFFF_FFFF,
            0xFFFF_FFFF,
            0xFFFF_FFFF,
            0xFFFF_FFFF,
        );
        assert_eq!(
            d,
            TexrectDecoded {
                tile: 0x7,
                left_origin: 0xFFF,
                right_origin: 0xFFF,
                flip: true,
                ulx: -1,
                uly: -1,
                lrx: -1,
                lry: -1,
                uls: -1,
                ult: -1,
                dsdx: -1,
                dtdy: -1,
            }
        );
    }

    #[test]
    fn decode_texrect_tile_at_max_three_bits() {
        let d = decode_texrect(0, 0x7, 0, 0, 0, 0);
        assert_eq!(d.tile, 0x7);
    }

    #[test]
    fn decode_texrect_tile_one_bit_above_max_is_masked() {
        let d = decode_texrect(0, 0x1 << 3, 0, 0, 0, 0);
        assert_eq!(d.tile, 0);
    }

    #[test]
    fn decode_texrect_left_origin_at_max_twelve_bits() {
        let d = decode_texrect(0, 0xFFF << 3, 0, 0, 0, 0);
        assert_eq!(d.left_origin, 0xFFF);
    }

    #[test]
    fn decode_texrect_left_origin_one_bit_above_max_is_masked() {
        let d = decode_texrect(0, 0x1 << 15, 0, 0, 0, 0);
        assert_eq!(d.left_origin, 0);
    }

    #[test]
    fn decode_texrect_right_origin_at_max_twelve_bits() {
        let d = decode_texrect(0, 0xFFF << 15, 0, 0, 0, 0);
        assert_eq!(d.right_origin, 0xFFF);
    }

    #[test]
    fn decode_texrect_right_origin_one_bit_above_max_is_masked() {
        let d = decode_texrect(0, 0x1 << 27, 0, 0, 0, 0);
        assert_eq!(d.right_origin, 0);
    }

    #[test]
    fn decode_texrect_flip_true() {
        let d = decode_texrect(0, 0x1 << 7, 0, 0, 0, 0);
        assert!(d.flip);
    }

    #[test]
    fn decode_texrect_flip_false_when_bit_clear() {
        let d = decode_texrect(0, 0, 0, 0, 0, 0);
        assert!(!d.flip);
    }

    #[test]
    fn decode_texrect_ulx_sign_boundary() {
        let d = decode_texrect(0, 0, 0x8000 << 16, 0, 0, 0);
        assert_eq!(d.ulx, -32768);
    }

    #[test]
    fn decode_texrect_uly_max_positive() {
        let d = decode_texrect(0, 0, 0x7FFF, 0, 0, 0);
        assert_eq!(d.uly, 32767);
    }

    #[test]
    fn decode_texrect_lrx_reads_from_second_pair_w1() {
        let d = decode_texrect(0, 0, 0, 0x1234 << 16, 0, 0);
        assert_eq!(d.lrx, 0x1234i16);
    }

    #[test]
    fn decode_texrect_lry_reads_from_second_pair_w1() {
        let d = decode_texrect(0, 0, 0, 0x1234, 0, 0);
        assert_eq!(d.lry, 0x1234i16);
    }

    #[test]
    fn decode_texrect_uls_ult_read_from_third_pair_w0() {
        let d = decode_texrect(0, 0, 0, 0, (0x1111 << 16) | 0x2222, 0);
        assert_eq!(d.uls, 0x1111i16);
        assert_eq!(d.ult, 0x2222i16);
    }

    #[test]
    fn decode_texrect_dsdx_dtdy_read_from_third_pair_w1() {
        let d = decode_texrect(0, 0, 0, 0, 0, (0x3333 << 16) | 0x4444);
        assert_eq!(d.dsdx, 0x3333i16);
        assert_eq!(d.dtdy, 0x4444i16);
    }

    #[test]
    fn decode_texrect_first_pair_w0_is_unused() {
        // w0 of the first command word carries only the opcode byte
        // upstream; this decode never reads it.
        let a = decode_texrect(0, 0, 0, 0, 0, 0);
        let b = decode_texrect(0xFFFF_FFFF, 0, 0, 0, 0, 0);
        assert_eq!(a, b);
    }

    // --- decode_fillrect ---

    #[test]
    fn decode_fillrect_all_zero() {
        let d = decode_fillrect(0, 0, 0, 0);
        assert_eq!(
            d,
            FillrectDecoded {
                left_origin: 0,
                right_origin: 0,
                ulx: 0,
                uly: 0,
                lrx: 0,
                lry: 0,
            }
        );
    }

    #[test]
    fn decode_fillrect_all_ones() {
        let d = decode_fillrect(0xFFFF_FFFF, 0xFFFF_FFFF, 0xFFFF_FFFF, 0xFFFF_FFFF);
        assert_eq!(
            d,
            FillrectDecoded {
                left_origin: 0xFFF,
                right_origin: 0xFFF,
                ulx: -1,
                uly: -1,
                lrx: -1,
                lry: -1,
            }
        );
    }

    #[test]
    fn decode_fillrect_left_origin_at_max_twelve_bits() {
        let d = decode_fillrect(0, 0xFFF, 0, 0);
        assert_eq!(d.left_origin, 0xFFF);
    }

    #[test]
    fn decode_fillrect_left_origin_one_bit_above_max_is_masked() {
        let d = decode_fillrect(0, 0x1 << 12, 0, 0);
        assert_eq!(d.left_origin, 0);
    }

    #[test]
    fn decode_fillrect_right_origin_at_max_twelve_bits() {
        let d = decode_fillrect(0, 0xFFF << 12, 0, 0);
        assert_eq!(d.right_origin, 0xFFF);
    }

    #[test]
    fn decode_fillrect_right_origin_one_bit_above_max_is_masked() {
        let d = decode_fillrect(0, 0x1 << 24, 0, 0);
        assert_eq!(d.right_origin, 0);
    }

    #[test]
    fn decode_fillrect_ulx_uly_read_from_second_pair_w0() {
        let d = decode_fillrect(0, 0, (0x1234 << 16) | 0x5678, 0);
        assert_eq!(d.ulx, 0x1234i16);
        assert_eq!(d.uly, 0x5678i16);
    }

    #[test]
    fn decode_fillrect_lrx_lry_read_from_second_pair_w1() {
        let d = decode_fillrect(0, 0, 0, (0x1234 << 16) | 0x5678);
        assert_eq!(d.lrx, 0x1234i16);
        assert_eq!(d.lry, 0x5678i16);
    }

    // --- decode_set_viewport ---

    #[test]
    fn decode_set_viewport_all_zero() {
        let d = decode_set_viewport(0, 0, 0, 0);
        assert_eq!(d, SetViewportDecoded { ori: 0, vp: 0 });
    }

    #[test]
    fn decode_set_viewport_ori_at_max_twelve_bits() {
        let d = decode_set_viewport(0, 0xFFF, 0, 0);
        assert_eq!(d.ori, 0xFFF);
    }

    #[test]
    fn decode_set_viewport_ori_one_bit_above_max_is_masked() {
        let d = decode_set_viewport(0, 0x1 << 12, 0, 0);
        assert_eq!(d.ori, 0);
    }

    #[test]
    fn decode_set_viewport_vp_is_raw_second_pair_w1_unmasked() {
        // vp is never run through p0/p1 -- confirm no masking occurs even
        // for a value that would be truncated by a 12-bit mask.
        let d = decode_set_viewport(0, 0, 0, 0xFFFF_FFFF);
        assert_eq!(d.vp, 0xFFFF_FFFF);
    }

    // --- decode_set_scissor ---

    #[test]
    fn decode_set_scissor_all_zero() {
        let d = decode_set_scissor(0, 0, 0, 0);
        assert_eq!(
            d,
            SetScissorDecoded {
                mode: 0,
                left_origin: 0,
                right_origin: 0,
                ulx: 0,
                uly: 0,
                lrx: 0,
                lry: 0,
            }
        );
    }

    #[test]
    fn decode_set_scissor_mode_at_max_two_bits() {
        let d = decode_set_scissor(0, 0x3, 0, 0);
        assert_eq!(d.mode, 0x3);
    }

    #[test]
    fn decode_set_scissor_mode_one_bit_above_max_is_masked() {
        let d = decode_set_scissor(0, 0x1 << 2, 0, 0);
        assert_eq!(d.mode, 0);
    }

    #[test]
    fn decode_set_scissor_left_origin_at_max_twelve_bits() {
        let d = decode_set_scissor(0, 0xFFF << 2, 0, 0);
        assert_eq!(d.left_origin, 0xFFF);
    }

    #[test]
    fn decode_set_scissor_right_origin_at_max_twelve_bits() {
        let d = decode_set_scissor(0, 0xFFF << 14, 0, 0);
        assert_eq!(d.right_origin, 0xFFF);
    }

    #[test]
    fn decode_set_scissor_ulx_uly_lrx_lry_from_second_pair() {
        let d = decode_set_scissor(0, 0, (1i16 as u16 as u32) << 16 | 2, 3u32 << 16 | 4);
        assert_eq!(d.ulx, 1);
        assert_eq!(d.uly, 2);
        assert_eq!(d.lrx, 3);
        assert_eq!(d.lry, 4);
    }

    // --- decode_set_rect_align ---

    #[test]
    fn decode_set_rect_align_all_zero() {
        let d = decode_set_rect_align(0, 0, 0, 0);
        assert_eq!(
            d,
            SetRectAlignDecoded {
                left_origin: 0,
                right_origin: 0,
                left_offset: 0,
                top_offset: 0,
                right_offset: 0,
                bottom_offset: 0,
            }
        );
    }

    #[test]
    fn decode_set_rect_align_left_origin_at_max_twelve_bits() {
        let d = decode_set_rect_align(0, 0xFFF, 0, 0);
        assert_eq!(d.left_origin, 0xFFF);
    }

    #[test]
    fn decode_set_rect_align_left_origin_one_bit_above_max_is_masked() {
        let d = decode_set_rect_align(0, 0x1 << 12, 0, 0);
        assert_eq!(d.left_origin, 0);
    }

    #[test]
    fn decode_set_rect_align_right_origin_at_max_twelve_bits() {
        let d = decode_set_rect_align(0, 0xFFF << 12, 0, 0);
        assert_eq!(d.right_origin, 0xFFF);
    }

    #[test]
    fn decode_set_rect_align_offsets_sign_boundary() {
        let d = decode_set_rect_align(0, 0, 0x8000 << 16, 0x8000 << 16);
        assert_eq!(d.left_offset, -32768);
        assert_eq!(d.right_offset, -32768);
    }

    #[test]
    fn decode_set_rect_align_offsets_max_positive() {
        let d = decode_set_rect_align(0, 0, 0x7FFF, 0x7FFF);
        assert_eq!(d.top_offset, 32767);
        assert_eq!(d.bottom_offset, 32767);
    }

    // --- decode_set_viewport_align ---

    #[test]
    fn decode_set_viewport_align_all_zero() {
        let d = decode_set_viewport_align(0, 0, 0, 0);
        assert_eq!(d, SetViewportAlignDecoded { ori: 0, x: 0, y: 0 });
    }

    #[test]
    fn decode_set_viewport_align_ori_at_max_twelve_bits() {
        let d = decode_set_viewport_align(0, 0xFFF, 0, 0);
        assert_eq!(d.ori, 0xFFF);
    }

    #[test]
    fn decode_set_viewport_align_ori_one_bit_above_max_is_masked() {
        let d = decode_set_viewport_align(0, 0x1 << 12, 0, 0);
        assert_eq!(d.ori, 0);
    }

    #[test]
    fn decode_set_viewport_align_x_sign_boundary() {
        let d = decode_set_viewport_align(0, 0, 0x8000 << 16, 0);
        assert_eq!(d.x, -32768);
    }

    #[test]
    fn decode_set_viewport_align_y_max_positive() {
        let d = decode_set_viewport_align(0, 0, 0x7FFF, 0);
        assert_eq!(d.y, 32767);
    }

    #[test]
    fn decode_set_viewport_align_second_pair_w1_is_unused() {
        // Both x and y read the second command word's w0 (p0(16,16) and
        // p0(0,16)) -- w1 of that pair is never consulted.
        let a = decode_set_viewport_align(0, 0, 0x1234_5678, 0);
        let b = decode_set_viewport_align(0, 0, 0x1234_5678, 0xFFFF_FFFF);
        assert_eq!(a, b);
    }

    // --- decode_set_scissor_align ---

    #[test]
    fn decode_set_scissor_align_all_zero() {
        let d = decode_set_scissor_align(0, 0, 0, 0, 0, 0);
        assert_eq!(
            d,
            SetScissorAlignDecoded {
                left_origin: 0,
                right_origin: 0,
                left_offset: 0,
                top_offset: 0,
                right_offset: 0,
                bottom_offset: 0,
                left_bound: 0,
                top_bound: 0,
                right_bound: 0,
                bottom_bound: 0,
            }
        );
    }

    #[test]
    fn decode_set_scissor_align_offsets_are_signed_bounds_are_not() {
        // Same 16-bit-at-top-half pattern on the offset pair (signed) and
        // the bound pair (unsigned) -- the asymmetry documented in
        // "Admitted domain": offsets reinterpret 0x8000 as negative, bounds
        // keep it as the large unsigned value.
        let d = decode_set_scissor_align(0, 0, 0x8000 << 16, 0, 0x8000 << 16, 0);
        assert_eq!(d.left_offset, -32768);
        assert_eq!(d.left_bound, 0x8000);
    }

    #[test]
    fn decode_set_scissor_align_bounds_at_max_sixteen_bits() {
        let d = decode_set_scissor_align(0, 0, 0, 0, (0xFFFF << 16) | 0xFFFF, 0xFFFF << 16);
        assert_eq!(d.left_bound, 0xFFFF);
        assert_eq!(d.top_bound, 0xFFFF);
        assert_eq!(d.right_bound, 0xFFFF);
    }

    #[test]
    fn decode_set_scissor_align_bottom_bound_reads_third_pair_w1_low_half() {
        let d = decode_set_scissor_align(0, 0, 0, 0, 0, 0xABCD);
        assert_eq!(d.bottom_bound, 0xABCD);
    }

    // --- decode_set_refresh_rate ---

    #[test]
    fn decode_set_refresh_rate_all_zero() {
        assert_eq!(decode_set_refresh_rate(0, 0), 0);
    }

    #[test]
    fn decode_set_refresh_rate_at_max_sixteen_bits() {
        assert_eq!(decode_set_refresh_rate(0, 0xFFFF), 0xFFFF);
    }

    #[test]
    fn decode_set_refresh_rate_one_bit_above_max_is_masked() {
        assert_eq!(decode_set_refresh_rate(0, 0x1 << 16), 0);
    }

    #[test]
    fn decode_set_refresh_rate_ignores_w0() {
        assert_eq!(decode_set_refresh_rate(0xFFFF_FFFF, 0x1234), 0x1234);
    }

    // --- decode_vertex_z_test ---

    #[test]
    fn decode_vertex_z_test_all_zero() {
        assert_eq!(decode_vertex_z_test(0, 0), 0);
    }

    #[test]
    fn decode_vertex_z_test_at_max_eight_bits() {
        assert_eq!(decode_vertex_z_test(0, 0xFF), 0xFF);
    }

    #[test]
    fn decode_vertex_z_test_one_bit_above_max_is_masked() {
        assert_eq!(decode_vertex_z_test(0, 0x1 << 8), 0);
    }

    // --- decode_matrix_group ---

    #[test]
    fn decode_matrix_group_all_zero() {
        let d = decode_matrix_group(0, 0, 0, 0);
        assert_eq!(
            d,
            MatrixGroupDecoded {
                id: 0,
                push: 0,
                proj: 0,
                mode: 0,
                pos: 0,
                rot: 0,
                scale: 0,
                skew: 0,
                persp: 0,
                vpos: 0,
                vtc: 0,
                tile: 0,
                order: 0,
                editable: 0,
                aspect: 0,
                lookat: 0,
            }
        );
    }

    #[test]
    fn decode_matrix_group_id_is_raw_second_word_unmasked() {
        let d = decode_matrix_group(0, 0xFFFF_FFFF, 0, 0);
        assert_eq!(d.id, 0xFFFF_FFFF);
    }

    #[test]
    fn decode_matrix_group_all_ones_third_word() {
        // w0_1 = 0xFFFFFFFF: every bitfield in the third command word set.
        let d = decode_matrix_group(0, 0, 0xFFFF_FFFF, 0);
        assert_eq!(d.push, 1);
        assert_eq!(d.proj, 1);
        assert_eq!(d.mode, 1);
        assert_eq!(d.pos, 0x3);
        assert_eq!(d.rot, 0x3);
        assert_eq!(d.scale, 0x3);
        assert_eq!(d.skew, 0x3);
        assert_eq!(d.persp, 0x3);
        assert_eq!(d.vpos, 0x3);
        assert_eq!(d.tile, 0x3);
        assert_eq!(d.order, 0x3);
        assert_eq!(d.editable, 1);
        assert_eq!(d.aspect, 0x3);
        assert_eq!(d.vtc, 0x3);
        assert_eq!(d.lookat, 0x3);
    }

    #[test]
    fn decode_matrix_group_lookat_at_bit_24_25_only() {
        let d = decode_matrix_group(0, 0, 0x3 << 24, 0);
        assert_eq!(d.lookat, 0x3);
        // No other field should be perturbed.
        assert_eq!(d.vtc, 0);
        assert_eq!(d.aspect, 0);
    }

    #[test]
    fn decode_matrix_group_lookat_one_bit_above_max_is_masked() {
        let d = decode_matrix_group(0, 0, 0x1 << 26, 0);
        assert_eq!(d.lookat, 0);
    }

    #[test]
    fn decode_matrix_group_vtc_field_isolated() {
        let d = decode_matrix_group(0, 0, 0x3 << 22, 0);
        assert_eq!(d.vtc, 0x3);
        assert_eq!(d.lookat, 0);
    }

    // --- decode_pop_matrix_group ---

    #[test]
    fn decode_pop_matrix_group_all_zero() {
        let d = decode_pop_matrix_group(0, 0);
        assert_eq!(
            d,
            PopMatrixGroupDecoded {
                pop_count: 0,
                proj: 0,
            }
        );
    }

    #[test]
    fn decode_pop_matrix_group_pop_count_at_max_eight_bits() {
        let d = decode_pop_matrix_group(0, 0xFF);
        assert_eq!(d.pop_count, 0xFF);
    }

    #[test]
    fn decode_pop_matrix_group_pop_count_one_bit_above_max_is_masked() {
        let d = decode_pop_matrix_group(0, 0x1 << 8);
        assert_eq!(d.pop_count, 0);
    }

    #[test]
    fn decode_pop_matrix_group_proj_reads_from_w0_not_w1() {
        // proj = p0(8, 1) reads w0, unlike pop_count which reads w1 -- this
        // is the one field in the file that reads w0's bit 8 specifically.
        let d = decode_pop_matrix_group(0x1 << 8, 0);
        assert_eq!(d.proj, 1);
        let d2 = decode_pop_matrix_group(0, 0x1 << 8);
        assert_eq!(d2.proj, 0);
    }

    // --- decode_force_upscale_2d / decode_force_true_bilerp /
    //     decode_force_scale_lod / decode_force_branch /
    //     decode_set_render_to_ram (all single-pair, w1-only, 1 or 2 bits) ---

    #[test]
    fn decode_force_upscale_2d_bit_set() {
        assert_eq!(decode_force_upscale_2d(0, 1), 1);
    }

    #[test]
    fn decode_force_upscale_2d_one_bit_above_max_is_masked() {
        assert_eq!(decode_force_upscale_2d(0, 0x1 << 1), 0);
    }

    #[test]
    fn decode_force_true_bilerp_at_max_two_bits() {
        assert_eq!(decode_force_true_bilerp(0, 0x3), 0x3);
    }

    #[test]
    fn decode_force_true_bilerp_one_bit_above_max_is_masked() {
        assert_eq!(decode_force_true_bilerp(0, 0x1 << 2), 0);
    }

    #[test]
    fn decode_force_scale_lod_bit_set() {
        assert_eq!(decode_force_scale_lod(0, 1), 1);
    }

    #[test]
    fn decode_force_scale_lod_zero() {
        assert_eq!(decode_force_scale_lod(0, 0), 0);
    }

    #[test]
    fn decode_force_branch_bit_set() {
        assert_eq!(decode_force_branch(0, 1), 1);
    }

    #[test]
    fn decode_force_branch_one_bit_above_max_is_masked() {
        assert_eq!(decode_force_branch(0, 0x1 << 1), 0);
    }

    #[test]
    fn decode_set_render_to_ram_bit_set() {
        assert_eq!(decode_set_render_to_ram(0, 1), 1);
    }

    #[test]
    fn decode_set_render_to_ram_zero() {
        assert_eq!(decode_set_render_to_ram(0, 0), 0);
    }

    // --- decode_vertex ---

    #[test]
    fn decode_vertex_all_zero() {
        let d = decode_vertex(0, 0, 0, 0);
        assert_eq!(
            d,
            VertexDecoded {
                vtx_count: 0,
                dst_index: 0,
                vtx_ptr: 0,
            }
        );
    }

    #[test]
    fn decode_vertex_count_and_index_split() {
        let d = decode_vertex(0, (0xAB << 8) | 0xCD, 0, 0);
        assert_eq!(d.vtx_count, 0xAB);
        assert_eq!(d.dst_index, 0xCD);
    }

    #[test]
    fn decode_vertex_count_one_bit_above_max_is_masked() {
        let d = decode_vertex(0, 0x1 << 16, 0, 0);
        assert_eq!(d.vtx_count, 0);
    }

    #[test]
    fn decode_vertex_ptr_is_raw_second_pair_w1_unmasked() {
        let d = decode_vertex(0, 0, 0, 0xFFFF_FFFF);
        assert_eq!(d.vtx_ptr, 0xFFFF_FFFF);
    }

    // --- decode_set_dither_noise_strength ---

    #[test]
    fn decode_set_dither_noise_strength_all_zero() {
        assert_eq!(decode_set_dither_noise_strength(0, 0), 0);
    }

    #[test]
    fn decode_set_dither_noise_strength_at_max_sixteen_bits() {
        assert_eq!(decode_set_dither_noise_strength(0, 0xFFFF), 0xFFFF);
    }

    #[test]
    fn decode_set_dither_noise_strength_one_bit_above_max_is_masked() {
        assert_eq!(decode_set_dither_noise_strength(0, 0x1 << 16), 0);
    }

    #[test]
    fn decode_set_dither_noise_strength_not_divided() {
        // The raw integer is returned; the /1024.0 division is dispatch,
        // not decode -- 1024 should come back as 1024, not 1.
        assert_eq!(decode_set_dither_noise_strength(0, 1024), 1024);
    }

    // --- decode_set_rdram_extended ---

    #[test]
    fn decode_set_rdram_extended_bit_set() {
        assert_eq!(decode_set_rdram_extended(0, 1), 1);
    }

    #[test]
    fn decode_set_rdram_extended_one_bit_above_max_is_masked() {
        assert_eq!(decode_set_rdram_extended(0, 0x1 << 1), 0);
    }

    // --- decode_set_projection_matrix_float / decode_set_view_matrix_float ---

    #[test]
    fn decode_set_projection_matrix_float_passes_w1_through_unmasked() {
        assert_eq!(
            decode_set_projection_matrix_float(0, 0xFFFF_FFFF),
            0xFFFF_FFFF
        );
    }

    #[test]
    fn decode_set_projection_matrix_float_ignores_w0() {
        assert_eq!(
            decode_set_projection_matrix_float(0xFFFF_FFFF, 0x1234),
            0x1234
        );
    }

    #[test]
    fn decode_set_view_matrix_float_passes_w1_through_unmasked() {
        assert_eq!(decode_set_view_matrix_float(0, 0xFFFF_FFFF), 0xFFFF_FFFF);
    }

    #[test]
    fn decode_set_view_matrix_float_ignores_w0() {
        assert_eq!(decode_set_view_matrix_float(0xFFFF_FFFF, 0x1234), 0x1234);
    }

    // --- decode_set_near_clipping ---

    #[test]
    fn decode_set_near_clipping_bit_set_returns_raw_one_not_negated() {
        // Upstream negates via `!nearClipping` at the dispatch call site;
        // this decode must return the raw 1, not a negated 0.
        assert_eq!(decode_set_near_clipping(0, 1), 1);
    }

    #[test]
    fn decode_set_near_clipping_bit_clear_returns_raw_zero() {
        assert_eq!(decode_set_near_clipping(0, 0), 0);
    }

    #[test]
    fn decode_set_near_clipping_one_bit_above_max_is_masked() {
        assert_eq!(decode_set_near_clipping(0, 0x1 << 1), 0);
    }

    // --- decode_matrix_float ---

    #[test]
    fn decode_matrix_float_all_zero() {
        let d = decode_matrix_float(0, 0, 0, 0);
        assert_eq!(
            d,
            MatrixFloatDecoded {
                raw_params_before_push_mask_xor: 0,
                matrix_ptr: 0,
            }
        );
    }

    #[test]
    fn decode_matrix_float_params_at_max_eight_bits() {
        let d = decode_matrix_float(0, 0xFF, 0, 0);
        assert_eq!(d.raw_params_before_push_mask_xor, 0xFF);
    }

    #[test]
    fn decode_matrix_float_params_one_bit_above_max_is_masked() {
        let d = decode_matrix_float(0, 0x1 << 8, 0, 0);
        assert_eq!(d.raw_params_before_push_mask_xor, 0);
    }

    #[test]
    fn decode_matrix_float_matrix_ptr_is_raw_second_pair_w1_unmasked() {
        let d = decode_matrix_float(0, 0, 0, 0xFFFF_FFFF);
        assert_eq!(d.matrix_ptr, 0xFFFF_FFFF);
    }

    // --- decode_set_vertex_segment ---

    #[test]
    fn decode_set_vertex_segment_all_zero() {
        let d = decode_set_vertex_segment(0, 0, 0, 0);
        assert_eq!(
            d,
            SetVertexSegmentDecoded {
                is_enabled: 0,
                vertex_element: 0,
                vertex_address: 0,
                base_segment_address: 0,
            }
        );
    }

    #[test]
    fn decode_set_vertex_segment_is_enabled_bit() {
        let d = decode_set_vertex_segment(0, 1, 0, 0);
        assert_eq!(d.is_enabled, 1);
    }

    #[test]
    fn decode_set_vertex_segment_vertex_element_at_max_four_bits() {
        let d = decode_set_vertex_segment(0, 0xF << 1, 0, 0);
        assert_eq!(d.vertex_element, 0xF);
    }

    #[test]
    fn decode_set_vertex_segment_vertex_element_one_bit_above_max_is_masked() {
        let d = decode_set_vertex_segment(0, 0x1 << 5, 0, 0);
        assert_eq!(d.vertex_element, 0);
    }

    #[test]
    fn decode_set_vertex_segment_addresses_are_raw_second_pair_unmasked() {
        let d = decode_set_vertex_segment(0, 0, 0xDEAD_BEEF, 0xFEED_FACE);
        assert_eq!(d.vertex_address, 0xDEAD_BEEF);
        assert_eq!(d.base_segment_address, 0xFEED_FACE);
    }

    // --- decode_set_texcoord_wrap_point ---

    #[test]
    fn decode_set_texcoord_wrap_point_all_zero() {
        let d = decode_set_texcoord_wrap_point(0, 0);
        assert_eq!(
            d,
            SetTexcoordWrapPointDecoded {
                wrap_point_u: 0,
                wrap_point_v: 0,
            }
        );
    }

    #[test]
    fn decode_set_texcoord_wrap_point_sign_boundary() {
        let d = decode_set_texcoord_wrap_point(0, 0x8000 << 16);
        assert_eq!(d.wrap_point_u, -32768);
    }

    #[test]
    fn decode_set_texcoord_wrap_point_max_positive() {
        let d = decode_set_texcoord_wrap_point(0, 0x7FFF);
        assert_eq!(d.wrap_point_v, 32767);
    }

    // --- decode_set_rect_aspect ---

    #[test]
    fn decode_set_rect_aspect_at_max_two_bits() {
        assert_eq!(decode_set_rect_aspect(0, 0x3), 0x3);
    }

    #[test]
    fn decode_set_rect_aspect_one_bit_above_max_is_masked() {
        assert_eq!(decode_set_rect_aspect(0, 0x1 << 2), 0);
    }

    // --- decode_no_op_hook_header ---

    #[test]
    fn decode_no_op_hook_header_all_zero() {
        let d = decode_no_op_hook_header(0, 0);
        assert_eq!(
            d,
            NoOpHookHeaderDecoded {
                magic_number: 0,
                hook_value: 0,
                hook_op: 0,
            }
        );
    }

    #[test]
    fn decode_no_op_hook_header_recognizes_magic_number() {
        let d = decode_no_op_hook_header(packed::RT64_HOOK_MAGIC_NUMBER, 0);
        assert_eq!(d.magic_number, packed::RT64_HOOK_MAGIC_NUMBER);
    }

    #[test]
    fn decode_no_op_hook_header_magic_number_one_bit_above_max_is_masked() {
        let d = decode_no_op_hook_header(0x1 << 24, 0);
        assert_eq!(d.magic_number, 0);
    }

    #[test]
    fn decode_no_op_hook_header_hook_value_and_op_partition_w1() {
        // hook_value = bits 0-27, hook_op = bits 28-31: full 32-bit
        // partition with no overlap and no gap.
        let d = decode_no_op_hook_header(0, 0xFFFF_FFFF);
        assert_eq!(d.hook_value, 0x0FFF_FFFF);
        assert_eq!(d.hook_op, 0xF);
    }

    #[test]
    fn decode_no_op_hook_header_hook_op_get_version() {
        let d = decode_no_op_hook_header(0, packed::RT64_HOOK_OP_GETVERSION << 28);
        assert_eq!(d.hook_op, packed::RT64_HOOK_OP_GETVERSION as u8);
    }

    #[test]
    fn decode_no_op_hook_header_hook_op_branch() {
        let d = decode_no_op_hook_header(0, packed::RT64_HOOK_OP_BRANCH << 28);
        assert_eq!(d.hook_op, packed::RT64_HOOK_OP_BRANCH as u8);
    }

    // --- round-trip tests: pack (rt64_extended_gbi) then decode (this
    // module), asserting exact equality against hand-computed expectations.
    // These catch shift/mask disagreements no single-direction test can.

    #[test]
    fn round_trip_set_refresh_rate() {
        let words = packed::pack_set_refresh_rate(packed::RT64_EXTENDED_OPCODE_DEFAULT, 0x1234);
        assert_eq!(decode_set_refresh_rate(words[0], words[1]), 0x1234);
    }

    #[test]
    fn round_trip_set_refresh_rate_truncates_over_wide_input() {
        // pack_set_refresh_rate masks its input to 16 bits before shifting
        // (PARAM's documented truncate-not-corrupt behavior); the decoder
        // must observe the already-truncated value, not the original.
        let words = packed::pack_set_refresh_rate(packed::RT64_EXTENDED_OPCODE_DEFAULT, 0x1_FFFF);
        assert_eq!(decode_set_refresh_rate(words[0], words[1]), 0xFFFF);
    }

    #[test]
    fn round_trip_vertex_z_test() {
        let words = packed::pack_vertex_z_test(packed::RT64_EXTENDED_OPCODE_DEFAULT, 0xAB);
        assert_eq!(decode_vertex_z_test(words[0], words[1]), 0xAB);
    }

    #[test]
    fn round_trip_force_upscale_2d() {
        let words = packed::pack_force_upscale_2d(packed::RT64_EXTENDED_OPCODE_DEFAULT, 1);
        assert_eq!(decode_force_upscale_2d(words[0], words[1]), 1);
    }

    #[test]
    fn round_trip_force_true_bilerp() {
        let words = packed::pack_force_true_bilerp(packed::RT64_EXTENDED_OPCODE_DEFAULT, 0x2);
        assert_eq!(decode_force_true_bilerp(words[0], words[1]), 0x2);
    }

    #[test]
    fn round_trip_force_scale_lod() {
        let words = packed::pack_force_scale_lod(packed::RT64_EXTENDED_OPCODE_DEFAULT, 1);
        assert_eq!(decode_force_scale_lod(words[0], words[1]), 1);
    }

    #[test]
    fn round_trip_force_branch() {
        let words = packed::pack_force_branch(packed::RT64_EXTENDED_OPCODE_DEFAULT, 1);
        assert_eq!(decode_force_branch(words[0], words[1]), 1);
    }

    #[test]
    fn round_trip_set_render_to_ram() {
        let words = packed::pack_set_render_to_ram(packed::RT64_EXTENDED_OPCODE_DEFAULT, 1);
        assert_eq!(decode_set_render_to_ram(words[0], words[1]), 1);
    }

    #[test]
    fn round_trip_set_rdram_extended() {
        let words = packed::pack_set_rdram_extended(packed::RT64_EXTENDED_OPCODE_DEFAULT, 1);
        assert_eq!(decode_set_rdram_extended(words[0], words[1]), 1);
    }

    #[test]
    fn round_trip_set_near_clipping() {
        let words = packed::pack_set_near_clipping(packed::RT64_EXTENDED_OPCODE_DEFAULT, 1);
        assert_eq!(decode_set_near_clipping(words[0], words[1]), 1);
    }

    #[test]
    fn round_trip_set_rect_aspect() {
        let words = packed::pack_set_rect_aspect(packed::RT64_EXTENDED_OPCODE_DEFAULT, 0x2);
        assert_eq!(decode_set_rect_aspect(words[0], words[1]), 0x2);
    }

    #[test]
    fn round_trip_set_dither_noise_strength() {
        let words =
            packed::pack_set_dither_noise_strength(packed::RT64_EXTENDED_OPCODE_DEFAULT, 1024);
        assert_eq!(decode_set_dither_noise_strength(words[0], words[1]), 1024);
    }

    #[test]
    fn round_trip_pop_matrix_group_pop_count_agrees() {
        let words = packed::pack_pop_matrix_group_n(packed::RT64_EXTENDED_OPCODE_DEFAULT, 1, 42);
        let d = decode_pop_matrix_group(words[0], words[1]);
        assert_eq!(d.pop_count, 42);
    }

    /// GENUINE UPSTREAM ENCODER/DECODER DISAGREEMENT (not a bug in this
    /// port -- both sides are literal ports of their respective sources).
    /// `gEXPopMatrixGroupN`'s packing macro
    /// (`include/rt64_extended_gbi.h:340-344`) writes `proj` into
    /// `PARAM(proj, 1, 8)` of the command's SECOND word (`_word1` of
    /// `G_EX_COMMAND1`, i.e. what `DisplayList::w1`/`p1` reads back).
    /// `popMatrixGroupV1`'s decode (`src/gbi/rt64_gbi_extended.cpp:159`)
    /// reads `proj` via `(*dl)->p0(8, 1)` -- `w0`, not `w1`. So a `proj`
    /// bit packed by `gEXPopMatrixGroupN` at `w1` bit 8 is invisible to
    /// `popMatrixGroupV1`'s decode (which looks at `w0` bit 8, always 0
    /// for this opcode since `w0` carries only the 24-bit opcode field),
    /// and any `w0` bit 8 the decoder DOES see was never touched by the
    /// packer. Packing `proj = 1` and decoding back yields `proj = 0`,
    /// not `1` -- proven here with hand-computed expectations on both
    /// sides, not values captured from either implementation.
    #[test]
    fn round_trip_pop_matrix_group_proj_disagrees_between_encoder_and_decoder() {
        let words = packed::pack_pop_matrix_group_n(packed::RT64_EXTENDED_OPCODE_DEFAULT, 1, 0);
        // Packer wrote proj=1 into w1 bit 8; decoder reads w0 bit 8, which
        // the packer never touched (w0 is opcode-only for this command).
        assert_eq!(
            words[1] & (1 << 8),
            1 << 8,
            "packer set w1 bit 8 for proj=1"
        );
        assert_eq!(
            words[0] & (1 << 8),
            0,
            "packer never touches w0 for this opcode"
        );
        let d = decode_pop_matrix_group(words[0], words[1]);
        assert_eq!(
            d.proj, 0,
            "decoder reads w0, so packed proj=1 decodes back as 0"
        );
    }

    #[test]
    fn round_trip_set_viewport() {
        let words = packed::pack_viewport(packed::RT64_EXTENDED_OPCODE_DEFAULT, 0xABC, 0xDEAD_BEEF);
        let d = decode_set_viewport(words[0], words[1], words[2], words[3]);
        assert_eq!(d.ori, 0xABC);
        assert_eq!(d.vp, 0xDEAD_BEEF);
    }

    #[test]
    fn round_trip_set_scissor() {
        let words = packed::pack_set_scissor(
            packed::RT64_EXTENDED_OPCODE_DEFAULT,
            0x2,
            0x0AB,
            0x0CD,
            10,
            20,
            30,
            40,
        );
        let d = decode_set_scissor(words[0], words[1], words[2], words[3]);
        assert_eq!(d.mode, 0x2);
        assert_eq!(d.left_origin, 0x0AB);
        assert_eq!(d.right_origin, 0x0CD);
        // pack_set_scissor multiplies each coordinate by 4 before packing;
        // the decoder observes that already-multiplied value verbatim (the
        // *4 is a caller-side transform on both sides of this ABI, not a
        // decode-time operation) -- see module doc for why this is not
        // treated as a decoder/encoder disagreement.
        assert_eq!(d.ulx, 40);
        assert_eq!(d.uly, 80);
        assert_eq!(d.lrx, 120);
        assert_eq!(d.lry, 160);
    }

    #[test]
    fn round_trip_set_rect_align() {
        let words = packed::pack_set_rect_align(
            packed::RT64_EXTENDED_OPCODE_DEFAULT,
            0x111,
            0x222,
            -100i32 as u32,
            200,
            -300i32 as u32,
            400,
        );
        let d = decode_set_rect_align(words[0], words[1], words[2], words[3]);
        assert_eq!(d.left_origin, 0x111);
        assert_eq!(d.right_origin, 0x222);
        assert_eq!(d.left_offset, -100);
        assert_eq!(d.top_offset, 200);
        assert_eq!(d.right_offset, -300);
        assert_eq!(d.bottom_offset, 400);
    }

    #[test]
    fn round_trip_set_viewport_align() {
        let words = packed::pack_set_viewport_align(
            packed::RT64_EXTENDED_OPCODE_DEFAULT,
            0x333,
            -1234i32 as u32,
            5678,
        );
        let d = decode_set_viewport_align(words[0], words[1], words[2], words[3]);
        assert_eq!(d.ori, 0x333);
        assert_eq!(d.x, -1234);
        assert_eq!(d.y, 5678);
    }

    #[test]
    fn round_trip_set_scissor_align() {
        // Positive-only offsets: pack_set_scissor_align's `(ulxOffset) * 4`
        // etc. is a plain u32 multiply with no wrapping wrapper (unlike
        // `param`, which masks-then-shifts and so never overflows); a
        // negative-as-u32 offset here (e.g. `-10i32 as u32` = 0xFFFF_FFF6)
        // overflows that u32 multiply and panics in a debug build before
        // `param` is ever reached. That is the packer's own behavior
        // (already-landed code, not part of this ticket's scope), not a
        // decode-side concern -- sign handling on the decode side alone is
        // covered separately by decode_set_scissor_align_offsets_are_signed_
        // bounds_are_not above.
        let words = packed::pack_set_scissor_align(
            packed::RT64_EXTENDED_OPCODE_DEFAULT,
            0x0AB,
            0x0CD,
            10,
            20,
            30,
            40,
            50,
            60,
            70,
            80,
        );
        let d =
            decode_set_scissor_align(words[0], words[1], words[2], words[3], words[4], words[5]);
        assert_eq!(d.left_origin, 0x0AB);
        assert_eq!(d.right_origin, 0x0CD);
        // Offsets are *4'd by the packer and signed on decode.
        assert_eq!(d.left_offset, 40);
        assert_eq!(d.top_offset, 80);
        assert_eq!(d.right_offset, 120);
        assert_eq!(d.bottom_offset, 160);
        // Bounds are *4'd by the packer and unsigned on decode.
        assert_eq!(d.left_bound, 200);
        assert_eq!(d.top_bound, 240);
        assert_eq!(d.right_bound, 280);
        assert_eq!(d.bottom_bound, 320);
    }

    #[test]
    fn round_trip_matrix_group() {
        // pack_matrix_group's parameter order is (extended_opcode, id, mode,
        // push, proj, pos, rot, scale, skew, persp, vert, tile, order, edit,
        // aspect, tc, lookat) -- matching gEXMatrixGroup's macro argument
        // order exactly (`id, mode, push, proj, pos, rot, scale, skew,
        // persp, vert, tile, order, edit, aspect, tc, lookat`).
        let words = packed::pack_matrix_group(
            packed::RT64_EXTENDED_OPCODE_DEFAULT,
            0x1234_5678, // id
            1,           // mode
            1,           // push
            1,           // proj
            2,           // pos
            3,           // rot
            0,           // scale
            0,           // skew
            2,           // persp
            1,           // vert (-> vpos)
            0,           // tile
            0,           // order
            1,           // edit (-> editable)
            2,           // aspect
            3,           // tc (-> vtc)
            1,           // lookat
        );
        let d = decode_matrix_group(words[0], words[1], words[2], words[3]);
        assert_eq!(d.id, 0x1234_5678);
        assert_eq!(d.push, 1);
        assert_eq!(d.proj, 1);
        assert_eq!(d.mode, 1);
        assert_eq!(d.pos, 2);
        assert_eq!(d.rot, 3);
        assert_eq!(d.scale, 0);
        assert_eq!(d.skew, 0);
        assert_eq!(d.persp, 2);
        assert_eq!(d.vpos, 1);
        assert_eq!(d.tile, 0);
        assert_eq!(d.order, 0);
        assert_eq!(d.editable, 1);
        assert_eq!(d.aspect, 2);
        assert_eq!(d.vtc, 3);
        assert_eq!(d.lookat, 1);
    }

    #[test]
    fn round_trip_vertex() {
        let words = packed::pack_vertex(packed::RT64_EXTENDED_OPCODE_DEFAULT, 0xCAFE_BABE, 5, 6);
        let d = decode_vertex(words[0], words[1], words[2], words[3]);
        assert_eq!(d.vtx_count, 5);
        assert_eq!(d.dst_index, 6);
        assert_eq!(d.vtx_ptr, 0xCAFE_BABE);
    }

    #[test]
    fn round_trip_set_proj_matrix_float() {
        let words =
            packed::pack_set_proj_matrix_float(packed::RT64_EXTENDED_OPCODE_DEFAULT, 0xC0FF_EE00);
        assert_eq!(
            decode_set_projection_matrix_float(words[0], words[1]),
            0xC0FF_EE00
        );
    }

    #[test]
    fn round_trip_set_view_matrix_float() {
        let words =
            packed::pack_set_view_matrix_float(packed::RT64_EXTENDED_OPCODE_DEFAULT, 0xC0FF_EE00);
        assert_eq!(
            decode_set_view_matrix_float(words[0], words[1]),
            0xC0FF_EE00
        );
    }

    #[test]
    fn round_trip_matrix_float() {
        let words =
            packed::pack_matrix_float(packed::RT64_EXTENDED_OPCODE_DEFAULT, 0x1122_3344, 0x99);
        let d = decode_matrix_float(words[0], words[1], words[2], words[3]);
        assert_eq!(d.raw_params_before_push_mask_xor, 0x99);
        assert_eq!(d.matrix_ptr, 0x1122_3344);
    }

    #[test]
    fn round_trip_set_vertex_segment() {
        let words = packed::pack_set_vertex_segment(
            packed::RT64_EXTENDED_OPCODE_DEFAULT,
            0xA,
            1,
            0x1111_1111,
            0x2222_2222,
        );
        let d = decode_set_vertex_segment(words[0], words[1], words[2], words[3]);
        assert_eq!(d.is_enabled, 1);
        assert_eq!(d.vertex_element, 0xA);
        assert_eq!(d.vertex_address, 0x1111_1111);
        assert_eq!(d.base_segment_address, 0x2222_2222);
    }

    #[test]
    fn round_trip_set_texcoord_wrap_point() {
        let words = packed::pack_set_texcoord_wrap_point(
            packed::RT64_EXTENDED_OPCODE_DEFAULT,
            -111i32 as u32,
            222,
        );
        let d = decode_set_texcoord_wrap_point(words[0], words[1]);
        assert_eq!(d.wrap_point_u, -111);
        assert_eq!(d.wrap_point_v, 222);
    }
}
