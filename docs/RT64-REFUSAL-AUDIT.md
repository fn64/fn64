# RT64 refusal audit

An audit of all 83 non-`ported` sources in
[`rt64-port-inventory.json`](rt64-port-inventory.json) — the 73 `refused` and
the 10 `authority-gated` — against the **current** crediting rule and the
**current** state of the tree. This document ports nothing and changes no
inventory state; it is a findings artifact.

Assessed at `53525d0e`. C++/HLSL read from the port-source pin
`5473732a822a4423b5696e7cb18fecc425a59875`, oracle
`f0728a2520d5aa735886240de3fee75cc805f6d6`; both checkouts clean and at the
declared pins. `python3 tools/rt64_port_inventory.py --check` reports
`rt64-port-inventory: clean`, so every refusal's 40-hex subject-matched commit
and evidence-path existence validate as the schema requires.

## Why this audit exists

`tools/rt64_port_inventory.py`'s `port_state_for` (`:855-892`) was reordered so
digest evidence outranks the authority gate:

```python
if ported_as:
    return "ported"
if gated:
    return "authority-gated"
return "refused" if refusal else "not-started"
```

Two classes of refusal reason died with that change, and a third had already
rotted:

1. **Metric-invisibility** — "a port here would not be credited". Dead.
2. **Gate-as-veto** — "its `.cpp` is authority-gated, so it is not ours to
   port". The generated inventory now states the opposite in terms:
   `authority-gated` is "a source-overlay constraint, never completion
   evidence -- and never a veto".
3. **"Uncited dependency"** — a reason naming a header as uncited when the
   inventory now reads it `ported`.

## Reason classes

- **STILL VALID** — fn64 owns the same arithmetic by another route (with an
  owner `file:line`), or the content is genuinely out of scope.
- **EXPIRED** — the reason rests on a fact that has since changed.
- **UNSUPPORTED** — a set-level claim with no per-file evidence, or a cited
  evidence path that does not substantiate this specific file.

A refusal can carry an unsupported *citation* while its *conclusion* survives
on the source itself. Those are recorded as STILL VALID with the citation
defect noted, because the conclusion is what the inventory asserts.

## Headline counts

| Class | Refused (73) | Gated (10) | Total (83) |
|---|---|---|---|
| STILL VALID | 48 | 3 | **51** |
| EXPIRED | 17 | 7 | **24** |
| UNSUPPORTED | 8 | 0 | **8** |

**Genuinely portable now: 24 of 83 files, ~795 real lines.** That is roughly
1.7% of the 48,065-line denominator — the 38.2% non-ported share is mostly, but
not entirely, legitimately refused.

### The citation defect, measured

Independently of reason class, **46 of 73 refusals cite an evidence file that
never names the file at all** (searched for full path, basename, and stem):

| Evidence file | Substantiated |
|---|---|
| `crates/fn64-render-wgpu/src/rt64_render_pipeline_types.rs` | 2 / 23 |
| `crates/fn64-render/src/settings.rs` | 0 / 16 |
| `crates/fn64-render-wgpu/src/rt64_vi_registers.rs` | 1 / 9 |
| `docs/RT64-GUI-ASSESSMENT.md` | 10 / 10 |
| `docs/RT64-M6-M7-SCOPING.md` | 5 / 5 |
| `crates/fn64-render-wgpu/src/rt64_hle_geometry.rs` | 5 / 5 |
| `crates/fn64-render-wgpu/src/rt64_workload_geometry.rs` | 2 / 2 |
| `crates/fn64-render-wgpu/src/rt64_frame_compatibility.rs` | 1 / 1 |
| `crates/fn64-render-wgpu/src/rt64_postprocess.rs` | 1 / 1 |
| `crates/fn64-render-wgpu/src/rt64_extended_gbi.rs` | 0 / 1 |

Three batch citations account for 43 of the 46. `settings.rs` scopes itself
(`:1-70`) to `rt64_user_configuration`, `rt64_enhancement_configuration` and
`rt64_emulator_configuration` — all already `ported` — and names none of the 16
`src/common` files it is cited for. The GUI, M6-M7 and HLE-geometry batches are
fully substantiated; they are the model.

### The expiry patterns, measured

A sweep of every `uncited` claim in `crates/**/*.rs` against current inventory
state — **5 of 6 name a file that now reads `ported`**:

| Claim site | Names | State |
|---|---|---|
| `crates/fn64-render-wgpu/src/rt64_hle_geometry.rs:584` | `src/common/rt64_common.h` | `ported` — EXPIRED |
| `crates/fn64-render-wgpu/src/rt64_gbi_opcodes.rs:387` | `src/shared/rt64_f3d_defines.h` | `ported` — EXPIRED |
| `crates/fn64-render-wgpu/src/rt64_gbi_opcodes.rs:771` | `src/shared/rt64_f3d_defines.h` | `ported` — EXPIRED |
| `crates/fn64-render-wgpu/src/rt64_vi_timing.rs:78` | `src/hle/rt64_vi.h` | `ported` — EXPIRED |
| `crates/fn64-render-wgpu/src/rt64_shared_params.rs:114` | `src/shared/rt64_hlsl.h` | `ported` — EXPIRED |
| `crates/fn64-render-wgpu/src/rt64_gbi_opcodes.rs:135` | `src/common/rt64_plume.h` | `refused` — holds |

A `ported` header does not imply every symbol in it is owned:
`src/common/rt64_common.h` reads `ported`, but `adjustVector` has no Rust owner
(`grep adjust_vector crates --include='*.rs'` returns nothing). The expiry is of
the *phrasing*, not automatically of the conclusion.

**Metric-invisibility, found verbatim in the tree** at
`crates/fn64-render-wgpu/src/rt64_workload_geometry.rs:37-38`: two files are
refused because "citing them would falsely credit 58 ported lines". Restated at
`:508-510`. Both conclusions survive on substance; both reasons must be
rewritten.

**Gate-as-veto, found verbatim** at
`crates/fn64-render-wgpu/src/rt64_vi_registers.rs:59`: "An authority-gated file
is not this card's to port." One instance in `crates/`; three more in the
inventory's own `src/render` reasons.

## Findings table

### `src/common` — 17 refused

All 17 cite `crates/fn64-render/src/settings.rs`, except `rt64_plume.h`
(`docs/RT64-M6-M7-SCOPING.md`, which does substantiate it). No file in this
batch expired: none invoked metric-invisibility, a now-`pub` symbol, or a
now-`ported` dependency. The defect here is insufficient reading, not the rule
change.

| Source | State | Class | Disposition |
|---|---|---|---|
| `rt64_dynamic_libraries.cpp` | refused | STILL VALID (citation unsupported) | Not portable. Body entirely `#if defined(_WIN32)`; the only non-syscall content is a three-entry Windows-DLL name table with no fn64 referent. |
| `rt64_dynamic_libraries.h` | refused | STILL VALID (citation unsupported) | Not portable. One struct, one `static bool load()` declaration. |
| `rt64_filesystem.h` | refused | **UNSUPPORTED** | Portable, ~6 lines. "Pure virtual declarations, no behavior" is false: `toForwardSlashes` (`:57-60`) is a byte-wise `'\\'`→`'/'` map and `load` (`:47-55`) carries a zero-size-is-failure predicate. Unowned in fn64. |
| `rt64_filesystem_directory.h` | refused | STILL VALID (citation unsupported) | Not portable. Every method bottoms out in `std::filesystem`/`std::ifstream`. |
| `rt64_filesystem_zip.cpp` | refused | **UNSUPPORTED** | Portable, ~20 lines. Five PKZIP wire constants (`:13-19`), the local-header offset arithmetic (`:140-147`), the 3-way compression-tag map with silent-skip default (`:83-96`). fn64 has no zip dependency at all. |
| `rt64_filesystem_zip.h` | refused | **UNSUPPORTED** | Portable, ~13 lines — the `Compression` enum and POD defaults (`:15-27`). Same card as the `.cpp`. |
| `rt64_hlslpp.h` | refused | STILL VALID (citation unsupported) | Not portable. Two `#if` guards defining `HLSLPP_SCALAR` plus an include. |
| `rt64_load_types.cpp` | refused | STILL VALID (citation unsupported) | Not portable. 38 of 60 lines are one-to-one field copies; verified no clamp, no enum map, no version check. fn64's ported `rt64_replacement_resolve.rs:16` already excludes JSON serde by policy. |
| `rt64_load_types.h` | refused | **UNSUPPORTED** | Portable, ~45 lines — the batch's biggest miss. Three `LoadOperation*` PODs (`:43-65`), the `Type` tagged-union discriminant (`:67-83`), `enum class LoadTLUT` (`:85-89`), two wire-string tables (`:91-101`). fn64's same-named types are *not* the same content: `rt64_tmem_hasher.rs:188-198` mirrors 3 of 15 fields by its own admission; `rt64_hle_geometry.rs:1059-1068` collapses `{Tile,Block,TLUT}` to `{Block,Tile,Other}`. |
| `rt64_mapped_file.cpp` | refused | STILL VALID (citation unsupported) | Not portable. `mmap`/`CreateFileMappingW`; no constants, no arithmetic. |
| `rt64_mapped_file.h` | refused | STILL VALID (citation unsupported) | Not portable. Platform handle fields with OS sentinels. |
| `rt64_plume.h` | refused | **STILL VALID** | Not portable. Five lines; the only refusal in this batch whose citation substantiates it (`docs/RT64-M6-M7-SCOPING.md:95-103`, `:367`). |
| `rt64_sommelier.h` | refused | STILL VALID (citation unsupported) | Not portable. Verified: `#if defined(_WIN32)` at `:7` closes at `:33` — the "compiles to nothing on macOS" claim is true as stated. |
| `rt64_thread.cpp` | refused | STILL VALID, **reason wrong** | Not portable, but restate: the reason names only thread naming and misses `toWindowsPriority` (`:19-37`), a 6-way enum map. That map is Win32-gated and its documented non-Windows behavior is an intentional no-op (`:58-66`). |
| `rt64_thread.h` | refused | STILL VALID, **reason wrong** | Not portable. The `Priority` enum (`:12-19`) is here, not in the `.cpp`; refusable because its only consumer is Win32-gated. |
| `rt64_user_paths.cpp` | refused | **UNSUPPORTED** | Portable, ~6 lines. Four RT64 on-disk filename constants (`:19-22`) plus `setupPaths`' empty-root guard (`:62-71`). `"rt64.json"` is currently a bare literal in 7 fn64 locations with no cited owner. |
| `rt64_user_paths.h` | refused | STILL VALID (citation unsupported) | Not portable alone; the struct shape rides along with the `.cpp` card. |

### `src/gui` and `src/imgui` — 10 refused

All 10 cite `docs/RT64-GUI-ASSESSMENT.md`, which names every one. This is the
best-substantiated batch in the inventory — and it still contains the audit's
cleanest expiry.

| Source | State | Class | Disposition |
|---|---|---|---|
| `rt64_camera_controller.cpp` | refused | **EXPIRED** | Portable, ~28 lines. The reason calls `hle/rt64_workload.h` "uncited"; it is digest-cited at `rt64_workload_geometry.rs:33` (`e5902f19…`) and reads `ported`. The reason's own fallback ("all four methods mutate `DebuggerCamera&`") is true but no longer load-bearing, and every primitive is already `pub`: `matrix_translation`, `matrix_rotation_y`/`_z`, `extract_3x3`, `inverse4` (`rt64_math_matrix.rs:572`, `:609`, `:626`, `:395`, `:655`). `rt64_math_matrix.rs:585` states outright that RT64's names were kept *because* `rt64_camera_controller.cpp:53-63` calls them — fn64 ported the primitives for this file. `lookAtPerspective` (`:65-75`) has zero ImGui references. |
| `rt64_camera_controller.h` | refused | **EXPIRED** | Portable, ~6 lines (the four signatures) as part of the `.cpp` card. Same "uncited `rt64_workload.h`" defect. |
| `rt64_debugger_inspector.cpp` | refused | **STILL VALID** | Not portable. Verified 531 `ImGui::` call sites over 1,714 lines; its five math calls are defined in `common/rt64_math.cpp` and all four are `pub` in `rt64_math.rs` (`:192`, `:202`, `:211`, `:225`). |
| `rt64_debugger_inspector.h` | refused | **STILL VALID** | Not portable. Field layout plus 8 declarations. |
| `rt64_file_dialog.cpp` | refused | **STILL VALID** | Not portable. Thin nfd wrapper; verified zero arithmetic. |
| `rt64_file_dialog.h` | refused | **STILL VALID** | Not portable. Verified: the only body is `FileFilter`'s constructor, whose two branches differ solely by `win32::Utf8ToUtf16`. |
| `rt64_inspector.cpp` | refused | **STILL VALID** | Not portable. Vulkan/D3D12/Win32 ImGui backends; fn64 targets wgpu. |
| `rt64_inspector.h` | refused | **STILL VALID** | Not portable. Declarations over Plume/Vulkan/SDL2 handles. |
| `imgui_impl_sdl2_custom.cpp` | refused | **STILL VALID** | Not portable. Vendored Dear ImGui backend; 126 `SDL_` references and no SDL2 dependency in the workspace. |
| `imgui_impl_sdl2_custom.h` | refused | **STILL VALID** | Not portable. Nine `IMGUI_IMPL_API` declarations, no bodies. |

### `src/hle` — 18 refused

Nine cite `rt64_vi_registers.rs`, which names one of them; those nine carry an
unsupported citation independently of their reason class.

| Source | State | Class | Disposition |
|---|---|---|---|
| `rt64_application.h` | refused | **EXPIRED** | Portable, ~14 lines. "Config defaults plus enums" is a dismissal, not a refusal: `SetupResult` (`:46-52`, 5 values), `DeveloperShortcut` (`:54-59`, 4 values), 3 config defaults (`:38-43`). Unowned. `settings.rs:677-712` ports four tag enums of exactly this shape. |
| `rt64_application_window.cpp` | refused | STILL VALID (citation unsupported) | Not portable. Verified `:270` `* 2` for `DM_INTERLACED`, the `// FIXME` at `:276-278`, and `(refreshRate % 10) == 9` at `:279-281`. |
| `rt64_application_window.h` | refused | STILL VALID (citation unsupported) | Not portable. Pure virtuals plus sentinel defaults. |
| `rt64_command_warning.cpp` | refused | STILL VALID (citation unsupported) | Not portable. `format` is `va_start`/`vsnprintf`; it never sets the tag or the union. |
| `rt64_command_warning.h` | refused | **EXPIRED** | Portable, ~10 lines — the `IndexType` tag and its 3-arm union (`:11-34`) as a Rust tagged enum. Lowest value in the batch; defensibly re-refusable on the sharper ground that the `.cpp` never writes the tag, so the correspondence is unwitnessed. |
| `rt64_framebuffer_changes.h` | refused | **UNSUPPORTED** | Not portable, correct verdict, wrong reason. The reason lists only RHI members and omits that the header's one portable construct, `FramebufferChange::Type` (`:22-25`), is **already ported** at `rt64_hle_geometry.rs:665-672`. |
| `rt64_framebuffer_pair.h` | refused | **EXPIRED** | Portable, ~8 lines. The "six scalar fields" ownership claim is contradicted by its own evidence: `rt64_frame_compatibility.rs:280-298` states `formatChanged` "is not read by either predicate and is not carried here" — fn64 owns five, not six. `FlushReason` (`:12-19`, 6 values) is unowned and is the sole argument of `State::submitFramebufferPair`. |
| `rt64_game_call.h` | refused | STILL VALID, **reason wrong** | Not portable. Five members, not four as its own evidence says at `rt64_hle_geometry.rs:551-552`; and `DrawCall` is *not* owned (`rt64_hle_geometry.rs:40` records it cited-but-not-ported). Zero initializers, zero arithmetic. |
| `rt64_game_configuration.h` | refused | **EXPIRED** | Portable, ~8 lines. Five literal defaults (`1.5f`, `25000.0f`, `false`, `false`, `1.0f`) verified unowned — `grep 25000 crates --include='*.rs'` returns nothing. They are the missing calibration input to an already-landed port: `rt64_state.cpp:1764` feeds them to `estimatedSunLight`, which fn64 ports at `rt64_light_estimation.rs:292` and whose tests invent 1.0/100.0. |
| `rt64_interpreter.h` | refused | **EXPIRED** | Not portable, but the reason is dead. `rt64_workload_geometry.rs:37-38` refuses it because citing it "would falsely credit 58 ported lines" — metric-invisibility verbatim. Conclusion survives on "four zero-valued initializers, five bodiless declarations". |
| `rt64_microcode.h` | refused | **STILL VALID** | Not portable. Verified: two `uint32_t` fields and nothing else. Best-reasoned refusal in the batch (§3.7 field order is not pinnable). |
| `rt64_present.h` | refused | **EXPIRED** | Not portable, same dead reason (`rt64_workload_geometry.rs:37-38`, `:508-510`). Conclusion survives: all six defaults are zero or false. |
| `rt64_present_queue.h` | refused | STILL VALID (citation unsupported) | Not portable. Object graph. Minor overcount: two condition variables, not three. |
| `rt64_projection.h` | refused | **STILL VALID** | Not portable. The `Type` enum is already ported at `rt64_hle_geometry.rs:955-966` with `uses_viewport` at `:975-981`. |
| `rt64_rdp_tmem.h` | refused | **STILL VALID** | Not portable. Adjudicated at `rt64_hle_geometry.rs:585-596` (the reason's `:585-593` is a slightly short range). |
| `rt64_shared_queue_resources.h` | refused | STILL VALID (citation unsupported) | Not portable. Verified six bodies. Sharpen the reason: four of the five assignments are unconditional single-field writes; only `setSwapChainSize` (`:61-68`) has a two-field inequality gating a dirty flag. |
| `rt64_state.h` | refused | STILL VALID, **reason incomplete** | Not portable at header granularity, but "zero bodies" omits `RDRAMSize = 0x7FFFFF` (`:39`) and five `Extended` sentinel defaults (`:126-131`). Their meaning is the setter protocol in the authority-gated `.cpp`; tie the refusal to that, not to body count. |
| `rt64_transform_group.h` | refused | **STILL VALID** | Not portable. All six G_EX constants verified equal between `include/rt64_extended_gbi.h:94,96,98,104,106,113` and `rt64_extended_gbi.rs:742,744,746,752,754,761`. Nit: fourteen fields, of which twelve are G_EX constants plus `bool decompose = true` — the "thirteen, every one a G_EX constant" phrasing is off by one. |

### `src/render` — 28 refused

Twenty-three cite `rt64_render_pipeline_types.rs`, which names two of them.

| Source | State | Class | Disposition |
|---|---|---|---|
| `rt64_descriptor_sets.h` | refused | STILL VALID, **reason wrong** | Not portable. "Zero arithmetic" is true but not load-bearing: the binding indices *are* enumerated content. Survives on fn64's standing bindings-have-no-CPU-meaning precedent (`rt64_framebuffer_shaders.rs:105`, `rt64_rsp_process.rs:93-100`) — cite that instead. |
| `rt64_framebuffer_renderer.h` | refused | STILL VALID, **reason wrong** | Not portable. "Its `.cpp` arithmetic is separately owned" is false: `rt64_render_target_geometry.rs:18-21` ports only `viewportScissorIntersection` and calls the other ~1,912 lines a standing reject. |
| `rt64_framebuffer_renderer_call.h` | refused | **EXPIRED** | Portable, ~11 lines. "No `repr(C)` claim" is a non-sequitur — it conflates the union layout (correctly unportable) with the 7-value `Type` enum (`:14-22`) and three named bit-flags (`:41-43`), neither of which needs a layout claim. Unowned: fn64's `FillRectangle` hits are the libultra `gDPFillRectangle` opcode, a different authority. |
| `rt64_geometry_mode.cpp` | refused | **STILL VALID** | Not portable. Verified `this->v = v;` and nothing else. Stronger than stated: the `GeometryMode` struct has zero referrers anywhere in RT64 — dead code. |
| `rt64_geometry_mode.h` | refused | **STILL VALID** | Not portable. One field, one declaration. |
| `rt64_look_at_processor.h` | refused | **STILL VALID** | Not portable. Two float defaults; owner verified at `rt64_interpolation_helpers.rs:26-30` porting `:38-42`'s exact expression. |
| `rt64_native_target.h` | refused | **STILL VALID** | Not portable. Eight zero-inits. `getNativeSize`'s body is owned at `rt64_framebuffer_geometry.rs:227-245`. |
| `rt64_optimus.cpp` | refused | **STILL VALID** | Not portable. Verified entirely `#ifdef _WIN32`; one exported DWORD. |
| `rt64_projection_processor.h` | refused | **STILL VALID** | Not portable. Three float defaults; owner verified at `rt64_interpolation_helpers.rs:32-38`. |
| `rt64_raster_shader.h` | refused | **EXPIRED** | Portable, ~2 lines from the header (the `pipelines[8]` cardinality at `:81` and its z/cvg meaning). "Its `.cpp` is authority-gated" is gate-as-veto and must be dropped. The substantive `pipelineStateIndex` bit-pack lives in the gated `.cpp` — see below. |
| `rt64_raster_shader_cache.h` | refused | **STILL VALID** | Not portable. Thread/mutex/queue members; substantiated by `docs/RT64-M6-M7-SCOPING.md:62`, `:96-99`. |
| `rt64_render_target.h` | refused | **STILL VALID** | Not portable. `MaxDimension`'s value is already owned as `MAX_DIMENSION` (`rt64_render_target_geometry.rs:55-56`); the rest is handles and zero-inits. |
| `rt64_render_target_manager.cpp` | refused | **UNSUPPORTED** | Portable, ~14 lines. The hash sub-claim is sound (verified two `XXH3_64bits(this, sizeof(...))` at `:20` and `:72`, not one), but "its only non-plumbing content" is false: `isEmpty` (`:24`), the override-before-cache lookup order (`:39-55`), the depth-≥-color invariant (`:79`), the revision-staleness rule (`:111-128`) and the target-referencing sweep (`:134-144`) are all unowned and independent of the ABI question. |
| `rt64_render_target_manager.h` | refused | **UNSUPPORTED** | Portable, ~9 lines. Making the header's fate derivative of the `.cpp`'s hash question is unsound; the two key shapes and the read-only-depth variant (`:15-25`, `:42-60`) are independent content. Fold into the `.cpp` card. |
| `rt64_render_worker.cpp` | refused | **STILL VALID** | Not portable. Pure plume RHI; substantiated at `docs/RT64-M6-M7-SCOPING.md:87`. |
| `rt64_render_worker.h` | refused | **STILL VALID** | Not portable. Substantiated at `docs/RT64-M6-M7-SCOPING.md:86`. |
| `rt64_rsp_processor.cpp` | refused | **EXPIRED** | Portable, ~9 lines. The clearest scope error in the audit: the cited fn64 refusal (`rt64_rsp_process.rs:82-87`) is about **`RSPProcessCS.hlsl`'s push-constant struct** — the shader's per-thread index. This C++ file computes a different thing: a host-side watermark `vertexStart = computedSize / (sizeof(float)*4)` (`:22-23`), three buffer advances at different strides (`:26-28`), and `dispatchCount = (n + 63) / 64` (`:73`, `:86`). Unowned. |
| `rt64_rsp_processor.h` | refused | **EXPIRED** | Portable, ~5 lines (the two CB field sets, `:13-22`). Same basis. |
| `rt64_sampler_library.h` | refused | **STILL VALID** | Not portable. Twelve plume sampler handles. |
| `rt64_shader_library.h` | refused | **EXPIRED** | Portable, ~41 lines. Gate-as-veto phrasing must be dropped. The header independently enumerates **39 named `ShaderRecord` slots** (`:21-59`) plus two capability flags. Verified unowned: `crates/fn64-render-wgpu/src/shader_manifest.rs` is fn64's own direct-texel-decode component, not a port of these; `grep fbWriteDepthMS\|rspVertexTestZMS\|videoInterfaceLinear crates` returns zero. |
| `rt64_texture.h` | refused | STILL VALID, **reason wrong** | Not portable (nine zero-init fields). But the named owner is inaccurate: `rt64_texture_map_lru.rs` ports `rt64_texture_cache.{h,cpp}`, a different file, and treats `Texture *` as an opaque `u64` (`:470`). |
| `rt64_tile_processor.h` | refused | **STILL VALID** | Not portable. Verified two float defaults; owner at `rt64_render_pipeline_types.rs:26-38`. |
| `rt64_transform_processor.cpp` | refused | **EXPIRED** | Portable, ~22 lines. "Its math is `inverse`/`transpose` plus an owned `lerp`" describes the calls and omits the structure: two loops (`:40`, `:64`), a two-level branch (`:26`, `:42`), two `lerp` calls at *different weights* from the same endpoints (`:45-46`), and a real invariant — the `prevFrameValid` path fills all three output vectors while the else path fills only `invTWorldTransforms`, compensated at `:81-82`. `RigidBody::lerp` is `pub` (`rt64_rigid_body.rs:529`) and `inverse4` is `pub` (`rt64_math_matrix.rs:655`), but `transpose4` is **private** (`rt64_math_decompose.rs:733`) — a widen, not a port. |
| `rt64_transform_processor.h` | refused | **STILL VALID** | Not portable. Two RHI members, two float defaults, five declarations. |
| `rt64_upscaler.h` | refused | **STILL VALID** | Not portable — the best-evidenced refusal of the 28. `rt64_postprocess.rs:254-260` names the header explicitly as the ticket's non-goal and `:424-438` ports five of the eight `QualityMode` values with matching discriminants, disclosing at `:425-427` that `Native`/`Auto`/`MAX` are unreached. Residual is 3 unreachable names and 6 pure virtuals. |
| `rt64_vertex_processor.cpp` | refused | **EXPIRED** | Portable, ~8 lines. Same shader-vs-host substitution as `rt64_rsp_processor.cpp`: `computedSize / 16` at `:21-27`, `(vertexCount + 63) / 64` at `:59`. |
| `rt64_vertex_processor.h` | refused | **EXPIRED** | Portable, ~4 lines (`WorldCB`, `:13-18`). Same basis. |
| `rt64_vi_renderer.h` | refused | **EXPIRED** | Portable, ~1 line (`filtering = Filtering::Linear`, `:29`). Gate-as-veto phrasing must be dropped. Not worth a card alone — but dropping the veto exposes that the `.cpp` holds ~45 lines of unowned geometry (below). |

### `src/contrib`, `src/gbi`, `src/hle`, `src/render`, `src/shaders` — 10 authority-gated

**Baseline: not one of the 10 digests is cited by any Rust module.** All ten
were recomputed with `shasum -a 256`; the only hits are
`crates/fn64-render-rt64/ffi/CMakeLists.txt` (build-time tripwire),
`adapter_source_identity.rs:230,231,322` (source identity), and the two docs.
So the rule change flips **zero** of these ten today — it removes the *reason*
they could not be ported, not the work.

The worked example of a legitimate gated credit is `src/hle/rt64_vi.cpp`, which
reads `ported` via `rt64_vi_timing.rs:14` while still emitting its
`authority_gate` record, with an explicit ~74-of-177 boundary at
`rt64_vi_timing.rs:66-99`.

| Source | Lines | Class | Portable content no fn64 owner covers | Overlay |
|---|---|---|---|---|
| `src/gbi/rt64_gbi_s2dex.cpp` | 664 | **EXPIRED** | **YES, ~260.** `bg1Cyc` (`:129-400`) fixed-point background arithmetic: `frameWmax = (imageW<<10)/scaleW` and `(frameWmax-1)&~0x3` (`:152-155`), the U5.10/U5.7 chain (`:220-230`), TMEM slicing (`:252-320`), `bg1CycTMEMLoad` (`:60-127`), `bgCopy` (`:420-425`). Verified unowned: `rt64_gbi_s2dex2.rs` ports a **different 91-line file** (`rt64_gbi_s2dex2.cpp`, `cf219a09…`) and names `bg1Cyc` only to place it out of scope (`:211-216`). | Only `objLoadTxRect` (CMake `:312-355`). `bg1Cyc` compiled verbatim. |
| `src/shaders/RasterPS.hlsl` | 313 | **EXPIRED** | **YES, ~65.** `MaxDepth = 1022.0f/1024.0f` gated on `!renderFlagRect && !zSourcePrim` (`:71-89`) — verified unowned, zero `1022` hits in `crates`; decal tolerance composition (`:97-111`); low-res UV correction (`:114-121`); the 1-cycle two-tile aliasing (`:150-157`); float coverage `(8.0/cvgRange)` (`:216-255`), which fn64's integer `Coverage(0..8)` model (`coverage.rs:138-173`) does **not** cover. | Noise and alpha-dither only (CMake `:465-548`) — and both are already owned (`random.rs:171`, `formats_dither.rs:147`). Exclude those; the rest is verbatim. |
| `src/render/rt64_vi_renderer.cpp` | 125 | **EXPIRED** | **YES, ~45.** `computeHDSize` (`:17-19`), `fromSDtoHD` (`:21-24`), `fromHDtoWindow` (`:26-42`), `getViewportAndScissor` (`:89-124`). fn64's `pillarbox_derive` (`rt64_render_target_geometry.rs:417`) is a *different* derivation from a *different* file — it branches on `resolutionScale.x > resolutionScale.y`, not window-vs-HD aspect, and floors/ceils where this `lround`s. Same word, different arithmetic. | Only the `pushConstants` block (CMake `:830-844`). Geometry verbatim. |
| `src/hle/rt64_interpreter.cpp` | 200 | **EXPIRED** | **YES, ~45.** `AddressMask = 0xFFFFF8` (`:31-33`); the `& 0x3F` opcode masking at `:63`,`:90`,`:115` versus the unmasked `>>24` at `:173`; split-command reassembly (`:78-137`). Zero hits for `0xFFFFF8` or the masking shape in `crates`. | `loadUCodeGBI`'s cache body deleted (CMake `:975-988`). The prior re-refusal is right for GBI selection, **not** for the reassembly. |
| `src/render/rt64_raster_shader.cpp` | 611 | **EXPIRED** | **YES, ~35.** The PSO decision table (`:222-233`: `zCmp && zMode() != ZMODE_DEC`, `cvgDst() == CVG_DST_WRAP/SAVE`); `pipelineStateIndex` (`:601-606`, `zCmp<<0 \| zUpd<<1 \| cvgAdd<<2`) — verified zero hits in `crates`; the spec-constant key packing (`:127-131`). fn64's `targets/triangle_pipeline.rs` cites `:317`/`:460` in prose and takes a 4-variant subset with no `cvgAdd` axis — prose citation is not ownership. | Blob symbol renames only (CMake `:717-744`). Decisions verbatim. |
| `src/hle/rt64_state.cpp` | 2804 | **EXPIRED** (sampled) | **YES, ~30 in the sampled region**, UNKNOWN for the ~2,400 lines not read. The reinterpret-hash alignment block (`:704-732`): `rectMultiplier = rectDsdx>>10`, `powerOfTwo = (m & (m-1)) == 0`, `texelMask.x = ~(m-1)`, the TLUT offset and `byteCount`. fn64's `rt64_fb_reinterpret.rs` ports `FbReinterpretCS.hlsl` — the shader-side conversion, not this CPU-side decision. | Only the `updateScreen` condition (CMake `:886-889`). **Pin hazard: CMake pins the ORACLE digest `07d3a7c1…`; the port pin's TMEM-region block (`:339-356`) is not in the tree fn64 compiles.** |
| `src/hle/rt64_present_queue.cpp` | 549 | **EXPIRED** (sampled) | **YES, ~25.** Ring wrap `(writeCursor+1) % presents.size()` (`:39`) — a modulo, where fn64's `previous_write_cursor` (`rt64_workload_geometry.rs:803-809`) is a backward step from a different file; the pacing period (`:393-394`); the `framesToPresent` clamp (`:280-289`). | Two observe calls under `FN64_RT64_HFR_EVIDENCE` (CMake `:1189-1196`); non-mutating. |
| `src/render/rt64_raster_shader_cache.cpp` | 182 | **STILL VALID** | **NO.** Read in full: `std::thread`/`mutex`/`condition_variable`/`queue` throughout; the only non-plumbing expression is `desc.hash()`, whose body is in an uncited header. Zero arithmetic. | Two-line startup/loop reorder (CMake `:1036-1069`), itself a race fix. |
| `src/render/rt64_shader_library.cpp` | 919 | **STILL VALID** | **NO.** The only arithmetic is `sizeof(uint32_t)*N` push-constant sizes. | **Decisive.** All four gate mechanisms are VI filters, and CMake `:790-799` redirects VI selection to fn64's own 492-line `fn64_rt64_video_interface_ps.hlsl`. Porting the selection would port the selection of a shader fn64 does not use. |
| `src/contrib/plume/plume_metal.cpp` | 4229 | **STILL VALID** | **NO** (out of scope). A Metal RHI backend; fn64 delegates Metal to wgpu (`device/mod.rs:33`). Porting ~30 format-enum tables would build a second backend fn64 never calls. | Six `retain()` insertions under `if(APPLE)` (CMake `:1233-1362`). **Pin hazard: CMake pins the ORACLE digest `73d8405d…`.** |

## Genuinely portable, ranked

24 files, ~795 real lines. Each line is a scope note an executor can turn into
a card.

| # | Source(s) | ~Lines | Scope note |
|---|---|---|---|
| 1 | `src/gbi/rt64_gbi_s2dex.cpp` | 260 | Port `bg1Cyc`/`bg1CycTMEMLoad`/`bgCopy` S2DEX fixed-point background arithmetic as pure functions over the decoded `uObjBg` fields; **exclude the overlaid `objLoadTxRect` region**. |
| 2 | `src/shaders/RasterPS.hlsl` | 65 | Port the depth-clip `1022/1024` constant and its `!renderFlagRect && !zSourcePrim` gate, the decal tolerance composition, the low-res UV correction, the 1-cycle two-tile alias, and the float coverage estimate; **exclude noise and alpha-dither** (owned and overlaid). |
| 3 | `src/render/rt64_vi_renderer.cpp` | 45 | Port `computeHDSize`/`fromSDtoHD`/`fromHDtoWindow`/`getViewportAndScissor` as pure geometry; state explicitly that this is a *different* derivation from `pillarbox_derive`, not a duplicate. |
| 4 | `src/hle/rt64_interpreter.cpp` | 45 | Port the `0xFFFFF8` address masking, the `& 0x3F` opcode-mask asymmetry, and the split-command reassembly; **exclude `loadUCodeGBI`'s GBI selection** (overlay deletes it). |
| 5 | `src/common/rt64_load_types.h` | 45 | Own `LoadTlut {None,Rgba16,Ia16}`, `LoadOperationType {Tile,Block,Tlut}`, the three payload PODs and the two wire-string tables; retires the "uncited header" caveats at `rt64_hle_geometry.rs:595`, `:613-617`. |
| 6 | `src/render/rt64_shader_library.h` | 41 | Pin the 39-slot `ShaderRecord` manifest and the MS-variant pairing rule as a checkable enumeration; no pipeline, no compile, no plume. |
| 7 | `src/render/rt64_raster_shader.cpp` (+`.h`) | 37 | Port the PSO decision table (`ZMODE_DEC`, `CVG_DST_WRAP/SAVE`) and `pipelineStateIndex`'s 3-bit pack with its 8-entry cardinality. |
| 8 | `src/hle/rt64_state.cpp` | 30 | Port the reinterpret-hash alignment block only (`:704-732`); **name the pin** — CMake compiles the oracle. |
| 9 | `src/gui/rt64_camera_controller.{cpp,h}` | 34 | Port `movePerspective`/`rotatePerspective`/`lookAtPerspective` over an owned camera struct, reusing the five already-`pub` primitives in `rt64_math_matrix.rs`; `moveCursor`'s ImGui gating stays refused. |
| 10 | `src/render/rt64_rsp_processor.{cpp,h}` + `rt64_vertex_processor.{cpp,h}` | 26 | One card, four files: the host-side buffer watermark, three stride advances, `div_ceil(n,64)`, and the skip guards. Distinguish explicitly from the shader-side refusal in `rt64_rsp_process.rs`. |
| 11 | `src/hle/rt64_present_queue.cpp` | 25 | Port the ring-wrap modulo, the pacing period, and the `framesToPresent` clamp. |
| 12 | `src/render/rt64_render_target_manager.{cpp,h}` | 23 | Port `isEmpty`, the override-before-cache order, the revision-staleness rule and the two key shapes as a value-equality key; **the XXH3 padding hash stays refused**. |
| 13 | `src/render/rt64_transform_processor.cpp` | 22 | Port the two-branch fill discipline and the three-vector emptiness invariant; reuse `rt64_rigid_body::lerp` and `inverse4`, and widen `transpose4` rather than reimplementing it. |
| 14 | `src/common/rt64_filesystem_zip.{cpp,h}` | 33 | Own the five PKZIP wire constants, the local-header data-offset arithmetic, and the compression-tag map with its silent-skip default; decompression stays out. |
| 15 | `src/hle/rt64_application.h` | 14 | Two tag enums and three `ApplicationConfiguration` defaults, in the shape `settings.rs:677-712` already uses four times. |
| 16 | `src/render/rt64_framebuffer_renderer_call.h` | 11 | The 7-variant draw-call `Type` enum and three named bit-flag semantics; no layout, size, or bit-position claim. |
| 17 | `src/hle/rt64_command_warning.h` | 10 | `IndexType` plus its 3-arm union as a Rust tagged enum — or re-refuse on the sharper "the `.cpp` never writes the tag" ground. |
| 18 | `src/hle/rt64_framebuffer_pair.h` | 8 | The 6-value `FlushReason` enum; correct the "six scalar fields" claim to five. |
| 19 | `src/hle/rt64_game_configuration.h` | 8 | The five literal defaults, pinned by a test feeding them into the landed `estimated_sun_light` (`rt64_light_estimation.rs:292`). |
| 20 | `src/common/rt64_user_paths.cpp` + `rt64_filesystem.h` | 12 | The four RT64 on-disk filename constants and `setupPaths`' empty-root guard, plus `toForwardSlashes`; replaces 7 bare `"rt64.json"` literals with a cited owner. |
| 21 | `src/render/rt64_raster_shader.h` | 2 | Folded into #7. |

## UNKNOWNs

- **`src/hle/rt64_state.cpp`**, the ~2,400 lines not read (`:200-460`,
  `:461-643`, `:792-1838`, `:1978-2657`). The ~30-line finding is from sampled
  regions and must not be generalized. To resolve: a full read of `fullSync`
  and `inspect`, against the **oracle** pin.
- **`src/contrib/plume/plume_metal.cpp`**, the ~3,900 lines of encoder body not
  read. The out-of-scope disposition rests on the file's role, not a full read.
- **`rt64_hle_geometry.rs:596`**'s `G_TT_RGBA16`/`G_TT_IA16` → `LoadTLUT`
  mapping, refused over "constants from an uncited header". If that header is
  `src/shared/rt64_f3d_defines.h` it reads `ported` and the reason is expired;
  the path is not named in the reason, so this is unresolved. To resolve: name
  the header at the refusal site.

## Recommended corrections that create no executor work

Reason rewrites where the verdict survives but the stated ground is wrong:
`rt64_thread.{cpp,h}` (names the wrong function; the `Priority` enum is in the
header), `rt64_framebuffer_changes.h` (omits the enum it in fact ported),
`rt64_game_call.h` (five members, not four; `DrawCall` is not owned),
`rt64_transform_group.h` (twelve G_EX constants plus one bool, not thirteen),
`rt64_present_queue.h` (two condition variables, not three), `rt64_rdp_tmem.h`
(evidence range is `:585-596`), `rt64_state.h` (name the five `Extended`
sentinels and tie the refusal to the gate), `rt64_descriptor_sets.h` (cite the
bindings precedent), `rt64_framebuffer_renderer.h` (the `.cpp` is 99% refused,
not owned), `rt64_texture.h` (the named owner ports a different file),
`rt64_interpreter.h` and `rt64_present.h` (replace the metric-invisibility
reason with the zero-initializer substance).

## Honest read

**The remaining 38.2% is mostly legitimately refused, but the portable residue
is real, and it is not where the file count suggests.**

Of the 38.2 points, **22.0 are the ten authority-gated files** and only 16.2 are
the seventy-three refusals. The refusal set is genuinely thin: 48 of 73 are
correctly refused host syscalls, plume/Vulkan handles, ImGui, vendored
third-party bindings, and zero-body declaration headers. What is portable there
is small-bore — enums, defaults, wire constants — worth about 250 lines total,
and much of it is worth porting less for the credit than because it retires
"uncited header" caveats that are currently blocking *other* refusals.

The gated set is the opposite: three of ten are firmly out of scope, but the
other seven hold ~505 lines of real, unowned arithmetic — and `bg1Cyc` alone
(~260 lines of S2DEX fixed-point background math) is larger than the entire
portable residue of the 73 refusals combined. None of it was blocked by a
measurement; it was blocked by a rule that has now been retired, and by three
refusals that used "its `.cpp` is authority-gated" as a veto.

Two cautions for whoever executes. First, the **existence-vs-scope trap fired
repeatedly** in the set I audited: `pillarbox_derive` versus `fromHDtoWindow`,
integer `Coverage(0..8)` versus float `cvgRange`, `LoadTile`'s 3 fields versus
15, `{Block,Tile,Other}` versus `{Tile,Block,TLUT}`, and `rt64_gbi_s2dex2.rs`
versus `rt64_gbi_s2dex.cpp` are five separate cases of the same name denoting
different arithmetic. A duplication pre-screen must compare the computation, not
the identifier. Second, **the overlay really does overlay the prize away in one
case**: `rt64_shader_library.cpp`'s four gate mechanisms are all VI filters and
CMake `:790-799` redirects them to fn64's own shader, so its 919 lines yield
nothing. Checking the overlay before scoping saved one wasted card here and
narrowed three others.

Finally, the citation hygiene finding stands on its own and is arguably the
most actionable: **46 of 73 refusals cite an evidence file that never mentions
them.** Three batch citations produce all but three of those. The refusals are
mostly *right*; they are just not, at present, *checkable* — which is the one
property the inventory's own prose says a declared state must have.
