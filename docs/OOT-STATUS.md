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
- **CPU whole-ROM link gate:** OoT emits as one typed-Rust module with 13,190
  native functions, a sorted safe `vram -> fn` table, and 43 trap bodies held
  behind a host resolver. A clean out-of-tree build links and calls the native
  entrypoint with no unresolved game/project symbol. The dispatcher ABI shape
  follows the MIT N64Recomp `LOOKUP_FUNC`/`get_function` contract
  (`refs/N64RecompSource/include/recomp.h:443-451`).
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

1. **Hyrule Field title-camera projection — the claimed raw-eye matrix bug was
   falsified by writer tracing (2026-07-16).** Physical `0x1888c8` is written
   only by recompiled `guMtxF2L` (`funcs_57.c:3275-3344`), called by recompiled
   `guLookAt` at `funcs_57.c:4368`. Immediately before conversion, recompiled
   `guLookAtF` writes its translation at `funcs_57.c:4166,4251,4280`. The
   traced inputs are eye `(-4000,-1,5228)`, at `(-4083,10,5263)`, and up
   `(0.111461,0.992645,-0.0470212)`, supplied unchanged through
   `Camera_Demo1` (`funcs_15.c:367-379`) → `Camera_Update`
   (`funcs_15.c:13254`) → `View_LookAt` → `View_ApplyPerspective`
   (`funcs_35.c:2661`). For that eye and the emitted basis, the three negated
   dot products are exactly `(3262.99,694.05,5674.78)`. The matrix maps the
   traced eye to the view-space origin, so it is a valid `guLookAt` view—not a
   camera-to-world matrix. The proposed `(6496.7,-786,-711.7)` rewrite computes
   `-translation·basis`, treating the existing translation as a second eye;
   it moves the camera and therefore cannot validate this path. Its higher
   76.5% loaded-vertex in-frustum ratio and recognizable overview are an
   alternate camera view, while 8.8% is not by itself a correctness oracle
   because the display list loads off-screen geometry. Regression
   `hyrule_field_live_gu_look_at_translation_matches_traced_eye` locks the
   exact traced invariant and fails under that rewrite. The remaining visual
   artifact is **not yet root-caused**; do not resume from the discarded
   raw-eye premise.

   The source-to-generated-code cross-check agrees at every boundary:
   `Camera_Demo1` copies the spline eye into `camera->eye` at
   `z_camera.c:5860-5884` (`funcs_15.c:367,375,379`); `Camera_Update` passes
   the derived eye/at/up to `View_LookAt` at `z_camera.c:8259-8265`
   (`funcs_15.c:13254`); `View_LookAt` copies those values into `View` at
   `z_view.c:84-92` (`funcs_35.c:1122-1159`); and
   `View_ApplyPerspective` calls `guLookAt` and submits that matrix at
   `z_view.c:371-406` (`funcs_35.c:2661`). The expected float writer is
   `lookat.c:39-57`: its three translation expressions at lines 42, 47, and
   52 correspond to `funcs_57.c:4166,4251,4280` and produce
   `(3262.99,694.05,5674.78)`. The wrapper at `lookat.c:60-65` then calls the
   expected fixed-point writer `guMtxF2L` (`funcs_57.c:4368`), whose stores at
   `funcs_57.c:3275,3281,3340,3344` implement the packing loop in
   `mtxutil.c:3-19`. Finally, `z_play.c:1173-1188` reads the already-written
   viewing matrix to derive separate billboard data; it does not overwrite
   the projection-stack view slot.
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
- Native-Rust boot currently executes the typed OoT entrypoint through the
  dispatcher and reaches the first host-owned boundary, `__osGetSR` at
  `0x80003430`. Both direct JAL and computed JALR calls to omitted trap bodies
  take `lookup(vram)`, which asks the thread-local host resolver before its
  native table and otherwise fails loudly. Completing this path requires safe
  typed adapters for the missing COP0/exception services (starting with
  `__osGetSR`), sharing the native context/RDRAM model with the executor, and
  selecting the Rust module in `examples/oot-boot` instead of its C bridge.

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
