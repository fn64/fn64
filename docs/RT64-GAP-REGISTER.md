# RT64 Gap Register — for fn64's Rust port

Purpose: fn64 will port RT64's N64 RDP renderer to Rust (wgpu). This register catalogs where
RT64 is **known-incomplete or approximate**, so the port **closes** those gaps rather than
faithfully reproducing them. Read-only research; no code changed.

- **Checkout:** `/Users/jer/Code/no-mercy-recompiled/third_party/rt64` @ `f0728a2`.
- **Upstream drift:** `f0728a2` is the **tip of `origin/main`** (0 commits behind, `git rev-list --count f0728a2..origin/main == 0`). No missed merged accuracy fixes. **Pin recommendation: keep `f0728a2`.** All open fixes live in open PRs, not in unmerged main commits (see PR section).
- Sources: in-source markers (~92 in src/hle,render,shared,gbi), open issues (`gh`, authed jeremyw), open PRs.

---

## A. Accuracy gaps the port should fix

### A1. Microcode / GBI command coverage (hard stops today — `assert(false)`)
These are microcode features RT64 does **not** implement; it aborts. A faithful renderer that
targets games using these needs them. (For OoT specifically, most are irrelevant — see scope §D.)

| Site | Gap | Fix direction |
|---|---|---|
| `src/gbi/rt64_gbi_s2dex.cpp:576,595,614` | S2DEX `objLoadTxSprite/objLoadTxRect/objLoadTxRectR` draw side is `assert(false)` — texture load runs but `doObjSprite/doObjRectangle(R)` unimplemented | Implement the sprite/rect draw path (2D microcode) |
| `src/gbi/rt64_gbi_s2dex.cpp:403,415,450` | S2DEX BG (`objRenderMode`, `bgLoadTile`-only, texrect count) approximate/partial: "Reimplement more accurately to match the microcode"; only `S2DEX_G_BGLT_LOADTILE` variant handled | Match microcode; handle non-loadtile BG, remainder-pixel last texrect |
| `src/gbi/rt64_gbi_s2dex.cpp:19,30,37 & s2dex2.cpp:19,30` | Bare `assert(false)` stubs in S2DEX/S2DEX2 dispatch | Implement remaining S2DEX(2) opcodes |
| `src/gbi/rt64_gbi_f3dex2.cpp:58,91,130,134` | Unimplemented `moveMem`, `moveWord`, "combine matrices mode", `special1` | Handle the missing F3DEX2 sub-ops |
| `src/gbi/rt64_gbi_f3dex2.cpp:157` (`line3D`), `f3dex.cpp:42`, `f3d.cpp:90,107,114,136` | `line3D` and assorted F3D/F3DEX ops empty/`assert` | Implement line3D + the F3D TODO ops |
| `src/gbi/rt64_gbi_l3dex2.cpp:12` | `assert(false)` in L3DEX2 (line microcode) | Implement line microcode dispatch |
| `src/gbi/rt64_gbi_f3dwave.cpp:38` | `F3DWAVE_G_UNKNOWN` command nulled out "until it's figured out" | RE the unknown Wave Race command |
| `src/gbi/rt64_gbi_extended.cpp:19,349,384` | Extended-GBI unrecognized/invalid opcodes unhandled | (EX GBI is an RT64 enhancement — likely skip for faithful port, see §D) |
| `src/hle/rt64_rsp.cpp:832` | `assert(false && "Unsupported modify vertex")` — `G_MODIFYVTX` sub-modes unhandled | Implement modify-vertex sub-commands |
| **issue #195** | No **Rej** microcode support (F3DLX.Rej / clipping-reject variants) | Add Rej ucode |
| **issue #217** | **F3DEX2 missing `resetFromLoad` for loaducode** — lights/lookat/fog/geom-mode/texture state not re-zeroed on ucode load (differs from F3D) | Zero-init per issue's checklist on loaducode; audit F3DEX too |
| **issue #243** | `setPerspNorm` not handled — `0` should mean nothing renders (degenerate persp) | Detect and cull |

### A2. Rasterization / RDP pixel-pipeline accuracy (the CORE — port these correctly)
These are where RT64 is **approximate** in the base rasterizer. Highest fidelity value.

| Site / Issue | What's wrong | Fix if known |
|---|---|---|
| `src/hle/rt64_rdp.cpp:115` | `assert(false && "Crash screen not implemented.")` — RDP crash/error path missing | Low priority for faithful render; port graceful handling |
| `src/hle/rt64_framebuffer_manager.cpp:345` | `assert("Unimplemented reinterpretation logic.")` — some framebuffer format reinterpretations unhandled | Implement the missing fmt-reinterpret cases |
| `src/hle/rt64_state.cpp:556` | Direct FB reinterpretation "not implemented yet" → forces a sync fallback | Implement direct reinterpret to avoid stalls + correctness edge |
| `src/render/rt64_native_target.cpp:167-176, 254-262` | Native FB **readback/writeback** unimplemented for 4-bit, RGBA8, IA8, CI16/32, IA16/32, I16/32 (long assert list) | Implement the missing color-image read/writeback format combos |
| `src/render/rt64_render_target.cpp:180,268` | `assert(false)` unsupported render-target copy / buffer format | Handle remaining copy/format combos |
| `src/render/rt64_texture_cache.cpp:730` | Unsupported DDS format (replacement-texture path) | Enhancement-adjacent (§D); low priority |
| **issue #200** | **RDP register timing off by one cycle** in 2-cycle mode: Texel0/Texel1 fetch, shade color, alpha-compare, memory color all use next/prev pixel's value. "Can't be emulated exactly; approximate with derivatives." | Port the texel1-derivative trick + extend to shade/alpha/mem; add debugger warnings |
| **issue #189** | **TLUT accuracy**: in copy mode / bilerp, the 4 samples of a palette pixel should each use their own palette entry | Per-sample TLUT lookup |
| **issue #116** | **Tile index wrap**: tile0=7 ⇒ tile1 should wrap mod 8 (not saturate at 7). Also a **buffer overflow**: `RDPTiles[8]` OOB when `rdpTileIndex==7` w/ LOD off (`RasterPS.hlsl:139`) | mod-8 tile selection + bounds-fix; handle LOD clamp special case |
| **issue #201** | **Light color copy** not applied to odd-indexed vertices (SM64 Koopa) | Fix per-vertex light-color copy indexing |
| **issue #203** | **F3D near-clip** doesn't trigger at exactly z=0 (tri at z=0 with near=0 wrongly shows) | Correct near-clip boundary (≤ vs <) |
| **issue #106** | **FRUST_RATIO clipping** imposed by RSP viewport not implemented | Implement frustum-ratio clip |
| **issue #103** | **Prim Z (`gsDPSetPrimDepth`) inaccurate** — Prim Z −1 renders behind on HW but is *missing* in RT64 (needs 0x7FFE hack) | Fix prim-depth mapping to HW range |
| **issue #198** | **Depth precision** light flicker (Perfect Dark) | Match N64 depth quantization |
| **issue #235** | **Vertex coloring wrong** (Mystical Ninja Goemon frog white not green) — shading/combiner bug | Root-cause combiner/shade path |
| **issue #194** | Goldeneye sky needs **multiple triangles from LLE triangles** (rasterizer-shape emulation) | Hard; only if targeting affected games |
| **issue #183** | **Flat shading** currently via shader flat-output (driver-fragile); should duplicate vertices with copied normals | Duplicate-vertex flat shading (also simpler shaders) |
| **issue #150** | **F3D dz decal tolerance**: F3D/F3DEX tweak `dzdy` to raise computed dz → higher decal tolerance; replicate in pixel shader | Tweak computed dz to match |
| **issue #210 / #202 / #197 / #193 / #199** | Unverified RDP edge behaviors: copy-mode w/ mismatched formats; ZBUFFER-off + depth mode; fill-rect vs scissor in copy mode; fillrect `lrx+1` inclusivity; erroneous loadtile assert (`rt64_rdp.cpp:579`) | Verify against HW and encode correct behavior; remove bogus assert |
| **issue #82** | Extended-scissor/rect misalignment should round up (not down) when origin right | Rounding fix (widescreen-adjacent, §D) |

### A3. RSP transform / lighting / texgen approximations
| Site | Gap |
|---|---|
| `src/hle/rt64_rsp.cpp:245` | Assumes RDRAM loads never unaligned/overlapping — unverified assumption in vertex load |
| `src/hle/rt64_rsp.cpp:1123` | TEXGEN texcoord tracking unhandled (`// TODO: Figure out how to handle texcoord tracking on TEXGEN cases`) |
| `src/hle/rt64_rsp.cpp:966` | bare `// TODO` in RSP command handling |
| `src/hle/rt64_game_frame.cpp:566-567` | look-at matching stubbed (`(true && true)`); assumes one look-at per draw call, only checks first vertex — texgen/env-map interpolation approximate |

### A4. Temporal / interpolation heuristics (RT64's frame-interpolation layer)
RT64 does motion interpolation via workload/projection matching and a rigid-body decomposition.
This is **RT64-specific machinery**, not N64 semantics — mostly §D (skip) for a faithful port,
but noted because it drives *transform correctness* if the port keeps any interpolation:
- `rt64_game_frame.cpp:128,139,268,512,929,948,1012,1028` — linear-lookup workload/projection matching, "must be per projection", "assume same order between frames", "support more than perspective projections", velocity-buffer TODOs.
- `rt64_rigid_body.cpp:22,23,42,55,74` — velocity tolerances hardcoded, scale/skew assumed to match rotation, "defaults to always interpolate", perspective-interp TODO.

---

## B. Open PRs — ready-made fixes to fold into the port

| PR | Fixes | Port relevance |
|---|---|---|
| **#254** | Improve synchronization detection for tiles being sampled (don't require sync for tagged FB regions when the fbPair is synced) | **Correctness+perf** — fold the sync logic into the port's framebuffer/tile sync model |
| **#246** | Removes `VK_EXT_scalar_block_layout` requirement — fixed serious graphical corruption on devices lacking it (Switch/Nvidia). Closes #23 | Reveals a **buffer-layout assumption**; the wgpu port must not depend on scalar block layout for its uniform/storage buffers |
| **#248** | `texture-cache`: parse replacement-DB bytes portably (libc++ `char_traits<unsigned char>`) | Build-portability only; irrelevant to a Rust port |
| **#251** | Bump hlslpp to v3.9 (macOS 26 build) | Irrelevant (Rust/wgpu, no hlslpp) |
| **#249** | Fix device-vendor console log formatting | Irrelevant |
| **#252** | Librashader support | Enhancement, §D — skip |

Only **#254** (tile-sampling sync) and **#246** (no scalar-block-layout assumption) carry lessons for the port. Neither main nor these PRs contain an un-ported base-rasterization accuracy fix beyond what's already in `f0728a2`.

---

## C. What RT64 itself does NOT do well / at all (from code + domain knowledge)
- **2-cycle RDP register timing** (#200) — fundamentally approximate; can't be bit-exact.
- **Coverage / AA / dither** — RT64 renders on a modern rasterizer; N64 3-point coverage-based AA and RDP dither are approximated, not emulated (issue #38, #73/#107 MSAA sample positions).
- **TMEM edge cases** (#196) and **TLUT copy/bilerp** (#189) — partial.
- **S2DEX 2D sprite/rect microcode** — draw side unimplemented (§A1).
- **Rej / line (L3DEX2) / Wave microcodes** — missing/partial.
- **Prim-depth, near-clip-at-0, frustum-ratio clip, flat shading** — each individually wrong (§A2).
- **Framebuffer format reinterpretation & native read/writeback** — several format combos assert-out.
- **PAL/50Hz** — `rt64_vi.cpp:166` hardcodes 60Hz FullRate (`// TODO: PAL support`).

---

## D. Scope boundary — CORE-faithful (port) vs ENHANCEMENT/driver/perf (skip or defer)

**PORT (accurate base renderer — this is fn64's target):**
- `src/gbi/*` GBI/microcode decode (F3D, F3DEX, F3DEX2, F3DZEX2 — OoT uses F3DZEX2/F3DEX2), `src/hle/rt64_rsp.cpp` (transform/lighting/clip), `rt64_rdp.cpp` + `rt64_rdp_tmem.cpp` (RDP state, tiles, TMEM), `rt64_state.cpp` core draw path, `src/shared/rt64_blender.h` + `rt64_color_combiner.h` + `rt64_other_mode.h` (blender/CC/othermode semantics), the raster pixel-shader logic (`src/shaders/RasterPS.hlsl`, `TextureSampler.hlsli`), framebuffer manager + native target read/writeback, `rt64_vi.cpp` (VI scan-out).
- **These carry the §A accuracy gaps — port the corrected versions.**

**SKIP / DEFER (RT64 enhancement + platform surface — not needed for faithful N64 output):**
- **Ray tracing:** `rt64_framebuffer_renderer.cpp` RT paths, `rt64_raytracing_params.h`, `rtProj`/`rtScenes`/anyhit — entirely enhancement.
- **Upscaling / widescreen / interpolation:** `rt64_upscaler.cpp`, widescreen hacks (`rt64_rdp.cpp:1181`, #28, #82), the frame-interpolation machinery (`rt64_game_frame.cpp`, `rt64_rigid_body.cpp`, `rt64_workload_queue.cpp` interpolation), light-manager scene heuristics.
- **Extended GBI (EX GBI):** `rt64_gbi_extended.cpp`, EX viewport/matrix/branch issues (#79,#93,#114,#166,#169) — RT64-project-specific extensions, not stock N64.
- **Texture replacement / DDS packs:** `rt64_texture_cache.cpp` DDS/replacement (#120,#167,#182,#83), `#37/#36` resampling/gamma — enhancement.
- **GPU-driver/platform workarounds (IRRELEVANT — wgpu):** everything in `rt64_application.cpp`, `rt64_application_window.cpp` (Android/refresh-rate asserts), Nvidia power-state hacks (`rt64_workload_queue.cpp:1180-1194`, #118/#219/#39), D3D12/Vulkan/Metal issues (#255,#218,#176,#112,#105,#102,#101,#85,#27,#25,#23,#21,#17), swap-chain/format TODOs.
- **Perf TODOs (nice-to-have):** linear-lookup elimination, per-projection caching (`rt64_game_frame.cpp:128,948`), shader-list rework (#57), 3-point-filter optimization (#177).

---

## E. Top 5 gaps to close in the port (prioritized for faithful OoT output)

OoT uses **F3DZEX2** microcode; it is a 3D game (no S2DEX 2D-sprite draws), so 2D-microcode and
Rej/Wave/line gaps are low priority *for OoT specifically*. Prioritization below is OoT-first.

1. **F3DEX2/F3DZEX2 loaducode state reset (#217)** — lights/lookat/fog/geom-mode/texture must zero on ucode load. OoT loads/reloads ucode; missing resets cause wrong lighting/fog on the first draws after a task. High-frequency, concrete checklist, cheap to port right.
2. **RDP 2-cycle register timing + combiner correctness (#200, #235)** — OoT leans on 2-cycle CC/blender heavily; texel1/shade/alpha timing and the shade-color path drive most surface coloring. Port the texel1-derivative approximation and audit the combiner shade path (the Goemon-frog class of bug).
3. **Tile-index mod-8 wrap + `RDPTiles[8]` OOB fix (#116)** — an actual out-of-bounds read plus wrong multi-tile/LOD texturing. Correctness + memory-safety; trivially wrong today.
4. **Decal / Prim-Z / near-clip depth accuracy (#150, #103, #203, `state.cpp:1655`)** — OoT uses decals (shadows, floor marks) and depth-sorted effects; RT64's decal path is admittedly re-approximated ("Reimplement proper decals in RT") and prim-depth/near-clip are off. Fixes z-fighting and missing/mis-depthed sprites.
5. **Blender simple-vs-approx emulation completeness (`rt64_blender.h:224`) + framebuffer reinterpretation (`framebuffer_manager.cpp:345`, `state.cpp:556`)** — the blender falls back to approximations for framebuffer-color-dependent 2-cycle modes (fog, fades, transparency common in OoT); FB reinterpretation asserts out on some format transitions. Port the full blender-requirement logic and the missing reinterpret cases so effects don't degrade or crash.

(If the port later targets non-OoT AKI/other titles: promote S2DEX 2D-sprite draw (§A1) and Rej ucode (#195) — WWF/2D-heavy titles hit those.)
