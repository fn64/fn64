# Roadmap — full Rust decomp/recomp pipeline + runtime

Decided 2026-07-16 (user + session). Three phases; R and D run as **parallel
wave tracks** (disjoint crates). Render endgame this phase: **RT64 as the
faithful renderer, wgpu port deferred to Phase P**. Executor mix:
**codex-heavy implementation waves, session-model adversarial verify + merge
gate** (see DELEGATION.md).

Status legend: `[ ]` open, `[~]` dispatched/in-flight, `[x]` merged+verified
(AGENTS.md bars). Update this file in the same commit as the work it tracks.

## Phase V — the verification gaps (open findings, 2026-07-17)

Found by auditing what the gates actually check. These are first because they
undermine every other item's evidence: a bar that does not measure what it
claims makes all the greens beneath it worth less.

- [ ] **V1a ROOT-CAUSED: fn64's recompiler is RIGHT; the C-lane oracle is
  systematically wrong.** The c/rs divergence (identical through swap 231,
  differs from 234) is not an opcode bug and not the renderer.

  Both lanes read the same `oot.toml`, whose `[patches] stubs` lists **127**
  functions under the rule "any cop0/cache/cop2/**break** opcode -> stub". A
  branch-guarded `break` (a compiler divide-by-zero assert, reachable only on
  pathological input) is perfectly recompilable, so most are FALSE POSITIVES.
  `gen_stubs.py` says so about itself: it calls the rule "a blunt bootstrap
  heuristic" and keeps a hand-maintained `force_recompile` escape hatch for
  whichever cases someone noticed.

  **fn64 already fixed this and the legacy tool did not.**
  `recompile_rom.rs`'s `compiler_div_guards_only()` (:532) disassembles each
  stubbed function and auto-un-stubs it when its only traps are guarded
  div/overflow asserts (`auto_div_guards`, :288-311). N64Recomp has no such
  recovery, so it emits a body with NO statements for `Letterbox_Update`
  (`RecompiledFuncs/funcs_37.c:1654`) while the game calls it (:9090). The
  letterbox feeds `Setup_View`'s `gDPFillRectangle` (decomp `z_rcp.c:1576`):
  rs steps 32->22->12->2->0 (its real `step = 10` + clamp), C is pinned at 32
  forever. That `G_FILLRECT` is the first differing GBI command. Casualties
  also include `Interface_Draw`, `Camera_Normal1`, `KaleidoScope_Update`,
  `Message_DrawMain`, `TitleCard_Draw` — the C lane silently no-ops the HUD,
  camera, and pause menu.

  **Do NOT "fix" aki-recomp for fn64's sake.** Nothing here needs a change in
  the legacy tree: the rs lane is correct, and P1 retires the C lane to
  CI-oracle-only. The real cost is to METHOD, not to output —

- [ ] **V1b DESIGN.md §4's A/B is compromised: the oracle is wrong in a known
  direction.** §4 says diff the lanes and investigate disagreements. But past
  the title screen the C lane is missing bodies for ~127 functions that fn64
  correctly recompiles, so every future disagreement opens with "is this our
  bug, or stub #94?" A differential whose reference is systematically degraded
  cannot arbitrate. Options: (a) run the A/B with `force_recompile` covering
  the auto-un-stubbed set so both lanes carry the same bodies; (b) bound the
  A/B to swaps < 232, where they provably agree, and say so; (c) retire the
  C-lane A/B early and lean on `scripts/lane-parity.sh` only as a regression
  detector, not an arbiter. Until one is chosen, DESIGN.md §4 overstates what
  the A/B can prove.

  Not proven: that no OTHER stub contributes before task 232. With 127 on the
  same footing, others are plausibly live on this path.
- [ ] **V0 doc hashes are now linted; 5 were unbacked.** `lint-docs.py` fails
  when a doc asserts a *content* hash no test contains (commit pins are
  provenance and exempt). All five in OOT-STATUS.md were unbacked — verified
  once by hand, re-checked by nothing, unable to fail and therefore unable to
  warn. Marked honestly for now; gating them is V1's job. The generalization:
  a hash in a doc is either load-bearing (a test owns it) or prose (don't cite
  it as evidence).
- [ ] **V2 the only integration gate asserts liveness, not correctness.**
  `examples/oot-boot/tests/boot_depth.rs:27,91` asserts only
  `swaps >= MIN_EXPECTED_SWAPS` (200). OoT could render pure garbage for 200
  swaps and pass. It is a floor against regression-to-dead, nothing more —
  which is fine, but nothing else covers correctness end-to-end, so the gate's
  green overstates what is known. V1 is the cheapest real correctness signal
  to add beside it.
- [ ] **V4 the delegation loop has no "done, awaiting merge" state.** Found
  2026-07-17: `wave/d2-detector-closure` finished, self-gated green, and sat
  unmerged for 8 hours while the roadmap still listed D2 as open — invisible
  to everyone. `dispatch.sh` launches a job; the job correctly stops without
  pushing (DELEGATION.md keeps supervision with the dispatcher); and then
  nothing surfaces that a branch is READY. The information exists —
  `scripts/wt.sh` shows ahead/dirty/job-alive per worktree — but only if
  someone runs it, and a finished job looks identical to an abandoned one.
  Cheapest fix: have `dispatch.sh` write a completion marker, and teach
  `wt.sh` a READY state (branch ahead of main + no live job + gates recorded).
  Without it, delegated work silently rots at exactly the rate we dispatch it.
- [x] **V3 `fn64-diff` split** (done 2026-07-17) — 1791 -> 405 lines: the
  comparator stays, the faki-tools oracle client and mupen savestate parser
  are gone. AGENTS.md now cites differentials that run.

## Phase R — close the "OoT renders faithfully" gate

The standing milestone gate (OOT-STATUS.md "Beyond OoT"). The renderer lands
faithful frames for title/attract; the open frontier is audio (R5) and the
outdoor gameplay eye-gate (R3b).

Closed-item policy: a finished item is deleted, not archived — git has the
history. What survives below is only what changes what the next session DOES:
negative results (a ruled-out cause someone would otherwise re-investigate)
and scope limits that make an open item meaningful.

- **R2 — the artifact was NOT projection** (closed 2026-07-16). Kept because
  the name misleads: root cause was the ReferenceBackend quartering
  G_LOADTLUT counts and decoding every G_LOADTILE rect from the source-image
  origin (CI palette + tiled source layout). Do not re-open a projection
  hunt. Reference is the oracle; RT64 is the faithful lane.
- **R3 eye-gate PASSED — scope: title/attract camera ONLY** (2026-07-16).
  Kept because it bounds R3b: 7 RT64-lane frames at swaps 400-1300 judged
  faithful by the user. Gameplay cameras are NOT covered by this pass.
- [ ] **R3b outdoor gameplay eye-gate — blocked on the capture route, NOT on
  the renderer.** Indoor scenes (Link's house, Navi cutscene) render
  faithfully. The "missing Kokiri Forest world geometry" was investigated at
  length (R6, 2026-07-16) and the renderer is **exonerated** — do not re-open
  it as a render bug:

  - The scripted route never takes a scene/room transition. Across swaps
    4200-4300 the PlayState stays a live `Play_Main`, room 0 stays loaded and
    unchanged, `load_active=0`, and a PI-DMA trace shows no room/scene load
    after swap 4015. The apparent Kokiri view is not a loaded Kokiri room —
    the script drove Link/camera out of the Link's-house interior.
  - An independent task differential agrees: tasks 4149 (good) and 4289 (bad)
    submit the *same* populated segment-3 room graph; the bad task decodes 737
    triangles (good: 1006). There is no outdoor graph in the input to skip, so
    neither an rs-lane payload handoff nor an RT64 HLE skip can be the cause.
  - The green speckle is secondary: the bad task switches to S2DEX2 and issues
    `G_BG_COPY` from unwritten background data. No-oping just that copy turned
    speckle black without revealing geometry.

  To close: make the capture route deliberately trigger and verify an exterior
  transition, then re-gate. Evidence: `FN64_GFX_TASK_DUMP`, `OOT_STATE_TRACE`
  (see FAST-LOOP.md).
- [ ] **R4 branch hygiene — do NOT blanket-prune.** Checked 2026-07-17:
  `fix/render-combiner`, `fix/render-blend`, `fix/render-othermode`, and
  `fix/render-texfmt` are fully merged to main (0 unmerged commits — safe to
  delete). But **`fix/render-scissor` still has 1 commit not in main**; triage
  it before deleting anything. `scripts/wt.sh prune` is the safe tool (it
  refuses unmerged/dirty/live-job worktrees). ~35 remote branches exist total;
  most predate the rename.
- [ ] **R5 audio out — implementation and deterministic bar complete; by-ear
  validation still open.** Status 2026-07-17. Three coupled runtime defects
  were measured:
  (a) the host device rejected 32 kHz and drained at 48 kHz without conversion;
  (b) a VI swap incorrectly ended the scheduler pump, so the following pump
  advanced retrace before finishing same-retrace work and OoT coalesced exactly
  one queued AudioMgr notification in three; (c) zero-filled boot RDRAM left
  `osTvType` at PAL while the shell supplied an NTSC VI clock/retrace. Together
  these produced either chronic underrun or a pegged/drop-oldest ring depending
  on which partial fix was active.

  The mechanism now makes each boundary explicit: `RetraceDrain` treats swap
  as an observation and ends only at guest quiescence; `TvType` is a required
  boot-harness input and seeds the IPL-owned global before thread 0; cpal
  resamples guest rate to device rate; and the audio backend distinguishes the
  current AI DMA (what `AI_LEN` exposes) from its host jitter prebuffer. Playback
  starts only after the N64-equivalent two-DMA queue is primed.

  Current live rs+RT64 evidence before the host-resampler quality pass: 60.0
  windowed retraces/sec, pump p95 about
  5.5 ms, present about 0.6 ms, 180 AI buffers per 180 retraces after the title
  transition, stable host depth about 2.6–3.1k frames, no overflow warning, and
  exactly zero callback underrun samples from stream start through swap 900.
  The live title window was inspected in `/tmp/fn64-timing-audio-fixed.png`.
  The final working tree passed 10/10 consecutive whole-workspace nextest runs
  (635/635 each), strict clippy, doc/layout lints, and the C/rs framebuffer
  differential through swap 60. User listening still reported faint buzz after
  these mechanical timing fixes, so R5 stayed open. The next measured defect was
  the fallback host resampler: linear interpolation suppressed underruns but
  left a measurable image near 20 kHz when converting valid ~32 kHz guest audio
  to a 48 kHz device stream. `fn64-audio` now uses a stateful windowed-sinc
  resampler with a spectral regression test.

  Live rs+RT64 rerun after the resampler pass, optimized shell, 2026-07-17:
  device rejected 32 kHz, opened at 48 kHz with band-limited resampling; visual
  screenshot inspected at `/tmp/fn64-live-release-bandlimited.png` (title/horse
  sequence coherent, no horizontal banding); foreground held 60.0 windowed Hz
  with zero callback underruns through swap 1740; backgrounded by activating
  Terminal and continued clean through swap 4380. Representative final window:
  interval median/p95 16.67/17.41 ms, pump 1.49/4.84 ms, present 0.50/0.54 ms,
  ring depth 2851 frames, `underrun_samples=0 (+0 window)`, guest/stream
  32006/48000 Hz. Do not mark R5 complete until the user confirms the new build
  is right by ear in foreground and background.

  Follow-up after a float CoreAudio callback: the user still heard unchanged
  crackle. That rules out the final host sample-format conversion as sufficient.
  The current frontier is upstream PCM production: RSP audio-task replay,
  RDRAM/DMEM byte-lane boundaries, and AI-buffer decode. `FN64_DUMP_AUDIO_TASK`
  now accepts one-based `FN64_DUMP_AUDIO_TASK_INDEX`; task #435 was captured
  from the live shell with normal OSTask fields and command-list shape for
  offline replay.

  Root cause found: the out-of-tree generated OoT `oot_aspmain.rs` was stale
  relative to current fn64-audio codegen. Its not-taken conditional branches
  inlined the delay-slot instruction and then resumed at the delay-slot label
  (`pc+4`) instead of after it (`pc+8`), so the delay slot ran twice. In task
  #435, generated-vs-interpreter replay first diverged at RSP DMA #8:
  generated wrote from DMEM `0x680` while the interpreter wrote from `0x660`;
  final RDRAM then differed inside the AI PCM buffer. Regenerating aspMain with
  the current emitter made `/tmp/fn64-task-435-generated-default.rdram` and
  `/tmp/fn64-task-435-interp.rdram` byte-identical (`cmp=0`). The
  `oot-audio-ucode` build script now traps the known stale OoT generated shape,
  and fn64-audio has a regression asserting conditional-branch fallthrough
  skips the already-inlined delay slot.

  Ruled out as *sufficient* causes — each was real, each was fixed, static
  survived all of them: App Nap throttling the producer; the 48 kHz/32 kHz
  rate mismatch; the severed AI_LEN feedback loop; linear host resampler image
  bands; the final host i16 callback path. The pattern to resist:
  every one of these looked like the whole answer at the time. A fourth
  plausible-and-partial cause is the expected shape of the next finding, so
  do not close R5 on a single fix that merely improves it — close it by ear,
  foreground and backgrounded.

  **THE REQUIREMENT IS FAITHFUL RATE — not "audio stops hissing"** (user,
  2026-07-17). The game must run at the SAME SPEED as hardware with audio and
  video both paced to it. Static is a symptom of a free-running producer; the
  over-speed feel is the same disease. Do not close R5 on quiet audio at the
  wrong speed. Close it only when BOTH hold, each with its own evidence:
  - **Video clock**: VI retrace ticks land at 60 Hz NTSC (~16.67 ms) over a
    long run — windowed median + p95, measured, not felt. Do not require 60
    framebuffer swaps/second: OoT legitimately submits a new frame every
    three retraces in its 20 fps title/game path.
  - **Audio**: production tracks retraces rather than framebuffer swaps, ring
    depth is stable rather than pegged at its 12000-frame cap, and output is
    right by ear (foreground AND backgrounded).

  **THE TWO CLOCKS (the thing to understand before probing).** R7 made the
  shell pace its PUMP on wall-clock — `crates/fn64-shell/src/main.rs:529`,
  `FRAME = 16_666_667ns` via `ControlFlow::WaitUntil`. That governs when the
  shell asks the game to advance. It does NOT govern how often the guest's VI
  RETRACE fires. If the retrace ticker over-delivers, the audio thread runs
  its produce cycle too often AND the game logic advances too fast — one
  cause, both symptoms. That is probe 3, and it is the only hypothesis on the
  list that explains everything with one mechanism.

  **TOOLING — what exists, and the gap (answer this before iterating).**
  - EXISTS: `OOT_SWAP_TIMING=1` prints `SWAP_TIMING swap=N dt_ms=X` per swap
    (`examples/oot-boot/src/main.rs:588,643`). But the headless harness runs
    flat out, so this measures COMPUTE COST, not delivery cadence. Useful as a
    budget check (rs gameplay is ~3.8 ms median, far inside 16.67), useless as
    a rate check.
  - EXISTS: the shell's 60-swap heartbeat and `ring_frames`, which is how the
    pegged-ring evidence was gathered. It now also reports a bounded window's
    retrace interval, pump, and present median/p95 plus windowed Hz; this
    separates cumulative-rate history from the current bottleneck.
  - **ORIGINAL MEASUREMENT 2026-07-17. PROBE 3 WAS CONFIRMED.**
    `fn64_abi::retrace_cadence()` -> `(ticks, secs, hz)` since the first tick,
    or `None` before any (never a fake 0 Hz). Counted in
    `Executor::advance_time` where ticks really fire; correlated with wall-clock
    in `fn64-abi` (`fn64-runtime` is wall-clock-free by design). Reported on the
    shell heartbeat next to `ring_frames`.

    Real shell run (C lane, reference backend, 25s). retrace_hz is CUMULATIVE
    since the first tick, so these are averages — the instantaneous rate at the
    end is worse:

        swap  #60: retrace_hz=59.9   ring=8518    ai_buffers=58
        swap #120: retrace_hz=60.0   ring=12000   ai_buffers=118
        swap #180: retrace_hz=60.0   ring=11912   ai_buffers=178
        swap #240: retrace_hz=63.9   ring=12000   ai_buffers=254
        swap #300: retrace_hz=87.2   ring=12000   ai_buffers=434
        swap #480: retrace_hz=122.1  ring=12000   ai_buffers=974

    **The ticker starts CORRECT (59.9 Hz) and runs away** — ~2x target by swap
    480, still climbing. So this is NOT a wrong `arm_vi_retrace(1000)` constant:
    a bad interval would be wrong from swap #1. Something makes delivery
    ACCELERATE.

    The causal chain is in one table. Through #180 retrace holds ~60 and
    ai_buffers tracks swaps 1:1 (118/120). The moment retrace departs at #240,
    ai_buffers outpaces swaps, reaching 974/480 (~2:1) — matching the ~2x
    retrace rate. The audio thread produces PER RETRACE, so an over-delivering
    ticker over-produces audio; the ring pegs at its 12000 cap and drop-oldest
    skips playback = the static. Same mechanism drives the over-speed feel.
    This explains R5's original "~3 AI buffers/frame where ~1 is expected".

    Caveats, stated rather than glossed: `nonzero=0` because the C lane links
    no audio ucode — the PACING bug is upstream of sample content, but the rs
    lane is where the ucode runs and where R5's 3x was first measured. And a
    windowed (not cumulative) rate would sharpen the onset.

    **SUPERSEDED INTERMEDIATE MEASUREMENT (kept as causal evidence).**
    Re-measured after the R8 fix, rs lane, reference renderer, no audio:
    Commit `0611d35` closed the runaway: one wall-paced pump now advances
    exactly one retrace interval, so the rate never accelerates above 60.
    The new windowed probe exposes a different limit after the title path
    begins doing one swap per three retraces:

        swaps 60/120/180: interval median 16.67 ms, p95 <=18.53 ms,
                          pump median <=0.46 ms, windowed 60.0 Hz
        swap 240:         interval 16.82/42.59 ms median/p95,
                          pump 0.40/42.00 ms, windowed 59.4 Hz
        swaps 300..480:   interval median 31.23..31.73 ms,
                          pump median 30.74..31.23 ms, windowed 31.5..32.0 Hz
                          present median 0.50..0.51 ms

    The later trace showed that this was the priority-0 idle thread being
    resumed to the arbitrary 200,000-step cap, not render work. The typed
    idle/quiescence boundary removed that spin; the faithful RT64 lane now has
    pump p95 around 5.5 ms and holds 60.0 windowed retraces/sec.

  **A REFERENCE CAPTURE — yes, but capture it, never download it** (user
  asked 2026-07-17). A golden WAV of the intro pulled from the web is
  copyrighted game audio and breaks this repo's own rule (README/AGENTS: no
  game content, no ROM-derived bytes, ever). Capturing it yourself from an
  emulator running YOUR ROM is the same footing as the ROM you already
  recompile locally, and gets the same handling as `.eyegate/` frame captures:
  evidence-only, gitignored, never committed. That is the established pattern
  — R3's eye-gate compared fn64 PNGs against the emulator side-by-side.

  But note WHAT it answers. R5's bug is RATE, and a reference recording does
  not detect a pegged ring — the retrace-cadence counter above does. A capture
  earns its keep in two places:
  - **Now, mechanically**: fn64 already dumps PCM (`FN64_DUMP_AUDIO_PCM`
    writes s16le + a `.meta` sidecar with sample count/range/nonzero). Two
    dumps over the same swap window — fn64's and the emulator's — give a
    DURATION RATIO, which proves or disproves over-speed numerically instead
    of by feel. That directly fills the "by ear" hole in this item's close
    condition.
  - **Later, for faithfulness**: once the rate is right, "does it sound
    correct" is the remaining question, and that is what a reference answers.

  **HISTORICAL START POINT (probes below are completed; kept for provenance):**
  1. **Use the rs lane, not C.** V1a proved the C lane silently no-ops 127
     stubbed functions; if any sits on the audio path you would chase a ghost
     that exists only in the oracle. `FN64_RECOMP=rs`.
  2. **Knobs renamed** (H2b): `FN64_SKIP_AUDIO_UCODE`, `FN64_PHASE_TIMING`,
     `FN64_DUMP_AUDIO_PCM`, `FN64_AUDIO_UCODE_TIMING`. Stale `OOT_*` spellings
     now panic naming their replacement instead of silently no-oping.
  3. **`FN64_GAME_DIR` has no default** (H1b); export it or `./oot` exits with
     instructions. `./oot run` also works again on stock macOS bash.
  4. **Probe 1 forks the investigation and is cheap.** Evidence already says
     the producer ignores backpressure (ring pegged from ~swap 250; ~3 AI
     buffers/frame vs ~1; the AI_LEN fix changed nothing). So: does the guest
     CALL `osAiGetLength` at all? NEVER CALLED -> the AI_LEN work was
     irrelevant, go to probe 2. CALLED AND IGNORED -> go straight to probe 3.
  Superseded note (kept for history): the static was first blamed on
  sample-rate mismatch alone, and before that on App Nap alone. Both were
  real contributors; neither was sufficient.
  fn64-shell's ladder opened the stream at 48 kHz FIRST while the game
  produces 32 kHz with no resampler anywhere, so the ring starved ~1/3 of
  the time and the callback zero-fill rendered as static (backgrounding
  worsened it by throttling the producer). Fix: CpalBackend negotiates the
  stream rate with the device and linear-resamples producer-side;
  set_frequency is now real; osAiSetFrequency forwards the true DAC rate;
  both harness ladders replaced with one guest-rate create. Verify by ear
  (foreground + backgrounded) to close.

- [ ] **R8 rdram word-swizzle class — implementation verified, merge pending.**
  The fourth instance was `ReferenceBackend` writing flat big-endian RGBA5551
  halfwords into N64Recomp's native-word RDRAM storage. The window's read was
  serial with guest execution, so the suspected torn read was not possible.
  The working-tree correction centralizes logical addressing in
  `RdramView`/`RdramViewMut` plus the explicitly unsafe ABI-only `RdramPtr`;
  all found framebuffer, DMA, controller, PCM, renderer, and diagnostic
  boundaries use those types. `scripts/lint-rdram-layout.py` rejects new
  production manual lane XOR/raw writes. Evidence: live before/after window
  screenshots inspected, 10/10 integrated deterministic runs, 630/630
  workspace tests, strict clippy, and C/rs parity across 58 captured frames
  through swap 60. Mark `[x]` only once this working tree is merged.

## Phase D — fn64 owns discover → decomp

Today OoT symbol/section metadata comes entirely from the zeldaret decomp
via aki-recomp's Python (`import_oot_syms.py`, `gen_stubs.py`). The zeldaret
answer key (10,833 named fns) makes OoT the perfect graded target.
fn64-discover has Phases 1/2/4/5 + bounded-6, plus candidate-only region,
homology, loader, trace, probe, and tool-adapter foundations; pack emission
and end-to-end closure remain open (DISCOVER-DESIGN.md). The active
measurements and implementation sequence are
in `DISCOVER-PLAN.md`; `DISCOVER-STORAGE.md`, `DISCOVER-OWNER-PROOF.md`, and
`DISCOVER-TOOLCHAIN.md` define the graph/index, exact-owner, and external-tool
boundaries.

Current grading (D1+D1.5+D2, 2026-07-17, `gate_d1`; inputs are
`FN64_DISCOVER_*` env-declared since H3 closed — but see H3: these numbers
are reproducible on one machine only): OoT 98.7% precision / 72.3% recall;
NW4E 48.4%/89.7%; NWXE 50.0%/86.9% (NWXE was 36.4%/28.5% until its overlay
bank table geometry was wired into the gate — the Phase-2 recall lesson
below, confirmed a second time). The AKI figures use SHA-bound external
text intervals, so they measure the payoff from correct executable regions,
not mechanical recovery of those regions. Mapping-only baselines and byte
coverage remain separate in `DISCOVER-PLAN.md`.

NWXE's mapping-only overlay gap is closed mechanically: ROM-only
descriptor-family recovery plus unique delta/destination agreement produces
four proven load images, and wiring the overlay bank table geometry into
the gate took the held-out D1 grade from 36.4%/28.5% boot-only to
50.0%/86.9%. Whole-image data scanning is still why this sits below the
external-text-filter result above.

Two findings worth not rediscovering:
- **Recall is a Phase-2 problem, not a detector problem.** OoT recall was
  0.82% until D1.5 taught Phase 2 to discover DMA-table overlay load-images
  (OoT loads overlays via DMA tables, not descriptor tables), which alone
  took it to 72.3%. If recall stalls again, look at what Phase 2 exposes
  before tuning detectors — they can only hunt inside discovered images.
- **Table-derived candidates are an honest 0 everywhere.** Descriptor tables
  prove load-images, not entry points. Not a bug; don't "fix" it.
- **Cross-ROM homology is already a high-precision seed, not full recall.**
  Relocation-masked whole-body matches recovered 16.0% of NW4E and 22.6% of
  NWXE target entries at 99.6% and 98.8% boundary precision respectively;
  ambiguous bodies stayed unresolved. Ten gate runs were byte-identical.
- **External function finders still need native ownership proof.** An unseeded
  spimdisasm 1.42.2 run on NWXE resident text reached 92.0% entry precision /
  97.6% recall, but only 666 of 827 common starts had exact extents. A strict
  candidate-only CSV adapter now binds those claims to exact tool, bank,
  mapping, configuration, and provider-output identities. The native exact-
  owner proof type exists, but no real-ROM owner is claimed until its gate is
  integrated and every blocker is discharged.
- **External-tool claims now have a non-contaminating merge boundary.**
  Accepted adapter runs freeze into a canonical `ToolClaimSetV1` sidecar bound
  to the exact discovery snapshot, bank bytes, mapping, tool, configuration,
  and provider output. Deserialized sets recompute source/claim digests and
  bank-local constraints. The type remains candidate-only and never enters
  `FactDb`, so disabling a provider removes only its sidecar claims. Rich
  Ghidra/spimdisasm xrefs, blocks, data objects, and types are still open.
- **Strict boot-loader semantics transfer without title descriptors.** The
  generic entry-stub recognizer proves complete countdown zero-fill loops and
  constructed post-clear transfers in both current AKI grading ROMs, deriving
  their different ranges. Ten real-ROM gate runs were byte-identical. This
  does not yet recover general PI DMA overlay tables. The static slicer now
  models the public seven-argument `osPiStartDma` ABI and the distinct
  three-argument `osEPiStartDma` message ABI separately, including delay-slot
  stores and exact `OSIoMesg` geometry. The descriptor-free affine table-use
  stage now normalizes biased pointers, validates stable role offsets and the
  public ROM/text/data/BSS relations, and refuses to call a consecutive subset
  complete without independently proven loop base/count/stride. Producing
  those semantic loads and exact enumeration through real wrapper chains is
  the next frontier.
- **RE tools are now typed Decomp Pack inputs, not parallel truth stores.** A
  strict spimdisasm adapter preserves bank and lineage, and stock headless
  Ghidra passed ten deterministic synthetic runs with same-VA banks isolated
  and seeded/unseeded provenance distinct. Their next useful output is graph,
  xref, data, and type metadata; exact ownership and recompiler admission stay
  native proof stages.

Still open from D1.5: resident `code`/`n64dd` destination discovery.
- [x] **D2 Phase-6 completion** (merged 2026-07-17) — jump tables +
  value-set analysis; OoT precision 90.6% -> 98.7%.
- [~] **D2b shared decode + composed owner gate** (working tree,
  2026-07-18) — discovery now uses recompilation's decoder, invalid/missing
  delay words are typed owner blockers, REGIMM link and `jalr $zero`
  semantics are covered, and one physical bank composes into a byte-bound
  `ProgramSnapshotV1`. The real NWXE resident run is 10/10 byte-identical,
  keeps 0 wrong answer-key splits, and reports its honest blocker histogram.
  Link-free resolved jumps no longer fabricate callable roots; this reduced
  NWXE partition ambiguity from 5 blocks to 0 and improved its grading from
  26 exact + 1 coarse to 26 exact + 3 coarse. The ROM-header entry is now a
  typed hardware-authoritative fact. No real-ROM exact owner is admitted yet
  because executable-range authority is not mechanical (27/27 NWXE owner
  assessments carry that blocker, and it is the sole blocker for 25). A
  separate function-independent proof admits all 197 currently reached NWXE
  blocks / 4,156 bytes with exact ROM backing. Canonical leader splitting
  prevents overlapping pseudo-blocks. `BlockPackV1` binds those disjoint
  spans to ROM and per-block digests without serializing ROM words; the real
  gate re-materializes 1,039 words, emits a sparse arbitrary-PC runner, and
  compiles it with `rustc`. Holes receive no arms or same-bank authority. This
  does not claim the remaining resident text or overlays. The typed runtime
  catalog now owns/searches disjoint bank-bound spans and the real gate re-
  resolves every packed word while rejecting a real hole. `BlockProgram`
  atomically pairs code and generated callables, rejects bank mismatch or
  duplicate registration, and resolves sparse admission before invocation;
  the emitter's registration helper passes the compile/run gate. This owned
  program is not yet connected to live guest dispatch.
- [ ] **D3 Phases 7-8**: targeted dynamic probes; assembly/relink
  verification. Digest-bound strict trace ingestion and emulator-neutral
  bounded probe plans exist; a black-box emulator adapter and proof-rule
  integration remain open.
- [ ] **D4 pack emission**: emit fn64-owned `dump.toml`-equivalent
  (Decomp Pack) from discover output, replacing the aki-recomp Python for
  metadata production.
- [ ] **D4b full-game execution closure**: emit a bank-qualified dispatch
  table whose destinations are exact-owner AOT, basic-block AOT, or explicit
  instrumented MIPS fallback. A release has zero unsupported destinations;
  function-boundary ambiguity is not allowed to become a missing-code path.
- [ ] **D-gate**: `recompile_rom` consumes the fn64-discovered pack for OoT
  and the boot is **byte-identical** (framebuffer SHA at fixed swaps) to the
  decomp-metadata build. Then produce the WM2000 pack with zero
  game-specific code.

## Phase U — general N64 execution closure

`UNIVERSAL-RUNTIME-PLAN.md` is the load-bearing capability matrix and staged
gate. Discovery and exact decomp remain valuable, but execution may not depend
on recovering historical function boundaries. The universal destination is a
bank-qualified PC; every path ends as exact AOT, block AOT, dynamic MIPS, or an
explicit unsupported result, and a release admits zero unsupported results.

- [~] **U0 execution identity** — typed bank/PC keys, immutable code-bank
  admission, overlapping same-VA isolation, and typed block exits exist in the
  working tree; arbitrary-PC execution is U1, so this is not yet complete.
- [~] **U1 one-bank arbitrary-PC runner** — the working tree emits, compiles,
  and executes every aligned entry with typed transfers/faults, ordinary-entry
  parity, deterministic instruction budgets, delay-pair-safe checkpoints, and
  an outer dispatcher that follows bank-qualified direct/resolved transfers.
  It also consumes digest-verified disjoint Block Pack spans without decoding
  data gaps; a real NWXE 197-block/1,039-word runner passes `rustc`, and the
  sparse catalog re-resolves every packed word while rejecting a real hole.
  Code/runner registration is now atomic and bank-checked in `BlockProgram`.
  Live dispatcher ownership and guest-cycle charging remain open, so this is
  not a full execution lane yet.
- [~] **U2 deterministic PI/device slice** — a typed working-tree fabric makes
  raw PI registers and the shim API schedule the same deadline and atomically
  orders bytes, PI busy, MI pending, and notification in a cycle-stamped trace.
  Existing ABI/MMIO routing, hardware PI timing, executable generations, and
  executor notification delivery are not connected yet.
- [ ] **U3 runtime code generations** — load, translate, rewrite, invalidate,
  and redispatch overlay/decompressed/generated code without stale blocks.
- [ ] **U4/U5 CPU + device closure** — precise exceptions, CP0/TLB/FPU and
  complete PI/SI/AI/VI/MI/controller/save/timing behavior.
- [ ] **U6 general RSP/RDP** — persistent IMEM generations and LLE fallback for
  every non-proven HLE task; no skip or synthetic completion.
- [ ] **U7 exploration/release gate** — digest-bound forced-state exploration,
  deterministic output traces, and zero unsupported execution closure.

## Phase H — cut the aki-recomp tether (blocks going public)

`/Users/jer/Code/aki-recomp` is a **legacy project** (user, 2026-07-17), yet
fn64 cannot boot OoT without it. Everything the toolchain needs should live in
fn64. This phase is not "declare the dependency" — it is delete it.

Inventoried 2026-07-17. Five things are pulled from the legacy checkout; only
ONE has a permanent reason to be out-of-tree:

What fn64 still pulls from it, and why each is allowed to stay:

| What | Why it stays out-of-tree |
|---|---|
| `games/*/*.z64` | ROM. Forever out-of-tree; a plain `ROM=` path. |
| `games/OOTU/rsp-recomp/src/oot_aspmain.rs` | ROM-derived ucode. Can't be vendored; `OOT_ASPMAIN`/`FN64_GAME_DIR` locate it (H1b). |
| `games/OOTU/RecompiledFuncs/` | ROM-derived generated C. `RECOMPILED_DIR`. |
| `games/OOTU/oot.toml` | Its relative paths reach the ROM + `syms/dump.toml`; in-tree it would point back out. Dies at D-gate. |
| `games/*/syms/dump.toml`, `refs/oot-decomp/**/segments.csv` | Oracle metadata. Dies at D-gate — D4 emits fn64's own pack. |

Everything else is now owned here. After D-gate, only a user's own ROM remains.

- [x] **H1 vendor `recomp.h`** (2026-07-17) — in-tree at
  `bridge/include/vendor/`; `RECOMP_H_DIR` now only overrides.
- [x] **H1b audio-ucode lookup + prune `AKI`** (2026-07-17) — `OOT_ASPMAIN` or
  `$FN64_GAME_DIR`, no relative fallback, no default, loud on unset.
- [x] **H2 `ABI-SURFACE.md` not needed** (2026-07-17) — nothing consumes it;
  `fn64-abi`'s `c_smoke` link test is the live oracle. Read order now points at
  the code (AGENTS.md carries the reasoning).
- [x] **H2b `OOT_*` debug knobs read inside game-agnostic core crates.** Core
  knobs are now `FN64_*`; `fn64-render-rt64`'s `debug_flag()` and `fn64-abi`'s
  `assert_no_legacy_env_vars()` panic on a retired `OOT_*` spelling so an old
  invocation cannot silently no-op. `examples/` keeps `OOT_*` by design.
- [x] **H3 `fn64-discover` gates cannot run off the author's machine.** Fixed
  2026-07-18: personal paths became required `FN64_DISCOVER_*` env vars with
  loud unset errors; grades byte-identical; `gate_b2` digest gated.

- [ ] **H4 `cargo test -p fn64-abi` flakes ~60%; nextest does NOT.** Measured
  2026-07-17 across the day: `cargo test -p fn64-abi --lib` ->
  101/0/0/0/101/101/101/101/101/0 (6 of 10 red, zero real assertion failures).
  `cargo nextest run --workspace` -> 622/622 green, repeatedly; it gives each
  test its own process. So the DELEGATION.md merge gate is SOUND; `cargo test`
  is the broken lane. Do not "fix" this by weakening what the tests assert:
  the `__*_abort_subprocess_entry` tests exist to prove fn64's loud traps
  really abort.

  Standing hypothesis (still the only measured one): `assert_subprocess_aborts`
  (`test_support.rs:41`) spawns children that `abort()`, and the child's
  status/signal races into the parent runner's exit code.

  Two dead ends, recorded so nobody re-derives them: (1) a wave blamed a test
  calling `std::env::set_var` and tripping the H2b rename trap in parallel
  siblings. That test was real but was *this session's own* transient bug,
  since removed — and the flake persists at 6/10 WITHOUT it, so it was an
  aggravator, never the cause. (2) `--test-threads=1` was claimed to fail
  deterministically; measured 0/101/0. Trust nothing here that is not a
  re-measured exit code.

  Lesson, kept: **treat exit-101 as investigate, not assume-flake** — an abort
  killing the runner looks exactly like a real regression.

- [ ] **P1**: retire the C lane to CI-oracle-only (DESIGN.md M3); relicense
  checkpoint.
- [ ] **P2**: wgpu render backend (RENDER-WGPU-PORT-PLAN.md), eye-gated
  against the then-verified RT64 output.
- [ ] **P3**: shell polish — input, audio device handling, windowing.
