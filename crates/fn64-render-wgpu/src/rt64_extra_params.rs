//! `interop::ExtraParams`' shared layout, the `RT64_ATTRIBUTE_*` bit masks and
//! `ExtraParams::applyExtraAttributes`: a literal port of the permitted MIT
//! RT64 source pinned at commit
//! `5473732a822a4423b5696e7cb18fecc425a59875` (`docs/RT64-PORT-AUTHORITY.md`),
//! `src/shared/rt64_extra_params.h` (SHA-256 of the whole file,
//! `4dbd323ad05fbf6cf8fdab6cb6bdb62f10a460981e91f0f31c9708b91291a90f`, 131
//! newline-terminated lines plus a final unterminated line -- the trailing
//! `#endif` -- which the inventory records as 132). That digest was computed
//! independently here with `shasum -a 256` against the pinned checkout at
//! `src/shared/rt64_extra_params.h` and cross-checked verbatim against
//! `docs/rt64-port-inventory.json`'s
//! `files[path="src/shared/rt64_extra_params.h"].sources.port.sha256`, which
//! records the identical digest -- no mismatch. (The inventory's
//! `sources.oracle.sha256` for this path records the same digest, so the
//! oracle and port trees agree on this file byte for byte.)
//!
//! This is a **full port** of the cited file: the header's entire content is
//! the 19 `RT64_ATTRIBUTE_*` macros, the `ExtraParams` struct's 20 fields, and
//! `applyExtraAttributes`. All three are ported here; nothing in the file is
//! left unported. See "Nonclaims" for the one construct that is deliberately
//! *not* modelled as a Rust language feature (the `#ifdef HLSL_CPU` /
//! `namespace interop` scaffolding), which is preprocessor plumbing rather
//! than behavior.
//!
//! `docs/rt64-port-inventory.json` does not yet record this path's
//! `ported_as` as pointing at this module (it currently lists
//! `"ported_as": []`) -- `scripts/lint-docs.py`'s inventory scanner is
//! expected to report a `ported_as` drift for that until a follow-up
//! regenerates the inventory to add this module; this card's writable surface
//! does not include `docs/rt64-port-inventory.json`, so that reconciliation is
//! deliberately left to the owning ticket rather than done here. Note also
//! that the inventory's whole-file digest marks a source `ported` at **file**
//! granularity: a partial port of a large file would still be credited in full
//! by that mechanism. This particular module happens to be a genuine full
//! port, so no over-credit arises here, but the granularity caveat is recorded
//! because the burndown is known to over-credit for exactly this reason.
//!
//! ## Ported / refused boundary, and the criterion
//!
//! **Criterion**: a construct is ported when its behavior is fully determined
//! by values and control flow present in the cited file -- no GPU, no ImGui
//! context, no type from an uncited file.
//!
//! **Ported** (all three constructs in the file):
//! - the 19 `RT64_ATTRIBUTE_*` macros (lines 9-27), as `u32` constants.
//! - `struct ExtraParams`' 20-field layout (lines 31-51).
//! - `ExtraParams::applyExtraAttributes` (lines 54-127), all 18 branches.
//!
//! **Refused / not modelled** (named, per the card's requirement):
//! - the `#ifdef HLSL_CPU` / `#ifdef HLSL_CPU namespace interop { ... };`
//!   scaffolding (lines 29-30, 52-53, 128-131). This is preprocessor and
//!   namespace plumbing selecting whether the file is compiled as C++ or as
//!   HLSL; it has no runtime behavior. `applyExtraAttributes` exists **only**
//!   in the `HLSL_CPU` build -- the HLSL side gets the plain data struct with
//!   no method. The Rust port carries the CPU-side view, which is the one with
//!   observable behavior.
//! - the *shared GPU layout* claim. `ExtraParams` is an HLSL/C++ interop
//!   struct, so upstream its field order additionally constitutes a constant
//!   buffer layout that a shader reads. This port claims the field set, the
//!   field order, and `applyExtraAttributes`' semantics; it does **not** claim
//!   byte offsets, HLSL packing rules, or `#pragma pack` equivalence, and the
//!   Rust struct carries no `repr(C)`. Reproducing HLSL's float3-straddling
//!   packing rules would need `rt64_hlsl.h`'s alignment behavior verified
//!   against a real shader compile, which is a GPU concern this card refuses.
//! - `float3` / `float4` / `uint`'s *definitions*, which live in the uncited
//!   `src/shared/rt64_hlsl.h`. Only the one fact needed to give the fields
//!   Rust types is admitted, and it is cited inline below.
//!
//! ## Verbatim key logic
//!
//! ```text
//! // rt64_extra_params.h lines 9-27 (attribute masks)
//! #define RT64_ATTRIBUTE_NONE                         0x00000
//! #define RT64_ATTRIBUTE_IGNORE_NORMAL_FACTOR         0x00001
//! #define RT64_ATTRIBUTE_UV_DETAIL_SCALE              0x00002
//! #define RT64_ATTRIBUTE_REFLECTION_FACTOR            0x00004
//! #define RT64_ATTRIBUTE_REFLECTION_FRESNEL_FACTOR    0x00008
//! #define RT64_ATTRIBUTE_ROUGHNESS_FACTOR             0x00010
//! #define RT64_ATTRIBUTE_REFRACTION_FACTOR            0x00020
//! #define RT64_ATTRIBUTE_SHADOW_CATCHER_FACTOR        0x00040
//! #define RT64_ATTRIBUTE_SPECULAR_COLOR               0x00080
//! #define RT64_ATTRIBUTE_SPECULAR_EXPONENT            0x00100
//! #define RT64_ATTRIBUTE_SOLID_ALPHA_MULTIPLIER       0x00200
//! #define RT64_ATTRIBUTE_SHADOW_ALPHA_MULTIPLIER      0x00400
//! #define RT64_ATTRIBUTE_DEPTH_ORDER_BIAS             0x00800
//! #define RT64_ATTRIBUTE_SHADOW_RAY_BIAS              0x02000
//! #define RT64_ATTRIBUTE_SELF_LIGHT                   0x04000
//! #define RT64_ATTRIBUTE_LIGHT_GROUP_MASK_BITS        0x08000
//! #define RT64_ATTRIBUTE_DIFFUSE_COLOR_MIX            0x10000
//! #define RT64_ATTRIBUTE_RSP_LIGHT_DIFFUSE_MIX        0x20000
//! #define RT64_ATTRIBUTE_DEPTH_DECAL_BIAS             0x40000
//!
//! // rt64_extra_params.h lines 31-51 (struct layout)
//! struct ExtraParams {
//!     float rspLightDiffuseMix;
//!     float lockMask;
//!     float ignoreNormalFactor;
//!     float uvDetailScale;
//!     float reflectionFactor;
//!     float reflectionFresnelFactor;
//!     float roughnessFactor;
//!     float refractionFactor;
//!     float shadowCatcherFactor;
//!     float3 specularColor;
//!     float specularExponent;
//!     float solidAlphaMultiplier;
//!     float shadowAlphaMultiplier;
//!     float depthOrderBias;
//!     float depthDecalBias;
//!     float shadowRayBias;
//!     float3 selfLight;
//!     uint lightGroupMaskBits;
//!     float4 diffuseColorMix;
//!     uint enabledAttributes;
//!
//! // rt64_extra_params.h lines 54-127 (applyExtraAttributes; the 18 branches
//! // are all of this exact shape, shown here in full declaration order)
//! void applyExtraAttributes(const ExtraParams &src) {
//!     if (src.enabledAttributes & RT64_ATTRIBUTE_IGNORE_NORMAL_FACTOR) {
//!         ignoreNormalFactor = src.ignoreNormalFactor;
//!     }
//!
//!     if (src.enabledAttributes & RT64_ATTRIBUTE_UV_DETAIL_SCALE) {
//!         uvDetailScale = src.uvDetailScale;
//!     }
//!
//!     if (src.enabledAttributes & RT64_ATTRIBUTE_REFLECTION_FACTOR) {
//!         reflectionFactor = src.reflectionFactor;
//!     }
//!
//!     if (src.enabledAttributes & RT64_ATTRIBUTE_REFLECTION_FRESNEL_FACTOR) {
//!         reflectionFresnelFactor = src.reflectionFresnelFactor;
//!     }
//!
//!     if (src.enabledAttributes & RT64_ATTRIBUTE_ROUGHNESS_FACTOR) {
//!         roughnessFactor = src.roughnessFactor;
//!     }
//!
//!     if (src.enabledAttributes & RT64_ATTRIBUTE_REFRACTION_FACTOR) {
//!         refractionFactor = src.refractionFactor;
//!     }
//!
//!     if (src.enabledAttributes & RT64_ATTRIBUTE_SHADOW_CATCHER_FACTOR) {
//!         shadowCatcherFactor = src.shadowCatcherFactor;
//!     }
//!
//!     if (src.enabledAttributes & RT64_ATTRIBUTE_SPECULAR_COLOR) {
//!         specularColor = src.specularColor;
//!     }
//!
//!     if (src.enabledAttributes & RT64_ATTRIBUTE_SPECULAR_EXPONENT) {
//!         specularExponent = src.specularExponent;
//!     }
//!
//!     if (src.enabledAttributes & RT64_ATTRIBUTE_SOLID_ALPHA_MULTIPLIER) {
//!         solidAlphaMultiplier = src.solidAlphaMultiplier;
//!     }
//!
//!     if (src.enabledAttributes & RT64_ATTRIBUTE_SHADOW_ALPHA_MULTIPLIER) {
//!         shadowAlphaMultiplier = src.shadowAlphaMultiplier;
//!     }
//!
//!     if (src.enabledAttributes & RT64_ATTRIBUTE_DEPTH_ORDER_BIAS) {
//!         depthOrderBias = src.depthOrderBias;
//!     }
//!
//!     if (src.enabledAttributes & RT64_ATTRIBUTE_DEPTH_DECAL_BIAS) {
//!         depthDecalBias = src.depthDecalBias;
//!     }
//!
//!     if (src.enabledAttributes & RT64_ATTRIBUTE_SHADOW_RAY_BIAS) {
//!         shadowRayBias = src.shadowRayBias;
//!     }
//!
//!     if (src.enabledAttributes & RT64_ATTRIBUTE_SELF_LIGHT) {
//!         selfLight = src.selfLight;
//!     }
//!
//!     if (src.enabledAttributes & RT64_ATTRIBUTE_LIGHT_GROUP_MASK_BITS) {
//!         lightGroupMaskBits = src.lightGroupMaskBits;
//!     }
//!
//!     if (src.enabledAttributes & RT64_ATTRIBUTE_DIFFUSE_COLOR_MIX) {
//!         diffuseColorMix = src.diffuseColorMix;
//!     }
//!
//!     if (src.enabledAttributes & RT64_ATTRIBUTE_RSP_LIGHT_DIFFUSE_MIX) {
//!         rspLightDiffuseMix = src.rspLightDiffuseMix;
//!     }
//! }
//! ```
//!
//! ## Reuse, not new type
//!
//! `interop::float3` and `interop::float4` are represented by
//! [`fn64_render_ir::Vec3`] and [`fn64_render_ir::Vec4`]
//! (`crates/fn64-render-ir/src/rsp_math.rs:42` and `:72`), the workspace's
//! backend-neutral HLSL `float3`/`float4` equivalents, already used for this
//! purpose by sibling modules in this crate -- `rt64_light_estimation.rs`
//! spells its `interop::PointLight` port's vector fields with `Vec3`, and
//! `rt64_preset_material.rs` spells *this very struct*'s `specularColor`,
//! `selfLight` and `diffuseColorMix` with `Vec3`/`Vec4`. No new vector type is
//! introduced and no `fn64-render-ir` edit is made. `interop::uint` is
//! `typedef uint32_t uint` (`src/shared/rt64_hlsl.h:16` -- the single admitted
//! fact from that uncited file), so the two `uint` fields and the attribute
//! masks are `u32`.
//!
//! `ExtraParams` itself is **also** already defined, identically, by
//! `rt64_preset_material.rs` in this crate. That module is another lane's
//! exclusive path and cannot be edited from this card, so this module defines
//! its own [`ExtraParams`] rather than importing across an exclusive boundary.
//! The two definitions were compared field by field and agree exactly (see
//! "Admitted domain"); collapsing them into one shared definition is left to a
//! follow-up that owns both paths.
//!
//! ## Admitted domain
//!
//! - **The `0x01000` gap in the mask sequence** (pinned, not fixed). The
//!   macros ascend by single-bit doubling from `0x00001`
//!   (`IGNORE_NORMAL_FACTOR`) through `0x00800` (`DEPTH_ORDER_BIAS`) -- twelve
//!   consecutive bits, 0 through 11 -- and then the next macro,
//!   `SHADOW_RAY_BIAS`, is `0x02000`, i.e. **bit 13**. Bit 12 (`0x01000`) is
//!   assigned to no attribute at all. The sequence then resumes contiguously:
//!   `0x04000`, `0x08000`, `0x10000`, `0x20000`, `0x40000` (bits 14-18). So
//!   the file defines 18 real attribute bits occupying bits 0-11 and 13-18,
//!   with bit 12 a hole. This is upstream's own numbering; it is reproduced
//!   verbatim rather than compacted, and a test asserts both that bit 12 is
//!   claimed by no mask and that the whole set is otherwise a contiguous
//!   pairwise-disjoint single-bit run. A reader "correcting" `SHADOW_RAY_BIAS`
//!   to `0x01000` would silently change which serialized presets apply which
//!   attribute.
//! - **`RT64_ATTRIBUTE_NONE` is `0x00000`**, a zero mask and not a bit. It is
//!   ported as a constant because it is in the file, but `x & NONE` is always
//!   `0`, so it can never enable a branch. A test pins that.
//! - **19 macros, 18 branches, 20 fields** -- three different counts, all
//!   genuine:
//!   - 19 macros = 18 attribute bits + `NONE`.
//!   - 18 branches: every attribute bit has exactly one branch. `NONE` has
//!     none, since it selects nothing.
//!   - 20 fields: the 18 branch-writable fields plus **`lockMask`**, which no
//!     branch and no macro mentions, and which `applyExtraAttributes`
//!     therefore never writes. (`rt64_preset_material.cpp`'s `from_json` /
//!     `to_json` do not mention it either -- it has no JSON key. It is written
//!     only by the constructor's whole-struct `memset`.) A test asserts
//!     `lockMask` survives `applyExtraAttributes` with **all** bits set.
//! - **`applyExtraAttributes` merges into `self` from `src`, and every guard
//!   reads `src.enabledAttributes`** -- never `self.enabledAttributes`. The
//!   destination's own attribute set is irrelevant to which fields get
//!   overwritten. Equally, **`enabledAttributes` itself is never assigned**:
//!   after the call, `self.enabledAttributes` still holds its pre-call value,
//!   not `src`'s. Both facts are pinned by test, because "apply the incoming
//!   preset" reads as if it should adopt the incoming enable set too, and it
//!   does not.
//! - **Branch order vs. field order.** The 18 branches run in the struct's
//!   declaration order *of the fields they write*, with two exceptions that
//!   are pinned as written: `DEPTH_DECAL_BIAS` is tested **before**
//!   `SHADOW_RAY_BIAS` (the struct declares `depthDecalBias` then
//!   `shadowRayBias`, so this one matches), while `rspLightDiffuseMix` -- the
//!   struct's **first** field -- is written by the **last** branch. Since the
//!   18 branches write 18 *distinct* fields and each guard reads only
//!   `src.enabledAttributes` (never a field the loop body has already
//!   written), the order is not observable: no branch can affect another's
//!   guard or destination. Order is preserved anyway, per the literal-port
//!   rule, and a test asserts that applying with all bits set yields the same
//!   result as `src` for all 18 fields regardless.
//! - **The guard is `& mask`, C++ integer-to-bool contextual conversion**, so
//!   it fires on any nonzero result. Each mask is a single bit (or zero), so
//!   `!= 0` is exact; the Rust port writes `!= 0` explicitly. There is no
//!   `== mask` comparison to get wrong, and no `>`/`>=` comparison anywhere in
//!   this file -- the card's comparison-strictness hazard has no instances
//!   here, and no `&&`/`||` predicate either: all 18 guards are single-term.
//! - **This header declares no defaults at all.** It has no constructor, no
//!   default member initializers, and no `= {}`. A C++
//!   `interop::ExtraParams e;` is therefore *uninitialized*, not zeroed. This
//!   matters for the known three-way question: `rt64_preset_material.cpp`'s
//!   constructor zeroes the struct via `memset`, while its `from_json`
//!   fallback table gives `1` to four fields (`specularColor`'s three
//!   components, `specularExponent`, `solidAlphaMultiplier`,
//!   `shadowAlphaMultiplier`). Those two disagree with each other -- but the
//!   disagreement is **two-way, not three-way**, because this header
//!   contributes no third opinion. It is silent. Consequently this module
//!   deliberately provides **no** `Default` impl and no `zeroed()`
//!   constructor: inventing one here would manufacture exactly the third
//!   default that upstream does not have. Callers construct the struct
//!   explicitly, or use the defaults `rt64_preset_material.rs` ports from the
//!   files that actually declare them.
//!
//! ## Nonclaims
//!
//! - No claim about the struct's **memory layout, size, alignment, or byte
//!   offsets**, and no claim of ABI or constant-buffer compatibility with the
//!   HLSL side or with the C++ `interop::ExtraParams`. The Rust struct is not
//!   `repr(C)`; only the field set, field order, and per-field semantics are
//!   claimed. Field order is preserved as documentation of upstream's
//!   declaration order, not as a layout guarantee.
//! - No claim about the **HLSL (non-`HLSL_CPU`) compilation** of this header,
//!   nor about any shader that reads `ExtraParams`. No GPU is involved and
//!   none was consulted.
//! - No claim about **where `enabledAttributes` is populated**, how presets
//!   are serialized, or how the attribute bits reach this struct. Those live
//!   in `src/preset/*`, outside this card's cited source.
//! - No claim about **`lockMask`'s meaning**. This file only shows that
//!   nothing here writes it.
//! - No claim about the unused **bit 12** having any reserved purpose. The
//!   file shows only that no macro claims it.
//! - No **UB** was found in this file: no array indexing, no signed
//!   arithmetic, no casts, no pointer arithmetic. Every operation is a
//!   single-bit mask test against an unsigned value followed by a
//!   same-type assignment. Nothing here is a deviation from upstream; every
//!   test below pins the original's behavior, not a repair of it.

use fn64_render_ir::{Vec3, Vec4};

/// `RT64_ATTRIBUTE_NONE` (`rt64_extra_params.h:9`). A zero mask, not a bit:
/// `x & NONE` is `0` for every `x`, so this can never enable a branch.
pub const RT64_ATTRIBUTE_NONE: u32 = 0x00000;

/// `RT64_ATTRIBUTE_IGNORE_NORMAL_FACTOR` (`rt64_extra_params.h:10`), bit 0.
pub const RT64_ATTRIBUTE_IGNORE_NORMAL_FACTOR: u32 = 0x00001;

/// `RT64_ATTRIBUTE_UV_DETAIL_SCALE` (`rt64_extra_params.h:11`), bit 1.
pub const RT64_ATTRIBUTE_UV_DETAIL_SCALE: u32 = 0x00002;

/// `RT64_ATTRIBUTE_REFLECTION_FACTOR` (`rt64_extra_params.h:12`), bit 2.
pub const RT64_ATTRIBUTE_REFLECTION_FACTOR: u32 = 0x00004;

/// `RT64_ATTRIBUTE_REFLECTION_FRESNEL_FACTOR` (`rt64_extra_params.h:13`),
/// bit 3.
pub const RT64_ATTRIBUTE_REFLECTION_FRESNEL_FACTOR: u32 = 0x00008;

/// `RT64_ATTRIBUTE_ROUGHNESS_FACTOR` (`rt64_extra_params.h:14`), bit 4.
pub const RT64_ATTRIBUTE_ROUGHNESS_FACTOR: u32 = 0x00010;

/// `RT64_ATTRIBUTE_REFRACTION_FACTOR` (`rt64_extra_params.h:15`), bit 5.
pub const RT64_ATTRIBUTE_REFRACTION_FACTOR: u32 = 0x00020;

/// `RT64_ATTRIBUTE_SHADOW_CATCHER_FACTOR` (`rt64_extra_params.h:16`), bit 6.
pub const RT64_ATTRIBUTE_SHADOW_CATCHER_FACTOR: u32 = 0x00040;

/// `RT64_ATTRIBUTE_SPECULAR_COLOR` (`rt64_extra_params.h:17`), bit 7.
pub const RT64_ATTRIBUTE_SPECULAR_COLOR: u32 = 0x00080;

/// `RT64_ATTRIBUTE_SPECULAR_EXPONENT` (`rt64_extra_params.h:18`), bit 8.
pub const RT64_ATTRIBUTE_SPECULAR_EXPONENT: u32 = 0x00100;

/// `RT64_ATTRIBUTE_SOLID_ALPHA_MULTIPLIER` (`rt64_extra_params.h:19`), bit 9.
pub const RT64_ATTRIBUTE_SOLID_ALPHA_MULTIPLIER: u32 = 0x00200;

/// `RT64_ATTRIBUTE_SHADOW_ALPHA_MULTIPLIER` (`rt64_extra_params.h:20`),
/// bit 10.
pub const RT64_ATTRIBUTE_SHADOW_ALPHA_MULTIPLIER: u32 = 0x00400;

/// `RT64_ATTRIBUTE_DEPTH_ORDER_BIAS` (`rt64_extra_params.h:21`), bit 11. The
/// last macro before upstream's bit-12 gap.
pub const RT64_ATTRIBUTE_DEPTH_ORDER_BIAS: u32 = 0x00800;

/// `RT64_ATTRIBUTE_SHADOW_RAY_BIAS` (`rt64_extra_params.h:22`), bit **13**,
/// not bit 12. Upstream skips `0x01000` entirely; see the module doc's
/// "`0x01000` gap" note. Pinned as written.
pub const RT64_ATTRIBUTE_SHADOW_RAY_BIAS: u32 = 0x02000;

/// `RT64_ATTRIBUTE_SELF_LIGHT` (`rt64_extra_params.h:23`), bit 14.
pub const RT64_ATTRIBUTE_SELF_LIGHT: u32 = 0x04000;

/// `RT64_ATTRIBUTE_LIGHT_GROUP_MASK_BITS` (`rt64_extra_params.h:24`), bit 15.
pub const RT64_ATTRIBUTE_LIGHT_GROUP_MASK_BITS: u32 = 0x08000;

/// `RT64_ATTRIBUTE_DIFFUSE_COLOR_MIX` (`rt64_extra_params.h:25`), bit 16.
pub const RT64_ATTRIBUTE_DIFFUSE_COLOR_MIX: u32 = 0x10000;

/// `RT64_ATTRIBUTE_RSP_LIGHT_DIFFUSE_MIX` (`rt64_extra_params.h:26`), bit 17.
pub const RT64_ATTRIBUTE_RSP_LIGHT_DIFFUSE_MIX: u32 = 0x20000;

/// `RT64_ATTRIBUTE_DEPTH_DECAL_BIAS` (`rt64_extra_params.h:27`), bit 18.
pub const RT64_ATTRIBUTE_DEPTH_DECAL_BIAS: u32 = 0x40000;

/// Literal port of `interop::ExtraParams` (`rt64_extra_params.h:31-51`), in
/// upstream declaration order. `float3`/`float4` reuse
/// [`fn64_render_ir::Vec3`] / [`fn64_render_ir::Vec4`]; `interop::uint` is
/// `uint32_t` (`rt64_hlsl.h:16`), so `u32`.
///
/// Deliberately **no** `Default` impl: the cited header declares no defaults
/// of any kind (see the module doc's "This header declares no defaults at
/// all"), and inventing one would manufacture a third default that upstream
/// does not have.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct ExtraParams {
    pub rsp_light_diffuse_mix: f32,
    /// Mentioned by no `RT64_ATTRIBUTE_*` macro and written by no
    /// [`ExtraParams::apply_extra_attributes`] branch. It survives the merge
    /// unchanged even with every attribute bit set.
    pub lock_mask: f32,
    pub ignore_normal_factor: f32,
    pub uv_detail_scale: f32,
    pub reflection_factor: f32,
    pub reflection_fresnel_factor: f32,
    pub roughness_factor: f32,
    pub refraction_factor: f32,
    pub shadow_catcher_factor: f32,
    pub specular_color: Vec3,
    pub specular_exponent: f32,
    pub solid_alpha_multiplier: f32,
    pub shadow_alpha_multiplier: f32,
    pub depth_order_bias: f32,
    pub depth_decal_bias: f32,
    pub shadow_ray_bias: f32,
    pub self_light: Vec3,
    pub light_group_mask_bits: u32,
    pub diffuse_color_mix: Vec4,
    pub enabled_attributes: u32,
}

impl ExtraParams {
    /// Literal port of `ExtraParams::applyExtraAttributes`
    /// (`rt64_extra_params.h:54-127`), all 18 branches in upstream order.
    ///
    /// Every guard reads **`src.enabled_attributes`**, never `self`'s, and
    /// `self.enabled_attributes` is itself never assigned -- after the call it
    /// still holds its pre-call value. `lock_mask` is never written by any
    /// branch.
    pub fn apply_extra_attributes(&mut self, src: &ExtraParams) {
        if src.enabled_attributes & RT64_ATTRIBUTE_IGNORE_NORMAL_FACTOR != 0 {
            self.ignore_normal_factor = src.ignore_normal_factor;
        }

        if src.enabled_attributes & RT64_ATTRIBUTE_UV_DETAIL_SCALE != 0 {
            self.uv_detail_scale = src.uv_detail_scale;
        }

        if src.enabled_attributes & RT64_ATTRIBUTE_REFLECTION_FACTOR != 0 {
            self.reflection_factor = src.reflection_factor;
        }

        if src.enabled_attributes & RT64_ATTRIBUTE_REFLECTION_FRESNEL_FACTOR != 0 {
            self.reflection_fresnel_factor = src.reflection_fresnel_factor;
        }

        if src.enabled_attributes & RT64_ATTRIBUTE_ROUGHNESS_FACTOR != 0 {
            self.roughness_factor = src.roughness_factor;
        }

        if src.enabled_attributes & RT64_ATTRIBUTE_REFRACTION_FACTOR != 0 {
            self.refraction_factor = src.refraction_factor;
        }

        if src.enabled_attributes & RT64_ATTRIBUTE_SHADOW_CATCHER_FACTOR != 0 {
            self.shadow_catcher_factor = src.shadow_catcher_factor;
        }

        if src.enabled_attributes & RT64_ATTRIBUTE_SPECULAR_COLOR != 0 {
            self.specular_color = src.specular_color;
        }

        if src.enabled_attributes & RT64_ATTRIBUTE_SPECULAR_EXPONENT != 0 {
            self.specular_exponent = src.specular_exponent;
        }

        if src.enabled_attributes & RT64_ATTRIBUTE_SOLID_ALPHA_MULTIPLIER != 0 {
            self.solid_alpha_multiplier = src.solid_alpha_multiplier;
        }

        if src.enabled_attributes & RT64_ATTRIBUTE_SHADOW_ALPHA_MULTIPLIER != 0 {
            self.shadow_alpha_multiplier = src.shadow_alpha_multiplier;
        }

        if src.enabled_attributes & RT64_ATTRIBUTE_DEPTH_ORDER_BIAS != 0 {
            self.depth_order_bias = src.depth_order_bias;
        }

        if src.enabled_attributes & RT64_ATTRIBUTE_DEPTH_DECAL_BIAS != 0 {
            self.depth_decal_bias = src.depth_decal_bias;
        }

        if src.enabled_attributes & RT64_ATTRIBUTE_SHADOW_RAY_BIAS != 0 {
            self.shadow_ray_bias = src.shadow_ray_bias;
        }

        if src.enabled_attributes & RT64_ATTRIBUTE_SELF_LIGHT != 0 {
            self.self_light = src.self_light;
        }

        if src.enabled_attributes & RT64_ATTRIBUTE_LIGHT_GROUP_MASK_BITS != 0 {
            self.light_group_mask_bits = src.light_group_mask_bits;
        }

        if src.enabled_attributes & RT64_ATTRIBUTE_DIFFUSE_COLOR_MIX != 0 {
            self.diffuse_color_mix = src.diffuse_color_mix;
        }

        if src.enabled_attributes & RT64_ATTRIBUTE_RSP_LIGHT_DIFFUSE_MIX != 0 {
            self.rsp_light_diffuse_mix = src.rsp_light_diffuse_mix;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The 18 real attribute bits, in the macro-declaration order of the
    /// cited file. `RT64_ATTRIBUTE_NONE` is excluded: it is a zero mask, not
    /// a bit.
    const ALL_ATTRIBUTE_BITS: [u32; 18] = [
        RT64_ATTRIBUTE_IGNORE_NORMAL_FACTOR,
        RT64_ATTRIBUTE_UV_DETAIL_SCALE,
        RT64_ATTRIBUTE_REFLECTION_FACTOR,
        RT64_ATTRIBUTE_REFLECTION_FRESNEL_FACTOR,
        RT64_ATTRIBUTE_ROUGHNESS_FACTOR,
        RT64_ATTRIBUTE_REFRACTION_FACTOR,
        RT64_ATTRIBUTE_SHADOW_CATCHER_FACTOR,
        RT64_ATTRIBUTE_SPECULAR_COLOR,
        RT64_ATTRIBUTE_SPECULAR_EXPONENT,
        RT64_ATTRIBUTE_SOLID_ALPHA_MULTIPLIER,
        RT64_ATTRIBUTE_SHADOW_ALPHA_MULTIPLIER,
        RT64_ATTRIBUTE_DEPTH_ORDER_BIAS,
        RT64_ATTRIBUTE_SHADOW_RAY_BIAS,
        RT64_ATTRIBUTE_SELF_LIGHT,
        RT64_ATTRIBUTE_LIGHT_GROUP_MASK_BITS,
        RT64_ATTRIBUTE_DIFFUSE_COLOR_MIX,
        RT64_ATTRIBUTE_RSP_LIGHT_DIFFUSE_MIX,
        RT64_ATTRIBUTE_DEPTH_DECAL_BIAS,
    ];

    /// A destination with a distinct, hand-chosen value in every one of the
    /// 20 fields, so any accidental write is visible. Values are the
    /// "base" side of every merge test.
    fn dst_extra_params() -> ExtraParams {
        ExtraParams {
            rsp_light_diffuse_mix: 1.0f32,
            lock_mask: 2.0f32,
            ignore_normal_factor: 3.0f32,
            uv_detail_scale: 4.0f32,
            reflection_factor: 5.0f32,
            reflection_fresnel_factor: 6.0f32,
            roughness_factor: 7.0f32,
            refraction_factor: 8.0f32,
            shadow_catcher_factor: 9.0f32,
            specular_color: Vec3::new(10.0f32, 11.0f32, 12.0f32),
            specular_exponent: 13.0f32,
            solid_alpha_multiplier: 14.0f32,
            shadow_alpha_multiplier: 15.0f32,
            depth_order_bias: 16.0f32,
            depth_decal_bias: 17.0f32,
            shadow_ray_bias: 18.0f32,
            self_light: Vec3::new(19.0f32, 20.0f32, 21.0f32),
            light_group_mask_bits: 22u32,
            diffuse_color_mix: Vec4::new(23.0f32, 24.0f32, 25.0f32, 26.0f32),
            enabled_attributes: 27u32,
        }
    }

    /// A source with 20 values disjoint from [`dst_extra_params`]'s, so
    /// "was it copied?" is decidable per field. `enabled_attributes` is set
    /// by each test.
    fn src_extra_params(enabled_attributes: u32) -> ExtraParams {
        ExtraParams {
            rsp_light_diffuse_mix: 101.0f32,
            lock_mask: 102.0f32,
            ignore_normal_factor: 103.0f32,
            uv_detail_scale: 104.0f32,
            reflection_factor: 105.0f32,
            reflection_fresnel_factor: 106.0f32,
            roughness_factor: 107.0f32,
            refraction_factor: 108.0f32,
            shadow_catcher_factor: 109.0f32,
            specular_color: Vec3::new(110.0f32, 111.0f32, 112.0f32),
            specular_exponent: 113.0f32,
            solid_alpha_multiplier: 114.0f32,
            shadow_alpha_multiplier: 115.0f32,
            depth_order_bias: 116.0f32,
            depth_decal_bias: 117.0f32,
            shadow_ray_bias: 118.0f32,
            self_light: Vec3::new(119.0f32, 120.0f32, 121.0f32),
            light_group_mask_bits: 122u32,
            diffuse_color_mix: Vec4::new(123.0f32, 124.0f32, 125.0f32, 126.0f32),
            enabled_attributes,
        }
    }

    /// Merge with exactly one bit set, and return the result.
    fn merge_with_only(bit: u32) -> ExtraParams {
        let mut dst = dst_extra_params();
        dst.apply_extra_attributes(&src_extra_params(bit));
        dst
    }

    // ---- mask values (hand-transcribed from rt64_extra_params.h:9-27) ----

    #[test]
    fn attribute_none_is_zero() {
        assert_eq!(RT64_ATTRIBUTE_NONE, 0x00000u32);
    }

    #[test]
    fn attribute_ignore_normal_factor_is_bit_zero() {
        assert_eq!(RT64_ATTRIBUTE_IGNORE_NORMAL_FACTOR, 0x00001u32);
    }

    #[test]
    fn attribute_uv_detail_scale_is_bit_one() {
        assert_eq!(RT64_ATTRIBUTE_UV_DETAIL_SCALE, 0x00002u32);
    }

    #[test]
    fn attribute_reflection_factor_is_bit_two() {
        assert_eq!(RT64_ATTRIBUTE_REFLECTION_FACTOR, 0x00004u32);
    }

    #[test]
    fn attribute_reflection_fresnel_factor_is_bit_three() {
        assert_eq!(RT64_ATTRIBUTE_REFLECTION_FRESNEL_FACTOR, 0x00008u32);
    }

    #[test]
    fn attribute_roughness_factor_is_bit_four() {
        assert_eq!(RT64_ATTRIBUTE_ROUGHNESS_FACTOR, 0x00010u32);
    }

    #[test]
    fn attribute_refraction_factor_is_bit_five() {
        assert_eq!(RT64_ATTRIBUTE_REFRACTION_FACTOR, 0x00020u32);
    }

    #[test]
    fn attribute_shadow_catcher_factor_is_bit_six() {
        assert_eq!(RT64_ATTRIBUTE_SHADOW_CATCHER_FACTOR, 0x00040u32);
    }

    #[test]
    fn attribute_specular_color_is_bit_seven() {
        assert_eq!(RT64_ATTRIBUTE_SPECULAR_COLOR, 0x00080u32);
    }

    #[test]
    fn attribute_specular_exponent_is_bit_eight() {
        assert_eq!(RT64_ATTRIBUTE_SPECULAR_EXPONENT, 0x00100u32);
    }

    #[test]
    fn attribute_solid_alpha_multiplier_is_bit_nine() {
        assert_eq!(RT64_ATTRIBUTE_SOLID_ALPHA_MULTIPLIER, 0x00200u32);
    }

    #[test]
    fn attribute_shadow_alpha_multiplier_is_bit_ten() {
        assert_eq!(RT64_ATTRIBUTE_SHADOW_ALPHA_MULTIPLIER, 0x00400u32);
    }

    #[test]
    fn attribute_depth_order_bias_is_bit_eleven() {
        assert_eq!(RT64_ATTRIBUTE_DEPTH_ORDER_BIAS, 0x00800u32);
    }

    /// Upstream skips `0x01000`: `SHADOW_RAY_BIAS` is bit 13, not bit 12.
    /// Pinned, not fixed.
    #[test]
    fn attribute_shadow_ray_bias_is_bit_thirteen_skipping_bit_twelve() {
        assert_eq!(RT64_ATTRIBUTE_SHADOW_RAY_BIAS, 0x02000u32);
        assert_ne!(RT64_ATTRIBUTE_SHADOW_RAY_BIAS, 0x01000u32);
        assert_eq!(
            RT64_ATTRIBUTE_SHADOW_RAY_BIAS,
            RT64_ATTRIBUTE_DEPTH_ORDER_BIAS << 2
        );
    }

    #[test]
    fn attribute_self_light_is_bit_fourteen() {
        assert_eq!(RT64_ATTRIBUTE_SELF_LIGHT, 0x04000u32);
    }

    #[test]
    fn attribute_light_group_mask_bits_is_bit_fifteen() {
        assert_eq!(RT64_ATTRIBUTE_LIGHT_GROUP_MASK_BITS, 0x08000u32);
    }

    #[test]
    fn attribute_diffuse_color_mix_is_bit_sixteen() {
        assert_eq!(RT64_ATTRIBUTE_DIFFUSE_COLOR_MIX, 0x10000u32);
    }

    #[test]
    fn attribute_rsp_light_diffuse_mix_is_bit_seventeen() {
        assert_eq!(RT64_ATTRIBUTE_RSP_LIGHT_DIFFUSE_MIX, 0x20000u32);
    }

    #[test]
    fn attribute_depth_decal_bias_is_bit_eighteen() {
        assert_eq!(RT64_ATTRIBUTE_DEPTH_DECAL_BIAS, 0x40000u32);
    }

    // ---- mask set structure ----

    #[test]
    fn every_attribute_mask_is_a_single_bit() {
        for (i, mask) in ALL_ATTRIBUTE_BITS.iter().enumerate() {
            assert_eq!(
                mask.count_ones(),
                1,
                "mask at declaration index {i} is not a single bit"
            );
        }
    }

    #[test]
    fn attribute_masks_are_pairwise_disjoint() {
        for (i, a) in ALL_ATTRIBUTE_BITS.iter().enumerate() {
            for (j, b) in ALL_ATTRIBUTE_BITS.iter().enumerate() {
                if i != j {
                    assert_eq!(a & b, 0u32, "masks {i} and {j} overlap");
                }
            }
        }
    }

    /// The union of all 18 bits is bits 0-11 and 13-18: `0x7EFFF`. Bit 12
    /// (`0x01000`) is absent. Hand-derived two ways: bits 0-11 are `0x00FFF`
    /// and bits 13-18 are `0x7E000`, whose union is `0x7EFFF`; equivalently
    /// `0x7FFFF` (bits 0-18, all set) with `0x01000` cleared.
    #[test]
    fn union_of_all_attribute_masks_has_the_bit_twelve_hole() {
        let union = ALL_ATTRIBUTE_BITS.iter().fold(0u32, |acc, m| acc | m);
        assert_eq!(union, 0x7EFFFu32);
        assert_eq!(union, 0x00FFFu32 | 0x7E000u32);
        assert_eq!(union, 0x7FFFFu32 & !0x01000u32);
        assert_eq!(union.count_ones(), 18);
    }

    #[test]
    fn bit_twelve_is_claimed_by_no_attribute_mask() {
        for mask in ALL_ATTRIBUTE_BITS {
            assert_eq!(mask & 0x01000u32, 0u32);
        }
        assert_eq!(RT64_ATTRIBUTE_NONE & 0x01000u32, 0u32);
    }

    /// Bits 0 through 11 are contiguous doublings from
    /// `IGNORE_NORMAL_FACTOR`; the run stops at `DEPTH_ORDER_BIAS`.
    #[test]
    fn first_twelve_masks_are_a_contiguous_doubling_run() {
        for i in 0..11usize {
            assert_eq!(
                ALL_ATTRIBUTE_BITS[i + 1],
                ALL_ATTRIBUTE_BITS[i] << 1,
                "declaration index {i} does not double into {}",
                i + 1
            );
        }
        assert_eq!(ALL_ATTRIBUTE_BITS[11], RT64_ATTRIBUTE_DEPTH_ORDER_BIAS);
    }

    /// After the bit-12 hole, bits 13-18 are again contiguous doublings in
    /// *macro* declaration order (`SHADOW_RAY_BIAS`, `SELF_LIGHT`,
    /// `LIGHT_GROUP_MASK_BITS`, `DIFFUSE_COLOR_MIX`,
    /// `RSP_LIGHT_DIFFUSE_MIX`, `DEPTH_DECAL_BIAS`).
    #[test]
    fn masks_after_the_hole_are_a_contiguous_doubling_run() {
        let after = [
            RT64_ATTRIBUTE_SHADOW_RAY_BIAS,
            RT64_ATTRIBUTE_SELF_LIGHT,
            RT64_ATTRIBUTE_LIGHT_GROUP_MASK_BITS,
            RT64_ATTRIBUTE_DIFFUSE_COLOR_MIX,
            RT64_ATTRIBUTE_RSP_LIGHT_DIFFUSE_MIX,
            RT64_ATTRIBUTE_DEPTH_DECAL_BIAS,
        ];
        for i in 0..after.len() - 1 {
            assert_eq!(after[i + 1], after[i] << 1);
        }
        assert_eq!(after[0], 0x02000u32);
        assert_eq!(after[5], 0x40000u32);
    }

    /// The macro count: 18 attribute bits plus `NONE` is 19 macros, against
    /// the struct's 20 fields and `applyExtraAttributes`' 18 branches.
    #[test]
    fn there_are_eighteen_attribute_bits_plus_a_zero_none() {
        assert_eq!(ALL_ATTRIBUTE_BITS.len(), 18);
        assert_eq!(RT64_ATTRIBUTE_NONE.count_ones(), 0);
    }

    // ---- applyExtraAttributes: each branch taken in isolation ----

    #[test]
    fn ignore_normal_factor_branch_copies_only_its_field() {
        let out = merge_with_only(RT64_ATTRIBUTE_IGNORE_NORMAL_FACTOR);
        let mut want = dst_extra_params();
        want.ignore_normal_factor = 103.0f32;
        assert_eq!(out, want);
    }

    #[test]
    fn uv_detail_scale_branch_copies_only_its_field() {
        let out = merge_with_only(RT64_ATTRIBUTE_UV_DETAIL_SCALE);
        let mut want = dst_extra_params();
        want.uv_detail_scale = 104.0f32;
        assert_eq!(out, want);
    }

    #[test]
    fn reflection_factor_branch_copies_only_its_field() {
        let out = merge_with_only(RT64_ATTRIBUTE_REFLECTION_FACTOR);
        let mut want = dst_extra_params();
        want.reflection_factor = 105.0f32;
        assert_eq!(out, want);
    }

    #[test]
    fn reflection_fresnel_factor_branch_copies_only_its_field() {
        let out = merge_with_only(RT64_ATTRIBUTE_REFLECTION_FRESNEL_FACTOR);
        let mut want = dst_extra_params();
        want.reflection_fresnel_factor = 106.0f32;
        assert_eq!(out, want);
    }

    #[test]
    fn roughness_factor_branch_copies_only_its_field() {
        let out = merge_with_only(RT64_ATTRIBUTE_ROUGHNESS_FACTOR);
        let mut want = dst_extra_params();
        want.roughness_factor = 107.0f32;
        assert_eq!(out, want);
    }

    #[test]
    fn refraction_factor_branch_copies_only_its_field() {
        let out = merge_with_only(RT64_ATTRIBUTE_REFRACTION_FACTOR);
        let mut want = dst_extra_params();
        want.refraction_factor = 108.0f32;
        assert_eq!(out, want);
    }

    #[test]
    fn shadow_catcher_factor_branch_copies_only_its_field() {
        let out = merge_with_only(RT64_ATTRIBUTE_SHADOW_CATCHER_FACTOR);
        let mut want = dst_extra_params();
        want.shadow_catcher_factor = 109.0f32;
        assert_eq!(out, want);
    }

    #[test]
    fn specular_color_branch_copies_all_three_components() {
        let out = merge_with_only(RT64_ATTRIBUTE_SPECULAR_COLOR);
        let mut want = dst_extra_params();
        want.specular_color = Vec3::new(110.0f32, 111.0f32, 112.0f32);
        assert_eq!(out, want);
        assert_eq!(out.specular_color.x, 110.0f32);
        assert_eq!(out.specular_color.y, 111.0f32);
        assert_eq!(out.specular_color.z, 112.0f32);
    }

    #[test]
    fn specular_exponent_branch_copies_only_its_field() {
        let out = merge_with_only(RT64_ATTRIBUTE_SPECULAR_EXPONENT);
        let mut want = dst_extra_params();
        want.specular_exponent = 113.0f32;
        assert_eq!(out, want);
    }

    #[test]
    fn solid_alpha_multiplier_branch_copies_only_its_field() {
        let out = merge_with_only(RT64_ATTRIBUTE_SOLID_ALPHA_MULTIPLIER);
        let mut want = dst_extra_params();
        want.solid_alpha_multiplier = 114.0f32;
        assert_eq!(out, want);
    }

    #[test]
    fn shadow_alpha_multiplier_branch_copies_only_its_field() {
        let out = merge_with_only(RT64_ATTRIBUTE_SHADOW_ALPHA_MULTIPLIER);
        let mut want = dst_extra_params();
        want.shadow_alpha_multiplier = 115.0f32;
        assert_eq!(out, want);
    }

    #[test]
    fn depth_order_bias_branch_copies_only_its_field() {
        let out = merge_with_only(RT64_ATTRIBUTE_DEPTH_ORDER_BIAS);
        let mut want = dst_extra_params();
        want.depth_order_bias = 116.0f32;
        assert_eq!(out, want);
    }

    #[test]
    fn depth_decal_bias_branch_copies_only_its_field() {
        let out = merge_with_only(RT64_ATTRIBUTE_DEPTH_DECAL_BIAS);
        let mut want = dst_extra_params();
        want.depth_decal_bias = 117.0f32;
        assert_eq!(out, want);
    }

    #[test]
    fn shadow_ray_bias_branch_copies_only_its_field() {
        let out = merge_with_only(RT64_ATTRIBUTE_SHADOW_RAY_BIAS);
        let mut want = dst_extra_params();
        want.shadow_ray_bias = 118.0f32;
        assert_eq!(out, want);
    }

    #[test]
    fn self_light_branch_copies_all_three_components() {
        let out = merge_with_only(RT64_ATTRIBUTE_SELF_LIGHT);
        let mut want = dst_extra_params();
        want.self_light = Vec3::new(119.0f32, 120.0f32, 121.0f32);
        assert_eq!(out, want);
        assert_eq!(out.self_light.x, 119.0f32);
        assert_eq!(out.self_light.y, 120.0f32);
        assert_eq!(out.self_light.z, 121.0f32);
    }

    #[test]
    fn light_group_mask_bits_branch_copies_only_its_field() {
        let out = merge_with_only(RT64_ATTRIBUTE_LIGHT_GROUP_MASK_BITS);
        let mut want = dst_extra_params();
        want.light_group_mask_bits = 122u32;
        assert_eq!(out, want);
    }

    #[test]
    fn diffuse_color_mix_branch_copies_all_four_components() {
        let out = merge_with_only(RT64_ATTRIBUTE_DIFFUSE_COLOR_MIX);
        let mut want = dst_extra_params();
        want.diffuse_color_mix = Vec4::new(123.0f32, 124.0f32, 125.0f32, 126.0f32);
        assert_eq!(out, want);
        assert_eq!(out.diffuse_color_mix.x, 123.0f32);
        assert_eq!(out.diffuse_color_mix.y, 124.0f32);
        assert_eq!(out.diffuse_color_mix.z, 125.0f32);
        assert_eq!(out.diffuse_color_mix.w, 126.0f32);
    }

    #[test]
    fn rsp_light_diffuse_mix_branch_copies_only_its_field() {
        let out = merge_with_only(RT64_ATTRIBUTE_RSP_LIGHT_DIFFUSE_MIX);
        let mut want = dst_extra_params();
        want.rsp_light_diffuse_mix = 101.0f32;
        assert_eq!(out, want);
    }

    // ---- applyExtraAttributes: each branch NOT taken ----

    /// The negative side of all 18 guards: for each bit, clearing *only* that
    /// bit out of the full set must leave exactly that one field at the
    /// destination's value while all others take the source's.
    #[test]
    fn clearing_one_bit_leaves_exactly_that_field_untouched() {
        let all = ALL_ATTRIBUTE_BITS.iter().fold(0u32, |acc, m| acc | m);
        let full = {
            let mut d = dst_extra_params();
            d.apply_extra_attributes(&src_extra_params(all));
            d
        };
        for bit in ALL_ATTRIBUTE_BITS {
            let mut out = dst_extra_params();
            out.apply_extra_attributes(&src_extra_params(all & !bit));
            assert_ne!(out, full, "clearing {bit:#07x} changed nothing");

            let mut want = full;
            let base = dst_extra_params();
            match bit {
                RT64_ATTRIBUTE_IGNORE_NORMAL_FACTOR => {
                    want.ignore_normal_factor = base.ignore_normal_factor
                }
                RT64_ATTRIBUTE_UV_DETAIL_SCALE => want.uv_detail_scale = base.uv_detail_scale,
                RT64_ATTRIBUTE_REFLECTION_FACTOR => want.reflection_factor = base.reflection_factor,
                RT64_ATTRIBUTE_REFLECTION_FRESNEL_FACTOR => {
                    want.reflection_fresnel_factor = base.reflection_fresnel_factor
                }
                RT64_ATTRIBUTE_ROUGHNESS_FACTOR => want.roughness_factor = base.roughness_factor,
                RT64_ATTRIBUTE_REFRACTION_FACTOR => want.refraction_factor = base.refraction_factor,
                RT64_ATTRIBUTE_SHADOW_CATCHER_FACTOR => {
                    want.shadow_catcher_factor = base.shadow_catcher_factor
                }
                RT64_ATTRIBUTE_SPECULAR_COLOR => want.specular_color = base.specular_color,
                RT64_ATTRIBUTE_SPECULAR_EXPONENT => want.specular_exponent = base.specular_exponent,
                RT64_ATTRIBUTE_SOLID_ALPHA_MULTIPLIER => {
                    want.solid_alpha_multiplier = base.solid_alpha_multiplier
                }
                RT64_ATTRIBUTE_SHADOW_ALPHA_MULTIPLIER => {
                    want.shadow_alpha_multiplier = base.shadow_alpha_multiplier
                }
                RT64_ATTRIBUTE_DEPTH_ORDER_BIAS => want.depth_order_bias = base.depth_order_bias,
                RT64_ATTRIBUTE_DEPTH_DECAL_BIAS => want.depth_decal_bias = base.depth_decal_bias,
                RT64_ATTRIBUTE_SHADOW_RAY_BIAS => want.shadow_ray_bias = base.shadow_ray_bias,
                RT64_ATTRIBUTE_SELF_LIGHT => want.self_light = base.self_light,
                RT64_ATTRIBUTE_LIGHT_GROUP_MASK_BITS => {
                    want.light_group_mask_bits = base.light_group_mask_bits
                }
                RT64_ATTRIBUTE_DIFFUSE_COLOR_MIX => want.diffuse_color_mix = base.diffuse_color_mix,
                RT64_ATTRIBUTE_RSP_LIGHT_DIFFUSE_MIX => {
                    want.rsp_light_diffuse_mix = base.rsp_light_diffuse_mix
                }
                other => panic!("unhandled attribute bit {other:#07x}"),
            }
            // `enabled_attributes` is never written by the merge, so it is
            // the destination's throughout.
            want.enabled_attributes = base.enabled_attributes;
            assert_eq!(out, want, "clearing {bit:#07x} touched the wrong field");
        }
    }

    /// Each single-bit merge writes exactly one field, so the result differs
    /// from the untouched destination in exactly that field -- and setting a
    /// bit whose *source value equals the destination's* would be
    /// indistinguishable, which is why the two fixtures are disjoint.
    #[test]
    fn each_single_bit_merge_differs_from_the_untouched_destination() {
        let base = dst_extra_params();
        for bit in ALL_ATTRIBUTE_BITS {
            assert_ne!(merge_with_only(bit), base, "{bit:#07x} wrote nothing");
        }
    }

    // ---- zero / NONE / unused-bit guards ----

    #[test]
    fn zero_enabled_attributes_writes_nothing() {
        let out = merge_with_only(0u32);
        assert_eq!(out, dst_extra_params());
    }

    #[test]
    fn attribute_none_writes_nothing() {
        let out = merge_with_only(RT64_ATTRIBUTE_NONE);
        assert_eq!(out, dst_extra_params());
    }

    /// The unclaimed bit 12 enables no branch, because no guard tests it.
    #[test]
    fn the_unused_bit_twelve_writes_nothing() {
        let out = merge_with_only(0x01000u32);
        assert_eq!(out, dst_extra_params());
    }

    /// Bits above the defined range (19..=31) likewise enable nothing.
    #[test]
    fn bits_above_the_defined_range_write_nothing() {
        for shift in 19..32u32 {
            let out = merge_with_only(1u32 << shift);
            assert_eq!(out, dst_extra_params(), "bit {shift} wrote something");
        }
    }

    /// `0xFFFFFFFF` sets every bit including the hole and the undefined high
    /// bits; the result must equal the all-defined-bits merge exactly.
    #[test]
    fn all_ones_matches_the_all_defined_bits_merge() {
        let all = ALL_ATTRIBUTE_BITS.iter().fold(0u32, |acc, m| acc | m);
        let mut defined = dst_extra_params();
        defined.apply_extra_attributes(&src_extra_params(all));

        let mut ones = dst_extra_params();
        ones.apply_extra_attributes(&src_extra_params(0xFFFF_FFFFu32));

        assert_eq!(ones, defined);
    }

    // ---- whole-merge invariants ----

    /// With all 18 bits set, all 18 branch-writable fields take the source's
    /// value. `lock_mask` and `enabled_attributes` do not.
    #[test]
    fn all_bits_set_copies_all_eighteen_writable_fields() {
        let all = ALL_ATTRIBUTE_BITS.iter().fold(0u32, |acc, m| acc | m);
        let mut out = dst_extra_params();
        out.apply_extra_attributes(&src_extra_params(all));

        assert_eq!(out.rsp_light_diffuse_mix, 101.0f32);
        assert_eq!(out.ignore_normal_factor, 103.0f32);
        assert_eq!(out.uv_detail_scale, 104.0f32);
        assert_eq!(out.reflection_factor, 105.0f32);
        assert_eq!(out.reflection_fresnel_factor, 106.0f32);
        assert_eq!(out.roughness_factor, 107.0f32);
        assert_eq!(out.refraction_factor, 108.0f32);
        assert_eq!(out.shadow_catcher_factor, 109.0f32);
        assert_eq!(out.specular_color, Vec3::new(110.0f32, 111.0f32, 112.0f32));
        assert_eq!(out.specular_exponent, 113.0f32);
        assert_eq!(out.solid_alpha_multiplier, 114.0f32);
        assert_eq!(out.shadow_alpha_multiplier, 115.0f32);
        assert_eq!(out.depth_order_bias, 116.0f32);
        assert_eq!(out.depth_decal_bias, 117.0f32);
        assert_eq!(out.shadow_ray_bias, 118.0f32);
        assert_eq!(out.self_light, Vec3::new(119.0f32, 120.0f32, 121.0f32));
        assert_eq!(out.light_group_mask_bits, 122u32);
        assert_eq!(
            out.diffuse_color_mix,
            Vec4::new(123.0f32, 124.0f32, 125.0f32, 126.0f32)
        );
    }

    /// `lockMask` is mentioned by no macro and no branch: it survives a
    /// merge with every bit set.
    #[test]
    fn lock_mask_survives_a_merge_with_every_bit_set() {
        let mut out = dst_extra_params();
        out.apply_extra_attributes(&src_extra_params(0xFFFF_FFFFu32));
        assert_eq!(out.lock_mask, 2.0f32);
        assert_ne!(out.lock_mask, src_extra_params(0).lock_mask);
    }

    /// `enabledAttributes` is never assigned by the merge: the destination
    /// keeps its own, and does *not* adopt the source's.
    #[test]
    fn enabled_attributes_is_never_adopted_from_the_source() {
        let mut out = dst_extra_params();
        out.apply_extra_attributes(&src_extra_params(0xFFFF_FFFFu32));
        assert_eq!(out.enabled_attributes, 27u32);
        assert_ne!(out.enabled_attributes, 0xFFFF_FFFFu32);
    }

    /// Every guard reads `src.enabled_attributes`. The destination's own
    /// attribute set has no effect on the merge.
    #[test]
    fn the_destination_attribute_set_does_not_gate_the_merge() {
        let all = ALL_ATTRIBUTE_BITS.iter().fold(0u32, |acc, m| acc | m);

        let mut with_zero_dst_bits = dst_extra_params();
        with_zero_dst_bits.enabled_attributes = 0u32;
        with_zero_dst_bits.apply_extra_attributes(&src_extra_params(all));

        let mut with_all_dst_bits = dst_extra_params();
        with_all_dst_bits.enabled_attributes = all;
        with_all_dst_bits.apply_extra_attributes(&src_extra_params(all));

        assert_eq!(with_zero_dst_bits.ignore_normal_factor, 103.0f32);
        assert_eq!(with_all_dst_bits.ignore_normal_factor, 103.0f32);
        assert_eq!(with_zero_dst_bits.enabled_attributes, 0u32);
        assert_eq!(with_all_dst_bits.enabled_attributes, all);
    }

    /// A source whose bits are all set but whose *values* equal the
    /// destination's is a no-op, confirming the merge only ever assigns and
    /// never derives.
    #[test]
    fn merging_a_source_with_identical_values_is_a_no_op() {
        let mut src = dst_extra_params();
        src.enabled_attributes = 0xFFFF_FFFFu32;
        let mut out = dst_extra_params();
        out.apply_extra_attributes(&src);
        assert_eq!(out, dst_extra_params());
    }

    /// Applying twice equals applying once: every branch is a plain
    /// assignment with no accumulation.
    #[test]
    fn applying_the_same_source_twice_is_idempotent() {
        let all = ALL_ATTRIBUTE_BITS.iter().fold(0u32, |acc, m| acc | m);
        let src = src_extra_params(all);

        let mut once = dst_extra_params();
        once.apply_extra_attributes(&src);

        let mut twice = dst_extra_params();
        twice.apply_extra_attributes(&src);
        twice.apply_extra_attributes(&src);

        assert_eq!(once, twice);
    }

    /// Applying one bit then another equals applying both at once, in either
    /// sequencing: the 18 branches write disjoint fields, so branch order is
    /// unobservable (the module doc's "Branch order vs. field order").
    #[test]
    fn disjoint_bits_compose_regardless_of_order() {
        let a = RT64_ATTRIBUTE_RSP_LIGHT_DIFFUSE_MIX;
        let b = RT64_ATTRIBUTE_IGNORE_NORMAL_FACTOR;

        let mut both = dst_extra_params();
        both.apply_extra_attributes(&src_extra_params(a | b));

        let mut a_then_b = dst_extra_params();
        a_then_b.apply_extra_attributes(&src_extra_params(a));
        a_then_b.apply_extra_attributes(&src_extra_params(b));

        let mut b_then_a = dst_extra_params();
        b_then_a.apply_extra_attributes(&src_extra_params(b));
        b_then_a.apply_extra_attributes(&src_extra_params(a));

        assert_eq!(both, a_then_b);
        assert_eq!(both, b_then_a);
        assert_eq!(both.rsp_light_diffuse_mix, 101.0f32);
        assert_eq!(both.ignore_normal_factor, 103.0f32);
    }

    /// The last branch writes the struct's *first* field. Setting only
    /// `RSP_LIGHT_DIFFUSE_MIX` must still reach it.
    #[test]
    fn the_last_branch_writes_the_first_field() {
        let out = merge_with_only(RT64_ATTRIBUTE_RSP_LIGHT_DIFFUSE_MIX);
        assert_eq!(out.rsp_light_diffuse_mix, 101.0f32);
        assert_eq!(out.ignore_normal_factor, 3.0f32);
    }

    /// `DEPTH_DECAL_BIAS` is tested before `SHADOW_RAY_BIAS` in the body even
    /// though it is the *higher* mask (`0x40000` vs `0x02000`); both still
    /// resolve independently.
    #[test]
    fn depth_decal_and_shadow_ray_bias_resolve_independently() {
        let decal_only = merge_with_only(RT64_ATTRIBUTE_DEPTH_DECAL_BIAS);
        assert_eq!(decal_only.depth_decal_bias, 117.0f32);
        assert_eq!(decal_only.shadow_ray_bias, 18.0f32);

        let ray_only = merge_with_only(RT64_ATTRIBUTE_SHADOW_RAY_BIAS);
        assert_eq!(ray_only.shadow_ray_bias, 118.0f32);
        assert_eq!(ray_only.depth_decal_bias, 17.0f32);
    }

    /// Float assignment is a bit-exact copy: no arithmetic, no normalization.
    /// A negative zero, an infinity and a NaN all survive the branch.
    #[test]
    fn branch_assignment_copies_float_bits_exactly() {
        let mut src = dst_extra_params();
        src.enabled_attributes = RT64_ATTRIBUTE_SPECULAR_EXPONENT | RT64_ATTRIBUTE_ROUGHNESS_FACTOR;
        src.specular_exponent = -0.0f32;
        src.roughness_factor = f32::INFINITY;

        let mut out = dst_extra_params();
        out.apply_extra_attributes(&src);

        assert_eq!(out.specular_exponent.to_bits(), (-0.0f32).to_bits());
        assert_ne!(out.specular_exponent.to_bits(), 0.0f32.to_bits());
        assert!(out.roughness_factor.is_infinite());
        assert!(out.roughness_factor.is_sign_positive());
    }

    #[test]
    fn branch_assignment_copies_nan_without_touching_it() {
        let mut src = dst_extra_params();
        src.enabled_attributes = RT64_ATTRIBUTE_UV_DETAIL_SCALE;
        src.uv_detail_scale = f32::NAN;

        let mut out = dst_extra_params();
        out.apply_extra_attributes(&src);

        assert!(out.uv_detail_scale.is_nan());
        // Every other field is untouched, so the struct is otherwise equal.
        assert_eq!(out.reflection_factor, 5.0f32);
    }

    /// `lightGroupMaskBits` is a full 32-bit `uint`: the branch copies it
    /// whole, with no masking against the attribute bits.
    #[test]
    fn light_group_mask_bits_copies_the_full_u32() {
        let mut src = dst_extra_params();
        src.enabled_attributes = RT64_ATTRIBUTE_LIGHT_GROUP_MASK_BITS;
        src.light_group_mask_bits = 0xDEAD_BEEFu32;

        let mut out = dst_extra_params();
        out.apply_extra_attributes(&src);

        assert_eq!(out.light_group_mask_bits, 0xDEAD_BEEFu32);
    }
}
