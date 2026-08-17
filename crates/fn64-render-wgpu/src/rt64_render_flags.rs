//! The `NATIVE_SAMPLER_*` constants, `interop::RenderFlags`' bit layout and
//! its twenty HLSL-side accessors, plus `interop::GPUTileFlags`' bit layout,
//! its six HLSL-side predicates and `interop::GPUTile`'s field list: a literal
//! port of the permitted MIT RT64 source pinned at commit
//! `5473732a822a4423b5696e7cb18fecc425a59875`
//! (`docs/rt64-port-authority.json`), from two headers in the same shared
//! HLSL/C++ family that this crate has already ported `rt64_hlsl.h` and
//! `rt64_extra_params.h` from.
//!
//! One module covers both headers -- the pattern `rt64_common.rs` already uses
//! for its `.h`/`.cpp` pair -- because `rt64_gpu_tile.h`'s `GPUTileFlags` is
//! the same construct as `rt64_render_flags.h`'s `RenderFlags` (a `union` of a
//! bitfield `struct` with a `uint value`, mirrored by manual shift/mask
//! accessors on the HLSL side for the same stated reason), and because the two
//! interlock: `RenderFlags::dynamicTiles` and `RenderFlags::canDecodeTMEM`
//! govern the tiles that `GPUTile` describes.
//!
//! ## Source digests (independently computed, cross-checked against the inventory)
//!
//! Both digests below were computed here with `shasum -a 256` against the
//! pinned checkout, then cross-checked verbatim against
//! `docs/rt64-port-inventory.json`'s `sources.port.sha256` for each path.
//! **Both match; there is no mismatch to report.**
//!
//! - `src/shared/rt64_render_flags.h` --
//!   `f4e6a5a7704c7f315ba03054027bd4ccf218c9005c7b370f8a883e21cdc68a5d`,
//!   133 newline-terminated lines plus a final unterminated line -- the
//!   trailing `#endif` -- which the inventory records as 134.
//! - `src/shared/rt64_gpu_tile.h` --
//!   `4d7aff6f3191007a094f3556fbe47ffcf30f387cdc70c03bb84c3b93b303c6c3`,
//!   63 newline-terminated lines plus a final unterminated `#endif`, which
//!   the inventory records as 64.
//!
//! For **both** paths the inventory's `sources.oracle.sha256` records the
//! *identical* digest and `"port_delta": "unchanged"`, so the port pin
//! (`5473732a`) and the deliberately-different oracle pin (`f0728a25`) agree
//! on these two files byte for byte. No delta was detected and none is
//! claimed.
//!
//! ## Inventory drift disclosure
//!
//! Per file, what fraction of the cited source this module ports:
//!
//! - **`rt64_render_flags.h`: full port.** Every construct in the file is
//!   represented: the 10 `NATIVE_SAMPLER_*` macros (lines 9-18), the
//!   `RenderFlags` bitfield's 20 fields and their widths (lines 24-49), and
//!   all 20 HLSL accessor functions (lines 52-130). What remains is the
//!   `#ifdef`/`namespace` scaffolding (lines 5-7, 20-23, 50-51, 131-134),
//!   refused below as preprocessor plumbing with no runtime behavior.
//! - **`rt64_gpu_tile.h`: full port of the behavior, partial port of the
//!   file.** The `GPUTileFlags` bitfield's 6 fields (lines 13-24), the
//!   `typedef uint GPUTileFlags` (line 27) and all 6 predicates (lines 29-51)
//!   are ported. `struct GPUTile`'s 8-field *list* (lines 53-61) is recorded
//!   as documentation and as a field-name/type constant list, but **no Rust
//!   `GPUTile` struct is defined** -- see "Reuse, not new type", because this
//!   crate already carries the two of its fields that have consumers. Call it
//!   roughly 50 of the file's 64 lines carrying behavior, all of which is
//!   ported.
//!
//! `docs/rt64-port-inventory.json` records `"ported_as": []` and
//! `"port_state": "not-started"` for both paths, and its `task_card`s name
//! **two separate** writable paths -- `rt64_render_flags_h.rs` and
//! `rt64_gpu_tile.rs` -- whereas this card's exclusive path is the single
//! combined `rt64_render_flags.rs`. Both are therefore drifted from the
//! inventory in two ways (state and filename), and `scripts/lint-docs.py`'s
//! inventory scanner is expected to report a `ported_as` drift line until a
//! follow-up regenerates the inventory. This card's writable surface does not
//! include `docs/rt64-port-inventory.json`, so that reconciliation is left to
//! the owning ticket. Note also that the inventory credits a source as
//! `ported` at **file** granularity, so a partial port of a large file would
//! still be credited in full; that is why the per-file fraction is stated
//! explicitly above rather than left to the inventory's boolean.
//!
//! ## Ported / refused boundary, and the criterion
//!
//! **Criterion**: a construct is ported when its behavior is fully determined
//! by values and control flow present in the cited file -- no shader
//! compiler, no C++ compiler's layout or bitfield-allocation algorithm, no
//! GPU, and no type from an uncited or unpopulated file. A construct that
//! merely *names* a layout the hardware or a compiler decides is refused.
//!
//! **Ported**:
//! - the 10 `NATIVE_SAMPLER_*` macros, as `u32` constants.
//! - `RenderFlags`' 20 accessors, as free functions taking `u32`.
//! - `RenderFlags`' field widths and bit offsets, as the [`RENDER_FLAG_FIELDS`]
//!   table -- see "Admitted domain" for why this is defensible when a
//!   `repr(C)` claim would not be.
//! - `GPUTileFlags`' 6 predicates, as free functions.
//! - `GPUTileFlags`' 6 field widths/offsets, as [`GPU_TILE_FLAG_FIELDS`].
//! - `GPUTile`'s field name/type list, as [`GPU_TILE_FIELDS`] documentation
//!   data (not a struct).
//!
//! **Refused / not modelled** (named):
//! - **The `#ifdef HLSL_CPU` / `#else` / `#endif` scaffolding and the
//!   `namespace interop { ... };` wrapper** (`rt64_render_flags.h:20-23`,
//!   `50-51`, `131-134`; `rt64_gpu_tile.h:9-12`, `25`, `52`, `62-64`), and the
//!   `#pragma once` / `#include "shared/rt64_hlsl.h"` prologue at the top of
//!   each. Preprocessor and namespace plumbing selecting whether the file
//!   compiles as C++ or as HLSL. Rust has no preprocessor and this is not
//!   behavior.
//! - **The C++ `union` construct itself.** Upstream reads and writes the same
//!   32 bits two ways: through named bitfield members and through the
//!   overlapping `uint value`. Rust's `union` would reproduce the *syntax*,
//!   but the correspondence between the two views is exactly the thing this
//!   port cannot prove (next bullet). The accessors are ported as functions
//!   over a plain `u32`, which is how the HLSL side of the very same file
//!   already expresses it.
//! - **Every `repr(C)`, size, alignment, byte-offset, bitfield-allocation and
//!   constant-buffer-layout claim, for both unions and for `GPUTile`.** No
//!   type here carries `repr(C)`. C++ bitfield allocation order within a
//!   storage unit is *implementation-defined*, so "`rect` occupies the least
//!   significant bit of `value`" is a property of MSVC/Clang on the platforms
//!   RT64 builds for, not of the C++ standard -- and it is not something this
//!   port can establish from the file text. Worse, the already-ported sibling
//!   `rt64_hlsl.h` **declares an alignment mismatch across this exact interop
//!   boundary** in its own words at lines 18-19: *"These types do not have the
//!   same alignment in HLSLPP as HLSL. We define them and auto-convert them
//!   wherever is possible."* `rt64_hlsl_interop.rs` records that quote and
//!   `rt64_extra_params.rs` refuses the shared-GPU-layout claim on its
//!   strength. This module repeats both refusals on its own account. Settling
//!   any of it would need a real shader compile, which this card refuses.
//! - **`float2` / `float3` / `uint2` / `uint`'s definitions**, which live in
//!   `src/shared/rt64_hlsl.h`. Only `typedef uint32_t uint` (that file's line
//!   16) is admitted, and it is already independently ported and cited by
//!   `rt64_hlsl_interop.rs` in this crate.
//! - **Anything depending on `hlslpp::` types.** `src/contrib/hlslpp` is an
//!   **unpopulated submodule** in the pinned checkout, so its contents are
//!   unreadable and anything requiring them is unportable. Neither of this
//!   card's two headers names `hlslpp::` directly, so nothing was lost here --
//!   but `rt64_hlsl.h`, which both headers `#include`, does define
//!   `hlslpp::`-converting constructors for the very `float2`/`float3` types
//!   `GPUTile` is built from. That is a second, independent reason the
//!   `GPUTile` layout claim is refused rather than merely deferred.
//! - **`GPUTile` as a Rust struct.** See "Reuse, not new type".
//!
//! ## Verbatim key definitions
//!
//! ```text
//! // rt64_render_flags.h lines 9-18
//! #define NATIVE_SAMPLER_NONE           0
//! #define NATIVE_SAMPLER_WRAP_WRAP      1
//! #define NATIVE_SAMPLER_WRAP_MIRROR    2
//! #define NATIVE_SAMPLER_WRAP_CLAMP     3
//! #define NATIVE_SAMPLER_MIRROR_WRAP    4
//! #define NATIVE_SAMPLER_MIRROR_MIRROR  5
//! #define NATIVE_SAMPLER_MIRROR_CLAMP   6
//! #define NATIVE_SAMPLER_CLAMP_WRAP     7
//! #define NATIVE_SAMPLER_CLAMP_MIRROR   8
//! #define NATIVE_SAMPLER_CLAMP_CLAMP    9
//!
//! // rt64_render_flags.h lines 24-49 (the HLSL_CPU bitfield view)
//! union RenderFlags {
//!     struct {
//!         uint rect : 1;
//!         uint NoN : 1;
//!         uint culling : 1;
//!         uint smoothShade : 1;
//!         uint linearFiltering : 1;
//!         uint blenderApproximation : 2;
//!         uint dynamicTiles : 1;
//!         uint canDecodeTMEM : 1;
//!         uint cms0 : 2;
//!         uint cmt0 : 2;
//!         uint cms1 : 2;
//!         uint cmt1 : 2;
//!         uint usesTexture0 : 1;
//!         uint usesTexture1 : 1;
//!         uint nativeSampler0 : 4;
//!         uint nativeSampler1 : 4;
//!         uint upscale2D : 1;
//!         uint upscaleLOD : 1;
//!         uint usesHDR : 1;
//!         uint sampleCount : 2;
//!     };
//!
//!     uint value;
//! };
//!
//! // rt64_render_flags.h lines 51-130 (the HLSL view; comment verbatim)
//! // SPIR-V code generation does not seem to like bitfields at the moment, so we work around it by querying the flags manually.
//! bool renderFlagRect(uint flags) {
//!     return (flags & 0x1) != 0;
//! }
//! bool renderFlagNoN(uint flags) {
//!     return ((flags >> 1) & 0x1) != 0;
//! }
//! ... (culling >> 2, smoothShade >> 3, linearFiltering >> 4)
//! uint renderBlenderApproximation(uint flags) {
//!     return (flags >> 5) & 0x3;
//! }
//! ... (dynamicTiles >> 7, canDecodeTMEM >> 8)
//! uint renderCMS0(uint flags) { return (flags >> 9) & 0x3; }
//! uint renderCMT0(uint flags) { return (flags >> 11) & 0x3; }
//! uint renderCMS1(uint flags) { return (flags >> 13) & 0x3; }
//! uint renderCMT1(uint flags) { return (flags >> 15) & 0x3; }
//! ... (usesTexture0 >> 17, usesTexture1 >> 18)
//! uint renderFlagNativeSampler0(uint flags) { return (flags >> 19) & 0xF; }
//! uint renderFlagNativeSampler1(uint flags) { return (flags >> 23) & 0xF; }
//! ... (upscale2D >> 27, upscaleLOD >> 28, usesHDR >> 29)
//! uint renderFlagSampleCount(uint flags) { return (flags >> 30) & 0x3; }
//!
//! // rt64_gpu_tile.h lines 13-24 (the HLSL_CPU bitfield view)
//! union GPUTileFlags {
//!     struct {
//!         uint alphaIsCvg : 1;
//!         uint highRes : 1;
//!         uint fromCopy : 1;
//!         uint rawTMEM : 1;
//!         uint hasMipmaps : 1;
//!         uint shiftedByHalf : 1;
//!     };
//!
//!     uint value;
//! };
//!
//! // rt64_gpu_tile.h lines 27-51 (the HLSL view)
//! typedef uint GPUTileFlags;
//! bool gpuTileFlagAlphaIsCvg(GPUTileFlags flags)    { return flags & 0x1; }
//! bool gpuTileFlagHighRes(GPUTileFlags flags)       { return flags & 0x2; }
//! bool gpuTileFlagFromCopy(GPUTileFlags flags)      { return flags & 0x4; }
//! bool gpuTileFlagRawTMEM(GPUTileFlags flags)       { return flags & 0x8; }
//! bool gpuTileFlagHasMipmaps(GPUTileFlags flags)    { return flags & 0x10; }
//! bool gpuTileFlagShiftedByHalf(GPUTileFlags flags) { return flags & 0x20; }
//!
//! // rt64_gpu_tile.h lines 53-61
//! struct GPUTile {
//!     float2 ulScale;
//!     float2 tcScale;
//!     uint2 texelShift;
//!     uint2 texelMask;
//!     uint textureIndex;
//!     float3 textureDimensions;
//!     GPUTileFlags flags;
//! };
//! ```
//!
//! ## Reuse, not new type
//!
//! - **No new vector type.** `GPUTile`'s `float3 textureDimensions` is
//!   [`fn64_render_ir::Vec3`], the workspace's HLSL `float3` equivalent
//!   already used for this purpose by `rt64_extra_params.rs`,
//!   `rt64_light_estimation.rs` and `rt64_lights_math.rs`. The workspace has
//!   **no** `Vec2`, and `rt64_texture_sampler.rs` in this crate already spells
//!   `GPUTile`'s `uint2 texelShift` / `uint2 texelMask` as plain `(u32, u32)`
//!   tuples in its `TexelMaskShift` struct. This module follows that existing
//!   spelling rather than inventing a competing `Vec2`.
//! - **No Rust `GPUTile` struct.** Two of its fields already have an owner in
//!   this crate: `rt64_texture_sampler.rs`'s `TexelMaskShift` carries
//!   `texel_mask` and `texel_shift` and consumes them in
//!   `clamp_wrap_mirror_address`, a port of `clampWrapMirrorSample`'s texel
//!   arithmetic which takes a `GPUTile` upstream. That module is another
//!   lane's exclusive path and cannot be edited from this card. Defining a
//!   parallel whole-`GPUTile` struct here would fork the crate's
//!   representation of the same upstream type while adding no behavior --
//!   the struct is pure POD with no methods, and its only unproven property
//!   (layout) is refused above. So [`GPU_TILE_FIELDS`] records the field list
//!   as data for a future owner and no struct is defined.
//! - **No new sampler/tile enum.** See "Overlap with fn64's own types".
//!
//! ## Overlap with fn64's own types
//!
//! This crate implements N64 tile handling independently from public SGI
//! documentation, so the overlap was checked before defining anything:
//!
//! - **`RenderFlags::cms0/cmt0/cms1/cmt1` (2 bits each) versus fn64's
//!   `RdpTileAddressing::cms`/`cmt`** (`rt64_texture_sampler.rs:390-398`,
//!   `i32`). These are the *same* RDP clamp/mirror/wrap mode field.
//!   `rt64_texture_sampler.rs` also already defines `G_TX_MIRROR = 1` and
//!   `G_TX_CLAMP = 2` from `rt64_f3d_defines.h:66-67`, giving the 2-bit
//!   field's meaning. **No divergence found**: RT64 packs two bits per axis
//!   per tile, and the SGI `G_TX_*` values that occupy them are 1 and 2, which
//!   fit exactly. This module therefore defines **no** clamp-mode enum and no
//!   `G_TX_*` constant -- it ports only the *packing* (which two bits of the
//!   render-flags word hold each of the four fields), which is genuinely
//!   additional and lives nowhere in fn64 today.
//! - **`NATIVE_SAMPLER_*`'s 3x3 grid.** The 9 non-`NONE` values enumerate
//!   (S-axis, T-axis) over wrap/mirror/clamp, the same three modes as
//!   `G_TX_MIRROR`/`G_TX_CLAMP` above. But these are RT64's own
//!   *host-sampler-object* selectors, not RDP hardware values: they index
//!   RT64's set of pre-created native samplers. fn64 has no equivalent, so
//!   they are ported as-is. See "Admitted domain" for the ordering they
//!   encode.
//! - **`GPUTileFlags::rawTMEM` versus fn64's `requires_raw_tmem`**
//!   (`rt64_tmem_hasher.rs:266`). Similar name, different construct: fn64's is
//!   a *predicate computed from* a load tile's size and format, while this is
//!   a stored bit in an already-built GPU tile. No overlap in definition.
//! - **`crates/fn64-render-wgpu/src/state.rs` and `src/tmem/`** were searched
//!   for `cms`/`cmt`, the sampler names, and every `GPUTileFlags` field name.
//!   `state.rs` contains none of them. `tmem/` contains only an unrelated
//!   `ReversedClampExtent` error variant. **No duplicate definition exists**,
//!   and consequently **no bit-level divergence between RT64 and fn64's
//!   independent implementation was found** -- there is nothing to compare
//!   against, because fn64 has never needed to name these bit positions.
//!   Every existing mention of `RenderFlags`/`GPUTile` in this crate
//!   (`rt64_shader_description.rs`, `raster_vs.rs`,
//!   `raw_dpc/triangle_composition.rs`, `targets/triangle_pipeline.rs`) is a
//!   *documentation citation only*; those modules pass the flags word around
//!   as an opaque `u32`. This module is the first to state the layout, and
//!   `raster_vs.rs:17`'s second-hand citation -- "`renderFlagRect`,
//!   `shared/rt64_render_flags.h:52-54`, wire bit 0" -- was checked against
//!   the real source and **is correct, line numbers included**.
//!
//! ## Admitted domain
//!
//! - **The bitfield view and the accessor view agree exactly, and that is a
//!   file-determined fact.** Summing the bitfield widths in declaration order
//!   (1,1,1,1,1,2,1,1,2,2,2,2,1,1,4,4,1,1,1,2) yields the running offsets
//!   0,1,2,3,4,5,7,8,9,11,13,15,17,18,19,23,27,28,29,30 -- and every one of
//!   those offsets is *exactly* the shift the corresponding HLSL accessor
//!   applies, with a mask whose width matches the declared bitfield width
//!   (`0x1`, `0x3`, `0xF`). The two independent representations in the same
//!   file corroborate each other. The port therefore records offsets and
//!   widths as *upstream's own stated correspondence between its two views*,
//!   not as a compiler layout claim -- and a test asserts the agreement in
//!   both directions rather than trusting one derivation.
//! - **`RenderFlags` fills all 32 bits with no gaps.** 1+1+1+1+1+2+1+1+2+2+2+
//!   2+1+1+4+4+1+1+1+2 = 32, and the last field `sampleCount` sits at offset
//!   30 with width 2, ending at 32. Unlike the sibling `rt64_extra_params.h`
//!   -- whose masks jump from `0x00800` to `0x02000`, leaving bit 12 a
//!   genuine, pinned hole -- this header has **no** hole. A test pins the
//!   exhaustive-and-disjoint property, so a future field insertion that
//!   silently shifted everything above it would fail.
//! - **`GPUTileFlags` uses only 6 of 32 bits; bits 6-31 are unassigned.**
//!   Pinned as found, not padded out. `0x3F` is the union of the six, and a
//!   test derives that union two independent ways -- by folding the six
//!   predicate masks, and by `(1 << 6) - 1` from the summed widths -- because
//!   an off-by-one in a hand-written mask union is invisible to a test that
//!   computes the expectation the same way it computes the value.
//! - **The two headers spell the same predicate differently, and the port
//!   preserves both spellings' *result* without preserving the C++ type
//!   pun.** `rt64_render_flags.h` writes `((flags >> n) & 0x1) != 0`;
//!   `rt64_gpu_tile.h` writes bare `flags & 0x20` returned as `bool`, relying
//!   on C++'s implicit `uint`-to-`bool` narrowing (nonzero becomes `true`).
//!   The two are behaviorally identical for every input. Rust has no implicit
//!   numeric-to-`bool` conversion, so the `gpuTileFlag*` ports write the
//!   explicit `!= 0` that C++ performs implicitly. This is a spelling change,
//!   not a semantic one, and it is the *only* deviation in the file; a test
//!   pins that both spellings agree across all 64 low-6-bit inputs.
//! - **`NATIVE_SAMPLER_*`'s value ordering is
//!   `(s_mode * 3) + t_mode + 1`** for `s_mode`/`t_mode` in
//!   `{wrap: 0, mirror: 1, clamp: 2}` -- e.g. `MIRROR_CLAMP` = `1*3 + 2 + 1` =
//!   6. The `+ 1` is because `NONE = 0` precedes the grid. This is a *derived
//!   observation* about the constants as written, offered because it makes the
//!   ten values checkable at a glance; a test asserts it holds for all nine.
//!   Upstream does not state the formula, so the constants are defined
//!   literally, exactly as the macros give them, and the formula is only
//!   asserted about them.
//! - **`renderFlagSampleCount` returns a 2-bit count, not a sample count.**
//!   The name suggests a number of samples but the field is 2 bits wide, so
//!   its range is 0-3. What those four values *mean* (a literal count of 0-3,
//!   or an index into {1,2,4,8}, or something else) is decided by shader code
//!   in files this card does not cite. The port returns the raw 2-bit field
//!   and claims nothing about its interpretation. Open question, below.
//!
//! ## Nonclaims
//!
//! - **No `repr(C)`, size, alignment, byte-offset, bitfield-allocation or
//!   constant-buffer-layout claim is made**, for `RenderFlags`, for
//!   `GPUTileFlags`, or for `GPUTile`. No type in this module carries
//!   `repr(C)`, and no test asserts a `size_of` or `align_of`. Establishing
//!   any of it would require a real shader compile plus the C++ ABI's
//!   implementation-defined bitfield-allocation rules; `rt64_hlsl.h:18-19`
//!   states outright that the types do not share alignment across the
//!   HLSLPP/HLSL boundary. Refused, not deferred.
//! - **No claim that a Rust `u32` written by these accessors is
//!   byte-compatible with a constant buffer an RT64 shader reads.** The port
//!   claims only the shift/mask arithmetic the HLSL functions perform on a
//!   `uint` the caller already holds.
//! - **No claim about `hlslpp::`.** `src/contrib/hlslpp` is an unpopulated
//!   submodule in the pinned checkout and was not read.
//! - **No hardware-behavior claim.** Nothing here was validated against an N64
//!   or against fn64's RDP tests. It is a source-level port of RT64's own
//!   packing convention, which is a renderer's internal wire format, not
//!   silicon.
//! - **One deviation, disclosed**: the implicit `uint`-to-`bool` narrowing in
//!   the six `gpuTileFlag*` functions is written explicitly as `!= 0` in Rust
//!   (see "Admitted domain"). The behavior is identical for every input; the
//!   test that covers it is labelled as pinning the **agreement between the
//!   two spellings**, not as pinning C++'s implicit conversion. No undefined
//!   behavior was found in either header, so none is reproduced.
//! - **Unused module.** Nothing in this crate calls into it yet; it is wired
//!   into `lib.rs` as a private `mod` only, so dead-code warnings are
//!   expected.
//!
//! ## Open questions
//!
//! 1. What do `sampleCount`'s four values mean? The 2-bit width rules out a
//!    literal sample count above 3, so it is probably an index, but the
//!    mapping lives in shader code outside this card's citations.
//! 2. `RenderFlags` has no spare bits. Any future upstream field must either
//!    steal from an existing one or widen `value` past 32 bits, which would be
//!    a breaking change to every serialized shader description
//!    (`rt64_shader_description.rs` emits `rp.flags = <value>;`). Worth
//!    knowing before this layout is depended on.
//! 3. `RenderFlags` carries `cms0/cmt0/cms1/cmt1` *and*
//!    `nativeSampler0/nativeSampler1`, which encode overlapping information
//!    (the sampler selectors are a 3x3 grid over the same wrap/mirror/clamp
//!    modes). Whether upstream requires them to be consistent, or lets the
//!    native sampler override, is not determined by these two files.

use fn64_render_ir::Vec3;

// ---------------------------------------------------------------------------
// rt64_render_flags.h lines 9-18: the NATIVE_SAMPLER_* macros.
//
// RT64's selectors for its set of pre-created host sampler objects. The nine
// non-NONE values are the (S-axis, T-axis) grid over wrap/mirror/clamp, in
// row-major order with S as the outer axis. `interop::uint` is
// `typedef uint32_t uint` (rt64_hlsl.h:16), so these are u32.
// ---------------------------------------------------------------------------

/// `NATIVE_SAMPLER_NONE` (`rt64_render_flags.h:9`).
pub const NATIVE_SAMPLER_NONE: u32 = 0;
/// `NATIVE_SAMPLER_WRAP_WRAP` (`rt64_render_flags.h:10`).
pub const NATIVE_SAMPLER_WRAP_WRAP: u32 = 1;
/// `NATIVE_SAMPLER_WRAP_MIRROR` (`rt64_render_flags.h:11`).
pub const NATIVE_SAMPLER_WRAP_MIRROR: u32 = 2;
/// `NATIVE_SAMPLER_WRAP_CLAMP` (`rt64_render_flags.h:12`).
pub const NATIVE_SAMPLER_WRAP_CLAMP: u32 = 3;
/// `NATIVE_SAMPLER_MIRROR_WRAP` (`rt64_render_flags.h:13`).
pub const NATIVE_SAMPLER_MIRROR_WRAP: u32 = 4;
/// `NATIVE_SAMPLER_MIRROR_MIRROR` (`rt64_render_flags.h:14`).
pub const NATIVE_SAMPLER_MIRROR_MIRROR: u32 = 5;
/// `NATIVE_SAMPLER_MIRROR_CLAMP` (`rt64_render_flags.h:15`).
pub const NATIVE_SAMPLER_MIRROR_CLAMP: u32 = 6;
/// `NATIVE_SAMPLER_CLAMP_WRAP` (`rt64_render_flags.h:16`).
pub const NATIVE_SAMPLER_CLAMP_WRAP: u32 = 7;
/// `NATIVE_SAMPLER_CLAMP_MIRROR` (`rt64_render_flags.h:17`).
pub const NATIVE_SAMPLER_CLAMP_MIRROR: u32 = 8;
/// `NATIVE_SAMPLER_CLAMP_CLAMP` (`rt64_render_flags.h:18`).
pub const NATIVE_SAMPLER_CLAMP_CLAMP: u32 = 9;

// ---------------------------------------------------------------------------
// rt64_render_flags.h lines 24-49: the RenderFlags bitfield's declared
// field order and widths, recorded as data.
//
// This is upstream's own stated correspondence between its two views of the
// same 32 bits -- NOT a `repr(C)` or compiler-layout claim. See the module
// doc's "Admitted domain" and "Nonclaims".
// ---------------------------------------------------------------------------

/// One `RenderFlags` / `GPUTileFlags` bitfield member: its upstream name, the
/// bit offset it starts at, and its declared width in bits.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BitField {
    /// The upstream member name, spelled exactly as the header declares it.
    pub name: &'static str,
    /// Offset of the field's least significant bit within the 32-bit word.
    pub offset: u32,
    /// Declared width of the field, in bits.
    pub width: u32,
}

impl BitField {
    /// The field's mask *after* shifting down by [`Self::offset`] -- i.e. the
    /// `0x1` / `0x3` / `0xF` the corresponding HLSL accessor applies.
    ///
    /// Widths here are 1, 2 and 4, so no shift overflow is reachable.
    pub const fn value_mask(&self) -> u32 {
        (1u32 << self.width) - 1
    }

    /// The field's mask *in place* within the 32-bit word.
    pub const fn word_mask(&self) -> u32 {
        self.value_mask() << self.offset
    }

    /// Extracts this field from a flags word: `(flags >> offset) & mask`.
    pub const fn extract(&self, flags: u32) -> u32 {
        (flags >> self.offset) & self.value_mask()
    }
}

/// `RenderFlags`' 20 members in declaration order
/// (`rt64_render_flags.h:26-45`).
///
/// The widths are transcribed from the bitfield declaration; the offsets are
/// the running sum of those widths, which the header's own HLSL accessors
/// independently confirm (each accessor's shift equals the offset here, and
/// each accessor's mask equals [`BitField::value_mask`]). The 20 widths sum to
/// exactly 32, so this table is exhaustive over the word with no gaps.
pub const RENDER_FLAG_FIELDS: [BitField; 20] = [
    BitField {
        name: "rect",
        offset: 0,
        width: 1,
    },
    BitField {
        name: "NoN",
        offset: 1,
        width: 1,
    },
    BitField {
        name: "culling",
        offset: 2,
        width: 1,
    },
    BitField {
        name: "smoothShade",
        offset: 3,
        width: 1,
    },
    BitField {
        name: "linearFiltering",
        offset: 4,
        width: 1,
    },
    BitField {
        name: "blenderApproximation",
        offset: 5,
        width: 2,
    },
    BitField {
        name: "dynamicTiles",
        offset: 7,
        width: 1,
    },
    BitField {
        name: "canDecodeTMEM",
        offset: 8,
        width: 1,
    },
    BitField {
        name: "cms0",
        offset: 9,
        width: 2,
    },
    BitField {
        name: "cmt0",
        offset: 11,
        width: 2,
    },
    BitField {
        name: "cms1",
        offset: 13,
        width: 2,
    },
    BitField {
        name: "cmt1",
        offset: 15,
        width: 2,
    },
    BitField {
        name: "usesTexture0",
        offset: 17,
        width: 1,
    },
    BitField {
        name: "usesTexture1",
        offset: 18,
        width: 1,
    },
    BitField {
        name: "nativeSampler0",
        offset: 19,
        width: 4,
    },
    BitField {
        name: "nativeSampler1",
        offset: 23,
        width: 4,
    },
    BitField {
        name: "upscale2D",
        offset: 27,
        width: 1,
    },
    BitField {
        name: "upscaleLOD",
        offset: 28,
        width: 1,
    },
    BitField {
        name: "usesHDR",
        offset: 29,
        width: 1,
    },
    BitField {
        name: "sampleCount",
        offset: 30,
        width: 2,
    },
];

// ---------------------------------------------------------------------------
// rt64_render_flags.h lines 52-130: the 20 HLSL accessors.
//
// Upstream's comment on line 51, verbatim: "SPIR-V code generation does not
// seem to like bitfields at the moment, so we work around it by querying the
// flags manually."
//
// Each function below reproduces its upstream body's shift direction, shift
// amount and mask exactly. The `bool` returns keep upstream's explicit
// `!= 0`.
// ---------------------------------------------------------------------------

/// `renderFlagRect` (`rt64_render_flags.h:52-54`): `(flags & 0x1) != 0`.
///
/// Note upstream writes this one *without* a shift, unlike the other 19.
/// Equivalent to `>> 0` and reproduced in that spelling.
pub const fn render_flag_rect(flags: u32) -> bool {
    (flags & 0x1) != 0
}

/// `renderFlagNoN` (`rt64_render_flags.h:56-58`): `((flags >> 1) & 0x1) != 0`.
pub const fn render_flag_no_n(flags: u32) -> bool {
    ((flags >> 1) & 0x1) != 0
}

/// `renderFlagCulling` (`rt64_render_flags.h:60-62`):
/// `((flags >> 2) & 0x1) != 0`.
pub const fn render_flag_culling(flags: u32) -> bool {
    ((flags >> 2) & 0x1) != 0
}

/// `renderFlagSmoothShade` (`rt64_render_flags.h:64-66`):
/// `((flags >> 3) & 0x1) != 0`.
pub const fn render_flag_smooth_shade(flags: u32) -> bool {
    ((flags >> 3) & 0x1) != 0
}

/// `renderFlagLinearFiltering` (`rt64_render_flags.h:68-70`):
/// `((flags >> 4) & 0x1) != 0`.
pub const fn render_flag_linear_filtering(flags: u32) -> bool {
    ((flags >> 4) & 0x1) != 0
}

/// `renderBlenderApproximation` (`rt64_render_flags.h:72-74`):
/// `(flags >> 5) & 0x3`.
///
/// Returns the raw 2-bit field; this module claims nothing about what its
/// four values select.
pub const fn render_blender_approximation(flags: u32) -> u32 {
    (flags >> 5) & 0x3
}

/// `renderFlagDynamicTiles` (`rt64_render_flags.h:76-78`):
/// `((flags >> 7) & 0x1) != 0`.
pub const fn render_flag_dynamic_tiles(flags: u32) -> bool {
    ((flags >> 7) & 0x1) != 0
}

/// `renderFlagCanDecodeTMEM` (`rt64_render_flags.h:80-82`):
/// `((flags >> 8) & 0x1) != 0`.
pub const fn render_flag_can_decode_tmem(flags: u32) -> bool {
    ((flags >> 8) & 0x1) != 0
}

/// `renderCMS0` (`rt64_render_flags.h:84-86`): `(flags >> 9) & 0x3`.
///
/// Tile 0's S-axis clamp/mirror/wrap mode -- the same RDP field
/// `rt64_texture_sampler.rs`'s `RdpTileAddressing::cms` carries, tested there
/// against `G_TX_MIRROR = 1` / `G_TX_CLAMP = 2`.
pub const fn render_cms0(flags: u32) -> u32 {
    (flags >> 9) & 0x3
}

/// `renderCMT0` (`rt64_render_flags.h:88-90`): `(flags >> 11) & 0x3`.
pub const fn render_cmt0(flags: u32) -> u32 {
    (flags >> 11) & 0x3
}

/// `renderCMS1` (`rt64_render_flags.h:92-94`): `(flags >> 13) & 0x3`.
pub const fn render_cms1(flags: u32) -> u32 {
    (flags >> 13) & 0x3
}

/// `renderCMT1` (`rt64_render_flags.h:96-98`): `(flags >> 15) & 0x3`.
pub const fn render_cmt1(flags: u32) -> u32 {
    (flags >> 15) & 0x3
}

/// `renderFlagUsesTexture0` (`rt64_render_flags.h:100-102`):
/// `((flags >> 17) & 0x1) != 0`.
pub const fn render_flag_uses_texture0(flags: u32) -> bool {
    ((flags >> 17) & 0x1) != 0
}

/// `renderFlagUsesTexture1` (`rt64_render_flags.h:104-106`):
/// `((flags >> 18) & 0x1) != 0`.
pub const fn render_flag_uses_texture1(flags: u32) -> bool {
    ((flags >> 18) & 0x1) != 0
}

/// `renderFlagNativeSampler0` (`rt64_render_flags.h:108-110`):
/// `(flags >> 19) & 0xF`.
///
/// A `NATIVE_SAMPLER_*` selector. The 4-bit field can hold 0-15 while only
/// 0-9 are defined; upstream does not validate, and neither does this port.
pub const fn render_flag_native_sampler0(flags: u32) -> u32 {
    (flags >> 19) & 0xF
}

/// `renderFlagNativeSampler1` (`rt64_render_flags.h:112-114`):
/// `(flags >> 23) & 0xF`.
pub const fn render_flag_native_sampler1(flags: u32) -> u32 {
    (flags >> 23) & 0xF
}

/// `renderFlagUpscale2D` (`rt64_render_flags.h:116-118`):
/// `((flags >> 27) & 0x1) != 0`.
pub const fn render_flag_upscale2d(flags: u32) -> bool {
    ((flags >> 27) & 0x1) != 0
}

/// `renderFlagUpscaleLOD` (`rt64_render_flags.h:120-122`):
/// `((flags >> 28) & 0x1) != 0`.
pub const fn render_flag_upscale_lod(flags: u32) -> bool {
    ((flags >> 28) & 0x1) != 0
}

/// `renderFlagUsesHDR` (`rt64_render_flags.h:124-126`):
/// `((flags >> 29) & 0x1) != 0`.
pub const fn render_flag_uses_hdr(flags: u32) -> bool {
    ((flags >> 29) & 0x1) != 0
}

/// `renderFlagSampleCount` (`rt64_render_flags.h:128-130`):
/// `(flags >> 30) & 0x3`.
///
/// Returns the raw 2-bit field. Despite the name, the width caps the value at
/// 3; whether that is a literal count or an index is decided outside these
/// two files (module doc, open question 1).
pub const fn render_flag_sample_count(flags: u32) -> u32 {
    (flags >> 30) & 0x3
}

// ---------------------------------------------------------------------------
// rt64_gpu_tile.h lines 13-24 and 27-51: GPUTileFlags.
//
// Upstream's HLSL side is `typedef uint GPUTileFlags`, so the ports below
// take a plain u32. The six predicates use bare `flags & mask` returned as
// C++ `bool`; Rust has no implicit numeric-to-bool conversion, so the `!= 0`
// C++ performs implicitly is written explicitly. Identical for every input;
// disclosed in the module doc's "Nonclaims".
// ---------------------------------------------------------------------------

/// `GPUTileFlags`' 6 members in declaration order (`rt64_gpu_tile.h:15-20`).
///
/// All six are 1 bit wide, occupying bits 0-5. Bits 6-31 of the word are
/// assigned to no member; that is pinned as found, not padded out.
pub const GPU_TILE_FLAG_FIELDS: [BitField; 6] = [
    BitField {
        name: "alphaIsCvg",
        offset: 0,
        width: 1,
    },
    BitField {
        name: "highRes",
        offset: 1,
        width: 1,
    },
    BitField {
        name: "fromCopy",
        offset: 2,
        width: 1,
    },
    BitField {
        name: "rawTMEM",
        offset: 3,
        width: 1,
    },
    BitField {
        name: "hasMipmaps",
        offset: 4,
        width: 1,
    },
    BitField {
        name: "shiftedByHalf",
        offset: 5,
        width: 1,
    },
];

/// `gpuTileFlagAlphaIsCvg` (`rt64_gpu_tile.h:29-31`): `flags & 0x1`.
pub const fn gpu_tile_flag_alpha_is_cvg(flags: u32) -> bool {
    (flags & 0x1) != 0
}

/// `gpuTileFlagHighRes` (`rt64_gpu_tile.h:33-35`): `flags & 0x2`.
pub const fn gpu_tile_flag_high_res(flags: u32) -> bool {
    (flags & 0x2) != 0
}

/// `gpuTileFlagFromCopy` (`rt64_gpu_tile.h:37-39`): `flags & 0x4`.
pub const fn gpu_tile_flag_from_copy(flags: u32) -> bool {
    (flags & 0x4) != 0
}

/// `gpuTileFlagRawTMEM` (`rt64_gpu_tile.h:41-43`): `flags & 0x8`.
///
/// A stored bit on an already-built GPU tile. Not to be confused with this
/// crate's `rt64_tmem_hasher::requires_raw_tmem`, which *computes* a
/// same-named property from a load tile's size and format.
pub const fn gpu_tile_flag_raw_tmem(flags: u32) -> bool {
    (flags & 0x8) != 0
}

/// `gpuTileFlagHasMipmaps` (`rt64_gpu_tile.h:45-47`): `flags & 0x10`.
pub const fn gpu_tile_flag_has_mipmaps(flags: u32) -> bool {
    (flags & 0x10) != 0
}

/// `gpuTileFlagShiftedByHalf` (`rt64_gpu_tile.h:49-51`): `flags & 0x20`.
pub const fn gpu_tile_flag_shifted_by_half(flags: u32) -> bool {
    (flags & 0x20) != 0
}

// ---------------------------------------------------------------------------
// rt64_gpu_tile.h lines 53-61: struct GPUTile.
//
// Recorded as a field list rather than a Rust struct -- see the module doc's
// "Reuse, not new type". The struct is pure POD with no methods, two of its
// fields already have an owner in `rt64_texture_sampler.rs`, and its only
// unproven property (layout) is refused.
// ---------------------------------------------------------------------------

/// `GPUTile`'s 7 fields in declaration order, as `(name, upstream type)`
/// (`rt64_gpu_tile.h:54-60`).
///
/// Recorded as documentation data. Deliberately **not** a Rust struct, and
/// the order carries **no** byte-offset or constant-buffer-layout claim --
/// only the order in which the header declares them.
///
/// Rust spellings a future owner should use, per this crate's existing
/// precedent: `float2` and `uint2` as `(f32, f32)` / `(u32, u32)` tuples (as
/// `rt64_texture_sampler.rs`'s `TexelMaskShift` already does for
/// `texelShift`/`texelMask`), `float3` as [`fn64_render_ir::Vec3`], `uint` and
/// `GPUTileFlags` as `u32`.
pub const GPU_TILE_FIELDS: [(&str, &str); 7] = [
    ("ulScale", "float2"),
    ("tcScale", "float2"),
    ("texelShift", "uint2"),
    ("texelMask", "uint2"),
    ("textureIndex", "uint"),
    ("textureDimensions", "float3"),
    ("flags", "GPUTileFlags"),
];

/// The Rust type this crate uses for `GPUTile::textureDimensions`' `float3`.
///
/// Present so the reuse decision is checked by the compiler rather than only
/// asserted in prose: if [`fn64_render_ir::Vec3`] ever stopped being the
/// crate's `float3`, this alias would be the thing that has to change.
pub type GpuTileTextureDimensions = Vec3;

#[cfg(test)]
mod tests {
    use super::*;

    // -----------------------------------------------------------------
    // NATIVE_SAMPLER_* constants (rt64_render_flags.h:9-18).
    // -----------------------------------------------------------------

    /// Each of the ten macros, hand-transcribed from the header.
    #[test]
    fn native_sampler_constants_match_the_header_literally() {
        assert_eq!(NATIVE_SAMPLER_NONE, 0);
        assert_eq!(NATIVE_SAMPLER_WRAP_WRAP, 1);
        assert_eq!(NATIVE_SAMPLER_WRAP_MIRROR, 2);
        assert_eq!(NATIVE_SAMPLER_WRAP_CLAMP, 3);
        assert_eq!(NATIVE_SAMPLER_MIRROR_WRAP, 4);
        assert_eq!(NATIVE_SAMPLER_MIRROR_MIRROR, 5);
        assert_eq!(NATIVE_SAMPLER_MIRROR_CLAMP, 6);
        assert_eq!(NATIVE_SAMPLER_CLAMP_WRAP, 7);
        assert_eq!(NATIVE_SAMPLER_CLAMP_MIRROR, 8);
        assert_eq!(NATIVE_SAMPLER_CLAMP_CLAMP, 9);
    }

    /// The ten values are 0..=9 with no duplicates and no gaps -- a second,
    /// independent check on the transcription above.
    #[test]
    fn native_sampler_constants_are_a_dense_run_of_ten() {
        let all = [
            NATIVE_SAMPLER_NONE,
            NATIVE_SAMPLER_WRAP_WRAP,
            NATIVE_SAMPLER_WRAP_MIRROR,
            NATIVE_SAMPLER_WRAP_CLAMP,
            NATIVE_SAMPLER_MIRROR_WRAP,
            NATIVE_SAMPLER_MIRROR_MIRROR,
            NATIVE_SAMPLER_MIRROR_CLAMP,
            NATIVE_SAMPLER_CLAMP_WRAP,
            NATIVE_SAMPLER_CLAMP_MIRROR,
            NATIVE_SAMPLER_CLAMP_CLAMP,
        ];
        let mut sorted = all;
        sorted.sort_unstable();
        assert_eq!(sorted, [0, 1, 2, 3, 4, 5, 6, 7, 8, 9]);
        // Already in ascending order as declared.
        assert_eq!(all, sorted);
    }

    /// The nine non-NONE values form the (S, T) grid over
    /// {wrap: 0, mirror: 1, clamp: 2} at `s * 3 + t + 1`. Derived
    /// observation, asserted about the literal constants above.
    #[test]
    fn native_sampler_grid_is_s_major_over_wrap_mirror_clamp() {
        const WRAP: u32 = 0;
        const MIRROR: u32 = 1;
        const CLAMP: u32 = 2;
        let expect = |s: u32, t: u32| s * 3 + t + 1;

        assert_eq!(NATIVE_SAMPLER_WRAP_WRAP, expect(WRAP, WRAP));
        assert_eq!(NATIVE_SAMPLER_WRAP_MIRROR, expect(WRAP, MIRROR));
        assert_eq!(NATIVE_SAMPLER_WRAP_CLAMP, expect(WRAP, CLAMP));
        assert_eq!(NATIVE_SAMPLER_MIRROR_WRAP, expect(MIRROR, WRAP));
        assert_eq!(NATIVE_SAMPLER_MIRROR_MIRROR, expect(MIRROR, MIRROR));
        assert_eq!(NATIVE_SAMPLER_MIRROR_CLAMP, expect(MIRROR, CLAMP));
        assert_eq!(NATIVE_SAMPLER_CLAMP_WRAP, expect(CLAMP, WRAP));
        assert_eq!(NATIVE_SAMPLER_CLAMP_MIRROR, expect(CLAMP, MIRROR));
        assert_eq!(NATIVE_SAMPLER_CLAMP_CLAMP, expect(CLAMP, CLAMP));
    }

    /// Every defined sampler selector fits the 4-bit `nativeSampler0/1`
    /// fields that carry it. 9 <= 15, so nothing is truncated.
    #[test]
    fn every_native_sampler_value_fits_the_four_bit_field() {
        assert_eq!(NATIVE_SAMPLER_CLAMP_CLAMP, 9);
        assert!(NATIVE_SAMPLER_CLAMP_CLAMP <= 0xF);
        let word = NATIVE_SAMPLER_CLAMP_CLAMP << 19;
        assert_eq!(
            render_flag_native_sampler0(word),
            NATIVE_SAMPLER_CLAMP_CLAMP
        );
    }

    // -----------------------------------------------------------------
    // RENDER_FLAG_FIELDS: the bitfield table, checked against itself and
    // against the header's independent accessor view.
    // -----------------------------------------------------------------

    /// The 20 declared widths, hand-transcribed from
    /// `rt64_render_flags.h:26-45`, in declaration order.
    const RENDER_FLAG_WIDTHS: [u32; 20] =
        [1, 1, 1, 1, 1, 2, 1, 1, 2, 2, 2, 2, 1, 1, 4, 4, 1, 1, 1, 2];

    /// The table's widths match the hand-transcribed list.
    #[test]
    fn render_flag_widths_match_the_declaration() {
        let from_table: Vec<u32> = RENDER_FLAG_FIELDS.iter().map(|f| f.width).collect();
        assert_eq!(from_table, RENDER_FLAG_WIDTHS.to_vec());
    }

    /// Each offset is the running sum of the preceding widths -- the table's
    /// offsets recomputed from scratch rather than read back.
    #[test]
    fn render_flag_offsets_are_the_running_sum_of_widths() {
        let mut running = 0u32;
        for (i, field) in RENDER_FLAG_FIELDS.iter().enumerate() {
            assert_eq!(field.offset, running, "field {} ({})", i, field.name);
            running += field.width;
        }
        assert_eq!(running, 32, "RenderFlags must fill the 32-bit word exactly");
    }

    /// Hand-derived offsets, written out literally as a third independent
    /// statement of the same fact.
    #[test]
    fn render_flag_offsets_match_the_hand_derived_list() {
        const EXPECTED: [u32; 20] = [
            0, 1, 2, 3, 4, 5, 7, 8, 9, 11, 13, 15, 17, 18, 19, 23, 27, 28, 29, 30,
        ];
        let from_table: Vec<u32> = RENDER_FLAG_FIELDS.iter().map(|f| f.offset).collect();
        assert_eq!(from_table, EXPECTED.to_vec());
    }

    /// The 20 field names, exactly as the header spells them.
    #[test]
    fn render_flag_names_match_the_header_spelling() {
        const EXPECTED: [&str; 20] = [
            "rect",
            "NoN",
            "culling",
            "smoothShade",
            "linearFiltering",
            "blenderApproximation",
            "dynamicTiles",
            "canDecodeTMEM",
            "cms0",
            "cmt0",
            "cms1",
            "cmt1",
            "usesTexture0",
            "usesTexture1",
            "nativeSampler0",
            "nativeSampler1",
            "upscale2D",
            "upscaleLOD",
            "usesHDR",
            "sampleCount",
        ];
        let from_table: Vec<&str> = RENDER_FLAG_FIELDS.iter().map(|f| f.name).collect();
        assert_eq!(from_table, EXPECTED.to_vec());
    }

    /// The 20 in-place masks are pairwise disjoint and their union is exactly
    /// `u32::MAX` -- no overlap, no hole. Unlike the sibling
    /// `rt64_extra_params.h`, this header has no bit-12-style gap.
    ///
    /// The union is derived two independent ways: by folding the per-field
    /// masks, and from the summed widths. An off-by-one in either derivation
    /// makes them contradict.
    #[test]
    fn render_flag_masks_tile_the_word_exhaustively_and_disjointly() {
        let mut union = 0u32;
        for field in RENDER_FLAG_FIELDS.iter() {
            let m = field.word_mask();
            assert_eq!(
                union & m,
                0,
                "field {} overlaps an earlier field",
                field.name
            );
            union |= m;
        }
        assert_eq!(union, u32::MAX, "folded union");

        let total_width: u32 = RENDER_FLAG_WIDTHS.iter().sum();
        assert_eq!(total_width, 32);
        // Second derivation: 32 bits set. Computed without `1 << 32`, which
        // would overflow.
        let from_widths = u32::MAX >> (32 - total_width);
        assert_eq!(union, from_widths, "two derivations of the union disagree");
    }

    /// A few in-place masks written out by hand, so the whole table is not
    /// validated only by self-consistency.
    #[test]
    fn selected_render_flag_word_masks_are_hand_checked() {
        // rect: 1 bit at 0.
        assert_eq!(RENDER_FLAG_FIELDS[0].word_mask(), 0x0000_0001);
        // blenderApproximation: 2 bits at 5 -> 0b11 << 5 = 0x60.
        assert_eq!(RENDER_FLAG_FIELDS[5].word_mask(), 0x0000_0060);
        // cms0: 2 bits at 9 -> 0b11 << 9 = 0x600.
        assert_eq!(RENDER_FLAG_FIELDS[8].word_mask(), 0x0000_0600);
        // cmt1: 2 bits at 15 -> 0b11 << 15 = 0x1_8000.
        assert_eq!(RENDER_FLAG_FIELDS[11].word_mask(), 0x0001_8000);
        // nativeSampler0: 4 bits at 19 -> 0xF << 19 = 0x0078_0000.
        assert_eq!(RENDER_FLAG_FIELDS[14].word_mask(), 0x0078_0000);
        // nativeSampler1: 4 bits at 23 -> 0xF << 23 = 0x0780_0000.
        assert_eq!(RENDER_FLAG_FIELDS[15].word_mask(), 0x0780_0000);
        // sampleCount: 2 bits at 30 -> 0b11 << 30 = 0xC000_0000.
        assert_eq!(RENDER_FLAG_FIELDS[19].word_mask(), 0xC000_0000);
    }

    /// `value_mask` is `0x1` / `0x3` / `0xF` per width, matching the literal
    /// masks the header's accessors apply.
    #[test]
    fn render_flag_value_masks_are_one_three_or_fifteen() {
        for field in RENDER_FLAG_FIELDS.iter() {
            let expected = match field.width {
                1 => 0x1,
                2 => 0x3,
                4 => 0xF,
                w => panic!("unexpected width {} on {}", w, field.name),
            };
            assert_eq!(field.value_mask(), expected, "field {}", field.name);
        }
    }

    // -----------------------------------------------------------------
    // The bitfield view and the accessor view must agree. This is the
    // header's own internal corroboration, asserted in both directions.
    // -----------------------------------------------------------------

    /// The accessor functions, indexed to match `RENDER_FLAG_FIELDS`. Boolean
    /// accessors are adapted to `u32` so all 20 can be compared uniformly.
    fn render_accessor(index: usize, flags: u32) -> u32 {
        match index {
            0 => render_flag_rect(flags) as u32,
            1 => render_flag_no_n(flags) as u32,
            2 => render_flag_culling(flags) as u32,
            3 => render_flag_smooth_shade(flags) as u32,
            4 => render_flag_linear_filtering(flags) as u32,
            5 => render_blender_approximation(flags),
            6 => render_flag_dynamic_tiles(flags) as u32,
            7 => render_flag_can_decode_tmem(flags) as u32,
            8 => render_cms0(flags),
            9 => render_cmt0(flags),
            10 => render_cms1(flags),
            11 => render_cmt1(flags),
            12 => render_flag_uses_texture0(flags) as u32,
            13 => render_flag_uses_texture1(flags) as u32,
            14 => render_flag_native_sampler0(flags),
            15 => render_flag_native_sampler1(flags),
            16 => render_flag_upscale2d(flags) as u32,
            17 => render_flag_upscale_lod(flags) as u32,
            18 => render_flag_uses_hdr(flags) as u32,
            19 => render_flag_sample_count(flags),
            _ => unreachable!(),
        }
    }

    /// For every field, packing a saturating value at that field's offset and
    /// reading it back through the *accessor function* returns the value, and
    /// every other accessor reads zero. This is the cross-check that catches
    /// a wrong shift in any one accessor.
    #[test]
    fn each_accessor_reads_exactly_its_own_field() {
        for (i, field) in RENDER_FLAG_FIELDS.iter().enumerate() {
            let value = field.value_mask();
            let word = value << field.offset;
            for (j, other) in RENDER_FLAG_FIELDS.iter().enumerate() {
                let got = render_accessor(j, word);
                if i == j {
                    assert_eq!(got, value, "accessor {} on its own field", other.name);
                } else {
                    assert_eq!(
                        got, 0,
                        "accessor {} bled from field {}",
                        other.name, field.name
                    );
                }
            }
        }
    }

    /// The table's `extract` and the hand-written accessor functions agree on
    /// every field for a spread of adversarial words. Two independent
    /// implementations of the same quantity: if one has an off-by-one, they
    /// contradict.
    #[test]
    fn table_extract_and_accessor_functions_agree() {
        let words = [
            0x0000_0000u32,
            0xFFFF_FFFF,
            0xAAAA_AAAA,
            0x5555_5555,
            0x1234_5678,
            0xDEAD_BEEF,
            0x8000_0001,
            0x0000_FFFF,
            0xFFFF_0000,
            0x0F0F_0F0F,
        ];
        for &word in words.iter() {
            for (i, field) in RENDER_FLAG_FIELDS.iter().enumerate() {
                assert_eq!(
                    field.extract(word),
                    render_accessor(i, word),
                    "field {} on word {:#010x}",
                    field.name,
                    word
                );
            }
        }
    }

    /// Reassembling all 20 accessor results back into a word reproduces the
    /// input exactly, for every test word. Only possible if the offsets and
    /// widths are collectively exact.
    #[test]
    fn accessors_round_trip_every_bit_of_the_word() {
        let words = [
            0x0000_0000u32,
            0xFFFF_FFFF,
            0xAAAA_AAAA,
            0x5555_5555,
            0xDEAD_BEEF,
            0xCAFE_BABE,
            0x0000_0001,
            0x8000_0000,
        ];
        for &word in words.iter() {
            let mut rebuilt = 0u32;
            for (i, field) in RENDER_FLAG_FIELDS.iter().enumerate() {
                rebuilt |= render_accessor(i, word) << field.offset;
            }
            assert_eq!(rebuilt, word, "round trip of {:#010x}", word);
        }
    }

    /// Exhaustive single-bit sweep: for each of the 32 bits, exactly one
    /// field claims it, and that field's accessor is the only nonzero one.
    #[test]
    fn every_single_bit_is_claimed_by_exactly_one_field() {
        for bit in 0..32u32 {
            let word = 1u32 << bit;
            let mut claimants = 0;
            for (i, field) in RENDER_FLAG_FIELDS.iter().enumerate() {
                if render_accessor(i, word) != 0 {
                    claimants += 1;
                    assert!(
                        bit >= field.offset && bit < field.offset + field.width,
                        "bit {} read by {} which spans {}..{}",
                        bit,
                        field.name,
                        field.offset,
                        field.offset + field.width
                    );
                }
            }
            assert_eq!(claimants, 1, "bit {} claimed by {} fields", bit, claimants);
        }
    }

    // -----------------------------------------------------------------
    // Individual RenderFlags accessors, hand-derived expectations.
    // -----------------------------------------------------------------

    /// `renderFlagRect` reads bit 0. Upstream writes it without a shift.
    #[test]
    fn render_flag_rect_reads_bit_zero() {
        assert!(!render_flag_rect(0x0000_0000));
        assert!(render_flag_rect(0x0000_0001));
        assert!(!render_flag_rect(0x0000_0002));
        assert!(render_flag_rect(0xFFFF_FFFF));
        // Bit 0 set among unrelated noise.
        assert!(render_flag_rect(0xDEAD_BEEF));
    }

    /// `renderFlagNoN` reads bit 1. The odd capitalization is upstream's.
    #[test]
    fn render_flag_no_n_reads_bit_one() {
        assert!(!render_flag_no_n(0x0000_0001));
        assert!(render_flag_no_n(0x0000_0002));
        assert!(render_flag_no_n(0x0000_0003));
        assert!(!render_flag_no_n(0x0000_0004));
    }

    /// `renderFlagCulling` reads bit 2.
    #[test]
    fn render_flag_culling_reads_bit_two() {
        assert!(!render_flag_culling(0x0000_0003));
        assert!(render_flag_culling(0x0000_0004));
        assert!(!render_flag_culling(0x0000_0008));
    }

    /// `renderFlagSmoothShade` reads bit 3.
    #[test]
    fn render_flag_smooth_shade_reads_bit_three() {
        assert!(!render_flag_smooth_shade(0x0000_0007));
        assert!(render_flag_smooth_shade(0x0000_0008));
        assert!(!render_flag_smooth_shade(0x0000_0010));
    }

    /// `renderFlagLinearFiltering` reads bit 4.
    #[test]
    fn render_flag_linear_filtering_reads_bit_four() {
        assert!(!render_flag_linear_filtering(0x0000_000F));
        assert!(render_flag_linear_filtering(0x0000_0010));
        assert!(!render_flag_linear_filtering(0x0000_0020));
    }

    /// `renderBlenderApproximation` reads bits 5-6, all four values.
    /// Hand-derived: value v sits at v << 5, i.e. 0x00, 0x20, 0x40, 0x60.
    #[test]
    fn render_blender_approximation_reads_bits_five_and_six() {
        assert_eq!(render_blender_approximation(0x0000_0000), 0);
        assert_eq!(render_blender_approximation(0x0000_0020), 1);
        assert_eq!(render_blender_approximation(0x0000_0040), 2);
        assert_eq!(render_blender_approximation(0x0000_0060), 3);
        // Bit 7 (dynamicTiles) must not leak in.
        assert_eq!(render_blender_approximation(0x0000_0080), 0);
        // Bit 4 (linearFiltering) must not leak in.
        assert_eq!(render_blender_approximation(0x0000_0010), 0);
    }

    /// `renderFlagDynamicTiles` reads bit 7 -- the bit immediately above the
    /// 2-bit `blenderApproximation`, which is where an off-by-one would land.
    #[test]
    fn render_flag_dynamic_tiles_reads_bit_seven() {
        assert!(!render_flag_dynamic_tiles(0x0000_0060));
        assert!(render_flag_dynamic_tiles(0x0000_0080));
        assert!(!render_flag_dynamic_tiles(0x0000_0100));
    }

    /// `renderFlagCanDecodeTMEM` reads bit 8.
    #[test]
    fn render_flag_can_decode_tmem_reads_bit_eight() {
        assert!(!render_flag_can_decode_tmem(0x0000_0080));
        assert!(render_flag_can_decode_tmem(0x0000_0100));
        assert!(!render_flag_can_decode_tmem(0x0000_0200));
    }

    /// The four clamp-mode fields sit at 9, 11, 13, 15, two bits each, back
    /// to back with no gap. Hand-derived words: value 3 at each offset is
    /// 0x600, 0x1800, 0x6000, 0x18000.
    #[test]
    fn cms_cmt_fields_are_four_adjacent_two_bit_fields() {
        assert_eq!(render_cms0(0x0000_0600), 3);
        assert_eq!(render_cmt0(0x0000_1800), 3);
        assert_eq!(render_cms1(0x0000_6000), 3);
        assert_eq!(render_cmt1(0x0001_8000), 3);

        // Each reads zero from the others' bits.
        assert_eq!(render_cms0(0x0001_9800), 0);
        assert_eq!(render_cmt0(0x0001_6600), 0);
        assert_eq!(render_cms1(0x0001_9A00), 0);
        assert_eq!(render_cmt1(0x0000_7E00), 0);
    }

    /// Each clamp-mode field carries all four 2-bit values independently.
    /// The RDP modes that occupy them are `G_TX_MIRROR = 1` and
    /// `G_TX_CLAMP = 2` (`rt64_texture_sampler.rs:401-404`), both in range.
    #[test]
    fn each_clamp_mode_field_carries_all_four_values() {
        for v in 0..4u32 {
            assert_eq!(render_cms0(v << 9), v);
            assert_eq!(render_cmt0(v << 11), v);
            assert_eq!(render_cms1(v << 13), v);
            assert_eq!(render_cmt1(v << 15), v);
        }
        // The two SGI mode values fn64 already knows both fit.
        assert_eq!(render_cms0(1 << 9), 1); // G_TX_MIRROR
        assert_eq!(render_cms0(2 << 9), 2); // G_TX_CLAMP
    }

    /// All four clamp fields set to distinct values are read back
    /// independently. Hand-derived word: cms0=1, cmt0=2, cms1=3, cmt1=0 ->
    /// (1<<9) | (2<<11) | (3<<13) = 0x200 | 0x1000 | 0x6000 = 0x7200.
    #[test]
    fn clamp_mode_fields_are_independent() {
        let word = (1u32 << 9) | (2u32 << 11) | (3u32 << 13);
        assert_eq!(word, 0x0000_7200);
        assert_eq!(render_cms0(word), 1);
        assert_eq!(render_cmt0(word), 2);
        assert_eq!(render_cms1(word), 3);
        assert_eq!(render_cmt1(word), 0);
    }

    /// `renderFlagUsesTexture0/1` read bits 17 and 18 -- immediately above
    /// `cmt1`'s top bit (16), a likely off-by-one site.
    #[test]
    fn uses_texture_flags_read_bits_seventeen_and_eighteen() {
        assert!(render_flag_uses_texture0(0x0002_0000));
        assert!(!render_flag_uses_texture1(0x0002_0000));
        assert!(!render_flag_uses_texture0(0x0004_0000));
        assert!(render_flag_uses_texture1(0x0004_0000));
        // cmt1's top bit (16) must not be read by either.
        assert!(!render_flag_uses_texture0(0x0001_0000));
        assert!(!render_flag_uses_texture1(0x0001_0000));
    }

    /// `renderFlagNativeSampler0` reads bits 19-22 (4 bits at 19).
    /// Hand-derived: value 9 -> 9 << 19 = 0x0048_0000.
    #[test]
    fn native_sampler0_reads_four_bits_at_nineteen() {
        assert_eq!(render_flag_native_sampler0(0x0048_0000), 9);
        assert_eq!(render_flag_native_sampler0(0x0078_0000), 0xF);
        // Bit 18 (usesTexture1) below must not leak in.
        assert_eq!(render_flag_native_sampler0(0x0004_0000), 0);
        // Bit 23 (nativeSampler1's low bit) above must not leak in.
        assert_eq!(render_flag_native_sampler0(0x0080_0000), 0);
    }

    /// `renderFlagNativeSampler1` reads bits 23-26 (4 bits at 23).
    /// Hand-derived: value 9 -> 9 << 23 = 0x0480_0000.
    #[test]
    fn native_sampler1_reads_four_bits_at_twentythree() {
        assert_eq!(render_flag_native_sampler1(0x0480_0000), 9);
        assert_eq!(render_flag_native_sampler1(0x0780_0000), 0xF);
        // Bit 22 (nativeSampler0's top bit) must not leak in.
        assert_eq!(render_flag_native_sampler1(0x0040_0000), 0);
        // Bit 27 (upscale2D) must not leak in.
        assert_eq!(render_flag_native_sampler1(0x0800_0000), 0);
    }

    /// The two 4-bit sampler fields are adjacent and independent: distinct
    /// values in each are read back separately.
    #[test]
    fn the_two_native_sampler_fields_are_adjacent_and_independent() {
        let word = (NATIVE_SAMPLER_WRAP_CLAMP << 19) | (NATIVE_SAMPLER_CLAMP_WRAP << 23);
        // 3 << 19 = 0x0018_0000; 7 << 23 = 0x0380_0000.
        assert_eq!(word, 0x0398_0000);
        assert_eq!(render_flag_native_sampler0(word), NATIVE_SAMPLER_WRAP_CLAMP);
        assert_eq!(render_flag_native_sampler1(word), NATIVE_SAMPLER_CLAMP_WRAP);
    }

    /// `renderFlagUpscale2D` / `UpscaleLOD` / `UsesHDR` read bits 27, 28, 29 --
    /// three adjacent single bits above the 4-bit sampler field.
    #[test]
    fn upscale_and_hdr_flags_read_bits_twentyseven_to_twentynine() {
        assert!(render_flag_upscale2d(0x0800_0000));
        assert!(!render_flag_upscale_lod(0x0800_0000));
        assert!(!render_flag_uses_hdr(0x0800_0000));

        assert!(!render_flag_upscale2d(0x1000_0000));
        assert!(render_flag_upscale_lod(0x1000_0000));
        assert!(!render_flag_uses_hdr(0x1000_0000));

        assert!(!render_flag_upscale2d(0x2000_0000));
        assert!(!render_flag_upscale_lod(0x2000_0000));
        assert!(render_flag_uses_hdr(0x2000_0000));
    }

    /// `renderFlagSampleCount` reads the top two bits (30-31), all four
    /// values. Hand-derived: v << 30 -> 0x0, 0x4000_0000, 0x8000_0000,
    /// 0xC000_0000.
    #[test]
    fn sample_count_reads_the_top_two_bits() {
        assert_eq!(render_flag_sample_count(0x0000_0000), 0);
        assert_eq!(render_flag_sample_count(0x4000_0000), 1);
        assert_eq!(render_flag_sample_count(0x8000_0000), 2);
        assert_eq!(render_flag_sample_count(0xC000_0000), 3);
        // Bit 29 (usesHDR) must not leak in.
        assert_eq!(render_flag_sample_count(0x2000_0000), 0);
        // The field is 2 bits, so it can never exceed 3.
        assert!(render_flag_sample_count(u32::MAX) <= 3);
    }

    /// An all-ones word: every boolean accessor is true and every multi-bit
    /// accessor is saturated. Catches a mask that is too narrow.
    #[test]
    fn all_ones_saturates_every_field() {
        let f = u32::MAX;
        assert!(render_flag_rect(f));
        assert!(render_flag_no_n(f));
        assert!(render_flag_culling(f));
        assert!(render_flag_smooth_shade(f));
        assert!(render_flag_linear_filtering(f));
        assert_eq!(render_blender_approximation(f), 3);
        assert!(render_flag_dynamic_tiles(f));
        assert!(render_flag_can_decode_tmem(f));
        assert_eq!(render_cms0(f), 3);
        assert_eq!(render_cmt0(f), 3);
        assert_eq!(render_cms1(f), 3);
        assert_eq!(render_cmt1(f), 3);
        assert!(render_flag_uses_texture0(f));
        assert!(render_flag_uses_texture1(f));
        assert_eq!(render_flag_native_sampler0(f), 0xF);
        assert_eq!(render_flag_native_sampler1(f), 0xF);
        assert!(render_flag_upscale2d(f));
        assert!(render_flag_upscale_lod(f));
        assert!(render_flag_uses_hdr(f));
        assert_eq!(render_flag_sample_count(f), 3);
    }

    /// An all-zeroes word: every accessor reads false / zero. Catches an
    /// inverted predicate.
    #[test]
    fn all_zeroes_clears_every_field() {
        let f = 0u32;
        assert!(!render_flag_rect(f));
        assert!(!render_flag_no_n(f));
        assert!(!render_flag_culling(f));
        assert!(!render_flag_smooth_shade(f));
        assert!(!render_flag_linear_filtering(f));
        assert_eq!(render_blender_approximation(f), 0);
        assert!(!render_flag_dynamic_tiles(f));
        assert!(!render_flag_can_decode_tmem(f));
        assert_eq!(render_cms0(f), 0);
        assert_eq!(render_cmt0(f), 0);
        assert_eq!(render_cms1(f), 0);
        assert_eq!(render_cmt1(f), 0);
        assert!(!render_flag_uses_texture0(f));
        assert!(!render_flag_uses_texture1(f));
        assert_eq!(render_flag_native_sampler0(f), 0);
        assert_eq!(render_flag_native_sampler1(f), 0);
        assert!(!render_flag_upscale2d(f));
        assert!(!render_flag_upscale_lod(f));
        assert!(!render_flag_uses_hdr(f));
        assert_eq!(render_flag_sample_count(f), 0);
    }

    /// A fully-packed realistic word, assembled field by field and read back
    /// through every accessor. Every value hand-chosen and distinct where the
    /// width allows.
    #[test]
    fn a_fully_packed_word_reads_back_field_by_field() {
        let word = 1u32              // rect         bit 0
            | (0 << 1)               // NoN          bit 1
            | (1 << 2)               // culling      bit 2
            | (1 << 3)               // smoothShade  bit 3
            | (0 << 4)               // linearFilt   bit 4
            | (2 << 5)               // blenderApprox bits 5-6
            | (1 << 7)               // dynamicTiles bit 7
            | (0 << 8)               // canDecodeTMEM bit 8
            | (1 << 9)               // cms0         bits 9-10
            | (2 << 11)              // cmt0         bits 11-12
            | (3 << 13)              // cms1         bits 13-14
            | (0 << 15)              // cmt1         bits 15-16
            | (1 << 17)              // usesTexture0 bit 17
            | (1 << 18)              // usesTexture1 bit 18
            | (9 << 19)              // nativeSampler0 bits 19-22
            | (5 << 23)              // nativeSampler1 bits 23-26
            | (1 << 27)              // upscale2D    bit 27
            | (0 << 28)              // upscaleLOD   bit 28
            | (1 << 29)              // usesHDR      bit 29
            | (2 << 30); // sampleCount  bits 30-31

        assert!(render_flag_rect(word));
        assert!(!render_flag_no_n(word));
        assert!(render_flag_culling(word));
        assert!(render_flag_smooth_shade(word));
        assert!(!render_flag_linear_filtering(word));
        assert_eq!(render_blender_approximation(word), 2);
        assert!(render_flag_dynamic_tiles(word));
        assert!(!render_flag_can_decode_tmem(word));
        assert_eq!(render_cms0(word), 1);
        assert_eq!(render_cmt0(word), 2);
        assert_eq!(render_cms1(word), 3);
        assert_eq!(render_cmt1(word), 0);
        assert!(render_flag_uses_texture0(word));
        assert!(render_flag_uses_texture1(word));
        assert_eq!(
            render_flag_native_sampler0(word),
            NATIVE_SAMPLER_CLAMP_CLAMP
        );
        assert_eq!(
            render_flag_native_sampler1(word),
            NATIVE_SAMPLER_MIRROR_MIRROR
        );
        assert!(render_flag_upscale2d(word));
        assert!(!render_flag_upscale_lod(word));
        assert!(render_flag_uses_hdr(word));
        assert_eq!(render_flag_sample_count(word), 2);
    }

    // -----------------------------------------------------------------
    // GPUTileFlags (rt64_gpu_tile.h).
    // -----------------------------------------------------------------

    /// The six field names and offsets, hand-transcribed.
    #[test]
    fn gpu_tile_flag_table_matches_the_header() {
        const EXPECTED: [(&str, u32); 6] = [
            ("alphaIsCvg", 0),
            ("highRes", 1),
            ("fromCopy", 2),
            ("rawTMEM", 3),
            ("hasMipmaps", 4),
            ("shiftedByHalf", 5),
        ];
        let got: Vec<(&str, u32)> = GPU_TILE_FLAG_FIELDS
            .iter()
            .map(|f| (f.name, f.offset))
            .collect();
        assert_eq!(got, EXPECTED.to_vec());
        // All six are single bits.
        for field in GPU_TILE_FLAG_FIELDS.iter() {
            assert_eq!(field.width, 1, "field {}", field.name);
        }
    }

    /// The six literal masks the header's predicates apply, hand-transcribed:
    /// 0x1, 0x2, 0x4, 0x8, 0x10, 0x20.
    #[test]
    fn gpu_tile_flag_masks_match_the_header_literals() {
        const EXPECTED: [u32; 6] = [0x1, 0x2, 0x4, 0x8, 0x10, 0x20];
        let got: Vec<u32> = GPU_TILE_FLAG_FIELDS.iter().map(|f| f.word_mask()).collect();
        assert_eq!(got, EXPECTED.to_vec());
    }

    /// The union of the six masks is `0x3F`, derived two independent ways --
    /// by folding the six literal masks, and as `(1 << 6) - 1` from the six
    /// declared widths. An off-by-one nibble in either makes them contradict.
    #[test]
    fn gpu_tile_flag_mask_union_is_derived_two_ways() {
        let folded = GPU_TILE_FLAG_FIELDS
            .iter()
            .fold(0u32, |acc, f| acc | f.word_mask());
        assert_eq!(folded, 0x3F, "folded union of the six masks");

        let total_width: u32 = GPU_TILE_FLAG_FIELDS.iter().map(|f| f.width).sum();
        assert_eq!(total_width, 6);
        let from_widths = (1u32 << total_width) - 1;
        assert_eq!(folded, from_widths, "two derivations of the union disagree");

        // Third statement: the six masks are pairwise disjoint, so the union
        // must equal their sum.
        let summed: u32 = GPU_TILE_FLAG_FIELDS.iter().map(|f| f.word_mask()).sum();
        assert_eq!(folded, summed, "masks are not pairwise disjoint");
    }

    /// Bits 6-31 are assigned to no `GPUTileFlags` member -- pinned as found.
    /// Setting only those bits leaves every predicate false.
    #[test]
    fn gpu_tile_flag_bits_six_and_above_are_unassigned() {
        let high = !0x3Fu32;
        assert_eq!(high, 0xFFFF_FFC0);
        assert!(!gpu_tile_flag_alpha_is_cvg(high));
        assert!(!gpu_tile_flag_high_res(high));
        assert!(!gpu_tile_flag_from_copy(high));
        assert!(!gpu_tile_flag_raw_tmem(high));
        assert!(!gpu_tile_flag_has_mipmaps(high));
        assert!(!gpu_tile_flag_shifted_by_half(high));
    }

    /// The six predicates, indexed to match `GPU_TILE_FLAG_FIELDS`.
    fn gpu_tile_predicate(index: usize, flags: u32) -> bool {
        match index {
            0 => gpu_tile_flag_alpha_is_cvg(flags),
            1 => gpu_tile_flag_high_res(flags),
            2 => gpu_tile_flag_from_copy(flags),
            3 => gpu_tile_flag_raw_tmem(flags),
            4 => gpu_tile_flag_has_mipmaps(flags),
            5 => gpu_tile_flag_shifted_by_half(flags),
            _ => unreachable!(),
        }
    }

    /// Each predicate reads exactly its own bit and no other.
    #[test]
    fn each_gpu_tile_predicate_reads_exactly_its_own_bit() {
        for (i, field) in GPU_TILE_FLAG_FIELDS.iter().enumerate() {
            let word = 1u32 << field.offset;
            for (j, other) in GPU_TILE_FLAG_FIELDS.iter().enumerate() {
                let got = gpu_tile_predicate(j, word);
                assert_eq!(
                    got,
                    i == j,
                    "predicate {} on bit {} ({})",
                    other.name,
                    field.offset,
                    field.name
                );
            }
        }
    }

    /// Exhaustive over all 64 low-6-bit words: each predicate equals the
    /// table's `extract != 0`. Two independent implementations, every input.
    ///
    /// This is also the test that pins the one **deviation**: upstream's
    /// predicates return `flags & mask` and rely on C++'s implicit
    /// `uint`-to-`bool` narrowing, while the Rust ports write `!= 0`
    /// explicitly. This asserts the two spellings agree on every input --
    /// it pins the *agreement*, not C++'s implicit conversion.
    #[test]
    fn gpu_tile_predicates_agree_with_the_table_over_all_sixty_four_words() {
        for word in 0..64u32 {
            for (i, field) in GPU_TILE_FLAG_FIELDS.iter().enumerate() {
                let via_table = field.extract(word) != 0;
                let via_predicate = gpu_tile_predicate(i, word);
                assert_eq!(
                    via_table, via_predicate,
                    "field {} on word {:#04x}",
                    field.name, word
                );
                // Third derivation: the raw C++ spelling, `flags & mask`
                // narrowed to bool.
                let via_raw_mask = (word & field.word_mask()) != 0;
                assert_eq!(via_raw_mask, via_predicate, "raw mask spelling disagrees");
            }
        }
    }

    /// Exhaustive round trip: every one of the 64 low words is exactly
    /// reconstructed from its six predicate results.
    #[test]
    fn gpu_tile_predicates_round_trip_all_sixty_four_words() {
        for word in 0..64u32 {
            let mut rebuilt = 0u32;
            for (i, field) in GPU_TILE_FLAG_FIELDS.iter().enumerate() {
                if gpu_tile_predicate(i, word) {
                    rebuilt |= 1u32 << field.offset;
                }
            }
            assert_eq!(rebuilt, word, "round trip of {:#04x}", word);
        }
    }

    /// Each predicate, hand-checked against its literal header mask with the
    /// adjacent bits explicitly cleared.
    #[test]
    fn gpu_tile_predicates_hand_checked_against_header_literals() {
        assert!(gpu_tile_flag_alpha_is_cvg(0x1));
        assert!(!gpu_tile_flag_alpha_is_cvg(0x2));

        assert!(gpu_tile_flag_high_res(0x2));
        assert!(!gpu_tile_flag_high_res(0x1));
        assert!(!gpu_tile_flag_high_res(0x4));

        assert!(gpu_tile_flag_from_copy(0x4));
        assert!(!gpu_tile_flag_from_copy(0x2));
        assert!(!gpu_tile_flag_from_copy(0x8));

        assert!(gpu_tile_flag_raw_tmem(0x8));
        assert!(!gpu_tile_flag_raw_tmem(0x4));
        assert!(!gpu_tile_flag_raw_tmem(0x10));

        assert!(gpu_tile_flag_has_mipmaps(0x10));
        assert!(!gpu_tile_flag_has_mipmaps(0x8));
        assert!(!gpu_tile_flag_has_mipmaps(0x20));

        assert!(gpu_tile_flag_shifted_by_half(0x20));
        assert!(!gpu_tile_flag_shifted_by_half(0x10));
        assert!(!gpu_tile_flag_shifted_by_half(0x40));
    }

    /// All six set, then all six clear. Catches an inverted predicate.
    #[test]
    fn gpu_tile_predicates_all_set_and_all_clear() {
        let all = 0x3Fu32;
        assert!(gpu_tile_flag_alpha_is_cvg(all));
        assert!(gpu_tile_flag_high_res(all));
        assert!(gpu_tile_flag_from_copy(all));
        assert!(gpu_tile_flag_raw_tmem(all));
        assert!(gpu_tile_flag_has_mipmaps(all));
        assert!(gpu_tile_flag_shifted_by_half(all));

        assert!(!gpu_tile_flag_alpha_is_cvg(0));
        assert!(!gpu_tile_flag_high_res(0));
        assert!(!gpu_tile_flag_from_copy(0));
        assert!(!gpu_tile_flag_raw_tmem(0));
        assert!(!gpu_tile_flag_has_mipmaps(0));
        assert!(!gpu_tile_flag_shifted_by_half(0));
    }

    /// `u32::MAX` sets every defined tile flag -- a high garbage word must
    /// not be read as "no flags".
    #[test]
    fn gpu_tile_predicates_on_all_ones() {
        let f = u32::MAX;
        for (i, field) in GPU_TILE_FLAG_FIELDS.iter().enumerate() {
            assert!(
                gpu_tile_predicate(i, f),
                "predicate {} on u32::MAX",
                field.name
            );
        }
    }

    // -----------------------------------------------------------------
    // GPUTile's field list.
    // -----------------------------------------------------------------

    /// The 7 fields in declaration order with their upstream types, exactly
    /// as `rt64_gpu_tile.h:54-60` gives them. Order carries no layout claim.
    #[test]
    fn gpu_tile_field_list_matches_the_header() {
        const EXPECTED: [(&str, &str); 7] = [
            ("ulScale", "float2"),
            ("tcScale", "float2"),
            ("texelShift", "uint2"),
            ("texelMask", "uint2"),
            ("textureIndex", "uint"),
            ("textureDimensions", "float3"),
            ("flags", "GPUTileFlags"),
        ];
        assert_eq!(GPU_TILE_FIELDS, EXPECTED);
    }

    /// `texelShift` precedes `texelMask` in the declaration -- worth pinning
    /// because `rt64_texture_sampler.rs`'s `TexelMaskShift` names them the
    /// other way round in its *type name* while its fields are independent.
    /// The header's order is shift-then-mask.
    #[test]
    fn gpu_tile_declares_texel_shift_before_texel_mask() {
        let shift = GPU_TILE_FIELDS.iter().position(|(n, _)| *n == "texelShift");
        let mask = GPU_TILE_FIELDS.iter().position(|(n, _)| *n == "texelMask");
        assert_eq!(shift, Some(2));
        assert_eq!(mask, Some(3));
        assert!(shift < mask);
    }

    /// The `float3` field's Rust spelling is the workspace `Vec3`, not a new
    /// type. Compiles only if the reuse holds.
    #[test]
    fn gpu_tile_texture_dimensions_reuses_the_workspace_vec3() {
        let dims: GpuTileTextureDimensions = Vec3::new(64.0, 32.0, 1.0);
        let same: Vec3 = dims;
        assert_eq!(same.x, 64.0);
        assert_eq!(same.y, 32.0);
        assert_eq!(same.z, 1.0);
    }

    // -----------------------------------------------------------------
    // Cross-header and nonclaim pins.
    // -----------------------------------------------------------------

    /// The two headers' flag words are independent: a `RenderFlags` accessor
    /// must not be usable as a `GPUTileFlags` predicate. Bit 3 is
    /// `smoothShade` in one and `rawTMEM` in the other -- same bit, unrelated
    /// meanings. Pinned so nobody conflates the two words.
    #[test]
    fn the_two_flag_words_are_independent_namespaces() {
        let word = 0x8u32;
        // Bit 3 in a RenderFlags word.
        assert!(render_flag_smooth_shade(word));
        assert!(!render_flag_can_decode_tmem(word));
        // The same bit in a GPUTileFlags word.
        assert!(gpu_tile_flag_raw_tmem(word));
        assert!(!gpu_tile_flag_has_mipmaps(word));
    }

    /// `BitField` is a port-local descriptor, not an interop type: it carries
    /// no `repr(C)` and this module asserts nothing about its size or
    /// alignment. This test states the nonclaim positively -- the accessors'
    /// behavior is defined over a plain `u32` the caller already holds.
    #[test]
    fn accessors_are_defined_over_a_plain_u32_with_no_layout_claim() {
        // Every accessor's input and output are plain integers/bools; there
        // is no struct on the boundary at all.
        let flags: u32 = 0x0000_0001;
        let _: bool = render_flag_rect(flags);
        let _: u32 = render_cms0(flags);
        let _: bool = gpu_tile_flag_alpha_is_cvg(flags);
        // The descriptor table is data about the header, not a wire type.
        assert_eq!(RENDER_FLAG_FIELDS.len(), 20);
        assert_eq!(GPU_TILE_FLAG_FIELDS.len(), 6);
        assert_eq!(GPU_TILE_FIELDS.len(), 7);
    }

    /// All accessors are `const fn`, so they are usable in const context --
    /// evaluated here at compile time, which also proves no runtime state is
    /// involved.
    #[test]
    fn accessors_evaluate_in_const_context() {
        const RECT: bool = render_flag_rect(0x1);
        const CMS0: u32 = render_cms0(0x0000_0600);
        const TILE: bool = gpu_tile_flag_shifted_by_half(0x20);
        const MASK: u32 = RENDER_FLAG_FIELDS[19].word_mask();
        assert!(RECT);
        assert_eq!(CMS0, 3);
        assert!(TILE);
        assert_eq!(MASK, 0xC000_0000);
    }
}
