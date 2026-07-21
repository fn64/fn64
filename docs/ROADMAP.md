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
  systematically wrong.** A historical deeper c/rs run remained identical
  through swap 231 and first differed at framebuffer 234 after gfx task 232;
  that is not an opcode bug and not the renderer. The current mechanically
  repeated gate is deliberately bounded to the 60 swaps it measured.

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

- [x] **V1b C/rs arbitration fails closed when callable bodies differ.** The audit finds 13,047 shared nonempty functions with identical unique instruction-PC sets, but 116 callable empty C bodies with nonempty Rust counterparts.
  Default `scripts/lane-parity.sh` therefore rejects C authority from swap zero; `--observe` retains only a labeled, non-authoritative framebuffer comparison.
  Focused semantic oracles remain independent evidence; fixed-cycle digests and zero-unsupported full-ROM closure remain the replacement release authority. See `PARITY-METHOD.md`.
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
  boot-harness input, seeds the IPL-owned global before thread 0, and is carried
  into RT64 so PAL stable-factor workloads derive from 50 Hz rather than the
  pinned upstream 60 Hz constant. Ten fresh live Metal processes prove the
  production-context PAL/MPAL workload sequences `[0,0,0,50]` and
  `[0,0,0,60]` without an Extended refresh override. Report schema v20 now
  co-binds normalized ROM TV region, committed device TV state, and renderer
  create-time TV configuration; representative private PAL/MPAL exact-ten
  evidence remains to be retained. cpal
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

Current grading (2026-07-17, `gate_d1`; private inputs are declared through
`FN64_DISCOVER_*` rather than machine-specific paths): OoT is 98.7% precision /
72.3% recall. The AKI mapping-only baselines are NW4E 48.4%/89.7% and NWXE
50.0%/86.9%; applying SHA-bound external text intervals gives NW4E
82.4%/88.1% and NWXE 81.3%/84.1%. The filtered figures measure the payoff from
known executable regions, not mechanical recovery of those regions. The
mapping-only baselines, filtered measurements, and byte coverage remain
separate in `DISCOVER-PLAN.md`; the private inputs also mean these measurements
are not a repository-contained reproducibility claim.

NWXE's mapping-only overlay gap is closed mechanically: ROM-only
descriptor-family recovery plus unique delta/destination agreement produces
four proven load images, and wiring the overlay bank-table geometry into the
gate took the held-out D1 grade from 36.4%/28.5% boot-only to 50.0%/86.9%.
Whole-image data scanning is still why that mapping-only precision remains
below the external-text-filter result.

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
  the emitter's registration helper passes the compile/run gate. The ABI now
  boots an owned `BlockProgram` through the real executor for thread 0 and
  spawned OSThreads; block checkpoints charge instruction time only after the
  coroutine suspends. A synthetic live gate proves two arbitrary-PC turns,
  exact 3+2 virtual-cycle charging, RDRAM mutation, and typed thread return.
  Generated `jr` recognizes only the installed OSThread return sentinel, and a
  supplied static host-JAL inventory emits typed HostCall/resume boundaries
  with its delay slot intact. Dynamic JAL/JALR now emits `ResolveCall`; the
  live resolver distinguishes installed host functions from guest banks and
  preserves the delay slot and exact resume key. Real discovered packs still
  need boot wiring and runtime generation ownership.
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

Runtime-surface baseline (2026-07-18): the clean-room NMR gate is now
**116/116 implemented**, and the broader live-export gate is **137/137** with
zero source-shaped partial/trap bodies. This closes the ABI denominator, not
universal behavior: `scripts/check-nmr-surface.py --require-complete
--require-all-exports --check-doc` is now the permanent floor while U2-U7
close timed devices, architectural CPU behavior, general RSP/RDP execution,
and deterministic output traces.

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
  `boot_thread0_block_program` now makes that owned program the real
  coroutine's dispatcher and converts instruction checkpoints into executor
  virtual time before device service/rescheduling. The live integration gate
  covers thread 0, inherited spawned-thread ownership, RDRAM mutation, and
  typed return. Static known-host JALs now emit a typed host/resume boundary;
  `jr`/`jalr` returns terminate only at the installed thread sentinel.
  Dynamic JAL/JALR calls resolve distinctly to the installed host table or an
  active guest bank. The OoT host can explicitly select and source-hash-bind a
  generated pack, rejects a missing artifact, and cannot fall back to guest
  whole-function lookup in that mode. Generating the real OoT pack and
  automatically detecting executable PI/decompression writes remain open, so
  this is not a full execution lane.
- [~] **U2 deterministic PI/device slice** — the typed fabric is now live for
  managed/raw PI starts and typed-Rust word MMIO. It schedules one deadline,
  leaves bytes untouched while busy, then orders byte commit, PI-idle, MI
  pending, and executor completion delivery before any coroutine can resume.
  Both lanes use the process's one RDRAM allocation; the fixed default latency
  is explicitly configurable and not claimed hardware-exact. The block lane
  now commits checkpoint-due PI work before another resume, drives masked MI
  output onto CPU IP2, enters precise Cause/EPC/Status exception state,
  acknowledges PI, and returns through ERET (20 consecutive clean live-gate
  runs). SP, SI, AI, VI, PI, and DP now share its level-sensitive pending/mask
  gate. Typed raw writes implement the public SP/VI/AI/SI/DP acknowledgement
  commands rather than clearing a disconnected register mirror. Generated-C
  `MEM_W` now preserves direct RDRAM lvalues while routing KSEG1 RCP word loads
  and assignments through those same handlers; SP DMEM/IMEM, DMA registers,
  status, semaphore, and the real `0xA4080000` PC share that path. Subword RCP
  access still traps.
  Hardware-derived PI timing and function-interior timed-device checkpoints
  remain open.
- [~] **U3 runtime code generations** — `ExecutableRegion` now installs one
  immutable bank+runner generation, atomically retires both halves of the old
  generation, and re-resolves interrupt/checkpoint/host/spawned-thread entries
  through the new active bank at the same virtual PC. Equal-length registered
  physical/virtual regions now share post-commit CPU-store, generated-C-store,
  and DMA write observation. Focused live gates prove a CPU rewrite retires A
  before its suspended checkpoint resolves and a due PI DMA snapshots final
  architectural bytes and installs B before completion visibility; both gates
  passed 10 consecutive clean runs. A runner may still execute instructions
  after its store until its next boundary;
  store-interior checkpoints, translation of newly uploaded words,
  page-granular regions, and real-pack boot wiring remain open.
- [~] **U4/U5 CPU + device closure** — bank runners now return typed
  SYSCALL/BREAK/conditional-trap, signed-overflow, instruction-fetch AdEL, and
  aligned-memory AdEL/AdES faults with exact PC/EPC/BD/BadVAddr context and can
  apply them to Status/Cause/EPC/BadVAddr plus the BEV-selected general vector. All naturally
  aligned integer, LL/SC, and COP1 loads/stores check before mutating register,
  memory, or reservation state; byte and left/right merge operations remain
  intentionally unaligned. Misaligned initial and computed PCs now fault as a
  counted fetch attempt; an exhausted branch/delay budget checkpoints at the
  target before raising AdEL on the next dispatch. Every decoded COP1 move,
  memory, arithmetic, comparison, conversion, and branch checks Status.CU1
  before any visible effect. Disabled COP1 raises ExcCode 11 with Cause.CE=1;
  branch and delay-slot ordering preserve precise EPC/BD, and CU1 failure takes
  priority over COP1 address alignment. `BlockProgram::dispatch` resolves that
  vector through the active executable mapping and runs the registered handler bank while
  preserving one deterministic instruction budget. The same lane executes
  ERET with ErrorEPC/ERL precedence, EPC/EXL fallback, and LLbit clearing, then
  resolves the return bank. Typed COP0 reads now cover BadVAddr, Count, Compare,
  Status, Cause, EPC, and ErrorEPC; typed writes cover Count, Compare, Status,
  Cause software-pending bits, EPC, and ErrorEPC. The live block owner samples
  the level-sensitive RCP line at block boundaries and resolves enabled
  interrupts through the same active handler mapping as synchronous faults.
  Count now advances once per two guest CPU cycles with persistent odd-cycle
  phase; wrap-safe Compare equality latches IP7 until an MTC0 Compare write,
  and the live handler path acknowledges it before ERET resumes guest code
  (20 consecutive clean live-gate runs).
  The RCP/MI fabric is now always present rather than being created as a side
  effect of cartridge ROM installation; a separate typed flag preserves loud
  missing-ROM PI failures. AI has a deterministic two-slot current/next FIFO:
  shim and typed raw submissions share BUSY/FULL, guest-cycle drain deadlines,
  decrementing `AI_LEN`, MI AI assertion, and OS_EVENT_AI delivery after the
  device transition. The duration uses the 93.75 MHz CPU clock and libultra's
  quantized playback rate and the IPL-selected NTSC/PAL/MPAL video clock;
  hardware timing traces remain open. SI now has a scheduled 64-byte DRAM/PIF
  engine with persistent
  PIF RAM, BUSY/error/interrupt status, distinct write/execute/read phases,
  and shim/raw-register convergence. Its current one-cycle latency is an
  explicit deterministic policy, not hardware timing. HLE ucode effects still
  execute atomically. Successful renderer operations now publish typed
  FullSync evidence: tasks without it schedule SP only, tasks reaching it
  schedule SP then DP, and raw CPU/RSP DPC FullSync schedules DP without a
  fabricated SP event. Exact-width raw decoding prevents triangle payload
  words from masquerading as commands; unresolved successful backends trap.
  The visible SP and DP completions are distinct fabric events after measured
  rspboot work and at successive deadlines, with SP-before-DP MI/message
  ordering when both exist. The same fabric now owns persistent 4 KiB RSP DMEM and IMEM, PC,
  status, semaphore, and a double-buffered SP DMA engine. Raw word MMIO models
  the public 8-byte address/length alignment plus length/count/skip rectangle,
  keeps DMA_BUSY asserted while a queued request becomes active, and changes an
  IMEM generation only after bytes commit. Its deterministic eight-cycle setup
  plus one-cycle-per-64-bit-beat policy is explicit, not hardware-exact.
  `osSpTaskLoad` now copies the complete 64-byte `OSTask` to DMEM `0xfc0`,
  aligned rspboot bytes to IMEM zero, and resets PC as the public RSP guide
  specifies. The public yield handshake is now observable end to end at the OS
  boundary: `osSpTaskYield` sets SIG0, `osSpTaskYielded` distinguishes SIG1
  from normal completion and prepares the acknowledged task's yield buffer for
  restart, task load clears stale SIG0/SIG1, and the query cannot redispatch
  graphics or audio work. Known tasks classify boot-overlay versus direct
  IMEM admission: physical `ucode_boot == ucode` plus a covering aligned copy
  enters at PC zero, while ordinary rspboot still must reach a DMA-loaded
  generation and truncated equal-pointer images remain loud failures. A renderer `Yielded` result now sets SIG1 and posts
  SP completion without fabricating DP completion. Missing/failed task, raw
  DPC, XBUS DPC, and presentation backends share one loud error gate. A
  subsequent Load+StartGo supplies `OS_TASK_YIELDED` and the saved data range
  back to the renderer, proving cooperative resume. Unknown/custom tasks now execute that image through the clean-room
  scalar/vector interpreter, resolve IMEM overlay DMA generations, resume at
  the saved PC, commit DMEM/RDRAM/status at BREAK, and forward both DRAM and
  XBUS/DMEM DPC ranges. Exact graphics/audio HLE paths now execute admitted
  rspboot until the first DMA-loaded ucode instruction and commit RDRAM, DMEM,
  IMEM, status, and entry PC before backend dispatch; direct-image tasks use
  their already-admitted PC-zero state. Transactional LLE fallback restores
  the rspboot scalar/VU/SP/DMA/DPC snapshot or the untouched direct state.
  Command-triggered DP completion is now closed for successful admitted HLE,
  LLE DPC, and raw DPC operations. The ABI/device seam now also owns a typed
  resumable-HLE protocol: one opaque backend token is retained after each real
  committed chunk, SIG0 is checked before token consumption, the matching task
  and yield buffer own suspension, and reload/resume consumes each token once.
  Exact hardware RDP latency remains open, so complete device timing is not yet
  claimed. Reference and native RT64 adapters explicitly report `Atomic`:
  reference decode/raster state still needs a checkpoint representation, while
  RT64's public native task entry exposes no continuation. VI now joins the same event
  heap and MI gate: typed raw access covers its complete 14-register block,
  `VI_CURRENT` advances through the documented progressive even and serrated
  interlaced even/odd half-line sequences over `VI_V_SYNC`, `VI_INTR` schedules the interrupt,
  and `osViSetMode`'s public register image plus pending framebuffer/scales/
  blanking latch before messages and renderer presentation at V-blank.
  The public current-line/field/status/mode queries read that live state and
  distinguish queued from latched mode. `osViSetSpecialFeatures` now applies
  the public gamma/gamma-dither/divot/dither-filter command pairs to that
  queued control image, retaining the hardware's bit-16 dither-filter state.
  The VI-manager message target now honors `retraceCount` without dividing
  the independent hardware/`OS_EVENT_VI` cadence.
  Both `OSViMode` field-register images now alternate with live field parity,
  and field origin is correctly relative to the queued framebuffer. X/Y scale
  calls now update the live 2.10 coefficients at retrace, preserve
  subpixel offsets, enforce the public ranges, and obey mode-reset ordering.
  `osViBlack` now crosses a typed presentation boundary: black/unblack changes
  present at V-blank even without a framebuffer swap, the Rust reference lane
  preserves the underlying RDP image across black scanout, and invalid
  non-unit Y-scale use traps. Beyond NMR's canonical inventory, `osViFade` and
  `osViRepeatLine` are now exported through both recompiler lanes, latch at
  V-blank, and execute on the Rust scanout image without modifying the RDP
  source. The RT64 adapter now maps the same state to VI pixel type, vertical
  scale, vertical subpixel offset, and the latched gamma/gamma-dither/divot/
  dither-filter status through its C boundary. The Rust lane implements the
  public square-root gamma transfer, partial-coverage horizontal-median divot,
  RGBA16 3x3-neighbor dither restoration, and retrace-seeded stochastic
  seven-bit gamma dither. `VI-FILTERS.md` separates those deterministic digital
  mechanisms from the bounded partial-coverage fallback: exact coverage
  AA/resampling, the silicon gamma ROM and random stream, and post-DAC analog
  video remain open. The feature build proves the RT64 boundary, while GPU
  screenshot validation remains open. The typed
  IPL standard now selects the shared VI/AI clock; nominal
  60/50 Hz boot timing gives way to H_SYNC/V_SYNC-derived field deadlines once
  a mode latches, and host loops consume the live interval. Hardware-trace
  validation, exact random-stream identity, resampling/AA arithmetic, and
  pixel-level RT64 capture remain open. The other
  fault classes and remaining CP0/TLB/FPU/controller/save
  behavior remain open.
- [~] **U6 general RSP/RDP** — the F3DEX2 reference lane now emits ordered
  triangle/color-image/fill/full-sync operations and executes all three legal
  public color-image layouts: size-defined 8-bit index/intensity, RGBA16, and
  RGBA32. One typed classifier drives validation, import, fill, copy, and
  commit; target switches can reinterpret the same RDRAM bytes. The 8-bit path
  supports direct I8, packed IA8, and undereferenced CI8 copy-cycle sources,
  preserving each original TMEM byte while comparing format-correct alpha;
  RGBA32 preserves the
  public five-bit memory alpha and three-bit coverage packing. It also decodes the
  F3DEX2 control path now retains six homogeneous clip-plane codes,
  executes inclusive-range `G_CULLDL` as an end of the current list,
  performs the compound `G_RDPHALF_1`/`G_BRANCH_Z` unsigned-16.16 screen-depth
  tail branch, pops the full `G_POPMTX` count, and applies both public
  `G_MW_LIGHTCOL` color-copy destinations without changing light direction.
  Signed `G_MW_FOG` factors now replace vertex shade alpha from projected
  depth when fog geometry mode is active; exact microcode fixed-point rounding
  remains a hardware-trace item. `G_LINE3D` now carries public variable width,
  six-plane homogeneous clipping, flat/smooth shade, perspective texture
  attributes, scissor/eight-sample coverage, blender state, and read-only Z
  through a typed operation. Exact microcode line-edge coefficients remain a
  hardware-trace item. Exact task-text digest admission now distinguishes
  legacy L3DEX from L3DEX2 and F3DEX2 without opcode guessing. The public
  legacy vertex/line envelope and the L3DEX2 line form normalize into that
  same typed operation; their equivalent raster output is regression-tested.
  Line-microcode `G_TRI1` is a validated no-op, while polygon-only and
  undocumented commands remain loud. See
  `crates/fn64-render-reference/L3DEX-CONCEPTS.md`. Exact digest admission now
  also distinguishes the public Fast3D, F3DEX, and F3DEX2 polygon envelopes. Their
  distinct vertex, matrix, move, cull, modify, branch, and triangle layouts
  normalize into the same typed mechanisms, and equivalent display lists have
  byte-identical raster regressions. Legacy self-load resets all published RSP
  geometry state while preserving independent RDP state; undocumented base
  quadrangle forms and unrepresented clipping policy remain loud. Both public
  texture-rectangle continuation forms now share one typed mechanism: raw RDP
  coefficient words and digest-selected legacy/modern `G_RDPHALF_1/2`
  envelopes are exhaustive across all admitted geometry/line families;
  malformed or mixed-family wrappers trap. See
  `crates/fn64-render-reference/FAST3D-F3DEX-CONCEPTS.md`. Exact digest identity
  now also distinguishes F3DLX and F3DLX.Rej despite their F3DEX-compatible wire.
  The bounded decoder enforces the ordinary 32-entry versus Rej 64-entry
  caches, legacy 32-vertex load maximum, F3DLX-only clipping toggle, Rej
  front-cull restriction, default `FRUSTRATIO_2`, and whole-triangle X/Y/far
  reject policy with no near rejection. Transformed F3DLX vertices remain a
  loud frontier because the public manuals name pixel precision without
  specifying its exact fixed-point rounding. Current F3DEX quadrangle
  emulation and F3DEX2 `G_QUAD` have byte-identical raster regressions; the
  removed historical dedicated Fast3D form remains loud because no allowed
  public source specifies its opcode/layout. See
  `crates/fn64-render-reference/F3DLX-CONCEPTS.md`. Exact digest identity also
  distinguishes public F3DEX2.NoN, F3DEX2.Rej, and F3DLX2.Rej. Their bounded
  policies cover the public 32/64-entry cache split, modern Rej 1--64 vertex
  loads, initial `FRUSTRATIO_2`, NoN near-admission distinction, and
  whole-triangle X/Y/far rejection without near rejection. F3DLX2.Rej
  transformed vertices remain loud at the same unpublished pixel-rounding
  boundary as legacy F3DLX. F3DZEX2 has a named identity but cannot enter HLE:
  allowed sources do not specify its family-specific continuation/branch wire,
  and public `G_SPECIAL_*` opcodes remain reserved. See
  `crates/fn64-render-reference/F3DEX2-VARIANTS.md`. The content-admitted
  S2DEX slice now retains object clamp, filter correction, perimeter, and ignored
  legacy edge requests as typed task-local state. It admits the current public
  header's ignored XLU/AA bits, Point-or-Average filtering with the shared RDP
  four-sample box path, exact-quarter-pixel 3/8-texel WIDEN edges, and
  NOTXCLAMP only where point samples provably remain inside the image. Integer
  point-sampled Copy backgrounds share an exhaustive row-major wrap
  partitioner across both S2DEX wires and LoadBlock/LoadTile. A distinct typed
  scaled planner preserves u10.5/u5.10 sample identity across both wrap axes,
  both wires/loaders, flip, fractional horizontal origins, and integer-valued
  `imageYorig`; its exhaustive sweep covers 168,000 configurations. Filtered
  scaled partitioning, vertical subpixel origins, filtered clamp bypass,
  WIDEN now composes with either public SHRINKSIZE through one exact typed
  perimeter mechanism across rectangle/matrix/rotating/compound paths and
  both S2DEX wires. WIDEN/filter/flip/Copy composition, historical XLU/AA
  revisions, and any required sub-quarter-pixel rounding remain loud. See
  `crates/fn64-render-reference/S2DEX-CONCEPTS.md`. The admitted family/opcode
  audit and its evidence boundary are recorded in `docs/MICROCODE-DENOMINATOR.md`.
  Public `gSPLookAt` X/Y
  DMAs now feed typed screen-direction state, and regular/linear texture
  generation maps signed
  normal projections or their inverse cosine through the `gSPTexture` scale.
  The two-command F3DEX2 `gSPForceMatrix` path now replaces the concatenated
  vertex transform while retaining the real stacks, and ordinary/modelview-
  only matrix loads correctly supersede it. Perspective normalization now
  retains its `.16` scale across ucode reloads and rejects geometry at zero;
  exact divider precision remains open. All four public clip-ratio destinations
  now retain per-side state, expand line clipping, and leave `G_CULLDL`
  frustum codes unchanged. The renderer seam now borrows the device fabric's
  one persistent DMEM/IMEM image. Public `G_DMA_IO` READ/WRITE commands move
  logical bytes between it and RDRAM in decode order, including same-task
  display-list rewrites. Compound `G_LOAD_UCODE` now loads its declared data
  prefix and fixed 4 KiB text image into those live banks before the next
  command. Its reset follows the public F3DEX2 maintained-state list rather
  than the older F3DEX contract: DL/matrix stacks, modelview/projection,
  segments, viewport, scissor, other mode, and perspective normalization
  survive while combined MP, geometry, lights, vertices, fog factors, texture
  selection, and clip ratio reset. Both HLE backends now require exact 4 KiB
  text SHA-256 admission at task entry; choosing F3DEX2 no longer trusts the
  rspboot-populated image. The reference backend applies the same catalog to
  self-load targets, so any other IMEM generation stops at the load before
  being misdecoded as F3DEX2. That preflight is
  transactional: rejected clone state is discarded and the runtime replays
  the complete ucode phase from untouched post-rspboot state through LLE,
  including BREAK and DRAM/XBUS DPC forwarding. RT64 now accepts those bounded
  raw ranges through its public LLE RDP entry and waits for render-to-RAM, so
  unknown task microcode does not depend on RT64 GBI recognition. Raw
  submissions carry the current VI output explicitly rather than relying on
  a preceding task call. That VI output is scanout state, not an RDP color
  register: the Rust F3DEX2/raw lanes now require persistent `G_SETCIMG`
  before color writes, while only the simple fixture decoder may synthesize
  an RGBA16 target from `output_addr`. Production task entry re-imports the
  selected RDRAM image so CPU/device writes between tasks remain visible. The Rust lane now owns one persistent RDP decode
  state shared by HLE tasks and raw DPC submissions: other mode, combiner,
  key/convert/constants, fill/scissor, texture-image/tile/TLUT registers, and
  physical TMEM survive `OSTask` boundaries while RSP-owned `G_TEXTURE` resets.
  Enabling texture without a live TMEM image traps rather than sampling white.
  Public tagged/RSP no-ops and RDP sync barriers are
  explicit handlers; reserved `G_SPECIAL_*`, non-public move subindices, and
  unknown opcodes trap with wire context instead of entering a silent-skip
  path. The fixture-only simple decoder now also rejects unknown/truncated
  commands and invalid vertex ranges rather than silently ending. Exact
  F3DEX2 decode now traps on a truncated command or texture-rectangle pair,
  command-budget cycle, over-deep call, malformed other-mode range, invalid
  vertex/triangle cache range, and incomplete vertex/matrix/viewport/light DMA
  before retaining partial or stale state. Transformed vertices now require
  the display list's explicit `G_MV_VIEWPORT` DMA rather than a fabricated
  320×240 host default. `G_AC_DITHER` uses the same typed per-fragment noise
  byte as combiner/dither noise instead of an ordered approximation. The
  public manual's exact pseudo-random hardware generator remains trace work. One/two-cycle ordered
  dither is implemented: RGB magic-square and Bayer selectors modify low color
  bits before target-format storage, while alpha Pattern and InversePattern use
  the selected ordered matrix. RGB and alpha Noise use the explicit seedable
  deterministic reference stream; its silicon identity remains unclaimed. The disabled path retains truncation behavior rather
  than rounding. Exact
  microcode catalog population/subdivision/rounding remains a frontier. The
  hardware NOISE combiner source consumes the same typed eight-bit sample
  rather than the former zero approximation. The lane decodes the
  full 16-byte texture-rectangle command and executes the public non-flipped
  RGBA16 copy-cycle path with per-tile state, fixed-point stepping, and the
  format-specific threshold rule: RGBA16's alpha bit is a direct write enable,
  including when blend alpha is zero. The
  one/two-cycle path now executes TEXRECT/TEXRECTFLIP through shared point,
  average, and documented three-nearest filtering, the color combiner, alpha
  compare, framebuffer blender, and distinct TEXEL1 input. Public Chapter
  13.7 mip/detail/sharpen LOD selection now shares one adjacent-coordinate
  derivative path across texture rectangles, F3DEX2 triangles, and raw RDP
  coefficient triangles. Primitive tile/max level, minimum LOD, modulo-eight
  tile selection, and RGB/alpha LOD_FRACTION inputs are retained in immutable
  per-primitive tile snapshots; missing selected tiles trap by index.
  Copy-cycle TEXRECTFLIP now applies the public S/T screen-axis swap while
  retaining copy-mode gradient scaling. Bounded DRAM command ranges submitted
  through `osDpSetNextBuffer` or raw DPC START/END execute the proven
  state/fill/texture subset without requiring a synthetic `G_ENDDL`. At
  `CMD_END`, the runtime captures the submitted words and stages that immutable
  command image at the physical 8 MiB boundary outside guest RDRAM before
  backend dispatch; task and raw-DPC renderer entries expose exactly the full
  8 MiB physical device and exclude the host allocation's appended MMIO/non-RDRAM
  backing. Later guest or RSP DMA writes cannot rewrite an already queued
  stream. The real C-lane boot reaches its first presented swap at
  executor step 445 after 28 graphics tasks, and 20/20 independent bounded
  runs completed cleanly. All eight
  raw RDP triangle layouts now have bounded widths and typed signed edge,
  shade, texture, and Z coefficient ingestion from SGI *RDP Command Summary*
  Tables 11-15. Edge-only, shade, Z, loaded-texture, and combined `0x0f`
  records execute as coefficient-bearing render operations through a direct
  pixel-center span walker: the major-edge bit chooses span sides and
  shade/texture/Z use `d/de + d/dx`. XBUS DMEM also submits a variable-width
  Z triangle. `Z_CMP` and `Z_UPD` now independently control compare and write.
  The Programming Manual Chapter 16 compressed-Z/DeltaZ codec is implemented
  and exhaustively checked. Passing updates commit both the visible compressed
  halfword and the two physical-address-owned hidden DeltaZ bits; selecting an
  image reloads both across task and switch-away/back boundaries. `G_SETZIMG`
  persists across color-image switches, and zero-DeltaZ depth-directed fills
  write raw halfwords while replacing covered software depth samples.
  Unmodeled state opcodes and arbitrary nonzero-DeltaZ depth fills fail by
  name. The flipped-copy, shim/MMIO
  raw-range, depth-image-clear, and focused raw-triangle gates each passed 10
  consecutive clean runs.
  `G_SETPRIMDEPTH`/`G_ZS_PRIM` now supply persistent uniform Z/DeltaZ to raw
  triangles and combined texture rectangles. Raw triangles, high-level
  triangles, and lines now retain the public eight-sample checkerboard identity
  in one type until the fragment boundary; exhaustive raw edge/scissor sweeps
  reject count-correct but identity-wrong masks. RGBA16 coverage persists across
  its visible LSB and the shared physical-address hidden-bit sidecar; all four
  `CVG_DST` rules, coverage/alpha selection, clear-on-wrap color writes,
  memory-coverage blending, and opaque coverage-wrap strict Z execute. The
  high-level F3DEX2 path now evaluates the same eight sample centers at polygon
  edges instead of manufacturing full center coverage. Full-coverage triangles
  retain center attributes; partial raw/high-level triangles share a typed
  covered checkerboard sample for shade, texture, and Z. Nearest-to-center and
  stable tie order remain a bounded policy because the silicon selector is
  unpublished. `G_SETSCISSOR` now
  retains the public field-enable and odd/even-line selector, and every color,
  depth, rectangle, raw-triangle, and high-level-triangle raster path rejects
  opposite-parity scanlines.
  Shade-dependent rectangle programs, pixel-Z rectangles,
  interpenetration coverage adjustment, exact coverage representative lookup,
  exact LOD derivative norm/fixed-point boundary behavior, exact alpha-coverage rounding,
  silicon-internal
  accumulator truncation/subpixel coefficient correction, same-visible-value CPU hidden-bit rewrites,
  mid-task RSP preemption/resume, arbitrary depth-
  fill hidden-bit behavior, and hardware-derived SP/DP timing are still
  required; no
  non-proven task may skip or synthetic-complete.
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
- [ ] **H3 `fn64-discover` gates cannot run off the author's machine.** Not
  env vars with defaults — compile-time `const`s: `gate_b1.rs:18/20-22`,
  `gate_d1.rs:18-19/24-27`, `gate_b2.rs:39/51` hold
  `/Users/jer/Downloads/...z64` and `/Users/jer/Code/aki-recomp/...`. The
  D1/B1 grading numbers cited in Phase D are reproducible by exactly one
  person. Fix: env var + a loud, named skip when unset (never a silent pass —
  that is the "silent shrug" AGENTS.md bans).

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
