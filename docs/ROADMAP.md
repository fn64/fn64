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
- [ ] **R5 audio out — STILL BROKEN (static + over-speed); two causes found
  and fixed, at least one remains.** Status 2026-07-17. Do NOT treat the
  landed fixes as closing this; the user still hears static.

  FIXED + verified so far: (a) rate mismatch — the harness ladders opened
  the stream at 48 kHz while the game produces 32 kHz with no converter, so
  the ring starved ~1/3 of the time and the callback zero-fill was static
  (CpalBackend now negotiates + linear-resamples producer-side); (b) the
  AI_LEN feedback loop was severed — osAiGetLength returned a static latched
  value, so the game could not pace production (now reports live ring depth
  converted to guest-rate bytes; regression-tested).

  NOT FIXED — the open frontier: the game STILL overproduces. Live evidence
  (heartbeat `ring_frames`, rs+rt64 shell, 40 s): ring pegged at exactly
  12000 frames (its 250 ms cap) from ~swap 250 to the end of the run, with
  ~180 AI buffers per 60 VI swaps (~3 per frame, where ~1 is expected).
  The drop-oldest ring cap then skips playback continuously = the static the
  user hears. So the AI_LEN fix did NOT change the producer's behavior.
  Next probes, in order: (1) confirm the guest actually CALLS osAiGetLength
  in the rs lane (its vram 0x800D32E0 IS in the host-lookup table, but that
  proves wiring, not calls) — log call count + returned value; (2) if it is
  called and ignored, read decomp `AudioMgr_ThreadEntry`/`AudioMgr_HandleRetrace`
  for what actually gates queueing (likely an osAiGetStatus AI_STATUS_FULL
  bit, or a retrace-message cadence our VI ticker over-delivers); (3) suspect
  the VI retrace ticker itself — if retrace messages arrive faster than
  60 Hz, the audio thread runs its whole produce cycle too often, which
  would ALSO explain the user-reported over-speed feel.

  Ruled out as *sufficient* causes — each was real, each was fixed, static
  survived all of them: App Nap throttling the producer; the 48 kHz/32 kHz
  rate mismatch; the severed AI_LEN feedback loop. The pattern to resist:
  every one of these looked like the whole answer at the time. A fourth
  plausible-and-partial cause is the expected shape of the next finding, so
  do not close R5 on a single fix that merely improves it — close it by ear,
  foreground and backgrounded.

  **THE REQUIREMENT IS FAITHFUL RATE — not "audio stops hissing"** (user,
  2026-07-17). The game must run at the SAME SPEED as hardware with audio and
  video both paced to it. Static is a symptom of a free-running producer; the
  over-speed feel is the same disease. Do not close R5 on quiet audio at the
  wrong speed. Close it only when BOTH hold, each with its own evidence:
  - **Video**: VI swaps land at 60 Hz NTSC (~16.67 ms) over a long run —
    median + p95, measured, not felt.
  - **Audio**: ~1 AI buffer per swap, ring depth stable rather than pegged at
    its 12000-frame cap, and right by ear (foreground AND backgrounded).

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
  - EXISTS: the shell's 60-swap heartbeat (`main.rs:406`) and `ring_frames`,
    which is how the pegged-ring evidence was gathered.
  - **BUILT 2026-07-17: `fn64_abi::retrace_cadence()`** -> `(ticks, secs, hz)`
    since the first tick, or `None` before any (never a fake 0 Hz). Counted in
    `Executor::advance_time` where ticks really fire; correlated with wall-clock
    in `fn64-abi` because `fn64-runtime` is wall-clock-free by design. Reported
    by the harness summary and, more importantly, on the shell's heartbeat line
    NEXT TO `ring_frames` — that pairing is the experiment.
    First reading, headless, 60 swaps: **59 ticks in 0.314s = 184.9 Hz**. The
    59:60 tick:swap ratio confirms the probe watches the right clock. The rate
    itself proves nothing here — headless runs unpaced, so ~3x is expected. THE
    NUMBER THAT DECIDES PROBE 3 IS THE SHELL'S, which paces its pump at 60 Hz:
    retrace_hz >> 60 there = ticker over-delivers = probe 3 CONFIRMED, one
    cause for both symptoms. retrace_hz ~= 60 with a pegged ring = probe 3
    REFUTED, cause is downstream in the producer. Either way the hypothesis
    stops being unfalsifiable.
    Suspicious but unproven: headless runs ~3x fast, and R5's evidence is ~3 AI
    buffers/frame where ~1 is expected. Do not read that as confirmation —
    headless SHOULD run fast. Measure the shell.

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

  **START HERE — four things changed under this item on 2026-07-17:**
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

- [ ] **R8 rdram word-swizzle is a recurring bug class, not three bugs.**
  Surfaced a third time in the R7 shell work (green-tinted, noisy logo in the
  window presenter); previously in DMA and framebuffer capture. Each time it
  was fixed at the site. AGENTS.md ("mechanism over patch") wants the sweep
  that finds the rest of the class, not a fourth one-off: every host<->rdram
  byte-order boundary should be enumerated and typed so the next instance
  fails to compile. Open because the fixes landed but the mechanism did not.

## Phase D — fn64 owns discover → decomp

Today OoT symbol/section metadata comes entirely from the zeldaret decomp
via aki-recomp's Python (`import_oot_syms.py`, `gen_stubs.py`). The zeldaret
answer key (10,833 named fns) makes OoT the perfect graded target.
fn64-discover has Phases 1/2/4/5 + bounded-6; the rest is design-only
(DISCOVER-DESIGN.md).

Current grading (D1+D1.5, 2026-07-16, `gate_d1` — but see H3: these numbers
are reproducible on one machine only): OoT 98.7% precision / 72.3% recall;
NW4E 48.4%/89.7%; NWXE 36.4%/28.5% (D1+D1.5+D2).

Two findings worth not rediscovering:
- **Recall is a Phase-2 problem, not a detector problem.** OoT recall was
  0.82% until D1.5 taught Phase 2 to discover DMA-table overlay load-images
  (OoT loads overlays via DMA tables, not descriptor tables), which alone
  took it to 72.3%. If recall stalls again, look at what Phase 2 exposes
  before tuning detectors — they can only hunt inside discovered images.
- **Table-derived candidates are an honest 0 everywhere.** Descriptor tables
  prove load-images, not entry points. Not a bug; don't "fix" it.

Still open from D1.5: resident `code`/`n64dd` destination discovery.
- [x] **D2 Phase-6 completion** (merged 2026-07-17) — jump tables +
  value-set analysis; OoT precision 90.6% -> 98.7%.
- [ ] **D3 Phases 7-8**: targeted dynamic probes; assembly/relink
  verification.
- [ ] **D4 pack emission**: emit fn64-owned `dump.toml`-equivalent
  (Decomp Pack) from discover output, replacing the aki-recomp Python for
  metadata production.
- [ ] **D-gate**: `recompile_rom` consumes the fn64-discovered pack for OoT
  and the boot is **byte-identical** (framebuffer SHA at fixed swaps) to the
  decomp-metadata build. Then produce the WM2000 pack with zero
  game-specific code.

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
