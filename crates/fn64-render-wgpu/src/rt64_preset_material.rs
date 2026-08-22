//! `PresetMaterial`'s default-state construction and its two value-fallback
//! tables: a literal port of the permitted MIT RT64 source pinned at commit
//! `5473732a822a4423b5696e7cb18fecc425a59875`
//! (`docs/RT64-PORT-AUTHORITY.md`), `src/preset/rt64_preset_material.h`
//! (SHA-256 of the whole file,
//! `4a64bc0709e22032990ff3305f0c8a383823dfaa499a966786eeed3fe8002eea`, 47
//! newline-terminated lines plus a final unterminated line, which the
//! inventory records as 48) + `src/preset/rt64_preset_material.cpp` (SHA-256
//! of the whole file,
//! `c119bb06f95692cb3ba3f0d9f82931beace044d66d53fbab37c7cc5bb5f678ae`, 290
//! newline-terminated lines plus a final unterminated line, which the
//! inventory records as 291). Both digests were computed independently here
//! with `shasum -a 256` against the pinned checkout and cross-checked
//! verbatim against `docs/rt64-port-inventory.json`'s
//! `files[path="src/preset/rt64_preset_material.{h,cpp}"].sources.port.sha256`,
//! which records the identical two digests -- no mismatch.
//!
//! ## Ported / refused boundary, and the criterion
//!
//! **Criterion**: a construct is ported when its behavior is fully determined
//! by values and control flow already present in the two cited files -- no
//! `nlohmann::json` object, no `PresetBase` vtable, no filesystem, no ImGui
//! context. Everything whose observable behavior *is* the library call is
//! refused, and named below.
//!
//! **Ported** (3 constructs, covering `.cpp` lines 37-55, 62-73 and the
//! default/guard structure of 75-95):
//! - `PresetMaterial::PresetMaterial()` (lines 62-73) -- the complete default
//!   state, `memset` included.
//! - the fallback table inside `interop::from_json` (lines 37-55) -- the 19
//!   `j.value(key, default)` defaults, i.e. the `ExtraParams` you get from an
//!   empty JSON object.
//! - the fallback table and mandatory-key guard inside
//!   `PresetMaterial::readJson` (lines 80-94) -- the 7 `jsonObj.value(key,
//!   default)` defaults plus the "`description` absent => return false"
//!   guard, ported against already-extracted `Option`s rather than a `json`.
//!
//! **Refused** (the remaining ~253 of 290 `.cpp` lines):
//! - `interop::to_json` (lines 12-34) and `PresetMaterial::writeJson` (lines
//!   97-112): pure `nlohmann::json` object construction. Their only content
//!   is the key-name-to-field mapping; there is no arithmetic. The key names
//!   are nevertheless reproduced in the quotes below since the read side's
//!   fallback table is keyed by them.
//! - the `j.value(...)` / `jsonObj.value(...)` *lookups* themselves, and
//!   `description = *it` (line 85), which is `nlohmann`'s ADL dispatch into
//!   `from_json`: library plumbing, no local behavior.
//! - `PresetBase::readJson`/`PresetBase::writeJson` (lines 76-78, 98-100):
//!   defined in `src/preset/rt64_preset.cpp`, which is not a cited source
//!   here. Their `false` return short-circuits both functions; that
//!   short-circuit is modelled as an explicit precondition on the ported
//!   functions rather than guessed at.
//! - `PresetMaterialLibraryInspector::inspectLibrary` (lines 116-139) and
//!   `::inspectSelection` (lines 141-290): 175 lines, wall-to-wall ImGui
//!   (`BeginChild`, `PushID`, `Selectable`, `Checkbox`, `DragFloat`,
//!   `BeginCombo`, ...). Every value in them is produced by a widget call.
//!   Refused entirely. The five `pushCommon`/`pushFloat`/`pushVector3`/
//!   `pushVector4`/`pushInt` lambdas (lines 160-218) do contain a genuine
//!   non-UI fragment -- the attribute bit set/clear at lines 165-170 -- but
//!   it is inseparable from `ImGui::Checkbox`'s in-out parameter, which
//!   supplies `checkboxValue`, so it is refused with the rest rather than
//!   half-ported around an invented input.
//! - `PresetMaterialLibrary` and `PresetLibraryInspector` (`.h` lines 42-47):
//!   an empty derivation and an inspector base; nothing to port.
//! - `interop::ExtraParams::applyExtraAttributes` and the
//!   `RT64_ATTRIBUTE_*` macros live in `src/shared/rt64_extra_params.h`,
//!   which is **not** one of this card's two cited sources. `ExtraParams`'
//!   field list and field order are read from there only as the type
//!   definition the ported constructor's `memset` and the ported fallback
//!   table both operate on; `applyExtraAttributes`' 18-branch body is not
//!   ported *here* and this module makes no claim about it. Both the type and
//!   that body are ported by the sibling module that does cite the header,
//!   `rt64_extra_params.rs`, from which this module now imports the type.
//! - `SCRIPT_ENABLED`-gated `interpolation.callMatchCallback` (`.h` lines
//!   32-34, `.cpp` lines 70-72): a raw `CallMatchCallback *` under a
//!   compile-time feature this port does not model. Omitted; its only
//!   behavior in the cited lines is `= nullptr`.
//!
//! ## Verbatim key logic
//!
//! ```text
//! // rt64_preset_material.h lines 18-40 (struct declaration)
//! struct PresetMaterial : public PresetBase {
//!     interop::ExtraParams description;
//!
//!     struct {
//!         std::string presetName;
//!         float primColorTint;
//!         float primAlphaAttenuation;
//!         float envColorTint;
//!         float envAlphaAttenuation;
//!         float scale;
//!     } light;
//!
//!     struct {
//!         bool enabled;
//! #       if SCRIPT_ENABLED
//!         CallMatchCallback *callMatchCallback;
//! #       endif
//!     } interpolation;
//!
//!     PresetMaterial();
//!     virtual bool readJson(const json &jsonObj) override;
//!     virtual bool writeJson(json &jsonObj) const override;
//! };
//!
//! // rt64_preset_material.cpp lines 36-56 (from_json fallback table)
//! void from_json(const json &j, ExtraParams &e) {
//!     e.ignoreNormalFactor = j.value("ignoreNormalFactor", 0.0f);
//!     e.uvDetailScale = j.value("uvDetailScale", 0.0f);
//!     e.reflectionFactor = j.value("reflectionFactor", 0.0f);
//!     e.reflectionFresnelFactor = j.value("reflectionFresnelFactor", 0.0f);
//!     e.roughnessFactor = j.value("roughnessFactor", 0.0f);
//!     e.refractionFactor = j.value("refractionFactor", 0.0f);
//!     e.shadowCatcherFactor = j.value("shadowCatcherFactor", 0.0f);
//!     e.specularColor = j.value("specularColor", float3(1.0f, 1.0f, 1.0f));
//!     e.specularExponent = j.value("specularExponent", 1.0f);
//!     e.solidAlphaMultiplier = j.value("solidAlphaMultiplier", 1.0f);
//!     e.shadowAlphaMultiplier = j.value("shadowAlphaMultiplier", 1.0f);
//!     e.depthOrderBias = j.value("depthOrderBias", 0.0f);
//!     e.depthDecalBias = j.value("depthDecalBias", 0.0f);
//!     e.shadowRayBias = j.value("shadowRayBias", 0.0f);
//!     e.selfLight = j.value("selfLight", float3(0.0f, 0.0f, 0.0f));
//!     e.lightGroupMaskBits = j.value("lightGroupMaskBits", 0UL);
//!     e.diffuseColorMix = j.value("diffuseColorMix", float4(0.0f, 0.0f, 0.0f, 0.0f));
//!     e.rspLightDiffuseMix = j.value("rspLightDiffuseMix", 0.0f);
//!     e.enabledAttributes = j.value("enabledAttributes", 0L);
//! }
//!
//! // rt64_preset_material.cpp lines 62-73 (constructor)
//! PresetMaterial::PresetMaterial() {
//!     memset(&description, 0, sizeof(description));
//!     light.primColorTint = 0.0f;
//!     light.primAlphaAttenuation = 0.0f;
//!     light.envColorTint = 0.0f;
//!     light.envAlphaAttenuation = 0.0f;
//!     light.scale = 1.0f;
//!     interpolation.enabled = true;
//! #   if SCRIPT_ENABLED
//!     interpolation.callMatchCallback = nullptr;
//! #   endif
//! }
//!
//! // rt64_preset_material.cpp lines 75-95 (readJson)
//! bool PresetMaterial::readJson(const json &jsonObj) {
//!     if (!PresetBase::readJson(jsonObj)) {
//!         return false;
//!     }
//!
//!     auto it = jsonObj.find("description");
//!     if (it == jsonObj.end()) {
//!         return false;
//!     }
//!
//!     description = *it;
//!     light.presetName = jsonObj.value("lightPresetName", "");
//!     light.primColorTint = jsonObj.value("lightPrimColorTint", 0.0f);
//!     light.primAlphaAttenuation = jsonObj.value("lightPrimAlphaAttenuation", 0.0f);
//!     light.envColorTint = jsonObj.value("lightEnvColorTint", 0.0f);
//!     light.envAlphaAttenuation = jsonObj.value("lightEnvAlphaAttenuation", 0.0f);
//!     light.scale = jsonObj.value("lightScale", 1.0f);
//!     interpolation.enabled = jsonObj.value("interpolationEnabled", true);
//!
//!     return true;
//! }
//!
//! // rt64_preset_material.cpp lines 97-112 (writeJson -- refused, key names only)
//! jsonObj["description"]                = description;
//! jsonObj["lightPresetName"]            = light.presetName;
//! jsonObj["lightPrimColorTint"]         = light.primColorTint;
//! jsonObj["lightPrimAlphaAttenuation"]  = light.primAlphaAttenuation;
//! jsonObj["lightEnvColorTint"]          = light.envColorTint;
//! jsonObj["lightEnvAlphaAttenuation"]   = light.envAlphaAttenuation;
//! jsonObj["lightScale"]                 = light.scale;
//! jsonObj["interpolationEnabled"]       = interpolation.enabled;
//! ```
//!
//! ## Reuse, not new type
//!
//! `crates/fn64-render-wgpu/src/` was grepped for an existing `ExtraParams`,
//! `extra_params`, `ignore_normal_factor`, `light_group_mask*`. That search
//! read "no hit" when this module was first written, and on that reading it
//! declared its own `ExtraParams` -- correctly flagged at the time, since the
//! header that declares the type upstream was outside this card's cited
//! sources and could not be reached across the path boundary. The reading is
//! now **out of date**: the sibling `rt64_extra_params.rs` is a full port of
//! `src/shared/rt64_extra_params.h` and owns `ExtraParams`. This module no
//! longer defines the type; it imports
//! [`crate::rt64_extra_params::ExtraParams`], and the two definitions were
//! confirmed identical field-for-field (20 fields, same names, same order,
//! same types, same derives) before the duplicate was removed. What this
//! module still owns is the *defaults*, which upstream declares in the two
//! files cited here and not in that header. `rt64_math.rs`
//! defines only `Mat3` (and consumes `Mat4` from `fn64-render-ir`);
//! `rt64_common.rs` defines `FixedRect` and `FixedMatrix`;
//! `rt64_float4_quantize.rs` quantizes bare `[f32; 4]` rather than owning a
//! vector type; the closest sibling `rt64_preset_draw_call_match.rs` defines
//! `DrawCallKey`, `DrawCallMask`, `PresetDrawCall`, `RawDrawCallSample`,
//! `RawDrawCallTileSample` -- none preset-material-adjacent, and it
//! deliberately does not port `PresetBase` either.
//!
//! The vector-type search was narrower, and its earlier "no hit" reading was
//! wrong: grepping this crate's `src/` finds no `Float3`/`Float4`/`Vector3`/
//! `Vec3`/`Vec4` struct *definition*, but a definition-scoped search of one
//! crate is not the workspace. `fn64_render_ir::Vec3` and
//! `fn64_render_ir::Vec4` **do** exist
//! (`crates/fn64-render-ir/src/rsp_math.rs:42` and `:72`), each documented
//! verbatim as "a backend-neutral N-component float vector, matching HLSL
//! `float3`/`float4`" -- exactly what `interop::float3`/`float4` denote --
//! and `fn64-render-ir` is already a dependency of this crate
//! (`Cargo.toml:15`), already imported by three siblings in it
//! (`rt64_interpolation_helpers.rs:155-159,186`, `rt64_math_matrix.rs:214`,
//! and discussed by name in `rt64_common.rs:220`).
//!
//! So `interop::float3` and `interop::float4` (`src/shared/rt64_hlsl.h`
//! lines 187-218, each a plain `{ float x, y, z[, w]; }` union'd with an
//! `f32[N]` array; read only to confirm the field count and element type,
//! nothing else of that file is ported) are represented by reusing
//! `fn64_render_ir::Vec3` / `fn64_render_ir::Vec4`, matching those siblings,
//! rather than by two new one-use structs or by bare `[f32; 3]` / `[f32; 4]`
//! arrays. The reuse is layout-agnostic: this card ports default *values*,
//! and every assertion about them is component-wise, so nothing depends on
//! the shared type's representation.
//!
//! ## Admitted domain
//!
//! - **`ExtraParams` field set and order** come from
//!   `src/shared/rt64_extra_params.h`'s struct: `rspLightDiffuseMix`,
//!   `lockMask`, `ignoreNormalFactor`, `uvDetailScale`, `reflectionFactor`,
//!   `reflectionFresnelFactor`, `roughnessFactor`, `refractionFactor`,
//!   `shadowCatcherFactor`, `specularColor`, `specularExponent`,
//!   `solidAlphaMultiplier`, `shadowAlphaMultiplier`, `depthOrderBias`,
//!   `depthDecalBias`, `shadowRayBias`, `selfLight`, `lightGroupMaskBits`,
//!   `diffuseColorMix`, `enabledAttributes` -- 20 fields. Declaration order
//!   is preserved here because the ported constructor's `memset` is
//!   whole-struct and the field order is what makes "all 20, including any
//!   the JSON table skips" checkable.
//! - **`PresetMaterial::default_construct` (the constructor)**: `memset(...,
//!   0, sizeof(description))` zeroes the *entire* `ExtraParams`, all 20
//!   fields, to `+0.0f` / `0u`. It is a byte-wise zero fill, not a
//!   field-by-field assignment, so it also covers `lockMask` -- a field the
//!   JSON fallback table below never touches. Then five explicit `light`
//!   floats (`primColorTint`, `primAlphaAttenuation`, `envColorTint`,
//!   `envAlphaAttenuation` all `0.0f`; `scale` `1.0f`) and
//!   `interpolation.enabled = true`. `light.presetName` is a `std::string`
//!   member and is **not** assigned in the constructor -- it is
//!   default-constructed empty by `std::string`'s own default ctor, which is
//!   why it does not appear in the ported body's assignment list; the ported
//!   `String::new()` reproduces that same empty result.
//! - **The zero-vs-one asymmetry between the two default tables** (pinned,
//!   not fixed): the constructor's `memset` leaves `specularColor`
//!   `[0,0,0]`, `specularExponent` `0.0`, `solidAlphaMultiplier` `0.0` and
//!   `shadowAlphaMultiplier` `0.0`, while `from_json`'s fallback table gives
//!   those same four fields `[1,1,1]`, `1.0`, `1.0`, `1.0`. A
//!   default-constructed `PresetMaterial` and a `PresetMaterial` whose
//!   `description` came from an empty JSON object therefore do **not** agree
//!   on four of twenty fields. Both are ported literally and a test pins the
//!   exact disagreement set.
//! - **`from_json`'s fallback table covers 19 of `ExtraParams`' 20 fields**;
//!   `lockMask` has no JSON key on either the read or the write side, so
//!   `from_json` leaves whatever was already in the destination `e`
//!   untouched. Ported as an explicit "carried through from the incoming
//!   value" parameter rather than silently defaulted, since the C++ genuinely
//!   does not write it.
//! - **`0UL` for `lightGroupMaskBits` and `0L` for `enabledAttributes`**
//!   (lines 52, 55): both destination fields are `interop::uint`
//!   (`uint32_t`, `rt64_hlsl.h` line 16), yet the two literals have
//!   different C++ types -- `unsigned long` and `long`, neither of them
//!   `uint32_t`, and one signed where the field is unsigned. Both defaults
//!   are numerically zero so the converted value is `0u` either way; the
//!   inconsistency is a source oddity with no behavioral consequence at this
//!   value, recorded here and pinned by test rather than tidied into a
//!   matching pair.
//! - **`read_json_fields`'s two guards, in order**: `PresetBase::readJson`
//!   first (modelled as the `preset_base_ok` parameter -- `false` returns
//!   `Err(ReadJsonReject::PresetBase)` before anything else is examined),
//!   then `jsonObj.find("description") == jsonObj.end()` (modelled as
//!   `description_present`). Both reject with the same C++ `false`; they are
//!   distinguished in the Rust return only so a test can tell which fired.
//!   The order matters and is preserved: a payload with *both* problems
//!   rejects on the `PresetBase` guard, and `description` is never examined.
//! - **`description` is mandatory; the other seven keys are optional.**
//!   `description` is the only key read with `find`/`end()` rather than
//!   `value(key, default)`. There is no default for it -- absence is a hard
//!   reject. Note the asymmetry with `writeJson`, which writes all eight
//!   keys unconditionally, so a round-trip of a written object always
//!   satisfies the guard.
//! - **`readJson`'s seven fallbacks**: `lightPresetName` `""`,
//!   `lightPrimColorTint` `0.0f`, `lightPrimAlphaAttenuation` `0.0f`,
//!   `lightEnvColorTint` `0.0f`, `lightEnvAlphaAttenuation` `0.0f`,
//!   `lightScale` `1.0f`, `interpolationEnabled` `true`. These agree exactly
//!   with the constructor's `light`/`interpolation` defaults -- unlike the
//!   `description` pair above, this pair is consistent, and a test pins that
//!   agreement so a future drift in either is caught.
//! - **All eight `readJson` assignments are unconditional overwrites.** On
//!   the success path every field of `light` and `interpolation` is written,
//!   whether or not its key was present -- an absent key writes the default
//!   *over* whatever the receiver held, it does not preserve the prior
//!   value. Ported by having `read_json_fields` build and return a fresh
//!   value rather than patching in place, and pinned by a test that starts
//!   from a non-default receiver.
//! - **`interpolation.enabled` survives `readJson` untouched only when the
//!   function rejects.** On either reject path the C++ has assigned nothing
//!   yet, so the receiver is unmodified; that is why the Rust reject arms
//!   return no value at all rather than a partially-filled one.
//! - **Float semantics**: every default is an exact binary32 value
//!   (`0.0f`, `1.0f`), so no rounding is involved. The `memset` path
//!   produces `+0.0f` specifically -- all-zero bytes are positive zero in
//!   IEEE-754 binary32, not negative zero -- and a test distinguishes the two
//!   by sign bit (`is_sign_negative`), since `-0.0 == 0.0` compares equal and
//!   would not catch a drift. No NaN arises from any ported path; NaN
//!   *passing through* a caller-supplied value is covered by test rather
//!   than claimed to be handled.
//!
//! ## Nonclaims
//!
//! No GPU, no wgpu resource, no bind group, no shader. No production wiring:
//! this module is declared `mod`, not `pub mod`, is not re-exported from the
//! crate root, and nothing in `fn64-render-wgpu` calls it. No parity or
//! performance claim -- this is a CPU-only characterization of default-state
//! construction and two fallback tables, not a validated match against RT64
//! runtime output. No JSON is parsed, produced, or round-tripped here: the
//! ported `read_json_fields` takes already-extracted `Option`s and makes no
//! claim about how `nlohmann::json`'s `value()` behaves when a key is
//! *present but of the wrong type* (it throws or returns the default
//! depending on version and type -- outside the cited files, and outside
//! this port). No claim is made about `applyExtraAttributes`,
//! `PresetBase::readJson`/`writeJson`, `PresetLibrary`'s file I/O, or any
//! ImGui behavior.

use fn64_render_ir::{Vec3, Vec4};

/// `interop::ExtraParams` is **not** defined here. It is declared upstream in
/// `src/shared/rt64_extra_params.h`, which is not one of this card's two
/// cited sources, and it is ported by the sibling module that does cite that
/// header: [`crate::rt64_extra_params::ExtraParams`]. That module is the full
/// port of the header and owns the type's field set, its field order, the 19
/// `RT64_ATTRIBUTE_*` masks, and `apply_extra_attributes`. This module imports
/// it rather than declaring a second, identical copy.
///
/// This module still owns the *defaults* for that type, because the two files
/// it cites are where upstream actually declares them: the constructor's
/// whole-struct `memset` ([`ExtraParams::zeroed`], below) and `from_json`'s
/// 19-entry fallback table ([`extra_params_from_json_defaults`]). The owning
/// module deliberately declares none, since its header declares none.
///
/// One field-level fact from this card's own sources, which the shared
/// definition documents from the header's side instead: `lock_mask` is
/// written by neither `from_json` nor `to_json` -- it has no JSON key at all.
/// It is zeroed by the constructor's `memset` and carried through unchanged by
/// [`extra_params_from_json_defaults`].
use crate::rt64_extra_params::ExtraParams;

impl ExtraParams {
    /// Literal port of `memset(&description, 0, sizeof(description))`
    /// (`rt64_preset_material.cpp` line 63). A byte-wise zero fill over the
    /// whole struct: every float becomes `+0.0f` (all-zero bytes are IEEE-754
    /// positive zero, not negative zero), every `uint` becomes `0`. All 20
    /// fields, including `lock_mask`, which no JSON path touches.
    pub fn zeroed() -> ExtraParams {
        ExtraParams {
            rsp_light_diffuse_mix: 0.0f32,
            lock_mask: 0.0f32,
            ignore_normal_factor: 0.0f32,
            uv_detail_scale: 0.0f32,
            reflection_factor: 0.0f32,
            reflection_fresnel_factor: 0.0f32,
            roughness_factor: 0.0f32,
            refraction_factor: 0.0f32,
            shadow_catcher_factor: 0.0f32,
            specular_color: Vec3::new(0.0f32, 0.0f32, 0.0f32),
            specular_exponent: 0.0f32,
            solid_alpha_multiplier: 0.0f32,
            shadow_alpha_multiplier: 0.0f32,
            depth_order_bias: 0.0f32,
            depth_decal_bias: 0.0f32,
            shadow_ray_bias: 0.0f32,
            self_light: Vec3::new(0.0f32, 0.0f32, 0.0f32),
            light_group_mask_bits: 0u32,
            diffuse_color_mix: Vec4::new(0.0f32, 0.0f32, 0.0f32, 0.0f32),
            enabled_attributes: 0u32,
        }
    }
}

/// Literal port of the fallback table in `interop::from_json`
/// (`rt64_preset_material.cpp` lines 36-56): the `ExtraParams` produced when
/// **every** JSON key is absent, i.e. from an empty JSON object.
///
/// The `j.value(key, default)` lookups themselves are `nlohmann::json`
/// plumbing and are refused (see module doc); this is their default half,
/// which is pure data.
///
/// `incoming` is the destination `e` as it stood before the call. Nineteen of
/// its twenty fields are overwritten by this function. The twentieth,
/// `lock_mask`, has no JSON key in either `from_json` or `to_json`, so the
/// C++ leaves it exactly as it found it -- that carry-through is reproduced
/// here rather than being silently defaulted to zero.
///
/// Note the two literal types at lines 52 and 55: `0UL` (`unsigned long`) for
/// `lightGroupMaskBits` and `0L` (`long`) for `enabledAttributes`, though
/// both destination fields are `interop::uint` (`uint32_t`). Both convert to
/// `0u`; the mismatched literal types are a source oddity, pinned rather than
/// tidied.
pub fn extra_params_from_json_defaults(incoming: ExtraParams) -> ExtraParams {
    ExtraParams {
        // No JSON key on either side -- untouched by `from_json`.
        lock_mask: incoming.lock_mask,

        ignore_normal_factor: 0.0f32,
        uv_detail_scale: 0.0f32,
        reflection_factor: 0.0f32,
        reflection_fresnel_factor: 0.0f32,
        roughness_factor: 0.0f32,
        refraction_factor: 0.0f32,
        shadow_catcher_factor: 0.0f32,
        specular_color: Vec3::new(1.0f32, 1.0f32, 1.0f32),
        specular_exponent: 1.0f32,
        solid_alpha_multiplier: 1.0f32,
        shadow_alpha_multiplier: 1.0f32,
        depth_order_bias: 0.0f32,
        depth_decal_bias: 0.0f32,
        shadow_ray_bias: 0.0f32,
        self_light: Vec3::new(0.0f32, 0.0f32, 0.0f32),
        light_group_mask_bits: 0u32,
        diffuse_color_mix: Vec4::new(0.0f32, 0.0f32, 0.0f32, 0.0f32),
        rsp_light_diffuse_mix: 0.0f32,
        enabled_attributes: 0u32,
    }
}

/// Literal port of `PresetMaterial`'s anonymous `light` sub-struct
/// (`rt64_preset_material.h` lines 21-28).
#[derive(Clone, Debug, PartialEq)]
pub struct PresetMaterialLight {
    pub preset_name: String,
    pub prim_color_tint: f32,
    pub prim_alpha_attenuation: f32,
    pub env_color_tint: f32,
    pub env_alpha_attenuation: f32,
    pub scale: f32,
}

/// Literal port of `PresetMaterial`'s anonymous `interpolation` sub-struct
/// (`rt64_preset_material.h` lines 30-35), minus the `SCRIPT_ENABLED`-gated
/// `callMatchCallback` pointer (see module doc's refused list).
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct PresetMaterialInterpolation {
    pub enabled: bool,
}

/// Literal port of `PresetMaterial` (`rt64_preset_material.h` lines 18-40),
/// minus the inherited `PresetBase::enabled` (`rt64_preset.h` line 34 --
/// `PresetBase` is not a cited source here and is not ported) and minus the
/// two `virtual` JSON methods, whose pure halves are ported as free functions
/// below.
#[derive(Clone, Debug, PartialEq)]
pub struct PresetMaterial {
    pub description: ExtraParams,
    pub light: PresetMaterialLight,
    pub interpolation: PresetMaterialInterpolation,
}

impl PresetMaterial {
    /// Literal port of `PresetMaterial::PresetMaterial()`
    /// (`rt64_preset_material.cpp` lines 62-73), in source order:
    /// `memset` the whole `description` to zero, then the five `light`
    /// floats, then `interpolation.enabled = true`.
    ///
    /// `light.presetName` is not assigned in the C++ body -- `std::string`'s
    /// own default constructor already leaves it empty, which `String::new()`
    /// reproduces.
    ///
    /// The `SCRIPT_ENABLED`-gated `interpolation.callMatchCallback = nullptr`
    /// (lines 70-72) is not modelled.
    pub fn default_construct() -> PresetMaterial {
        PresetMaterial {
            description: ExtraParams::zeroed(),
            light: PresetMaterialLight {
                preset_name: String::new(),
                prim_color_tint: 0.0f32,
                prim_alpha_attenuation: 0.0f32,
                env_color_tint: 0.0f32,
                env_alpha_attenuation: 0.0f32,
                scale: 1.0f32,
            },
            interpolation: PresetMaterialInterpolation { enabled: true },
        }
    }
}

/// Which of `PresetMaterial::readJson`'s two guards rejected.
///
/// The C++ returns a bare `false` for both (`rt64_preset_material.cpp` lines
/// 77 and 82); the two are distinguished here only so a test can tell which
/// one fired. Both are equally "return false" as far as the original's
/// callers can observe.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum ReadJsonReject {
    /// `if (!PresetBase::readJson(jsonObj)) { return false; }` (lines 76-78).
    /// Checked first, before `description` is looked for at all.
    PresetBase,
    /// `auto it = jsonObj.find("description"); if (it == jsonObj.end()) {
    /// return false; }` (lines 80-83). `description` is the only mandatory
    /// key.
    DescriptionAbsent,
}

/// The eight JSON values `PresetMaterial::readJson` reads, already extracted
/// from the `json` object -- `None` meaning "key absent", which selects the
/// fallback.
///
/// This shape exists because the `nlohmann::json` lookups themselves are
/// refused (see module doc); the ported behavior is what happens to the
/// *defaults* and the *guards*, which is fully determined by presence and
/// value.
#[derive(Clone, Debug, PartialEq)]
pub struct ReadJsonInput {
    /// `PresetBase::readJson`'s return. Not ported (`rt64_preset.cpp` is not
    /// a cited source); supplied by the caller so the first guard's ordering
    /// stays observable.
    pub preset_base_ok: bool,
    /// `jsonObj.find("description")`: `None` models `== jsonObj.end()`.
    /// There is no default -- absence is a hard reject.
    pub description: Option<ExtraParams>,
    /// `jsonObj.value("lightPresetName", "")`.
    pub light_preset_name: Option<String>,
    /// `jsonObj.value("lightPrimColorTint", 0.0f)`.
    pub light_prim_color_tint: Option<f32>,
    /// `jsonObj.value("lightPrimAlphaAttenuation", 0.0f)`.
    pub light_prim_alpha_attenuation: Option<f32>,
    /// `jsonObj.value("lightEnvColorTint", 0.0f)`.
    pub light_env_color_tint: Option<f32>,
    /// `jsonObj.value("lightEnvAlphaAttenuation", 0.0f)`.
    pub light_env_alpha_attenuation: Option<f32>,
    /// `jsonObj.value("lightScale", 1.0f)`.
    pub light_scale: Option<f32>,
    /// `jsonObj.value("interpolationEnabled", true)`.
    pub interpolation_enabled: Option<bool>,
}

impl ReadJsonInput {
    /// An input with `PresetBase::readJson` succeeding and every key absent
    /// except `description`, which is mandatory. Every ported fallback fires.
    pub fn all_absent(description: ExtraParams) -> ReadJsonInput {
        ReadJsonInput {
            preset_base_ok: true,
            description: Some(description),
            light_preset_name: None,
            light_prim_color_tint: None,
            light_prim_alpha_attenuation: None,
            light_env_color_tint: None,
            light_env_alpha_attenuation: None,
            light_scale: None,
            interpolation_enabled: None,
        }
    }
}

/// Literal port of `PresetMaterial::readJson` (`rt64_preset_material.cpp`
/// lines 75-95), minus the `nlohmann::json` lookups (refused; supplied
/// pre-extracted via [`ReadJsonInput`]).
///
/// Branch structure, preserved exactly and in order:
/// 1. `PresetBase::readJson` fails => reject, `description` never examined.
/// 2. `description` key absent => reject.
/// 3. otherwise assign all eight fields unconditionally (an absent optional
///    key writes its default *over* the receiver's prior value, it does not
///    preserve it) and succeed.
///
/// Because every field on the success path is written, this returns a fresh
/// value rather than patching a receiver in place -- and on either reject
/// path returns no value at all, matching the C++ leaving the receiver
/// entirely unmodified.
pub fn read_json_fields(input: &ReadJsonInput) -> Result<PresetMaterial, ReadJsonReject> {
    if !input.preset_base_ok {
        return Err(ReadJsonReject::PresetBase);
    }

    let description = match input.description {
        Some(description) => description,
        None => return Err(ReadJsonReject::DescriptionAbsent),
    };

    Ok(PresetMaterial {
        description,
        light: PresetMaterialLight {
            preset_name: match &input.light_preset_name {
                Some(preset_name) => preset_name.clone(),
                None => String::from(""),
            },
            prim_color_tint: match input.light_prim_color_tint {
                Some(value) => value,
                None => 0.0f32,
            },
            prim_alpha_attenuation: match input.light_prim_alpha_attenuation {
                Some(value) => value,
                None => 0.0f32,
            },
            env_color_tint: match input.light_env_color_tint {
                Some(value) => value,
                None => 0.0f32,
            },
            env_alpha_attenuation: match input.light_env_alpha_attenuation {
                Some(value) => value,
                None => 0.0f32,
            },
            scale: match input.light_scale {
                Some(value) => value,
                None => 1.0f32,
            },
        },
        interpolation: PresetMaterialInterpolation {
            enabled: match input.interpolation_enabled {
                Some(value) => value,
                None => true,
            },
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A recognisable non-default `ExtraParams` for "did the assignment
    /// actually happen" tests. Every field is distinct and non-zero.
    fn marker_extra_params() -> ExtraParams {
        ExtraParams {
            rsp_light_diffuse_mix: 11.0f32,
            lock_mask: 12.0f32,
            ignore_normal_factor: 13.0f32,
            uv_detail_scale: 14.0f32,
            reflection_factor: 15.0f32,
            reflection_fresnel_factor: 16.0f32,
            roughness_factor: 17.0f32,
            refraction_factor: 18.0f32,
            shadow_catcher_factor: 19.0f32,
            specular_color: Vec3::new(20.0f32, 21.0f32, 22.0f32),
            specular_exponent: 23.0f32,
            solid_alpha_multiplier: 24.0f32,
            shadow_alpha_multiplier: 25.0f32,
            depth_order_bias: 26.0f32,
            depth_decal_bias: 27.0f32,
            shadow_ray_bias: 28.0f32,
            self_light: Vec3::new(29.0f32, 30.0f32, 31.0f32),
            light_group_mask_bits: 32u32,
            diffuse_color_mix: Vec4::new(33.0f32, 34.0f32, 35.0f32, 36.0f32),
            enabled_attributes: 37u32,
        }
    }

    // -----------------------------------------------------------------
    // ExtraParams::zeroed -- the constructor's `memset` (line 63).
    // -----------------------------------------------------------------

    #[test]
    fn zeroed_sets_every_scalar_float_field_to_zero() {
        let e = ExtraParams::zeroed();
        assert_eq!(e.rsp_light_diffuse_mix, 0.0f32);
        assert_eq!(e.lock_mask, 0.0f32);
        assert_eq!(e.ignore_normal_factor, 0.0f32);
        assert_eq!(e.uv_detail_scale, 0.0f32);
        assert_eq!(e.reflection_factor, 0.0f32);
        assert_eq!(e.reflection_fresnel_factor, 0.0f32);
        assert_eq!(e.roughness_factor, 0.0f32);
        assert_eq!(e.refraction_factor, 0.0f32);
        assert_eq!(e.shadow_catcher_factor, 0.0f32);
        assert_eq!(e.specular_exponent, 0.0f32);
        assert_eq!(e.solid_alpha_multiplier, 0.0f32);
        assert_eq!(e.shadow_alpha_multiplier, 0.0f32);
        assert_eq!(e.depth_order_bias, 0.0f32);
        assert_eq!(e.depth_decal_bias, 0.0f32);
        assert_eq!(e.shadow_ray_bias, 0.0f32);
    }

    #[test]
    fn zeroed_sets_every_vector_field_to_all_zero() {
        let e = ExtraParams::zeroed();
        assert_eq!(e.specular_color, Vec3::new(0.0f32, 0.0f32, 0.0f32));
        assert_eq!(e.self_light, Vec3::new(0.0f32, 0.0f32, 0.0f32));
        assert_eq!(
            e.diffuse_color_mix,
            Vec4::new(0.0f32, 0.0f32, 0.0f32, 0.0f32)
        );
    }

    #[test]
    fn zeroed_sets_both_uint_fields_to_zero() {
        let e = ExtraParams::zeroed();
        assert_eq!(e.light_group_mask_bits, 0u32);
        assert_eq!(e.enabled_attributes, 0u32);
    }

    /// All-zero bytes are IEEE-754 **positive** zero in binary32, so a
    /// `memset(0)` cannot produce `-0.0`. `-0.0 == 0.0` compares equal, so
    /// this must check the sign bit rather than the value.
    #[test]
    fn zeroed_floats_are_positive_zero_not_negative_zero() {
        let e = ExtraParams::zeroed();
        assert!(!e.rsp_light_diffuse_mix.is_sign_negative());
        assert!(!e.lock_mask.is_sign_negative());
        assert!(!e.ignore_normal_factor.is_sign_negative());
        assert!(!e.shadow_ray_bias.is_sign_negative());
        assert!(!e.specular_color.x.is_sign_negative());
        assert!(!e.self_light.z.is_sign_negative());
        assert!(!e.diffuse_color_mix.w.is_sign_negative());
    }

    /// `memset` is whole-struct, so its every-float-bit-pattern is literally
    /// all zeroes -- hand-derived: binary32 `+0.0` is `0x00000000`.
    #[test]
    fn zeroed_float_bit_patterns_are_all_zero_words() {
        let e = ExtraParams::zeroed();
        assert_eq!(e.rsp_light_diffuse_mix.to_bits(), 0x0000_0000u32);
        assert_eq!(e.specular_exponent.to_bits(), 0x0000_0000u32);
        assert_eq!(e.solid_alpha_multiplier.to_bits(), 0x0000_0000u32);
        assert_eq!(e.shadow_alpha_multiplier.to_bits(), 0x0000_0000u32);
    }

    /// `memset(&description, 0, sizeof(description))` covers the *whole*
    /// struct, so `lock_mask` -- which no JSON key ever touches -- is zeroed
    /// too. This is the one field where the constructor and the JSON table
    /// genuinely differ in *reach*, not just in value.
    #[test]
    fn zeroed_reaches_lock_mask_which_has_no_json_key() {
        assert_eq!(ExtraParams::zeroed().lock_mask, 0.0f32);
    }

    #[test]
    fn zeroed_is_idempotent_and_stateless() {
        assert_eq!(ExtraParams::zeroed(), ExtraParams::zeroed());
    }

    // -----------------------------------------------------------------
    // extra_params_from_json_defaults -- from_json's fallback table
    // (lines 37-55).
    // -----------------------------------------------------------------

    #[test]
    fn from_json_defaults_zero_valued_floats() {
        let e = extra_params_from_json_defaults(ExtraParams::zeroed());
        assert_eq!(e.ignore_normal_factor, 0.0f32);
        assert_eq!(e.uv_detail_scale, 0.0f32);
        assert_eq!(e.reflection_factor, 0.0f32);
        assert_eq!(e.reflection_fresnel_factor, 0.0f32);
        assert_eq!(e.roughness_factor, 0.0f32);
        assert_eq!(e.refraction_factor, 0.0f32);
        assert_eq!(e.shadow_catcher_factor, 0.0f32);
        assert_eq!(e.depth_order_bias, 0.0f32);
        assert_eq!(e.depth_decal_bias, 0.0f32);
        assert_eq!(e.shadow_ray_bias, 0.0f32);
        assert_eq!(e.rsp_light_diffuse_mix, 0.0f32);
    }

    /// The four fields whose JSON default is `1.0f`, not `0.0f`:
    /// `specularColor` (as `float3(1,1,1)`), `specularExponent`,
    /// `solidAlphaMultiplier`, `shadowAlphaMultiplier` (lines 44-47).
    #[test]
    fn from_json_defaults_the_four_one_valued_fields() {
        let e = extra_params_from_json_defaults(ExtraParams::zeroed());
        assert_eq!(e.specular_color, Vec3::new(1.0f32, 1.0f32, 1.0f32));
        assert_eq!(e.specular_exponent, 1.0f32);
        assert_eq!(e.solid_alpha_multiplier, 1.0f32);
        assert_eq!(e.shadow_alpha_multiplier, 1.0f32);
    }

    /// `specularColor` defaults to `float3(1,1,1)` but `selfLight` to
    /// `float3(0,0,0)` and `diffuseColorMix` to `float4(0,0,0,0)` -- the
    /// three vector defaults are not uniform.
    #[test]
    fn from_json_vector_defaults_are_not_uniform() {
        let e = extra_params_from_json_defaults(ExtraParams::zeroed());
        assert_eq!(e.specular_color, Vec3::new(1.0f32, 1.0f32, 1.0f32));
        assert_eq!(e.self_light, Vec3::new(0.0f32, 0.0f32, 0.0f32));
        assert_eq!(
            e.diffuse_color_mix,
            Vec4::new(0.0f32, 0.0f32, 0.0f32, 0.0f32)
        );
    }

    /// `0UL` (line 52) and `0L` (line 55) both land in `interop::uint`
    /// (`uint32_t`) fields as `0u`, despite one being `unsigned long` and the
    /// other signed `long`.
    #[test]
    fn from_json_defaults_both_uint_fields_to_zero_despite_mismatched_literals() {
        let e = extra_params_from_json_defaults(ExtraParams::zeroed());
        assert_eq!(e.light_group_mask_bits, 0u32);
        assert_eq!(e.enabled_attributes, 0u32);
    }

    /// `lockMask` has no JSON key on either side, so `from_json` never writes
    /// it -- whatever was in the destination survives.
    #[test]
    fn from_json_carries_lock_mask_through_untouched() {
        let incoming = ExtraParams {
            lock_mask: 7.5f32,
            ..ExtraParams::zeroed()
        };
        assert_eq!(extra_params_from_json_defaults(incoming).lock_mask, 7.5f32);
    }

    /// Same carry-through, from a fully-populated marker value, to prove the
    /// other 19 fields are overwritten while `lock_mask` alone is not.
    #[test]
    fn from_json_overwrites_all_nineteen_keyed_fields_over_a_marker() {
        let e = extra_params_from_json_defaults(marker_extra_params());
        assert_eq!(e.lock_mask, 12.0f32);
        let mut expected = extra_params_from_json_defaults(ExtraParams::zeroed());
        expected.lock_mask = 12.0f32;
        assert_eq!(e, expected);
    }

    /// A NaN in `lock_mask` passes through unexamined -- `from_json` neither
    /// reads nor writes that field. Compared by bit pattern because
    /// `NaN != NaN`.
    #[test]
    fn from_json_carries_a_nan_lock_mask_through_by_bit_pattern() {
        let incoming = ExtraParams {
            lock_mask: f32::NAN,
            ..ExtraParams::zeroed()
        };
        let out = extra_params_from_json_defaults(incoming);
        assert!(out.lock_mask.is_nan());
        assert_eq!(out.lock_mask.to_bits(), f32::NAN.to_bits());
    }

    /// A negative-zero `lock_mask` keeps its sign bit through the
    /// carry-through, which `-0.0 == 0.0` would not detect.
    #[test]
    fn from_json_carries_negative_zero_lock_mask_with_its_sign_bit() {
        let incoming = ExtraParams {
            lock_mask: -0.0f32,
            ..ExtraParams::zeroed()
        };
        let out = extra_params_from_json_defaults(incoming);
        assert!(out.lock_mask.is_sign_negative());
        assert_eq!(out.lock_mask.to_bits(), 0x8000_0000u32);
    }

    /// Hand-derived binary32 bit patterns: `+1.0f` is `0x3F800000`,
    /// `+0.0f` is `0x00000000`.
    #[test]
    fn from_json_one_defaults_have_the_expected_binary32_bits() {
        let e = extra_params_from_json_defaults(ExtraParams::zeroed());
        assert_eq!(e.specular_exponent.to_bits(), 0x3F80_0000u32);
        assert_eq!(e.solid_alpha_multiplier.to_bits(), 0x3F80_0000u32);
        assert_eq!(e.shadow_alpha_multiplier.to_bits(), 0x3F80_0000u32);
        assert_eq!(e.specular_color.x.to_bits(), 0x3F80_0000u32);
        assert_eq!(e.specular_color.y.to_bits(), 0x3F80_0000u32);
        assert_eq!(e.specular_color.z.to_bits(), 0x3F80_0000u32);
        assert_eq!(e.uv_detail_scale.to_bits(), 0x0000_0000u32);
    }

    #[test]
    fn from_json_defaults_are_idempotent() {
        let once = extra_params_from_json_defaults(ExtraParams::zeroed());
        let twice = extra_params_from_json_defaults(once);
        assert_eq!(once, twice);
    }

    // -----------------------------------------------------------------
    // The zero-vs-one asymmetry between the two default tables.
    // -----------------------------------------------------------------

    /// **Pinned upstream oddity.** The constructor's `memset` and
    /// `from_json`'s fallback table disagree on exactly four of twenty
    /// fields: `specularColor`, `specularExponent`, `solidAlphaMultiplier`
    /// and `shadowAlphaMultiplier` are `0` from the constructor but `1` from
    /// an empty JSON object. Ported literally, not reconciled.
    #[test]
    fn constructor_and_json_defaults_disagree_on_exactly_four_fields() {
        let ctor = ExtraParams::zeroed();
        let from_json = extra_params_from_json_defaults(ExtraParams::zeroed());

        assert_ne!(ctor.specular_color, from_json.specular_color);
        assert_ne!(ctor.specular_exponent, from_json.specular_exponent);
        assert_ne!(
            ctor.solid_alpha_multiplier,
            from_json.solid_alpha_multiplier
        );
        assert_ne!(
            ctor.shadow_alpha_multiplier,
            from_json.shadow_alpha_multiplier
        );

        // Patching exactly those four onto the constructor's value makes the
        // two agree, proving no fifth field differs.
        let patched = ExtraParams {
            specular_color: from_json.specular_color,
            specular_exponent: from_json.specular_exponent,
            solid_alpha_multiplier: from_json.solid_alpha_multiplier,
            shadow_alpha_multiplier: from_json.shadow_alpha_multiplier,
            ..ctor
        };
        assert_eq!(patched, from_json);
    }

    /// The disagreeing four are `0.0` on the constructor side specifically --
    /// stated as absolute values so a drift on either side is caught, not
    /// just a drift in their difference.
    #[test]
    fn constructor_side_of_the_disagreement_is_zero() {
        let ctor = ExtraParams::zeroed();
        assert_eq!(ctor.specular_color, Vec3::new(0.0f32, 0.0f32, 0.0f32));
        assert_eq!(ctor.specular_exponent, 0.0f32);
        assert_eq!(ctor.solid_alpha_multiplier, 0.0f32);
        assert_eq!(ctor.shadow_alpha_multiplier, 0.0f32);
    }

    /// The other sixteen fields agree between the two tables.
    #[test]
    fn the_other_sixteen_fields_agree_between_the_two_tables() {
        let ctor = ExtraParams::zeroed();
        let from_json = extra_params_from_json_defaults(ExtraParams::zeroed());
        assert_eq!(ctor.rsp_light_diffuse_mix, from_json.rsp_light_diffuse_mix);
        assert_eq!(ctor.lock_mask, from_json.lock_mask);
        assert_eq!(ctor.ignore_normal_factor, from_json.ignore_normal_factor);
        assert_eq!(ctor.uv_detail_scale, from_json.uv_detail_scale);
        assert_eq!(ctor.reflection_factor, from_json.reflection_factor);
        assert_eq!(
            ctor.reflection_fresnel_factor,
            from_json.reflection_fresnel_factor
        );
        assert_eq!(ctor.roughness_factor, from_json.roughness_factor);
        assert_eq!(ctor.refraction_factor, from_json.refraction_factor);
        assert_eq!(ctor.shadow_catcher_factor, from_json.shadow_catcher_factor);
        assert_eq!(ctor.depth_order_bias, from_json.depth_order_bias);
        assert_eq!(ctor.depth_decal_bias, from_json.depth_decal_bias);
        assert_eq!(ctor.shadow_ray_bias, from_json.shadow_ray_bias);
        assert_eq!(ctor.self_light, from_json.self_light);
        assert_eq!(ctor.light_group_mask_bits, from_json.light_group_mask_bits);
        assert_eq!(ctor.diffuse_color_mix, from_json.diffuse_color_mix);
        assert_eq!(ctor.enabled_attributes, from_json.enabled_attributes);
    }

    // -----------------------------------------------------------------
    // PresetMaterial::default_construct -- the constructor (lines 62-73).
    // -----------------------------------------------------------------

    #[test]
    fn default_construct_zeroes_the_whole_description() {
        assert_eq!(
            PresetMaterial::default_construct().description,
            ExtraParams::zeroed()
        );
    }

    #[test]
    fn default_construct_light_preset_name_is_empty() {
        let m = PresetMaterial::default_construct();
        assert_eq!(m.light.preset_name, "");
        assert!(m.light.preset_name.is_empty());
    }

    #[test]
    fn default_construct_light_tints_and_attenuations_are_zero() {
        let m = PresetMaterial::default_construct();
        assert_eq!(m.light.prim_color_tint, 0.0f32);
        assert_eq!(m.light.prim_alpha_attenuation, 0.0f32);
        assert_eq!(m.light.env_color_tint, 0.0f32);
        assert_eq!(m.light.env_alpha_attenuation, 0.0f32);
    }

    /// `light.scale = 1.0f` is the single non-zero `light` default (line 68).
    #[test]
    fn default_construct_light_scale_is_one_not_zero() {
        let m = PresetMaterial::default_construct();
        assert_eq!(m.light.scale, 1.0f32);
        assert_ne!(m.light.scale, 0.0f32);
        assert_eq!(m.light.scale.to_bits(), 0x3F80_0000u32);
    }

    /// `interpolation.enabled = true` (line 69) -- a `true` default, not the
    /// `false` a zero-initialised `bool` would give.
    #[test]
    fn default_construct_interpolation_is_enabled() {
        assert!(PresetMaterial::default_construct().interpolation.enabled);
    }

    #[test]
    fn default_construct_light_floats_are_positive_zero() {
        let m = PresetMaterial::default_construct();
        assert!(!m.light.prim_color_tint.is_sign_negative());
        assert!(!m.light.prim_alpha_attenuation.is_sign_negative());
        assert!(!m.light.env_color_tint.is_sign_negative());
        assert!(!m.light.env_alpha_attenuation.is_sign_negative());
    }

    #[test]
    fn default_construct_is_deterministic() {
        assert_eq!(
            PresetMaterial::default_construct(),
            PresetMaterial::default_construct()
        );
    }

    // -----------------------------------------------------------------
    // read_json_fields -- the guards (lines 76-83).
    // -----------------------------------------------------------------

    /// Guard 1: `PresetBase::readJson` returning false rejects immediately.
    #[test]
    fn read_json_rejects_when_preset_base_fails() {
        let mut input = ReadJsonInput::all_absent(marker_extra_params());
        input.preset_base_ok = false;
        assert_eq!(read_json_fields(&input), Err(ReadJsonReject::PresetBase));
    }

    /// Guard 2: an absent `description` rejects, even with `PresetBase` ok.
    #[test]
    fn read_json_rejects_when_description_is_absent() {
        let mut input = ReadJsonInput::all_absent(marker_extra_params());
        input.description = None;
        assert_eq!(
            read_json_fields(&input),
            Err(ReadJsonReject::DescriptionAbsent)
        );
    }

    /// **Guard ordering.** With *both* guards failing, the `PresetBase` guard
    /// fires first -- `description` is never examined. This is the mixed case
    /// that distinguishes the source's order from the reverse.
    #[test]
    fn read_json_preset_base_guard_precedes_the_description_guard() {
        let input = ReadJsonInput {
            preset_base_ok: false,
            description: None,
            ..ReadJsonInput::all_absent(marker_extra_params())
        };
        assert_eq!(read_json_fields(&input), Err(ReadJsonReject::PresetBase));
        assert_ne!(
            read_json_fields(&input),
            Err(ReadJsonReject::DescriptionAbsent)
        );
    }

    /// Both guards passing succeeds; neither guard alone is sufficient.
    #[test]
    fn read_json_succeeds_only_when_both_guards_pass() {
        let ok = ReadJsonInput::all_absent(marker_extra_params());
        assert!(read_json_fields(&ok).is_ok());

        let mut base_bad = ok.clone();
        base_bad.preset_base_ok = false;
        assert!(read_json_fields(&base_bad).is_err());

        let mut desc_bad = ok.clone();
        desc_bad.description = None;
        assert!(read_json_fields(&desc_bad).is_err());
    }

    /// Every optional key present but `description` absent still rejects --
    /// `description` really is the only mandatory one, and no quantity of
    /// present optional keys substitutes for it.
    #[test]
    fn read_json_rejects_on_absent_description_even_with_all_optionals_present() {
        let input = ReadJsonInput {
            preset_base_ok: true,
            description: None,
            light_preset_name: Some(String::from("sunset")),
            light_prim_color_tint: Some(0.25f32),
            light_prim_alpha_attenuation: Some(0.5f32),
            light_env_color_tint: Some(0.75f32),
            light_env_alpha_attenuation: Some(1.0f32),
            light_scale: Some(2.0f32),
            interpolation_enabled: Some(false),
        };
        assert_eq!(
            read_json_fields(&input),
            Err(ReadJsonReject::DescriptionAbsent)
        );
    }

    // -----------------------------------------------------------------
    // read_json_fields -- the seven fallbacks (lines 86-92).
    // -----------------------------------------------------------------

    #[test]
    fn read_json_all_absent_gives_the_documented_seven_defaults() {
        let m = read_json_fields(&ReadJsonInput::all_absent(marker_extra_params()))
            .expect("both guards pass");
        assert_eq!(m.light.preset_name, "");
        assert_eq!(m.light.prim_color_tint, 0.0f32);
        assert_eq!(m.light.prim_alpha_attenuation, 0.0f32);
        assert_eq!(m.light.env_color_tint, 0.0f32);
        assert_eq!(m.light.env_alpha_attenuation, 0.0f32);
        assert_eq!(m.light.scale, 1.0f32);
        assert!(m.interpolation.enabled);
    }

    /// `readJson`'s seven fallbacks agree exactly with the constructor's
    /// `light`/`interpolation` defaults -- unlike the `description` pair,
    /// which disagrees on four fields. Pinned so a drift in either is caught.
    #[test]
    fn read_json_light_defaults_agree_with_the_constructor() {
        let ctor = PresetMaterial::default_construct();
        let read = read_json_fields(&ReadJsonInput::all_absent(ExtraParams::zeroed()))
            .expect("both guards pass");
        assert_eq!(read.light, ctor.light);
        assert_eq!(read.interpolation, ctor.interpolation);
        assert_eq!(read, ctor);
    }

    /// `lightScale`'s default is `1.0f`, the only non-zero float fallback in
    /// `readJson`.
    #[test]
    fn read_json_light_scale_default_is_one() {
        let m = read_json_fields(&ReadJsonInput::all_absent(ExtraParams::zeroed()))
            .expect("both guards pass");
        assert_eq!(m.light.scale, 1.0f32);
        assert_eq!(m.light.scale.to_bits(), 0x3F80_0000u32);
    }

    /// `interpolationEnabled`'s default is `true`, not `false`.
    #[test]
    fn read_json_interpolation_enabled_default_is_true() {
        let m = read_json_fields(&ReadJsonInput::all_absent(ExtraParams::zeroed()))
            .expect("both guards pass");
        assert!(m.interpolation.enabled);
    }

    /// A present `interpolationEnabled: false` must beat the `true` default.
    #[test]
    fn read_json_present_false_interpolation_beats_the_true_default() {
        let mut input = ReadJsonInput::all_absent(ExtraParams::zeroed());
        input.interpolation_enabled = Some(false);
        let m = read_json_fields(&input).expect("both guards pass");
        assert!(!m.interpolation.enabled);
    }

    /// A present `interpolationEnabled: true` is indistinguishable from
    /// absence -- both give `true`. The other half of the boolean's domain.
    #[test]
    fn read_json_present_true_interpolation_matches_the_default() {
        let mut input = ReadJsonInput::all_absent(ExtraParams::zeroed());
        input.interpolation_enabled = Some(true);
        assert!(
            read_json_fields(&input)
                .expect("both guards pass")
                .interpolation
                .enabled
        );
    }

    /// A present `lightScale: 0.0` must beat the `1.0` default -- the case a
    /// falsy-value bug would get wrong.
    #[test]
    fn read_json_present_zero_light_scale_beats_the_one_default() {
        let mut input = ReadJsonInput::all_absent(ExtraParams::zeroed());
        input.light_scale = Some(0.0f32);
        assert_eq!(
            read_json_fields(&input)
                .expect("both guards pass")
                .light
                .scale,
            0.0f32
        );
    }

    /// A present empty `lightPresetName` is indistinguishable from absence --
    /// both give `""`.
    #[test]
    fn read_json_present_empty_preset_name_matches_the_default() {
        let mut input = ReadJsonInput::all_absent(ExtraParams::zeroed());
        input.light_preset_name = Some(String::new());
        assert_eq!(
            read_json_fields(&input)
                .expect("both guards pass")
                .light
                .preset_name,
            ""
        );
    }

    #[test]
    fn read_json_present_non_empty_preset_name_is_carried() {
        let mut input = ReadJsonInput::all_absent(ExtraParams::zeroed());
        input.light_preset_name = Some(String::from("cavern"));
        assert_eq!(
            read_json_fields(&input)
                .expect("both guards pass")
                .light
                .preset_name,
            "cavern"
        );
    }

    #[test]
    fn read_json_each_present_float_beats_its_own_default_independently() {
        let mut input = ReadJsonInput::all_absent(ExtraParams::zeroed());
        input.light_prim_color_tint = Some(0.125f32);
        let m = read_json_fields(&input).expect("both guards pass");
        assert_eq!(m.light.prim_color_tint, 0.125f32);
        // The other three tint/attenuation fields keep their own defaults.
        assert_eq!(m.light.prim_alpha_attenuation, 0.0f32);
        assert_eq!(m.light.env_color_tint, 0.0f32);
        assert_eq!(m.light.env_alpha_attenuation, 0.0f32);
        assert_eq!(m.light.scale, 1.0f32);
    }

    /// Each of the four zero-defaulted `light` floats is wired to its own
    /// input slot -- a cross-wiring would be invisible when all four are
    /// absent, so this gives each a distinct present value.
    #[test]
    fn read_json_light_floats_are_not_cross_wired() {
        let input = ReadJsonInput {
            light_prim_color_tint: Some(1.0f32),
            light_prim_alpha_attenuation: Some(2.0f32),
            light_env_color_tint: Some(3.0f32),
            light_env_alpha_attenuation: Some(4.0f32),
            light_scale: Some(5.0f32),
            ..ReadJsonInput::all_absent(ExtraParams::zeroed())
        };
        let m = read_json_fields(&input).expect("both guards pass");
        assert_eq!(m.light.prim_color_tint, 1.0f32);
        assert_eq!(m.light.prim_alpha_attenuation, 2.0f32);
        assert_eq!(m.light.env_color_tint, 3.0f32);
        assert_eq!(m.light.env_alpha_attenuation, 4.0f32);
        assert_eq!(m.light.scale, 5.0f32);
    }

    /// `description = *it` assigns whatever the key held -- no default, no
    /// validation, no merging with the receiver's prior `description`.
    #[test]
    fn read_json_assigns_the_description_verbatim() {
        let m = read_json_fields(&ReadJsonInput::all_absent(marker_extra_params()))
            .expect("both guards pass");
        assert_eq!(m.description, marker_extra_params());
    }

    /// Negative and out-of-widget-range values pass through untouched --
    /// `readJson` applies no clamping. The `DragFloat` ranges at lines
    /// 264-280 (`0.0f..1.0f` for the tints, `0.0f..FLT_MAX` for the scale)
    /// belong to the refused ImGui half and constrain nothing on this path.
    #[test]
    fn read_json_does_not_clamp_out_of_widget_range_values() {
        let input = ReadJsonInput {
            light_prim_color_tint: Some(-3.0f32),
            light_env_color_tint: Some(50.0f32),
            light_scale: Some(-1.0f32),
            ..ReadJsonInput::all_absent(ExtraParams::zeroed())
        };
        let m = read_json_fields(&input).expect("both guards pass");
        assert_eq!(m.light.prim_color_tint, -3.0f32);
        assert_eq!(m.light.env_color_tint, 50.0f32);
        assert_eq!(m.light.scale, -1.0f32);
    }

    /// A present NaN is carried, not defaulted. Checked with `is_nan` since
    /// `NaN != NaN`.
    #[test]
    fn read_json_carries_a_present_nan_light_scale() {
        let mut input = ReadJsonInput::all_absent(ExtraParams::zeroed());
        input.light_scale = Some(f32::NAN);
        assert!(read_json_fields(&input)
            .expect("both guards pass")
            .light
            .scale
            .is_nan());
    }

    /// A present `-0.0` keeps its sign bit rather than collapsing to the
    /// `+0.0` default -- `-0.0 == 0.0` would hide a wrong branch here.
    #[test]
    fn read_json_carries_a_present_negative_zero_with_its_sign_bit() {
        let mut input = ReadJsonInput::all_absent(ExtraParams::zeroed());
        input.light_prim_color_tint = Some(-0.0f32);
        let value = read_json_fields(&input)
            .expect("both guards pass")
            .light
            .prim_color_tint;
        assert!(value.is_sign_negative());
        assert_eq!(value.to_bits(), 0x8000_0000u32);
    }

    /// An absent key yields the `+0.0` default, positive-signed -- the
    /// counterpart to the test above.
    #[test]
    fn read_json_absent_float_default_is_positive_zero() {
        let m = read_json_fields(&ReadJsonInput::all_absent(ExtraParams::zeroed()))
            .expect("both guards pass");
        assert!(!m.light.prim_color_tint.is_sign_negative());
        assert_eq!(m.light.prim_color_tint.to_bits(), 0x0000_0000u32);
    }

    /// Infinities pass through unchanged in both directions.
    #[test]
    fn read_json_carries_present_infinities() {
        let input = ReadJsonInput {
            light_scale: Some(f32::INFINITY),
            light_env_alpha_attenuation: Some(f32::NEG_INFINITY),
            ..ReadJsonInput::all_absent(ExtraParams::zeroed())
        };
        let m = read_json_fields(&input).expect("both guards pass");
        assert_eq!(m.light.scale, f32::INFINITY);
        assert_eq!(m.light.env_alpha_attenuation, f32::NEG_INFINITY);
    }

    /// All eight assignments are unconditional: starting from a *non-default*
    /// receiver, an all-absent payload overwrites every field with its
    /// default rather than preserving the prior value.
    #[test]
    fn read_json_success_overwrites_a_non_default_receiver_entirely() {
        let mut receiver = PresetMaterial::default_construct();
        receiver.description = marker_extra_params();
        receiver.light = PresetMaterialLight {
            preset_name: String::from("stale"),
            prim_color_tint: 9.0f32,
            prim_alpha_attenuation: 9.0f32,
            env_color_tint: 9.0f32,
            env_alpha_attenuation: 9.0f32,
            scale: 9.0f32,
        };
        receiver.interpolation = PresetMaterialInterpolation { enabled: false };

        let after = read_json_fields(&ReadJsonInput::all_absent(ExtraParams::zeroed()))
            .expect("both guards pass");
        assert_ne!(after, receiver);
        assert_eq!(after, PresetMaterial::default_construct());
    }

    /// A fully-present payload is carried in every field at once.
    #[test]
    fn read_json_all_present_carries_every_field() {
        let input = ReadJsonInput {
            preset_base_ok: true,
            description: Some(marker_extra_params()),
            light_preset_name: Some(String::from("dusk")),
            light_prim_color_tint: Some(0.1f32),
            light_prim_alpha_attenuation: Some(0.2f32),
            light_env_color_tint: Some(0.3f32),
            light_env_alpha_attenuation: Some(0.4f32),
            light_scale: Some(0.5f32),
            interpolation_enabled: Some(false),
        };
        let m = read_json_fields(&input).expect("both guards pass");
        assert_eq!(m.description, marker_extra_params());
        assert_eq!(m.light.preset_name, "dusk");
        assert_eq!(m.light.prim_color_tint, 0.1f32);
        assert_eq!(m.light.prim_alpha_attenuation, 0.2f32);
        assert_eq!(m.light.env_color_tint, 0.3f32);
        assert_eq!(m.light.env_alpha_attenuation, 0.4f32);
        assert_eq!(m.light.scale, 0.5f32);
        assert!(!m.interpolation.enabled);
    }

    /// Reading is a pure function of the input: same input, same output, and
    /// the input is not mutated.
    #[test]
    fn read_json_is_pure_and_repeatable() {
        let input = ReadJsonInput {
            light_preset_name: Some(String::from("repeat")),
            light_scale: Some(3.25f32),
            ..ReadJsonInput::all_absent(marker_extra_params())
        };
        let before = input.clone();
        let a = read_json_fields(&input);
        let b = read_json_fields(&input);
        assert_eq!(a, b);
        assert_eq!(input, before);
    }

    /// The two reject variants are distinct values, so the ordering test
    /// above is meaningful.
    #[test]
    fn the_two_reject_variants_are_distinct() {
        assert_ne!(
            ReadJsonReject::PresetBase,
            ReadJsonReject::DescriptionAbsent
        );
    }

    // -----------------------------------------------------------------
    // Cross-checks between the three ported constructs.
    // -----------------------------------------------------------------

    /// A default-constructed material fed straight back through an
    /// all-absent `readJson` is a fixed point: the constructor's `light` and
    /// `interpolation` defaults are exactly `readJson`'s.
    #[test]
    fn default_construct_is_a_fixed_point_of_an_all_absent_read() {
        let ctor = PresetMaterial::default_construct();
        let round = read_json_fields(&ReadJsonInput::all_absent(ctor.description))
            .expect("both guards pass");
        assert_eq!(round, ctor);
    }

    /// But feeding the *JSON* description defaults through the same path is
    /// **not** a fixed point of the constructor, because of the four-field
    /// zero-vs-one disagreement. This is the asymmetry stated as a whole-type
    /// inequality.
    #[test]
    fn a_material_built_from_json_defaults_differs_from_a_constructed_one() {
        let ctor = PresetMaterial::default_construct();
        let from_empty_json = read_json_fields(&ReadJsonInput::all_absent(
            extra_params_from_json_defaults(ExtraParams::zeroed()),
        ))
        .expect("both guards pass");

        assert_ne!(from_empty_json, ctor);
        // ...and the difference is confined to `description`.
        assert_eq!(from_empty_json.light, ctor.light);
        assert_eq!(from_empty_json.interpolation, ctor.interpolation);
        assert_ne!(from_empty_json.description, ctor.description);
    }

    /// Applying the JSON fallback table to a freshly constructed material's
    /// `description` changes exactly the four disagreeing fields and nothing
    /// else -- the constructor's `lock_mask` zero survives the carry-through.
    #[test]
    fn json_defaults_over_a_constructed_description_change_only_the_four() {
        let ctor = PresetMaterial::default_construct();
        let defaulted = extra_params_from_json_defaults(ctor.description);
        assert_eq!(defaulted.lock_mask, 0.0f32);
        assert_eq!(
            defaulted,
            extra_params_from_json_defaults(ExtraParams::zeroed())
        );
    }

    /// Every ported default value is an exactly-representable binary32, so
    /// none of them round. Hand-derived: `0.0` and `1.0` are both exact.
    #[test]
    fn every_ported_default_is_an_exact_binary32() {
        let e = extra_params_from_json_defaults(ExtraParams::zeroed());
        for bits in [
            e.specular_exponent.to_bits(),
            e.solid_alpha_multiplier.to_bits(),
            e.shadow_alpha_multiplier.to_bits(),
            e.uv_detail_scale.to_bits(),
            PresetMaterial::default_construct().light.scale.to_bits(),
            PresetMaterial::default_construct()
                .light
                .prim_color_tint
                .to_bits(),
        ] {
            assert!(bits == 0x0000_0000u32 || bits == 0x3F80_0000u32);
        }
    }
}
