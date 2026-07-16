# OoT bring-up on fn64 — done / todo map

OoT (NTSC 1.0) is fn64's correctness oracle. This is the durable status map.
Every "done" here is verified (byte-exact test, or an actual frame/PCM looked
at) — not a tracker label. Updated 2026-07-15.

## Verification contract (do not weaken)
- **Data** (ROM/savestate → verts/faces/matrices/PCM): byte/index-exact tests
  against real fixtures. No test → not done.
- **Visual** (the rasterized frame): verified by looking at the actual PNG
  side-by-side with the emulator. No agent self-certifies a render.
- A green unit test on a piece ≠ the whole program runs. Both audio and
  projection bugs this project hit were unit-green but end-to-end broken.

---

## ✅ DONE (verified)

### Boot & runtime
- 14+ boot-ladder rungs cleared; OoT reaches real game logic (~8 frames deep,
  30+ VI swaps, file-select scene reachable).
- Clean-room libultra shim layer (public headers/manuals only), DMA
  word-swizzle, OSTask dispatch, thread/queue model.
- Windowed harness (`fn64-shell`, winit+pixels+cpal): live framebuffer per
  swap, keyboard→controller, audio-out wired.
- Fast loop: `--release` + `OOT_MAX_SWAPS` early-exit (~250x), `./oot` runner,
  observability flags (`OOT_RENDER_STATS`, `OOT_DUMP_PROJ`, `OOT_NO_DEPTH`,
  `OOT_AUDIO_UCODE_TIMING`, `OOT_SKIP_AUDIO_UCODE`, `OOT_STOP_ON_FRAME`).

### Recompilers (both from-scratch, typed Rust, no external tool, no GPL)
- **CPU** `fn64-recomp-native`: MIPS III + COP1/FPU + 64-bit dword + COP0 +
  ELF/symbol front-end. Oracle-validated (differential vs N64Recomp C).
- **RSP audio** `fn64-audio`: 46 VU ops + scalar + dispatch. aspMain
  recompiles, **runs** (terminates in 112 steps, not the old 5M runaway) and
  **produces PCM**. Verified live in the boot (audio enabled, no hang) and by
  a real-command-list PCM test.

### Correctness bugs fixed this session (merged to main, pushed)
- **Projection transpose** (`gbi.rs transform_point`): applied `mvpᵀ·v`
  instead of `v·mvp`; clip-w became garbage. Fixed → simple/title frames now
  project 100% in-cube. (⚠ a SECOND projection issue remains on large-world
  scenes — see TODO #1.)
- **RSP aspMain IMEM base** 0x1080→0x1000: absolute jump targets were all off
  by 0x80 → runaway loop, zero PCM. Fixed → terminates + PCM. Native-endian +
  KSEG0-mask OSTask reads also fixed. (aki-recomp side committed local-only on
  `fix/rsp-aspmain-base-and-endianness`; needs the fixed `fn64-audio`.)

### Render — geometry & texture layers
- Geometry: G_VTX, G_TRI1/TRI2/QUAD, G_MTX (LOAD/MUL/PUSH), G_POPMTX, G_DL
  (call/branch, recursion-limited), G_ENDDL — **implemented**.
- **Depth / Z-test: implemented AND verified correct.** Viewport-mapped NDC-z
  (`sz=tz=127.75` → screen-z [0,255.5]), nearer wins (`z < depth[pix]`),
  rejects ~124k farther fragments/frame, 42% pixel delta vs painter's-order.
  Fail-against-bug regression tests in `raster.rs`. (Uses an internal z-array,
  not G_SETZIMG — functionally correct.)
- Textures: G_SETTIMG/SETTILE/SETTILESIZE/LOADTLUT implemented;
  LOADBLOCK/LOADTILE partial (direct decode, not byte-exact TMEM DMA). Texels
  **are** sampled in the rasterizer (nearest, screen-linear). Formats:
  RGBA16/32, IA16, I4/IA4, CI8/CI4.

---

## 🔲 TODO — render fidelity (the frontier)

Ordered by leverage, from ground-truth on the reachable file-select 3D scene
(`/tmp/fn64-depth-nodepth-opaque.png`: recognizable green field + flowers +
road, but the top half misprojects).

1. **Large-world projection — ROOT-CAUSED to a recompiler-output bug, NOT the
   render crate (2026-07-16).** The render pipeline is CORRECT — proven 3 ways:
   `read_mtx` is byte-identical to RT64 `FixedMatrix::fixedToFloat`; the MVP
   compose matches RT64; the recompiled `guLookAtF` provably negates
   (`neg.s` + dot stores). The Hyrule Field frame (VI swap ~230) projects only
   ~8.9% in-frustum because the matrix written to the projection slot (rdram
   0x1888c8) is a **raw camera-to-world matrix** carrying the raw eye position
   `(3263,694,5674)` in its translation row, instead of a `guLookAt` view whose
   translation must be `-(eye·basis) = (6496.7,-786,-711)`. A temporary
   corrective transform took the frame **8.9%→76.5% in-frustum** and rendered a
   **coherent, recognizable Hyrule Field** (`/tmp/fn64-lw-after.png`: green
   field, horizon, Lon Lon Ranch, trees, fence) — proving the pipeline is right
   once the matrix is. The true fix is UPSTREAM in **aki-recomp's recompiled
   runtime** (a matrix-WRITE bug — needs rdram-0x1888c8 write-tracing to find
   which recompiled fn writes raw eye instead of the guLookAt result). This is
   very likely a **recompilation correctness bug** — folds into the whole-ROM
   native-recomp effort (a mistranslated float/matrix op). Regression test
   `large_world_perspective_view_model_projects_in_frustum` landed on
   `fix/projection-largeworld-mvp` (fail-against-bug proven); the render crate
   itself needs NO fix.
2. **G_SETOTHERMODE_L/H** — currently *not even decoded* (name-table only). No
   blend/render-mode/alpha state exists. Gates alpha-test + blending.
3. **Alpha-test** (alpha compare) — fixes black-box-around-cutouts on
   grass/trees/grates.
4. **Alpha blending** — translucent water/fog/UI (blender currently always
   overwrites).
5. **G_SETCOMBINE + G_SETPRIMCOLOR/G_SETENVCOLOR** — combiner hardwired to
   texel×shade MODULATE; real CC formula + prim/env colors ignored (STUB).
6. **G_SETSCISSOR** clip + perspective-correct S/T & depth (HUD split, floor
   swim).

### Partial / loose ends
- G_GEOMETRYMODE partial (only cull+lighting bits act); G_MOVEMEM/MOVEWORD
  partial; G_SETZIMG/SETCIMG/TEXRECT stubs.
- Process-exit teardown panic (`_Fault_ThreadEntry`/`panic_cannot_unwind`
  during executor drop) — pre-existing, audio-independent, cosmetic.

---

## 🔲 Whole-ROM native recompile (task #28 — the "recomp done" milestone, IN PROGRESS)
fn64-recomp-native (from-scratch Rust MIPS→typed-Rust recompiler) can now recompile
the WHOLE OoT ROM. Driver `recompile_rom` + config loader (`fn64-recomp/src/load.rs`,
loads all 472 sections / 13,358 fns from oot.toml+dump.toml) landed on
`feat/native-whole-rom-driver`. Gap report over the full ROM:
- **13,188 clean (98.73%)**, 99.06% compilable. Emitted funcs.rs = 122MB / 2.44M lines
  of typed Rust (the whole ROM). 0 ROM-range errors.
- FPU conversion gaps (FLOOR.W/CEIL.W/ROUND.W) FOUND + CLOSED with oracle tests.
- 45 runtime-traps (cop0/break/tlb/eret — libultra/OS fns; should defer to fn64 shims,
  not the panic-bodies) + 124 config-stubs.
- 1 unknown-opcode left = `rspbootTextStart` (an RSP blob mislisted as CPU — config-stub).
- **The ONE real link blocker: the `lookup(u32)->fn` indirect dispatcher** (2,078 call
  sites, empirically the ONLY undefined symbol). = N64Recomp's `get_function(vram)`.
  Building it (branch `feat/native-lookup-dispatcher`) is what makes the whole module LINK.
Remaining to "OoT boots on native Rust": dispatcher → link (0 undefined) → shim-seam for
the 45 OS fns → boot on funcs.rs instead of the N64Recomp C files.

## 🔲 Beyond OoT (deferred until it renders faithfully)
- Generalize the pipeline: fn64 owns discover→decomp→recomp→run generically
  with a plugin architecture; absorb aki-recomp's game-specific logic into
  fn64-discover/decomp. Land the OoT proof first.

## Open branches (not yet merged into main)
- `fix/depth-verify-oot` (eec2ac0) — depth regression tests + projection
  instrumentation, no fix (depth was already correct). Safe to merge.
- aki-recomp `fix/rsp-aspmain-base-and-endianness` — local-only, keep checked
  out for oot-boot to build.
