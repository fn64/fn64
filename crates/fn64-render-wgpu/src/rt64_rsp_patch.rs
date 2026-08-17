//! Literal port of RT64's fixed-point matrix patch region/index decode
//! (`RSP::insertMatrix`) and the `G_MODIFYVTX` vertex-attribute patch field
//! decode (`RSP::modifyVertex`'s attribute `switch`), a literal port of the
//! permitted MIT RT64 Rust-port source pinned at commit
//! `5473732a822a4423b5696e7cb18fecc425a59875`
//! (`docs/RT64-PORT-AUTHORITY.md`), `src/hle/rt64_rsp.cpp`/`.h` (SHA-256 of
//! the whole files,
//! `7dfdf40254d44d92c247d9c876bb8ca55995927ad534981bd48868bb44f1f695` /
//! `832c092bf7021ec08a46de85c95d9973b69fa7c560ca96e43215c2fb18f54d95`):
//!
//! ```text
//! // src/hle/rt64_rsp.h:27
//! #define RSP_MAX_VERTICES            256
//!
//! // src/hle/rt64_rsp.h:247, 263 (declarations)
//! void insertMatrix(uint32_t address, uint32_t value);
//! void modifyVertex(uint16_t dstIndex, uint16_t dstAttribute, uint32_t value);
//!
//! // src/hle/rt64_rsp.cpp:240-302
//! void RSP::insertMatrix(uint32_t address, uint32_t value) {
//!     // We assume unaligned addresses or overlapping is impossible until we have verification
//!     // from the microcode itself if this is allowed or not due to how much more complex
//!     // the implementation could get.
//!     assert(((address & 0x3) == 0) && "Unaligned addresses for insert matrix are not currently supported.");
//!
//!     // Copies a 32-bit value into the location occupied by the fixed point matrices.
//!     const uint16_t MatrixSize = 0x40;
//!     const uint16_t FractionalMatrixAddress = MatrixSize / 2;
//!     const uint16_t ModelAddress = 0x0;
//!     const uint16_t ViewProjAddress = ModelAddress + MatrixSize;
//!     const uint16_t ModelViewProjAddress = ViewProjAddress + MatrixSize;
//!
//!     // According to the microcode, the address requires this kind of wrapping
//!     // to access the real destination.
//!     uint32_t dstAddr = (address + ModelViewProjAddress) & 0xFFFFU;
//!
//!     // Figure out which matrix should be modified and compute a relative address to it.
//!     uint32_t relAddr = 0;
//!     hlslpp::float4x4 *dstMat = nullptr;
//!     hlslpp::float4x4 &viewProjMatrix = viewProjMatrixStack[projectionMatrixStackSize - 1];
//!     if (dstAddr >= (ModelViewProjAddress + MatrixSize)) {
//!         assert(false && "Undefined behavior due to destination address extending outside of the allowed bounds.");
//!         return;
//!     }
//!     if (dstAddr >= ModelViewProjAddress) {
//!         dstMat = &modelViewProjMatrix;
//!         relAddr = dstAddr - ModelViewProjAddress;
//!         modelViewProjInserted = true;
//!     }
//!     else if (dstAddr >= ViewProjAddress) {
//!         dstMat = &viewProjMatrix;
//!         relAddr = dstAddr - ViewProjAddress;
//!         projectionMatrixChanged = true;
//!         projectionMatrixInversed = false;
//!     }
//!     else if (dstAddr >= ModelAddress) {
//!         dstMat = &modelMatrixStack[modelMatrixStackSize - 1];
//!         relAddr = dstAddr - ModelAddress;
//!     }
//!
//!     // Modify two fractional parts or two integer parts.
//!     const bool modifyFractional = relAddr >= FractionalMatrixAddress;
//!     if (modifyFractional) {
//!         relAddr -= FractionalMatrixAddress;
//!     }
//!
//!     const uint32_t index = relAddr / 2;
//!     const uint32_t row = index / 4;
//!     const uint32_t column = index % 4;
//!     if (modifyFractional) {
//!         FixedMatrix::modifyMatrix4x4Fraction(*dstMat, row, column, uint16_t((value >> 16U) & 0xFFFFU));
//!         FixedMatrix::modifyMatrix4x4Fraction(*dstMat, row, column + 1, uint16_t(value & 0xFFFFU));
//!     }
//!     else {
//!         FixedMatrix::modifyMatrix4x4Integer(*dstMat, row, column, int16_t((value >> 16) & 0xFFFF));
//!         FixedMatrix::modifyMatrix4x4Integer(*dstMat, row, column + 1, int16_t(value & 0xFFFF));
//!     }
//! }
//!
//! // src/hle/rt64_rsp.cpp:737-835 (attribute switch only -- see "Nonclaims")
//! void RSP::modifyVertex(uint16_t dstIndex, uint16_t dstAttribute, uint32_t value) {
//!     if (dstIndex >= RSP_MAX_VERTICES) {
//!         assert(false && "Vertex index is not valid. DL is possibly corrupted.");
//!         return;
//!     }
//!
//!     // ... [15-vector clone of the vertex-in-use case, out of scope] ...
//!
//!     // Modify the attributes.
//!     switch (dstAttribute) {
//!     case G_MWO_POINT_RGBA: {
//!         normColBytes[globalIndex * 4 + 0] = (value >> 24) & 0xFF;
//!         normColBytes[globalIndex * 4 + 1] = (value >> 16) & 0xFF;
//!         normColBytes[globalIndex * 4 + 2] = (value >> 8) & 0xFF;
//!         normColBytes[globalIndex * 4 + 3] = value & 0xFF;
//!         fogIndices[globalIndex] = 0;
//!         lightIndices[globalIndex] = 0;
//!         lightCounts[globalIndex] = 0;
//!         break;
//!     }
//!     case G_MWO_POINT_ST: {
//!         const float s = int16_t((value >> 16) & 0xFFFF) / 32.0f;
//!         const float t = int16_t(value & 0xFFFF) / 32.0f;
//!         tcFloats[globalIndex * 2 + 0] = s;
//!         tcFloats[globalIndex * 2 + 1] = t;
//!         lookAtIndices[globalIndex] = 0;
//!         break;
//!     }
//!     case G_MWO_POINT_XYSCREEN: {
//!         // First bit being 0 indicates it should modify only XY.
//!         modifyPosUints.emplace_back(globalIndex << 1);
//!         modifyPosUints.emplace_back(value);
//!
//!         // We decode on the CPU anyway for draw area tracking and the debugger.
//!         const uint16_t extX = (value >> 16) & 0xFFFF;
//!         const uint16_t extY = value & 0xFFFF;
//!         posScreen[globalIndex][0] = int16_t(extX) / 4.0f;
//!         posScreen[globalIndex][1] = int16_t(extY) / 4.0f;
//!         break;
//!     }
//!     case G_MWO_POINT_ZSCREEN: {
//!         // First bit being 1 indicates it should modify only Z.
//!         modifyPosUints.emplace_back((globalIndex << 1) | 0x1);
//!         modifyPosUints.emplace_back(value);
//!
//!         // We decode on the CPU anyway for depth tracking, branchZ and the debugger.
//!         posScreen[globalIndex][2] = value / 65536.0f;
//!         break;
//!     }
//!     default:
//!         assert(false && "Unsupported modify vertex");
//!         break;
//!     }
//! }
//!
//! // src/hle/rt64_rsp.h:27
//! #define RSP_MAX_VERTICES            256
//!
//! // src/shared/rt64_f3d_defines.h:80-83
//! #define G_MWO_POINT_RGBA 0x10
//! #define G_MWO_POINT_ST 0x14
//! #define G_MWO_POINT_XYSCREEN 0x18
//! #define G_MWO_POINT_ZSCREEN 0x1C
//! ```
//!
//! **Reuse, not new type.** This module reuses `rt64_common.rs`'s
//! [`crate::rt64_common::FixedMatrix::modify_matrix4x4_integer`] and
//! [`crate::rt64_common::FixedMatrix::modify_matrix4x4_fraction`] directly
//! for the two `dstMat` write calls at the bottom of `insertMatrix` --
//! both are already `pub`, already take the `(matrix: &mut Mat4, i, j,
//! value)` shape `insertMatrix` calls them with (`i` = `row`, `j` =
//! `column`/`column + 1`), and already carry the `j ^ 1` column-swap
//! semantics inside [`crate::rt64_common::FixedMatrix::to_float`] that
//! `insertMatrix` depends on for its writes to read back correctly later.
//! This module does not reimplement either helper, does not reimplement
//! [`crate::rt64_common::FixedMatrix::fixed_to_float`], and does not
//! introduce a new fixed-point-matrix type -- [`fn64_render_ir::Mat4`] is
//! the same "already a general matrix type; the *fixed-point encoding* is
//! the domain-specific fact this port owns" reuse this crate's other
//! modules already establish (see `rt64_common.rs`'s own "Reuse, not new
//! type" section for `Mat4`/`Vec4`). What this module *adds* locally is
//! only the parts `rt64_common.rs` does not have and this ticket's scope
//! statement calls out by name: `insertMatrix`'s region/index decode
//! (which of the three logical matrices `address` selects, the integer/
//! fraction half split, and the `row`/`column` computation feeding into
//! the reused helpers) and `modifyVertex`'s `G_MODIFYVTX` attribute-field
//! decode. Both are pure bitfield/arithmetic decode with no `FixedMatrix`
//! counterpart to reuse.
//!
//! ## Admitted domain
//!
//! **`insertMatrix` region decode:**
//!
//! - **Region selection and its base address.** `dstAddr = (address +
//!   0x80) & 0xFFFF` first folds the caller's raw 16-bit-domain `address`
//!   into a 0..0xFFFF window anchored so that offset `0` corresponds to
//!   the *Model* matrix's first byte, not `address`'s own zero. The three
//!   regions are then checked **in address-descending order** --
//!   ModelViewProj (`>= 0x80`) first, then ViewProj (`>= 0x40`), then
//!   Model (`>= 0x0`, i.e. the fallback: any remaining `dstAddr` in
//!   `0x00..0x40`) -- matching the C++ `if`/`else if`/`else if` chain
//!   exactly, including which branch wins when ranges are adjacent (they
//!   are disjoint and contiguous here, `[0,0x40)`, `[0x40,0x80)`,
//!   `[0x80,0xC0)`, so descending-order evaluation cannot actually change
//!   which region a given `dstAddr` lands in -- this port preserves the
//!   evaluation order anyway as a literal-port fidelity choice, not
//!   because it is observably load-bearing). `relAddr` is then `dstAddr`
//!   minus that region's base (`0x80`, `0x40`, or `0x0` respectively), so
//!   `relAddr` is always in `[0, 0x40)` for any region-matching `dstAddr`.
//! - **Out-of-range region index: a no-op, not a clamp or a wrap.**
//!   `dstAddr >= 0xC0` (`ModelViewProjAddress + MatrixSize`) is checked
//!   *before* any region branch and is the only reachable "falls through
//!   all three regions" case (the `else if (dstAddr >= ModelAddress)`
//!   arm, `ModelAddress == 0`, otherwise always matches since `dstAddr`
//!   is unsigned). Upstream's `assert(false); return;` performs no matrix
//!   write at all in this case (a release build compiles the assert out
//!   and `return`s unconditionally either way). This port's
//!   [`decode_insert_matrix`] returns `None` for `dstAddr in
//!   0xC0..=0xFFFF`, following the same "pure decode returns `None`
//!   instead of panicking on an out-of-domain input; the C++ assert is
//!   dispatch-layer debug tooling, not a bitfield-decode fact" precedent
//!   [`rt64_gbi_f3d.rs`]'s `decode_move_mem` already set (cited directly
//!   in this ticket's brief) and [`rt64_rsp_segment.rs`]'s "DECODE-only"
//!   framing follows too. There is no clamp and no address wraparound in
//!   this path at all -- `dstAddr` itself already wrapped once (the `&
//!   0xFFFF`), but the *region* index is a hard reject, never clamped
//!   into range and never wrapped back into `0..0xC0`.
//! - **Integer vs. fraction half, and which address sub-range picks
//!   which.** `FractionalMatrixAddress = MatrixSize / 2 = 0x20`. Each
//!   64-byte (`0x40`) matrix region's low half, `relAddr in [0, 0x20)`,
//!   is the *integer* plane; its high half, `relAddr in [0x20, 0x40)`,
//!   is the *fraction* plane -- `modifyFractional = relAddr >= 0x20`,
//!   and when true, `relAddr -= 0x20` before the index computation, so
//!   `index`/`row`/`column` are computed identically for both halves
//!   (the half only decides which of `modify_matrix4x4_integer` /
//!   `modify_matrix4x4_fraction` the caller should invoke, and whether
//!   the two 16-bit halves of `value` are read as `i16`/signed
//!   (integer half) or `u16`/unsigned-bit-pattern (fraction half) --
//!   both are just bit reinterpretations, no numeric conversion, exactly
//!   matching `modify_matrix4x4_integer`'s `i16` and
//!   `modify_matrix4x4_fraction`'s `u16` parameter types in
//!   `rt64_common.rs`).
//! - **Index / row / column computation.** `index = relAddr / 2` (each
//!   matrix cell's fixed-point half is a 2-byte/16-bit halfword, so
//!   dividing the byte offset by 2 gives a halfword index in `0..16`);
//!   `row = index / 4`, `column = index % 4` (row-major 4x4 layout,
//!   4 halfwords per row). This port represents the computation as
//!   plain unsigned integer division/modulo on `u32`, identical to the
//!   C++ `uint32_t` arithmetic -- no rounding or truncation divergence
//!   is possible since both are the same floor-division semantics for
//!   non-negative operands.
//! - **The `column + 1` write, and why it can walk off a 4x4 matrix.**
//!   Both halves write *two* halfwords per `insertMatrix` call --
//!   `column` and `column + 1` -- from `value`'s high and low 16 bits
//!   respectively, since one 32-bit DMA word patches two adjacent
//!   fixed-point halfwords at once. `insertMatrix`'s only alignment
//!   guard is `assert(((address & 0x3) == 0))` -- 4-byte alignment on
//!   the *raw* `address` parameter, a debug-only precondition (compiled
//!   out under `NDEBUG`, and RT64 ships release builds -- the same
//!   "debug-only `assert()` precondition becomes `debug_assert!`, not an
//!   enforced invariant" situation `rt64_rsp_segment.rs`'s `set_segment`
//!   already documents for this same source file). Because `0x80` (the
//!   `dstAddr` offset) and every region base (`0x0`/`0x40`/`0x80`) and
//!   `0x20` (the fractional split) are all multiples of 4, a 4-byte-
//!   aligned `address` keeps `relAddr` a multiple of 4 all the way
//!   through, so `index = relAddr / 2` is always even and `column =
//!   index % 4` is always `0` or `2` -- `column + 1` is then always `1`
//!   or `3`, safely in a 4x4 matrix's `0..4` column range. **If the
//!   alignment precondition is violated** (reachable only in a release
//!   build with the debug assert compiled out, or if this port's
//!   `debug_assert!` equivalent is bypassed), `index` can be odd and
//!   `column` can be `1` or `3`, making `column + 1` equal `2` or `4` --
//!   `4` is one past a 4x4 matrix's last column. Upstream's C++ passes
//!   this to `hlslpp::float4x4::operator[]`, whose out-of-range behavior
//!   this port does not have visibility into (unverified upstream UB,
//!   not characterized here). [`crate::rt64_common`]'s `get_elem`/
//!   `set_elem` helpers underlying `modify_matrix4x4_integer`/`fraction`
//!   `panic!` on any `j` outside `0..4` -- so on this port, an unaligned
//!   `address` reaching the `column == 3` case is a **loud panic**, not
//!   silent corruption, a divergence from unspecified upstream UB in the
//!   same direction (and for the same "no UB-preserving cast/write
//!   exists in Rust" reason) `rt64_rsp_segment.rs`'s `set_segment`
//!   admits for its own out-of-range array write. [`decode_insert_matrix`]
//!   itself never calls `modify_matrix4x4_integer`/`fraction` (that is
//!   the caller's job per "Nonclaims" below), so the decode function
//!   itself cannot panic on this path -- it always returns `column`
//!   values `0..=3` from its `u32 % 4` computation by construction; the
//!   panic risk described here is a fact about the *caller* squaring a
//!   decoded `(row, column)` against `modify_matrix4x4_integer`/
//!   `fraction` with `column + 1`, documented here because it is a real
//!   property of the source arithmetic this decode reproduces exactly,
//!   not a property of this module's own code.
//! - **s16.16 sign handling and wraparound.** Neither half of
//!   `insertMatrix` performs any addition/subtraction on the patched
//!   value -- it is a pure bit-move: the integer half reinterprets 16
//!   raw bits as `int16_t` (two's-complement, full `-32768..32767`
//!   range, no saturation), the fraction half keeps 16 raw bits as an
//!   unsigned bit pattern. Ported here as `i16::from_ne_bytes`-equivalent
//!   plain `as i16` truncating casts from the extracted `u16` (matching
//!   C++'s `int16_t(value & 0xFFFF)`, an implementation-defined-then-
//!   universally-two's-complement narrowing conversion that Rust's `as`
//!   cast reproduces bit-for-bit) and a plain `u16` extraction for the
//!   fraction half. Because this is a bit-move with no arithmetic, there
//!   is no wraparound to characterize in *this* module -- the composed
//!   s16.16 value's sign/wraparound behavior on read-back
//!   (`fixed_to_float`'s cast-to-`i32`-then-divide) is already
//!   characterized in `rt64_common.rs` and is out of this module's scope
//!   (this module only decodes which halfword of `value` goes where; it
//!   does not call `fixed_to_float` or read a matrix cell back).
//!
//! **`G_MODIFYVTX` field decode:**
//!
//! - **`dstIndex` bound: `RSP_MAX_VERTICES = 256`.** `dstIndex >= 256` is
//!   upstream's `assert(false); return;` -- a no-op, ported as
//!   [`decode_modify_vertex`] returning `None`, same reasoning as
//!   `insertMatrix`'s out-of-range region. `dstIndex` is `uint16_t`
//!   upstream (max `65535`), so `>= 256` is a real, reachable input
//!   range, not merely a documentation nicety -- ported as `u16`.
//! - **`G_MWO_POINT_RGBA` (`0x10`): four unsigned byte extractions, no
//!   division, no signedness.** `(value >> 24) & 0xFF`, `(value >> 16) &
//!   0xFF`, `(value >> 8) & 0xFF`, `value & 0xFF`, each truncated to
//!   `u8`. Exact max value per field is `255` (`0xFF`); there is no
//!   "one above max" for a full-width byte mask (every `u32` input
//!   produces an in-range `u8` by construction) -- this module's tests
//!   instead cover the field-boundary values `0x00`/`0xFF` per byte lane
//!   and lane-order (verifying byte 3 of `value`, not byte 0, lands in
//!   `normColBytes[... + 0]`).
//! - **`G_MWO_POINT_ST` (`0x14`): two `int16_t` halves, each `/ 32.0f`.**
//!   `int16_t((value >> 16) & 0xFFFF) / 32.0f` for `s`, `int16_t(value &
//!   0xFFFF) / 32.0f` for `t`. Signed 16-bit halfwords (full
//!   `-32768..32767` range), divided by a plain `f32` literal `32.0` --
//!   ordinary float division, not fixed-point (`fixed_to_float`'s
//!   `65536.0` divisor and `FixedMatrix` machinery do **not** apply
//!   here; this is `modifyVertex`'s own, unrelated, coarser fixed-point
//!   convention: 5 fractional bits, not 16). Max magnitude is
//!   `32767 / 32.0 = 1024.90625`; the negative boundary `-32768 / 32.0 =
//!   -1024.0` is exact (no rounding, since `32768` is a multiple of
//!   `32`).
//! - **`G_MWO_POINT_XYSCREEN` (`0x18`): two `int16_t` halves, each `/
//!   4.0f`, plus a tag-encoded tuple.** `int16_t(extX) / 4.0f`,
//!   `int16_t(extY) / 4.0f` where `extX = (value >> 16) & 0xFFFF`,
//!   `extY = value & 0xFFFF` (both extracted as `uint16_t` first, then
//!   reinterpreted as `int16_t` -- ported as extracting `u16` then `as
//!   i16`, an identical bit-preserving cast). This port also decodes the
//!   `modifyPosUints` tag *formula* the C++ pushes ahead of the
//!   XY-vs-Z-selecting bit -- `global_index << 1` -- as a pure function
//!   of a caller-supplied `global_index: u32` (see "Nonclaims": this
//!   port does not own or touch the actual `modifyPosUints`/`posScreen`
//!   vectors, only the two formulas that would feed them).
//! - **`G_MWO_POINT_ZSCREEN` (`0x1C`): `value / 65536.0f` directly on
//!   the *unsigned* `uint32_t value`, no `int16_t` split.** This is the
//!   one attribute whose divisor (`65536.0`) matches `FixedMatrix`'s
//!   s16.16 convention numerically, but it is **not** the same
//!   operation as `fixed_to_float`: there is no `(int16_t, uint16_t)`
//!   pair here, no cast-to-`i32` reinterpret, and no sign involved at
//!   all -- `value` converts straight from `uint32_t` to `float` (an
//!   unsigned-to-float conversion; C++ never reinterprets it as signed
//!   for this field), then divides by `65536.0f`. Ported as `value as
//!   f32 / 65536.0`, an unsigned widening conversion, not a bit-pattern
//!   reinterpret -- deliberately **not** routed through
//!   `FixedMatrix::fixed_to_float` despite the shared divisor, since
//!   that helper's `(full_word as i32) as f32` step assumes a *signed*
//!   s16.16 value this field never constructs. Max value is
//!   `u32::MAX / 65536.0 = 65535.99998...`; there is no negative range
//!   for this field at all (a genuine asymmetry with `ST`/`XYSCREEN`,
//!   documented and tested explicitly). The tag formula is `(global_index
//!   << 1) | 0x1` (the low bit set distinguishes "Z-only" from
//!   XYSCREEN's "XY-only" `global_index << 1`), decoded the same way as
//!   XYSCREEN's tag.
//! - **Unrecognized `dstAttribute`.** Upstream's `default:` arm is
//!   `assert(false); break;` -- a debug-only guard with no field writes
//!   either way (the `switch` simply does nothing for an unrecognized
//!   selector in a release build). Ported as [`decode_modify_vertex`]
//!   returning `None` for any `dst_attribute` other than the four
//!   `G_MWO_POINT_*` constants, matching `decode_move_mem`'s established
//!   "unrecognized selector -> `None`" precedent again.
//!
//! ## Nonclaims
//!
//! This is a **third partial port** of `rt64_rsp.cpp`/`.h`: this same
//! source pair already has [`rt64_rsp_segment.rs`] (segmented-address
//! translation: `fromSegmented`/`fromSegmentedMasked`/
//! `fromSegmentedMaskedPD`/`maskPhysicalAddress`/`setSegment`) and a
//! concurrent M5.2 card (matrix *stack* push/pop, ported in a separate
//! worktree this module's author cannot see) both already claiming
//! disjoint slices of the same 1,314-line file. This module claims only
//! `insertMatrix`'s region/index decode and `modifyVertex`'s attribute-
//! field decode; it does not re-port, wrap, or depend on either sibling
//! slice, and no claim is made here about `rt64_rsp.cpp`/`.h`'s coverage
//! as a whole.
//!
//! No GPU, no WGSL, no production wiring -- this module is not called
//! from anywhere yet (dead-code warnings on the unused public surface are
//! expected and correct, matching every other characterization-first
//! module's precedent in this crate: `rt64_rsp_segment.rs`,
//! `rt64_gbi_f3d.rs`, and others), and no RT64 visual/pixel/silicon
//! parity or performance claim of any kind.
//!
//! `insertMatrix`'s orchestration is **not** ported: the three logical
//! matrices' *storage* (`modelMatrixStack`, `viewProjMatrixStack`,
//! `modelViewProjMatrix`), the stack-size-dependent selection of
//! *which* stack slot (`modelMatrixStack[modelMatrixStackSize - 1]`,
//! `viewProjMatrixStack[projectionMatrixStackSize - 1]`), and the
//! `modelViewProjInserted`/`projectionMatrixChanged`/
//! `projectionMatrixInversed` side-effect flag writes are all out of
//! scope -- this module's [`decode_insert_matrix`] returns which
//! [`MatrixRegion`] variant was selected (a bare enum tag, not a matrix
//! reference) plus the `row`/`column`/fractional-half facts; wiring that
//! to an actual `Mat4` and calling
//! `crate::rt64_common::FixedMatrix::modify_matrix4x4_integer`/
//! `modify_matrix4x4_fraction` is left to a future integration card, as
//! is anything resembling the actual per-matrix stacks themselves.
//!
//! `modifyVertex`'s **15-vector clone-on-first-write path** (the `if
//! (used[dstIndex])` block that duplicates a vertex's every attribute
//! array entry into a fresh slot before patching) is explicitly excluded
//! per this ticket's scope statement -- it is `Workload`/`DrawData`
//! object-graph plumbing with no bitfield decode to characterize, not a
//! pure function. This module's [`decode_modify_vertex`] takes an
//! already-resolved `global_index: u32` as a parameter (standing in for
//! whatever index the clone-or-reuse step above it would have produced)
//! and never reads or writes `indices`/`used`/any `Workload` field.
//! `branchZ` (the sibling function immediately after `modifyVertex` in
//! the source, at `rt64_rsp.cpp:837-848`) is unrelated dispatch and out
//! of scope entirely, not merely deferred.

use fn64_render_ir::Mat4;

/// `RSP_MAX_VERTICES` (`src/hle/rt64_rsp.h:27`): the fixed vertex-cache
/// capacity `modifyVertex`'s `dstIndex` bound-check compares against.
pub const RSP_MAX_VERTICES: u16 = 256;

/// `G_MWO_POINT_RGBA` (`src/shared/rt64_f3d_defines.h:80`).
pub const G_MWO_POINT_RGBA: u16 = 0x10;
/// `G_MWO_POINT_ST` (`src/shared/rt64_f3d_defines.h:81`).
pub const G_MWO_POINT_ST: u16 = 0x14;
/// `G_MWO_POINT_XYSCREEN` (`src/shared/rt64_f3d_defines.h:82`).
pub const G_MWO_POINT_XYSCREEN: u16 = 0x18;
/// `G_MWO_POINT_ZSCREEN` (`src/shared/rt64_f3d_defines.h:83`).
pub const G_MWO_POINT_ZSCREEN: u16 = 0x1C;

/// Which of `insertMatrix`'s three logical fixed-point matrices a given
/// `dstAddr` selected. Mirrors the `dstMat` C++ pointer's three possible
/// targets (`&modelMatrixStack[...]`, `&viewProjMatrix`,
/// `&modelViewProjMatrix`) as a bare tag -- see module doc "Nonclaims" for
/// why this port does not carry an actual matrix reference.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MatrixRegion {
    /// `dstAddr in [0x00, 0x40)`: `modelMatrixStack[modelMatrixStackSize - 1]`.
    Model,
    /// `dstAddr in [0x40, 0x80)`: `viewProjMatrixStack[projectionMatrixStackSize - 1]`.
    ViewProj,
    /// `dstAddr in [0x80, 0xC0)`: `modelViewProjMatrix`.
    ModelViewProj,
}

/// Decoded `insertMatrix(address, value)` region/index/half selection,
/// everything upstream computes before its two
/// `FixedMatrix::modifyMatrix4x4Integer`/`Fraction` calls. Does not itself
/// touch a [`Mat4`] -- see module doc "Nonclaims".
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InsertMatrixTarget {
    /// Which logical matrix `address` selected.
    pub region: MatrixRegion,
    /// `row = index / 4`, always in `0..4` (see module doc's `column + 1`
    /// discussion for why `column` itself does not carry the same
    /// unconditional guarantee).
    pub row: u32,
    /// `column = index % 4`, always in `0..4` for the decode itself (the
    /// *paired* write at `column + 1` can reach `4` for an unaligned
    /// `address` -- see module doc "Admitted domain").
    pub column: u32,
    /// `true` selects the fraction half (`modifyMatrix4x4Fraction`),
    /// `false` selects the integer half (`modifyMatrix4x4Integer`).
    pub modify_fractional: bool,
    /// `int16_t((value >> 16) & 0xFFFF)` (integer half) / `uint16_t((value
    /// >> 16) & 0xFFFF)` reinterpreted (fraction half) -- the halfword
    /// written to `column`. Always the raw 16 bits; caller narrows per
    /// `modify_fractional`.
    pub high_halfword: u16,
    /// `int16_t(value & 0xFFFF)` / `uint16_t(value & 0xFFFF)` -- the
    /// halfword written to `column + 1`.
    pub low_halfword: u16,
}

/// Literal port of `RSP::insertMatrix`'s region/index decode
/// (`src/hle/rt64_rsp.cpp:240-301`, decode portion only -- see module doc
/// "Nonclaims"). Returns `None` for `dstAddr >= 0xC0`, matching upstream's
/// `assert(false); return;` no-op (see module doc "Admitted domain").
pub fn decode_insert_matrix(address: u32, value: u32) -> Option<InsertMatrixTarget> {
    const MATRIX_SIZE: u32 = 0x40;
    const FRACTIONAL_MATRIX_ADDRESS: u32 = MATRIX_SIZE / 2;
    const MODEL_ADDRESS: u32 = 0x0;
    const VIEW_PROJ_ADDRESS: u32 = MODEL_ADDRESS + MATRIX_SIZE;
    const MODEL_VIEW_PROJ_ADDRESS: u32 = VIEW_PROJ_ADDRESS + MATRIX_SIZE;

    let dst_addr = address.wrapping_add(MODEL_VIEW_PROJ_ADDRESS) & 0xFFFF;

    if dst_addr >= (MODEL_VIEW_PROJ_ADDRESS + MATRIX_SIZE) {
        return None;
    }

    let (region, mut rel_addr) = if dst_addr >= MODEL_VIEW_PROJ_ADDRESS {
        (
            MatrixRegion::ModelViewProj,
            dst_addr - MODEL_VIEW_PROJ_ADDRESS,
        )
    } else if dst_addr >= VIEW_PROJ_ADDRESS {
        (MatrixRegion::ViewProj, dst_addr - VIEW_PROJ_ADDRESS)
    } else {
        (MatrixRegion::Model, dst_addr - MODEL_ADDRESS)
    };

    let modify_fractional = rel_addr >= FRACTIONAL_MATRIX_ADDRESS;
    if modify_fractional {
        rel_addr -= FRACTIONAL_MATRIX_ADDRESS;
    }

    let index = rel_addr / 2;
    let row = index / 4;
    let column = index % 4;

    Some(InsertMatrixTarget {
        region,
        row,
        column,
        modify_fractional,
        high_halfword: ((value >> 16) & 0xFFFF) as u16,
        low_halfword: (value & 0xFFFF) as u16,
    })
}

/// Applies a decoded [`InsertMatrixTarget`] to `matrix` via
/// `crate::rt64_common::FixedMatrix`'s reused helpers -- the two calls
/// `insertMatrix` itself makes at the bottom of the function
/// (`src/hle/rt64_rsp.cpp:294-301`). Convenience wrapper only; the decode
/// in [`decode_insert_matrix`] is the tested unit, this function is not
/// independently characterized beyond that composition (no additional
/// behavior of its own).
pub fn apply_insert_matrix(matrix: &mut Mat4, target: &InsertMatrixTarget) {
    if target.modify_fractional {
        crate::rt64_common::FixedMatrix::modify_matrix4x4_fraction(
            matrix,
            target.row as usize,
            target.column as usize,
            target.high_halfword,
        );
        crate::rt64_common::FixedMatrix::modify_matrix4x4_fraction(
            matrix,
            target.row as usize,
            target.column as usize + 1,
            target.low_halfword,
        );
    } else {
        crate::rt64_common::FixedMatrix::modify_matrix4x4_integer(
            matrix,
            target.row as usize,
            target.column as usize,
            target.high_halfword as i16,
        );
        crate::rt64_common::FixedMatrix::modify_matrix4x4_integer(
            matrix,
            target.row as usize,
            target.column as usize + 1,
            target.low_halfword as i16,
        );
    }
}

/// Decoded `G_MODIFYVTX` attribute patch (`RSP::modifyVertex`'s attribute
/// `switch`, `src/hle/rt64_rsp.cpp:791-834`). Each variant carries exactly
/// the fields upstream computes for that attribute; see module doc
/// "Admitted domain" for each field's divisor/signedness.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ModifyVertexPatch {
    /// `G_MWO_POINT_RGBA`: four raw unsigned bytes, MSB-first.
    Rgba { r: u8, g: u8, b: u8, a: u8 },
    /// `G_MWO_POINT_ST`: two signed-fixed (`/32.0`) texture-coordinate
    /// floats, plus the `lookAtIndices[globalIndex] = 0` reset tag (a
    /// bare `bool` here, since the reset value itself is a constant --
    /// see module doc "Nonclaims" for why the actual array write is out
    /// of scope).
    St { s: f32, t: f32 },
    /// `G_MWO_POINT_XYSCREEN`: two signed-fixed (`/4.0`) screen
    /// coordinates, plus the `modifyPosUints` tag word
    /// (`global_index << 1`).
    XyScreen { x: f32, y: f32, tag: u32 },
    /// `G_MWO_POINT_ZSCREEN`: one unsigned-fixed (`/65536.0`) screen
    /// depth, plus the `modifyPosUints` tag word
    /// (`(global_index << 1) | 0x1`).
    ZScreen { z: f32, tag: u32 },
}

/// Literal port of `RSP::modifyVertex`'s vertex-index bound check plus
/// attribute-field decode (`src/hle/rt64_rsp.cpp:737-834`, decode portion
/// only -- see module doc "Nonclaims"). Returns `None` for `dst_index >=
/// RSP_MAX_VERTICES` (upstream's `assert(false); return;`) or an
/// unrecognized `dst_attribute` (upstream's `default: assert(false);
/// break;`).
pub fn decode_modify_vertex(
    dst_index: u16,
    dst_attribute: u16,
    value: u32,
    global_index: u32,
) -> Option<ModifyVertexPatch> {
    if dst_index >= RSP_MAX_VERTICES {
        return None;
    }

    match dst_attribute {
        G_MWO_POINT_RGBA => Some(ModifyVertexPatch::Rgba {
            r: ((value >> 24) & 0xFF) as u8,
            g: ((value >> 16) & 0xFF) as u8,
            b: ((value >> 8) & 0xFF) as u8,
            a: (value & 0xFF) as u8,
        }),
        G_MWO_POINT_ST => {
            let s = (((value >> 16) & 0xFFFF) as u16 as i16) as f32 / 32.0;
            let t = ((value & 0xFFFF) as u16 as i16) as f32 / 32.0;
            Some(ModifyVertexPatch::St { s, t })
        }
        G_MWO_POINT_XYSCREEN => {
            let ext_x = ((value >> 16) & 0xFFFF) as u16;
            let ext_y = (value & 0xFFFF) as u16;
            let x = (ext_x as i16) as f32 / 4.0;
            let y = (ext_y as i16) as f32 / 4.0;
            Some(ModifyVertexPatch::XyScreen {
                x,
                y,
                tag: global_index << 1,
            })
        }
        G_MWO_POINT_ZSCREEN => Some(ModifyVertexPatch::ZScreen {
            z: value as f32 / 65536.0,
            tag: (global_index << 1) | 0x1,
        }),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- decode_insert_matrix: region selection ---

    #[test]
    fn region_model_low_boundary() {
        // dstAddr = (address + 0x80) & 0xFFFF must land at 0x00 (Model's
        // first byte): address = 0xFF80 wraps to 0.
        let t = decode_insert_matrix(0xFF80, 0).unwrap();
        assert_eq!(t.region, MatrixRegion::Model);
        assert_eq!(t.row, 0);
        assert_eq!(t.column, 0);
        assert!(!t.modify_fractional);
    }

    #[test]
    fn region_model_high_boundary_last_integer_halfword() {
        // dstAddr = 0x3C -> address = 0x3C - 0x80 wraps low; use address
        // such that dstAddr lands exactly at 0x3C (relAddr=0x3C, index=30,
        // row=7... wait relAddr must stay < 0x40 for Model). address =
        // 0xFFBC -> dstAddr = (0xFFBC + 0x80) & 0xFFFF = 0x3C.
        let t = decode_insert_matrix(0xFFBC, 0).unwrap();
        assert_eq!(t.region, MatrixRegion::Model);
        // relAddr = 0x3C, modifyFractional (>= 0x20) -> true, relAddr -= 0x20 -> 0x1C
        assert!(t.modify_fractional);
        let index = 0x1Cu32 / 2;
        assert_eq!(t.row, index / 4);
        assert_eq!(t.column, index % 4);
    }

    #[test]
    fn region_model_upper_exclusive_boundary_is_viewproj() {
        // dstAddr = 0x40 must be ViewProj, not Model.
        let t = decode_insert_matrix(0xFFC0, 0).unwrap(); // (0xFFC0+0x80)&0xFFFF = 0x40
        assert_eq!(t.region, MatrixRegion::ViewProj);
        assert_eq!(t.row, 0);
        assert_eq!(t.column, 0);
        assert!(!t.modify_fractional);
    }

    #[test]
    fn region_viewproj_low_boundary() {
        let t = decode_insert_matrix(0xFFC0, 0).unwrap();
        assert_eq!(t.region, MatrixRegion::ViewProj);
    }

    #[test]
    fn region_viewproj_high_boundary_last_fraction_halfword() {
        // dstAddr = 0x7C -> relAddr(ViewProj) = 0x3C -> fractional, rel=0x1C
        let t = decode_insert_matrix(0xFFFC, 0).unwrap(); // (0xFFFC+0x80)&0xFFFF=0x7C
        assert_eq!(t.region, MatrixRegion::ViewProj);
        assert!(t.modify_fractional);
    }

    #[test]
    fn region_viewproj_upper_exclusive_boundary_is_modelviewproj() {
        // dstAddr = 0x80 -> address = 0 (address+0x80=0x80).
        let t = decode_insert_matrix(0, 0).unwrap();
        assert_eq!(t.region, MatrixRegion::ModelViewProj);
        assert_eq!(t.row, 0);
        assert_eq!(t.column, 0);
        assert!(!t.modify_fractional);
    }

    #[test]
    fn region_modelviewproj_low_boundary() {
        let t = decode_insert_matrix(0, 0).unwrap();
        assert_eq!(t.region, MatrixRegion::ModelViewProj);
    }

    #[test]
    fn region_modelviewproj_high_boundary_last_fraction_halfword() {
        // dstAddr = 0xBC -> address = 0xBC - 0x80 = 0x3C.
        let t = decode_insert_matrix(0x3C, 0).unwrap();
        assert_eq!(t.region, MatrixRegion::ModelViewProj);
        assert!(t.modify_fractional);
        // relAddr = 0x3C - 0x20 = 0x1C -> index=14, row=3, column=2
        assert_eq!(t.row, 3);
        assert_eq!(t.column, 2);
    }

    #[test]
    fn region_out_of_range_at_lower_bound_is_none() {
        // dstAddr = 0xC0 exactly: out of range (first out-of-range value).
        let addr = 0xC0u32.wrapping_sub(0x80) & 0xFFFF; // address such that dstAddr=0xC0
        assert!(decode_insert_matrix(addr, 0).is_none());
    }

    #[test]
    fn region_in_range_one_below_out_of_range_boundary() {
        // dstAddr = 0xBF: still ModelViewProj (last relAddr=0x3F).
        let addr = 0xBFu32.wrapping_sub(0x80) & 0xFFFF;
        let t = decode_insert_matrix(addr, 0).unwrap();
        assert_eq!(t.region, MatrixRegion::ModelViewProj);
    }

    #[test]
    fn region_out_of_range_at_upper_bound_is_none() {
        // dstAddr = 0xFFFF: out of range.
        let addr = 0xFFFFu32.wrapping_sub(0x80) & 0xFFFF;
        assert!(decode_insert_matrix(addr, 0).is_none());
    }

    #[test]
    fn region_out_of_range_mid_range_is_none() {
        let addr = 0x8000u32.wrapping_sub(0x80) & 0xFFFF;
        assert!(decode_insert_matrix(addr, 0).is_none());
    }

    #[test]
    fn address_wraps_modulo_0x10000() {
        // address = 0xFFFFFFFF: (address + 0x80) wraps around u32, then &
        // 0xFFFF selects the low 16 bits, matching C++ uint32_t wraparound
        // (well-defined modular arithmetic in C++, ported via wrapping_add).
        let dst_addr = (0xFFFF_FFFFu32.wrapping_add(0x80)) & 0xFFFF;
        assert_eq!(dst_addr, 0x7F); // sanity on the expected wrapped dstAddr
        let t = decode_insert_matrix(0xFFFF_FFFF, 0).unwrap();
        assert_eq!(t.region, MatrixRegion::ViewProj);
    }

    // --- decode_insert_matrix: every region index, exhaustive-ish sweep ---

    #[test]
    fn every_region_every_row_column_integer_half_model() {
        for index in 0u32..16 {
            let rel_addr = index * 2; // integer half: relAddr < 0x20
            let address = rel_addr.wrapping_sub(0x80) & 0xFFFF; // dstAddr = rel_addr
            let t = decode_insert_matrix(address, 0).unwrap();
            assert_eq!(t.region, MatrixRegion::Model);
            assert!(!t.modify_fractional);
            assert_eq!(t.row, index / 4);
            assert_eq!(t.column, index % 4);
        }
    }

    #[test]
    fn every_region_every_row_column_fraction_half_viewproj() {
        for index in 0u32..16 {
            let rel_addr = 0x20 + index * 2; // fraction half within ViewProj
            let dst_addr = 0x40 + rel_addr;
            let address = dst_addr.wrapping_sub(0x80) & 0xFFFF;
            let t = decode_insert_matrix(address, 0).unwrap();
            assert_eq!(t.region, MatrixRegion::ViewProj);
            assert!(t.modify_fractional);
            assert_eq!(t.row, index / 4);
            assert_eq!(t.column, index % 4);
        }
    }

    #[test]
    fn every_region_every_row_column_modelviewproj_both_halves() {
        for index in 0u32..16 {
            // integer half
            let rel_addr_int = index * 2;
            let dst_addr_int = 0x80 + rel_addr_int;
            let address_int = dst_addr_int.wrapping_sub(0x80) & 0xFFFF;
            let t_int = decode_insert_matrix(address_int, 0).unwrap();
            assert_eq!(t_int.region, MatrixRegion::ModelViewProj);
            assert!(!t_int.modify_fractional);
            assert_eq!(t_int.row, index / 4);
            assert_eq!(t_int.column, index % 4);

            // fraction half
            let rel_addr_frac = 0x20 + index * 2;
            let dst_addr_frac = 0x80 + rel_addr_frac;
            let address_frac = dst_addr_frac.wrapping_sub(0x80) & 0xFFFF;
            let t_frac = decode_insert_matrix(address_frac, 0).unwrap();
            assert_eq!(t_frac.region, MatrixRegion::ModelViewProj);
            assert!(t_frac.modify_fractional);
            assert_eq!(t_frac.row, index / 4);
            assert_eq!(t_frac.column, index % 4);
        }
    }

    // --- decode_insert_matrix: halfword extraction, sign boundary ---

    #[test]
    fn halfwords_split_high_low_from_value() {
        let t = decode_insert_matrix(0xFF80, 0x1234_5678).unwrap();
        assert_eq!(t.high_halfword, 0x1234);
        assert_eq!(t.low_halfword, 0x5678);
    }

    #[test]
    fn halfword_zero() {
        let t = decode_insert_matrix(0xFF80, 0x0000_0000).unwrap();
        assert_eq!(t.high_halfword, 0);
        assert_eq!(t.low_halfword, 0);
    }

    #[test]
    fn halfword_all_ones() {
        let t = decode_insert_matrix(0xFF80, 0xFFFF_FFFF).unwrap();
        assert_eq!(t.high_halfword, 0xFFFF);
        assert_eq!(t.low_halfword, 0xFFFF);
    }

    #[test]
    fn integer_half_sign_boundary_positive_max() {
        let t = decode_insert_matrix(0xFF80, 0x7FFF_0000).unwrap();
        assert!(!t.modify_fractional);
        assert_eq!(t.high_halfword as i16, 32767);
    }

    #[test]
    fn integer_half_sign_boundary_negative_min() {
        let t = decode_insert_matrix(0xFF80, 0x8000_0000).unwrap();
        assert!(!t.modify_fractional);
        assert_eq!(t.high_halfword as i16, -32768);
    }

    #[test]
    fn integer_half_sign_boundary_negative_one() {
        let t = decode_insert_matrix(0xFF80, 0xFFFF_0000).unwrap();
        assert_eq!(t.high_halfword as i16, -1);
    }

    // --- apply_insert_matrix composition (exercises the reused helpers) ---

    #[test]
    fn apply_integer_half_writes_row_column_and_column_plus_one() {
        let mut m = Mat4::from_rows([fn64_render_ir::Vec4::new(0.0, 0.0, 0.0, 0.0); 4]);
        // value = 0x00030002: high_halfword=3 -> column 0, low_halfword=2 -> column 1.
        let target = decode_insert_matrix(0xFF80, 0x0003_0002).unwrap(); // Model, row0,col0
        assert_eq!(target.row, 0);
        assert_eq!(target.column, 0);
        assert!(!target.modify_fractional);
        apply_insert_matrix(&mut m, &target);
        // modify_matrix4x4_integer(m, 0, 0, 3) then (m, 0, 1, 2): each sets the
        // integer half only, with a previously-zero (hence zero) fraction half,
        // so the resulting float is exactly the integer value.
        assert_eq!(m.rows[0].x, 3.0);
        assert_eq!(m.rows[0].y, 2.0);
    }

    #[test]
    fn apply_fraction_half_preserves_prior_integer_bits() {
        let mut m = Mat4::from_rows([fn64_render_ir::Vec4::new(0.0, 0.0, 0.0, 0.0); 4]);
        crate::rt64_common::FixedMatrix::modify_matrix4x4_integer(&mut m, 0, 0, 5);
        assert_eq!(m.rows[0].x, 5.0);
        // dstAddr for Model fraction half, row0 col0: relAddr=0x20 -> dstAddr=0x20
        let address = 0x20u32.wrapping_sub(0x80) & 0xFFFF;
        let target = decode_insert_matrix(address, 0x8000_0000).unwrap();
        assert!(target.modify_fractional);
        assert_eq!(target.row, 0);
        assert_eq!(target.column, 0);
        apply_insert_matrix(&mut m, &target);
        // high_halfword = 0x8000 written as the fraction half of [0][0],
        // preserving the integer half (5): 5 + 0x8000/65536 = 5.5.
        assert_eq!(m.rows[0].x, 5.5);
    }

    // --- decode_modify_vertex: dstIndex bound ---

    #[test]
    fn modify_vertex_index_at_max_valid() {
        assert!(decode_modify_vertex(255, G_MWO_POINT_RGBA, 0, 0).is_some());
    }

    #[test]
    fn modify_vertex_index_one_above_max_is_none() {
        assert!(decode_modify_vertex(256, G_MWO_POINT_RGBA, 0, 0).is_none());
    }

    #[test]
    fn modify_vertex_index_zero_is_valid() {
        assert!(decode_modify_vertex(0, G_MWO_POINT_RGBA, 0, 0).is_some());
    }

    #[test]
    fn modify_vertex_index_far_above_max_is_none() {
        assert!(decode_modify_vertex(65535, G_MWO_POINT_RGBA, 0, 0).is_none());
    }

    // --- decode_modify_vertex: G_MWO_POINT_RGBA ---

    #[test]
    fn rgba_byte_lane_order() {
        let patch = decode_modify_vertex(0, G_MWO_POINT_RGBA, 0x11223344, 0).unwrap();
        match patch {
            ModifyVertexPatch::Rgba { r, g, b, a } => {
                assert_eq!((r, g, b, a), (0x11, 0x22, 0x33, 0x44));
            }
            _ => panic!("expected Rgba"),
        }
    }

    #[test]
    fn rgba_max_byte_values() {
        let patch = decode_modify_vertex(0, G_MWO_POINT_RGBA, 0xFFFFFFFF, 0).unwrap();
        match patch {
            ModifyVertexPatch::Rgba { r, g, b, a } => {
                assert_eq!((r, g, b, a), (0xFF, 0xFF, 0xFF, 0xFF));
            }
            _ => panic!("expected Rgba"),
        }
    }

    #[test]
    fn rgba_zero() {
        let patch = decode_modify_vertex(0, G_MWO_POINT_RGBA, 0, 0).unwrap();
        match patch {
            ModifyVertexPatch::Rgba { r, g, b, a } => assert_eq!((r, g, b, a), (0, 0, 0, 0)),
            _ => panic!("expected Rgba"),
        }
    }

    // --- decode_modify_vertex: G_MWO_POINT_ST ---

    #[test]
    fn st_positive_values() {
        let patch = decode_modify_vertex(0, G_MWO_POINT_ST, 0x0020_0040, 0).unwrap();
        match patch {
            ModifyVertexPatch::St { s, t } => {
                assert_eq!(s, 32.0 / 32.0);
                assert_eq!(t, 64.0 / 32.0);
            }
            _ => panic!("expected St"),
        }
    }

    #[test]
    fn st_negative_values() {
        // -32 as i16 = 0xFFE0
        let patch = decode_modify_vertex(0, G_MWO_POINT_ST, 0xFFE0_FFE0, 0).unwrap();
        match patch {
            ModifyVertexPatch::St { s, t } => {
                assert_eq!(s, -1.0);
                assert_eq!(t, -1.0);
            }
            _ => panic!("expected St"),
        }
    }

    #[test]
    fn st_sign_boundary_min_i16() {
        let patch = decode_modify_vertex(0, G_MWO_POINT_ST, 0x8000_0000, 0).unwrap();
        match patch {
            ModifyVertexPatch::St { s, .. } => assert_eq!(s, -1024.0),
            _ => panic!("expected St"),
        }
    }

    #[test]
    fn st_sign_boundary_max_i16() {
        let patch = decode_modify_vertex(0, G_MWO_POINT_ST, 0x7FFF_0000, 0).unwrap();
        match patch {
            ModifyVertexPatch::St { s, .. } => assert_eq!(s, 32767.0 / 32.0),
            _ => panic!("expected St"),
        }
    }

    #[test]
    fn st_zero() {
        let patch = decode_modify_vertex(0, G_MWO_POINT_ST, 0, 0).unwrap();
        match patch {
            ModifyVertexPatch::St { s, t } => {
                assert_eq!(s, 0.0);
                assert_eq!(t, 0.0);
            }
            _ => panic!("expected St"),
        }
    }

    // --- decode_modify_vertex: G_MWO_POINT_XYSCREEN ---

    #[test]
    fn xyscreen_positive_values_and_tag() {
        let patch = decode_modify_vertex(0, G_MWO_POINT_XYSCREEN, 0x0008_0010, 7).unwrap();
        match patch {
            ModifyVertexPatch::XyScreen { x, y, tag } => {
                assert_eq!(x, 8.0 / 4.0);
                assert_eq!(y, 16.0 / 4.0);
                assert_eq!(tag, 7 << 1);
            }
            _ => panic!("expected XyScreen"),
        }
    }

    #[test]
    fn xyscreen_negative_values() {
        // -4 as i16 = 0xFFFC
        let patch = decode_modify_vertex(0, G_MWO_POINT_XYSCREEN, 0xFFFC_FFFC, 0).unwrap();
        match patch {
            ModifyVertexPatch::XyScreen { x, y, .. } => {
                assert_eq!(x, -1.0);
                assert_eq!(y, -1.0);
            }
            _ => panic!("expected XyScreen"),
        }
    }

    #[test]
    fn xyscreen_sign_boundary_min_i16() {
        let patch = decode_modify_vertex(0, G_MWO_POINT_XYSCREEN, 0x8000_8000, 0).unwrap();
        match patch {
            ModifyVertexPatch::XyScreen { x, y, .. } => {
                assert_eq!(x, -32768.0 / 4.0);
                assert_eq!(y, -32768.0 / 4.0);
            }
            _ => panic!("expected XyScreen"),
        }
    }

    #[test]
    fn xyscreen_sign_boundary_max_i16() {
        let patch = decode_modify_vertex(0, G_MWO_POINT_XYSCREEN, 0x7FFF_7FFF, 0).unwrap();
        match patch {
            ModifyVertexPatch::XyScreen { x, y, .. } => {
                assert_eq!(x, 32767.0 / 4.0);
                assert_eq!(y, 32767.0 / 4.0);
            }
            _ => panic!("expected XyScreen"),
        }
    }

    #[test]
    fn xyscreen_tag_low_bit_always_clear() {
        let patch = decode_modify_vertex(0, G_MWO_POINT_XYSCREEN, 0, 3).unwrap();
        match patch {
            ModifyVertexPatch::XyScreen { tag, .. } => assert_eq!(tag & 0x1, 0),
            _ => panic!("expected XyScreen"),
        }
    }

    #[test]
    fn xyscreen_tag_large_global_index() {
        let patch = decode_modify_vertex(0, G_MWO_POINT_XYSCREEN, 0, 255).unwrap();
        match patch {
            ModifyVertexPatch::XyScreen { tag, .. } => assert_eq!(tag, 510),
            _ => panic!("expected XyScreen"),
        }
    }

    // --- decode_modify_vertex: G_MWO_POINT_ZSCREEN ---

    #[test]
    fn zscreen_positive_value_and_tag() {
        let patch = decode_modify_vertex(0, G_MWO_POINT_ZSCREEN, 65536, 5).unwrap();
        match patch {
            ModifyVertexPatch::ZScreen { z, tag } => {
                assert_eq!(z, 1.0);
                assert_eq!(tag, (5 << 1) | 0x1);
            }
            _ => panic!("expected ZScreen"),
        }
    }

    #[test]
    fn zscreen_zero() {
        let patch = decode_modify_vertex(0, G_MWO_POINT_ZSCREEN, 0, 0).unwrap();
        match patch {
            ModifyVertexPatch::ZScreen { z, .. } => assert_eq!(z, 0.0),
            _ => panic!("expected ZScreen"),
        }
    }

    #[test]
    fn zscreen_max_u32_has_no_negative_range() {
        // Unlike ST/XYSCREEN, ZSCREEN's value is never reinterpreted as
        // signed -- the top bit does not flip the sign.
        let patch = decode_modify_vertex(0, G_MWO_POINT_ZSCREEN, 0xFFFF_FFFF, 0).unwrap();
        match patch {
            ModifyVertexPatch::ZScreen { z, .. } => {
                assert!(z > 0.0);
                assert_eq!(z, u32::MAX as f32 / 65536.0);
            }
            _ => panic!("expected ZScreen"),
        }
    }

    #[test]
    fn zscreen_top_bit_set_still_positive() {
        // Contrast with ST/XYSCREEN's sign boundary: 0x80000000 here is
        // still a large positive value, not -32768-scaled.
        let patch = decode_modify_vertex(0, G_MWO_POINT_ZSCREEN, 0x8000_0000, 0).unwrap();
        match patch {
            ModifyVertexPatch::ZScreen { z, .. } => assert_eq!(z, 32768.0),
            _ => panic!("expected ZScreen"),
        }
    }

    #[test]
    fn zscreen_tag_low_bit_always_set() {
        let patch = decode_modify_vertex(0, G_MWO_POINT_ZSCREEN, 0, 3).unwrap();
        match patch {
            ModifyVertexPatch::ZScreen { tag, .. } => assert_eq!(tag & 0x1, 1),
            _ => panic!("expected ZScreen"),
        }
    }

    // --- decode_modify_vertex: unrecognized attribute ---

    #[test]
    fn unrecognized_attribute_is_none() {
        assert!(decode_modify_vertex(0, 0x00, 0, 0).is_none());
    }

    #[test]
    fn unrecognized_attribute_one_below_rgba_is_none() {
        assert!(decode_modify_vertex(0, G_MWO_POINT_RGBA - 1, 0, 0).is_none());
    }

    #[test]
    fn unrecognized_attribute_between_rgba_and_st_is_none() {
        assert!(decode_modify_vertex(0, G_MWO_POINT_RGBA + 1, 0, 0).is_none());
    }

    #[test]
    fn unrecognized_attribute_one_above_zscreen_is_none() {
        assert!(decode_modify_vertex(0, G_MWO_POINT_ZSCREEN + 1, 0, 0).is_none());
    }

    #[test]
    fn unrecognized_attribute_far_out_of_range_is_none() {
        assert!(decode_modify_vertex(0, 0xFFFF, 0, 0).is_none());
    }

    // --- constants sanity (protects against silent constant drift) ---

    #[test]
    fn constants_match_upstream_literal_values() {
        assert_eq!(RSP_MAX_VERTICES, 256);
        assert_eq!(G_MWO_POINT_RGBA, 0x10);
        assert_eq!(G_MWO_POINT_ST, 0x14);
        assert_eq!(G_MWO_POINT_XYSCREEN, 0x18);
        assert_eq!(G_MWO_POINT_ZSCREEN, 0x1C);
    }
}
