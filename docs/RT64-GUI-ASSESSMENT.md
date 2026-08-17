# RT64 GUI and ImGui-backend port assessment

Ten files under RT64's `src/gui/` and `src/imgui/` — 2,961 lines by `wc -l`,
2,288 excluding the two files' trailing-newline accounting used in earlier
prose — assessed for portability into `fn64-render-wgpu`. **All ten are
refused.** Nothing was ported, and no Rust module was created.

This document exists so the refusal is citable. An earlier pass reached the
same conclusion but left no commit, so a later inventory card searched the
branch, found no assessing commit, and correctly declined to record the
`refused` state. The `refused` state requires a verifiable assessing commit;
this file plus its commit is that evidence.

Sources read from the **port-source pin**
`5473732a822a4423b5696e7cb18fecc425a59875`
([`rt64-port-authority.json`](rt64-port-authority.json)). All ten files are
`port_delta: unchanged` — byte-identical to the executable-oracle pin
`f0728a2520d5aa735886240de3fee75cc805f6d6` — so the citation is unambiguous
against either pin. Every file's digest was recomputed with `shasum -a 256`
against the pinned checkout and cross-checked against
`docs/rt64-port-inventory.json`'s `sources.port.sha256` and
`sources.oracle.sha256`; all twenty comparisons match. See
[Source identity](#source-identity) for why the digests are cited by
reference rather than restated here.

## The criterion

The standing port criterion applies unchanged: *a construct is ported when its
behavior is fully determined by values and control flow present in the cited
file — no GPU, no ImGui context, no type from an uncited file.*

Two independent disqualifiers do the work here, and it is worth separating
them because they fail differently:

- **ImGui selects control flow, not merely a value.** Where a branch condition
  is `ImGui::IsMouseDown(...)`, hoisting the input to a parameter does not
  leave a portable remainder behind; the surviving skeleton is the ImGui
  interaction model itself.
- **The observable effect is a mutation of an uncited type.** A function whose
  entire result is writing fields of a struct declared in a file this card does
  not cite is not fully determined by the cited file, regardless of how much
  arithmetic its body contains.

## Per-file decisions

| File | Lines | Decision and evidence |
|---|---|---|
| `src/gui/rt64_debugger_inspector.cpp` | 1714 | Refused. 532 `ImGui::` call sites; 1,507 non-blank non-comment lines. Its math calls — `pseudoRandom`, `barycentricCoordinates`, `nearPlaneFromProj`, `farPlaneFromProj`, `fovFromProj` — are all *defined in `common/rt64_math.cpp`*, not here, and belong to that file's card. `barycentricCoordinates` is already ported at `rt64_math.rs:225`. |
| `src/gui/rt64_debugger_inspector.h` | 41 | Refused. A 10-field struct plus 8 method declarations. Field layout only (§3.7 of the standing brief: declaration order is not pinnable in safe Rust); `Workload`, `VI`, `DrawCallKey`, `RenderWindow` all from uncited files. |
| `src/gui/rt64_inspector.cpp` | 273 | Refused. Vulkan and D3D12 backend plumbing — `VkDescriptorPool`, `VkRenderPass`, `vkDestroyRenderPass`, `ImGui_ImplVulkan_Init`, `imgui_impl_dx12`, `imgui_impl_win32`. fn64 targets wgpu; none of these APIs exist in the render crates. |
| `src/gui/rt64_inspector.h` | 43 | Refused. Declarations for the above, over Plume/Vulkan/SDL2 handle types. |
| `src/gui/rt64_file_dialog.cpp` | 78 | Refused. A thin wrapper over the `nfd` (Native File Dialog) library: `NFD_Init`, `NFD_PickFolderN`, `NFD_OpenDialogN`, `NFD_SaveDialogN`, `NFD_FreePathN`. Zero arithmetic in the file. |
| `src/gui/rt64_file_dialog.h` | 46 | Refused. A two-`std::string` filter struct and five static declarations returning `std::filesystem::path`. |
| `src/gui/rt64_camera_controller.cpp` | 75 | Refused — see the dedicated section below; its reasoning was re-derived rather than inherited. |
| `src/gui/rt64_camera_controller.h` | 18 | Refused. One `hlslpp::int2` field and four method declarations, three of which take `DebuggerCamera&` from the uncited `hle/rt64_workload.h`. |
| `src/imgui/imgui_impl_sdl2_custom.cpp` | 630 | Refused. Vendored Dear ImGui SDL2 platform backend, carrying upstream's own header and changelog. 126 `SDL_` references. **The workspace has no SDL2 dependency at all** — `grep` for `sdl2` across every `Cargo.toml` returns nothing. |
| `src/imgui/imgui_impl_sdl2_custom.h` | 43 | Refused. The backend's `IMGUI_IMPL_API` declarations and its `ImGui_ImplSDL2_GamepadMode` enum. |

### Source identity

The whole-file digests identifying the exact upstream bytes read are recorded
in `docs/rt64-port-inventory.json` under each path's `sources.port.sha256` and
`sources.oracle.sha256`. Each was independently recomputed with
`shasum -a 256` against the pinned checkout during this assessment and matched
the inventory on all twenty comparisons. They are deliberately not restated
here: no test re-checks them, this card ported nothing that could gate them,
and the inventory is already their single source of truth.

## `rt64_camera_controller`: re-judged on current facts

This file is the one where the earlier refusal's stated reason has since
expired, so it was re-derived from scratch rather than ratified.

**The expired reason.** The earlier pass rested partly on "all three matrix
methods terminate in `hlslpp::inverse`, and fn64's `inverse4` is private." That
argument is **gone**. `inverse4` was since widened to `pub(crate)` at
`rt64_math_decompose.rs:769`, and `matrix_translation`, `matrix_rotation_y` and
`matrix_rotation_z` all landed at `rt64_math_matrix.rs:572`, `:609` and `:626`.
Every dependency the old argument named is now available. **That reason must
not be repeated.**

**The reason that survives, for `moveCursor`.** ImGui here selects control
flow, not a value. `ImGui::IsMouseDown(ImGuiMouseButton_Middle)` gates the
outer branch; `ImGui::GetIO().WantCaptureMouse` gates entry; and
`IsKeyDown(ImGuiKey_LeftCtrl)` / `IsKeyDown(ImGuiKey_LeftAlt)` select which of
three different transforms runs. Hoisting the inputs to parameters leaves
behind only the ImGui interaction model — a dispatch over modifier keys — with
the arithmetic being three one-line calls into helpers fn64 already owns.

**The reason the ImGui argument does not by itself cover the rest.** This is
the correction worth recording. `lookAtPerspective` (lines 65-75) contains
**zero ImGui references**: it is pure vector math — two `cross` products, three
`normalize` calls, and four row writes. `movePerspective` and
`rotatePerspective` likewise reference no ImGui symbol. So an
ImGui-control-flow argument alone would *not* carry the refusal for three of
the four methods, and stating it as though it did would be the same class of
error the expired reason was.

**What actually carries it: the uncited mutated type.** All four methods take
`DebuggerCamera &camera` and produce no return value. Their entire observable
effect is writing `camera.viewMatrix` and `camera.invViewMatrix`.
`DebuggerCamera` is an 8-member struct declared at `hle/rt64_workload.h:193-202`
— a file this card does not cite, and one a prior card already refused by name:
`rt64_workload_geometry.rs:241` lists `DebuggerCamera` among the structures
refused as field layout with `hlslpp` types from uncited files. A function
whose whole result is mutating a foreign, unported struct is not fully
determined by the cited file. Porting it would mean first inventing a
`DebuggerCamera` this card has no authority to define.

**No consumer, either way.** `CameraController` has exactly two live references
in all of RT64 — `rt64_state.h:111` (the member) and `rt64_state.cpp:2015`
(the single `moveCursor` call) — and drives only RT64's own debugger window.
A grep for `lookAtPerspective` turns up an apparent third caller at
`preset/rt64_preset_light.cpp:104`, but it is **inside a `/* */` comment
block** spanning lines 99-107; the `projectionInspector` references in
`rt64_state.cpp:2030-2034` are `//`-commented as well. `lookAtPerspective`,
`movePerspective` and `rotatePerspective` therefore have **zero live callers
anywhere in RT64**. The two-reference count is correct for live code, and the
commented call is noted here only so a future grep does not read it as a
contradiction.

## What was checked and found already owned

- `barycentricCoordinates` — ported, `rt64_math.rs:225`. Called from
  `rt64_debugger_inspector.cpp:1666` but defined in `common/rt64_math.cpp:218`.
- `matrix_translation`, `matrix_rotation_y`, `matrix_rotation_z` — ported,
  `rt64_math_matrix.rs:572`, `:609`, `:626`.
- `inverse4` — ported and `pub(crate)`, `rt64_math_decompose.rs:769`.
- `pseudoRandom` (`common/rt64_math.cpp:213-216`) — **not** ported, and not
  this card's to port: it is defined in `common/rt64_math.cpp`, and the
  inspector only calls it at lines 434 and 438.
- `DebuggerCamera` — refused by a prior card, `rt64_workload_geometry.rs:241`.

## Inventory drift disclosure

All ten files are **cited-but-not-ported**. Zero lines of the 2,961 were
ported. Because port state is derived at file granularity, citing these ten
files at all would otherwise read as ten ported files; this disclosure is the
correction, and it is mandatory for exactly that reason.

`docs/rt64-port-inventory.json` was **not** regenerated — a concurrent lane
owns that file, and regeneration is a batch operation that rewrites it
wholesale from a tree snapshot.

## Nonclaims

No Rust module was created and no `mod` line was added. No behavior changed;
the workspace test count is identical before and after. No `repr(C)`, size,
alignment or ABI claim is made about any struct named here. No claim is made
about field declaration order. This document asserts portability decisions
only — it does not assert that RT64's GUI behavior has been reproduced
anywhere, because it has not been.
