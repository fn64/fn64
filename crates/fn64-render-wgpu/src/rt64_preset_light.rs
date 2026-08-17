//! `PresetLights` light-name generation, the `from_json` / "New light"
//! default descriptors, the spot-cosine clamp, the diffuse-color
//! normalization, and the gizmo/cone predicates: a literal port of the
//! permitted MIT RT64 source pinned at commit
//! `5473732a822a4423b5696e7cb18fecc425a59875`
//! (`docs/RT64-PORT-AUTHORITY.md`), `src/preset/rt64_preset_light.h` (SHA-256
//! of the whole file,
//! `0324f859f2c0e007600a10af37ffb798afb8208076b858ca67fc4171a7e45e52`, 39
//! lines) + `src/preset/rt64_preset_light.cpp` (SHA-256 of the whole file,
//! `201df84b46f97ae705a1871e0ba0d136aa2f93ff19280c2077f3d14a70659958`, 397
//! lines). Both digests were computed independently here with `shasum -a 256`
//! against the pinned checkout and cross-checked verbatim against
//! `docs/rt64-port-inventory.json`'s
//! `files[path="src/preset/rt64_preset_light.{h,cpp}"].sources.port.sha256`,
//! which records the identical two digests. (The inventory records
//! `lines: 398` / `lines: 40` for the `.cpp` / `.h`; `wc -l` reports 397 / 39
//! because neither file ends in a trailing newline, so the inventory is
//! counting lines-as-records rather than newline characters. Noted, not
//! treated as a mismatch: the SHA-256 digests -- the actual identity check --
//! agree exactly.)
//!
//! ## The ported / refused boundary and the criterion
//!
//! **Criterion**: a construct is ported when its result is a pure function of
//! plain scalar/string values -- computable, and checkable, with no ImGui
//! context, no Im3d context, no `nlohmann::json` document, and no RHI/window
//! handle. Everything whose *result* is a call into one of those four
//! subsystems is refused, even when the arithmetic feeding the call is
//! simple, because there is no observable value to pin. Where a refused
//! ImGui/Im3d call is fed by a pure expression, the pure expression alone is
//! ported and the call is not.
//!
//! **Ported** (each with a named function below):
//! - `generateLightName` (`.cpp` lines 175-222), the lambda inside
//!   `inspectLibrary`. This is the only nontrivial arithmetic in the file: a
//!   hand-rolled decimal string increment with carry, plus a
//!   zero-padded-suffix fallback. Its only environmental dependency is
//!   `lightMap.find(...) != lightMap.end()`, which is a pure
//!   name-already-taken predicate; it is ported as a `&BTreeSet<String>`
//!   lookup. Ported as [`generate_light_name`].
//! - `interop::from_json(const json &, PointLight &)`'s **default values**
//!   (`.cpp` lines 33-46). The `j.value(...)` machinery is `nlohmann::json`
//!   and is refused; the twelve fallback constants it supplies when a key is
//!   absent are plain data and are behavior. Ported as
//!   [`point_light_json_defaults`].
//! - The `New##light` button's descriptor initialization (`.cpp` lines
//!   224-242), minus the ImGui button test itself. Every assignment but
//!   `desc.position` is a constant; `desc.position` is
//!   `viewPos + viewDir * -150.0f`, pure arithmetic over two caller-supplied
//!   vectors. Ported as [`new_light_descriptor`].
//! - `lightDesc.spotFalloffCosine = std::min(spotFalloffCosine,
//!   spotMaxCosine)` (`.cpp` line 338), the post-widget clamp. Ported as
//!   [`clamp_spot_falloff_cosine`].
//! - The diffuse-color normalization repeated at `.cpp` lines 133-135 and
//!   381-385: `col / hlslpp::length(col)`, unguarded against a zero-length
//!   color. Ported as [`normalize_light_color`].
//! - `lightSelected` (`.cpp` line 247), an `&&` of a preset-name equality and
//!   a map-membership test. Ported as [`light_selected`].
//! - The gizmo-suppression one-frame toggle (`.cpp` lines 341-343) and the
//!   spot-cone draw predicates (`.cpp` lines 348, 369, 374). Ported as
//!   [`take_skip_gizmos`], [`should_draw_spot_cone`],
//!   [`should_draw_max_cone`], [`should_draw_falloff_cone`].
//! - `drawConeCircles`' geometry (`.cpp` lines 357-367): the spot radius
//!   `hlslpp::length(lightDir) * tanf(acosf(-cosine))` and the 20 per-segment
//!   `(center, radius)` pairs it feeds to `Im3d::DrawCircle`. The `DrawCircle`
//!   calls are refused; the pairs are values. Ported as
//!   [`spot_cone_radius`] and [`spot_cone_circles`].
//!
//! **Refused**, with the reason:
//! - `interop::to_json` / `from_json` bodies, `PresetLights::readJson`,
//!   `PresetLights::writeJson` (`.cpp` lines 16-91 apart from the defaults
//!   above). `nlohmann::json` document traversal, `jsonObj.find`,
//!   `json::push_back`, and the unchecked `jlight["name"]`/`["description"]`/
//!   `["enabled"]` subscripts. No JSON dependency is admitted for a
//!   mechanical port card, and the exception-on-missing-key behavior is a
//!   property of nlohmann, not of RT64.
//! - `PresetBase::readJson` / `PresetBase::writeJson`, called first in each
//!   of the two above. Not in the cited files at all.
//! - The entirety of `PresetLightsLibraryInspector::inspectLibrary` outside
//!   `generateLightName` and the New-button descriptor (`.cpp` lines 95-278):
//!   `ImGui::BeginChild`, `PushID`, `CollapsingHeader`, `Checkbox`,
//!   `Selectable`, `Button`, `BeginDisabled`, `SameLine`, `EndChild`,
//!   `IsMouseClicked`, `IsMouseDoubleClicked`; `Im3d::GetAppData`,
//!   `PushEnableSorting`, `SetColor`, `SetAlpha`, `DrawSphereFilled`,
//!   `GetContext().pixelsToWorldSize`, `Intersects`; and `inspectPresetBegin`
//!   / `inspectPresetEnd` / `inspectBottom`, which live in
//!   `rt64_preset_inspector.h` (not a cited source here). The map mutations
//!   these guard (`lightMap[name] = ...`, `lightMap.erase`) are ordinary
//!   `std::map` operations with no RT64-specific semantics.
//! - The entirety of `PresetLightsLibraryInspector::inspectSelection` outside
//!   the clamp, the predicates, the cone geometry and the normalization
//!   (`.cpp` lines 280-397): `ImGui::InputText`, eleven `DragFloat`/
//!   `DragFloat3` calls, `InputInt`, `PushID`/`PopID`; `Im3d::GizmoTranslation`,
//!   `DrawLine`, `DrawSphere`, `DrawSphereFilled`, `SetColor`, `SetAlpha`.
//!   The `DragFloat` min/max arguments (e.g. `0.0f, FLT_MAX` on attenuation
//!   radius, `-1.0f, 1.0f` on the spot cosines, `0.0f, 256.0f` on the
//!   attenuation exponent) are ImGui widget configuration, not clamps RT64
//!   applies itself, and are deliberately NOT ported as clamps -- ImGui
//!   enforces them inside the widget. The one clamp RT64 does apply in its
//!   own code, line 338, IS ported.
//! - The rename path (`.cpp` lines 300-318): `strncpy` into a fixed
//!   `char[256]`, `ImGui::InputText`, `strcmp`, `strlen`, and
//!   `std::map::extract`/`insert` node-handle rekeying. Driven end to end by
//!   an ImGui text field; the `strncpy(dst, src, 256)` with no forced NUL is
//!   a real truncation hazard, but it is unobservable without the widget and
//!   pinning it would require porting the widget.
//! - `PresetLights`, `PresetLightsLibrary`, `PresetLightsLibraryInspector`
//!   as types (`.h` lines 16-39), and `PresetLibrary`/`PresetLibraryInspector`
//!   /`PresetBase` (declared in `rt64_preset.h` / `rt64_preset_inspector.h`,
//!   not cited). `PresetLights::Light` is `{ PointLight; bool }` and the two
//!   library types are empty derivations; there is no behavior to pin, and
//!   `inspectLibrary` takes a `RenderWindow`. The two `.h` member
//!   initializers that ARE behavior -- `selectedLightName = ""` and
//!   `skipGizmos = false` -- are pinned by
//!   [`INITIAL_SELECTED_LIGHT_NAME`] / [`INITIAL_SKIP_GIZMOS`].
//! - `focusViewOnPoint` (`.cpp` lines 100-106) and its two call sites (148,
//!   167). Commented out in the pinned source; not live code.
//!
//! ## Verbatim key logic
//!
//! ```text
//! // rt64_preset_light.cpp lines 174-222 (generateLightName)
//! auto &lightMap = presetIt->second.lightMap;
//! auto generateLightName = [&](const std::string baseName) {
//!     // If the name already ends with a number, we try incrementing it instead.
//!     std::string newLightName = baseName;
//!     if (!newLightName.empty()) {
//!         size_t lastDigit = (newLightName.size() - 1);
//!         if (isdigit(newLightName[lastDigit])) {
//!             bool stopIncrementing = false;
//!             do {
//!                 int attemptDigit = static_cast<int>(lastDigit);
//!                 while ((newLightName[attemptDigit] == '9') && (attemptDigit >= 0)) {
//!                     newLightName[attemptDigit] = '0';
//!                     attemptDigit--;
//!
//!                     if (attemptDigit < 0) {
//!                         stopIncrementing = true;
//!                         break;
//!                     }
//!                     // Insert a new base 10 right before this digit.
//!                     else if (!isdigit(newLightName[attemptDigit])) {
//!                         newLightName = newLightName.substr(0, attemptDigit + 1) + "1" + newLightName.substr(attemptDigit + 1);
//!                         lastDigit++;
//!                         break;
//!                     }
//!                 }
//!
//!                 if (isdigit(newLightName[attemptDigit]) && (newLightName[attemptDigit] < '9')) {
//!                     newLightName[attemptDigit] += 1;
//!                 }
//!             } while (!stopIncrementing && (lightMap.find(newLightName) != lightMap.end()));
//!
//!             if (lightMap.find(newLightName) == lightMap.end()) {
//!                 return newLightName;
//!             }
//!         }
//!     }
//!
//!     // Generate a new light name automatically by appending a number suffix to the base name.
//!     unsigned int newLightCounter = 0;
//!     do {
//!         const size_t LeadingZeroes = 3;
//!         std::string numberSuffix = std::to_string(newLightCounter);
//!         numberSuffix = std::string(LeadingZeroes - std::min(LeadingZeroes, numberSuffix.length()), '0') + numberSuffix;
//!         newLightName = baseName + "_" + numberSuffix;
//!         newLightCounter++;
//!     } while (lightMap.find(newLightName) != lightMap.end());
//!
//!     return newLightName;
//! };
//!
//! // rt64_preset_light.cpp lines 33-46 (from_json defaults)
//! void from_json(const json &j, PointLight &l) {
//!     l.position = j.value("position", hlslpp::float3(0.0f, 0.0f, 0.0f));
//!     l.direction = j.value("direction", hlslpp::float3(0.0f, -10.0f, 0.0f));
//!     l.attenuationRadius = j.value("attenuationRadius", 500.0f);
//!     l.pointRadius = j.value("pointRadius", 5.0f);
//!     l.spotFalloffCosine = j.value("spotFalloffCosine", 1.0f);
//!     l.spotMaxCosine = j.value("spotMaxCosine", 1.0f);
//!     l.diffuseColor = j.value("diffuseColor", hlslpp::float3(1.0f, 1.0f, 1.0f));
//!     l.specularColor = j.value("specularColor", hlslpp::float3(0.5f, 0.5f, 0.5f));
//!     l.shadowOffset = j.value("shadowOffset", 5.0f);
//!     l.attenuationExponent = j.value("attenuationExponent", 1.0f);
//!     l.flickerIntensity = j.value("flickerIntensity", 0.0f);
//!     l.groupBits = j.value("groupBits", 1UL);
//! }
//!
//! // rt64_preset_light.cpp lines 226-242 (New##light descriptor)
//! lightMap[newLightName] = PresetLights::Light();
//! auto &desc = lightMap[newLightName].description;
//! const float DistanceFromView = -150.0f;
//! hlslpp::float3 viewPos = { appData.m_viewOrigin.x, appData.m_viewOrigin.y, appData.m_viewOrigin.z };
//! hlslpp::float3 viewDir = { appData.m_viewDirection.x, appData.m_viewDirection.y, appData.m_viewDirection.z };
//! desc.position = viewPos + viewDir * DistanceFromView;
//! desc.direction = { 0.0f, -1.0f, 0.0f };
//! desc.diffuseColor = { 1.0f, 1.0f, 1.0f };
//! desc.attenuationRadius = 500.0f;
//! desc.pointRadius = 15.0f;
//! desc.spotFalloffCosine = 1.0f;
//! desc.spotMaxCosine = 1.0f;
//! desc.specularColor = { 0.5f, 0.5f, 0.5f };
//! desc.shadowOffset = 5.0f;
//! desc.attenuationExponent = 1.0f;
//! desc.groupBits = 1;
//! lightMap[newLightName].enabled = true;
//!
//! // rt64_preset_light.cpp line 247 (lightSelected)
//! bool lightSelected = (selectedPresetName == presetIt->first) && (lightMap.find(selectedLightName) != lightMap.end());
//!
//! // rt64_preset_light.cpp lines 338-348, 357-377 (clamp, gizmo toggle, cones)
//! lightDesc.spotFalloffCosine = std::min(lightDesc.spotFalloffCosine, lightDesc.spotMaxCosine);
//!
//! if (skipGizmos) {
//!     skipGizmos = false;
//! }
//! else {
//!     ...
//!     if ((lightDesc.spotFalloffCosine < 1.0f) || (lightDesc.spotMaxCosine < 1.0f)) {
//!         ...
//!         auto drawConeCircles = [&](const float cosine) {
//!             const auto &srcDir = lightDesc.direction;
//!             const hlslpp::float3 lightDir = { srcDir.x, srcDir.y, srcDir.z };
//!             float spotRadius = hlslpp::length(lightDir) * tanf(acosf(-cosine));
//!             const int Segments = 20;
//!             const float SegmentMult = (3.0f / Segments);
//!             for (int i = 0; i < Segments; i++) {
//!                 const float iF = static_cast<float>(i);
//!                 Im3d::DrawCircle(lightPos + focusDir * SegmentMult * iF, focusDir, spotRadius * SegmentMult * iF);
//!             }
//!         };
//!
//!         if (lightDesc.spotMaxCosine < 0.0f) { ... drawConeCircles(lightDesc.spotMaxCosine); }
//!         if (lightDesc.spotFalloffCosine < 0.0f) { ... drawConeCircles(lightDesc.spotFalloffCosine); }
//!     }
//! }
//!
//! // rt64_preset_light.cpp lines 381-385 (diffuse-color normalization)
//! const auto &srcCol = lightDesc.diffuseColor;
//! const hlslpp::float3 col = { srcCol.x, srcCol.y, srcCol.z };
//! float colLength = hlslpp::length(col);
//! const Im3d::Color lightCol(lightDesc.diffuseColor.x / colLength, lightDesc.diffuseColor.y / colLength, lightDesc.diffuseColor.z / colLength);
//!
//! // rt64_preset_light.h lines 33-35 (inspector member initializers)
//! std::string selectedLightName = "";
//! bool skipGizmos = false;
//! ```
//!
//! ## Reuse, not new type
//!
//! This module defines **no** vector, color, or light type of its own. It
//! reuses:
//! - [`fn64_render_ir::Vec3`] (`crates/fn64-render-ir/src/rsp_math.rs`) for
//!   every `hlslpp::float3`, the same substitution the landed
//!   `rt64_light_estimation` module already made for this exact C++ type.
//! - [`crate::rt64_light_estimation::PointLight`], the already-landed literal
//!   port of `src/shared/rt64_point_light.h`'s `interop::PointLight` -- the
//!   same twelve fields in the same declaration order that both
//!   `from_json` and the New-light button here initialize. Redefining it
//!   would be a duplicate; this module constructs and returns that type.
//!
//! `lightMap` is `std::map<std::string, Light>`, and the only thing
//! `generateLightName` and `lightSelected` ask of it is "does this key
//! exist". Rather than require a caller to build a whole map of lights to
//! call a name generator, both take a `&BTreeSet<String>` -- `BTreeSet`
//! rather than `HashSet` because `std::map` is ordered and the set is
//! iterated nowhere here, so the ordered container is the closer analog and
//! makes the port deterministic without further argument.
//!
//! ## Admitted domain
//!
//! - **[`generate_light_name`] increment path**: entered only when the base
//!   name is non-empty AND its **last byte** is an ASCII digit. `isdigit` on
//!   a `char` is byte-wise in the C locale, so a multi-byte UTF-8 tail never
//!   qualifies; this port indexes bytes, not `char`s, for the same reason,
//!   and the digit test is `b'0'..=b'9'`.
//! - **Carry**: the inner `while` rewrites each trailing `'9'` to `'0'` and
//!   steps left. It stops in one of three ways: (a) the byte it steps onto is
//!   not `'9'`, ending the `while` normally; (b) it steps left of index 0,
//!   setting `stopIncrementing` and breaking; (c) it steps onto a non-digit,
//!   in which case a `'1'` is **inserted** immediately after that non-digit
//!   and `lastDigit` is incremented to account for the longer string.
//! - **The `while` condition is `(name[attemptDigit] == '9') && (attemptDigit
//!   >= 0)`** -- the bounds test is written AFTER the subscript it is
//!   supposed to guard, so it can never prevent an out-of-range read. It is
//!   nonetheless dead as a guard on every input, because case (b) above
//!   breaks out the instant `attemptDigit` goes negative, so the condition is
//!   never evaluated with a negative index and the two orderings agree
//!   everywhere. Ported in that same order (subscript first, `>= 0` second)
//!   rather than "fixed". See Nonclaims for the one place the C++ *does* read
//!   out of range -- it is not this loop condition, it is the post-carry
//!   `isdigit` below it.
//! - **Post-carry increment**: `if (isdigit(name[attemptDigit]) &&
//!   (name[attemptDigit] < '9')) name[attemptDigit] += 1;`. The `isdigit`
//!   half is load-bearing and is tested: a non-digit is never incremented,
//!   which is exactly what makes case (c)'s freshly inserted `'1'` survive --
//!   after case (c), `attemptDigit` points at the **non-digit**, so this `if`
//!   is false and the `'1'` stands. The **strict** `< '9'` half, by contrast,
//!   is dead on every reachable input, for the same reason the `>= 0`
//!   conjunct above is: the byte this test observes is never `'9'`. Each of
//!   the four ways control arrives here guarantees it -- the `while` was
//!   never entered (so the byte is the non-`'9'` last byte), the `while` exited
//!   normally (so the byte is not `'9'`), case (c) fired (so the byte is a
//!   non-digit), or case (b) fired (so the index is negative and the port
//!   skips the test entirely). Any `'9'` the loop walks over has already been
//!   rewritten to `'0'` before this test could observe it. Ported verbatim
//!   with the `< '9'` conjunct kept because the source has it, not because it
//!   is exercised; changing it to `<= '9'` leaves every test in this module
//!   passing.
//! - **`from_utf8_lossy` never actually loses anything here.** The port
//!   mutates a `Vec<u8>` taken from a `&str`, so the buffer starts as valid
//!   UTF-8, and all three mutations preserve that: rewriting a `'9'` to `'0'`
//!   and incrementing a digit both keep an ASCII byte ASCII, and the case-(c)
//!   insertion lands at a code-point boundary. The insertion index is
//!   `attemptDigit + 1`, and case (c) is reached only after at least one carry
//!   step, so every byte from there through `lastDigit` is one the carry loop
//!   just wrote as ASCII `'0'`; an ASCII byte is never a UTF-8 continuation
//!   byte, so that index cannot fall inside a multi-byte sequence. The
//!   `lossy` calls therefore never substitute U+FFFD, and `Vec<u8>` is used
//!   only because the original indexes bytes.
//! - **Loop repetition**: `do { ... } while (!stopIncrementing && taken)`. So
//!   the increment runs at least once even if the base name is free, and
//!   repeats while the produced name is still taken. `"a0"` with `"a1"` taken
//!   yields `"a2"`.
//! - **After the loop**, the incremented name is returned only if it is
//!   actually free (`find(...) == end()`); when `stopIncrementing` ended the
//!   loop on a still-taken name, control falls through to the suffix path.
//! - **Suffix path**: `newLightName = baseName + "_" + suffix` -- built from
//!   the **original** `baseName`, NOT the partially incremented
//!   `newLightName`, even when the increment path ran and mutated it. The
//!   suffix is `newLightCounter` in decimal, left-padded with `'0'` to a
//!   minimum width of 3 via `std::string(LeadingZeroes -
//!   std::min(LeadingZeroes, len), '0')`; the `std::min` is what keeps the
//!   `size_t` subtraction from wrapping once the counter reaches 4 digits, so
//!   counter 1000 yields `"1000"` unpadded rather than a ~2^64-length pad.
//!   That saturation is ported literally as `3usize.saturating_sub(len)`,
//!   which is the same value for every `len`.
//! - **The suffix loop is also `do`/`while`**, so `base_000` is always tried
//!   first, and the counter increments unboundedly until a free name is
//!   found.
//! - **[`point_light_json_defaults`]**: `direction` defaults to
//!   `(0, -10, 0)`, a length-10 vector, NOT a unit vector -- unlike the
//!   New-light button's `(0, -1, 0)`. The two entry points disagree on the
//!   default direction magnitude by 10x, and since [`spot_cone_radius`]
//!   scales by `length(direction)`, that difference is observable. Ported as
//!   two different constants, pinned by a test that asserts they differ.
//!   `pointRadius` likewise differs: `5.0` from JSON, `15.0` from the button.
//!   `groupBits`' JSON default is written `1UL` (an `unsigned long`) while
//!   the field is `uint`; the value 1 is representable either way, so the
//!   port uses `1u32`.
//! - **[`new_light_descriptor`]**: `flickerIntensity` is the one field the
//!   New button never assigns. `PresetLights::Light()` value-initializes
//!   `interop::PointLight`, which has no member initializers and no
//!   user-provided constructor, so `flickerIntensity` is zero-initialized;
//!   this port sets it to `0.0` explicitly and says so here rather than
//!   leaving it to a `Default` impl. `position` is `viewPos + viewDir *
//!   (-150.0)`, evaluated component-wise in that operand order (multiply
//!   first, then add), preserved exactly.
//! - **[`clamp_spot_falloff_cosine`]**: `std::min(a, b)` returns `b < a ? b :
//!   a`, i.e. it returns the **first** argument on a tie and, critically for
//!   NaN, returns `a` whenever the comparison `b < a` is false -- which
//!   includes every NaN case. So `min(NaN, 1.0)` is `NaN` and `min(1.0, NaN)`
//!   is `1.0`. Rust's `f32::min` differs (it returns the non-NaN operand), so
//!   this port writes the ternary out literally rather than calling
//!   `f32::min`, and both NaN orders are tested.
//! - **[`normalize_light_color`]**: `length` is
//!   `sqrt(x*x + y*y + z*z)` and there is no zero guard, so a pure-black
//!   diffuse color divides by zero. IEEE-754 gives `0.0 / 0.0 = NaN`, so a
//!   `(0,0,0)` color normalizes to three NaNs, and a `(0, 0, 5)` color
//!   normalizes to `(0, 0, 1)` with the two zeros preserving sign
//!   (`-0.0 / 5.0 = -0.0`). Both are tested.
//! - **[`light_selected`]**: `(selectedPresetName == presetIt->first) &&
//!   (lightMap.find(selectedLightName) != lightMap.end())` -- an `&&`, and
//!   here the `&&` is the sensible reading (both must hold), unlike the
//!   `&&` quirk in the neighbouring `rt64_preset_draw_call` port. All four
//!   mixed cases are tested anyway.
//! - **[`take_skip_gizmos`]**: `if (skipGizmos) { skipGizmos = false; } else
//!   { ...draw... }`. A single flagged frame suppresses the gizmos AND
//!   clears the flag, so the very next frame draws. Ported as a
//!   `&mut bool` returning whether the caller should draw.
//! - **[`should_draw_spot_cone`]**: `(spotFalloffCosine < 1.0f) ||
//!   (spotMaxCosine < 1.0f)` -- **strict** `<`, so a light with both cosines
//!   at exactly `1.0` (the default from both entry points) draws no cone.
//!   An `||`, so either cosine alone below 1 is enough.
//! - **[`should_draw_max_cone`] / [`should_draw_falloff_cone`]**: each
//!   `cosine < 0.0f`, again **strict**, so exactly `0.0` draws nothing. Note
//!   these are `< 0`, not `< 1`: a cosine in `[0, 1)` passes the outer gate
//!   at line 348 and draws the line-and-sphere marker but no cone circles.
//!   That gap is real and is pinned by a test.
//! - **[`spot_cone_radius`]**: `length(lightDir) * tanf(acosf(-cosine))`.
//!   The negation of the cosine before `acos` is literal. `acos` of an
//!   argument outside `[-1, 1]` is NaN, so `cosine` outside `[-1, 1]`
//!   produces NaN; `acos(-(-1.0)) = acos(1.0) = 0` gives `tan(0) = 0` and a
//!   zero radius; `acos(-0.0) = pi/2` gives a `tan` near the pole, which is
//!   a very large finite float rather than an infinity because `pi/2` is not
//!   exactly representable. All three are tested as behavior, with the
//!   pole case tested by magnitude rather than by an exact bit pattern.
//! - **[`spot_cone_circles`]**: `Segments = 20` and `SegmentMult = 3.0f / 20`
//!   -- an `int` divisor, so this is `3.0f / 20` in float after the usual
//!   arithmetic conversions, exactly `0.15`. The `i = 0` circle therefore has
//!   center `lightPos + focusDir * 0.15 * 0.0 == lightPos` and radius `0`;
//!   the largest, `i = 19`, is at `0.15 * 19 = 2.85` along `focusDir`, never
//!   reaching the `3.0` the multiplier suggests. Products are formed
//!   left-to-right, `((focusDir * SegmentMult) * iF)` and `((spotRadius *
//!   SegmentMult) * iF)`, matching C++'s left-associative `*`; the
//!   association is preserved because refactoring it would change rounding.
//!
//! ## Nonclaims
//!
//! No GPU, no wgpu resource, no bind group, no shader. No production wiring:
//! this module is declared `mod`, not `pub mod`, nothing re-exports it, and
//! nothing in `fn64-render-wgpu` calls it. No parity or performance claim --
//! this is a CPU-only characterization, not a validated match against RT64
//! runtime output. Specifically NOT claimed:
//! - **The all-nines out-of-range read is not reproduced.** When every byte
//!   of the name from `lastDigit` down to index 0 is `'9'` (e.g. `"99"`), the
//!   C++ carry loop drives `attemptDigit` to `-1`, breaks, and then
//!   unconditionally evaluates `isdigit(newLightName[-1])` -- a read one byte
//!   before the buffer. That is undefined behavior with no defined result to
//!   port. This module instead skips the post-carry increment entirely when
//!   `attemptDigit < 0`, which is what every observed-in-practice
//!   implementation does (the byte before a `std::string`'s data is not a
//!   digit in any libstdc++/libc++ layout), and marks it as a **deliberate
//!   deviation**. A test pins the chosen behavior (`"99"` -> `"00"`) and is
//!   labelled as pinning the deviation, not the original.
//! - **`isdigit` with a negative `char`.** C's `isdigit` is UB for arguments
//!   outside `unsigned char` and `EOF`; on a platform with signed `char`, a
//!   byte >= 0x80 is passed negative. This port treats every byte >= 0x80 as
//!   a non-digit, which is what the C locale's table gives for the
//!   well-defined `unsigned char` range, and does not claim to reproduce the
//!   UB path.
//! - **The `unsigned int newLightCounter` overflow.** The suffix loop could
//!   in principle wrap at 2^32; reaching it requires 2^32 taken names and it
//!   is not tested or claimed. The port uses `u32` and would panic in debug
//!   on overflow rather than wrapping.
//! - **hlslpp's `length`.** `hlslpp::length` is SIMD and may use a
//!   reciprocal-square-root refinement rather than a scalar `sqrtf`. This
//!   port uses `f32::sqrt` (an exact IEEE-754 operation). For a
//!   bit-exactness claim against a real hlslpp build this would need
//!   checking; it is not claimed here, and no test asserts a `length`-derived
//!   value to the last ulp.
//! - **`tanf`/`acosf` last-ulp agreement.** These are libm functions with no
//!   IEEE-754 exactness requirement; Rust's `f32::tan`/`f32::acos` may differ
//!   from a given C library's by an ulp. Tests assert to a tolerance except
//!   where the value is exactly representable (`tan(acos(1.0)) == 0.0`).

use std::collections::BTreeSet;

use fn64_render_ir::Vec3;

use crate::rt64_light_estimation::PointLight;

/// `PresetLightsLibraryInspector::selectedLightName`'s member initializer
/// (`rt64_preset_light.h` line 34): `std::string selectedLightName = ""`.
pub const INITIAL_SELECTED_LIGHT_NAME: &str = "";

/// `PresetLightsLibraryInspector::skipGizmos`'s member initializer
/// (`rt64_preset_light.h` line 35): `bool skipGizmos = false`.
pub const INITIAL_SKIP_GIZMOS: bool = false;

/// `generateLightName`'s `LeadingZeroes` (`rt64_preset_light.cpp` line 214).
const LEADING_ZEROES: usize = 3;

/// The base name the `New##light` button passes to `generateLightName`
/// (`rt64_preset_light.cpp` line 225): `generateLightName("point_00")`.
pub const NEW_LIGHT_BASE_NAME: &str = "point_00";

/// `drawConeCircles`' `Segments` (`rt64_preset_light.cpp` line 361).
pub const CONE_SEGMENTS: i32 = 20;

/// `drawConeCircles`' `SegmentMult`, `(3.0f / Segments)`
/// (`rt64_preset_light.cpp` line 362). Written with the same `3.0 / 20`
/// division rather than as the literal `0.15` it evaluates to.
pub const CONE_SEGMENT_MULT: f32 = 3.0 / (CONE_SEGMENTS as f32);

/// The `New##light` button's `DistanceFromView` (`rt64_preset_light.cpp`
/// line 228). Negative: the new light is placed *behind* the view origin
/// relative to the view direction.
pub const NEW_LIGHT_DISTANCE_FROM_VIEW: f32 = -150.0;

/// C `isdigit` in the C locale, over a single byte. Bytes `>= 0x80` are
/// non-digits here; see module doc "Nonclaims" on the signed-`char` UB the
/// original has and this does not reproduce.
fn is_digit_byte(b: u8) -> bool {
    b >= b'0' && b <= b'9'
}

/// Literal port of the `generateLightName` lambda
/// (`rt64_preset_light.cpp` lines 175-222). `taken` stands in for the
/// `lightMap.find(name) != lightMap.end()` membership test the lambda
/// captures; see the module doc's "Reuse, not new type" on why it is a
/// `BTreeSet<String>` rather than a map of lights.
///
/// Two behaviors are easy to misread and are spelled out in the module doc's
/// "Admitted domain": the suffix fallback rebuilds from `base_name`, not from
/// the mutated increment result; and the all-nines case is a deliberate
/// deviation from an out-of-range read in the original (see "Nonclaims").
pub fn generate_light_name(base_name: &str, taken: &BTreeSet<String>) -> String {
    // If the name already ends with a number, we try incrementing it instead.
    let mut new_light_name: Vec<u8> = base_name.as_bytes().to_vec();
    if !new_light_name.is_empty() {
        let mut last_digit: usize = new_light_name.len() - 1;
        if is_digit_byte(new_light_name[last_digit]) {
            let mut stop_incrementing = false;
            loop {
                let mut attempt_digit: i64 = last_digit as i64;
                // Original: `while ((newLightName[attemptDigit] == '9') &&
                // (attemptDigit >= 0))` -- the subscript first, the bounds
                // test second. `attempt_digit` is provably >= 0 at every
                // evaluation of this condition (it starts at `last_digit`,
                // and the only decrement below breaks out the instant it
                // goes negative), so the two orderings agree on every input;
                // the `>= 0` half is dead in both. The subscript is written
                // first here, matching the original, with the same trailing
                // `>= 0` conjunct kept verbatim.
                while new_light_name[attempt_digit as usize] == b'9' && attempt_digit >= 0 {
                    new_light_name[attempt_digit as usize] = b'0';
                    attempt_digit -= 1;

                    if attempt_digit < 0 {
                        stop_incrementing = true;
                        break;
                    }
                    // Insert a new base 10 right before this digit.
                    else if !is_digit_byte(new_light_name[attempt_digit as usize]) {
                        let at = (attempt_digit + 1) as usize;
                        new_light_name.insert(at, b'1');
                        last_digit += 1;
                        break;
                    }
                }

                // The original evaluates `isdigit(newLightName[attemptDigit])`
                // here even when `attemptDigit` is -1, reading out of range.
                // This guard is the deliberate deviation documented under
                // "Nonclaims"; everything after it is literal.
                //
                // The `< b'9'` conjunct below is dead on every reachable
                // input -- the byte at `idx` is never `b'9'`, because the
                // carry loop rewrites any `b'9'` it walks over to `b'0'`
                // before this test can observe it (see "Admitted domain").
                // It is kept because the source has it, not because it is
                // exercised; `<= b'9'` would pass every test here.
                if attempt_digit >= 0 {
                    let idx = attempt_digit as usize;
                    if is_digit_byte(new_light_name[idx]) && new_light_name[idx] < b'9' {
                        new_light_name[idx] += 1;
                    }
                }

                let taken_now = taken.contains(String::from_utf8_lossy(&new_light_name).as_ref());
                if stop_incrementing || !taken_now {
                    break;
                }
            }

            let candidate = String::from_utf8_lossy(&new_light_name).into_owned();
            if !taken.contains(&candidate) {
                return candidate;
            }
        }
    }

    // Generate a new light name automatically by appending a number suffix to
    // the base name. Note this rebuilds from `base_name`, discarding whatever
    // the increment path above left in `new_light_name`.
    let mut new_light_counter: u32 = 0;
    loop {
        let number_suffix = new_light_counter.to_string();
        let pad = LEADING_ZEROES.saturating_sub(number_suffix.len());
        let number_suffix = "0".repeat(pad) + &number_suffix;
        let name = format!("{base_name}_{number_suffix}");
        new_light_counter += 1;
        if !taken.contains(&name) {
            return name;
        }
    }
}

/// The twelve fallback values `interop::from_json` supplies for a
/// `PointLight` when the corresponding JSON key is absent
/// (`rt64_preset_light.cpp` lines 33-46). The `nlohmann::json` lookup itself
/// is refused; these constants are the behavior. Note `direction` here is
/// `(0, -10, 0)` -- ten times the length of [`new_light_descriptor`]'s
/// `(0, -1, 0)` -- and `point_radius` is `5.0` against the button's `15.0`.
pub fn point_light_json_defaults() -> PointLight {
    PointLight {
        position: Vec3::new(0.0, 0.0, 0.0),
        direction: Vec3::new(0.0, -10.0, 0.0),
        diffuse_color: Vec3::new(1.0, 1.0, 1.0),
        attenuation_radius: 500.0,
        point_radius: 5.0,
        spot_falloff_cosine: 1.0,
        spot_max_cosine: 1.0,
        specular_color: Vec3::new(0.5, 0.5, 0.5),
        shadow_offset: 5.0,
        attenuation_exponent: 1.0,
        flicker_intensity: 0.0,
        group_bits: 1,
    }
}

/// Literal port of the `New##light` descriptor initialization
/// (`rt64_preset_light.cpp` lines 226-242), minus the ImGui button test.
/// Returns the descriptor and its `enabled` flag as the pair the original
/// writes into `lightMap[newLightName]`.
///
/// `position` is `viewPos + viewDir * DistanceFromView` with
/// `DistanceFromView = -150.0`, multiply-then-add per component in that
/// order. `flicker_intensity` is the one field the button never assigns; it
/// comes from `PresetLights::Light()`'s value-initialization and is `0.0`
/// (see module doc).
pub fn new_light_descriptor(view_origin: Vec3, view_direction: Vec3) -> (PointLight, bool) {
    let distance_from_view = NEW_LIGHT_DISTANCE_FROM_VIEW;
    let desc = PointLight {
        position: Vec3::new(
            view_origin.x + view_direction.x * distance_from_view,
            view_origin.y + view_direction.y * distance_from_view,
            view_origin.z + view_direction.z * distance_from_view,
        ),
        direction: Vec3::new(0.0, -1.0, 0.0),
        diffuse_color: Vec3::new(1.0, 1.0, 1.0),
        attenuation_radius: 500.0,
        point_radius: 15.0,
        spot_falloff_cosine: 1.0,
        spot_max_cosine: 1.0,
        specular_color: Vec3::new(0.5, 0.5, 0.5),
        shadow_offset: 5.0,
        attenuation_exponent: 1.0,
        flicker_intensity: 0.0,
        group_bits: 1,
    };

    (desc, true)
}

/// Literal port of `rt64_preset_light.cpp` line 338:
/// `lightDesc.spotFalloffCosine = std::min(lightDesc.spotFalloffCosine,
/// lightDesc.spotMaxCosine);`.
///
/// Written as `std::min`'s defining ternary `b < a ? b : a` rather than as
/// `f32::min`, because the two disagree on NaN: `std::min` returns the first
/// argument whenever the `<` is false, so `min(NaN, x)` is `NaN`, while
/// `f32::min` would return `x`.
pub fn clamp_spot_falloff_cosine(spot_falloff_cosine: f32, spot_max_cosine: f32) -> f32 {
    if spot_max_cosine < spot_falloff_cosine {
        spot_max_cosine
    } else {
        spot_falloff_cosine
    }
}

/// `hlslpp::length` over a `float3`: `sqrt(x*x + y*y + z*z)`, summed in that
/// left-to-right order. See the module doc's "Nonclaims" on hlslpp's SIMD
/// implementation possibly differing in the last ulp.
pub fn light_vector_length(v: Vec3) -> f32 {
    (v.x * v.x + v.y * v.y + v.z * v.z).sqrt()
}

/// Literal port of the diffuse-color normalization at
/// `rt64_preset_light.cpp` lines 133-135 and 381-385: each component divided
/// by `hlslpp::length(col)`, with **no** zero-length guard. A `(0, 0, 0)`
/// color yields three NaNs.
pub fn normalize_light_color(col: Vec3) -> Vec3 {
    let col_length = light_vector_length(col);
    Vec3::new(col.x / col_length, col.y / col_length, col.z / col_length)
}

/// Literal port of `lightSelected` (`rt64_preset_light.cpp` line 247):
/// `(selectedPresetName == presetIt->first) &&
/// (lightMap.find(selectedLightName) != lightMap.end())`. An `&&`; both
/// halves must hold.
pub fn light_selected(
    selected_preset_name: &str,
    preset_name: &str,
    selected_light_name: &str,
    light_names: &BTreeSet<String>,
) -> bool {
    (selected_preset_name == preset_name) && light_names.contains(selected_light_name)
}

/// Literal port of the gizmo-suppression toggle
/// (`rt64_preset_light.cpp` lines 341-343). Returns `true` when the caller
/// should draw the gizmos this frame; when the flag was set, clears it and
/// returns `false`, so at most one frame is ever suppressed per set.
pub fn take_skip_gizmos(skip_gizmos: &mut bool) -> bool {
    if *skip_gizmos {
        *skip_gizmos = false;
        false
    } else {
        true
    }
}

/// Literal port of `rt64_preset_light.cpp` line 348:
/// `(lightDesc.spotFalloffCosine < 1.0f) || (lightDesc.spotMaxCosine < 1.0f)`.
/// Strict `<` and an `||`.
pub fn should_draw_spot_cone(spot_falloff_cosine: f32, spot_max_cosine: f32) -> bool {
    (spot_falloff_cosine < 1.0) || (spot_max_cosine < 1.0)
}

/// Literal port of `rt64_preset_light.cpp` line 369:
/// `if (lightDesc.spotMaxCosine < 0.0f)`. Strict `<`, and `0.0` rather than
/// the `1.0` of the enclosing [`should_draw_spot_cone`] gate.
pub fn should_draw_max_cone(spot_max_cosine: f32) -> bool {
    spot_max_cosine < 0.0
}

/// Literal port of `rt64_preset_light.cpp` line 374:
/// `if (lightDesc.spotFalloffCosine < 0.0f)`. Strict `<`.
pub fn should_draw_falloff_cone(spot_falloff_cosine: f32) -> bool {
    spot_falloff_cosine < 0.0
}

/// Literal port of `drawConeCircles`' radius
/// (`rt64_preset_light.cpp` line 360):
/// `hlslpp::length(lightDir) * tanf(acosf(-cosine))`. The cosine is negated
/// before `acos`, so a `cosine` of `-1` gives `acos(1) = 0` and a zero
/// radius, while a `cosine` of `0` gives `acos(0) = pi/2` and a `tan` near
/// the pole.
pub fn spot_cone_radius(light_dir: Vec3, cosine: f32) -> f32 {
    light_vector_length(light_dir) * (-cosine).acos().tan()
}

/// Literal port of `drawConeCircles`' loop
/// (`rt64_preset_light.cpp` lines 361-366), returning the 20
/// `(center, radius)` pairs it hands to `Im3d::DrawCircle` -- the draw calls
/// themselves are refused. Every circle shares `focus_dir` as its normal,
/// which is the loop's third `DrawCircle` argument and is not returned.
///
/// `center` is `lightPos + focusDir * SegmentMult * iF` and `radius` is
/// `spotRadius * SegmentMult * iF`, both left-associative: the `SegmentMult`
/// product is formed first, then scaled by `iF`.
pub fn spot_cone_circles(light_pos: Vec3, focus_dir: Vec3, spot_radius: f32) -> Vec<(Vec3, f32)> {
    let mut out = Vec::new();
    for i in 0..CONE_SEGMENTS {
        let i_f = i as f32;
        let center = Vec3::new(
            light_pos.x + focus_dir.x * CONE_SEGMENT_MULT * i_f,
            light_pos.y + focus_dir.y * CONE_SEGMENT_MULT * i_f,
            light_pos.z + focus_dir.z * CONE_SEGMENT_MULT * i_f,
        );
        out.push((center, spot_radius * CONE_SEGMENT_MULT * i_f));
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn taken(names: &[&str]) -> BTreeSet<String> {
        names.iter().map(|s| (*s).to_string()).collect()
    }

    fn empty() -> BTreeSet<String> {
        BTreeSet::new()
    }

    // -- generate_light_name: the suffix fallback path --

    #[test]
    fn non_digit_tail_skips_the_increment_path_entirely() {
        // "lamp" does not end in a digit, so the increment path is not
        // entered and the suffix loop runs from counter 0: "0" padded to
        // width 3 is "000".
        assert_eq!(generate_light_name("lamp", &empty()), "lamp_000");
    }

    #[test]
    fn empty_base_name_skips_the_increment_path() {
        // `!newLightName.empty()` is false, so the suffix path builds
        // "" + "_" + "000".
        assert_eq!(generate_light_name("", &empty()), "_000");
    }

    #[test]
    fn suffix_counter_advances_past_taken_names() {
        // 000 and 001 taken; the do/while tries 000, 001, then 002.
        assert_eq!(
            generate_light_name("lamp", &taken(&["lamp_000", "lamp_001"])),
            "lamp_002"
        );
    }

    #[test]
    fn suffix_pads_two_digit_counters_to_width_three() {
        // Counters 0..=9 give "000".."009"; counter 10 gives "010".
        let names: Vec<String> = (0..10).map(|i| format!("lamp_{i:03}")).collect();
        let set: BTreeSet<String> = names.into_iter().collect();
        assert_eq!(generate_light_name("lamp", &set), "lamp_010");
    }

    #[test]
    fn suffix_does_not_pad_a_four_digit_counter() {
        // `LeadingZeroes - min(LeadingZeroes, 4)` is 0, so "1000" is
        // unpadded. Blocking 0..=999 forces the counter to 1000.
        let set: BTreeSet<String> = (0..1000)
            .map(|i| {
                let s = i.to_string();
                format!("lamp_{}{}", "0".repeat(3usize.saturating_sub(s.len())), s)
            })
            .collect();
        assert_eq!(generate_light_name("lamp", &set), "lamp_1000");
    }

    #[test]
    fn suffix_rebuilds_from_base_name_not_the_mutated_increment() {
        // "99" all-nines: the carry path turns it into "00" (deliberate
        // deviation, see the dedicated test), which is taken, so
        // stopIncrementing ends the loop on a taken name and control falls
        // through. The suffix path then uses the ORIGINAL "99", not "00".
        assert_eq!(generate_light_name("99", &taken(&["00"])), "99_000");
    }

    // -- generate_light_name: the increment path --

    #[test]
    fn trailing_digit_increments_by_one_even_when_free() {
        // The loop is a do/while, so the increment runs at least once even
        // though "a0" is not taken. '0' + 1 == '1'.
        assert_eq!(generate_light_name("a0", &empty()), "a1");
    }

    #[test]
    fn trailing_digit_increments_repeatedly_while_taken() {
        // "a0" -> "a1" (taken) -> "a2" (taken) -> "a3" (free).
        assert_eq!(generate_light_name("a0", &taken(&["a1", "a2"])), "a3");
    }

    #[test]
    fn eight_increments_to_nine_in_place() {
        // The carry `while` never runs ('8' is not '9'), so the post-carry
        // increment fires on '8' directly: '8' -> '9'. Note this does NOT
        // exercise the `< '9'` conjunct as a discriminator -- that half is
        // dead (see the module doc); the `isdigit` half is what gates here.
        assert_eq!(generate_light_name("a8", &empty()), "a9");
    }

    #[test]
    fn nine_carries_into_an_inserted_one_after_the_non_digit() {
        // "a9": carry rewrites index 1 to '0', steps to index 0 ('a', a
        // non-digit), inserts '1' at index 1 giving "a10". attemptDigit then
        // points at 'a', so the post-carry increment's isdigit() half is
        // false and the '1' survives.
        assert_eq!(generate_light_name("a9", &empty()), "a10");
    }

    #[test]
    fn double_nine_carries_and_inserts_a_single_one() {
        // "a99": index 2 '9'->'0', step to 1; name[1] is '9' so the while
        // continues: index 1 '9'->'0', step to 0; 'a' is a non-digit, insert
        // '1' at index 1 -> "a100". No further increment.
        assert_eq!(generate_light_name("a99", &empty()), "a100");
    }

    #[test]
    fn carry_stops_at_the_first_non_nine_digit_and_increments_it() {
        // "a19": index 2 '9'->'0', step to 1; name[1] is '1', not '9', so
        // the while exits normally. isdigit('1') && '1' < '9' -> '1'+1='2'.
        assert_eq!(generate_light_name("a19", &empty()), "a20");
    }

    #[test]
    fn carry_across_two_digits_increments_the_third() {
        // "a799": index 3 '9'->'0', index 2 '9'->'0', step to 1; '7' is not
        // '9', while exits. '7' < '9' -> '8'. Result "a800".
        assert_eq!(generate_light_name("a799", &empty()), "a800");
    }

    #[test]
    fn point_00_the_new_button_base_name_increments_to_point_01() {
        // The New##light button calls generateLightName("point_00").
        // Last byte '0', not '9': while does not run, '0' < '9' -> '1'.
        assert_eq!(
            generate_light_name(NEW_LIGHT_BASE_NAME, &empty()),
            "point_01"
        );
    }

    #[test]
    fn point_09_carries_within_the_two_digit_suffix() {
        // "point_09": index 7 '9'->'0', step to 6; name[6] is '0', a digit
        // and not '9', so the while exits. '0' < '9' -> '1'. "point_10".
        assert_eq!(generate_light_name("point_09", &empty()), "point_10");
    }

    #[test]
    fn point_99_inserts_a_one_after_the_underscore() {
        // "point_99": both '9's become '0', step to index 5 ('_', a
        // non-digit), insert '1' at index 6 -> "point_100".
        assert_eq!(generate_light_name("point_99", &empty()), "point_100");
    }

    #[test]
    fn digits_only_name_with_a_leading_non_nine_increments_normally() {
        // "19": index 1 '9'->'0', step to 0; '1' is not '9', while exits.
        // '1' < '9' -> '2'. Result "20". No insertion, no stopIncrementing.
        assert_eq!(generate_light_name("19", &empty()), "20");
    }

    #[test]
    fn all_nines_name_wraps_to_all_zeroes_deliberate_deviation() {
        // DELIBERATE DEVIATION, not a claim about the original. "99":
        // index 1 '9'->'0', step to 0; name[0] is '9' so the while runs
        // again: index 0 '9'->'0', step to -1, stopIncrementing = true,
        // break. The original then reads newLightName[-1] out of range; this
        // port skips the increment. "00" is free, so it is returned.
        assert_eq!(generate_light_name("99", &empty()), "00");
    }

    #[test]
    fn single_nine_name_wraps_to_zero_deliberate_deviation() {
        // DELIBERATE DEVIATION. "9": index 0 '9'->'0', step to -1, stop.
        assert_eq!(generate_light_name("9", &empty()), "0");
    }

    #[test]
    fn single_digit_name_below_nine_increments_in_place() {
        // "7" is a one-byte name whose only byte is a digit below '9'.
        assert_eq!(generate_light_name("7", &empty()), "8");
    }

    #[test]
    fn incremented_name_still_taken_after_stop_falls_through_to_suffix() {
        // DELIBERATE DEVIATION path combined with the fallback: "9" wraps to
        // "0", which is taken, and stopIncrementing prevents another pass.
        // The post-loop `find == end` check fails, so the suffix path runs
        // from the original base "9".
        assert_eq!(generate_light_name("9", &taken(&["0"])), "9_000");
    }

    #[test]
    fn insertion_case_result_taken_loops_again_from_the_longer_name() {
        // "a9" -> "a10" (taken). stopIncrementing is false, so the do/while
        // runs again with lastDigit now 2: name[2] is '0', not '9', so
        // '0' -> '1' giving "a11".
        assert_eq!(generate_light_name("a9", &taken(&["a10"])), "a11");
    }

    #[test]
    fn multibyte_tail_is_not_a_digit_and_takes_the_suffix_path() {
        // The last BYTE of "lamp\u{00e9}" is 0xA9, which is >= 0x80 and so a
        // non-digit here (see Nonclaims on the original's signed-char UB).
        assert_eq!(
            generate_light_name("lamp\u{00e9}", &empty()),
            "lamp\u{00e9}_000"
        );
    }

    // -- is_digit_byte boundaries --

    #[test]
    fn digit_byte_boundaries_are_inclusive_on_both_ends() {
        assert!(!is_digit_byte(b'0' - 1)); // 0x2F '/'
        assert!(is_digit_byte(b'0'));
        assert!(is_digit_byte(b'9'));
        assert!(!is_digit_byte(b'9' + 1)); // 0x3A ':'
    }

    #[test]
    fn high_bytes_are_not_digits() {
        assert!(!is_digit_byte(0x80));
        assert!(!is_digit_byte(0xFF));
        assert!(!is_digit_byte(0x00));
    }

    // -- header member initializers --

    #[test]
    fn initial_selected_light_name_is_the_empty_string() {
        assert_eq!(INITIAL_SELECTED_LIGHT_NAME, "");
    }

    #[test]
    fn initial_skip_gizmos_is_false() {
        assert!(!INITIAL_SKIP_GIZMOS);
    }

    // -- from_json defaults --

    #[test]
    fn json_defaults_pin_every_scalar_field() {
        let d = point_light_json_defaults();
        assert_eq!(d.attenuation_radius, 500.0);
        assert_eq!(d.point_radius, 5.0);
        assert_eq!(d.spot_falloff_cosine, 1.0);
        assert_eq!(d.spot_max_cosine, 1.0);
        assert_eq!(d.shadow_offset, 5.0);
        assert_eq!(d.attenuation_exponent, 1.0);
        assert_eq!(d.flicker_intensity, 0.0);
        assert_eq!(d.group_bits, 1);
    }

    #[test]
    fn json_defaults_pin_every_vector_field() {
        let d = point_light_json_defaults();
        assert_eq!(d.position, Vec3::new(0.0, 0.0, 0.0));
        assert_eq!(d.direction, Vec3::new(0.0, -10.0, 0.0));
        assert_eq!(d.diffuse_color, Vec3::new(1.0, 1.0, 1.0));
        assert_eq!(d.specular_color, Vec3::new(0.5, 0.5, 0.5));
    }

    #[test]
    fn json_default_direction_is_length_ten_not_a_unit_vector() {
        // (0, -10, 0) -> sqrt(0 + 100 + 0) = 10.
        let d = point_light_json_defaults();
        assert_eq!(light_vector_length(d.direction), 10.0);
    }

    // -- New##light descriptor --

    #[test]
    fn new_light_descriptor_pins_every_constant_field() {
        let (d, enabled) = new_light_descriptor(Vec3::new(0.0, 0.0, 0.0), Vec3::new(0.0, 0.0, 0.0));
        assert_eq!(d.direction, Vec3::new(0.0, -1.0, 0.0));
        assert_eq!(d.diffuse_color, Vec3::new(1.0, 1.0, 1.0));
        assert_eq!(d.attenuation_radius, 500.0);
        assert_eq!(d.point_radius, 15.0);
        assert_eq!(d.spot_falloff_cosine, 1.0);
        assert_eq!(d.spot_max_cosine, 1.0);
        assert_eq!(d.specular_color, Vec3::new(0.5, 0.5, 0.5));
        assert_eq!(d.shadow_offset, 5.0);
        assert_eq!(d.attenuation_exponent, 1.0);
        assert_eq!(d.flicker_intensity, 0.0);
        assert_eq!(d.group_bits, 1);
        assert!(enabled);
    }

    #[test]
    fn new_light_position_places_the_light_behind_the_view_direction() {
        // viewPos (10, 20, 30) + viewDir (0, 0, 1) * -150 = (10, 20, -120).
        let (d, _) = new_light_descriptor(Vec3::new(10.0, 20.0, 30.0), Vec3::new(0.0, 0.0, 1.0));
        assert_eq!(d.position, Vec3::new(10.0, 20.0, -120.0));
    }

    #[test]
    fn new_light_position_with_a_zero_view_direction_is_the_view_origin() {
        // (1, 2, 3) + (0,0,0) * -150 = (1, 2, 3).
        let (d, _) = new_light_descriptor(Vec3::new(1.0, 2.0, 3.0), Vec3::new(0.0, 0.0, 0.0));
        assert_eq!(d.position, Vec3::new(1.0, 2.0, 3.0));
    }

    #[test]
    fn new_light_position_scales_each_component_independently() {
        // (0,0,0) + (1, -2, 0.5) * -150 = (-150, 300, -75).
        let (d, _) = new_light_descriptor(Vec3::new(0.0, 0.0, 0.0), Vec3::new(1.0, -2.0, 0.5));
        assert_eq!(d.position, Vec3::new(-150.0, 300.0, -75.0));
    }

    #[test]
    fn distance_from_view_is_negative_one_hundred_fifty() {
        assert_eq!(NEW_LIGHT_DISTANCE_FROM_VIEW, -150.0);
    }

    #[test]
    fn the_two_default_paths_disagree_on_direction_and_point_radius() {
        // Pins the 10x direction-length and 3x point-radius divergence
        // between from_json and the New button, called out in the module doc.
        let j = point_light_json_defaults();
        let (b, _) = new_light_descriptor(Vec3::new(0.0, 0.0, 0.0), Vec3::new(0.0, 0.0, 0.0));
        assert_eq!(j.direction, Vec3::new(0.0, -10.0, 0.0));
        assert_eq!(b.direction, Vec3::new(0.0, -1.0, 0.0));
        assert_ne!(j.direction, b.direction);
        assert_eq!(j.point_radius, 5.0);
        assert_eq!(b.point_radius, 15.0);
        assert_ne!(j.point_radius, b.point_radius);
    }

    #[test]
    fn the_two_default_paths_agree_on_every_other_field() {
        let j = point_light_json_defaults();
        let (b, _) = new_light_descriptor(Vec3::new(0.0, 0.0, 0.0), Vec3::new(0.0, 0.0, 0.0));
        assert_eq!(j.position, b.position);
        assert_eq!(j.diffuse_color, b.diffuse_color);
        assert_eq!(j.attenuation_radius, b.attenuation_radius);
        assert_eq!(j.spot_falloff_cosine, b.spot_falloff_cosine);
        assert_eq!(j.spot_max_cosine, b.spot_max_cosine);
        assert_eq!(j.specular_color, b.specular_color);
        assert_eq!(j.shadow_offset, b.shadow_offset);
        assert_eq!(j.attenuation_exponent, b.attenuation_exponent);
        assert_eq!(j.flicker_intensity, b.flicker_intensity);
        assert_eq!(j.group_bits, b.group_bits);
    }

    // -- clamp_spot_falloff_cosine --

    #[test]
    fn clamp_returns_the_max_cosine_when_it_is_strictly_smaller() {
        assert_eq!(clamp_spot_falloff_cosine(0.9, 0.5), 0.5);
    }

    #[test]
    fn clamp_leaves_the_falloff_alone_when_it_is_already_smaller() {
        assert_eq!(clamp_spot_falloff_cosine(0.2, 0.8), 0.2);
    }

    #[test]
    fn clamp_on_an_exact_tie_returns_the_falloff_first_argument() {
        // std::min's `b < a` is false on a tie, so `a` is returned. The two
        // are numerically equal here, but the branch taken is pinned by the
        // signed-zero test below.
        assert_eq!(clamp_spot_falloff_cosine(0.5, 0.5), 0.5);
    }

    #[test]
    fn clamp_on_a_signed_zero_tie_returns_the_first_argument_bitwise() {
        // -0.0 < 0.0 is false, so std::min(0.0, -0.0) returns the FIRST
        // argument, +0.0. A `f32::min` would be free to return either.
        let r = clamp_spot_falloff_cosine(0.0, -0.0);
        assert!(
            r.is_sign_positive(),
            "expected +0.0, got a negatively signed zero"
        );
        assert_eq!(r, 0.0);
    }

    #[test]
    fn clamp_on_the_mirrored_signed_zero_tie_also_returns_the_first() {
        // 0.0 < -0.0 is false, so the first argument -0.0 is returned.
        let r = clamp_spot_falloff_cosine(-0.0, 0.0);
        assert!(
            r.is_sign_negative(),
            "expected -0.0, got a positively signed zero"
        );
        assert_eq!(r, 0.0);
    }

    #[test]
    fn clamp_with_a_nan_falloff_returns_nan_unlike_f32_min() {
        // `max < NaN` is false, so std::min returns its first argument, NaN.
        // f32::min would have returned 1.0 here.
        assert!(clamp_spot_falloff_cosine(f32::NAN, 1.0).is_nan());
        assert!(!f32::NAN.min(1.0).is_nan(), "sanity: f32::min differs here");
    }

    #[test]
    fn clamp_with_a_nan_max_returns_the_falloff_unchanged() {
        // `NaN < 0.3` is false, so the first argument 0.3 is returned.
        assert_eq!(clamp_spot_falloff_cosine(0.3, f32::NAN), 0.3);
    }

    #[test]
    fn clamp_handles_the_negative_one_to_one_cosine_endpoints() {
        assert_eq!(clamp_spot_falloff_cosine(1.0, -1.0), -1.0);
        assert_eq!(clamp_spot_falloff_cosine(-1.0, 1.0), -1.0);
    }

    // -- light_vector_length / normalize_light_color --

    #[test]
    fn length_of_a_three_four_zero_vector_is_five() {
        assert_eq!(light_vector_length(Vec3::new(3.0, 4.0, 0.0)), 5.0);
    }

    #[test]
    fn length_of_the_zero_vector_is_zero() {
        assert_eq!(light_vector_length(Vec3::new(0.0, 0.0, 0.0)), 0.0);
    }

    #[test]
    fn length_ignores_component_signs() {
        // Each component is squared before summing.
        assert_eq!(light_vector_length(Vec3::new(-3.0, 0.0, -4.0)), 5.0);
    }

    #[test]
    fn normalizing_a_three_four_zero_color_divides_by_five() {
        let n = normalize_light_color(Vec3::new(3.0, 4.0, 0.0));
        assert_eq!(n.x, 0.6);
        assert_eq!(n.y, 0.8);
        assert_eq!(n.z, 0.0);
    }

    #[test]
    fn normalizing_a_black_color_yields_three_nans_no_zero_guard() {
        // 0/0 is NaN in IEEE-754 and there is no guard in the original.
        let n = normalize_light_color(Vec3::new(0.0, 0.0, 0.0));
        assert!(n.x.is_nan());
        assert!(n.y.is_nan());
        assert!(n.z.is_nan());
    }

    #[test]
    fn normalizing_a_single_channel_color_yields_a_unit_axis() {
        // length((0,0,5)) = 5; 0/5 = +0.0, 5/5 = 1.0.
        let n = normalize_light_color(Vec3::new(0.0, 0.0, 5.0));
        assert_eq!(n, Vec3::new(0.0, 0.0, 1.0));
        assert!(n.x.is_sign_positive());
    }

    #[test]
    fn normalizing_preserves_a_negative_zero_component_sign() {
        // -0.0 / 5.0 = -0.0, not +0.0.
        let n = normalize_light_color(Vec3::new(-0.0, 0.0, 5.0));
        assert!(
            n.x.is_sign_negative(),
            "expected -0.0 to survive the divide"
        );
        assert_eq!(n.x, 0.0);
    }

    #[test]
    fn normalizing_an_already_unit_color_is_the_identity() {
        let n = normalize_light_color(Vec3::new(1.0, 0.0, 0.0));
        assert_eq!(n, Vec3::new(1.0, 0.0, 0.0));
    }

    #[test]
    fn normalizing_white_divides_by_the_root_of_three() {
        // length((1,1,1)) = sqrt(3) ~= 1.7320508; 1/sqrt(3) ~= 0.5773503.
        let n = normalize_light_color(Vec3::new(1.0, 1.0, 1.0));
        assert!((n.x - 0.577_350_3).abs() < 1e-6);
        assert!((n.y - 0.577_350_3).abs() < 1e-6);
        assert!((n.z - 0.577_350_3).abs() < 1e-6);
    }

    // -- light_selected --

    #[test]
    fn light_selected_requires_both_halves_of_the_and() {
        let names = taken(&["l0"]);
        assert!(light_selected("p0", "p0", "l0", &names));
    }

    #[test]
    fn light_selected_is_false_when_only_the_preset_name_matches() {
        let names = taken(&["l0"]);
        assert!(!light_selected("p0", "p0", "missing", &names));
    }

    #[test]
    fn light_selected_is_false_when_only_the_light_exists() {
        let names = taken(&["l0"]);
        assert!(!light_selected("p0", "p1", "l0", &names));
    }

    #[test]
    fn light_selected_is_false_when_neither_half_holds() {
        let names = taken(&["l0"]);
        assert!(!light_selected("p0", "p1", "missing", &names));
    }

    #[test]
    fn light_selected_with_an_empty_light_name_needs_an_empty_key_present() {
        // The original does not special-case an empty selectedLightName here;
        // it is a plain map lookup, so "" must literally be a key.
        assert!(!light_selected("p0", "p0", "", &taken(&["l0"])));
        assert!(light_selected("p0", "p0", "", &taken(&[""])));
    }

    // -- take_skip_gizmos --

    #[test]
    fn gizmos_draw_when_the_skip_flag_is_clear() {
        let mut skip = false;
        assert!(take_skip_gizmos(&mut skip));
        assert!(!skip, "a clear flag stays clear");
    }

    #[test]
    fn a_set_skip_flag_suppresses_exactly_one_frame_and_clears_itself() {
        let mut skip = true;
        assert!(
            !take_skip_gizmos(&mut skip),
            "the flagged frame is suppressed"
        );
        assert!(!skip, "the flag is cleared by the suppressed frame");
        assert!(take_skip_gizmos(&mut skip), "the next frame draws again");
    }

    // -- cone predicates --

    #[test]
    fn no_spot_cone_when_both_cosines_are_exactly_one() {
        // Strict `<` on both halves of the ||; 1.0 < 1.0 is false. This is
        // the default state from both entry points.
        assert!(!should_draw_spot_cone(1.0, 1.0));
    }

    #[test]
    fn spot_cone_draws_when_only_the_falloff_is_below_one() {
        assert!(should_draw_spot_cone(0.999_999, 1.0));
    }

    #[test]
    fn spot_cone_draws_when_only_the_max_is_below_one() {
        assert!(should_draw_spot_cone(1.0, 0.999_999));
    }

    #[test]
    fn spot_cone_does_not_draw_for_cosines_above_one() {
        // 2.0 < 1.0 is false on both halves.
        assert!(!should_draw_spot_cone(2.0, 2.0));
    }

    #[test]
    fn spot_cone_gate_is_false_for_nan_cosines() {
        // Every NaN comparison is false, so neither half of the || fires.
        assert!(!should_draw_spot_cone(f32::NAN, f32::NAN));
        assert!(!should_draw_spot_cone(f32::NAN, 1.0));
        assert!(
            should_draw_spot_cone(f32::NAN, 0.5),
            "the non-NaN half still fires"
        );
    }

    #[test]
    fn max_cone_boundary_is_strict_at_zero() {
        assert!(!should_draw_max_cone(0.0));
        assert!(!should_draw_max_cone(-0.0), "-0.0 < 0.0 is false");
        assert!(should_draw_max_cone(-1e-7));
        assert!(!should_draw_max_cone(1e-7));
    }

    #[test]
    fn falloff_cone_boundary_is_strict_at_zero() {
        assert!(!should_draw_falloff_cone(0.0));
        assert!(!should_draw_falloff_cone(-0.0));
        assert!(should_draw_falloff_cone(-1.0));
        assert!(!should_draw_falloff_cone(1.0));
    }

    #[test]
    fn a_cosine_between_zero_and_one_passes_the_outer_gate_but_draws_no_cone() {
        // The documented gap: the outer gate is `< 1.0` but the two inner
        // gates are `< 0.0`, so 0.5 draws the marker and no circles.
        assert!(should_draw_spot_cone(0.5, 0.5));
        assert!(!should_draw_max_cone(0.5));
        assert!(!should_draw_falloff_cone(0.5));
    }

    // -- spot_cone_radius --

    #[test]
    fn spot_cone_radius_is_zero_when_the_negated_cosine_is_one() {
        // acos(-(-1.0)) = acos(1.0) = 0 exactly, and tan(0) = 0 exactly.
        assert_eq!(spot_cone_radius(Vec3::new(0.0, -1.0, 0.0), -1.0), 0.0);
    }

    #[test]
    fn spot_cone_radius_scales_linearly_with_the_direction_length() {
        // Same cosine, direction length 1 vs 3 -> radius exactly 3x.
        let a = spot_cone_radius(Vec3::new(0.0, -1.0, 0.0), -0.5);
        let b = spot_cone_radius(Vec3::new(0.0, -3.0, 0.0), -0.5);
        assert!((b - a * 3.0).abs() < 1e-4, "a={a}, b={b}");
    }

    #[test]
    fn spot_cone_radius_for_a_negated_cosine_of_one_half() {
        // cosine = -0.5 -> acos(0.5) = pi/3 -> tan(pi/3) = sqrt(3)
        // ~= 1.7320508. Direction length 1.
        let r = spot_cone_radius(Vec3::new(0.0, -1.0, 0.0), -0.5);
        assert!((r - 1.732_050_8).abs() < 1e-4, "got {r}");
    }

    #[test]
    fn spot_cone_radius_for_a_negated_cosine_of_minus_one_half() {
        // cosine = 0.5 -> acos(-0.5) = 2pi/3 -> tan(2pi/3) = -sqrt(3).
        // Negative: the sign is not guarded anywhere.
        let r = spot_cone_radius(Vec3::new(0.0, -1.0, 0.0), 0.5);
        assert!((r + 1.732_050_8).abs() < 1e-4, "got {r}");
    }

    #[test]
    fn spot_cone_radius_near_the_tangent_pole_is_large_but_finite() {
        // cosine = 0 -> acos(-0.0) = pi/2, which is not exactly
        // representable, so tan is a huge finite float rather than infinity.
        let r = spot_cone_radius(Vec3::new(0.0, -1.0, 0.0), 0.0);
        assert!(r.is_finite(), "expected a finite value, got {r}");
        assert!(r.abs() > 1e6, "expected a near-pole magnitude, got {r}");
    }

    #[test]
    fn spot_cone_radius_is_nan_for_a_cosine_outside_the_valid_range() {
        // acos(-2.0) and acos(2.0) are both NaN; NaN propagates through tan
        // and the multiply.
        assert!(spot_cone_radius(Vec3::new(0.0, -1.0, 0.0), 2.0).is_nan());
        assert!(spot_cone_radius(Vec3::new(0.0, -1.0, 0.0), -2.0).is_nan());
    }

    #[test]
    fn spot_cone_radius_is_zero_for_a_zero_length_direction() {
        // 0 * tan(acos(0.5)) = 0. Not NaN: the tan factor is finite here.
        assert_eq!(spot_cone_radius(Vec3::new(0.0, 0.0, 0.0), -0.5), 0.0);
    }

    // -- spot_cone_circles --

    #[test]
    fn segment_mult_is_three_over_twenty() {
        assert_eq!(CONE_SEGMENTS, 20);
        assert_eq!(CONE_SEGMENT_MULT, 0.15);
    }

    #[test]
    fn cone_emits_exactly_twenty_circles() {
        let c = spot_cone_circles(Vec3::new(0.0, 0.0, 0.0), Vec3::new(0.0, 1.0, 0.0), 1.0);
        assert_eq!(c.len(), 20);
    }

    #[test]
    fn the_first_cone_circle_is_degenerate_at_the_light_position() {
        // i = 0 -> iF = 0, so both the offset and the radius are 0.
        let c = spot_cone_circles(Vec3::new(7.0, 8.0, 9.0), Vec3::new(0.0, 1.0, 0.0), 4.0);
        assert_eq!(c[0].0, Vec3::new(7.0, 8.0, 9.0));
        assert_eq!(c[0].1, 0.0);
    }

    #[test]
    fn the_second_cone_circle_is_one_segment_mult_along_the_direction() {
        // i = 1 -> center = pos + dir * 0.15 * 1, radius = 4 * 0.15 * 1 = 0.6.
        let c = spot_cone_circles(Vec3::new(0.0, 0.0, 0.0), Vec3::new(0.0, 1.0, 0.0), 4.0);
        assert_eq!(c[1].0, Vec3::new(0.0, 0.15, 0.0));
        assert!((c[1].1 - 0.6).abs() < 1e-6, "got {}", c[1].1);
    }

    #[test]
    fn the_last_cone_circle_stops_short_of_three_segment_units() {
        // i = 19 -> 0.15 * 19 = 2.85, not 3.0. The loop is `i < Segments`.
        let c = spot_cone_circles(Vec3::new(0.0, 0.0, 0.0), Vec3::new(1.0, 0.0, 0.0), 1.0);
        assert!((c[19].0.x - 2.85).abs() < 1e-5, "got {}", c[19].0.x);
        assert!((c[19].1 - 2.85).abs() < 1e-5, "got {}", c[19].1);
    }

    #[test]
    fn cone_circle_centers_offset_every_component_of_the_direction() {
        // i = 2, dir (2, -4, 6): offset = dir * 0.15 * 2 = (0.6, -1.2, 1.8),
        // added to pos (1, 1, 1) -> (1.6, -0.2, 2.8).
        let c = spot_cone_circles(Vec3::new(1.0, 1.0, 1.0), Vec3::new(2.0, -4.0, 6.0), 0.0);
        assert!((c[2].0.x - 1.6).abs() < 1e-5, "x = {}", c[2].0.x);
        assert!((c[2].0.y + 0.2).abs() < 1e-5, "y = {}", c[2].0.y);
        assert!((c[2].0.z - 2.8).abs() < 1e-5, "z = {}", c[2].0.z);
    }

    #[test]
    fn a_negative_spot_radius_yields_negative_circle_radii_unguarded() {
        // The 2pi/3 case above produces a negative spot radius; nothing
        // clamps it, so the emitted circle radii are negative too.
        let c = spot_cone_circles(Vec3::new(0.0, 0.0, 0.0), Vec3::new(0.0, 1.0, 0.0), -2.0);
        assert!((c[10].1 + 3.0).abs() < 1e-5, "got {}", c[10].1);
    }

    #[test]
    fn a_nan_spot_radius_propagates_to_every_nonzero_circle() {
        // i = 0 multiplies NaN by 0.0, which is still NaN in IEEE-754.
        let c = spot_cone_circles(Vec3::new(0.0, 0.0, 0.0), Vec3::new(0.0, 1.0, 0.0), f32::NAN);
        assert!(c[0].1.is_nan(), "NaN * 0.0 is NaN, not 0.0");
        assert!(c[19].1.is_nan());
    }
}
