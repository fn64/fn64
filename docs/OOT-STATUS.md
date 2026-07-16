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
- **RSP audio** `fn64-audio`: all 44 canonical non-reserved VU compute ops,
  all 23 manual vector loads/stores, the exact 48-op SU subset, COP0, and
  general delay-slot/indirect-jump/overlay dispatch. aspMain
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

1. **Projection on large-world scenes (SECOND projection bug).** The
   file-select scene projects only ~8.9% of verts in-frustum; `pz` swings
   ±4000, many with negative `w`, when camera (~world 3263,694,5674) and
   objects (~-4000) both carry large translations. The transpose fix handled
   simple frames; this doesn't. Prime suspects: fixed-point `read_mtx`
   int-half precision for large translation values, or MVP accumulation with
   big camera offsets. **Highest leverage — texturing garbage geometry buys
   nothing.**
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

## 🔲 Beyond OoT (deferred until it renders faithfully)
- Generalize the pipeline: fn64 owns discover→decomp→recomp→run generically
  with a plugin architecture; absorb aki-recomp's game-specific logic into
  fn64-discover/decomp. Land the OoT proof first.

## Open branches (not yet merged into main)
- `fix/depth-verify-oot` (eec2ac0) — depth regression tests + projection
  instrumentation, no fix (depth was already correct). Safe to merge.
- aki-recomp `fix/rsp-aspmain-base-and-endianness` — local-only, keep checked
  out for oot-boot to build.
