//! `PresetBase` and `PresetScene` construction, JSON-field defaulting, and
//! writeback ordering: a literal port of the permitted MIT RT64 source pinned
//! at commit `5473732a822a4423b5696e7cb18fecc425a59875`
//! (`docs/RT64-PORT-AUTHORITY.md`), from four files:
//!
//! - `src/preset/rt64_preset.h` (SHA-256 of the whole file,
//!   `2aa1369e42bde37dffa1ebe9bfdec0333f394fa79861420d4c5370b7878eff4b`,
//!   106 lines) -- **partial**: only `PresetBase`'s `enabled = true` member
//!   initializer is ported. `PresetLibrary` is refused (see below).
//! - `src/preset/rt64_preset.cpp` (SHA-256 of the whole file,
//!   `9d31c5573855258d6126d324279c104c7e96f1a5df5d2b5759a1bfff9f0cbb99`,
//!   39 lines) -- **partial**: `PresetBase::readJson`/`::writeJson` are
//!   ported; the four `hlslpp` `to_json`/`from_json` free functions are
//!   refused.
//! - `src/preset/rt64_preset_scene.h` (SHA-256 of the whole file,
//!   `651da3fcee750d6bd53b70b5b470019e35e99601d2bdf89b0e2b756d8109eda4`,
//!   37 lines) -- **partial**: the `PresetScene` field list and its
//!   declaration order are ported; the two library derivations are refused.
//! - `src/preset/rt64_preset_scene.cpp` (SHA-256 of the whole file,
//!   `50ef6ac863795c1d35b27904368a095ab0473bf5c151f279f24155aabc80dd61`,
//!   141 lines) -- **partial**: `PresetScene::PresetScene()`,
//!   `::readJson` and `::writeJson` (lines 12-73) are ported; the
//!   `PresetSceneLibraryInspector` methods (lines 77-141) are refused.
//!
//! All four digests were computed independently here with `shasum -a 256`
//! against the pinned checkout and cross-checked verbatim against
//! `docs/rt64-port-inventory.json`'s
//! `files[path="src/preset/rt64_preset{,_scene}.{h,cpp}"].sources.port.sha256`,
//! which records the identical four digests.
//!
//! ## Inventory drift disclosure
//!
//! `docs/rt64-port-inventory.json` does not yet record any of the four paths'
//! `ported_as` as pointing at this module (all four currently list
//! `"ported_as": []`), so all four are in `ported_as` drift the moment this
//! module cites their digests. `tools/rt64_port_inventory.py`'s
//! `sha256_citation_index` was run directly here and confirms all four
//! digests now resolve to `crates/fn64-render-wgpu/src/rt64_preset_scene.rs`.
//!
//! Note on how that surfaces: the checker reports only the **first** drifting
//! path and exits non-zero, so `scripts/lint-docs.py`'s error count does not
//! rise when this module lands -- the pre-existing
//! `src/common/rt64_elapsed_timer.cpp` drift line masks these four behind it.
//! A flat lint count is therefore NOT evidence that this module is
//! drift-free; it is evidence that a louder drift is already queued ahead of
//! it. This module's own writable surface does not include
//! `docs/rt64-port-inventory.json`, so that reconciliation is deliberately
//! left to the owning ticket rather than done here.
//!
//! Critically: the citations above are **whole-file** SHA-256 digests, which
//! is the only digest granularity the inventory records. A whole-file digest
//! causes the burndown to mark each cited file `ported` at file granularity
//! **even though every one of the four is a partial port** -- see the
//! per-file `partial` annotations above and the Nonclaims section for exactly
//! what was left. None of these four files is a full port. Of the 323 total
//! source lines, this module ports roughly 60 (the `PresetBase` initializer
//! and its two methods, the `PresetScene` constructor, and the read/write
//! field tables); the remaining ~263 are refused as `nlohmann::json`
//! traversal, `std::filesystem`/`iostream` file I/O, or ImGui. Any burndown
//! reading these four files as fully discharged is over-crediting by roughly
//! 5x.
//!
//! ## Port criterion
//!
//! A construct is ported when its behavior is fully determined by values and
//! control flow present in the four cited files -- no `nlohmann::json`
//! object, no vtable dispatch, no `std::ifstream`/`std::ofstream`, no
//! `std::filesystem::path`, no ImGui context. Everything else is refused
//! below, by name.
//!
//! **Ported**, with the reason:
//! - `PresetBase`'s `enabled = true` member initializer (`.h` line 35), and
//!   `PresetBase::readJson`/`::writeJson` (`.cpp` lines 31-39). These are
//!   pure once the single `jsonObj.value("enabled", ...)` lookup is supplied
//!   as an `Option<bool>`; the read is a default substitution and an
//!   unconditional assignment, and the write is an unconditional copy-out.
//!   Both return a constant `true`. See "The `PresetBase` finding" below --
//!   this is the part two sibling cards had to model as an opaque
//!   precondition.
//! - `PresetScene::PresetScene()` (`.cpp` lines 12-27): fourteen literal
//!   float/bool assignments in declaration order. Pure data.
//! - `PresetScene::readJson` (`.cpp` lines 29-50): one guard on the base
//!   class's return, then fourteen unconditional assignments each selecting
//!   between a supplied value and a literal fallback.
//! - `PresetScene::writeJson` (`.cpp` lines 52-73): one guard on the base
//!   class's return, then fourteen key/value emissions. The *ordering* and
//!   the key names are the behavior; ported as an ordered key list plus the
//!   value each key carries.
//!
//! **Refused**, with the reason:
//! - `PresetLibrary<T>` in its entirety (`.h` lines 42-106). `readJson`
//!   iterates a `json` array and does unchecked `jpreset["name"]`
//!   subscripting; `writeJson` does `json::push_back`; `load`/`save` are
//!   `std::ifstream`/`std::ofstream` with `fprintf(stderr, ...)`,
//!   `std::setw(4)`, and `o.bad()` stream-state checks. Filesystem and JSON
//!   document plumbing end to end; the `std::map` used for the `presetMap` is
//!   ordinary sorted-container behavior with no RT64-specific semantics.
//! - `hlslpp::to_json`/`from_json` for `float3`/`float4` (`.cpp` lines 7-28).
//!   `j = { v[0], v[1], v[2] }` constructs a `json` array; `j.at(i).get_to()`
//!   is nlohmann's checked accessor, whose throw-on-out-of-range is a
//!   property of nlohmann, not of RT64. No arithmetic.
//! - The `jsonObj.value(...)` lookups themselves in both `readJson` bodies.
//!   Library plumbing; supplied pre-extracted as `Option`s so that the
//!   *defaulting* -- which is the real behavior -- stays observable.
//! - The `jsonObj[key] = value` assignments in both `writeJson` bodies as
//!   literal `json` mutation. What is ported instead is the key name and
//!   emission order, which is all the local behavior there is.
//! - `PresetSceneLibrary` and `PresetSceneLibraryInspector` as types (`.h`
//!   lines 32-37): empty derivations plus two methods taking a
//!   `RenderWindow`.
//! - `PresetSceneLibraryInspector::inspectLibrary` (`.cpp` lines 77-100) and
//!   `::inspectSelection` (lines 102-141): 63 lines, wall-to-wall ImGui
//!   (`BeginChild`, `PushID`, `Selectable`, `Checkbox`, `DragFloat`,
//!   `DragFloat3`, `PopID`, `EndChild`) plus `inspectPresetBegin` /
//!   `inspectPresetEnd` / `inspectBottom`, which live in
//!   `rt64_preset_inspector.h` -- not a cited source here. Every value in
//!   them is produced by a widget call.
//!
//!   The one branch inside `inspectSelection` that is not itself a widget
//!   call -- `if (scene.estimateAmbientLight)` at line 121, selecting between
//!   showing one `DragFloat` and two `DragFloat3`s -- is refused with the
//!   rest: its two arms differ only in which widgets are drawn, so it has no
//!   value-level result to pin.
//!
//!   The `DragFloat` min/max arguments (`0.0f, FLT_MAX` on ambient intensity,
//!   `0.0f, 100.0f` on the colors and GI strengths, `0.0f, 20.0f` on
//!   exposure, `-20.0f, 20.0f` on the two eye-adaption values, `0.0f, 4.0f`
//!   on the update time) are ImGui widget configuration, not clamps RT64
//!   applies in its own code, and are deliberately NOT ported as clamps.
//!   `PresetScene` contains no clamp, no `std::min`/`std::max`, and no
//!   arithmetic of any kind -- there is no comparison in the ported surface
//!   whose strictness could be pinned, and no float operation that could
//!   raise a NaN or signed-zero question.
//!
//! ## The `PresetBase` finding
//!
//! Two sibling port cards -- `rt64_preset_material.rs` and
//! `rt64_preset_light.rs` -- did not have `rt64_preset.cpp` among their cited
//! sources and correctly declined to guess at `PresetBase::readJson`. The
//! material card modelled it as a `preset_base_ok: bool` precondition
//! carrying a rejecting `ReadJsonReject::PresetBase` arm. That was the right
//! call under its citations, and the guard-ordering behavior it pinned is
//! faithful to its own file's text.
//!
//! With `rt64_preset.cpp` now cited, the actual implementation is visible and
//! it is **unconditional**: `PresetBase::readJson` sets one field and
//! `return true`, with no failure path at all, and `writeJson` likewise. So
//! the `!PresetBase::readJson(jsonObj)` guard at the head of every derived
//! `readJson` -- material's, light's, draw-call's, and this file's -- is dead
//! code in the pinned tree: it can never be taken. [`preset_base_read_json`]
//! and [`preset_base_write_json`] both return `bool` rather than `()`
//! precisely so this stays visible at the type level, and
//! [`PRESET_BASE_READ_JSON_ALWAYS_SUCCEEDS`] pins it as a value.
//!
//! This does not make the sibling cards wrong. `readJson` is `virtual`, so a
//! future or downstream override could return `false`, and their
//! precondition parameter keeps that observable. It does mean their rejecting
//! arm is unreachable for the pinned `PresetBase` itself.
//!
//! ## Verbatim key logic
//!
//! ```text
//! // rt64_preset.h (lines 34-40)
//! struct PresetBase {
//!     bool enabled = true;
//!
//!     virtual ~PresetBase() = default;
//!     virtual bool readJson(const json &jsonObj);
//!     virtual bool writeJson(json &jsonObj) const;
//! };
//!
//! // rt64_preset.cpp (lines 31-39)
//! bool PresetBase::readJson(const json &jsonObj) {
//!     enabled = jsonObj.value("enabled", false);
//!     return true;
//! }
//!
//! bool PresetBase::writeJson(json &jsonObj) const {
//!     jsonObj["enabled"] = enabled;
//!     return true;
//! }
//!
//! // rt64_preset_scene.cpp (lines 12-27)
//! PresetScene::PresetScene() {
//!     estimateAmbientLight = true;
//!     ambientLightIntensity = 0.03f;
//!     ambientBaseColor = { 0.03f, 0.03f, 0.03f };
//!     ambientNoGIColor = { 0.03f, 0.03f, 0.03f };
//!     eyeLightDiffuseColor = { 0.008f, 0.008f, 0.008f };
//!     eyeLightSpecularColor = { 0.004f, 0.004f, 0.004f };
//!     giDiffuseStrength = 1.5f;
//!     giBackgroundStrength = 0.5f;
//!     tonemapExposure = 0.35f;
//!     tonemapWhite = 1.05f;
//!     tonemapBlack = 0.0f;
//!     minLuminance = 0.3f;
//!     luminanceRange = 0.0f;
//!     lumaUpdateTime = 1.1f;
//! }
//!
//! // rt64_preset_scene.cpp (lines 29-50)
//! bool PresetScene::readJson(const json &jsonObj) {
//!     if (!PresetBase::readJson(jsonObj)) {
//!         return false;
//!     }
//!
//!     estimateAmbientLight = jsonObj.value("estimateAmbientLight", true);
//!     ambientLightIntensity = jsonObj.value("ambientLightIntensity", 0.03f);
//!     ambientBaseColor = jsonObj.value("ambientBaseColor", hlslpp::float3(0.0f, 0.0f, 0.0f));
//!     ambientNoGIColor = jsonObj.value("ambientNoGIColor", hlslpp::float3(0.0f, 0.0f, 0.0f));
//!     eyeLightDiffuseColor = jsonObj.value("eyeLightDiffuseColor", hlslpp::float3(0.0f, 0.0f, 0.0f));
//!     eyeLightSpecularColor = jsonObj.value("eyeLightSpecularColor", hlslpp::float3(0.0f, 0.0f, 0.0f));
//!     giDiffuseStrength = jsonObj.value("giDiffuseStrength", 0.0f);
//!     giBackgroundStrength = jsonObj.value("giBackgroundStrength", 0.0f);
//!     tonemapExposure = jsonObj.value("tonemapExposure", 0.0f);
//!     tonemapWhite = jsonObj.value("tonemapWhite", 0.0f);
//!     tonemapBlack = jsonObj.value("tonemapBlack", 0.0f);
//!     minLuminance = jsonObj.value("minLuminance", 0.0f);
//!     luminanceRange = jsonObj.value("luminanceRange", 0.0f);
//!     lumaUpdateTime = jsonObj.value("lumaUpdateTime", 0.0f);
//!
//!     return true;
//! }
//! ```
//!
//! ## Upstream oddities pinned, not fixed
//!
//! - **`enabled`'s default disagrees with its member initializer.** `.h` line
//!   35 gives `bool enabled = true`, but `.cpp` line 32 reads
//!   `jsonObj.value("enabled", false)`. A default-constructed `PresetBase`
//!   has `enabled == true`; the same object after `readJson({})` has
//!   `enabled == false`. This is the identical shape the material card found
//!   in `ExtraParams`, here in the base class. Pinned by
//!   [`ENABLED_CONSTRUCTOR_DEFAULT`] / [`ENABLED_JSON_DEFAULT`] and by
//!   `enabled_constructor_and_json_defaults_disagree`.
//!
//! - **Ten of `PresetScene`'s fourteen JSON defaults disagree with the
//!   constructor.** Only `estimateAmbientLight` (`true`),
//!   `ambientLightIntensity` (`0.03f`), `tonemapBlack` (`0.0f`) and
//!   `luminanceRange` (`0.0f`) agree. The four `float3` colors default to
//!   `(0,0,0)` from JSON but to non-zero greys from the constructor, and
//!   `giDiffuseStrength`, `giBackgroundStrength`, `tonemapExposure`,
//!   `tonemapWhite`, `minLuminance`, `lumaUpdateTime` all default to `0.0f`
//!   from JSON against `1.5`, `0.5`, `0.35`, `1.05`, `0.3`, `1.1` from the
//!   constructor. So `PresetScene()` and `PresetScene()` + `readJson({})`
//!   differ in eleven of fifteen fields (the ten here plus `enabled`).
//!   Pinned field by field by `default_construct_and_empty_json_differ_in_*`.
//!
//! - **Every read is an unconditional overwrite.** An absent optional key
//!   writes its default *over* the receiver's prior value; it does not
//!   preserve it. Every field is written on the success path, so this is
//!   ported as returning a fresh value. Pinned against a deliberately
//!   NON-default receiver by
//!   `read_json_all_absent_overwrites_non_default_receiver`, so the test is
//!   not vacuous.
//!
//! - **No field lacks a JSON key.** All fourteen `PresetScene` fields plus
//!   `PresetBase::enabled` are read and written; nothing carries through.
//!   Pinned by `every_declared_field_has_a_read_and_a_write_key`.
//!
//! - **The base-class guard is dead code.** See "The `PresetBase` finding".
//!
//! - **`writeJson` emits `enabled` first.** The base's `jsonObj["enabled"]`
//!   runs before any `PresetScene` key, because the derived `writeJson` calls
//!   `PresetBase::writeJson` at its head. Since `nlohmann::json` objects are
//!   key-sorted on serialization this is not observable in the output file,
//!   but the *call* order is what the source specifies, and it is what
//!   [`write_json_keys`] returns.
//!
//! ## Reuse, not new type
//!
//! `hlslpp::float3` is represented by [`fn64_render_ir::Vec3`], reused rather
//! than redeclared. `rsp_math.rs` documents `Vec3` as "a backend-neutral
//! 3-component float vector, matching HLSL `float3`", which is exactly what
//! `hlslpp::float3` is, and the sibling `rt64_preset_light.rs` card made the
//! same choice for the same C++ type.
//!
//! This resolves an inconsistency worth naming: `rt64_preset_material.rs`
//! used `[f32; 3]` for its three-float values. That is a *different* C++
//! type -- `interop::float3`, a plain shader-interop POD -- not
//! `hlslpp::float3`, so the two cards are not actually in conflict. The rule
//! this module follows, and recommends: `hlslpp::float3` maps to
//! `fn64_render_ir::Vec3`; `interop::float3` maps to `[f32; 3]`.
//!
//! `bool` and `f32` are used directly for the scalar fields. No new numeric
//! type is introduced.
//!
//! ## Admitted domain
//!
//! `PresetBase` and `PresetScene` as plain data, their constructor defaults,
//! the read-side default table and the write-side key order. The JSON lookups
//! are admitted only as pre-extracted `Option`s: `None` models "key absent"
//! (which selects the fallback) and `Some(v)` models "key present with value
//! `v`". Nothing here reads a file, parses JSON, or dispatches virtually.
//!
//! ## Nonclaims
//!
//! - This module is **not wired in**: `lib.rs` declares it `mod`, not
//!   `pub mod`, and nothing calls it. It is a characterization surface.
//! - It does not model `nlohmann::json`'s type-coercion behavior. Real
//!   `jsonObj.value("k", d)` returns `d` when the key is *absent*, but
//!   **throws** `json::type_error` when the key is present with an
//!   incompatible type -- it does not fall back. Only the absent-key path is
//!   ported; the type-error path is a nlohmann property and is out of scope.
//! - It does not port `PresetLibrary`'s `load`/`save`, nor its `readJson`'s
//!   skip-on-empty-name and skip-on-failed-preset `continue`s. Those sit on
//!   unchecked `jpreset["name"]` subscripting, which throws on a missing key.
//! - It does not port the four `hlslpp` `to_json`/`from_json` functions, so
//!   it makes no claim about how a `float3` is spelled on disk (an array of
//!   three numbers, per `.cpp` line 9, but the marshalling is nlohmann's).
//! - It makes no claim about `PresetSceneLibraryInspector`'s behavior, the
//!   ImGui widget ranges, or the selection/rename flows.
//! - It makes no claim that the four cited files are fully ported. They are
//!   not; see the inventory drift disclosure above.
//! - **No UB was found in these four files** within the ported surface. There
//!   is no array indexing, no signed arithmetic, and no pointer manipulation
//!   in the ported constructs, so nothing here is a deviation -- every test
//!   pins the original. (The refused `PresetLibrary`/`hlslpp` code can throw,
//!   but throwing is defined behavior, not UB.)

use fn64_render_ir::Vec3;

// ---------------------------------------------------------------------------
// PresetBase
// ---------------------------------------------------------------------------

/// `PresetBase::enabled`'s **member initializer** (`rt64_preset.h` line 35:
/// `bool enabled = true`), i.e. the value a default-constructed `PresetBase`
/// carries.
///
/// Deliberately distinct from [`ENABLED_JSON_DEFAULT`], which is `false`.
pub const ENABLED_CONSTRUCTOR_DEFAULT: bool = true;

/// `PresetBase::readJson`'s **fallback** for an absent `"enabled"` key
/// (`rt64_preset.cpp` line 32: `jsonObj.value("enabled", false)`).
///
/// Deliberately distinct from [`ENABLED_CONSTRUCTOR_DEFAULT`], which is
/// `true`. This disagreement is upstream behavior, pinned not fixed.
pub const ENABLED_JSON_DEFAULT: bool = false;

/// The JSON key `PresetBase` reads and writes (`rt64_preset.cpp` lines 32,
/// 37).
pub const ENABLED_KEY: &str = "enabled";

/// `PresetBase::readJson` returns a literal `true` with no failure path
/// (`rt64_preset.cpp` line 33), so the `!PresetBase::readJson(...)` guard at
/// the head of every derived `readJson` is dead code for the pinned base.
///
/// See the module doc's "The `PresetBase` finding".
pub const PRESET_BASE_READ_JSON_ALWAYS_SUCCEEDS: bool = true;

/// `PresetBase::writeJson` likewise returns a literal `true`
/// (`rt64_preset.cpp` line 38).
pub const PRESET_BASE_WRITE_JSON_ALWAYS_SUCCEEDS: bool = true;

/// Literal port of `PresetBase` (`rt64_preset.h` lines 34-40), minus the
/// virtual destructor and the two virtual method declarations.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct PresetBase {
    /// `bool enabled = true;`
    pub enabled: bool,
}

impl PresetBase {
    /// The default-constructed `PresetBase`, i.e. the `.h` member
    /// initializer: `enabled == true`.
    pub const fn default_construct() -> PresetBase {
        PresetBase {
            enabled: ENABLED_CONSTRUCTOR_DEFAULT,
        }
    }
}

impl Default for PresetBase {
    fn default() -> PresetBase {
        PresetBase::default_construct()
    }
}

/// Literal port of `PresetBase::readJson` (`rt64_preset.cpp` lines 31-34),
/// minus the `nlohmann::json` lookup (refused; supplied pre-extracted).
///
/// ```text
/// enabled = jsonObj.value("enabled", false);
/// return true;
/// ```
///
/// `enabled_value` is `None` when the `"enabled"` key is absent, which
/// selects the [`ENABLED_JSON_DEFAULT`] fallback of `false`. The assignment
/// is **unconditional**: an absent key writes `false` over `base.enabled`'s
/// prior value rather than preserving it.
///
/// Returns `true` always -- the source has no failure path.
pub fn preset_base_read_json(base: &mut PresetBase, enabled_value: Option<bool>) -> bool {
    base.enabled = match enabled_value {
        Some(value) => value,
        None => ENABLED_JSON_DEFAULT,
    };

    true
}

/// Literal port of `PresetBase::writeJson` (`rt64_preset.cpp` lines 36-39),
/// minus the `nlohmann::json` mutation (refused).
///
/// ```text
/// jsonObj["enabled"] = enabled;
/// return true;
/// ```
///
/// Reports the single key written and the value it carries, then the
/// constant `true` the source returns.
pub fn preset_base_write_json(base: &PresetBase) -> ((&'static str, bool), bool) {
    ((ENABLED_KEY, base.enabled), true)
}

// ---------------------------------------------------------------------------
// PresetScene
// ---------------------------------------------------------------------------

/// Literal port of `PresetScene` (`rt64_preset_scene.h` lines 11-30), in
/// declaration order, with the inherited [`PresetBase`] as the first member
/// (matching `struct PresetScene : public PresetBase`).
///
/// `hlslpp::float3` is [`Vec3`]; see the module doc's "Reuse, not new type".
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct PresetScene {
    /// The inherited `PresetBase` sub-object.
    pub base: PresetBase,
    /// `bool estimateAmbientLight;`
    pub estimate_ambient_light: bool,
    /// `float ambientLightIntensity;`
    pub ambient_light_intensity: f32,
    /// `hlslpp::float3 ambientBaseColor;`
    pub ambient_base_color: Vec3,
    /// `hlslpp::float3 ambientNoGIColor;`
    pub ambient_no_gi_color: Vec3,
    /// `hlslpp::float3 eyeLightDiffuseColor;`
    pub eye_light_diffuse_color: Vec3,
    /// `hlslpp::float3 eyeLightSpecularColor;`
    pub eye_light_specular_color: Vec3,
    /// `float giDiffuseStrength;`
    pub gi_diffuse_strength: f32,
    /// `float giBackgroundStrength;`
    pub gi_background_strength: f32,
    /// `float tonemapExposure;`
    pub tonemap_exposure: f32,
    /// `float tonemapWhite;`
    pub tonemap_white: f32,
    /// `float tonemapBlack;`
    pub tonemap_black: f32,
    /// `float minLuminance;`
    pub min_luminance: f32,
    /// `float luminanceRange;`
    pub luminance_range: f32,
    /// `float lumaUpdateTime;`
    pub luma_update_time: f32,
}

impl PresetScene {
    /// Builds a [`PresetScene`] from **positional** arguments in the source's
    /// declaration order (`rt64_preset_scene.h` lines 11-30), with the
    /// inherited `PresetBase` sub-object first.
    ///
    /// This transcribes the header's member order into an argument order a
    /// reviewer can read against `rt64_preset_scene.h` directly, and spares
    /// callers fifteen repeated field names.
    ///
    /// **It does not detect a declaration reorder, and no test here claims it
    /// does.** The body uses field-init shorthand, which binds by identifier
    /// rather than position, as do the accessors below.
    ///
    /// This struct is nonetheless worth flagging as genuinely unpinned. Two
    /// runs admit a completely silent swap: the four adjacent `Vec3` colours
    /// (`ambientBaseColor`, `ambientNoGIColor`, `eyeLightDiffuseColor`,
    /// `eyeLightSpecularColor`) and the eight adjacent trailing scalar
    /// `float`s. The default constructor makes it worse rather than better --
    /// `ambientBaseColor` and `ambientNoGIColor` default to the *identical*
    /// `(0.03, 0.03, 0.03)` triple, so even a field-by-field default
    /// comparison cannot tell those two apart. Nor does [`scene_write_json`]
    /// close the gap: it reads each field *by name* into a fixed list
    /// position, so swapping two declarations leaves its emitted key/value
    /// pairs byte-identical.
    ///
    /// Closing it would mean generating the declaration and an order witness
    /// from a single source, which changes the port's source text rather than
    /// adding a test.
    ///
    /// Nothing here is a memory layout, size, or byte-offset claim.
    #[must_use]
    #[allow(clippy::too_many_arguments)]
    pub const fn in_source_order(
        base: PresetBase,
        estimate_ambient_light: bool,
        ambient_light_intensity: f32,
        ambient_base_color: Vec3,
        ambient_no_gi_color: Vec3,
        eye_light_diffuse_color: Vec3,
        eye_light_specular_color: Vec3,
        gi_diffuse_strength: f32,
        gi_background_strength: f32,
        tonemap_exposure: f32,
        tonemap_white: f32,
        tonemap_black: f32,
        min_luminance: f32,
        luminance_range: f32,
        luma_update_time: f32,
    ) -> PresetScene {
        PresetScene {
            base,
            estimate_ambient_light,
            ambient_light_intensity,
            ambient_base_color,
            ambient_no_gi_color,
            eye_light_diffuse_color,
            eye_light_specular_color,
            gi_diffuse_strength,
            gi_background_strength,
            tonemap_exposure,
            tonemap_white,
            tonemap_black,
            min_luminance,
            luminance_range,
            luma_update_time,
        }
    }

    /// The four `hlslpp::float3` members in declaration order.
    #[must_use]
    pub const fn float3_members_in_source_order(&self) -> [Vec3; 4] {
        [
            self.ambient_base_color,
            self.ambient_no_gi_color,
            self.eye_light_diffuse_color,
            self.eye_light_specular_color,
        ]
    }

    /// The nine scalar `float` members in declaration order --
    /// `ambientLightIntensity` (which precedes the colours) then the eight
    /// that follow them.
    #[must_use]
    pub const fn float_members_in_source_order(&self) -> [f32; 9] {
        [
            self.ambient_light_intensity,
            self.gi_diffuse_strength,
            self.gi_background_strength,
            self.tonemap_exposure,
            self.tonemap_white,
            self.tonemap_black,
            self.min_luminance,
            self.luminance_range,
            self.luma_update_time,
        ]
    }

    /// Literal port of `PresetScene::PresetScene()` (`rt64_preset_scene.cpp`
    /// lines 12-27), in assignment order.
    ///
    /// The `base` sub-object is default-constructed first, per C++
    /// base-before-member initialization, so `base.enabled` is `true` here --
    /// the constructor body never touches it.
    ///
    /// Ten of these fourteen values disagree with the corresponding
    /// `readJson` fallback; see the module doc's oddities section.
    pub const fn default_construct() -> PresetScene {
        PresetScene {
            base: PresetBase::default_construct(),
            estimate_ambient_light: true,
            ambient_light_intensity: 0.03f32,
            ambient_base_color: Vec3::new(0.03f32, 0.03f32, 0.03f32),
            ambient_no_gi_color: Vec3::new(0.03f32, 0.03f32, 0.03f32),
            eye_light_diffuse_color: Vec3::new(0.008f32, 0.008f32, 0.008f32),
            eye_light_specular_color: Vec3::new(0.004f32, 0.004f32, 0.004f32),
            gi_diffuse_strength: 1.5f32,
            gi_background_strength: 0.5f32,
            tonemap_exposure: 0.35f32,
            tonemap_white: 1.05f32,
            tonemap_black: 0.0f32,
            min_luminance: 0.3f32,
            luminance_range: 0.0f32,
            luma_update_time: 1.1f32,
        }
    }
}

impl Default for PresetScene {
    fn default() -> PresetScene {
        PresetScene::default_construct()
    }
}

/// The fourteen `PresetScene` JSON values `readJson` reads, already extracted
/// from the `json` object -- `None` meaning "key absent", which selects the
/// fallback.
///
/// This shape exists because the `nlohmann::json` lookups themselves are
/// refused (see module doc); the ported behavior is what happens to the
/// *defaults*, which is fully determined by presence and value.
///
/// `preset_base_enabled` carries the base class's `"enabled"` lookup, since
/// `PresetScene::readJson` delegates to `PresetBase::readJson` at its head.
#[derive(Copy, Clone, Debug, PartialEq, Default)]
pub struct SceneReadJsonInput {
    /// `jsonObj.value("enabled", false)`, consumed by `PresetBase::readJson`.
    pub preset_base_enabled: Option<bool>,
    /// `jsonObj.value("estimateAmbientLight", true)`.
    pub estimate_ambient_light: Option<bool>,
    /// `jsonObj.value("ambientLightIntensity", 0.03f)`.
    pub ambient_light_intensity: Option<f32>,
    /// `jsonObj.value("ambientBaseColor", hlslpp::float3(0.0f, 0.0f, 0.0f))`.
    pub ambient_base_color: Option<Vec3>,
    /// `jsonObj.value("ambientNoGIColor", hlslpp::float3(0.0f, 0.0f, 0.0f))`.
    pub ambient_no_gi_color: Option<Vec3>,
    /// `jsonObj.value("eyeLightDiffuseColor", hlslpp::float3(0, 0, 0))`.
    pub eye_light_diffuse_color: Option<Vec3>,
    /// `jsonObj.value("eyeLightSpecularColor", hlslpp::float3(0, 0, 0))`.
    pub eye_light_specular_color: Option<Vec3>,
    /// `jsonObj.value("giDiffuseStrength", 0.0f)`.
    pub gi_diffuse_strength: Option<f32>,
    /// `jsonObj.value("giBackgroundStrength", 0.0f)`.
    pub gi_background_strength: Option<f32>,
    /// `jsonObj.value("tonemapExposure", 0.0f)`.
    pub tonemap_exposure: Option<f32>,
    /// `jsonObj.value("tonemapWhite", 0.0f)`.
    pub tonemap_white: Option<f32>,
    /// `jsonObj.value("tonemapBlack", 0.0f)`.
    pub tonemap_black: Option<f32>,
    /// `jsonObj.value("minLuminance", 0.0f)`.
    pub min_luminance: Option<f32>,
    /// `jsonObj.value("luminanceRange", 0.0f)`.
    pub luminance_range: Option<f32>,
    /// `jsonObj.value("lumaUpdateTime", 0.0f)`.
    pub luma_update_time: Option<f32>,
}

impl SceneReadJsonInput {
    /// The input modelling an **empty JSON object**: every key absent, so
    /// every fallback fires.
    ///
    /// Identical to `SceneReadJsonInput::default()`; named for the source
    /// situation it models.
    pub const fn all_absent() -> SceneReadJsonInput {
        SceneReadJsonInput {
            preset_base_enabled: None,
            estimate_ambient_light: None,
            ambient_light_intensity: None,
            ambient_base_color: None,
            ambient_no_gi_color: None,
            eye_light_diffuse_color: None,
            eye_light_specular_color: None,
            gi_diffuse_strength: None,
            gi_background_strength: None,
            tonemap_exposure: None,
            tonemap_white: None,
            tonemap_black: None,
            min_luminance: None,
            luminance_range: None,
            luma_update_time: None,
        }
    }
}

/// Literal port of `PresetScene::readJson` (`rt64_preset_scene.cpp` lines
/// 29-50), minus the `nlohmann::json` lookups (refused; supplied
/// pre-extracted via [`SceneReadJsonInput`]).
///
/// Branch structure, preserved exactly and in order:
/// 1. Call `PresetBase::readJson`, which writes `scene.base.enabled`. If it
///    returns `false`, return `false` immediately, leaving every
///    `PresetScene` field untouched. For the pinned `PresetBase` this arm is
///    unreachable ([`PRESET_BASE_READ_JSON_ALWAYS_SUCCEEDS`]) -- but note
///    that the base call has **already mutated `enabled`** by the time its
///    return value is inspected, so the guard would not roll that back.
/// 2. Assign all fourteen fields unconditionally -- an absent optional key
///    writes its default *over* `scene`'s prior value, it does not preserve
///    it -- then return `true`.
///
/// Mutates `scene` in place rather than returning a fresh value, so that
/// step 1's already-happened `enabled` write stays observable.
pub fn scene_read_json(scene: &mut PresetScene, input: &SceneReadJsonInput) -> bool {
    if !preset_base_read_json(&mut scene.base, input.preset_base_enabled) {
        return false;
    }

    scene.estimate_ambient_light = match input.estimate_ambient_light {
        Some(value) => value,
        None => true,
    };
    scene.ambient_light_intensity = match input.ambient_light_intensity {
        Some(value) => value,
        None => 0.03f32,
    };
    scene.ambient_base_color = match input.ambient_base_color {
        Some(value) => value,
        None => Vec3::new(0.0f32, 0.0f32, 0.0f32),
    };
    scene.ambient_no_gi_color = match input.ambient_no_gi_color {
        Some(value) => value,
        None => Vec3::new(0.0f32, 0.0f32, 0.0f32),
    };
    scene.eye_light_diffuse_color = match input.eye_light_diffuse_color {
        Some(value) => value,
        None => Vec3::new(0.0f32, 0.0f32, 0.0f32),
    };
    scene.eye_light_specular_color = match input.eye_light_specular_color {
        Some(value) => value,
        None => Vec3::new(0.0f32, 0.0f32, 0.0f32),
    };
    scene.gi_diffuse_strength = match input.gi_diffuse_strength {
        Some(value) => value,
        None => 0.0f32,
    };
    scene.gi_background_strength = match input.gi_background_strength {
        Some(value) => value,
        None => 0.0f32,
    };
    scene.tonemap_exposure = match input.tonemap_exposure {
        Some(value) => value,
        None => 0.0f32,
    };
    scene.tonemap_white = match input.tonemap_white {
        Some(value) => value,
        None => 0.0f32,
    };
    scene.tonemap_black = match input.tonemap_black {
        Some(value) => value,
        None => 0.0f32,
    };
    scene.min_luminance = match input.min_luminance {
        Some(value) => value,
        None => 0.0f32,
    };
    scene.luminance_range = match input.luminance_range {
        Some(value) => value,
        None => 0.0f32,
    };
    scene.luma_update_time = match input.luma_update_time {
        Some(value) => value,
        None => 0.0f32,
    };

    true
}

/// A single key written by `PresetScene::writeJson`, paired with the value it
/// carries. `PresetScene` writes exactly three value shapes.
#[derive(Copy, Clone, Debug, PartialEq)]
pub enum SceneJsonValue {
    /// `jsonObj[key] = <bool>` -- `enabled`, `estimateAmbientLight`.
    Bool(bool),
    /// `jsonObj[key] = <float>` -- the eight scalar floats.
    Float(f32),
    /// `jsonObj[key] = <hlslpp::float3>` -- the four colors, marshalled by
    /// the refused `hlslpp::to_json`.
    Float3(Vec3),
}

/// Literal port of `PresetScene::writeJson` (`rt64_preset_scene.cpp` lines
/// 52-73), minus the `nlohmann::json` mutation (refused).
///
/// Branch structure, preserved exactly:
/// 1. Call `PresetBase::writeJson`; if it returns `false`, return `false`
///    with nothing further emitted. Unreachable for the pinned base
///    ([`PRESET_BASE_WRITE_JSON_ALWAYS_SUCCEEDS`]).
/// 2. Emit the fourteen `PresetScene` keys, in source order.
///
/// The returned list is in **call order**, so `"enabled"` -- emitted by the
/// base at the head -- is first. `nlohmann::json` key-sorts on serialization,
/// so the on-disk order differs; the call order is what the source specifies.
///
/// The `bool` is the function's return value.
pub fn scene_write_json(scene: &PresetScene) -> (Vec<(&'static str, SceneJsonValue)>, bool) {
    let ((base_key, base_enabled), base_ok) = preset_base_write_json(&scene.base);
    if !base_ok {
        return (Vec::new(), false);
    }

    let keys = vec![
        (base_key, SceneJsonValue::Bool(base_enabled)),
        (
            "estimateAmbientLight",
            SceneJsonValue::Bool(scene.estimate_ambient_light),
        ),
        (
            "ambientLightIntensity",
            SceneJsonValue::Float(scene.ambient_light_intensity),
        ),
        (
            "ambientBaseColor",
            SceneJsonValue::Float3(scene.ambient_base_color),
        ),
        (
            "ambientNoGIColor",
            SceneJsonValue::Float3(scene.ambient_no_gi_color),
        ),
        (
            "eyeLightDiffuseColor",
            SceneJsonValue::Float3(scene.eye_light_diffuse_color),
        ),
        (
            "eyeLightSpecularColor",
            SceneJsonValue::Float3(scene.eye_light_specular_color),
        ),
        (
            "giDiffuseStrength",
            SceneJsonValue::Float(scene.gi_diffuse_strength),
        ),
        (
            "giBackgroundStrength",
            SceneJsonValue::Float(scene.gi_background_strength),
        ),
        (
            "tonemapExposure",
            SceneJsonValue::Float(scene.tonemap_exposure),
        ),
        ("tonemapWhite", SceneJsonValue::Float(scene.tonemap_white)),
        ("tonemapBlack", SceneJsonValue::Float(scene.tonemap_black)),
        ("minLuminance", SceneJsonValue::Float(scene.min_luminance)),
        (
            "luminanceRange",
            SceneJsonValue::Float(scene.luminance_range),
        ),
        (
            "lumaUpdateTime",
            SceneJsonValue::Float(scene.luma_update_time),
        ),
    ];

    (keys, true)
}

/// The keys `PresetScene::writeJson` emits, in call order, without values.
///
/// Convenience view over [`scene_write_json`] for order and coverage checks.
pub fn write_json_keys(scene: &PresetScene) -> Vec<&'static str> {
    let (pairs, _ok) = scene_write_json(scene);
    pairs.into_iter().map(|(key, _value)| key).collect()
}

/// The keys `PresetScene::readJson` consults, in source order -- the base's
/// `"enabled"` first, then the fourteen `PresetScene` keys.
///
/// Fixed list transcribed from `rt64_preset.cpp` line 32 and
/// `rt64_preset_scene.cpp` lines 34-47.
pub const READ_JSON_KEYS: [&str; 15] = [
    "enabled",
    "estimateAmbientLight",
    "ambientLightIntensity",
    "ambientBaseColor",
    "ambientNoGIColor",
    "eyeLightDiffuseColor",
    "eyeLightSpecularColor",
    "giDiffuseStrength",
    "giBackgroundStrength",
    "tonemapExposure",
    "tonemapWhite",
    "tonemapBlack",
    "minLuminance",
    "luminanceRange",
    "lumaUpdateTime",
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preset_scene_in_source_order_maps_arguments_to_named_fields() {
        // Checks that `in_source_order`'s fifteen arguments land in the fields
        // its parameter names promise, and that both accessors agree.
        //
        // This is NOT a reorder detector: the four adjacent `Vec3` colours and
        // the eight adjacent trailing `f32`s can still be swapped in the
        // declaration with every test in this module staying green, because
        // field-init shorthand and field access bind by name. Verified by
        // mutation; see the constructor docs for why no added test can close
        // this.
        //
        // Nothing here is a memory layout claim.
        let s = PresetScene::in_source_order(
            PresetBase { enabled: true },
            false,
            1.0,
            Vec3::new(2.0, 3.0, 4.0),
            Vec3::new(5.0, 6.0, 7.0),
            Vec3::new(8.0, 9.0, 10.0),
            Vec3::new(11.0, 12.0, 13.0),
            14.0,
            15.0,
            16.0,
            17.0,
            18.0,
            19.0,
            20.0,
            21.0,
        );

        assert_eq!(s.base, PresetBase { enabled: true });
        assert!(!s.estimate_ambient_light);
        assert_eq!(s.ambient_light_intensity, 1.0);
        assert_eq!(s.ambient_base_color, Vec3::new(2.0, 3.0, 4.0));
        assert_eq!(s.ambient_no_gi_color, Vec3::new(5.0, 6.0, 7.0));
        assert_eq!(s.eye_light_diffuse_color, Vec3::new(8.0, 9.0, 10.0));
        assert_eq!(s.eye_light_specular_color, Vec3::new(11.0, 12.0, 13.0));
        assert_eq!(s.gi_diffuse_strength, 14.0);
        assert_eq!(s.gi_background_strength, 15.0);
        assert_eq!(s.tonemap_exposure, 16.0);
        assert_eq!(s.tonemap_white, 17.0);
        assert_eq!(s.tonemap_black, 18.0);
        assert_eq!(s.min_luminance, 19.0);
        assert_eq!(s.luminance_range, 20.0);
        assert_eq!(s.luma_update_time, 21.0);

        // Second, independent derivation of each same-typed run's order.
        assert_eq!(
            s.float3_members_in_source_order(),
            [
                Vec3::new(2.0, 3.0, 4.0),
                Vec3::new(5.0, 6.0, 7.0),
                Vec3::new(8.0, 9.0, 10.0),
                Vec3::new(11.0, 12.0, 13.0),
            ]
        );
        assert_eq!(
            s.float_members_in_source_order(),
            [1.0, 14.0, 15.0, 16.0, 17.0, 18.0, 19.0, 20.0, 21.0]
        );
    }

    /// A receiver whose every field differs from BOTH the constructor default
    /// and the `readJson` fallback, so that an overwrite test cannot pass
    /// vacuously.
    fn non_default_receiver() -> PresetScene {
        PresetScene {
            base: PresetBase { enabled: true },
            estimate_ambient_light: false,
            ambient_light_intensity: -7.5f32,
            ambient_base_color: Vec3::new(11.0f32, 12.0f32, 13.0f32),
            ambient_no_gi_color: Vec3::new(21.0f32, 22.0f32, 23.0f32),
            eye_light_diffuse_color: Vec3::new(31.0f32, 32.0f32, 33.0f32),
            eye_light_specular_color: Vec3::new(41.0f32, 42.0f32, 43.0f32),
            gi_diffuse_strength: -1.0f32,
            gi_background_strength: -2.0f32,
            tonemap_exposure: -3.0f32,
            tonemap_white: -4.0f32,
            tonemap_black: -5.0f32,
            min_luminance: -6.0f32,
            luminance_range: -7.0f32,
            luma_update_time: -8.0f32,
        }
    }

    // -- PresetBase defaults ------------------------------------------------

    /// `.h` line 35: `bool enabled = true`.
    #[test]
    fn preset_base_member_initializer_is_true() {
        assert!(PresetBase::default_construct().enabled);
        assert!(ENABLED_CONSTRUCTOR_DEFAULT);
    }

    /// `.cpp` line 32's fallback is `false`, not the initializer's `true`.
    #[test]
    fn preset_base_json_fallback_is_false() {
        assert!(!ENABLED_JSON_DEFAULT);
    }

    /// **The oddity.** A default-constructed `PresetBase` and one loaded from
    /// `{}` disagree on `enabled`.
    #[test]
    fn enabled_constructor_and_json_defaults_disagree() {
        assert_ne!(ENABLED_CONSTRUCTOR_DEFAULT, ENABLED_JSON_DEFAULT);

        let ctor = PresetBase::default_construct();
        let mut loaded = PresetBase::default_construct();
        preset_base_read_json(&mut loaded, None);

        assert!(ctor.enabled);
        assert!(!loaded.enabled);
        assert_ne!(ctor, loaded);
    }

    /// `Default` agrees with the named constructor.
    #[test]
    fn preset_base_default_trait_matches_default_construct() {
        assert_eq!(PresetBase::default(), PresetBase::default_construct());
    }

    // -- PresetBase::readJson -----------------------------------------------

    /// Absent key selects the `false` fallback -- over a `true` receiver, so
    /// the test is not vacuous.
    #[test]
    fn preset_base_read_json_absent_overwrites_true_with_false() {
        let mut base = PresetBase { enabled: true };
        let ok = preset_base_read_json(&mut base, None);
        assert!(ok);
        assert!(!base.enabled);
    }

    /// Present `true` over a `false` receiver.
    #[test]
    fn preset_base_read_json_present_true_overwrites_false() {
        let mut base = PresetBase { enabled: false };
        assert!(preset_base_read_json(&mut base, Some(true)));
        assert!(base.enabled);
    }

    /// Present `false` over a `true` receiver.
    #[test]
    fn preset_base_read_json_present_false_overwrites_true() {
        let mut base = PresetBase { enabled: true };
        assert!(preset_base_read_json(&mut base, Some(false)));
        assert!(!base.enabled);
    }

    /// **The finding.** `PresetBase::readJson` returns `true` on every input
    /// -- there is no failure path in `rt64_preset.cpp`.
    #[test]
    fn preset_base_read_json_never_fails() {
        for value in [None, Some(true), Some(false)] {
            let mut base = PresetBase::default_construct();
            assert!(preset_base_read_json(&mut base, value));
        }
        assert!(PRESET_BASE_READ_JSON_ALWAYS_SUCCEEDS);
    }

    // -- PresetBase::writeJson ----------------------------------------------

    /// One key, named `"enabled"`, carrying the field verbatim.
    #[test]
    fn preset_base_write_json_emits_enabled_key() {
        let ((key, value), ok) = preset_base_write_json(&PresetBase { enabled: true });
        assert_eq!(key, "enabled");
        assert!(value);
        assert!(ok);
    }

    /// The written value tracks the field, it is not a constant.
    #[test]
    fn preset_base_write_json_carries_false_through() {
        let ((key, value), ok) = preset_base_write_json(&PresetBase { enabled: false });
        assert_eq!(key, "enabled");
        assert!(!value);
        assert!(ok);
    }

    /// `writeJson` likewise has no failure path.
    #[test]
    fn preset_base_write_json_never_fails() {
        assert!(PRESET_BASE_WRITE_JSON_ALWAYS_SUCCEEDS);
        for enabled in [true, false] {
            let (_pair, ok) = preset_base_write_json(&PresetBase { enabled });
            assert!(ok);
        }
    }

    /// Write-then-read is NOT a round trip when the key is dropped: writing
    /// `true` then reading with the key absent yields `false`.
    #[test]
    fn preset_base_write_then_absent_read_is_not_a_round_trip() {
        let written = PresetBase { enabled: true };
        let ((_key, value), _ok) = preset_base_write_json(&written);
        assert!(value);

        let mut reloaded = PresetBase::default_construct();
        preset_base_read_json(&mut reloaded, None);
        assert_ne!(reloaded, written);
    }

    /// Write-then-read IS a round trip when the key is carried across.
    #[test]
    fn preset_base_round_trips_when_key_is_present() {
        for enabled in [true, false] {
            let written = PresetBase { enabled };
            let ((_key, value), _ok) = preset_base_write_json(&written);
            let mut reloaded = PresetBase { enabled: !enabled };
            preset_base_read_json(&mut reloaded, Some(value));
            assert_eq!(reloaded, written);
        }
    }

    // -- PresetScene constructor --------------------------------------------

    /// `.cpp` lines 13-14, 19-26: the ten scalar constructor values.
    #[test]
    fn scene_constructor_scalars() {
        let s = PresetScene::default_construct();
        assert!(s.estimate_ambient_light);
        assert_eq!(s.ambient_light_intensity, 0.03f32);
        assert_eq!(s.gi_diffuse_strength, 1.5f32);
        assert_eq!(s.gi_background_strength, 0.5f32);
        assert_eq!(s.tonemap_exposure, 0.35f32);
        assert_eq!(s.tonemap_white, 1.05f32);
        assert_eq!(s.tonemap_black, 0.0f32);
        assert_eq!(s.min_luminance, 0.3f32);
        assert_eq!(s.luminance_range, 0.0f32);
        assert_eq!(s.luma_update_time, 1.1f32);
    }

    /// `.cpp` lines 15-18: the four constructor colors.
    #[test]
    fn scene_constructor_colors() {
        let s = PresetScene::default_construct();
        assert_eq!(s.ambient_base_color, Vec3::new(0.03f32, 0.03f32, 0.03f32));
        assert_eq!(s.ambient_no_gi_color, Vec3::new(0.03f32, 0.03f32, 0.03f32));
        assert_eq!(
            s.eye_light_diffuse_color,
            Vec3::new(0.008f32, 0.008f32, 0.008f32)
        );
        assert_eq!(
            s.eye_light_specular_color,
            Vec3::new(0.004f32, 0.004f32, 0.004f32)
        );
    }

    /// `ambientBaseColor` and `ambientNoGIColor` get the same literal, but
    /// diffuse and specular eye-light colors differ from each other (2:1).
    #[test]
    fn scene_constructor_eye_light_diffuse_is_twice_specular() {
        let s = PresetScene::default_construct();
        assert_eq!(s.ambient_base_color, s.ambient_no_gi_color);
        assert_ne!(s.eye_light_diffuse_color, s.eye_light_specular_color);
        assert_eq!(s.eye_light_diffuse_color.x, 0.008f32);
        assert_eq!(s.eye_light_specular_color.x, 0.004f32);
    }

    /// The base sub-object is default-constructed: `enabled == true`. The
    /// `PresetScene` constructor body never assigns it.
    #[test]
    fn scene_constructor_leaves_base_enabled_true() {
        assert!(PresetScene::default_construct().base.enabled);
    }

    /// `Default` agrees with the named constructor.
    #[test]
    fn scene_default_trait_matches_default_construct() {
        assert_eq!(PresetScene::default(), PresetScene::default_construct());
    }

    // -- readJson: the empty-object defaults --------------------------------

    /// The four fields whose JSON fallback AGREES with the constructor.
    #[test]
    fn read_json_defaults_that_agree_with_constructor() {
        let ctor = PresetScene::default_construct();
        let mut loaded = PresetScene::default_construct();
        scene_read_json(&mut loaded, &SceneReadJsonInput::all_absent());

        assert_eq!(loaded.estimate_ambient_light, ctor.estimate_ambient_light);
        assert_eq!(loaded.ambient_light_intensity, ctor.ambient_light_intensity);
        assert_eq!(loaded.tonemap_black, ctor.tonemap_black);
        assert_eq!(loaded.luminance_range, ctor.luminance_range);
    }

    /// The four colors default to the zero vector from JSON.
    #[test]
    fn read_json_color_defaults_are_zero() {
        let mut s = non_default_receiver();
        scene_read_json(&mut s, &SceneReadJsonInput::all_absent());

        let zero = Vec3::new(0.0f32, 0.0f32, 0.0f32);
        assert_eq!(s.ambient_base_color, zero);
        assert_eq!(s.ambient_no_gi_color, zero);
        assert_eq!(s.eye_light_diffuse_color, zero);
        assert_eq!(s.eye_light_specular_color, zero);
    }

    /// The six scalar floats that default to `0.0f` from JSON.
    #[test]
    fn read_json_scalar_defaults_are_zero() {
        let mut s = non_default_receiver();
        scene_read_json(&mut s, &SceneReadJsonInput::all_absent());

        assert_eq!(s.gi_diffuse_strength, 0.0f32);
        assert_eq!(s.gi_background_strength, 0.0f32);
        assert_eq!(s.tonemap_exposure, 0.0f32);
        assert_eq!(s.tonemap_white, 0.0f32);
        assert_eq!(s.min_luminance, 0.0f32);
        assert_eq!(s.luma_update_time, 0.0f32);
    }

    /// The two non-zero scalar JSON fallbacks: `estimateAmbientLight` is
    /// `true` and `ambientLightIntensity` is `0.03f`, NOT `0.0f`.
    #[test]
    fn read_json_nonzero_scalar_fallbacks() {
        let mut s = non_default_receiver();
        assert!(!s.estimate_ambient_light);
        scene_read_json(&mut s, &SceneReadJsonInput::all_absent());

        assert!(s.estimate_ambient_light);
        assert_eq!(s.ambient_light_intensity, 0.03f32);
    }

    /// **The oddity, field by field.** Eleven of fifteen fields differ
    /// between a default-constructed scene and a `{}`-loaded one.
    #[test]
    fn default_construct_and_empty_json_differ_in_eleven_fields() {
        let ctor = PresetScene::default_construct();
        let mut loaded = PresetScene::default_construct();
        scene_read_json(&mut loaded, &SceneReadJsonInput::all_absent());

        assert_ne!(ctor, loaded);

        // The eleven that differ.
        assert_ne!(ctor.base.enabled, loaded.base.enabled);
        assert_ne!(ctor.ambient_base_color, loaded.ambient_base_color);
        assert_ne!(ctor.ambient_no_gi_color, loaded.ambient_no_gi_color);
        assert_ne!(ctor.eye_light_diffuse_color, loaded.eye_light_diffuse_color);
        assert_ne!(
            ctor.eye_light_specular_color,
            loaded.eye_light_specular_color
        );
        assert_ne!(ctor.gi_diffuse_strength, loaded.gi_diffuse_strength);
        assert_ne!(ctor.gi_background_strength, loaded.gi_background_strength);
        assert_ne!(ctor.tonemap_exposure, loaded.tonemap_exposure);
        assert_ne!(ctor.tonemap_white, loaded.tonemap_white);
        assert_ne!(ctor.min_luminance, loaded.min_luminance);
        assert_ne!(ctor.luma_update_time, loaded.luma_update_time);

        // The four that agree.
        assert_eq!(ctor.estimate_ambient_light, loaded.estimate_ambient_light);
        assert_eq!(ctor.ambient_light_intensity, loaded.ambient_light_intensity);
        assert_eq!(ctor.tonemap_black, loaded.tonemap_black);
        assert_eq!(ctor.luminance_range, loaded.luminance_range);
    }

    /// The exact eleven-vs-four split, counted.
    #[test]
    fn empty_json_load_changes_exactly_eleven_of_fifteen_fields() {
        let c = PresetScene::default_construct();
        let mut l = PresetScene::default_construct();
        scene_read_json(&mut l, &SceneReadJsonInput::all_absent());

        let differs = [
            c.base.enabled != l.base.enabled,
            c.estimate_ambient_light != l.estimate_ambient_light,
            c.ambient_light_intensity != l.ambient_light_intensity,
            c.ambient_base_color != l.ambient_base_color,
            c.ambient_no_gi_color != l.ambient_no_gi_color,
            c.eye_light_diffuse_color != l.eye_light_diffuse_color,
            c.eye_light_specular_color != l.eye_light_specular_color,
            c.gi_diffuse_strength != l.gi_diffuse_strength,
            c.gi_background_strength != l.gi_background_strength,
            c.tonemap_exposure != l.tonemap_exposure,
            c.tonemap_white != l.tonemap_white,
            c.tonemap_black != l.tonemap_black,
            c.min_luminance != l.min_luminance,
            c.luminance_range != l.luminance_range,
            c.luma_update_time != l.luma_update_time,
        ];

        assert_eq!(differs.len(), 15);
        assert_eq!(differs.iter().filter(|d| **d).count(), 11);
        assert_eq!(differs.iter().filter(|d| !**d).count(), 4);
    }

    // -- readJson: unconditional overwrite ----------------------------------

    /// **Non-vacuous overwrite pin.** Every absent key writes its default
    /// over a receiver that shares no value with either default table.
    #[test]
    fn read_json_all_absent_overwrites_non_default_receiver() {
        let mut s = non_default_receiver();
        let before = s;
        assert!(scene_read_json(&mut s, &SceneReadJsonInput::all_absent()));

        // Nothing carried through.
        assert_ne!(s, before);
        assert!(!s.base.enabled);
        assert!(s.estimate_ambient_light);
        assert_eq!(s.ambient_light_intensity, 0.03f32);
        assert_eq!(s.ambient_base_color, Vec3::new(0.0, 0.0, 0.0));
        assert_eq!(s.ambient_no_gi_color, Vec3::new(0.0, 0.0, 0.0));
        assert_eq!(s.eye_light_diffuse_color, Vec3::new(0.0, 0.0, 0.0));
        assert_eq!(s.eye_light_specular_color, Vec3::new(0.0, 0.0, 0.0));
        assert_eq!(s.gi_diffuse_strength, 0.0f32);
        assert_eq!(s.gi_background_strength, 0.0f32);
        assert_eq!(s.tonemap_exposure, 0.0f32);
        assert_eq!(s.tonemap_white, 0.0f32);
        assert_eq!(s.tonemap_black, 0.0f32);
        assert_eq!(s.min_luminance, 0.0f32);
        assert_eq!(s.luminance_range, 0.0f32);
        assert_eq!(s.luma_update_time, 0.0f32);
    }

    /// The all-absent result equals the all-absent result from ANY receiver:
    /// every field is written, so the prior value cannot leak.
    #[test]
    fn read_json_all_absent_result_is_receiver_independent() {
        let mut from_default = PresetScene::default_construct();
        let mut from_other = non_default_receiver();
        scene_read_json(&mut from_default, &SceneReadJsonInput::all_absent());
        scene_read_json(&mut from_other, &SceneReadJsonInput::all_absent());
        assert_eq!(from_default, from_other);
    }

    /// **No field lacks a key.** All fifteen are read and all fifteen are
    /// written; nothing carries the destination's prior value through.
    #[test]
    fn every_declared_field_has_a_read_and_a_write_key() {
        assert_eq!(READ_JSON_KEYS.len(), 15);
        let write = write_json_keys(&PresetScene::default_construct());
        assert_eq!(write.len(), 15);
        assert_eq!(write.as_slice(), READ_JSON_KEYS.as_slice());
    }

    // -- readJson: present values --------------------------------------------

    /// Every present value lands on its own field, and only its own field.
    #[test]
    fn read_json_all_present_lands_on_the_right_fields() {
        let mut s = non_default_receiver();
        let input = SceneReadJsonInput {
            preset_base_enabled: Some(true),
            estimate_ambient_light: Some(false),
            ambient_light_intensity: Some(1.25f32),
            ambient_base_color: Some(Vec3::new(1.0, 2.0, 3.0)),
            ambient_no_gi_color: Some(Vec3::new(4.0, 5.0, 6.0)),
            eye_light_diffuse_color: Some(Vec3::new(7.0, 8.0, 9.0)),
            eye_light_specular_color: Some(Vec3::new(10.0, 11.0, 12.0)),
            gi_diffuse_strength: Some(13.0f32),
            gi_background_strength: Some(14.0f32),
            tonemap_exposure: Some(15.0f32),
            tonemap_white: Some(16.0f32),
            tonemap_black: Some(17.0f32),
            min_luminance: Some(18.0f32),
            luminance_range: Some(19.0f32),
            luma_update_time: Some(20.0f32),
        };

        assert!(scene_read_json(&mut s, &input));

        assert!(s.base.enabled);
        assert!(!s.estimate_ambient_light);
        assert_eq!(s.ambient_light_intensity, 1.25f32);
        assert_eq!(s.ambient_base_color, Vec3::new(1.0, 2.0, 3.0));
        assert_eq!(s.ambient_no_gi_color, Vec3::new(4.0, 5.0, 6.0));
        assert_eq!(s.eye_light_diffuse_color, Vec3::new(7.0, 8.0, 9.0));
        assert_eq!(s.eye_light_specular_color, Vec3::new(10.0, 11.0, 12.0));
        assert_eq!(s.gi_diffuse_strength, 13.0f32);
        assert_eq!(s.gi_background_strength, 14.0f32);
        assert_eq!(s.tonemap_exposure, 15.0f32);
        assert_eq!(s.tonemap_white, 16.0f32);
        assert_eq!(s.tonemap_black, 17.0f32);
        assert_eq!(s.min_luminance, 18.0f32);
        assert_eq!(s.luminance_range, 19.0f32);
        assert_eq!(s.luma_update_time, 20.0f32);
    }

    /// One present key among fourteen absent ones: the present key keeps its
    /// value and the other thirteen still take their defaults.
    #[test]
    fn read_json_one_present_key_does_not_protect_the_others() {
        let mut s = non_default_receiver();
        let input = SceneReadJsonInput {
            tonemap_white: Some(9.5f32),
            ..SceneReadJsonInput::all_absent()
        };
        scene_read_json(&mut s, &input);

        assert_eq!(s.tonemap_white, 9.5f32);
        assert_eq!(s.tonemap_exposure, 0.0f32);
        assert_eq!(s.tonemap_black, 0.0f32);
        assert_eq!(s.min_luminance, 0.0f32);
    }

    /// A present value that happens to EQUAL the fallback is indistinguishable
    /// from absence -- there is no presence flag on the field.
    #[test]
    fn read_json_present_value_equal_to_fallback_matches_absence() {
        let mut present = non_default_receiver();
        let mut absent = non_default_receiver();
        scene_read_json(
            &mut present,
            &SceneReadJsonInput {
                tonemap_exposure: Some(0.0f32),
                ..SceneReadJsonInput::all_absent()
            },
        );
        scene_read_json(&mut absent, &SceneReadJsonInput::all_absent());
        assert_eq!(present, absent);
    }

    /// A present value overriding a NON-zero fallback: `estimateAmbientLight`
    /// present as `false` beats its `true` default.
    #[test]
    fn read_json_present_false_beats_true_fallback() {
        let mut s = PresetScene::default_construct();
        scene_read_json(
            &mut s,
            &SceneReadJsonInput {
                estimate_ambient_light: Some(false),
                ..SceneReadJsonInput::all_absent()
            },
        );
        assert!(!s.estimate_ambient_light);
    }

    /// `readJson` returns `true` for both the all-absent and all-present
    /// inputs -- no input can make it fail, since the only guard is the
    /// unconditionally-succeeding base call.
    #[test]
    fn scene_read_json_never_fails() {
        let mut s = PresetScene::default_construct();
        assert!(scene_read_json(&mut s, &SceneReadJsonInput::all_absent()));
        assert!(scene_read_json(
            &mut s,
            &SceneReadJsonInput {
                preset_base_enabled: Some(false),
                ..SceneReadJsonInput::all_absent()
            }
        ));
        assert!(scene_read_json(
            &mut s,
            &SceneReadJsonInput {
                preset_base_enabled: Some(true),
                ..SceneReadJsonInput::all_absent()
            }
        ));
    }

    /// The base's `enabled` write happens before the guard's return value is
    /// inspected, so `readJson` mutates `enabled` even on the (unreachable)
    /// failure path. Pinned via the base call directly.
    #[test]
    fn base_enabled_is_written_before_the_guard_is_evaluated() {
        let mut base = PresetBase { enabled: true };
        let ok = preset_base_read_json(&mut base, Some(false));
        assert!(!base.enabled, "the write lands first");
        assert!(ok, "and only then is the return inspected");
    }

    /// `SceneReadJsonInput::default()` is the all-absent input.
    #[test]
    fn scene_read_json_input_default_is_all_absent() {
        assert_eq!(
            SceneReadJsonInput::default(),
            SceneReadJsonInput::all_absent()
        );
    }

    // -- float semantics -----------------------------------------------------

    /// A `NaN` present value is stored verbatim -- there is no `std::min`,
    /// `std::max`, or clamp anywhere in the ported surface to reshape it.
    #[test]
    fn read_json_stores_nan_verbatim() {
        let mut s = PresetScene::default_construct();
        scene_read_json(
            &mut s,
            &SceneReadJsonInput {
                tonemap_exposure: Some(f32::NAN),
                min_luminance: Some(f32::NAN),
                ..SceneReadJsonInput::all_absent()
            },
        );
        assert!(s.tonemap_exposure.is_nan());
        assert!(s.min_luminance.is_nan());
    }

    /// A negative-zero present value keeps its sign bit: no arithmetic runs
    /// on it, so `-0.0` does not become `0.0`.
    #[test]
    fn read_json_preserves_negative_zero_sign_bit() {
        let mut s = PresetScene::default_construct();
        scene_read_json(
            &mut s,
            &SceneReadJsonInput {
                tonemap_black: Some(-0.0f32),
                ..SceneReadJsonInput::all_absent()
            },
        );
        assert_eq!(s.tonemap_black.to_bits(), (-0.0f32).to_bits());
        assert_ne!(s.tonemap_black.to_bits(), 0.0f32.to_bits());
        // Still numerically equal to the `0.0f` fallback.
        assert_eq!(s.tonemap_black, 0.0f32);
    }

    /// The `0.0f` fallback itself is POSITIVE zero.
    #[test]
    fn read_json_zero_fallback_is_positive_zero() {
        let mut s = PresetScene::default_construct();
        scene_read_json(&mut s, &SceneReadJsonInput::all_absent());
        assert_eq!(s.tonemap_black.to_bits(), 0.0f32.to_bits());
        assert_eq!(s.luminance_range.to_bits(), 0.0f32.to_bits());
    }

    /// Infinities pass through unmodified in both directions.
    #[test]
    fn read_json_stores_infinities_verbatim() {
        let mut s = PresetScene::default_construct();
        scene_read_json(
            &mut s,
            &SceneReadJsonInput {
                gi_diffuse_strength: Some(f32::INFINITY),
                gi_background_strength: Some(f32::NEG_INFINITY),
                ..SceneReadJsonInput::all_absent()
            },
        );
        assert_eq!(s.gi_diffuse_strength, f32::INFINITY);
        assert_eq!(s.gi_background_strength, f32::NEG_INFINITY);
    }

    /// The ImGui `DragFloat` ranges are NOT applied as clamps: a value far
    /// outside `0.0..=20.0` survives `readJson` untouched.
    #[test]
    fn read_json_does_not_apply_imgui_widget_ranges_as_clamps() {
        let mut s = PresetScene::default_construct();
        scene_read_json(
            &mut s,
            &SceneReadJsonInput {
                // `DragFloat("Exposure", ..., 0.0f, 20.0f)`.
                tonemap_exposure: Some(1000.0f32),
                // `DragFloat("Eye Adaption Minimum", ..., -20.0f, 20.0f)`.
                min_luminance: Some(-1000.0f32),
                // `DragFloat("Eye Adaption Update Time", ..., 0.0f, 4.0f)`.
                luma_update_time: Some(-5.0f32),
                ..SceneReadJsonInput::all_absent()
            },
        );
        assert_eq!(s.tonemap_exposure, 1000.0f32);
        assert_eq!(s.min_luminance, -1000.0f32);
        assert_eq!(s.luma_update_time, -5.0f32);
    }

    /// Colors are likewise unclamped despite `DragFloat3(..., 0.0f, 100.0f)`.
    #[test]
    fn read_json_does_not_clamp_colors_to_the_widget_range() {
        let mut s = PresetScene::default_construct();
        let wild = Vec3::new(-50.0f32, 500.0f32, f32::NAN);
        scene_read_json(
            &mut s,
            &SceneReadJsonInput {
                ambient_base_color: Some(wild),
                ..SceneReadJsonInput::all_absent()
            },
        );
        assert_eq!(s.ambient_base_color.x, -50.0f32);
        assert_eq!(s.ambient_base_color.y, 500.0f32);
        assert!(s.ambient_base_color.z.is_nan());
    }

    // -- writeJson -----------------------------------------------------------

    /// The full emission order, in the source's call order, `"enabled"` first.
    #[test]
    fn write_json_key_order_is_base_first_then_declaration_order() {
        let keys = write_json_keys(&PresetScene::default_construct());
        assert_eq!(
            keys,
            vec![
                "enabled",
                "estimateAmbientLight",
                "ambientLightIntensity",
                "ambientBaseColor",
                "ambientNoGIColor",
                "eyeLightDiffuseColor",
                "eyeLightSpecularColor",
                "giDiffuseStrength",
                "giBackgroundStrength",
                "tonemapExposure",
                "tonemapWhite",
                "tonemapBlack",
                "minLuminance",
                "luminanceRange",
                "lumaUpdateTime",
            ]
        );
    }

    /// `"enabled"` is emitted by the base call, hence strictly first -- ahead
    /// of `"estimateAmbientLight"` even though `"a..." < "e..."` in the sort
    /// order nlohmann would apply on serialization.
    #[test]
    fn write_json_emits_enabled_before_any_scene_key() {
        let keys = write_json_keys(&PresetScene::default_construct());
        assert_eq!(keys[0], "enabled");
        let ambient = keys.iter().position(|k| *k == "ambientBaseColor").unwrap();
        assert!(ambient > 0);
    }

    /// Emitted key order is NOT alphabetical: this pins the call order rather
    /// than nlohmann's serialization order.
    #[test]
    fn write_json_key_order_is_not_alphabetical() {
        let keys = write_json_keys(&PresetScene::default_construct());
        let mut sorted = keys.clone();
        sorted.sort_unstable();
        assert_ne!(keys, sorted);
    }

    /// No key is emitted twice.
    #[test]
    fn write_json_keys_are_unique() {
        let keys = write_json_keys(&PresetScene::default_construct());
        let mut sorted = keys.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), keys.len());
    }

    /// The value shape per key: two bools, four float3s, nine floats.
    #[test]
    fn write_json_value_shapes() {
        let (pairs, ok) = scene_write_json(&PresetScene::default_construct());
        assert!(ok);

        let bools = pairs
            .iter()
            .filter(|(_k, v)| matches!(v, SceneJsonValue::Bool(_)))
            .count();
        let float3s = pairs
            .iter()
            .filter(|(_k, v)| matches!(v, SceneJsonValue::Float3(_)))
            .count();
        let floats = pairs
            .iter()
            .filter(|(_k, v)| matches!(v, SceneJsonValue::Float(_)))
            .count();

        assert_eq!(bools, 2);
        assert_eq!(float3s, 4);
        assert_eq!(floats, 9);
        assert_eq!(bools + float3s + floats, 15);
    }

    /// The written values are the constructor's, verbatim.
    #[test]
    fn write_json_carries_constructor_values() {
        let (pairs, _ok) = scene_write_json(&PresetScene::default_construct());
        assert_eq!(pairs[0], ("enabled", SceneJsonValue::Bool(true)));
        assert_eq!(
            pairs[1],
            ("estimateAmbientLight", SceneJsonValue::Bool(true))
        );
        assert_eq!(
            pairs[2],
            ("ambientLightIntensity", SceneJsonValue::Float(0.03f32))
        );
        assert_eq!(
            pairs[3],
            (
                "ambientBaseColor",
                SceneJsonValue::Float3(Vec3::new(0.03, 0.03, 0.03))
            )
        );
        assert_eq!(
            pairs[7],
            ("giDiffuseStrength", SceneJsonValue::Float(1.5f32))
        );
        assert_eq!(pairs[10], ("tonemapWhite", SceneJsonValue::Float(1.05f32)));
        assert_eq!(pairs[14], ("lumaUpdateTime", SceneJsonValue::Float(1.1f32)));
    }

    /// The written values track the object, not the constructor.
    #[test]
    fn write_json_carries_mutated_values() {
        let mut s = PresetScene::default_construct();
        s.base.enabled = false;
        s.tonemap_white = -3.5f32;
        s.ambient_no_gi_color = Vec3::new(9.0, 8.0, 7.0);

        let (pairs, ok) = scene_write_json(&s);
        assert!(ok);
        assert_eq!(pairs[0], ("enabled", SceneJsonValue::Bool(false)));
        assert_eq!(pairs[10], ("tonemapWhite", SceneJsonValue::Float(-3.5f32)));
        assert_eq!(
            pairs[4],
            (
                "ambientNoGIColor",
                SceneJsonValue::Float3(Vec3::new(9.0, 8.0, 7.0))
            )
        );
    }

    /// `writeJson` returns `true` for every object, since its only guard is
    /// the unconditionally-succeeding base call.
    #[test]
    fn scene_write_json_never_fails() {
        for scene in [PresetScene::default_construct(), non_default_receiver()] {
            let (pairs, ok) = scene_write_json(&scene);
            assert!(ok);
            assert_eq!(pairs.len(), 15);
        }
    }

    // -- read/write key agreement --------------------------------------------

    /// Read keys and write keys are the same fifteen names in the same order:
    /// nothing is written that cannot be read back, and vice versa.
    #[test]
    fn read_and_write_key_sets_match_exactly() {
        let write = write_json_keys(&PresetScene::default_construct());
        for (index, read_key) in READ_JSON_KEYS.iter().enumerate() {
            assert_eq!(*read_key, write[index], "key {index}");
        }
    }

    /// A full write-then-read with every key carried across is an exact round
    /// trip, from a non-default object -- so the defaults cannot be masking a
    /// dropped field.
    #[test]
    fn full_round_trip_from_non_default_object_is_exact() {
        let original = non_default_receiver();
        let (pairs, ok) = scene_write_json(&original);
        assert!(ok);

        let mut input = SceneReadJsonInput::all_absent();
        for (key, value) in pairs {
            match (key, value) {
                ("enabled", SceneJsonValue::Bool(v)) => input.preset_base_enabled = Some(v),
                ("estimateAmbientLight", SceneJsonValue::Bool(v)) => {
                    input.estimate_ambient_light = Some(v)
                }
                ("ambientLightIntensity", SceneJsonValue::Float(v)) => {
                    input.ambient_light_intensity = Some(v)
                }
                ("ambientBaseColor", SceneJsonValue::Float3(v)) => {
                    input.ambient_base_color = Some(v)
                }
                ("ambientNoGIColor", SceneJsonValue::Float3(v)) => {
                    input.ambient_no_gi_color = Some(v)
                }
                ("eyeLightDiffuseColor", SceneJsonValue::Float3(v)) => {
                    input.eye_light_diffuse_color = Some(v)
                }
                ("eyeLightSpecularColor", SceneJsonValue::Float3(v)) => {
                    input.eye_light_specular_color = Some(v)
                }
                ("giDiffuseStrength", SceneJsonValue::Float(v)) => {
                    input.gi_diffuse_strength = Some(v)
                }
                ("giBackgroundStrength", SceneJsonValue::Float(v)) => {
                    input.gi_background_strength = Some(v)
                }
                ("tonemapExposure", SceneJsonValue::Float(v)) => input.tonemap_exposure = Some(v),
                ("tonemapWhite", SceneJsonValue::Float(v)) => input.tonemap_white = Some(v),
                ("tonemapBlack", SceneJsonValue::Float(v)) => input.tonemap_black = Some(v),
                ("minLuminance", SceneJsonValue::Float(v)) => input.min_luminance = Some(v),
                ("luminanceRange", SceneJsonValue::Float(v)) => input.luminance_range = Some(v),
                ("lumaUpdateTime", SceneJsonValue::Float(v)) => input.luma_update_time = Some(v),
                other => panic!("unexpected key/shape {other:?}"),
            }
        }

        let mut reloaded = PresetScene::default_construct();
        assert!(scene_read_json(&mut reloaded, &input));
        assert_eq!(reloaded, original);
    }

    /// A write-then-read that DROPS the file entirely (`{}`) does not round
    /// trip -- and lands on the eleven-field disagreement above.
    #[test]
    fn round_trip_through_an_empty_object_loses_eleven_fields() {
        let original = PresetScene::default_construct();
        let mut reloaded = PresetScene::default_construct();
        scene_read_json(&mut reloaded, &SceneReadJsonInput::all_absent());
        assert_ne!(reloaded, original);
    }

    // -- key spellings --------------------------------------------------------

    /// The exact upstream spellings, including the `NoGI` capitalization,
    /// which is neither `NoGi` nor `Nogi`.
    #[test]
    fn key_spellings_are_verbatim() {
        assert!(READ_JSON_KEYS.contains(&"ambientNoGIColor"));
        assert!(!READ_JSON_KEYS.contains(&"ambientNoGiColor"));
        assert!(READ_JSON_KEYS.contains(&"lumaUpdateTime"));
        assert!(!READ_JSON_KEYS.contains(&"luminanceUpdateTime"));
        assert!(READ_JSON_KEYS.contains(&"minLuminance"));
        assert!(READ_JSON_KEYS.contains(&"luminanceRange"));
    }

    /// The three tonemap keys share a prefix; the two eye-light colors share
    /// theirs. No key is a prefix duplicate of another.
    #[test]
    fn key_prefix_families_are_distinct() {
        let tonemap: Vec<_> = READ_JSON_KEYS
            .iter()
            .filter(|k| k.starts_with("tonemap"))
            .collect();
        assert_eq!(tonemap.len(), 3);

        let eye: Vec<_> = READ_JSON_KEYS
            .iter()
            .filter(|k| k.starts_with("eyeLight"))
            .collect();
        assert_eq!(eye.len(), 2);

        let ambient: Vec<_> = READ_JSON_KEYS
            .iter()
            .filter(|k| k.starts_with("ambient"))
            .collect();
        assert_eq!(ambient.len(), 3);

        let gi: Vec<_> = READ_JSON_KEYS
            .iter()
            .filter(|k| k.starts_with("gi"))
            .collect();
        assert_eq!(gi.len(), 2);
    }

    /// `enabled` is the base's key and appears exactly once in the read list.
    #[test]
    fn enabled_key_appears_exactly_once() {
        assert_eq!(ENABLED_KEY, "enabled");
        assert_eq!(
            READ_JSON_KEYS.iter().filter(|k| **k == "enabled").count(),
            1
        );
    }
}
