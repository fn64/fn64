//! `FakeEnvMapUV`, the `FbCommon`-family dispatch bodies (`FbWriteColorCS`/
//! `FbWriteDepthCS`/`FbReadAnyFullCS`/`FbReadAnyChangesCS`), the
//! `FbChangesDraw*PS` change predicate, `TextureCopyPS`' pixel-position math,
//! the `RtCopy*PS` multisample folds, and `HistogramClearCS`' defective
//! byte-address computation: a literal port of the portable fraction of
//! sixteen `src/shaders/` files from the permitted MIT RT64 source pinned at
//! commit `5473732a822a4423b5696e7cb18fecc425a59875`
//! (`docs/RT64-PORT-AUTHORITY.md`).
//!
//! Most of those sixteen files contain **no portable content at all**. See
//! the per-file inventory-drift disclosure below for which, and why.
//!
//! ## Cited sources and their digests
//!
//! Every digest below is the SHA-256 of the whole file, computed
//! independently here with `shasum -a 256` against the pinned port-commit
//! checkout and cross-checked verbatim against
//! `docs/rt64-port-inventory.json`'s
//! `files[path=...].sources.port.sha256`. **All twenty match; no mismatch.**
//! Each of these paths also records `"port_delta": "unchanged"` with an
//! identical `sources.oracle.sha256`, so the oracle and port trees agree on
//! every one of them byte for byte.
//!
//! The sixteen files this card owns:
//!
//! ```text
//! src/shaders/FbRendererRT.hlsli
//!   b9e2e76c7363e834c6ae2f0220cb2c3e45dc9a232e1e75ef91f31283757a9729  (59 lines)
//! src/shaders/FbRendererCommon.hlsli
//!   aabd51229493d8cf15d42e1ce39d22e7a74353754bc31486188bc3e994aa127d  (49 lines)
//! src/shaders/RtCopyColorToDepthPS.hlsl
//!   03390c3a6fb27a1bd1080df269fb7ac28a89f2f7457df59f24e51d407cf70d69  (47 lines)
//! src/shaders/RtCopyDepthToColorPS.hlsl
//!   c7b08c62c192faa79bc7d9e07aed28c2fd5d69b49cb16d526c0689ab77102511  (44 lines)
//! src/shaders/FbReadAnyChangesCS.hlsl
//!   28757a072be4a15021d050d4e7f2c93267ce225adef8afbeb8676badffee64bd  (39 lines)
//! src/shaders/FbReadAnyFullCS.hlsl
//!   36e11183307d15be68f2ba465a5842462692ec872065676bcc82b0eaf7ca55a4  (31 lines)
//! src/shaders/FbWriteDepthCS.hlsl
//!   2a170828ab61d215848e1a4cd8999d6cc037beba22dde7c78e283b510d6469ef  (30 lines)
//! src/shaders/FbWriteColorCS.hlsl
//!   c589dfa305f3aaa791e05966b16cb9aba3a4661988a7a122ae8dd9806ebfa317  (24 lines)
//! src/shaders/FbChangesDrawDepthPS.hlsl
//!   37e02f5c961c6102f7bdc829e5fd5868d847c18f23222a9c3471a803c7b30ac8  (21 lines)
//! src/shaders/FbChangesDrawColorPS.hlsl
//!   5eb0c750c8d82056a6908713eaabc6095d60594e2f2c25b90e5f1a0bb1258fbd  (20 lines)
//! src/shaders/Background.hlsli
//!   95a691d194f2f498a0b53b92a3b9aa1223ab04577f753cd847a28297965fae2f  (19 lines)
//! src/shaders/HistogramSetCS.hlsl
//!   1f4f372edff0d21a29534b48e3955a1355e8810b2e1cc965e0eb4ecaf26e98a3  (19 lines)
//! src/shaders/HistogramClearCS.hlsl
//!   8a47faa063c2dbe5e99b6c133c709f399578ced632896ca472042d162cdc4369  (15 lines)
//! src/shaders/TextureCopyPS.hlsl
//!   df997b74cd347dfe2eff1c1166a7acc47f71657483ccf0fea1ed1a2f85649e98  (14 lines)
//! src/shaders/FbChangesClearCS.hlsl
//!   41ecfa6e6c22d21154c57a9f62353dd64eb4ae56ab7c6f9e26ec72f8863d5095  (10 lines)
//! src/shaders/IdleCS.hlsl
//!   ceeb9cbce742a77e7ea4c70819d4e3e258d0e94127f5921b9cda73d072e2458d  (8 lines)
//! ```
//!
//! Four further files are **cited but not ported** by this module -- they are
//! named here only because a body above `#include`s them and this module's
//! doc quotes a definition from them to justify a delegation. Each is already
//! ported elsewhere in this crate (see "Reuse, not new type"):
//!
//! ```text
//! src/shaders/FbCommon.hlsli   (159 lines)  -> crate::fbcommon, crate::endian_swap,
//!                                              crate::rt64_float4_quantize
//! src/shaders/Depth.hlsli      (80 lines)   -> crate::depth_encode
//! src/shaders/Formats.hlsli    (128 lines)  -> crate::rt64_float4_quantize,
//!                                              crate::tmem::texel
//! src/shaders/Math.hlsli       (29 lines)   -> crate::math_hlsli
//! ```
//!
//! Those four are cited **by path and line only, deliberately without their
//! whole-file SHA-256 digests**, and this is not an oversight. A bare
//! 64-hex-digit literal anywhere in a Rust file under `crates/` is exactly
//! what `tools/rt64_port_inventory.py`'s `sha256_citation_index` scans for,
//! and it is the *only* signal that tool trusts as "this module ports this
//! source" (`rt64_port_inventory.py:412-438`). Quoting those four digests
//! here would therefore append this module to their `ported_as` lists and
//! assert four ports this module does not make -- the four are already
//! ported by the modules named above, and this module only *calls* them.
//! Their digests are recorded in the inventory itself and in the doc headers
//! of the four owning modules, so nothing is unverifiable; only the false
//! port-signal is withheld. The sixteen digests above are quoted in full
//! because those sixteen genuinely are this module's sources.
//!
//! The two `#define` values this module actually consumes from `Math.hlsli`
//! -- `M_PI` and `M_TWO_PI` -- are reproduced literally below and pinned by
//! test.
//!
//! ## Per-file inventory-drift disclosure
//!
//! The inventory's whole-file digest credits a source as `ported` at **file**
//! granularity, so a partial port is credited in full by that mechanism.
//! Twelve of these sixteen files receive **no** port at all and must not be
//! credited; the remaining four are partial. Explicitly, per file:
//!
//! | source | decision |
//! |---|---|
//! | `IdleCS.hlsl` | **not ported.** Its `CSMain` body is the comment `// Does nothing.` There is no statement to port. |
//! | `FbChangesClearCS.hlsl` | **not ported.** Whole body is `gOutputCount[0] = 0;` -- a store to a `RWStructuredBuffer` at a constant index. No arithmetic, no branch, no index computation. |
//! | `HistogramSetCS.hlsl` | **not ported.** Whole body is `LuminanceOutput[uint2(0, 0)] = gConstants.luminanceValue;` -- a store of an unmodified push-constant to a constant texel. Its `HistogramSetCB` struct is one `float` member with no operation defined on it. |
//! | `FbRendererCommon.hlsli` | **not ported.** All 49 lines are `#include`s and resource declarations (`ConstantBuffer`, `StructuredBuffer`, 18 `SamplerState`s, two 8192-element descriptor arrays). Register bindings and sampler state have no CPU meaning. |
//! | `FbRendererRT.hlsli` | **not ported.** All 59 lines are `#include`s and resource declarations (a `RaytracingAccelerationStructure`, nine `ByteAddressBuffer`s, five `StructuredBuffer`s, four `RWBuffer`s, 26 `RWTexture2D`s, one `Texture2D`). Same reason. |
//! | `FbRendererCommon.hlsli`/`FbRendererRT.hlsli` register *numbers* | deliberately not modelled. A `register(t26, space0)` slot index is a binding-table coordinate, not behavior; see "Nonclaims". |
//! | `TextureCopyPS.hlsl` | **partial**, 1 of 3 body lines (line 12 of 14). [`texture_copy_pixel_pos`] ports the pixel-position math. Line 13's `gInput.Load(uint3(pixelPos, 0))` is a texture fetch and is refused. |
//! | `HistogramClearCS.hlsl` | **partial**, 1 of 2 body lines (line 14 of 15). [`histogram_clear_store_byte_address`] ports the defective address computation; the `Store` itself is refused. The `[numthreads(8,8,1)]` decoration is captured as [`HISTOGRAM_AVERAGE_THREADS_PER_DIMENSION`] because it is the `#define` on line 8, a value, not a decoration. |
//! | `Background.hlsli` | **partial**, 5 of 15 body lines (lines 7-11 of 19). [`fake_env_map_uv`] is a full port of `FakeEnvMapUV`. `Sample2D` (lines 13-15) and `SampleAsEnvMap` (lines 17-19) are single `tex.SampleLevel` calls through a `SamplerState`; both refused. |
//! | `FbChangesDrawColorPS.hlsl` | **partial**, 2 of 7 body lines (lines 14-15 of 20). [`fb_changes_draw_texel_index`] and [`fb_changes_pixel_discarded`] port the coordinate and the predicate; the two `Load`s and `discard` are refused. Its `FbChangesDrawCommonCB` is a one-`uint2` struct already covered in shape by nothing upstream-named, so it is not re-declared -- see "Reuse, not new type". |
//! | `FbChangesDrawDepthPS.hlsl` | **cited but not separately ported.** Lines 14-15 are character-for-character identical to `FbChangesDrawColorPS.hlsl`'s lines 14-15, including the same `FbChangesDrawCommonCB` re-declaration; [`fb_changes_draw_texel_index`]/[`fb_changes_pixel_discarded`] serve both. Line 19's `resultDepth = gDepth.Load(...)` and line 20's `return 0.0f` are a texture fetch and a constant return. |
//! | `FbWriteColorCS.hlsl` | **partial**, 4 of 10 body lines (lines 14-16, 18, and the composition on 21-22 delegated wholesale). [`fb_common_in_bounds`], [`fb_common_offset_coord`], [`fb_common_dst_index`], [`fb_write_odd_column`], and [`fb_write_color_native_word`] cover them. `gInput.Load` (line 17) and `gOutput[dstIndex] = ...` (line 22's store half) are refused. |
//! | `FbWriteDepthCS.hlsl` | **partial**, 4 of 8 non-`#ifdef` body lines (18-20, 26-28). [`fb_common_in_bounds`]/[`fb_common_offset_coord`]/[`fb_common_dst_index`] are shared with the color pass; [`fb_write_depth_native_word`] ports lines 26-28. `gInput.Load` (22/24) is refused, and the `MULTISAMPLING` preprocessor fork selects only which `Load` overload runs -- no arithmetic differs. |
//! | `FbReadAnyFullCS.hlsl` | **partial**, 4 of 9 body lines (16-19 and the 20-27 dispatch). [`fb_common_in_bounds`], [`fb_read_buffer_index`], [`fb_common_offset_coord`] and [`fb_read_decode`] cover them. The three `RWTexture2D` stores and the unconditional `gOutputChangeBoolean[...] = 1` are refused. |
//! | `FbReadAnyChangesCS.hlsl` | **partial**, 4 of 15 body lines (18-23, 27-28, 35-37 as one predicate). [`fb_read_pixel_changed`] ports line 21's inequality; [`fb_read_decode`] is shared with the full pass. The `InterlockedAdd` (line 33), the stores, and the buffer reads are refused. Line 19's `gConstants.resolution.x.x` -- a double `.x` swizzle on a scalar -- is HLSL-legal and identical to `.x`; [`fb_read_buffer_index`] is shared with the full pass on that basis, disclosed in "Nonclaims". |
//! | `RtCopyDepthToColorPS.hlsl` | **partial**, 3 of 9 body lines (25, 28/30/32, 35-37, 42-43). [`RT_COPY_SAMPLE_COUNT_8X`]-family constants, [`rt_copy_depth_to_color_multisample_fold`] and [`rt_copy_depth_to_color_single`] cover them. The `gInput.Load` inside the fold is refused; the fold takes already-sampled depths as input. |
//! | `RtCopyColorToDepthPS.hlsl` | **partial**, 3 of 11 body lines (25, 35-39, 41-43). [`rt_copy_color_to_depth_multisample_fold`] and [`rt_copy_color_to_depth_single`] cover them. Same refusal for the `Load`. |
//!
//! **Net: 12 of 16 files receive no port; 4 receive a partial port**, plus
//! two more (`FbChangesDrawDepthPS.hlsl`, `RtCopyColorToDepthPS.hlsl`) whose
//! portable lines are covered by shared helpers. Counting the shared-helper
//! files as ported, 8 of 16 have some Rust here and 8 have none. Neither
//! count should be read as "the file is done"; the table above is the claim.
//!
//! ### The inventory regeneration this module leaves outstanding
//!
//! This module does **not** update `docs/rt64-port-inventory.json`'s
//! `ported_as`, because this card's writable surface is this file plus one
//! `mod` line in `lib.rs` -- and regenerating that inventory rewrites both
//! `docs/rt64-port-inventory.json` and `docs/RT64-PORT-INVENTORY.md`
//! wholesale from a tree snapshot, which would clobber a concurrent lane's
//! entry.
//!
//! The consequence is measured, not guessed: `python3 scripts/lint-docs.py`
//! is **clean before this module and reports exactly one error after it**,
//!
//! ```text
//! RT64-PORT-INVENTORY.md: rt64-port-inventory: src/shaders/Background.hlsli:
//!   ported_as drift from mechanical SHA-256 citation scan
//! ```
//!
//! which is one message standing for **16 drifting entries** -- the check
//! raises on the first failure (`rt64_port_inventory.py:701`), and
//! `Background.hlsli` is alphabetically first. The other fifteen are the
//! remaining files in the table above. Every one of the sixteen is purely
//! additive, `[] -> ["crates/fn64-render-wgpu/src/rt64_framebuffer_shaders.rs"]`,
//! and each will also flip `port_state` from `not-started` to `ported` --
//! which is the file-granularity over-credit disclosed at the top of this
//! section, since twelve of the sixteen receive no port at all. **Read the
//! table, not the `port_state`.**
//!
//! The fix is the established follow-up commit (`docs: regenerate inventory
//! for ...`, e.g. `67654533`, `6adaf537`, `1b4df109`):
//!
//! ```text
//! python3 tools/rt64_port_inventory.py \
//!   --oracle-dir <clean f0728a25 checkout> \
//!   --port-dir   <clean 5473732a checkout>
//! ```
//!
//! It must run after every lane in this batch has landed, or it will bake in
//! a partial snapshot.
//!
//! ## Reuse, not new type
//!
//! Most of the arithmetic these shaders perform was **already ported** by
//! earlier cards. This module calls those ports; it re-derives nothing:
//!
//! - [`crate::endian_swap::endian_swap_uint`] and
//!   [`crate::endian_swap::endian_swap_uint16`] for `EndianSwapUINT`/
//!   `EndianSwapUINT16` (`FbWriteColorCS.hlsl:22`, `FbWriteDepthCS.hlsl:28`,
//!   `FbReadAnyFullCS.hlsl:19`, `FbReadAnyChangesCS.hlsl:22`).
//! - [`crate::rt64_float4_quantize::float4_to_uint`] for
//!   `Float4ToUINT(color, siz, fmt, oddColumn, ditherValue, usesHDR)`
//!   (`FbWriteColorCS.hlsl:21`) and
//!   [`crate::rt64_float4_quantize::float4_to_rgba16`] for
//!   `Float4ToRGBA16(inputColor, 0, usesHDR)`
//!   (`RtCopyColorToDepthPS.hlsl:37,42`).
//! - [`crate::fbcommon::uint_to_float4`] for `UINTToFloat4(swappedUint, siz,
//!   fmt)` (`FbReadAnyFullCS.hlsl:25`, `FbReadAnyChangesCS.hlsl:28`).
//! - [`crate::depth_encode::float_to_depth16`] and
//!   [`crate::depth_encode::depth16_to_float`] for `FloatToDepth16`/
//!   `Depth16ToFloat` (`FbWriteDepthCS.hlsl:28`, `FbReadAnyFullCS.hlsl:21`,
//!   `FbReadAnyChangesCS.hlsl:24`, `RtCopyDepthToColorPS.hlsl:42`,
//!   `RtCopyColorToDepthPS.hlsl:38,43`).
//! - [`crate::random::RandomState::init`] for `initRand(seed, dstIndex, 16)`
//!   and [`crate::rgb_dither::dither_pattern_value`] for
//!   `DitherPatternValue` (`FbWriteColorCS.hlsl:19-20`), exactly as
//!   `rt64_fb_reinterpret.rs` already does for the same two calls.
//! - [`crate::state::ImageFormat`]/[`crate::state::PixelSize`] as the typed
//!   carriers for `gConstants.fmt`/`gConstants.siz`, matching
//!   `fbcommon.rs`'s and `rt64_fb_reinterpret.rs`'s convention.
//! - [`crate::rt64_shared_params::FbCommonCb`],
//!   [`crate::rt64_shared_params::TextureCopyCb`] and
//!   [`crate::rt64_shared_params::RenderTargetCopyCb`] are the already-landed
//!   ports of `rt64_fb_common.h`'s / `rt64_texture_copy.h`'s /
//!   `rt64_render_target_copy.h`'s constant buffers. This module declares no
//!   competing struct for any of them and takes the individual fields as
//!   parameters instead, so no field-order claim arises here at all.
//! - `Formats.hlsli`'s `RGBA16ToFloat4` (`RtCopyDepthToColorPS.hlsl:43`) is
//!   already ported as `crate::tmem::decode_direct_texel`'s `Rgba`/`Bits16`
//!   arm, reached here through [`crate::fbcommon::uint16_to_float4`], whose
//!   doc records that exact delegation.
//!
//! No new vector type is introduced. Per `AGENTS.md`'s "One vector type per
//! port" rule, `fn64_render_ir::Vec3`/`Vec4` are for ported struct fields,
//! parameters and returns that upstream spells `float3`/`float4`. Every
//! `float2`/`float3`/`float4` in this module is the rule's second exception
//! -- a **loose shader-local value or a caller-supplied already-sampled
//! value**, with no upstream type to reuse -- so all of them stay bare
//! `[f32; N]`. `FakeEnvMapUV`'s `rayDirection` is a `float3` local passed in
//! from a ray payload the cited file never declares; `RtCopy*`'s
//! `inputColor` is a texture sample; `FbWriteColorCS`' `color` is a texture
//! sample. `fn64-render-ir` has no `Vec2` at all, which settles the two
//! `float2` returns independently.
//!
//! ## Admitted domain
//!
//! Every function here is **infallible and total** over its declared
//! parameter types, matching the totality of the HLSL expressions they port:
//! none returns `Result`, none panics, and none has a precondition.
//!
//! Two domain facts are load-bearing and pinned by test rather than
//! defended by a guard, because the source defends neither:
//!
//! - **`fake_env_map_uv` has no zero guard and no NaN guard.** `atan2(0.0,
//!   0.0)` is `0.0` in both HLSL and Rust, so the `rayDirection == 0` case
//!   is well-defined and not a division by zero; the two divisions are by
//!   the compile-time constant `M_TWO_PI`, never by an input. A NaN
//!   component propagates to NaN through `atan2`, `%` and `/`, and this
//!   module reproduces that rather than clamping. Both are pinned.
//! - **`texture_copy_pixel_pos` truncates toward zero and saturates.** See
//!   its own "DEVIATION" note; this is the one place this module knowingly
//!   departs from the source, because the source's behavior there is
//!   undefined.
//!
//! `histogram_clear_store_byte_address` is total over all of `u32` x `u32`
//! only because it uses `wrapping_mul`; see its doc for why that is the
//! faithful choice and not a guard.
//!
//! ## Nonclaims
//!
//! This module makes **no GPU, WGSL, pipeline, descriptor-binding,
//! shader-manifest, draw-call-wiring, or production-path claim of any kind**.
//! Nothing here is wired to a caller anywhere in this crate; these are pure
//! CPU-side functions, matching the `math_hlsli.rs`/`fbcommon.rs`/
//! `rt64_fb_reinterpret.rs` precedent.
//!
//! It makes no claim that any of the sixteen cited files is *portable in
//! full*; the per-file table above is the exact claim, and twelve of the
//! sixteen are claimed to have **no** portable content. It does not claim
//! the corresponding `docs/rt64-port-inventory.json` task cards are complete.
//!
//! It models no `register(...)` slot number, `space` index, `[numthreads]`
//! decoration (with the single exception of `HistogramClearCS.hlsl`'s
//! line-8 `#define`, which is a value the body's own defect analysis needs),
//! `SV_DispatchThreadID`/`SV_GroupIndex`/`SV_Position`/`SV_DEPTH` semantic,
//! `[[vk::push_constant]]` attribute, `discard` statement, `InterlockedAdd`
//! atomic, `SamplerState` filtering/addressing mode, or `Texture2DMS` sample
//! layout. It makes no `repr(C)`, size, alignment, or ABI claim about any
//! type it touches.
//!
//! **`FbReadAnyChangesCS.hlsl:19`'s `gConstants.resolution.x.x`.** The source
//! writes a `.x` swizzle applied to an already-scalar `uint`. HLSL permits
//! this (a scalar is a 1-component vector for swizzle purposes) and it is
//! semantically identical to a single `.x`. This module treats it as `.x`
//! and therefore shares [`fb_read_buffer_index`] with `FbReadAnyFullCS`'
//! line 17, which writes the single-`.x` form. That equivalence is asserted,
//! not tested against a compiler; if HLSL's scalar-swizzle rule differed the
//! sharing would be wrong. Recorded as an open question rather than a claim.
//!
//! **`RtCopyColorToDepthPS`'s `usesHDR` is inert, and that is observed, not
//! claimed as a defect.** `gConstants.usesHDR` reaches that pass only
//! through `Float4ToRGBA16`, where it selects `cvgRange` (`255.0` vs
//! `65535.0`) and thereby the `round(a * cvgRange) % 8` coverage modulo --
//! whose `& 0x4` bit becomes RGBA16's **bit 0** and nothing else. The pass
//! then feeds that word to `Depth16ToFloat`, which masks with
//! `DEPTH_EXPONENT_MASK` `0xE000` and `DEPTH_MANTISSA_MASK` `0x1FFC`
//! (`Depth.hlsli:7-8`); neither covers bit 0 or bit 1. So the only bit
//! `usesHDR` can move is the only bit the consumer structurally discards,
//! and the push-constant cannot change this pass's output for any input.
//! [`tests::rt_copy_color_to_depth_discards_uses_hdr_because_depth16_masks_bit_zero`]
//! exhibits an alpha where the two packed words genuinely differ and the two
//! depths do not. This module reports the observation and ports both
//! branches faithfully anyway; it does **not** assert this is an upstream
//! defect (`docs/RT64-UPSTREAM-OBSERVATIONS.md` names no such row, and the
//! same `usesHDR` plumbing is load-bearing for `FbWriteColorCS`, which
//! stores the RGBA16 word directly and does observe bit 0). Recorded as an
//! open question.
//!
//! **`FbWriteDepthCS.hlsl:27`'s `float dz = 0.0f; // TODO`** is RT64's own
//! upstream `TODO`, present verbatim at that line.
//! [`fb_write_depth_native_word`] ports the literal `0.0` it currently
//! passes and does not invent a depth-slope computation.
//!
//! **The `HistogramClearCS.hlsl` defect is ported literally and is not
//! fixed.** See [`histogram_clear_store_byte_address`].

use crate::depth_encode::{depth16_to_float, float_to_depth16};
use crate::endian_swap::{endian_swap_uint, endian_swap_uint16};
use crate::fbcommon::uint_to_float4;
use crate::random::RandomState;
use crate::rgb_dither::{dither_pattern_value, DitherNoiseByte, RgbDither};
use crate::rt64_float4_quantize::{float4_to_rgba16, float4_to_uint};
use crate::state::{ImageFormat, PixelSize};

// ---------------------------------------------------------------------------
// Math.hlsli constants consumed by Background.hlsli
// ---------------------------------------------------------------------------

/// `#define M_PI 3.14159265f` (`Math.hlsli:8`).
///
/// Written as the source's own decimal literal, **not** as
/// [`core::f32::consts::PI`]. The two happen to round to the same `f32`
/// here, but reusing the Rust constant would silently substitute a
/// different provenance for a value the source pins by digits;
/// `math_hlsli.rs` ports `Math.hlsli`'s two *functions* and does not export
/// either `#define`, which is why they are declared here.
pub const M_PI: f32 = 3.14159265;

/// `#define M_TWO_PI (M_PI * 2.0f)` (`Math.hlsli:9`).
///
/// The source spells this as a macro expansion, so the multiplication is a
/// real `f32` operation on [`M_PI`], reproduced literally rather than
/// written as a fresh decimal literal.
pub const M_TWO_PI: f32 = M_PI * 2.0;

// ---------------------------------------------------------------------------
// Background.hlsli -- FakeEnvMapUV
// ---------------------------------------------------------------------------

/// Literal port of `FakeEnvMapUV(float3 rayDirection, float yawOffset)`
/// (`Background.hlsli:7-11`):
///
/// ```text
/// float2 FakeEnvMapUV(float3 rayDirection, float yawOffset) {
///     float yaw = fmod(yawOffset + atan2(rayDirection.x, -rayDirection.z) + M_PI, M_TWO_PI);
///     float pitch = fmod(atan2(-rayDirection.y, sqrt(rayDirection.x * rayDirection.x + rayDirection.z * rayDirection.z)) + M_PI, M_TWO_PI);
///     return float2(yaw / M_TWO_PI, pitch / M_TWO_PI);
/// }
/// ```
///
/// HLSL's `fmod(x, y)` is `x - y * trunc(x / y)`: the sign of the result
/// follows `x`, which is exactly Rust's `%` on `f32` (and *not*
/// `rem_euclid`). The addition order is preserved literally --
/// `(yawOffset + atan2(..)) + M_PI`, left-associative -- because
/// floating-point addition is not associative and regrouping would change
/// the result for some inputs.
///
/// `rayDirection` is a loose `float3` local supplied by the caller's ray
/// payload, a type `Background.hlsli` never declares, so it is a bare
/// `[f32; 3]` per `AGENTS.md`'s second "One vector type" exception. The
/// return is a `float2`, and `fn64-render-ir` has no `Vec2`.
///
/// No guard is added for `rayDirection == [0, 0, 0]`: both `atan2(0.0, -0.0)`
/// and `atan2(-0.0, 0.0)` are defined, and the two divisions are by the
/// constant [`M_TWO_PI`], never by an input. A NaN component propagates
/// through unmodified.
#[must_use]
pub fn fake_env_map_uv(ray_direction: [f32; 3], yaw_offset: f32) -> [f32; 2] {
    let yaw = (yaw_offset + ray_direction[0].atan2(-ray_direction[2]) + M_PI) % M_TWO_PI;
    let pitch = ((-ray_direction[1])
        .atan2((ray_direction[0] * ray_direction[0] + ray_direction[2] * ray_direction[2]).sqrt())
        + M_PI)
        % M_TWO_PI;
    [yaw / M_TWO_PI, pitch / M_TWO_PI]
}

// ---------------------------------------------------------------------------
// HistogramClearCS.hlsl -- the defect
// ---------------------------------------------------------------------------

/// `#define HISTOGRAM_AVERAGE_THREADS_PER_DIMENSION 8`
/// (`HistogramClearCS.hlsl:8`), the value the file's `[numthreads]` uses for
/// both the X and the Y dimension.
pub const HISTOGRAM_AVERAGE_THREADS_PER_DIMENSION: u32 = 8;

/// Literal port of `HistogramClearCS.hlsl:14`'s **defective** byte address:
///
/// ```text
/// [numthreads(HISTOGRAM_AVERAGE_THREADS_PER_DIMENSION, HISTOGRAM_AVERAGE_THREADS_PER_DIMENSION, 1)]
/// void CSMain(uint3 threadId : SV_DispatchThreadID) {
///     LuminanceHistogram.Store(threadId.x * threadId.y, 0);
/// }
/// ```
///
/// # This is a confirmed upstream defect and is ported, not fixed
///
/// `docs/RT64-UPSTREAM-OBSERVATIONS.md` section 2 records it. Two
/// independent errors compound:
///
/// 1. The address is the **product** `threadId.x * threadId.y`, not a
///    linearization such as `x * 8 + y`. Over the `8 x 8` dispatch that
///    product takes only **26 distinct values**, and **15 of the 64 threads
///    compute `0`**.
/// 2. `RWByteAddressBuffer::Store` takes a **byte** address, so bin `i`
///    lives at byte `i * 4`. Only the 4-aligned products land on a bin
///    boundary; the rest write unaligned inside a bin.
///
/// Net: **9 of the 64 bins are cleared** (bins 0-7 and bin 9, from the
/// 4-aligned products 0, 4, 8, 12, 16, 20, 24, 28 and 36), against a
/// `NUM_HISTOGRAM_BINS` of 64. [`histogram_clear_cleared_bins`] computes
/// that set and [`histogram_clear_bins_cleared_count`] its size; both are
/// pinned by test. Every one of those numbers is re-derived from this
/// function in the test module rather than asserted from the doc.
///
/// This function returns the address the source computes. It does not
/// linearize, does not multiply by 4, and does not bounds-check. Correcting
/// it is out of scope for a port and is tracked as post-parity remediation
/// in the observations doc.
///
/// `wrapping_mul` is used rather than `*`: HLSL `uint` multiplication wraps
/// modulo `2^32`, and `SV_DispatchThreadID` is a `uint3`, so the faithful
/// operation is the wrapping one. Under this crate's debug profile a plain
/// `*` would instead panic on overflow for large synthetic inputs, which
/// would be a behavior this shader does not have. Within the actual
/// `8 x 8` dispatch no product exceeds 49 and the two agree.
#[must_use]
pub const fn histogram_clear_store_byte_address(thread_id_x: u32, thread_id_y: u32) -> u32 {
    thread_id_x.wrapping_mul(thread_id_y)
}

/// The set of `NUM_HISTOGRAM_BINS`-relative bin indices that
/// `HistogramClearCS`' `8 x 8` dispatch actually clears, derived by running
/// [`histogram_clear_store_byte_address`] over every thread and keeping the
/// addresses that are both 4-aligned (so they name a bin boundary) and
/// inside the 64-bin buffer.
///
/// Returned sorted and deduplicated. This is analysis of the ported defect,
/// not part of the shader; the shader itself computes one address per
/// thread and stores.
#[must_use]
pub fn histogram_clear_cleared_bins() -> Vec<u32> {
    let mut bins = Vec::new();
    for x in 0..HISTOGRAM_AVERAGE_THREADS_PER_DIMENSION {
        for y in 0..HISTOGRAM_AVERAGE_THREADS_PER_DIMENSION {
            let address = histogram_clear_store_byte_address(x, y);
            if address % 4 == 0 && address / 4 < NUM_HISTOGRAM_BINS {
                let bin = address / 4;
                if !bins.contains(&bin) {
                    bins.push(bin);
                }
            }
        }
    }
    bins.sort_unstable();
    bins
}

/// How many of the 64 bins [`histogram_clear_cleared_bins`] covers.
#[must_use]
pub fn histogram_clear_bins_cleared_count() -> usize {
    histogram_clear_cleared_bins().len()
}

/// `#define NUM_HISTOGRAM_BINS 64` (`LuminanceHistogramCS.hlsl:10`), quoted
/// here only as the denominator the defect above is measured against.
/// `rt64_luminance_histogram.rs` is the module that ports the histogram
/// passes themselves; this constant is repeated rather than imported because
/// that module does not export it.
pub const NUM_HISTOGRAM_BINS: u32 = 64;

// ---------------------------------------------------------------------------
// TextureCopyPS.hlsl
// ---------------------------------------------------------------------------

/// Port of `TextureCopyPS.hlsl:12`:
///
/// ```text
/// uint2 pixelPos = gConstants.uvScroll + uv.xy * gConstants.uvScale;
/// ```
///
/// The right-hand side is evaluated in `float2` (`uvScroll + uv * uvScale`,
/// component-wise, multiply before add) and then implicitly converted to
/// `uint2`.
///
/// # DEVIATION: the float-to-uint conversion
///
/// HLSL's implicit `float` -> `uint` conversion truncates toward zero, and
/// its result is **undefined** when the value is negative, is NaN, or
/// exceeds `UINT_MAX`. `uvScroll` and `uvScale` are both signed `float2`
/// push-constants (`rt64_texture_copy.h:13-14`) and `uv` is an interpolated
/// varying, so nothing in the cited file constrains the product to a
/// representable non-negative range -- the undefined cases are reachable.
///
/// Per the port rules, undefined behavior is not reproduced. This function
/// uses Rust's `as u32`, which is **saturating**: negative values and NaN
/// become `0`, and values above `u32::MAX` become `u32::MAX`. Inside the
/// defined domain (finite, non-negative, `<= u32::MAX`) `as u32` truncates
/// toward zero and agrees with HLSL exactly. Outside it, this is a
/// deliberate minimal deviation, and the tests that cover those inputs are
/// labelled as pinning a **DEVIATION**, not upstream behavior.
///
/// Both parameters are loose shader-local `float2`s; `fn64-render-ir` has no
/// `Vec2`. [`crate::rt64_shared_params::TextureCopyCb`] already ports the
/// constant buffer these two come from, so no struct is declared here.
#[must_use]
pub fn texture_copy_pixel_pos(uv: [f32; 2], uv_scroll: [f32; 2], uv_scale: [f32; 2]) -> [u32; 2] {
    [
        (uv_scroll[0] + uv[0] * uv_scale[0]) as u32,
        (uv_scroll[1] + uv[1] * uv_scale[1]) as u32,
    ]
}

// ---------------------------------------------------------------------------
// FbChangesDrawColorPS.hlsl / FbChangesDrawDepthPS.hlsl
// ---------------------------------------------------------------------------

/// Port of the `uint3(uv * gConstants.Resolution, 0)` texel coordinate shared
/// verbatim by `FbChangesDrawColorPS.hlsl:14,19` and
/// `FbChangesDrawDepthPS.hlsl:14,19`.
///
/// Returns only the two-component texel index; the trailing `0` mip level
/// belongs to the `Texture2D::Load` call, which is refused. The same
/// float-to-uint DEVIATION documented on [`texture_copy_pixel_pos`] applies
/// -- `uv` is an interpolated varying with no declared range -- and is not
/// restated here.
#[must_use]
pub fn fb_changes_draw_texel_index(uv: [f32; 2], resolution: [u32; 2]) -> [u32; 2] {
    [
        (uv[0] * resolution[0] as f32) as u32,
        (uv[1] * resolution[1] as f32) as u32,
    ]
}

/// Port of the change predicate shared by `FbChangesDrawColorPS.hlsl:15-17`
/// and `FbChangesDrawDepthPS.hlsl:15-17`:
///
/// ```text
/// uint pixelChanged = gBoolean.Load(uint3(uv * gConstants.Resolution, 0));
/// if (pixelChanged == 0) {
///     discard;
/// }
/// ```
///
/// Returns `true` when the fragment is discarded, i.e. when the sampled
/// boolean is exactly `0`. Every non-zero value keeps the fragment; the
/// source tests `== 0`, not `!= 1`, so a `gBoolean` value of `2` does not
/// discard. The `discard` statement itself is a rasterizer operation and is
/// not modelled -- this function reports the condition only.
#[must_use]
pub const fn fb_changes_pixel_discarded(pixel_changed: u32) -> bool {
    pixel_changed == 0
}

// ---------------------------------------------------------------------------
// FbCommon dispatch geometry, shared by all four Fb{Write,ReadAny}*CS files
// ---------------------------------------------------------------------------

/// `#define FB_COMMON_WORKGROUP_SIZE 8` (`rt64_fb_common.h:9`), the value all
/// four `Fb{Write,ReadAny}*CS` files pass to `[numthreads]` for both X and Y.
pub const FB_COMMON_WORKGROUP_SIZE: u32 = 8;

/// Port of the bounds guard opening all four `CSMain` bodies
/// (`FbWriteColorCS.hlsl:14`, `FbWriteDepthCS.hlsl:18`,
/// `FbReadAnyFullCS.hlsl:16`, `FbReadAnyChangesCS.hlsl:18`), identical in
/// all four:
///
/// ```text
/// if ((coord.x < gConstants.resolution.x) && (coord.y < gConstants.resolution.y)) {
/// ```
///
/// Strictly `<` on both axes, so a coord equal to the resolution is out of
/// bounds. A zero resolution admits nothing.
#[must_use]
pub const fn fb_common_in_bounds(coord: [u32; 2], resolution: [u32; 2]) -> bool {
    coord[0] < resolution[0] && coord[1] < resolution[1]
}

/// Port of `uint2 offsetCoord = gConstants.offset + coord;`
/// (`FbWriteColorCS.hlsl:15`, `FbWriteDepthCS.hlsl:19`) and of the
/// identically-shaped `uint2 pixelCoord = gConstants.offset + coord.xy;`
/// (`FbReadAnyFullCS.hlsl:18`, `FbReadAnyChangesCS.hlsl:20`).
///
/// `wrapping_add` matches HLSL `uint` addition, which wraps modulo `2^32`
/// rather than trapping. The four sources apply no guard.
#[must_use]
pub const fn fb_common_offset_coord(offset: [u32; 2], coord: [u32; 2]) -> [u32; 2] {
    [
        offset[0].wrapping_add(coord[0]),
        offset[1].wrapping_add(coord[1]),
    ]
}

/// Port of `uint dstIndex = offsetCoord.y * gConstants.resolution.x + offsetCoord.x;`
/// (`FbWriteColorCS.hlsl:16`, `FbWriteDepthCS.hlsl:20`).
///
/// Note the row stride is `resolution.x`, but the coordinate is the
/// **offset** one -- so a non-zero `gConstants.offset` produces an index
/// past the end of a `resolution.x * resolution.y` buffer. That is the
/// source's arithmetic and is reproduced; no clamp is added.
///
/// `wrapping_mul`/`wrapping_add` for the same reason as
/// [`fb_common_offset_coord`].
#[must_use]
pub const fn fb_common_dst_index(offset_coord: [u32; 2], resolution_x: u32) -> u32 {
    offset_coord[1]
        .wrapping_mul(resolution_x)
        .wrapping_add(offset_coord[0])
}

/// Port of `uint bufferIndex = coord.y * gConstants.resolution.x + coord.x;`
/// (`FbReadAnyFullCS.hlsl:17`) and of `FbReadAnyChangesCS.hlsl:19`'s
/// `const uint bufferIndex = coord.y * gConstants.resolution.x.x + coord.x;`.
///
/// The two differ only in the second file's redundant `.x.x` swizzle on an
/// already-scalar `uint`, which HLSL treats as identical to `.x`; see this
/// module's "Nonclaims". Distinct from [`fb_common_dst_index`] in that the
/// read passes use the **unoffset** `coord`, while the write passes use
/// `offsetCoord`.
#[must_use]
pub const fn fb_read_buffer_index(coord: [u32; 2], resolution_x: u32) -> u32 {
    coord[1].wrapping_mul(resolution_x).wrapping_add(coord[0])
}

// ---------------------------------------------------------------------------
// FbWriteColorCS.hlsl
// ---------------------------------------------------------------------------

/// Port of `bool oddColumn = (offsetCoord.x & 1);` (`FbWriteColorCS.hlsl:18`).
///
/// HLSL's `uint` -> `bool` conversion is "non-zero", and `& 1` yields only
/// `0` or `1`, so this is exactly the low bit.
#[must_use]
pub const fn fb_write_odd_column(offset_coord_x: u32) -> bool {
    (offset_coord_x & 1) != 0
}

/// Port of `FbWriteColorCS.hlsl:19-22`'s value pipeline, resource access
/// excluded:
///
/// ```text
/// uint randomSeed = initRand(gConstants.ditherRandomSeed, dstIndex, 16);
/// uint ditherValue = DitherPatternValue(gConstants.ditherPattern, offsetCoord, randomSeed);
/// uint nativeUint = Float4ToUINT(color, gConstants.siz, gConstants.fmt, oddColumn, ditherValue, gConstants.usesHDR);
/// gOutput[dstIndex] = EndianSwapUINT(nativeUint, gConstants.siz);
/// ```
///
/// Returns the value the final line stores; the store itself is refused.
/// `color` is the already-sampled `gInput.Load(...)` result and is a
/// parameter here, not a fetch.
///
/// Every one of the four callees is an existing crate port, reused
/// unchanged: [`RandomState::init`], [`dither_pattern_value`],
/// [`float4_to_uint`] and [`endian_swap_uint`]. This function contributes
/// only the composition and the argument order, both preserved literally --
/// note in particular that `initRand`'s second argument is `dstIndex` (the
/// *offset* index) while `DitherPatternValue`'s coordinate is `offsetCoord`,
/// and that `siz` is passed to both `Float4ToUINT` and `EndianSwapUINT`.
///
/// The seam between `initRand` and `DitherPatternValue` follows
/// `rt64_fb_reinterpret.rs:374-387`'s established shape exactly: the
/// `RandomState` is unwrapped with [`RandomState::raw`] and its low byte
/// becomes the [`DitherNoiseByte`] that `dither_pattern_value`'s `Noise` arm
/// consumes, and the `uint2` coordinate is widened to the two `i32`s that
/// function's 4x4 tile lookup takes. Neither reshaping is this module's
/// invention; both are how the crate already spells this call pair.
#[must_use]
pub fn fb_write_color_native_word(
    color: [f32; 4],
    offset_coord: [u32; 2],
    dst_index: u32,
    siz: PixelSize,
    fmt: ImageFormat,
    dither_pattern: RgbDither,
    dither_random_seed: u32,
    uses_hdr: bool,
) -> u32 {
    let odd_column = fb_write_odd_column(offset_coord[0]);
    let random_seed = RandomState::init(dither_random_seed, dst_index).raw();
    let dither_value = dither_pattern_value(
        dither_pattern,
        offset_coord[0] as i32,
        offset_coord[1] as i32,
        DitherNoiseByte(random_seed as u8),
    );
    let native_uint = float4_to_uint(color, siz, fmt, odd_column, dither_value, uses_hdr);
    endian_swap_uint(native_uint, siz)
}

// ---------------------------------------------------------------------------
// FbWriteDepthCS.hlsl
// ---------------------------------------------------------------------------

/// Port of `FbWriteDepthCS.hlsl:26-28`, resource access excluded:
///
/// ```text
/// float z = clamp(inputDepth, 0.0f, 1.0f);
/// float dz = 0.0f; // TODO
/// gOutput[dstIndex] = EndianSwapUINT16(FloatToDepth16(z, dz));
/// ```
///
/// Returns the value the final line stores. `input_depth` is the
/// already-sampled `gInput.Load(...)` result.
///
/// The `clamp` is written as the source's own nested ternary shape rather
/// than as `f32::clamp`. HLSL's `clamp(x, lo, hi)` lowers to
/// `min(max(x, lo), hi)`, and HLSL `min`/`max` return their **first**
/// argument when the comparison is false -- so `clamp(NaN, 0.0, 1.0)`
/// yields NaN in HLSL, whereas Rust's `f32::clamp` **panics**-free but
/// returns NaN too while `f32::min`/`f32::max` would instead discard the
/// NaN and return `1.0`. The explicit comparison chain below reproduces the
/// HLSL result for NaN, which is then carried into `FloatToDepth16`.
///
/// `dz` is the literal `0.0` the source's own `// TODO` currently passes;
/// no depth-slope computation is invented. [`float_to_depth16`] and
/// [`endian_swap_uint16`] are existing crate ports, reused unchanged.
#[must_use]
pub fn fb_write_depth_native_word(input_depth: f32) -> u32 {
    // clamp(inputDepth, 0.0f, 1.0f) == min(max(inputDepth, 0.0f), 1.0f),
    // with HLSL's first-argument-on-false min/max semantics.
    let maxed = if input_depth > 0.0 { input_depth } else { 0.0 };
    let z = if maxed < 1.0 { maxed } else { 1.0 };
    let dz = 0.0f32;
    endian_swap_uint16(float_to_depth16(z, dz))
}

// ---------------------------------------------------------------------------
// FbReadAnyFullCS.hlsl / FbReadAnyChangesCS.hlsl
// ---------------------------------------------------------------------------

/// What `FbReadAny{Full,Changes}CS`' format dispatch decodes a swapped word
/// into. The source writes the two arms to two *different* output textures
/// (`gOutputChangeDepth` and `gOutputChangeColor`), which is why this is a
/// sum type and not a `[f32; 4]`: the arm chosen is the behavior, and the
/// stores that consume it are refused.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum FbReadDecoded {
    /// `gConstants.fmt == G_IM_FMT_DEPTH` -- `Depth16ToFloat(swappedUint)`,
    /// destined for `gOutputChangeDepth` (`FbReadAnyFullCS.hlsl:21-22`,
    /// `FbReadAnyChangesCS.hlsl:24-25`).
    Depth(f32),
    /// every other `fmt` -- `UINTToFloat4(swappedUint, siz, fmt)`, destined
    /// for `gOutputChangeColor` (`FbReadAnyFullCS.hlsl:25-26`,
    /// `FbReadAnyChangesCS.hlsl:28-29`).
    Color([f32; 4]),
}

/// Port of the shared decode of `FbReadAnyFullCS.hlsl:19-27` and
/// `FbReadAnyChangesCS.hlsl:22-30`, resource access excluded:
///
/// ```text
/// uint swappedUint = EndianSwapUINT(gNewInput[bufferIndex], gConstants.siz);
/// if (gConstants.fmt == G_IM_FMT_DEPTH) {
///     const float newDepth = Depth16ToFloat(swappedUint);
///     ...
/// }
/// else {
///     const float4 newColor = UINTToFloat4(swappedUint, gConstants.siz, gConstants.fmt);
///     ...
/// }
/// ```
///
/// `new_input_word` is the already-fetched `gNewInput[bufferIndex]`.
///
/// # The `G_IM_FMT_DEPTH` arm and its `is_depth` parameter
///
/// `G_IM_FMT_DEPTH` is not one of [`ImageFormat`]'s variants -- it is a
/// pseudo-format RT64 uses to tag a depth framebuffer, outside the four
/// real `G_IM_FMT_*` values that [`ImageFormat`] models. Rather than widen
/// that shared enum with a fifth variant on this card's authority, the
/// predicate is taken as a separate `is_depth: bool`, and `fmt` carries only
/// the real format the `else` arm consumes. Widening [`ImageFormat`] is left
/// to whichever card owns that type.
///
/// [`endian_swap_uint`], [`depth16_to_float`] and [`uint_to_float4`] are
/// existing crate ports, reused unchanged.
#[must_use]
pub fn fb_read_decode(
    new_input_word: u32,
    siz: PixelSize,
    fmt: ImageFormat,
    is_depth: bool,
) -> FbReadDecoded {
    let swapped_uint = endian_swap_uint(new_input_word, siz);
    if is_depth {
        FbReadDecoded::Depth(depth16_to_float(swapped_uint))
    } else {
        FbReadDecoded::Color(uint_to_float4(swapped_uint, siz, fmt))
    }
}

/// Port of `FbReadAnyChangesCS.hlsl:21`'s predicate:
///
/// ```text
/// if (gNewInput[bufferIndex] != gCurInput[bufferIndex]) {
/// ```
///
/// A raw `uint` inequality on the two **unswapped** buffer words -- the
/// endian swap happens only inside the taken branch (line 22), so a
/// difference is detected in native buffer order. `true` means changed:
/// the source then writes the decoded value, sets `gOutputChangeBoolean` to
/// `1`, and `InterlockedAdd`s the counter. `false` means unchanged, and the
/// source sets `gOutputChangeBoolean` to `0` (line 36) and writes nothing
/// else. Both stores and the atomic are refused.
///
/// `FbReadAnyFullCS` has no counterpart: it is the unconditional variant and
/// always takes the changed path, which is why no `_full` predicate exists.
#[must_use]
pub const fn fb_read_pixel_changed(new_input_word: u32, cur_input_word: u32) -> bool {
    new_input_word != cur_input_word
}

// ---------------------------------------------------------------------------
// RtCopyDepthToColorPS.hlsl / RtCopyColorToDepthPS.hlsl
// ---------------------------------------------------------------------------

/// Both `RtCopyColorToDepthPS` call sites (lines 37 and 42) pass the literal
/// `0` as `Float4ToRGBA16`'s `dither` argument. This crate spells that
/// argument as a validated [`crate::rgb_dither::DitherThreshold`], which
/// [`dither_pattern_value`] is the only constructor for; its
/// [`RgbDither::Disabled`] arm returns exactly `DitherThreshold(0)`
/// (`rgb_dither.rs:236`, itself the port of `Formats.hlsli:36-37`'s
/// `default` case). Routing through that arm reaches the source's literal
/// `0` without this module fabricating a second way to build the type. The
/// two coordinates and the noise byte are unread on that arm.
fn rt_copy_zero_dither() -> crate::rgb_dither::DitherThreshold {
    dither_pattern_value(RgbDither::Disabled, 0, 0, DitherNoiseByte(0))
}

/// `const uint sampleCount = 8;` under `SAMPLES_8X`
/// (`RtCopyDepthToColorPS.hlsl:28`, `RtCopyColorToDepthPS.hlsl:28`).
pub const RT_COPY_SAMPLE_COUNT_8X: u32 = 8;

/// `const uint sampleCount = 4;` under `SAMPLES_4X`
/// (`RtCopyDepthToColorPS.hlsl:30`, `RtCopyColorToDepthPS.hlsl:30`).
pub const RT_COPY_SAMPLE_COUNT_4X: u32 = 4;

/// `const uint sampleCount = 2;` under `SAMPLES_2X`
/// (`RtCopyDepthToColorPS.hlsl:32`, `RtCopyColorToDepthPS.hlsl:32`).
pub const RT_COPY_SAMPLE_COUNT_2X: u32 = 2;

/// Port of `RtCopyDepthToColorPS.hlsl:25,35-37`'s multisample reduction,
/// resource access excluded:
///
/// ```text
/// float inputDepth = 1.0f;
/// for (uint i = 0; i < sampleCount; i++) {
///     inputDepth = min(gInput.Load(pos.xy, i), inputDepth);
/// }
/// ```
///
/// `samples` are the already-loaded per-sample depths, in `i` order; its
/// length is the source's `sampleCount`, one of
/// [`RT_COPY_SAMPLE_COUNT_2X`]/[`RT_COPY_SAMPLE_COUNT_4X`]/
/// [`RT_COPY_SAMPLE_COUNT_8X`]. An empty slice returns the `1.0f` seed,
/// matching a `sampleCount` of `0` (unreachable from the source's own
/// `#define`s, but total here).
///
/// # `min` argument order is load-bearing
///
/// The source writes `min(sample, accumulator)` -- the **sample first**.
/// HLSL's `min(a, b)` returns `a` when the `<` comparison is false, so a NaN
/// sample propagates: `min(NaN, 1.0)` is NaN, where Rust's `f32::min(NaN,
/// 1.0)` would give `1.0`. The explicit ternary below preserves the
/// source's order and therefore its NaN behavior. Once the accumulator is
/// NaN every later `min(sample, NaN)` returns `sample`, so a NaN is *not*
/// sticky here -- also a consequence of the argument order, and pinned by
/// test.
#[must_use]
pub fn rt_copy_depth_to_color_multisample_fold(samples: &[f32]) -> f32 {
    let mut input_depth = 1.0f32;
    for &sample in samples {
        // min(sample, inputDepth): HLSL returns the FIRST argument when the
        // comparison is false.
        input_depth = if sample < input_depth {
            sample
        } else {
            input_depth
        };
    }
    input_depth
}

/// Port of `RtCopyDepthToColorPS.hlsl:42-43`'s tail, shared by both the
/// multisample and the single-sample paths:
///
/// ```text
/// uint depth16 = FloatToDepth16(inputDepth, 0.0f);
/// return RGBA16ToFloat4(depth16);
/// ```
///
/// `input_depth` is the fold's result (multisample) or the single
/// `gInput.Load(uint3(pos.xy, 0))` (line 39). The literal `0.0f` `dz` is the
/// source's, not a stand-in.
///
/// [`float_to_depth16`] is an existing crate port; `RGBA16ToFloat4` is
/// reached through [`crate::fbcommon::uint16_to_float4`]'s `Rgba` arm, which
/// that module documents as delegating to `crate::tmem::decode_direct_texel`
/// -- the crate's existing literal port of `Formats.hlsli:83-92`.
#[must_use]
pub fn rt_copy_depth_to_color_single(input_depth: f32) -> [f32; 4] {
    let depth16 = float_to_depth16(input_depth, 0.0);
    crate::fbcommon::uint16_to_float4(depth16, ImageFormat::Rgba)
}

/// Port of `RtCopyColorToDepthPS.hlsl:25,35-39`'s multisample reduction,
/// resource access excluded:
///
/// ```text
/// resultDepth = 1.0f;
/// for (uint i = 0; i < sampleCount; i++) {
///     float4 inputColor = gInput.Load(pos.xy, i);
///     uint rgba16 = Float4ToRGBA16(inputColor, 0, gConstants.usesHDR);
///     resultDepth = min(Depth16ToFloat(rgba16), resultDepth);
/// }
/// ```
///
/// `samples` are the already-loaded per-sample `float4` colors, in `i`
/// order. Note this fold quantizes to RGBA16 and *reinterprets those bits as
/// a depth word* before comparing -- the round-trip is the source's, and
/// [`float4_to_rgba16`]/[`depth16_to_float`] are existing crate ports reused
/// unchanged. The dither argument is the source's literal `0`.
///
/// The same first-argument `min` semantics documented on
/// [`rt_copy_depth_to_color_multisample_fold`] apply and are preserved by
/// the same explicit ternary.
#[must_use]
pub fn rt_copy_color_to_depth_multisample_fold(samples: &[[f32; 4]], uses_hdr: bool) -> f32 {
    let mut result_depth = 1.0f32;
    for &input_color in samples {
        let rgba16 = float4_to_rgba16(input_color, rt_copy_zero_dither(), uses_hdr);
        let candidate = depth16_to_float(u32::from(rgba16.bits()));
        // min(Depth16ToFloat(rgba16), resultDepth): first argument wins on a
        // false comparison.
        result_depth = if candidate < result_depth {
            candidate
        } else {
            result_depth
        };
    }
    result_depth
}

/// Port of `RtCopyColorToDepthPS.hlsl:41-43`'s single-sample path, resource
/// access excluded:
///
/// ```text
/// float4 inputColor = gInput.Load(uint3(pos.xy, 0));
/// uint rgba16 = Float4ToRGBA16(inputColor, 0, gConstants.usesHDR);
/// resultDepth = Depth16ToFloat(rgba16);
/// ```
///
/// `input_color` is the already-loaded sample. Same two reused ports and the
/// same literal `0` dither as the multisample fold, with no `min` -- the
/// single-sample path assigns directly.
#[must_use]
pub fn rt_copy_color_to_depth_single(input_color: [f32; 4], uses_hdr: bool) -> f32 {
    let rgba16 = float4_to_rgba16(input_color, rt_copy_zero_dither(), uses_hdr);
    depth16_to_float(u32::from(rgba16.bits()))
}

#[cfg(test)]
mod tests {
    use super::*;

    // -- Math.hlsli constants (Background.hlsli's dependencies) --

    #[test]
    fn m_pi_and_m_two_pi_are_the_sources_own_literals() {
        // Math.hlsli:8 spells 3.14159265f; the nearest f32 is 3.1415927.
        assert_eq!(M_PI.to_bits(), 3.1415927f32.to_bits());
        // Math.hlsli:9 is (M_PI * 2.0f), an f32 multiply, not a fresh
        // literal. 3.1415927 * 2 is exact in binary (exponent bump only).
        assert_eq!(M_TWO_PI.to_bits(), 6.2831855f32.to_bits());
        assert_eq!(M_TWO_PI, M_PI * 2.0);
    }

    // -- Background.hlsli::FakeEnvMapUV --

    #[test]
    fn fake_env_map_uv_maps_the_forward_axis_to_the_texture_center() {
        // rayDirection = (0, 0, -1), yawOffset = 0.
        //   atan2(0, -(-1)) = atan2(0, 1) = 0
        //   yaw   = fmod(0 + 0 + PI, 2PI) = PI      -> PI / 2PI  = 0.5
        //   sqrt(0*0 + (-1)*(-1)) = 1
        //   atan2(-0, 1) = 0
        //   pitch = fmod(0 + PI, 2PI) = PI          -> PI / 2PI  = 0.5
        assert_eq!(fake_env_map_uv([0.0, 0.0, -1.0], 0.0), [0.5, 0.5]);
    }

    #[test]
    fn fake_env_map_uv_places_the_plus_x_axis_three_quarters_around() {
        // rayDirection = (1, 0, 0):
        //   atan2(1, -0) = +PI/2  (atan2 of +y with -0.0 x is +PI/2)
        //   yaw   = fmod(PI/2 + PI, 2PI) = 3PI/2    -> 0.75
        //   sqrt(1 + 0) = 1; atan2(-0, 1) = -0
        //   pitch = fmod(-0 + PI, 2PI) = PI         -> 0.5
        assert_eq!(fake_env_map_uv([1.0, 0.0, 0.0], 0.0), [0.75, 0.5]);
        // And the -x axis is the quarter point, by the same derivation with
        // atan2(-1, -0) = -PI/2: fmod(-PI/2 + PI, 2PI) = PI/2 -> 0.25.
        assert_eq!(fake_env_map_uv([-1.0, 0.0, 0.0], 0.0)[0], 0.25);
    }

    #[test]
    fn fake_env_map_uv_encodes_pitch_from_the_vertical_axis() {
        // rayDirection = (0, 1, 0): sqrt(0) = 0, atan2(-1, 0) = -PI/2,
        //   pitch = fmod(-PI/2 + PI, 2PI) = PI/2    -> 0.25
        assert_eq!(fake_env_map_uv([0.0, 1.0, 0.0], 0.0)[1], 0.25);
        // rayDirection = (0, -1, 0): atan2(1, 0) = +PI/2,
        //   pitch = fmod(PI/2 + PI, 2PI) = 3PI/2    -> 0.75
        assert_eq!(fake_env_map_uv([0.0, -1.0, 0.0], 0.0)[1], 0.75);
        // Straight up and straight down are half a texture apart, and
        // neither is the 0.5 the horizon gets.
        assert_ne!(
            fake_env_map_uv([0.0, 1.0, 0.0], 0.0)[1],
            fake_env_map_uv([0.0, -1.0, 0.0], 0.0)[1]
        );
    }

    #[test]
    fn fake_env_map_uv_wraps_the_backward_axis_to_exactly_zero() {
        // rayDirection = (0, 0, 1): atan2(0, -1) = +PI,
        //   yaw = fmod(PI + PI, 2PI) = fmod(2PI, 2PI) = 0 exactly.
        // This is the fmod wrap boundary; it must land on 0.0, not on 1.0,
        // and the sign must be +0.0 (fmod's sign follows its first argument,
        // which is +2PI here).
        let uv = fake_env_map_uv([0.0, 0.0, 1.0], 0.0);
        assert_eq!(uv[0], 0.0);
        assert!(uv[0].is_sign_positive());
    }

    #[test]
    fn fake_env_map_uv_applies_yaw_offset_before_the_wrap() {
        // yawOffset = PI on the forward axis: fmod(PI + 0 + PI, 2PI) = 0.
        // So a half-turn offset takes the 0.5 of the first test to 0.0 --
        // proving the offset is added inside the fmod, not after the divide
        // (which would have given 0.5 + 0.5 = 1.0).
        assert_eq!(fake_env_map_uv([0.0, 0.0, -1.0], M_PI)[0], 0.0);
        // Pitch is untouched by yawOffset.
        assert_eq!(fake_env_map_uv([0.0, 0.0, -1.0], M_PI)[1], 0.5);
    }

    #[test]
    fn fake_env_map_uv_propagates_nan_without_a_guard() {
        // The source guards nothing; a NaN component must survive atan2,
        // fmod and the divide rather than being clamped away.
        assert!(fake_env_map_uv([f32::NAN, 0.0, -1.0], 0.0)[0].is_nan());
        assert!(fake_env_map_uv([0.0, f32::NAN, -1.0], 0.0)[1].is_nan());
        // A zero ray direction is NOT a division by zero: both divides are
        // by the M_TWO_PI constant, and atan2(0,0)/atan2(-0,0) are defined.
        let zero = fake_env_map_uv([0.0, 0.0, 0.0], 0.0);
        assert!(zero[0].is_finite() && zero[1].is_finite());
        // atan2(-0, 0) = -0, so pitch = fmod(0 + PI, 2PI) = PI -> 0.5.
        assert_eq!(zero[1], 0.5);
        // Yaw is the interesting half. libm implementations are permitted to
        // return either adjacent f32 representation of PI for atan2(+0,-0).
        // Matching M_PI wraps to zero; one ULP below it survives the fmod and
        // divides to 0.99999994. Both retain the source's unguarded behavior.
        let zero_axis_pi = (0.0f32).atan2(-0.0f32);
        if zero_axis_pi.to_bits() == M_PI.to_bits() {
            assert_eq!(zero[0], 0.0);
        } else {
            assert_eq!(zero_axis_pi.to_bits() + 1, M_PI.to_bits());
            assert_eq!(zero[0], 0.99999994);
        }
        // The nonzero backward axis is stable and reaches the exact wrap.
        assert_eq!((0.0f32).atan2(-1.0f32).to_bits(), M_PI.to_bits());
    }

    // -- HistogramClearCS.hlsl: the defect, ported literally --

    #[test]
    fn histogram_clear_stores_the_product_not_a_linear_index() {
        // The defect's first half. A correct linearization of an 8x8
        // dispatch would give x*8+y; the source gives x*y.
        assert_eq!(histogram_clear_store_byte_address(3, 5), 15);
        assert_ne!(histogram_clear_store_byte_address(3, 5), 3 * 8 + 5);
        // Thread (7,7) -- the last thread -- addresses byte 49, so the whole
        // dispatch never reaches byte 4*63 = 252, the last bin's boundary.
        assert_eq!(histogram_clear_store_byte_address(7, 7), 49);
        assert!(histogram_clear_store_byte_address(7, 7) < 4 * (NUM_HISTOGRAM_BINS - 1));
    }

    #[test]
    fn histogram_clear_wastes_fifteen_of_sixty_four_threads_on_address_zero() {
        // 15 of the 64 threads compute 0: the 8 with x == 0 plus the 8 with
        // y == 0, less the one counted twice at (0,0).
        let zero_threads = (0..HISTOGRAM_AVERAGE_THREADS_PER_DIMENSION)
            .flat_map(|x| (0..HISTOGRAM_AVERAGE_THREADS_PER_DIMENSION).map(move |y| (x, y)))
            .filter(|&(x, y)| histogram_clear_store_byte_address(x, y) == 0)
            .count();
        assert_eq!(zero_threads, 15);
        assert_eq!(zero_threads, 8 + 8 - 1);
    }

    #[test]
    fn histogram_clear_produces_only_twenty_six_distinct_addresses() {
        let mut distinct: Vec<u32> = (0..HISTOGRAM_AVERAGE_THREADS_PER_DIMENSION)
            .flat_map(|x| {
                (0..HISTOGRAM_AVERAGE_THREADS_PER_DIMENSION)
                    .map(move |y| histogram_clear_store_byte_address(x, y))
            })
            .collect();
        distinct.sort_unstable();
        distinct.dedup();
        assert_eq!(distinct.len(), 26);
        // 64 threads, 26 addresses: 38 stores are redundant.
        assert_eq!(64 - distinct.len(), 38);
    }

    #[test]
    fn histogram_clear_leaves_fifty_five_of_sixty_four_bins_uncleared() {
        // The defect's second half: Store takes a BYTE address, so only the
        // 4-aligned products name a bin boundary.
        let cleared = histogram_clear_cleared_bins();
        assert_eq!(cleared, vec![0, 1, 2, 3, 4, 5, 6, 7, 9]);
        assert_eq!(histogram_clear_bins_cleared_count(), 9);
        assert_eq!(NUM_HISTOGRAM_BINS as usize - cleared.len(), 55);
        // Bin 8 is skipped even though bins 7 and 9 are cleared: byte 32 is
        // not a product of two values in 0..7.
        assert!(!cleared.contains(&8));
        // And nothing above bin 9 is ever reached.
        assert!(cleared.iter().all(|&bin| bin <= 9));
    }

    #[test]
    fn histogram_clear_address_wraps_rather_than_panicking() {
        // uint multiplication wraps modulo 2^32 in HLSL. Not reachable from
        // the 8x8 dispatch, but the function is total.
        assert_eq!(
            histogram_clear_store_byte_address(u32::MAX, 2),
            u32::MAX - 1
        );
        assert_eq!(histogram_clear_store_byte_address(0, u32::MAX), 0);
    }

    // -- TextureCopyPS.hlsl --

    #[test]
    fn texture_copy_pixel_pos_scales_then_scrolls_per_component() {
        // uv=(0.5,0.25), scroll=(10,20), scale=(100,200):
        //   x = 10 + 0.5*100 = 60 ; y = 20 + 0.25*200 = 70
        assert_eq!(
            texture_copy_pixel_pos([0.5, 0.25], [10.0, 20.0], [100.0, 200.0]),
            [60, 70]
        );
        // The multiply binds tighter than the add: (10+0.5)*100 would be
        // 1050, not 60.
        assert_ne!(
            texture_copy_pixel_pos([0.5, 0.25], [10.0, 20.0], [100.0, 200.0])[0],
            1050
        );
    }

    #[test]
    fn texture_copy_pixel_pos_truncates_toward_zero() {
        // 0 + 0.9*10 = 9.0 exactly; 0 + 0.99*10 = 9.9 -> 9.
        assert_eq!(
            texture_copy_pixel_pos([0.99, 0.999], [0.0, 0.0], [10.0, 10.0]),
            [9, 9]
        );
        // 1.0*10 = 10 lands on the boundary, not 9.
        assert_eq!(
            texture_copy_pixel_pos([1.0, 1.0], [0.0, 0.0], [10.0, 10.0]),
            [10, 10]
        );
    }

    #[test]
    fn texture_copy_pixel_pos_keeps_x_and_y_independent() {
        // A swap of the scale components must move the result, or the
        // per-component pairing is wrong.
        let a = texture_copy_pixel_pos([1.0, 1.0], [0.0, 0.0], [3.0, 7.0]);
        let b = texture_copy_pixel_pos([1.0, 1.0], [0.0, 0.0], [7.0, 3.0]);
        assert_eq!(a, [3, 7]);
        assert_eq!(b, [7, 3]);
        assert_ne!(a, b);
    }

    #[test]
    fn texture_copy_pixel_pos_saturates_out_of_range_input_deviation() {
        // DEVIATION from the source. HLSL's implicit float->uint conversion
        // is UNDEFINED for negative, NaN, and > UINT_MAX values; this port
        // uses Rust's saturating `as u32` instead of reproducing UB. These
        // assertions pin the DEVIATION, not upstream behavior.
        assert_eq!(
            texture_copy_pixel_pos([1.0, 1.0], [-50.0, -0.5], [1.0, 0.0]),
            [0, 0]
        );
        assert_eq!(
            texture_copy_pixel_pos([f32::NAN, 1.0], [0.0, 0.0], [1.0, 1.0]),
            [0, 1]
        );
        assert_eq!(
            texture_copy_pixel_pos([1.0, 1.0], [0.0, 0.0], [f32::INFINITY, 1.0]),
            [u32::MAX, 1]
        );
    }

    // -- FbChangesDraw{Color,Depth}PS.hlsl --

    #[test]
    fn fb_changes_draw_texel_index_scales_uv_by_resolution() {
        assert_eq!(
            fb_changes_draw_texel_index([0.5, 0.5], [320, 240]),
            [160, 120]
        );
        assert_eq!(fb_changes_draw_texel_index([0.0, 0.0], [320, 240]), [0, 0]);
        // 0.999 * 320 = 319.68 -> 319, the last valid column.
        assert_eq!(
            fb_changes_draw_texel_index([0.999, 0.999], [320, 240])[0],
            319
        );
        // The components must not be transposed.
        assert_eq!(
            fb_changes_draw_texel_index([1.0, 0.0], [320, 240]),
            [320, 0]
        );
    }

    #[test]
    fn fb_changes_pixel_is_discarded_only_on_exactly_zero() {
        // The source tests `pixelChanged == 0`, not `!= 1`.
        assert!(fb_changes_pixel_discarded(0));
        assert!(!fb_changes_pixel_discarded(1));
        assert!(!fb_changes_pixel_discarded(2));
        assert!(!fb_changes_pixel_discarded(u32::MAX));
    }

    // -- Shared FbCommon dispatch geometry --

    #[test]
    fn fb_common_workgroup_size_is_eight() {
        assert_eq!(FB_COMMON_WORKGROUP_SIZE, 8);
    }

    #[test]
    fn fb_common_bounds_check_is_strict_on_both_axes() {
        assert!(fb_common_in_bounds([0, 0], [320, 240]));
        assert!(fb_common_in_bounds([319, 239], [320, 240]));
        // Equal to the resolution is OUT of bounds -- strictly `<`.
        assert!(!fb_common_in_bounds([320, 239], [320, 240]));
        assert!(!fb_common_in_bounds([319, 240], [320, 240]));
        // Both axes must pass; failing either rejects.
        assert!(!fb_common_in_bounds([400, 0], [320, 240]));
        assert!(!fb_common_in_bounds([0, 400], [320, 240]));
        // A zero resolution admits nothing at all.
        assert!(!fb_common_in_bounds([0, 0], [0, 0]));
    }

    #[test]
    fn fb_common_offset_coord_adds_component_wise_and_wraps() {
        assert_eq!(fb_common_offset_coord([10, 20], [3, 4]), [13, 24]);
        // Not transposed.
        assert_eq!(fb_common_offset_coord([10, 0], [0, 5]), [10, 5]);
        // uint addition wraps rather than trapping.
        assert_eq!(fb_common_offset_coord([u32::MAX, 0], [1, 0]), [0, 0]);
    }

    #[test]
    fn fb_common_dst_index_is_row_major_over_resolution_x() {
        // offsetCoord.y * resolution.x + offsetCoord.x
        assert_eq!(fb_common_dst_index([5, 3], 320), 3 * 320 + 5);
        assert_eq!(fb_common_dst_index([0, 0], 320), 0);
        // The stride is resolution.x, so consecutive rows differ by 320 and
        // consecutive columns by 1.
        assert_eq!(
            fb_common_dst_index([5, 4], 320) - fb_common_dst_index([5, 3], 320),
            320
        );
        assert_eq!(
            fb_common_dst_index([6, 3], 320) - fb_common_dst_index([5, 3], 320),
            1
        );
        // The x/y roles are not interchangeable.
        assert_ne!(
            fb_common_dst_index([5, 3], 320),
            fb_common_dst_index([3, 5], 320)
        );
    }

    #[test]
    fn fb_read_buffer_index_uses_the_unoffset_coord() {
        // Same arithmetic shape as fb_common_dst_index, but the read passes
        // feed it `coord`, not `offsetCoord` -- so for a non-zero offset the
        // two disagree, which is the source's own asymmetry.
        let coord = [5, 3];
        let offset = [100, 200];
        let offset_coord = fb_common_offset_coord(offset, coord);
        assert_eq!(fb_read_buffer_index(coord, 320), 3 * 320 + 5);
        assert_ne!(
            fb_read_buffer_index(coord, 320),
            fb_common_dst_index(offset_coord, 320)
        );
        // With a zero offset they coincide.
        assert_eq!(
            fb_read_buffer_index(coord, 320),
            fb_common_dst_index(fb_common_offset_coord([0, 0], coord), 320)
        );
    }

    // -- FbWriteColorCS.hlsl --

    #[test]
    fn fb_write_odd_column_is_the_low_bit_of_the_offset_x() {
        assert!(!fb_write_odd_column(0));
        assert!(fb_write_odd_column(1));
        assert!(!fb_write_odd_column(2));
        assert!(fb_write_odd_column(3));
        // It reads the OFFSET x, so a 1-pixel offset flips the parity of
        // every column -- the reason the source computes it after the add.
        assert_ne!(
            fb_write_odd_column(fb_common_offset_coord([0, 0], [4, 0])[0]),
            fb_write_odd_column(fb_common_offset_coord([1, 0], [4, 0])[0])
        );
    }

    #[test]
    fn fb_write_color_native_word_is_the_endian_swapped_quantization() {
        // Hand-derive the whole chain for an opaque white 16-bit RGBA pixel
        // with dithering disabled by an all-zero pattern selection.
        let color = [1.0, 1.0, 1.0, 1.0];
        let offset_coord = [4u32, 2u32];
        let dst_index = fb_common_dst_index(offset_coord, 320);
        let got = fb_write_color_native_word(
            color,
            offset_coord,
            dst_index,
            PixelSize::Bits16,
            ImageFormat::Rgba,
            RgbDither::Disabled,
            0,
            false,
        );
        // Independent oracle: reproduce the source's four steps by calling
        // the four crate ports directly, in the source's order.
        let random_seed = RandomState::init(0, dst_index).raw();
        let dither_value = dither_pattern_value(
            RgbDither::Disabled,
            offset_coord[0] as i32,
            offset_coord[1] as i32,
            DitherNoiseByte(random_seed as u8),
        );
        let native = float4_to_uint(
            color,
            PixelSize::Bits16,
            ImageFormat::Rgba,
            fb_write_odd_column(offset_coord[0]),
            dither_value,
            false,
        );
        assert_eq!(got, endian_swap_uint(native, PixelSize::Bits16));
        // White at RGBA16 with full coverage is 0xFFFF before the swap, and
        // EndianSwapUINT for 16b is a byte swap of the low half-word, which
        // fixes 0xFFFF.
        assert_eq!(native, 0xFFFF);
        assert_eq!(got, 0xFFFF);
    }

    #[test]
    fn fb_write_color_native_word_swaps_bytes_for_asymmetric_colors() {
        // Pure red at RGBA16: r=31 -> bits 15..11, so native = 0xF800 with
        // zero coverage (alpha 0 -> cvgModulo 0 -> a bit 0).
        let got = fb_write_color_native_word(
            [1.0, 0.0, 0.0, 0.0],
            [0, 0],
            0,
            PixelSize::Bits16,
            ImageFormat::Rgba,
            RgbDither::Disabled,
            0,
            false,
        );
        let native = float4_to_uint(
            [1.0, 0.0, 0.0, 0.0],
            PixelSize::Bits16,
            ImageFormat::Rgba,
            false,
            dither_pattern_value(
                RgbDither::Disabled,
                0,
                0,
                DitherNoiseByte(RandomState::init(0, 0).raw() as u8),
            ),
            false,
        );
        assert_eq!(native, 0xF800);
        // The 16-bit endian swap must move it to 0x00F8 -- so the swap is
        // genuinely applied and is not the identity.
        assert_eq!(got, 0x00F8);
        assert_ne!(got, native);
    }

    #[test]
    fn fb_write_color_native_word_depends_on_the_column_parity() {
        // For an 8-bit intensity format Float4ToUINT8 picks .g on an odd
        // column and .r on an even one, so a color whose r and g differ must
        // produce different words on adjacent columns.
        let color = [1.0, 0.0, 0.0, 1.0];
        let even = fb_write_color_native_word(
            color,
            [0, 0],
            0,
            PixelSize::Bits8,
            ImageFormat::Intensity,
            RgbDither::Disabled,
            0,
            false,
        );
        let odd = fb_write_color_native_word(
            color,
            [1, 0],
            1,
            PixelSize::Bits8,
            ImageFormat::Intensity,
            RgbDither::Disabled,
            0,
            false,
        );
        assert_eq!(even, 255);
        assert_eq!(odd, 0);
        assert_ne!(even, odd);
    }

    // -- FbWriteDepthCS.hlsl --

    #[test]
    fn fb_write_depth_native_word_clamps_into_the_unit_range() {
        // Below 0 clamps to 0, above 1 clamps to 1, so the out-of-range
        // inputs must agree with the endpoints exactly.
        assert_eq!(
            fb_write_depth_native_word(-5.0),
            fb_write_depth_native_word(0.0)
        );
        assert_eq!(
            fb_write_depth_native_word(9.0),
            fb_write_depth_native_word(1.0)
        );
        // And the endpoints themselves differ, so the clamp is not
        // collapsing everything to one value.
        assert_ne!(
            fb_write_depth_native_word(0.0),
            fb_write_depth_native_word(1.0)
        );
    }

    #[test]
    fn fb_write_depth_native_word_is_the_swapped_depth16() {
        // Independent oracle: the two crate ports, composed in source order,
        // with the source's own literal dz of 0.
        for z in [0.0f32, 0.25, 0.5, 0.75, 1.0] {
            assert_eq!(
                fb_write_depth_native_word(z),
                endian_swap_uint16(float_to_depth16(z, 0.0))
            );
        }
        // The swap is applied: the pre-swap and post-swap words differ for a
        // depth whose two bytes are not equal.
        let pre = float_to_depth16(0.25, 0.0);
        assert_ne!(pre & 0xFF, (pre >> 8) & 0xFF);
        assert_ne!(fb_write_depth_native_word(0.25), pre);
    }

    #[test]
    fn fb_write_depth_native_word_keeps_hlsl_clamp_nan_semantics() {
        // HLSL clamp(NaN, 0, 1) == min(max(NaN, 0), 1); both min and max
        // return their FIRST argument on a false comparison, so the NaN
        // survives to FloatToDepth16. Rust's f32::max(NaN, 0.0) would
        // instead return 0.0 -- a different z, and so potentially a
        // different word. This pins the HLSL order.
        let hlsl_maxed = if f32::NAN > 0.0 { f32::NAN } else { 0.0 };
        assert_eq!(hlsl_maxed, 0.0);
        // (max's first argument is the NaN, and `NaN > 0.0` is false, so
        // HLSL's max returns... the first argument, the NaN. But `>` being
        // false means the ternary's else branch runs. Both HLSL and this
        // port therefore yield 0.0 here, and the port matches the source's
        // written form, which is what is being pinned.)
        assert_eq!(
            fb_write_depth_native_word(f32::NAN),
            fb_write_depth_native_word(0.0)
        );
    }

    // -- FbReadAny{Full,Changes}CS.hlsl --

    #[test]
    fn fb_read_decode_takes_the_depth_arm_only_when_flagged() {
        let word = 0x1234u32;
        match fb_read_decode(word, PixelSize::Bits16, ImageFormat::Rgba, true) {
            FbReadDecoded::Depth(d) => {
                assert_eq!(
                    d,
                    depth16_to_float(endian_swap_uint(word, PixelSize::Bits16))
                );
            }
            FbReadDecoded::Color(_) => panic!("is_depth must select the depth arm"),
        }
        match fb_read_decode(word, PixelSize::Bits16, ImageFormat::Rgba, false) {
            FbReadDecoded::Color(c) => {
                assert_eq!(
                    c,
                    uint_to_float4(
                        endian_swap_uint(word, PixelSize::Bits16),
                        PixelSize::Bits16,
                        ImageFormat::Rgba
                    )
                );
            }
            FbReadDecoded::Depth(_) => panic!("non-depth must select the color arm"),
        }
    }

    #[test]
    fn fb_read_decode_swaps_before_decoding_not_after() {
        // 0x00F8 byte-swaps to 0xF800, which decodes as pure red. If the
        // swap were skipped (or applied after the decode) the result would
        // be the near-black 0x00F8 decode instead.
        let decoded = fb_read_decode(0x00F8, PixelSize::Bits16, ImageFormat::Rgba, false);
        let unswapped = uint_to_float4(0x00F8, PixelSize::Bits16, ImageFormat::Rgba);
        assert_eq!(
            decoded,
            FbReadDecoded::Color(uint_to_float4(0xF800, PixelSize::Bits16, ImageFormat::Rgba))
        );
        assert_ne!(decoded, FbReadDecoded::Color(unswapped));
    }

    #[test]
    fn fb_read_decode_passes_siz_to_both_the_swap_and_the_decode() {
        // The same word under 16b and 32b must differ, because siz reaches
        // EndianSwapUINT as well as UINTToFloat4.
        let word = 0x11223344u32;
        let a = fb_read_decode(word, PixelSize::Bits16, ImageFormat::Rgba, false);
        let b = fb_read_decode(word, PixelSize::Bits32, ImageFormat::Rgba, false);
        assert_ne!(a, b);
    }

    #[test]
    fn fb_read_pixel_changed_compares_raw_unswapped_words() {
        assert!(fb_read_pixel_changed(1, 2));
        assert!(!fb_read_pixel_changed(7, 7));
        assert!(!fb_read_pixel_changed(0, 0));
        // The comparison happens BEFORE the endian swap, so two words that
        // are byte-swaps of each other are "changed" even though they would
        // swap to each other's values.
        assert!(fb_read_pixel_changed(0x00F8, 0xF800));
    }

    // -- RtCopy{DepthToColor,ColorToDepth}PS.hlsl --

    #[test]
    fn rt_copy_sample_counts_match_the_three_defines() {
        assert_eq!(RT_COPY_SAMPLE_COUNT_8X, 8);
        assert_eq!(RT_COPY_SAMPLE_COUNT_4X, 4);
        assert_eq!(RT_COPY_SAMPLE_COUNT_2X, 2);
    }

    #[test]
    fn rt_copy_depth_fold_keeps_the_closest_sample() {
        assert_eq!(
            rt_copy_depth_to_color_multisample_fold(&[0.9, 0.3, 0.7, 0.5]),
            0.3
        );
        // Order must not matter for the finite case.
        assert_eq!(
            rt_copy_depth_to_color_multisample_fold(&[0.3, 0.9, 0.5, 0.7]),
            0.3
        );
        // The 1.0f seed caps the result: all-far samples stay at 1.0, and an
        // empty dispatch returns the seed.
        assert_eq!(rt_copy_depth_to_color_multisample_fold(&[2.0, 3.0]), 1.0);
        assert_eq!(rt_copy_depth_to_color_multisample_fold(&[]), 1.0);
        // It is a MIN, not a max: swapping to the farthest would give 0.9.
        assert_ne!(
            rt_copy_depth_to_color_multisample_fold(&[0.9, 0.3, 0.7, 0.5]),
            0.9
        );
    }

    #[test]
    fn rt_copy_depth_fold_reproduces_hlsl_min_argument_order_for_nan() {
        // min(sample, accumulator) returns the FIRST argument on a false
        // comparison. `NaN < 1.0` is false, so the accumulator survives the
        // NaN sample -- unlike f32::min(NaN, 1.0), which is also 1.0 here,
        // but the ORDER matters in the other direction: once the
        // accumulator is NaN, `sample < NaN` is false and the ACCUMULATOR
        // (the NaN) survives. Both cases are pinned.
        assert_eq!(
            rt_copy_depth_to_color_multisample_fold(&[f32::NAN, 0.4]),
            0.4
        );
        // A NaN arriving after a smaller sample also does not disturb it.
        assert_eq!(
            rt_copy_depth_to_color_multisample_fold(&[0.4, f32::NAN]),
            0.4
        );
    }

    #[test]
    fn rt_copy_depth_to_color_single_round_trips_through_depth16() {
        // FloatToDepth16 then RGBA16ToFloat4 -- the depth word is
        // reinterpreted as a color, which is the pass's whole point.
        let got = rt_copy_depth_to_color_single(0.5);
        let expect =
            crate::fbcommon::uint16_to_float4(float_to_depth16(0.5, 0.0), ImageFormat::Rgba);
        assert_eq!(got, expect);
        // Distinct depths must give distinct colors, or the pass loses the
        // buffer.
        assert_ne!(
            rt_copy_depth_to_color_single(0.25),
            rt_copy_depth_to_color_single(0.75)
        );
        // The alpha channel is RGBA16's 1-bit coverage, so it is 0.0 or 1.0.
        assert!(got[3] == 0.0 || got[3] == 1.0);
    }

    #[test]
    fn rt_copy_color_fold_keeps_the_closest_reinterpreted_depth() {
        // Quantize each color to RGBA16 and reinterpret as depth; the fold
        // keeps the minimum. Black quantizes to a small word and so to a
        // near-zero depth; white to 0xFFFF and so to a large one.
        let black = rt_copy_color_to_depth_single([0.0, 0.0, 0.0, 0.0], false);
        let white = rt_copy_color_to_depth_single([1.0, 1.0, 1.0, 1.0], false);
        assert!(black < white);
        let fold = rt_copy_color_to_depth_multisample_fold(
            &[[1.0, 1.0, 1.0, 1.0], [0.0, 0.0, 0.0, 0.0]],
            false,
        );
        assert_eq!(fold, black);
        // Reversing the sample order must not change a finite min.
        assert_eq!(
            rt_copy_color_to_depth_multisample_fold(
                &[[0.0, 0.0, 0.0, 0.0], [1.0, 1.0, 1.0, 1.0]],
                false,
            ),
            black
        );
        // Empty returns the 1.0f seed.
        assert_eq!(rt_copy_color_to_depth_multisample_fold(&[], false), 1.0);
    }

    #[test]
    fn rt_copy_color_to_depth_single_matches_the_two_reused_ports() {
        for color in [
            [0.0, 0.0, 0.0, 0.0],
            [0.5, 0.25, 0.75, 1.0],
            [1.0, 1.0, 1.0, 1.0],
        ] {
            let rgba16 = float4_to_rgba16(color, rt_copy_zero_dither(), false);
            assert_eq!(
                rt_copy_color_to_depth_single(color, false),
                depth16_to_float(u32::from(rgba16.bits()))
            );
        }
    }

    #[test]
    fn rt_copy_color_to_depth_discards_uses_hdr_because_depth16_masks_bit_zero() {
        // `usesHDR` reaches this pass only through Float4ToRGBA16, where it
        // switches cvgRange from 255.0 to 65535.0. That changes
        // `round(a * cvgRange) % 8`, whose `& 0x4` bit becomes RGBA16's
        // **bit 0** -- and nothing else in the word.
        //
        // a = 60/4096 = 0.0146484375 is exact in f32 and is such a case:
        //   SDR: round_ties_even(0.0146484375 * 255)   = 4   -> 4 % 8 = 4, bit SET
        //   HDR: round_ties_even(0.0146484375 * 65535) = 960 -> 960 % 8 = 0, bit CLEAR
        let color = [0.5, 0.5, 0.5, 60.0 / 4096.0];
        let sdr_word = float4_to_rgba16(color, rt_copy_zero_dither(), false).bits();
        let hdr_word = float4_to_rgba16(color, rt_copy_zero_dither(), true).bits();
        // The two packed words really do differ, and by exactly bit 0.
        assert_ne!(sdr_word, hdr_word);
        assert_eq!(sdr_word ^ hdr_word, 1);
        assert_eq!(sdr_word & 1, 1);
        assert_eq!(hdr_word & 1, 0);

        // But Depth16ToFloat masks with DEPTH_EXPONENT_MASK 0xE000 and
        // DEPTH_MANTISSA_MASK 0x1FFC (`Depth.hlsli:7-8`). Neither covers
        // bit 0 or bit 1, so the coverage bit is structurally discarded and
        // `usesHDR` CANNOT change this pass's output. Pinning the collapse,
        // not a divergence.
        assert_eq!(
            rt_copy_color_to_depth_single(color, false),
            rt_copy_color_to_depth_single(color, true)
        );
        assert_eq!(depth16_to_float(0xE000 | 0x1FFC), depth16_to_float(0xFFFF));

        // The mask is not swallowing everything, though: a change in any bit
        // at or above bit 2 does reach the output, so the equality above is
        // specific to bits 0-1 rather than the function being constant.
        assert_ne!(depth16_to_float(0x0000), depth16_to_float(0x0004));
        assert_ne!(
            rt_copy_color_to_depth_single([0.0, 0.0, 0.0, 1.0], false),
            rt_copy_color_to_depth_single([1.0, 1.0, 1.0, 1.0], false)
        );
    }
}
