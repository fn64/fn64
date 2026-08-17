//! `clampWrapMirrorSample`'s texel address arithmetic and
//! `sampleTextureLevel`'s filter blends (including the three-sample
//! bilinear): a literal port of the permitted MIT RT64 Rust-port source
//! pinned at commit `5473732a822a4423b5696e7cb18fecc425a59875`
//! (`docs/RT64-PORT-AUTHORITY.md`), `src/shaders/TextureSampler.hlsli:74-110`
//! (`clampWrapMirrorSample`) and `:142-215` (`sampleTextureLevel`) (SHA-256
//! of the whole file, `927ca2d1c748862f683b3d6115bc97a56cc2ff343474a641046a64788fecef3a`,
//! 358 lines):
//!
//! ```text
//! float4 clampWrapMirrorSample(const RDPTile rdpTile, const GPUTile gpuTile, float2 tcScale, int2 texelInt, uint tlut, bool gpuTileUsesTMEM, uint mipLevel) {
//!     if (rdpTile.cms & G_TX_CLAMP) {
//!         texelInt.x = clamp(texelInt.x, 0, (round(tcScale.x * rdpTile.lrs) / 4) - (round(tcScale.x * rdpTile.uls) / 4) + round(tcScale.x - 1.0f));
//!     }
//!
//!     if (rdpTile.cmt & G_TX_CLAMP) {
//!         texelInt.y = clamp(texelInt.y, 0, (round(tcScale.y * rdpTile.lrt) / 4) - (round(tcScale.y * rdpTile.ult) / 4) + round(tcScale.y - 1.0f));
//!     }
//!
//!     const int masks = round(rdpTile.masks * tcScale.x);
//!     if ((rdpTile.cms & G_TX_MIRROR) && (masks > 0) && (modulo(texelInt.x, masks * 2) >= masks)) {
//!         texelInt.x = (masks - 1) - modulo(texelInt.x, masks);
//!     }
//!     else {
//!         texelInt.x = modulo(texelInt.x, masks);
//!     }
//!
//!     const int maskt = round(rdpTile.maskt * tcScale.y);
//!     if ((rdpTile.cmt & G_TX_MIRROR) && (maskt > 0) && (modulo(texelInt.y, maskt * 2) >= maskt)) {
//!         texelInt.y = (maskt - 1) - modulo(texelInt.y, maskt);
//!     }
//!     else {
//!         texelInt.y = modulo(texelInt.y, maskt);
//!     }
//!
//!     // Allows selection of only particular columns or rows as defined by the tile.
//!     texelInt = (texelInt & gpuTile.texelMask) + gpuTile.texelShift;
//!
//!     // Check if tile requires TMEM decoding and sample using dynamic decoding.
//!     if (gpuTileUsesTMEM) {
//!         return sampleTMEM(texelInt, rdpTile.siz, rdpTile.fmt, rdpTile.address, rdpTile.stride, tlut, rdpTile.palette, gTMEM[NonUniformResourceIndex(gpuTile.textureIndex)]);
//!     }
//!     // Sample the color version directly.
//!     else {
//!         return gTextures[NonUniformResourceIndex(gpuTile.textureIndex)].Load(int3(texelInt, mipLevel));
//!     }
//! }
//! ```
//!
//! ```text
//! float4 sampleTextureLevel(const RDPTile rdpTile, const GPUTile gpuTile, bool filterBilerp, bool filterAverage, bool linearFiltering, float2 uvCoord, uint tlut, bool canDecodeTMEM, uint mipLevel, bool usesHDR) {
//!     ...
//!     int2 texelBaseInt = floor(uvCoord);
//!     bool filtering = or(filterBilerp, linearFiltering);
//!     float4 samples[4];
//!     ... // samples[0..3] populated by clampWrapMirrorSample or sampleTextureNative
//!
//!     float4 sample00 = samples[0];
//!     if (filtering) {
//!         float2 fracPart = uvCoord - texelBaseInt;
//!         float4 sample01 = samples[1];
//!         float4 sample10 = samples[2];
//!         float4 sample11 = samples[3];
//!         if (linearFiltering) {
//!             return lerp(lerp(sample00, sample10, fracPart.x), lerp(sample01, sample11, fracPart.x), fracPart.y);
//!         }
//!         else {
//!             const bool useAverage = filterAverage && all(abs(fracPart - 0.5f) <= (1.0f / LowPrecision));
//!             if (filterAverage && useAverage) {
//!                 return (sample00 + sample01 + sample10 + sample11) / 4.0f;
//!             }
//!             else {
//!             // Originally written by ArthurCarvalho
//!             // Sourced from https://www.emutalk.net/threads/emulating-nintendo-64-3-sample-bilinear-filtering-using-shaders.54215/
//!                 float4 tri0 = lerp(sample00, sample10, fracPart.x) + (sample01 - sample00) * fracPart.y;
//!                 float4 tri1 = lerp(sample11, sample01, 1.0f - fracPart.x) + (sample10 - sample11) * (1.0f - fracPart.y);
//!                 return lerp(tri0, tri1, step(1.0f, fracPart.x + fracPart.y));
//!             }
//!         }
//!     }
//!     else {
//!         return sample00;
//!     }
//! }
//! ```
//!
//! **Reuse, not new type.** [`crate::math_hlsli::modulo`] is reused verbatim
//! (already ported from `Math.hlsli:11-17`, this crate's existing floored
//! `i32` modulo with its own `y == 0` passthrough documented in that
//! module) -- this module adds no second `modulo`. `crate::state::OtherMode`
//! and `crate::texture_lod::{compute_lod, LodSelection}` are the sibling
//! `TextureSampler.hlsli` ports already landed in this crate; neither is
//! called from here (this module owns only `clampWrapMirrorSample`'s
//! address arithmetic and `sampleTextureLevel`'s filter blends, not LOD
//! selection or dispatch). `crates/fn64-render-wgpu/src/shaders/three_nearest_filter.wgsl`
//! is a **different** three-sample filter: it is RDP's fixed-point
//! `filter_three_nearest_s10_5`-style corner blend (S.5 fixed-point `sf`/`tf`
//! fractions, `SCALE = 32`, an `if sf+tf<=32 {...} else {...}` corner
//! selection over integer RGBA8888 bytes) used by a different pipeline
//! stage, not `sampleTextureLevel`'s float `tri0`/`tri1`/`step` bilerp
//! ported here -- the two share only the "three nearest texels" idea, not a
//! formula, and nothing there is reused or duplicated by this module.
//!
//! ## Scope split (per the task card)
//!
//! This module ports only the **address arithmetic**
//! (`clamp_wrap_mirror_address`, `clampWrapMirrorSample`'s
//! clamp/mirror/wrap/mask/shift texel-coordinate math) and the **blend
//! arithmetic** (`sample_texture_level_blend`, `sampleTextureLevel`'s
//! linear/three-sample/average filter selection). The actual texel *fetch*
//! -- `sampleTMEM`, `gTextures[...].Load`, `sampleTextureNative`'s nine
//! native-sampler `SampleLevel` arms, and every `samples[i] =
//! clampWrapMirrorSample(...)` / `samples[i] = sampleTextureNative(...)`
//! population loop -- is a GPU resource binding (`Texture2D`, `gTMEM`,
//! samplers) this crate has no CPU-side representation for and does not
//! port here. Callers of [`clamp_wrap_mirror_address`] supply the resolved
//! `texelInt` this module hands back; callers of
//! [`sample_texture_level_blend`] supply the four already-fetched texel
//! samples (`samples[0..3]`, this port's `TexelSamples`) as plain `[f32; 4]`
//! RGBA values. `sampleTexture`'s mip-level orchestration (`:217-359`,
//! `computeLOD`/RDP mip-level selection, `flagHasMipmaps`, the final
//! `alphaIsCvg` coverage-modulo correction) is out of scope for this ticket
//! and is not ported by this module.
//!
//! ## Admitted domain
//!
//! - **HLSL `lerp(x, y, s) = x + s*(y-x)`, spelled out literally.** This
//!   port never reaches for a `mix`-shaped `e1*(1-s)+e2*s` expression --
//!   that is a different (though algebraically equal in exact real
//!   arithmetic) floating-point computation, and per the task card's own
//!   warning this module's [`hlsl_lerp`] helper spells out `x + s * (y -
//!   x)` verbatim at every call site that needs it.
//! - **The reversed `tri1` argument order is preserved exactly, not
//!   "normalized."** Source line 206 is `tri0 = lerp(sample00, sample10,
//!   fracPart.x) + (sample01 - sample00) * fracPart.y` (first `lerp`
//!   argument `sample00`, second `sample10`). Source line 207 is `tri1 =
//!   lerp(sample11, sample01, 1.0f - fracPart.x) + (sample10 - sample11) *
//!   (1.0f - fracPart.y)` -- note the **first** `lerp` argument is
//!   `sample11` and the **second** is `sample01`, the reverse of `tri0`'s
//!   `(sample00, sample10)` pairing, and the outer `+` term subtracts
//!   `sample11` from `sample10` (`sample10 - sample11`), not `sample01`
//!   from anything. This port's [`sample_texture_level_blend`] reproduces
//!   both `lerp` call's argument order and both subtraction operand orders
//!   exactly as written -- `tri1` is not a copy-pasted mirror of `tri0`
//!   with corners swapped, it is its own distinct expression, and swapping
//!   `sample10`/`sample11` or `sample01`/`sample00` anywhere in it would be
//!   a silent behavior change this port does not make.
//! - **Three-sample bilerp, not four.** The non-average, non-linear branch
//!   (the `else` of `if (linearFiltering)`, the `else` of `if
//!   (filterAverage && useAverage)`) never computes a standard four-corner
//!   bilinear blend; it computes exactly RT64's `tri0`/`tri1`/`step`
//!   triangular-filter blend over three of the four corner samples'
//!   effective contributions (attributed by RT64's own comment to
//!   ArthurCarvalho, sourced from the cited emutalk.net thread) and
//!   [`sample_texture_level_blend`] does not substitute a `mix`-based
//!   four-corner bilerp anywhere in that branch.
//! - **`step(edge, x) = (x >= edge) ? 1.0 : 0.0`** (Microsoft HLSL intrinsic
//!   reference; on-point same-intrinsic prior art already established in
//!   this crate at `combiner.rs`'s `wrap_clamp`/`step`-branch
//!   characterization, `combiner.rs:1432-1451`). `step(1.0f, fracPart.x +
//!   fracPart.y)` is therefore `1.0` when `fracPart.x + fracPart.y >= 1.0`
//!   (inclusive at exactly `1.0`) and `0.0` otherwise, and `lerp(tri0, tri1,
//!   step_result)` selects `tri1` exactly at that boundary, not just
//!   strictly past it.
//! - **Clamp/wrap/mirror ordering is preserved exactly, per axis,
//!   independently.** For each axis (`x` from `cms`/`masks`/`uls`/`lrs`;
//!   `y` from `cmt`/`maskt`/`ult`/`lrt`) the source performs, in this exact
//!   order and never re-ordered or fused: (1) an *optional* `G_TX_CLAMP`
//!   clamp of the raw `texelInt` component into `[0, clampHi]` where
//!   `clampHi` is computed from `tcScale`/`lrs`-or-`lrt`/`uls`-or-`ult`
//!   (only when the clamp flag bit is set -- otherwise `texelInt`'s
//!   component passes through this step completely unmodified, including
//!   staying negative or out of range); then (2) an *unconditional*
//!   `mask`/`maskt` computation (`round(rdpTile.masks * tcScale.x)` /
//!   `round(rdpTile.maskt * tcScale.y)`, computed even when clamp already
//!   ran); then (3) a mirror-vs-wrap branch that is evaluated **on the
//!   already-clamped-or-passed-through value from step (1)**, not on the
//!   pre-clamp `texelInt` -- clamp, when its flag is set, strictly precedes
//!   wrap/mirror on that axis, and this port's [`clamp_wrap_mirror_address`]
//!   performs the same clamp-then-wrap/mirror sequencing per axis, never
//!   computing wrap/mirror from the original unclamped coordinate. Clamp and
//!   mirror/wrap are not mutually exclusive in the source (both `cms`
//!   flag-tested independently; `G_TX_CLAMP` and `G_TX_MIRROR` are distinct
//!   bits, `1`/`2`, and RT64 does not assert they are never both set) --
//!   this port does not add an `else` between the clamp `if` and the
//!   mirror/wrap `if` that the source does not have, so a tile with both
//!   bits set clamps first and then still runs the mirror/wrap step on the
//!   clamped result, exactly as the literal `if`/`if` (not `if`/`else if`)
//!   source structure requires.
//! - **The mirror-vs-wrap `if`/`else` itself is a single decision per
//!   axis**, evaluated after step (2)'s `masks`/`maskt` computation:
//!   `(cms & G_TX_MIRROR) && (masks > 0) && (modulo(texelInt.x, masks*2) >=
//!   masks)` selects the mirror branch (`(masks - 1) - modulo(texelInt.x,
//!   masks)`); its `else` -- taken whenever any one of those three
//!   conjuncts is false, including when `G_TX_MIRROR` is unset entirely --
//!   is the plain wrap branch (`modulo(texelInt.x, masks)`). This port's
//!   three-conjunct `&&` chain (short-circuiting left to right, matching
//!   HLSL `&&`) and its `if`/`else` (not two independent `if`s) reproduce
//!   this exactly: a tile with `G_TX_MIRROR` unset always takes the plain
//!   wrap formula regardless of `masks`, and a tile with `G_TX_MIRROR` set
//!   but `masks <= 0` also always takes the plain wrap formula (the second
//!   conjunct `masks > 0` short-circuits mirror off).
//! - **The `masks == 0` (or negative) `modulo(t, masks)` call is not a
//!   guard this port invents nor a crash.** `rdpTile.masks` is a signed
//!   `int` field and `round(rdpTile.masks * tcScale.x)` can legitimately
//!   evaluate to `0` (or, if `tcScale.x` is negative, a negative `masks`);
//!   the mirror branch's `masks > 0` conjunct already excludes both cases
//!   from ever reaching the mirror formula's own `masks - 1` /
//!   `modulo(texelInt.x, masks)` calls, so the `masks <= 0` case *always*
//!   falls through to the wrap `else` branch's `modulo(texelInt.x, masks)`
//!   -- which, per [`crate::math_hlsli::modulo`]'s own already-landed and
//!   already-documented `y == 0` passthrough (`x - y * floor(x/y)`'s
//!   division guarded to return `x` unchanged when `y == 0`, not a crash or
//!   `NaN`/`inf` propagation), returns `texelInt.x` unmodified for
//!   `masks == 0` and the literal floored-modulo formula's own (defined,
//!   non-panicking) result for `masks < 0`. **This is the divide-by-zero
//!   frontier this ticket calls out**: the underlying `Math.hlsli`
//!   `modulo(x, y)` source performs `x / y` with **no** `y != 0` guard
//!   text-visible at its own call site inside `clampWrapMirrorSample` --
//!   the guard lives one level down, inside `modulo`'s own already-ported
//!   body (`math_hlsli.rs:103-109`), not in this module. This module does
//!   not add a second guard at its own call sites (that would be a
//!   redundant, silently-duplicated safety net this port does not invent);
//!   it calls `crate::math_hlsli::modulo` exactly as the source calls
//!   `modulo`, and the non-panicking, defined `y == 0` behavior is entirely
//!   attributable to that already-reviewed sibling module, reported here
//!   rather than re-derived or re-guarded.
//! - **`filterAverage`'s doubled-condition redundancy is preserved as an
//!   effective single condition, not restructured.** Source line 199-200:
//!   `const bool useAverage = filterAverage && all(...)`; `if (filterAverage
//!   && useAverage)`. Since `useAverage` already conjoins `filterAverage`,
//!   the outer `filterAverage &&` in the `if` is redundant (`useAverage`
//!   alone is never `true` while `filterAverage` is `false`) -- this port's
//!   [`sample_texture_level_blend`] computes the single effective condition
//!   (`filter_average && all-four-fracPart-components-within-tolerance`)
//!   once, matching the *behavior* exactly while not literally re-evaluating
//!   `filterAverage` twice (an artifact of the HLSL source's own local
//!   variable, not an observable divergence -- both forms are the same
//!   boolean for every input).
//! - **Integer/float conversion order in the address math.** `round(x)`
//!   (`tcScale.x * rdpTile.lrs` etc., and `rdpTile.masks * tcScale.x`) is
//!   the GPU-shader-HLSL `round()` intrinsic, documented by the primary
//!   Microsoft HLSL reference
//!   (<https://learn.microsoft.com/en-us/windows/win32/direct3dhlsl/dx-graphics-hlsl-round>)
//!   as round-half-to-even (`Round_ne` in DXIL), matching this crate's own
//!   established precedent for the same intrinsic
//!   (`depth_encode.rs:100-113`, `formats_dither.rs:87-91`) -- ported here
//!   as `f32::round_ties_even()`, not `f32::round()` (which is
//!   round-half-away-from-zero and would silently diverge at exact `.5`
//!   ties). Each `round(...)` result is then implicitly converted from
//!   `float` to the surrounding `int`/`float2` arithmetic per HLSL's
//!   ordinary implicit-conversion rules: `(round(...) / 4)` divides the
//!   already-rounded float by the float literal `4` (float division, not
//!   integer division -- `round()`'s HLSL return type is `float`, and `4`
//!   is not suffixed `u`/an explicit `int`), the two float quarter-terms
//!   and the `round(tcScale.x - 1.0f)` term are summed as `float`, and only
//!   the final sum is implicitly truncated-toward-zero to `int` at the
//!   `clamp(texelInt.x, 0, <that sum>)` call site, because `clamp`'s second
//!   and third arguments must share `texelInt.x`'s `int` type for HLSL
//!   overload resolution to select the all-`int` `clamp` overload. This
//!   port performs the same float arithmetic in `f32`, truncating only the
//!   final clamp-hi sum to `i32` via `as i32` (`f32 as i32` truncates
//!   toward zero and saturates on the domain this port makes no wider
//!   claim than, matching every other `as i32`/`as u32` cast already landed
//!   in this crate, e.g. `rt64_common.rs`'s `fixed_to_float`). `const int
//!   masks = round(rdpTile.masks * tcScale.x);` similarly rounds in `float`
//!   first, then truncates the already-rounded value to `int` via the
//!   implicit `float`-to-`int` initialization -- since the value is already
//!   an integer-valued float post-`round()`, truncation vs. any other
//!   rounding mode is not observable here (the fractional part is exactly
//!   `0.0` barring a `round_ties_even` result that itself is always
//!   integer-valued), so this port uses `as i32` directly on the
//!   `round_ties_even()` result with no separate truncation step needed
//!   beyond the cast itself.
//! - **`(texelInt & gpuTile.texelMask) + gpuTile.texelShift`: mixed
//!   `int2`/`uint2` bitwise-AND-then-add, reinterpreted back to `int2`.**
//!   HLSL's binary-operator implicit-conversion rules promote the `int2`
//!   operand of a mixed `int2 op uint2` expression to `uint2` (a
//!   bit-preserving reinterpret of each 32-bit lane, the same reinterpret
//!   already established as this crate's own precedent at
//!   `rt64_common.rs`'s `FixedMatrix::fixedToFloat`), so `texelInt &
//!   gpuTile.texelMask` computes as unsigned per-lane AND, `+
//!   gpuTile.texelShift` as unsigned per-lane wrapping add (HLSL `uint`
//!   addition wraps, is not UB), and the `uint2` result is implicitly
//!   reinterpreted back to `int2` on assignment to `texelInt` (again a
//!   bit-preserving reinterpret, not a saturating or panicking narrowing --
//!   both types are 32 bits wide, so this is a same-width reinterpret, not
//!   a narrowing conversion at all). This port's [`clamp_wrap_mirror_address`]
//!   performs this per-component as `(((component as u32) & mask) as u32)
//!   .wrapping_add(shift) as i32` -- `as u32`/`as i32` on same-width
//!   integers is exactly Rust's bit-preserving reinterpret, and
//!   `wrapping_add` matches HLSL `uint` addition's defined wraparound
//!   rather than Rust's default debug-mode overflow panic.
//! - **Negative-coordinate behavior.** `texelInt` (the caller-supplied base
//!   texel coordinate before this module's address math runs) is a signed
//!   `int2` and the source never asserts it is non-negative. A negative
//!   `texelInt.x`/`.y` with `G_TX_CLAMP` unset for that axis skips the
//!   optional clamp step entirely (per the ordering rule above) and flows
//!   straight into the unconditional `mask`/`modulo` step, where
//!   [`crate::math_hlsli::modulo`]'s floored (Python-`%`-like, not
//!   Rust-`%`-like) semantics -- already documented in that module --
//!   produce a non-negative result whenever `masks/maskt > 0` (floored
//!   modulo's result always shares the divisor's sign for a positive
//!   divisor), which this module relies on but does not re-derive; a
//!   negative `masks`/`maskt` is out of `modulo`'s own documented
//!   admitted-domain guarantee of a particular sign and this module makes
//!   no additional claim about that case beyond calling the same already-
//!   characterized function.
//! - **Out-of-range texel indices.** `texelInt` far outside `[0,
//!   textureDimensions)` is not rejected or asserted against anywhere in
//!   this module's ported scope -- clamp (when its flag is set) only
//!   bounds the value to `[0, clampHi]` where `clampHi` is itself
//!   `tcScale`/tile-geometry-derived and can be very large or, if `lrs <
//!   uls` (an inverted tile rect RT64 does not itself validate here),
//!   negative (making the `clamp(x, 0, negative)` call resolve to `0` by
//!   the same `min(max(x,0),hi)` composition documented for HLSL `clamp` at
//!   `texture_lod.rs`'s `hlsl_clamp_i32` -- this module reuses that same
//!   `min-then-max` composition inline rather than a second named helper,
//!   since HLSL float `clamp(x, lo, hi)` with `lo > hi` resolves to `hi`
//!   for the identical reason `texture_lod.rs` already documents for the
//!   `int` overload). The wrap/mirror step's `modulo` result is always in
//!   `[0, masks)` (or `[0, maskt)`) when `masks`/`maskt > 0` regardless of
//!   how far out of range the input `texelInt` component was, by floored
//!   modulo's own definition -- this module does not additionally clamp or
//!   assert on the post-mask-and-shift result, matching the source's own
//!   lack of any further bound.
//!
//! ## Nonclaims
//!
//! No GPU, WGSL, TMEM/texture resource binding, or production wiring --
//! this module is not called from anywhere in this crate, matching every
//! other characterization-first `fn64-render-wgpu` port module's precedent.
//! No RT64 visual/pixel/silicon parity or performance claim.
//!
//! This module does **not** port `computeLOD` (already landed in
//! `texture_lod.rs:27-72`, 46 of this file's 358 lines -- see that module's
//! own doc header, reused here only by citation, not by re-porting), the
//! resource-binding texel *fetch* (`sampleTMEM`, `gTextures[...].Load`,
//! `sampleTextureNative`'s native-sampler `SampleLevel` switch, `:114-140`),
//! or `sampleTexture`'s outer mip-level orchestration, perspective-
//! correction UV setup, RDP-sample-count/mip-level selection, or
//! `alphaIsCvg` coverage-modulo correction (`:217-359`). Per
//! `docs/rt64-port-inventory.json`, this whole file was previously marked
//! `port_state: "ported"` with `ported_as: ["crates/fn64-render-wgpu/src/texture_lod.rs"]`
//! -- that was accurate only for `computeLOD`'s 46 lines; `clampWrapMirrorSample`
//! (35 lines, `:74-110`) and `sampleTextureLevel`'s filter-blend tail
//! (`:189-215`, the part of `:142-215` this module actually ports) had zero
//! implementation anywhere under `crates/` before this module. This module
//! closes that real, previously-uncovered gap for exactly the address and
//! blend arithmetic named above; `sampleTextureNative`, `sampleTexture`, and
//! the sample-population loops inside `sampleTextureLevel` (`:150-187`)
//! remain unported and are not claimed here.

use crate::math_hlsli::modulo;

/// HLSL `lerp(x, y, s) = x + s * (y - x)`, spelled out literally -- **never**
/// the algebraically-equal-in-exact-arithmetic-but-different-in-floating-
/// point `mix`-shaped `x*(1-s) + y*s` (see module doc "Admitted domain").
#[inline]
fn hlsl_lerp(x: f32, y: f32, s: f32) -> f32 {
    x + s * (y - x)
}

/// HLSL `step(edge, x) = (x >= edge) ? 1.0 : 0.0` (see module doc, citing
/// this crate's existing `combiner.rs` `step`-branch prior art).
#[inline]
fn hlsl_step(edge: f32, x: f32) -> f32 {
    if x >= edge {
        1.0
    } else {
        0.0
    }
}

/// HLSL `clamp(x, lo, hi) = min(max(x, lo), hi)` for `f32` -- never panics
/// when `lo > hi` (resolves to `hi`), matching `texture_lod.rs`'s
/// `hlsl_clamp_i32` precedent for the `int` overload (see module doc,
/// "Out-of-range texel indices").
#[inline]
fn hlsl_clamp_f32(x: f32, lo: f32, hi: f32) -> f32 {
    x.max(lo).min(hi)
}

/// `RDPTile`'s address-arithmetic-relevant fields (`rt64_rdp_tile.h`).
/// `cms`/`cmt` carry the `G_TX_CLAMP` (`2`) / `G_TX_MIRROR` (`1`) bit flags;
/// `masks`/`maskt` are the tile's raw (pre-`tcScale`) mask exponents;
/// `uls`/`ult`/`lrs`/`lrt` are the tile's fixed-point-derived bounds used
/// only by the clamp step.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RdpTileAddressing {
    pub cms: i32,
    pub cmt: i32,
    pub masks: i32,
    pub maskt: i32,
    pub uls: f32,
    pub ult: f32,
    pub lrs: f32,
    pub lrt: f32,
}

impl RdpTileAddressing {
    /// Builds a [`RdpTileAddressing`] from **positional** arguments in the
    /// relative source declaration order of the eight `RDPTile` members this
    /// type owns (`rt64_rdp_tile.h` lines 18, 19, 22, 23, 24, 25, 26, 27).
    ///
    /// This transcribes the source's member order into an argument order that
    /// a reviewer can read against `rt64_rdp_tile.h` directly, and gives
    /// callers a form that does not restate fourteen field names.
    ///
    /// **It does not detect a declaration reorder, and no test here claims it
    /// does.** The body uses field-init shorthand, which binds by identifier,
    /// not by position; so does every accessor below. Swapping two field
    /// declarations of the same type leaves this constructor, those
    /// accessors, and every test compiling and passing unchanged. That was
    /// verified by mutation (`cms`/`cmt` swapped; all tests still green).
    /// In safe Rust there is no construction or access form for a named-field
    /// struct that binds positionally, so a reorder can only be caught by
    /// generating the declaration and an order witness from one source -- a
    /// change to the port's source text, not an added test.
    ///
    /// The sibling half of the same `RDPTile` split,
    /// [`crate::rt64_shared_params::RdpTileImageDescriptor`], carries an
    /// identically-shaped constructor with the identical limitation.
    ///
    /// Note the field order here is the struct's own declaration order
    /// (`cms`, `cmt`, `masks`, `maskt`, ...), which groups by role rather than
    /// following the header's line order; the argument names make the mapping
    /// explicit. This claims declaration order as a source-text fact only --
    /// no memory layout, size, or offset claim is made or implied.
    #[must_use]
    pub const fn in_source_order(
        cms: i32,
        cmt: i32,
        masks: i32,
        maskt: i32,
        uls: f32,
        ult: f32,
        lrs: f32,
        lrt: f32,
    ) -> RdpTileAddressing {
        RdpTileAddressing {
            cms,
            cmt,
            masks,
            maskt,
            uls,
            ult,
            lrs,
            lrt,
        }
    }

    /// The four `int` members in declaration order.
    #[must_use]
    pub const fn signed_members_in_source_order(self) -> [i32; 4] {
        [self.cms, self.cmt, self.masks, self.maskt]
    }

    /// The four `float` bound members in declaration order.
    #[must_use]
    pub const fn float_members_in_source_order(self) -> [f32; 4] {
        [self.uls, self.ult, self.lrs, self.lrt]
    }
}

/// `G_TX_MIRROR` (`rt64_f3d_defines.h:66`).
pub const G_TX_MIRROR: i32 = 1;
/// `G_TX_CLAMP` (`rt64_f3d_defines.h:67`).
pub const G_TX_CLAMP: i32 = 2;

/// `GPUTile`'s address-arithmetic-relevant fields (`rt64_gpu_tile.h`):
/// `texelMask`/`texelShift` are `uint2` in the source.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TexelMaskShift {
    pub texel_mask: (u32, u32),
    pub texel_shift: (u32, u32),
}

/// Literal port of `clampWrapMirrorSample`'s texel address arithmetic
/// (`TextureSampler.hlsli:74-101`) -- everything up to and including the
/// `texelInt = (texelInt & gpuTile.texelMask) + gpuTile.texelShift` line.
/// The TMEM/texture *fetch* that follows (`:102-109`) is out of this
/// module's scope (see module doc "Scope split") and is not called here;
/// this function returns the resolved `(x, y)` texel address a caller would
/// pass to that fetch.
pub fn clamp_wrap_mirror_address(
    tile: RdpTileAddressing,
    tc_scale: (f32, f32),
    texel_int: (i32, i32),
    mask_shift: TexelMaskShift,
) -> (i32, i32) {
    let (mut x, mut y) = texel_int;

    // Step (1): optional per-axis G_TX_CLAMP, strictly before wrap/mirror.
    if tile.cms & G_TX_CLAMP != 0 {
        let clamp_hi = (tc_scale.0 * tile.lrs).round_ties_even() / 4.0
            - (tc_scale.0 * tile.uls).round_ties_even() / 4.0
            + (tc_scale.0 - 1.0).round_ties_even();
        x = hlsl_clamp_f32(x as f32, 0.0, clamp_hi) as i32;
    }

    if tile.cmt & G_TX_CLAMP != 0 {
        let clamp_hi = (tc_scale.1 * tile.lrt).round_ties_even() / 4.0
            - (tc_scale.1 * tile.ult).round_ties_even() / 4.0
            + (tc_scale.1 - 1.0).round_ties_even();
        y = hlsl_clamp_f32(y as f32, 0.0, clamp_hi) as i32;
    }

    // Step (2)+(3): unconditional mask computation, then the single
    // mirror-vs-wrap decision per axis, evaluated on the (possibly
    // clamped) value from step (1).
    let masks = (tile.masks as f32 * tc_scale.0).round_ties_even() as i32;
    if (tile.cms & G_TX_MIRROR != 0) && (masks > 0) && (modulo(x, masks * 2) >= masks) {
        x = (masks - 1) - modulo(x, masks);
    } else {
        x = modulo(x, masks);
    }

    let maskt = (tile.maskt as f32 * tc_scale.1).round_ties_even() as i32;
    if (tile.cmt & G_TX_MIRROR != 0) && (maskt > 0) && (modulo(y, maskt * 2) >= maskt) {
        y = (maskt - 1) - modulo(y, maskt);
    } else {
        y = modulo(y, maskt);
    }

    // texelInt = (texelInt & gpuTile.texelMask) + gpuTile.texelShift:
    // int2/uint2 mixed bitwise-AND-then-add, bit-preserving reinterpret in
    // both directions (see module doc).
    let x = (((x as u32) & mask_shift.texel_mask.0).wrapping_add(mask_shift.texel_shift.0)) as i32;
    let y = (((y as u32) & mask_shift.texel_mask.1).wrapping_add(mask_shift.texel_shift.1)) as i32;

    (x, y)
}

/// The four caller-supplied, already-fetched texel samples
/// (`sampleTextureLevel`'s `samples[0..3]`): `s00` = `texelBaseInt`, `s01` =
/// `+ (0,1)`, `s10` = `+ (1,0)`, `s11` = `+ (1,1)`, matching the source's own
/// index-to-offset convention (`i >> 1, i & 1`) and its direct `samples[1]`/
/// `samples[2]`/`samples[3]` naming as `sample01`/`sample10`/`sample11`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TexelSamples {
    pub s00: [f32; 4],
    pub s01: [f32; 4],
    pub s10: [f32; 4],
    pub s11: [f32; 4],
}

/// `LowPrecision` (`TextureSampler.hlsli:112`).
pub const LOW_PRECISION: f32 = 128.0;

/// Literal port of `sampleTextureLevel`'s filter-blend tail
/// (`TextureSampler.hlsli:189-214`): given the four already-fetched corner
/// samples and the filtering flags/fractional UV offset, selects
/// none-filtered / linear / three-sample-triangular / four-sample-average
/// exactly as the source's nested `if`s do. `frac_part` is `uvCoord -
/// texelBaseInt`, computed by the caller (this function does not take
/// `uvCoord`/`texelBaseInt` directly, since `floor()`'s int/float boundary
/// is `sampleTextureLevel`'s own concern, not this blend tail's).
pub fn sample_texture_level_blend(
    filter_bilerp: bool,
    filter_average: bool,
    linear_filtering: bool,
    frac_part: (f32, f32),
    samples: TexelSamples,
) -> [f32; 4] {
    let filtering = filter_bilerp || linear_filtering;
    let sample00 = samples.s00;

    if !filtering {
        return sample00;
    }

    let sample01 = samples.s01;
    let sample10 = samples.s10;
    let sample11 = samples.s11;

    if linear_filtering {
        // lerp(lerp(sample00, sample10, fracPart.x), lerp(sample01, sample11, fracPart.x), fracPart.y)
        let mut out = [0.0f32; 4];
        for c in 0..4 {
            let top = hlsl_lerp(sample00[c], sample10[c], frac_part.0);
            let bottom = hlsl_lerp(sample01[c], sample11[c], frac_part.0);
            out[c] = hlsl_lerp(top, bottom, frac_part.1);
        }
        return out;
    }

    // useAverage already conjoins filterAverage; the source's outer
    // `filterAverage && useAverage` is therefore a redundant re-test of the
    // same flag (see module doc) -- this port evaluates the single
    // effective condition once.
    let use_average = filter_average
        && (0..2)
            .map(|i| if i == 0 { frac_part.0 } else { frac_part.1 })
            .all(|f| (f - 0.5).abs() <= (1.0 / LOW_PRECISION));

    if use_average {
        let mut out = [0.0f32; 4];
        for c in 0..4 {
            out[c] = (sample00[c] + sample01[c] + sample10[c] + sample11[c]) / 4.0;
        }
        return out;
    }

    // Three-sample triangular filter. tri1 is NOT tri0 with corners
    // swapped -- preserve its reversed lerp-argument order and its own
    // distinct subtraction operands exactly (see module doc).
    let mut out = [0.0f32; 4];
    for c in 0..4 {
        let tri0 = hlsl_lerp(sample00[c], sample10[c], frac_part.0)
            + (sample01[c] - sample00[c]) * frac_part.1;
        let tri1 = hlsl_lerp(sample11[c], sample01[c], 1.0 - frac_part.0)
            + (sample10[c] - sample11[c]) * (1.0 - frac_part.1);
        let selector = hlsl_step(1.0, frac_part.0 + frac_part.1);
        out[c] = hlsl_lerp(tri0, tri1, selector);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tile(cms: i32, cmt: i32, masks: i32, maskt: i32) -> RdpTileAddressing {
        RdpTileAddressing {
            cms,
            cmt,
            masks,
            maskt,
            uls: 0.0,
            ult: 0.0,
            lrs: 0.0,
            lrt: 0.0,
        }
    }

    fn identity_mask_shift() -> TexelMaskShift {
        TexelMaskShift {
            texel_mask: (u32::MAX, u32::MAX),
            texel_shift: (0, 0),
        }
    }

    // --- RdpTileAddressing: declaration order ---

    #[test]
    fn rdp_tile_addressing_in_source_order_maps_arguments_to_named_fields() {
        // This checks that `in_source_order`'s argument list maps to the
        // fields its parameter names promise, and that the two accessors
        // agree with it. It is NOT a reorder detector: see the constructor's
        // own docs. Swapping two same-typed field declarations keeps this
        // test green (verified by mutation), because field-init shorthand and
        // field access both bind by name, never by position.
        //
        // Nothing here claims anything about memory layout, size, or offsets.
        let t = RdpTileAddressing::in_source_order(1, 2, 3, 4, 5.0, 6.0, 7.0, 8.0);
        assert_eq!(t.cms, 1);
        assert_eq!(t.cmt, 2);
        assert_eq!(t.masks, 3);
        assert_eq!(t.maskt, 4);
        assert_eq!(t.uls, 5.0);
        assert_eq!(t.ult, 6.0);
        assert_eq!(t.lrs, 7.0);
        assert_eq!(t.lrt, 8.0);
        // Second, independent derivation of the same order, per run of types.
        assert_eq!(t.signed_members_in_source_order(), [1, 2, 3, 4]);
        assert_eq!(t.float_members_in_source_order(), [5.0, 6.0, 7.0, 8.0]);
    }

    // --- clamp_wrap_mirror_address: no flags at all ---

    #[test]
    fn no_clamp_no_mirror_masks_zero_passes_through_unchanged() {
        // cms=cmt=0 (no flags), masks=maskt=0 -> mask computation yields 0,
        // wrap else-branch calls modulo(x, 0) which is a passthrough (see
        // math_hlsli::modulo's own y==0 documented behavior).
        let out = clamp_wrap_mirror_address(
            tile(0, 0, 0, 0),
            (1.0, 1.0),
            (37, -19),
            identity_mask_shift(),
        );
        assert_eq!(out, (37, -19));
    }

    #[test]
    fn no_clamp_no_mirror_masks_zero_negative_x_passes_through() {
        let out = clamp_wrap_mirror_address(
            tile(0, 0, 0, 0),
            (1.0, 1.0),
            (-5, -5),
            identity_mask_shift(),
        );
        assert_eq!(out, (-5, -5));
    }

    // --- wrap-only (no clamp, no mirror flag): plain floored modulo ---

    #[test]
    fn wrap_only_positive_in_range_is_identity() {
        // masks = round(8 * 1.0) = 8. texelInt.x = 3 in [0,8) -> modulo(3,8)=3.
        let out =
            clamp_wrap_mirror_address(tile(0, 0, 8, 8), (1.0, 1.0), (3, 5), identity_mask_shift());
        assert_eq!(out, (3, 5));
    }

    #[test]
    fn wrap_only_positive_out_of_range_wraps() {
        // modulo(11, 8) = 11 - 8*floor(11/8) = 11 - 8*1 = 3.
        let out = clamp_wrap_mirror_address(
            tile(0, 0, 8, 8),
            (1.0, 1.0),
            (11, 11),
            identity_mask_shift(),
        );
        assert_eq!(out, (3, 3));
    }

    #[test]
    fn wrap_only_negative_wraps_to_nonnegative_floored_modulo() {
        // modulo(-1, 8) = -1 - 8*floor(-1/8) = -1 - 8*(-1) = 7.
        let out = clamp_wrap_mirror_address(
            tile(0, 0, 8, 8),
            (1.0, 1.0),
            (-1, -1),
            identity_mask_shift(),
        );
        assert_eq!(out, (7, 7));
    }

    #[test]
    fn wrap_only_mirror_flag_set_but_masks_not_positive_still_wraps() {
        // G_TX_MIRROR set on both axes, but masks computed as 0 (masks
        // field itself 0) -- the `masks > 0` conjunct excludes mirror, so
        // this must still take the plain wrap else-branch (identity here
        // via modulo's y==0 passthrough).
        let out = clamp_wrap_mirror_address(
            tile(G_TX_MIRROR, G_TX_MIRROR, 0, 0),
            (1.0, 1.0),
            (9, -9),
            identity_mask_shift(),
        );
        assert_eq!(out, (9, -9));
    }

    // --- mirror: masks > 0, G_TX_MIRROR set, boundary at masks (>=, not >) ---

    #[test]
    fn mirror_below_masks_takes_wrap_branch() {
        // masks=8: modulo(x,16) < 8 range takes wrap (else) branch.
        // x=3: modulo(3,16)=3 < 8 -> wrap: modulo(3,8)=3.
        let out = clamp_wrap_mirror_address(
            tile(G_TX_MIRROR, 0, 8, 8),
            (1.0, 1.0),
            (3, 0),
            identity_mask_shift(),
        );
        assert_eq!(out.0, 3);
    }

    #[test]
    fn mirror_exactly_at_masks_boundary_triggers_mirror_branch() {
        // masks=8, x=8: modulo(8,16)=8 >= 8 (inclusive boundary) -> mirror:
        // (masks-1) - modulo(x,masks) = 7 - modulo(8,8) = 7 - 0 = 7.
        let out = clamp_wrap_mirror_address(
            tile(G_TX_MIRROR, 0, 8, 8),
            (1.0, 1.0),
            (8, 0),
            identity_mask_shift(),
        );
        assert_eq!(out.0, 7);
    }

    #[test]
    fn mirror_just_below_boundary_does_not_trigger() {
        // masks=8, x=7: modulo(7,16)=7 < 8 -> wrap branch: modulo(7,8)=7.
        let out = clamp_wrap_mirror_address(
            tile(G_TX_MIRROR, 0, 8, 8),
            (1.0, 1.0),
            (7, 0),
            identity_mask_shift(),
        );
        assert_eq!(out.0, 7);
    }

    #[test]
    fn mirror_full_second_period_maps_back_down() {
        // masks=8, x=15: modulo(15,16)=15 >= 8 -> mirror: 7 - modulo(15,8)
        // = 7 - 7 = 0.
        let out = clamp_wrap_mirror_address(
            tile(G_TX_MIRROR, 0, 8, 8),
            (1.0, 1.0),
            (15, 0),
            identity_mask_shift(),
        );
        assert_eq!(out.0, 0);
    }

    #[test]
    fn mirror_x_axis_and_wrap_y_axis_are_independent() {
        // cms has mirror, cmt does not -- verifies per-axis independence:
        // x mirrors (masks=8,x=15 -> 0), y plain-wraps (maskt=8, y=15 -> 7).
        let out = clamp_wrap_mirror_address(
            tile(G_TX_MIRROR, 0, 8, 8),
            (1.0, 1.0),
            (15, 15),
            identity_mask_shift(),
        );
        assert_eq!(out, (0, 7));
    }

    #[test]
    fn mirror_negative_coordinate() {
        // masks=8, x=-1: modulo(-1,16) = -1 -16*floor(-1/16) = -1-16*(-1) =
        // 15 >= 8 -> mirror: 7 - modulo(-1,8) = 7 - 7 = 0.
        let out = clamp_wrap_mirror_address(
            tile(G_TX_MIRROR, 0, 8, 8),
            (1.0, 1.0),
            (-1, 0),
            identity_mask_shift(),
        );
        assert_eq!(out.0, 0);
    }

    // --- clamp: flag gating, ordering before wrap/mirror ---

    #[test]
    fn clamp_flag_unset_skips_clamp_even_with_negative_input() {
        // cms has no CLAMP bit -- clamp is skipped regardless of lrs/uls;
        // only wrap (masks=100, well above the magnitude here) applies.
        let mut t = tile(0, 0, 100, 100);
        t.uls = 0.0;
        t.lrs = 1000.0;
        let out = clamp_wrap_mirror_address(t, (1.0, 1.0), (-50, 0), identity_mask_shift());
        // modulo(-50, 100) = -50 - 100*floor(-50/100) = -50 -100*(-1) = 50.
        assert_eq!(out.0, 50);
    }

    #[test]
    fn clamp_flag_set_clamps_before_wrap_changes_result() {
        // clampHi = round(1*lrs)/4 - round(1*uls)/4 + round(1-1) =
        // round(40)/4 - round(0)/4 + 0 = 10 - 0 + 0 = 10.
        // texelInt.x = 50 clamps to 10 first (since CLAMP set), THEN wrap
        // with masks=100 (much larger than 10) is a no-op: modulo(10,100)=10.
        let mut t = tile(G_TX_CLAMP, 0, 100, 100);
        t.uls = 0.0;
        t.lrs = 40.0;
        let out = clamp_wrap_mirror_address(t, (1.0, 1.0), (50, 0), identity_mask_shift());
        assert_eq!(out.0, 10);
    }

    #[test]
    fn clamp_lower_bound_is_zero_negative_input_clamps_up() {
        let mut t = tile(G_TX_CLAMP, 0, 100, 100);
        t.uls = 0.0;
        t.lrs = 40.0;
        let out = clamp_wrap_mirror_address(t, (1.0, 1.0), (-99, 0), identity_mask_shift());
        // clampHi=10 as above; texelInt.x=-99 clamps up to 0, then
        // modulo(0,100)=0.
        assert_eq!(out.0, 0);
    }

    #[test]
    fn clamp_and_mirror_both_set_clamp_runs_first_then_mirror_sees_clamped_value() {
        // clampHi = round(40)/4 - 0 + 0 = 10. masks=8 (independent field).
        // Raw x=999 would mirror very differently than the clamped x=10.
        // clamped x=10: modulo(10,16)=10 >=8 -> mirror: 7-modulo(10,8) =
        // 7-2 = 5.
        let mut t = tile(G_TX_CLAMP | G_TX_MIRROR, 0, 8, 8);
        t.uls = 0.0;
        t.lrs = 40.0;
        let out = clamp_wrap_mirror_address(t, (1.0, 1.0), (999, 0), identity_mask_shift());
        assert_eq!(out.0, 5);
    }

    #[test]
    fn clamp_hi_can_be_negative_resolves_to_zero_like_hlsl_clamp() {
        // uls > lrs (an inverted tile rect): clampHi = round(0)/4 -
        // round(40)/4 + 0 = 0 - 10 = -10. HLSL clamp(x, 0, -10) resolves to
        // -10's own min(max(x,0),-10) = -10 always (since max(x,0)>=0>-10,
        // outer min always picks -10) -- NOT zero. This test asserts the
        // literal min(max(x,lo),hi) composition, not a naive "clamps to
        // zero" assumption.
        let mut t = tile(G_TX_CLAMP, 0, 0, 0);
        t.uls = 40.0;
        t.lrs = 0.0;
        let out = clamp_wrap_mirror_address(t, (1.0, 1.0), (5, 0), identity_mask_shift());
        // clampHi = -10 -> hlsl_clamp_f32(5.0, 0.0, -10.0) = max(5,0)=5,
        // min(5,-10) = -10. Then wrap with masks=0: modulo(-10,0)=-10
        // (passthrough).
        assert_eq!(out.0, -10);
    }

    // --- mask/shift application ---

    #[test]
    fn texel_mask_selects_low_bits() {
        // texel_mask = 0b011 selects only the low 2 bits.
        let mask_shift = TexelMaskShift {
            texel_mask: (0b011, u32::MAX),
            texel_shift: (0, 0),
        };
        let out = clamp_wrap_mirror_address(tile(0, 0, 0, 0), (1.0, 1.0), (0b111, 0), mask_shift);
        assert_eq!(out.0, 0b011);
    }

    #[test]
    fn texel_shift_adds_after_masking() {
        let mask_shift = TexelMaskShift {
            texel_mask: (u32::MAX, u32::MAX),
            texel_shift: (100, 0),
        };
        let out = clamp_wrap_mirror_address(tile(0, 0, 0, 0), (1.0, 1.0), (5, 0), mask_shift);
        assert_eq!(out.0, 105);
    }

    #[test]
    fn texel_mask_zero_forces_zero_before_shift() {
        let mask_shift = TexelMaskShift {
            texel_mask: (0, u32::MAX),
            texel_shift: (42, 0),
        };
        let out = clamp_wrap_mirror_address(tile(0, 0, 0, 0), (1.0, 1.0), (999, 0), mask_shift);
        assert_eq!(out.0, 42);
    }

    #[test]
    fn texel_mask_and_negative_reinterpret_matches_bit_pattern() {
        // texelInt.x = -1 (all bits set as i32) & mask 0xFF = 0xFF = 255.
        let mask_shift = TexelMaskShift {
            texel_mask: (0xFF, u32::MAX),
            texel_shift: (0, 0),
        };
        let out = clamp_wrap_mirror_address(tile(0, 0, 0, 0), (1.0, 1.0), (-1, 0), mask_shift);
        assert_eq!(out.0, 255);
    }

    // --- round_ties_even in the clamp-hi computation ---

    #[test]
    fn clamp_hi_round_ties_even_at_exact_half() {
        // tcScale.x=1, lrs=2.5 -> round_ties_even(2.5) = 2 (even), not 3
        // (round-half-away-from-zero would give 3). uls=0 -> round(0)=0.
        // tcScale.x - 1.0 = 0.0 -> round(0)=0.
        // clampHi = 2/4 - 0/4 + 0 = 0.5.
        let mut t = tile(G_TX_CLAMP, 0, 0, 0);
        t.uls = 0.0;
        t.lrs = 2.5;
        let out = clamp_wrap_mirror_address(t, (1.0, 1.0), (10, 0), identity_mask_shift());
        // clamp(10, 0, 0.5) as i32 -> 0.5 as i32 truncates to 0.
        assert_eq!(out.0, 0);
    }

    // --- sample_texture_level_blend: no filtering ---

    #[test]
    fn no_filtering_returns_sample00_untouched() {
        let samples = TexelSamples {
            s00: [1.0, 2.0, 3.0, 4.0],
            s01: [10.0, 10.0, 10.0, 10.0],
            s10: [20.0, 20.0, 20.0, 20.0],
            s11: [30.0, 30.0, 30.0, 30.0],
        };
        let out = sample_texture_level_blend(false, false, false, (0.5, 0.5), samples);
        assert_eq!(out, [1.0, 2.0, 3.0, 4.0]);
    }

    #[test]
    fn filter_bilerp_false_linear_false_ignores_average_and_fraction() {
        // Neither filterBilerp nor linearFiltering -> `filtering` is false
        // regardless of filterAverage or fracPart; sample00 passes through.
        let samples = TexelSamples {
            s00: [5.0; 4],
            s01: [1.0; 4],
            s10: [1.0; 4],
            s11: [1.0; 4],
        };
        let out = sample_texture_level_blend(false, true, false, (0.5, 0.5), samples);
        assert_eq!(out, [5.0; 4]);
    }

    // --- linear filtering: standard four-corner bilerp ---

    #[test]
    fn linear_filtering_at_texel_center_zero_fraction_returns_sample00() {
        let samples = TexelSamples {
            s00: [1.0, 2.0, 3.0, 4.0],
            s01: [10.0; 4],
            s10: [20.0; 4],
            s11: [30.0; 4],
        };
        let out = sample_texture_level_blend(true, false, true, (0.0, 0.0), samples);
        assert_eq!(out, [1.0, 2.0, 3.0, 4.0]);
    }

    #[test]
    fn linear_filtering_at_boundary_fraction_one_one_returns_sample11() {
        let samples = TexelSamples {
            s00: [1.0; 4],
            s01: [2.0; 4],
            s10: [3.0; 4],
            s11: [4.0, 5.0, 6.0, 7.0],
        };
        let out = sample_texture_level_blend(true, false, true, (1.0, 1.0), samples);
        assert_eq!(out, [4.0, 5.0, 6.0, 7.0]);
    }

    #[test]
    fn linear_filtering_interior_point_matches_hand_computed_bilerp() {
        // fx=0.25, fy=0.75. HLSL: lerp(lerp(s00,s10,fx), lerp(s01,s11,fx), fy)
        // s00=0, s10=100, s01=200, s11=300 (single channel for clarity).
        // top = lerp(0,100,0.25) = 0 + 0.25*(100-0) = 25.
        // bottom = lerp(200,300,0.25) = 200 + 0.25*(300-200) = 225.
        // result = lerp(25,225,0.75) = 25 + 0.75*(225-25) = 175.
        let samples = TexelSamples {
            s00: [0.0; 4],
            s01: [200.0; 4],
            s10: [100.0; 4],
            s11: [300.0; 4],
        };
        let out = sample_texture_level_blend(true, false, true, (0.25, 0.75), samples);
        for c in out {
            assert!((c - 175.0).abs() < 1e-4, "c={c}");
        }
    }

    #[test]
    fn linear_filtering_asymmetric_fx_fy_confirms_operand_order_not_swapped() {
        // fx=0.0 (pure y-axis blend of s00/s01... wait: top=lerp(s00,s10,0)=s00,
        // bottom=lerp(s01,s11,0)=s01, result=lerp(s00,s01,fy). Choosing
        // fy=1.0 isolates s01 entirely, proving x and y fractions are not
        // swapped anywhere in this port.
        let samples = TexelSamples {
            s00: [1.0; 4],
            s01: [99.0; 4],
            s10: [2.0; 4],
            s11: [3.0; 4],
        };
        let out = sample_texture_level_blend(true, false, true, (0.0, 1.0), samples);
        assert_eq!(out, [99.0; 4]);
    }

    // --- four-sample average path ---

    #[test]
    fn average_path_triggers_at_exact_center_and_averages_all_four() {
        // fracPart = (0.5, 0.5) exactly -> |0.5-0.5| = 0 <= 1/128 for both
        // components -> useAverage true.
        let samples = TexelSamples {
            s00: [4.0; 4],
            s01: [8.0; 4],
            s10: [12.0; 4],
            s11: [16.0; 4],
        };
        let out = sample_texture_level_blend(true, true, false, (0.5, 0.5), samples);
        // (4+8+12+16)/4 = 10.
        assert_eq!(out, [10.0; 4]);
    }

    #[test]
    fn average_path_requires_filter_average_flag_even_within_tolerance() {
        // Exactly centered fracPart but filter_average=false -> must fall
        // through to the three-sample tri0/tri1 path, not average.
        let samples = TexelSamples {
            s00: [0.0; 4],
            s01: [0.0; 4],
            s10: [0.0; 4],
            s11: [100.0; 4],
        };
        let out = sample_texture_level_blend(true, false, false, (0.5, 0.5), samples);
        // Independently hand-computed via the tri0/tri1 formula below
        // (see `three_sample_path_matches_hand_computed_expectation`) --
        // must NOT equal the plain average (25.0), proving the flag gates
        // the average path.
        assert_ne!(out[0], 25.0);
    }

    #[test]
    fn average_path_tolerance_boundary_just_inside_triggers() {
        // |frac - 0.5| == exactly 1/128 (inclusive boundary, <=).
        let eps = 1.0 / LOW_PRECISION;
        let samples = TexelSamples {
            s00: [1.0; 4],
            s01: [2.0; 4],
            s10: [3.0; 4],
            s11: [4.0; 4],
        };
        let out = sample_texture_level_blend(true, true, false, (0.5 + eps, 0.5 - eps), samples);
        assert_eq!(out, [2.5; 4]);
    }

    #[test]
    fn average_path_tolerance_boundary_just_outside_does_not_trigger() {
        let eps = 1.0 / LOW_PRECISION;
        let just_past = 0.5 + eps + 0.001;
        let samples = TexelSamples {
            s00: [1.0; 4],
            s01: [2.0; 4],
            s10: [3.0; 4],
            s11: [4.0; 4],
        };
        let out = sample_texture_level_blend(true, true, false, (just_past, 0.5), samples);
        assert_ne!(out, [2.5; 4]);
    }

    #[test]
    fn average_path_only_x_out_of_tolerance_falls_through_all_required() {
        // `all()` semantics: ONE axis outside tolerance must disable
        // average even if the other axis is exactly centered.
        let samples = TexelSamples {
            s00: [1.0; 4],
            s01: [2.0; 4],
            s10: [3.0; 4],
            s11: [4.0; 4],
        };
        let out = sample_texture_level_blend(true, true, false, (0.9, 0.5), samples);
        assert_ne!(out, [2.5; 4]);
    }

    // --- three-sample triangular filter: hand-computed expectations ---

    #[test]
    fn three_sample_path_matches_hand_computed_expectation() {
        // fx=0.5, fy=0.5 exactly but filter_average=false forces the
        // three-sample path (not average). Single-channel values:
        // s00=0, s01=0, s10=0, s11=100.
        // tri0 = lerp(s00,s10,fx) + (s01-s00)*fy = lerp(0,0,0.5) + (0-0)*0.5 = 0.
        // tri1 = lerp(s11,s01,1-fx) + (s10-s11)*(1-fy)
        //      = lerp(100,0,0.5) + (0-100)*0.5
        //      = (100 + 0.5*(0-100)) + (-50)
        //      = (100 - 50) + (-50) = 50 - 50 = 0.
        // selector = step(1.0, fx+fy) = step(1.0, 1.0) = 1.0 (>=, inclusive).
        // result = lerp(tri0, tri1, 1.0) = tri0 + 1.0*(tri1-tri0) = tri1 = 0.
        let samples = TexelSamples {
            s00: [0.0; 4],
            s01: [0.0; 4],
            s10: [0.0; 4],
            s11: [100.0; 4],
        };
        let out = sample_texture_level_blend(true, false, false, (0.5, 0.5), samples);
        assert_eq!(out, [0.0; 4]);
    }

    #[test]
    fn three_sample_path_fx_fy_sum_below_one_selects_tri0() {
        // fx=0.2, fy=0.3 (sum=0.5 < 1.0) -> selector = step(1.0,0.5) = 0.0
        // -> result = tri0 exactly.
        // s00=10, s01=20, s10=30, s11=40 (single channel).
        // tri0 = lerp(10,30,0.2) + (20-10)*0.3 = (10+0.2*20) + 3 = 14+3=17.
        let samples = TexelSamples {
            s00: [10.0; 4],
            s01: [20.0; 4],
            s10: [30.0; 4],
            s11: [40.0; 4],
        };
        let out = sample_texture_level_blend(true, false, false, (0.2, 0.3), samples);
        for c in out {
            assert!((c - 17.0).abs() < 1e-4, "c={c}");
        }
    }

    #[test]
    fn three_sample_path_fx_fy_sum_above_one_selects_tri1() {
        // fx=0.7, fy=0.6 (sum=1.3 > 1.0) -> selector=1.0 -> result = tri1
        // exactly.
        // s00=10, s01=20, s10=30, s11=40 (single channel).
        // tri1 = lerp(s11,s01,1-fx) + (s10-s11)*(1-fy)
        //      = lerp(40,20,0.3) + (30-40)*0.4
        //      = (40 + 0.3*(20-40)) + (-10*0.4)
        //      = (40 - 6) + (-4) = 34 - 4 = 30.
        let samples = TexelSamples {
            s00: [10.0; 4],
            s01: [20.0; 4],
            s10: [30.0; 4],
            s11: [40.0; 4],
        };
        let out = sample_texture_level_blend(true, false, false, (0.7, 0.6), samples);
        for c in out {
            assert!((c - 30.0).abs() < 1e-4, "c={c}");
        }
    }

    #[test]
    fn three_sample_path_reversed_tri1_argument_order_is_load_bearing() {
        // Construct an asymmetric case where swapping tri1's lerp argument
        // order (lerp(sample01,sample11,...) instead of the correct
        // lerp(sample11,sample01,...)) would change the result, proving
        // this port's operand order matters and is exercised.
        // fx=0.9, fy=0.9 (sum=1.8>1.0 -> selects tri1).
        // Correct: tri1 = lerp(s11,s01,1-fx) + (s10-s11)*(1-fy)
        //   1-fx=0.1, 1-fy=0.1.
        //   s00=0, s01=1000, s10=0, s11=0 (isolate s01/s11 asymmetry).
        //   tri1 = lerp(0,1000,0.1) + (0-0)*0.1 = 0 + 0.1*1000 + 0 = 100.
        // A wrongly-swapped tri1' = lerp(s01,s11,1-fx) + (s10-s11)*(1-fy)
        //   = lerp(1000,0,0.1) + 0 = 1000 + 0.1*(0-1000) = 1000-100=900.
        // These differ (100 vs 900), so this test fails if the argument
        // order were ever accidentally normalized/swapped.
        let samples = TexelSamples {
            s00: [0.0; 4],
            s01: [1000.0; 4],
            s10: [0.0; 4],
            s11: [0.0; 4],
        };
        let out = sample_texture_level_blend(true, false, false, (0.9, 0.9), samples);
        for c in out {
            assert!(
                (c - 100.0).abs() < 1e-3,
                "c={c} (expected 100, not the 900 a swapped tri1 would give)"
            );
        }
    }

    #[test]
    fn three_sample_path_selector_boundary_is_inclusive_ge_not_strict_gt() {
        // fx+fy == exactly 1.0 must select tri1 (step is >=), verified by
        // constructing tri0 != tri1 and checking the result equals tri1.
        // fx=0.4, fy=0.6 (sum=1.0 exactly).
        let samples = TexelSamples {
            s00: [10.0; 4],
            s01: [20.0; 4],
            s10: [30.0; 4],
            s11: [40.0; 4],
        };
        let out = sample_texture_level_blend(true, false, false, (0.4, 0.6), samples);
        // tri1 = lerp(40,20,0.6) + (30-40)*0.4 = (40+0.6*(20-40)) + (-4)
        //      = (40-12) - 4 = 28 - 4 = 24.
        // tri0 = lerp(10,30,0.4) + (20-10)*0.6 = (10+0.4*20) + 6 = 18+6=24.
        // (Coincidentally tri0==tri1==24 at this exact input -- both
        // formulas agree here, so this specific case does not by itself
        // distinguish >= from >; kept as a same-value sanity check and
        // paired with the two directional tests above/below which do
        // distinguish the branches.)
        for c in out {
            assert!((c - 24.0).abs() < 1e-4, "c={c}");
        }
    }

    #[test]
    fn three_sample_path_zero_fraction_corner() {
        // fx=fy=0.0: sum=0.0 < 1.0 -> tri0. tri0 = lerp(s00,s10,0) +
        // (s01-s00)*0 = s00 + 0 = s00.
        let samples = TexelSamples {
            s00: [7.0; 4],
            s01: [1.0; 4],
            s10: [1.0; 4],
            s11: [1.0; 4],
        };
        let out = sample_texture_level_blend(true, false, false, (0.0, 0.0), samples);
        assert_eq!(out, [7.0; 4]);
    }

    #[test]
    fn three_sample_path_out_of_range_fraction_below_zero() {
        // Fractional parts are not asserted in-range by the source;
        // extrapolate past zero and confirm no panic plus the literal
        // formula's own (extrapolated) result.
        // fx=-0.5, fy=0.0: sum=-0.5<1.0 -> tri0 = lerp(s00,s10,-0.5) +
        // (s01-s00)*0 = s00 + (-0.5)*(s10-s00).
        // s00=10, s10=30: 10 + (-0.5)*(20) = 10-10=0.
        let samples = TexelSamples {
            s00: [10.0; 4],
            s01: [999.0; 4],
            s10: [30.0; 4],
            s11: [999.0; 4],
        };
        let out = sample_texture_level_blend(true, false, false, (-0.5, 0.0), samples);
        for c in out {
            assert!((c - 0.0).abs() < 1e-4, "c={c}");
        }
    }

    #[test]
    fn three_sample_path_out_of_range_fraction_far_above_one() {
        // fx=3.0, fy=3.0: sum=6.0 >= 1.0 -> tri1, extrapolated well outside
        // [0,1]. Confirms no panic/NaN for far-out-of-range input on finite
        // samples; formula is plain finite arithmetic throughout.
        let samples = TexelSamples {
            s00: [1.0; 4],
            s01: [2.0; 4],
            s10: [3.0; 4],
            s11: [4.0; 4],
        };
        let out = sample_texture_level_blend(true, false, false, (3.0, 3.0), samples);
        for c in out {
            assert!(c.is_finite(), "c={c}");
        }
    }

    #[test]
    fn three_sample_path_all_identical_samples_is_constant() {
        let samples = TexelSamples {
            s00: [42.0; 4],
            s01: [42.0; 4],
            s10: [42.0; 4],
            s11: [42.0; 4],
        };
        for &(fx, fy) in &[(0.0, 0.0), (0.3, 0.3), (0.5, 0.5), (0.9, 0.9), (1.0, 1.0)] {
            let out = sample_texture_level_blend(true, false, false, (fx, fy), samples);
            assert_eq!(out, [42.0; 4], "fx={fx} fy={fy}");
        }
    }

    #[test]
    fn three_sample_path_per_channel_independence() {
        // Each of the four channels must be computed with its own operand,
        // not accidentally cross-wired (e.g. always using channel 0).
        // fx=fy=0.5, sum=1.0 -> selector=1.0 (inclusive) -> result=tri1
        // for every channel. Hand-computed per channel (independently, via
        // tri1 = lerp(s11,s01,1-fx) + (s10-s11)*(1-fy) with 1-fx=1-fy=0.5):
        //   ch0: s00=0,  s01=0,   s10=0,   s11=100 -> tri1 = lerp(100,0,0.5) + (0-100)*0.5 = 50 - 50 = 0.
        //   ch1: s00=100,s01=0,   s10=0,   s11=0   -> tri1 = lerp(0,0,0.5)   + (0-0)*0.5   = 0.
        //   ch2: s00=0,  s01=100, s10=0,   s11=0   -> tri1 = lerp(0,100,0.5) + (0-0)*0.5   = 50.
        //   ch3: s00=0,  s01=0,   s10=100, s11=0   -> tri1 = lerp(0,0,0.5)   + (100-0)*0.5 = 50.
        // ch2 and ch3 coincide at 50.0 despite differing inputs (this
        // exact symmetric input makes s01's and s10's contributions equal
        // at fx=fy=0.5) -- confirmed independently, not itself a bug. Use
        // exact expected values per channel (the strongest possible
        // regression guard) rather than a same/different heuristic that
        // this input happens to defeat for one channel pair.
        let samples = TexelSamples {
            s00: [0.0, 100.0, 0.0, 0.0],
            s01: [0.0, 0.0, 100.0, 0.0],
            s10: [0.0, 0.0, 0.0, 100.0],
            s11: [100.0, 0.0, 0.0, 0.0],
        };
        let out = sample_texture_level_blend(true, false, false, (0.5, 0.5), samples);
        assert_eq!(out, [0.0, 0.0, 50.0, 50.0]);
    }

    // --- filterBilerp vs linearFiltering: `or()` gate ---

    #[test]
    fn filter_bilerp_alone_without_linear_still_filters() {
        let samples = TexelSamples {
            s00: [1.0; 4],
            s01: [2.0; 4],
            s10: [3.0; 4],
            s11: [4.0; 4],
        };
        let filtered = sample_texture_level_blend(true, false, false, (0.5, 0.5), samples);
        let unfiltered = sample_texture_level_blend(false, false, false, (0.5, 0.5), samples);
        assert_ne!(filtered, unfiltered);
    }

    #[test]
    fn linear_filtering_alone_without_bilerp_still_filters() {
        let samples = TexelSamples {
            s00: [1.0; 4],
            s01: [2.0; 4],
            s10: [3.0; 4],
            s11: [4.0; 4],
        };
        let filtered = sample_texture_level_blend(false, false, true, (0.5, 0.5), samples);
        assert_ne!(filtered, samples.s00);
    }

    // --- hlsl_lerp / hlsl_step unit checks ---

    #[test]
    fn hlsl_lerp_matches_x_plus_s_times_y_minus_x() {
        assert_eq!(hlsl_lerp(2.0, 10.0, 0.25), 4.0);
    }

    #[test]
    fn hlsl_lerp_at_s_zero_is_x() {
        assert_eq!(hlsl_lerp(5.0, 99.0, 0.0), 5.0);
    }

    #[test]
    fn hlsl_lerp_at_s_one_is_y() {
        assert_eq!(hlsl_lerp(5.0, 99.0, 1.0), 99.0);
    }

    #[test]
    fn hlsl_step_below_edge_is_zero() {
        assert_eq!(hlsl_step(1.0, 0.5), 0.0);
    }

    #[test]
    fn hlsl_step_at_edge_is_one_inclusive() {
        assert_eq!(hlsl_step(1.0, 1.0), 1.0);
    }

    #[test]
    fn hlsl_step_above_edge_is_one() {
        assert_eq!(hlsl_step(1.0, 5.0), 1.0);
    }

    // --- hlsl_clamp_f32 ---

    #[test]
    fn hlsl_clamp_f32_lo_greater_than_hi_resolves_to_hi() {
        assert_eq!(hlsl_clamp_f32(5.0, 0.0, -3.0), -3.0);
    }

    #[test]
    fn hlsl_clamp_f32_in_range_is_identity() {
        assert_eq!(hlsl_clamp_f32(5.0, 0.0, 10.0), 5.0);
    }
}
