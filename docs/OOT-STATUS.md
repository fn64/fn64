# OoT bring-up on fn64 — done / todo map

OoT (NTSC 1.0) is fn64's correctness oracle. This is the durable status map.
Every "done" here is verified (byte-exact test, or an actual frame/PCM looked
at) — not a tracker label. Updated 2026-07-19.

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
  4,200+ VI swaps, file-select/new-file creation and controllable gameplay
  reachable in the rs lane).
- Clean-room libultra shim layer (public headers/manuals only), DMA
  word-swizzle, OSTask dispatch, thread/queue model.
- Windowed harness (`fn64-shell`, winit+pixels+cpal): live framebuffer per
  swap, keyboard→controller, audio-out wired.
- Fast loop: `--release` + `OOT_MAX_SWAPS` early-exit (~250x), `./oot` runner,
  observability flags (`OOT_RENDER_DUMP_START`, `FN64_DUMP_PROJ`, `FN64_NO_DEPTH`,
  `FN64_AUDIO_UCODE_TIMING`, `FN64_SKIP_AUDIO_UCODE`, `OOT_STOP_ON_FRAME`).
- Rs-lane real-time profiling: `OOT_PERF_NO_CAPTURE=1` removes only the
  harness's per-swap and reference backend's per-task diagnostic PNG work,
  while `FN64_PHASE_TIMING=1`
  attributes wall time to the executor, software renderer, and audio dispatch.
  Differential tracing is opt-in with `OOT_TRACE=1` (and remains crash-safe,
  flushing every event); `./oot trace` enables it automatically. Normal runs
  do not synchronously format, write, and flush every executor event.

### Real RSP/DPC first-frame gate (2026-07-19)

The C-lane boot now reaches its first presented swap at executor step 445 after
28 graphics tasks through the real rspboot/LLE-to-DPC path. Twenty of twenty
independent bounded runs reached that same clean exit without a runtime or
renderer panic. DPC submission ordering is fixed at `CMD_END`: the runtime
captures the bounded command words then stages that immutable image at the
physical 8 MiB boundary outside guest RDRAM for renderer dispatch. A later
guest store or RSP DMA therefore cannot rewrite a command stream that was
already submitted.

### Rs-lane gameplay wall time (2026-07-16, Apple Silicon macOS)

Release-mode samples over controllable gameplay swaps 4200--4300, using the
scripted input preset and `OOT_PERF_NO_CAPTURE=1`, measured 3.332 ms minimum,
3.788 ms median, 3.899 ms p95, and 4.038 ms maximum (3.781 ms mean). That is
comfortably inside the NTSC 16.7 ms VI budget. Before making differential
trace capture opt-in, the same range measured 7.705/9.287/9.748/10.655 ms
(min/median/p95/max; 9.195 ms mean): synchronous formatting, writing, and
flushing of 12,503,295 diagnostic events consumed 58.1% of an Apple `sample`
profile. Suppressing repeated per-frame copies of identical skipped-opcode
warnings reduced renderer diagnostics from 57,896 lines to 736 without
changing rendered output.

Coarse post-change attribution through swap 4300 was 11,298.579 ms in the
`ReferenceBackend` software renderer and 7,392.424 ms in recompiled guest CPU
plus executor overhead. A post-change sample attributed approximately 35.8%
of samples to the renderer (24.1% in rasterization), with guest gameplay code,
especially collision queries, the largest remaining sink. That historical
measurement predated the rs manifest's translated-audio integration and
used `FN64_SKIP_AUDIO_UCODE=1`.

After merging the perf work with live audio, the same skip-audio performance
configuration measured 3.468 ms minimum, 3.867 ms median, 4.071 ms p95, and
4.230 ms maximum over swaps 4200--4300 (3.874 ms mean). A separate run over
the identical gameplay window with translated audio enabled measured 4.654 ms
median (4.695 ms mean), ran all 11,647 submitted audio tasks, and delivered
14,859,648 AI samples, including 12,108,966 nonzero samples in the bounded
range `-29,727..=29,376`. Audio ucode accounted for 5,288.28 ms total, or
0.454 ms per task. Both configurations remain comfortably inside the NTSC
16.7 ms VI budget.

The current C/rs framebuffer **observation** is byte-identical through swap 60.
Both boot harnesses now advance virtual time only after guest quiescence; an
earlier swap-10 mismatch was caused by the retired 100-resume pacing policy
observing the rs lane's denser host checkpoints, not by guest or renderer
semantics. It is not an authoritative parity horizon. The mechanical body audit
finds 116 callable empty C bodies with nonempty Rust counterparts and therefore
rejects C arbitration from swap zero. A historical deeper observation first
diverged around gfx task 232/framebuffer 234, but does not prove the missing
bodies were irrelevant earlier. See `PARITY-METHOD.md`.

### Recompilers (both from-scratch, typed Rust, no external tool, no GPL)
- **CPU** `fn64-recomp-rs`: MIPS III + COP1/FPU + 64-bit dword + COP0 +
  ELF/symbol front-end. Oracle-validated (differential vs N64Recomp C).
- **CPU whole-ROM link gate:** OoT emits as one typed-Rust module with 13,190
  recompiled functions, a sorted safe `vram -> fn` table, and 43 trap bodies held
  behind a host resolver. A clean out-of-tree build links and calls the recompiled
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

### Rust-recompiled deep boot (2026-07-16, `fix/native-boot-deeper`)

- **Required one-command emission now consumes its sibling profile.** Before
  this fix, invoking `recompile_rom --config games/OOTU/oot.toml ...` without
  a redundant `--profile` emitted only 13,306 clean functions and omitted
  `AudioHeap_ResetStep` from recompiled dispatch. Boot stopped after 12 swaps at
  lookup `0x800B4EB4`. The body is ordinary game code: ROM PCs `0x800B4F60`
  (`018B001A`, `div $zero,$t4,$t3`), `0x800B4F6C` (`15600002`, nonzero-divisor
  guard), and `0x800B4F74` (`0007000D`, guarded `break 7`) implement decomp
  `src/audio/internal/heap.c:868-927`'s `2 / sp24` reset state machine. The
  vetted `force_recompile` evidence already existed in sibling
  `games/OOTU/profile.toml:156-175`; `recompile_rom.rs` now discovers that
  sibling when no explicit profile/env override is supplied. The unchanged
  command emits 13,324 clean functions and clears this rung to swap 231.
- **A newly loaded overlay replaces the old image at the same runtime base.**
  The pause and player overlays successively DMA from ROM `0x00BB11E0` and
  `0x00BCDB70` to the same Kaleido arena `0x80388B60`. Keeping both registry
  entries loaded made runtime player callback `0x8039D788` canonicalize through
  stale pause static base `0x808137C0`, yielding interior pause PC `0x808283E8`
  instead of `Player_Init` `0x80844DE8`; rs lane stopped after swap 231.
  Resident-ROM PCs `0x80097658`/`0x80097660` contain words
  `3C048084`/`24844DE8`, constructing that static `Player_Init` address, and
  `0x800976E8` contains `0320F809` (`jalr $t9`) to consume its relocated
  result. The generated C preserves the same sequence at
  `RecompiledFuncs/funcs_36.c:8685-8693,8779-8788`.
  Decomp `src/code/z_kaleido_manager.c:20-33,85-112` establishes the shared
  arena/load-offset mechanism, `src/code/z_kaleido_scope_call.c:18-39` shows
  player replacing the current overlay, and `src/code/z_player_call.c:41-51`
  consumes the relocated callback. `SectionRegistry::set_section_loaded_at`
  now evicts any displaced mapping at the destination before publishing the
  new section; `shared_runtime_base_replaces_prior_overlay` fails against the
  stale-two-images behavior.
- **Verified depth:** 10 consecutive release probes, each with
  `OOT_MAX_SWAPS=250 FN64_SKIP_AUDIO_UCODE=1`, reached 250 VI swaps / 250 gfx
  tasks and exited 0 with no recompiled execution panic. Swap 250 is a non-uniform
  title-demo/Hyrule Field framebuffer (`/tmp/fn64-deep-frame.png`), though the
  known renderer-state gaps below leave it mostly red with dark geometry. A
  collision-free C-lane probe also reached swap 250, and its swap-250 PNG is
  byte-identical (SHA-256
  `a0b354ea3c7056e90f316bc28f24d2c46761ce248b3279b9be1b6a21c320cc6b`, no test checks it).

### Rust-recompiled interactive boot (2026-07-16, `fix/native-boot-interactive`)

- **The verified controller route is now a harness preset.**
  `OOT_SCRIPT_INTERACTIVE=1` applies `START` at swaps 250/280; `A` at
  360/400; `START` at 420; and `A` at 440/490/540, with every named press
  released four swaps later. This creates/selects file 0 and enters normal
  Play at swap 568. From swap 620 it holds stick X=60 and taps `A` for two
  swaps every 25 swaps from 700 through 4150 to advance the opening dialogue
  and cutscenes. The C lane and rs lane follow the same title/file-select
  states through select-mode 1 at swap 499. The configured C oracle stops
  there because `games/OOTU/oot.toml` deliberately stubs
  `FileSelect_MoveSelectedFileToTop`; the rs profile recompiles its real
  body (ROM/static vram `0x80810A1C`) and reaches select-mode 2 at swap 500.
  The two swap-499 framebuffer PNGs compare byte-for-byte equal (SHA-256
  `c54906136189fde8b59b853d3b2f74fc75d7f77753c495d7110e9b950bfdd85e`, no test checks it).
- **Partially overlapping overlay allocations now replace stale images.**
  The first long scripted transition previously trapped nondeterministically
  after swap 1270 at lookup `0x80B2CBB0`, an interior address in
  `EffectSsDust_Draw`, while spawning fairy sparkles through
  `EffectSsKiraKira_SpawnDispersed`. The section dump identifies Dust as ROM
  `0x00EA80B0`, static vram `0x80B2C980`, size `0x740`, and KiraKira as ROM
  `0x00EA88E0`, static vram `0x80B2D1B0`, size `0x5C0`. Decomp
  `src/code/z_effect_soft_sprite.c:182-254` loads the selected overlay and
  invokes `profile->init`; KiraKira's profile names that function at
  `src/overlays/effects/ovl_Effect_Ss_KiraKira/z_eff_ss_kirakira.c:36-47`.
  ROM `0x00EA82E0` contains words `27A40134 27A60074 0C023B6E 00A12821` at
  the wrong `0x80B2CBB0` Dust interior PC; exact KiraKira entry ROM
  `0x00EA88E0` contains `AFA40000 AFA50004 8CEF0000 3C0100FF`. Therefore the
  callback must resolve to `0x80B2D1B0`. `SectionRegistry::set_section_loaded_at` used to
  evict only an old mapping with the same runtime base, leaving a stale image
  when the arena reused a partially overlapping range at a different base.
  It now evicts every overlapping half-open runtime range before publishing
  the new mapping. Regression
  `partially_overlapping_runtime_range_replaces_prior_overlay` uses the exact
  ROM/static section geometry and fails against the former exact-base rule.
  Classification: **overlay-bank mapping**.
- **Verified depth:** after the overlay fix, 10/10 consecutive clean release
  probes reached swap 1400 (the former failure window). Longer probes reached
  swap 3000, 4200, and 5200 with no recompiled trap. The opening sequence returns
  to Link's House and releases cutscene control at swap 4196. With stick X=60
  already held, Link moves from approximately `(0, 0, 60)` at swap 4195 to
  `(18, 14, 128)` at swap 4212 before meeting the room collision boundary.
  This is a live, controllable `PlayState`, not merely a surviving cutscene.
  `OOT_RENDER_DUMP_START=N` suppresses early diagnostic PNGs so this late
  gameplay window can be captured without dumping thousands of boot frames.

### Live rs-gameplay audio (2026-07-16, `feat/audio-in-gameplay`)

- **Real tasks flow and run real aspMain.** The rs manifest now links the
  out-of-tree generated 1,004-instruction OoT aspMain through a build-only
  adapter. The adapter copies the generated module from sibling `aki-recomp`
  into Cargo `OUT_DIR`; no game-derived bytes/code enter this repository. A
  scripted rs run reached controllable gameplay and swap 5,000 with
  13,747 `M_AUDTASK` submissions / 13,747 timed aspMain calls (6,055.76 ms
  total, 0.441 ms/call average).
- **The live PCM is nonzero and bounded.** Across that run, OoT submitted
  14,006 AI buffers containing 17,480,448 signed samples; 14,728,612 were
  nonzero and the aggregate range was `-29,727..=29,376`. The first non-silent
  `FN64_DUMP_AUDIO_PCM` capture is 1,248 samples / 624 stereo frames, 1,224
  nonzero, range `-10,985..=11,035`, RMS 4,870.53. It is plausible waveform
  data, not all-zero output or full-scale garbage.
- **The former output route was byte-disproved and fixed at the AI boundary.**
  The first captured live task is at RDRAM offset `0x120C90`: native-endian
  words show `type=2`, `output_buff=0`, `output_buff_size=0`,
  `data_ptr=0x801A58C0`, `data_size=0x1E0`; its command list contains real
  opcode-`0x15` `A_SAVEBUFF` destinations. Therefore `OSTask.output_buff`
  cannot feed host audio. The public libultra manual's
  **osAiSetNextBuffer** section names the completed PCM address/byte length;
  that shim now decodes fn64's `addr ^ 2`
  native-word halfwords in guest order and calls `AudioBackend::queue_samples`.
  The exact zero-output-field task followed by a real AI submit is locked by
  `os_ai_set_next_buffer_routes_live_pcm_to_the_registered_audio_backend`.
- **Verification bar:** 10/10 consecutive rs-lane swap-300 probes each reached
  300 swaps, ran exactly 200 audio tasks, and produced exactly 541,952 samples
  (175,857 nonzero, range `-23,166..=23,449`). The AI-to-backend regression
  also passed 10/10 consecutive runs.
- **Physical cpal playback is not verified on this machine.** Both cpal's
  `default_output_config()` and `supported_output_configs()` fail before
  stream creation with CoreAudio OSStatus `0x216F626A` (`!obj`), and every
  candidate-rate stream attempt fails identically. The rs harness and
  `fn64-shell` register the same `CpalBackend`; when a stream opens, the now-
  verified AI boundary feeds it. In this environment zero live buffers reached
  cpal, so claiming audible device output would be false.

### Render — geometry & texture layers
- Geometry: G_VTX, G_MODIFYVTX, G_TRI1/TRI2/QUAD, G_MTX (LOAD/MUL/PUSH),
  counted G_POPMTX, G_CULLDL (retained six-plane clip codes), compound
  G_RDPHALF_1/G_BRANCH_Z (exact screen-depth tail branch), G_DL (call/branch,
  recursion-limited), G_ENDDL, and typed G_LINE3D with public width,
  homogeneous clipping, shade/texture/scissor/blender state, and read-only Z
  — **implemented**. Exact line edge coefficients remain hardware-trace work.
- **Depth / Z-test: implemented AND verified correct.** Viewport-mapped NDC-z
  (`sz=tz=127.75` → screen-z [0,255.5]), nearer wins (`z < depth[pix]`),
  rejects ~124k farther fragments/frame, 42% pixel delta vs painter's-order.
  Fail-against-bug regression tests in `raster.rs`. (Uses an internal z-array,
  not G_SETZIMG — functionally correct.)
- Textures: G_SETTIMG/SETTILE/SETTILESIZE/LOADTLUT/LOADBLOCK/LOADTILE execute
  through a physical 4 KiB TMEM snapshot. Load and render tiles are distinct;
  base/line addressing, odd-row exchange, RGBA32 and YUV split halves,
  quadricated RGBA16/IA16 TLUTs, and per-sample CI lookup are modeled. Texels
  use point, four-sample average, or documented three-nearest filtering with
  perspective-correct S/T (`S/w`, `T/w`, `1/w`, then divide). Formats:
  RGBA16/32, YUV16, IA16/8/4, I8/4, CI8/4. Equal low/high fractional load
  bounds preserve subtexel origins, and source-sized transfers support a
  distinct load-descriptor size (including RGBA32 through 16-bit). Unequal
  low/high fractional edge selection remains open; uninitialized TMEM bits
  trap by address. Texture-image/tile/TLUT/TMEM and the other RDP registers now
  survive task boundaries and are shared with raw DPC execution; `G_TEXTURE`
  remains RSP-owned and resets. Enabling it without a live TMEM image traps
  rather than silently substituting white.
- `G_SETSCISSOR` snapshots quarter-pixel `[upper-left, lower-right)` state per
  triangle and intersects it with framebuffer raster bounds at pixel centers.

---

## 🔲 TODO — render fidelity (the frontier)

Ordered by leverage, from ground-truth on the reachable file-select 3D scene
(`/tmp/fn64-depth-nodepth-opaque.png`: recognizable green field + flowers +
road, but the top half misprojects).

1. **Large-world ReferenceBackend artifact — ROOT-CAUSED AND FIXED in texture
   loading, not projection.** Swap-1299 instrumentation separated geometry
   from shading. The first 26 emitted triangles have finite positive clip W,
   legal viewport coordinates, and tessellate the expected 320x240 sky
   rectangle. Rendering those triangles without texture produces the same
   correctly placed full-screen mesh, so neither the matrix transform nor
   raster screen mapping can cause the bands or right-side magenta region.

   The texture command trace proves two independent decoder defects. First,
   live CI8 palette loads encode 256 entries (`count - 1 = 255`), while the
   decoder shifted that field right by another two bits and loaded only 64;
   36,166 such commands were observed through swap 1299 (and 11,759 16-entry
   loads were likewise truncated to four). CI8 indices beyond the short vector
   became the decoder's magenta sentinel—133 of the sampled 32x32 diagnostic
   tile's texels—accounting for the magenta region directly. The public
   libultra `gDPLoadTLUTCmd` layout stores `count - 1` directly at bits 14..23;
   it is not an S/T quarter-texel field.

   Second, the sky is a 128-texel-wide CI8 source loaded as successive 32x32
   `G_LOADTILE` rectangles at distinct ULS/ULT positions. The old direct
   decoder addressed every rectangle as `ty * tile_width + tx`, ignoring both
   the `G_SETTIMG` width and load origin, so every sky patch reread the source's
   top-left rectangle. That repeated rectangle is the horizontal banding. The
   public `gDPSetTextureImage`/`gDPLoadTile` layouts instead require
   `(source_y + ty) * image_width + source_x + tx`; sampling then subtracts
   the render tile's ULS/ULT origin from vertex S/T. The decoder now preserves
   that image width and both origins, decodes the complete TLUT, and traps on
   an out-of-range palette/sample invariant instead of painting magenta.
   Synthetic regressions `load_tlut_count_uses_all_ten_wire_bits` and
   `load_tile_uses_settimg_stride_and_tile_coordinate_origin` reproduce the
   old failures without game data. The corrected swap-1299 dump is a continuous
   blue-to-dawn sky gradient with cloud/horizon detail and no magenta third;
   coarse ReferenceBackend texture/raster detail and absent terrain remain
   later fidelity work, not evidence of the former decoder failure. Ten of ten
   consecutive release probes reached swap 1300 at deterministic step 170788
   with rc=0 and byte-identical dumps (SHA-256
   `586eae90fe194222fe149f4450626df4bc2ca07de812a3353bc065c0551d4df8`, no test checks it).

   The earlier claimed raw-eye matrix bug remains **falsified by writer tracing
   (2026-07-16).** Physical `0x1888c8` is written only by recompiled
   `guMtxF2L` (`funcs_57.c:3275-3344`), called by recompiled
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
2. **G_SETOTHERMODE_L/H + threshold alpha compare: implemented.** F3DEX2 masked H/L
   updates now produce typed cycle/filter/dither/render/Z/coverage/blender
   state, snapshotted per triangle. `G_AC_THRESHOLD` compares post-combiner
   alpha with `G_SETBLENDCOLOR.a`; rejected fragments leave both color and
   depth untouched. The public Programming Manual defines `G_AC_DITHER` as a
   hardware-generated pseudo-random threshold; the Rust reference path now
   uses the same typed per-fragment byte as combiner and dither noise rather
   than substituting a screen-locked Bayer threshold.
   Ordered one/two-cycle dither is implemented: RGB MagicSquare/Bayer modifies
   low color bits before target-format storage, and alpha Pattern/InversePattern
   uses the selected ordered matrix. RGB Noise, alpha Noise, and `G_AC_DITHER`
   use an explicit seedable SplitMix64 reference policy whose exact hardware
   sequence remains unclaimed. The
   disabled path retains truncation behavior rather than rounding. The
   fail-against-bug tests cover state carry, the OoT render-mode macro's
   embedded alpha-dither bits, a transparent cutout texel, depth preservation,
   ordered tables, shared-noise routing, and seeded reproducibility. The bounded C-file boot reached 250 swaps and
   produced a changed actual frame sequence. The eyes-on dump proves corrected
   alpha coverage rather than a finished scene.
3. **Alpha blending: implemented and unit-verified; live visual exercise is
   pending.** Both full and partial other-mode writes feed per-triangle
   `GBL_c1`/`GBL_c2` state. The raster pipeline composites `P*A + M*B` over
   the framebuffer only after combiner, alpha compare, and depth test. The
   bounded boot depth used for this merge may not visibly exercise a
   translucent surface, so an eyes-on blend-specific scene remains TODO.
4. **Scissor and perspective-correct S/T: implemented and unit-verified;
   live visual exercise is pending.** `G_SETSCISSOR` clips pixel-center raster
   bounds to its quarter-pixel, exclusive-lower-right rectangle. Textured
   triangles interpolate `S/w`, `T/w`, and `1/w` before sampling. Depth
   interpolation remains screen-linear, and the later human eye-gate must
   still inspect HUD splits and glancing-angle floor texture stability.

### Partial / loose ends
- ~~`G_DL recursion exceeded MAX_DL_DEPTH (10)` spam during field render~~
  FIXED (2026-07-16, fn64#2): gsSPBranchList is a tail jump that consumes no
  return-stack entry, but the decoder recursed and counted it against the
  call cap, silently dropping every branch chain deeper than 10. Branch now
  reassigns the DL pointer; the call (G_DL_PUSH) cap is F3DEX2's 18. Exceeding
  that stack or the 2^20-command whole-decode budget is now a named corrupt-DL
  trap rather than a plausible partial render. Evidence: deterministic
  1300-swap reference-backend A/B 4,207 warnings -> 0 with identical boot
  progression; 10/10 consecutive clean release runs post-merge. Note: this
  decoder runs only under the ReferenceBackend (the oracle/fallback) — RT64
  walks display lists in its own code.
- G_GEOMETRYMODE consumers remain partial; G_MOVEMEM/MOVEWORD implement
  viewport, both LookAt directions, light, active-light-count, segment, and
  both public light-color copy destinations. Signed fog factors generate
  vertex shade alpha from projected depth. Regular and inverse-cosine texture
  generation replace explicit vertex coordinates from projected normals and
  the public `gSPTexture` scale. The public two-command `gSPForceMatrix` loads
  an already-concatenated transform without corrupting the underlying stacks;
  a later ordinary matrix command supersedes it. Perspective normalization is
  retained across ucode loads and an explicit zero rejects geometry. Exact microcode
  clipping ratios are retained per side and applied to line geometry without
  changing `G_CULLDL` codes. `G_DMA_IO` now transfers aligned logical bytes
  between RDRAM and the device fabric's persistent DMEM/IMEM in decode order;
  a write can rewrite the next command in the same display list. Compound
  `G_LOAD_UCODE` now copies its physical data/text sources into live DMEM/IMEM
  before later commands and applies the public F3DEX2 maintained-state list;
  the DL and matrix stacks, modelview/projection, segments, viewport, scissor,
  other mode, and perspective normalization survive the reload. Task-entry and
  self-loaded text must each match an explicitly registered 4 KiB SHA-256;
  selecting the F3DEX2 decoder no longer admits the live image. An unknown
  generation discards speculative clone state and replays
  the whole ucode phase through LLE from untouched post-rspboot memory and the
  typed scalar/VU/SP/DMA/DPC snapshot instead
  of being guessed F3DEX2. RT64 accepts the resulting bounded raw DPC ranges
  through its LLE RDP entry. Public RDP
  tagged no-op and RSP no-op commands are explicit, while all three reserved
  `G_SPECIAL_*`, non-public move subindices, and unknown opcodes trap with
  wire context. Truncated command pairs, malformed vertex/triangle ranges,
  malformed other-mode bit ranges, and incomplete vertex/matrix/viewport/light
  DMA also fail before partial or stale state can escape. Exact microcode subdivision/rounding/trigonometric lookup
  remains a hardware-trace frontier. The Rust
  reference lane now preserves
  SETCIMG/FILLRECT/TEXRECT/FULLSYNC in command order, executes persistent
  RGBA16/RGBA32 color targets and format-correct fill-cycle rectangles, and
  requires that persistent RDP target before F3DEX2/raw color writes instead
  of borrowing the VI/output address. Each task re-imports that target from
  RDRAM, preserving intervening CPU-visible changes to untouched pixels. It implements the public
  non-flipped RGBA16 copy-cycle TEXRECT path, including the alpha-bit write
  enable used by threshold compare. One/two-cycle TEXRECT and
  TEXRECTFLIP now share the triangle texture filter, combiner, alpha compare,
  and blender; point, four-sample average, Nintendo's documented
  three-nearest triangular filter, and distinct TEXEL1 execute. Programming
  Manual Chapter 13.7 mip/detail/sharpen LOD selection now uses immutable
  eight-tile snapshots, adjacent perspective-corrected derivatives,
  minimum/maximum levels, modulo-eight tile selection, and the LOD_FRACTION
  combiner input across rectangles, F3DEX2 triangles, and raw RDP triangles.
  Copy-cycle TEXRECTFLIP now uses the public S/T screen-axis swap. Bounded raw
  DPC state/fill/texture ranges also reach the reference executor through the
  shim and MMIO paths. YUV16 shared-chroma loads, all six `G_SETCONVERT`
  fields, the three public conversion/filter modes, and K4/K5 combiner inputs
  now share those raw and high-level texture paths. `G_SETKEYR`/`G_SETKEYGB`
  and `G_CK_KEY` also execute the public soft-edge chroma alpha-fixup equation
  and feed alpha compare. Shade-dependent rectangle
  programs, exact LOD derivative norm/fixed-point boundaries, arbitrary nonzero-DeltaZ depth-fill packing,
  and silicon-exact conversion/filter/
  coverage arithmetic remain loud gaps.
- G_SETCOMBINE + primitive/environment RGBA are implemented for the common
  OoT modulate, decal/replace, primitive-tint, environment-blend, and
  shade-only source set. TEXEL1 is distinct for rectangles and both triangle
  paths; a missing tile+1 or LOD-selected tile traps instead of aliasing.
  LOD_FRACTION is modeled. The hardware NOISE combiner source consumes the
  typed per-fragment byte instead of substituting black; the exact silicon
  generator remains a hardware-trace frontier.
- Bounded probes in BOTH lanes use `_exit(0)` after explicitly flushing the
  summary/trace (2026-07-16; was rs-lane-only, so C-lane probes aborted with
  exit 134 in TLS teardown after a clean summary and probe exit codes were
  untrustworthy). Suspended coroutines can be stopped inside an existing
  `extern "C"` blocking shim, where TLS teardown's forced unwind cannot cross
  the ABI boundary. This is harness teardown only; execution still uses the
  same single executor and host thread. Exit 0 is now the probe-success
  signal for both lanes.
- Rs-lane boot now reaches the bounded **250 VI swaps and 250 render tasks**
  in 10/10 consecutive release probes; swap 3 is the first non-uniform guest
  framebuffer. The former `AudioLoad_Dma` unaligned-`SW` frontier was a stale
  host-call return, not an alignment exception or an SWL/SWR decode:
  `AudioLoad_Init` preserves
  `osCartRomInit()` in `gAudioCtx.cartHandle`, but the shim had left `$v0`
  untouched. It now returns OoT's aligned guest `__CartRomHandle` address.
  The first post-fix probe exposed a local `jr` jump-table target inside
  `AudioSeq_SequenceChannelProcessScript`; Rust emission now gives every
  instruction a dispatch arm in functions containing a non-return `jr`.

---

## ✅ Whole-ROM Rust recompile + rs boot (task #28)
fn64-recomp-rs (from-scratch Rust MIPS→typed-Rust recompiler) can now recompile
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
The rs lane selects the emitted `oot-recompiled` crate with
`FN64_RECOMP=rs` and skips the C compiler/section bridge. The crate owns
64 balanced generated submodules and is linked as an rlib dependency rather
than textually `include!`d into the boot executable, so Cargo can cache it
independently. `fn64-abi::recompiled` explicitly marshals typed
GPR/HI/LO/COP0 status at the already-unsafe host boundary, while every spawned
OSThread stays on the existing executor/coroutine machinery and shared RDRAM.
The crate exports section geometry so the existing DMA-driven
`SectionRegistry` can canonicalize relocated overlay callbacks.

The emitted-crate build was measured on the same machine and clean release
target as the former 139,728,579-byte textual include: **380.73 s before,
110.26 s after** (3.45x faster; the cached rebuild is 0.08 s). Both preserved
executables reached swap 499 with exit code 0 and emitted byte-identical
307,528-byte framebuffer PNGs (SHA-256
`f029be05de404a8a5eedb1944069c9eb844c7daac66de0f430f1f15468ec4cbc`, no test checks it). The
crate-linked executable has no unresolved fn64/recompiler/libultra symbols.

The current profile-aware/guard-swept result is **13,324 clean + 25
host-bound = 13,349/13,358 (99.93%) linkable**, with nine genuine config
stubs. The AudioLoad repair is grounded in OoT decomp
`src/audio/internal/load.c` (`AudioLoad_Init` and `AudioLoad_Dma`) and
`src/libultra/io/cartrominit.c`: guest code dereferences the returned public
handle, so the host ABI must return the game-linked BSS object rather than an
opaque token or stale register. Ten consecutive release probes now reach
swap 250 with exit code 0; swaps 3-250 have non-uniform guest framebuffers. No
new loud recompiled frontier appeared through that bound. The current C/rs
framebuffer observation is byte-identical through swap 60 after both harnesses
adopted guest-quiescence timing. The callable-body audit rejects the legacy C
lane as a semantic arbiter independently of that output match; use
`scripts/lane-parity.sh --observe 60` for the explicitly weaker check. The prior
swap-10 trace difference was caused by advancing virtual time after 100 resumes
despite unequal host-checkpoint density; it is retained only as a superseded
harness finding.

## 🔲 Beyond OoT (deferred until it renders faithfully)
- Generalize the pipeline: fn64 owns discover→decomp→recomp→run generically
  with a plugin architecture; absorb aki-recomp's game-specific logic into
  fn64-discover/decomp. Land the OoT proof first.

## Open branches (not yet merged into main)
- `fix/depth-verify-oot` (eec2ac0) — depth regression tests + projection
  instrumentation, no fix (depth was already correct). Safe to merge.
- aki-recomp `fix/rsp-aspmain-base-and-endianness` — local-only, keep checked
  out for oot-boot to build.
