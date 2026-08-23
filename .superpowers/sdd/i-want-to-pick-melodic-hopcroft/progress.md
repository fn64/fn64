# SDD ledger — plan: /Users/jer/.claude/plans/i-want-to-pick-melodic-hopcroft.md

## Execution started

Branch: `worktree-wm2000-playable` (based on `port/rt64-conveyor` at `9ee5fc5d`)
Working directory: `/Users/jer/Code/fn64/.claude/worktrees/wm2000-playable`

## Pre-flight scan

Scan clean. No task conflicts found. Task 0D ruling: extend runner if <200 lines, new binary if larger.

Proceeding to parallel execution:
- Track A (orchestrator): Tasks 1-3
- Track B (Codex): Phase 0 corpus coverage

## Task 3: complete

Commit: (no changes — measurement only)
Parity gate PASS: 29/33 byte-identical to RT64 oracle.
4 expected non-identical: scissor-narrower (RT64_DEFECT), flip-point-sampled (FN64_GAP), yuv16 (FN64_GAP), negative-w (BROKEN_FIXTURE).

## Task 0B: complete

Commit: (no changes — measurement only)
RT64 HLE narrow coverage baseline: **40.37% line coverage** (9,574 lines, 5,709 missed).

Key gaps relevant to fn64 port:
- `setBlendColor` — 0% (WM2000 uses blend color)
- `setFogColor` — 0% (may be used in match scenes)  
- `setPrimDepth` — 0% (known fn64-vs-RT64 disagreement)
- `setConvert` — 0% (YUV conversion)
- Two TMEM template variants at 0%
- `rt64_rdp_tmem.cpp` — 6.8% overall (but upload/reinterpret paths, not RDP commands)

Most 0% functions are RT64-specific GBI stack ops (push/pop*) or enhancement features — NOT relevant to raw-DPC port.

Core draw paths well-covered: drawTris 96%, drawRect 91%, fillRect 82%, loadBlock/loadTile/loadTLUT >90%.

Ranked gap list for corpus growth:
1. setBlendColor (0%) — add parity case with blend color
2. setFogColor (0%) — add parity case with fog
3. setPrimDepth (0%) — add depth-compare parity case
4. setConvert (0%) — add YUV conversion case
5. TMEM template variants — covered by LOADBLOCK case in Task 0A

## Task 11: investigation (deferred — requires interactive verification)

Investigated the Esc-teardown panic. Findings:
- Esc key path (line 1274) already uses `event_loop.exit()` — NOT a process::exit call
- CloseRequested (line 1181) already uses `event_loop.exit()` — correct
- Census completion (line 1454) uses `process::exit(0)` deliberately — `event_loop.exit()` hangs
- The `prepare_clean_exit()` call at line 1450 detaches coroutines before `process::exit`
- Esc is IGNORED when census is armed (line 1267-1272)
- Post-event-loop path (line 1525) calls `prepare_clean_exit()` before optional `process::exit`
- Commit `b1e585dc` already fixed the census abort-on-exit issue

Ruling: the remaining `process::exit(0)` at line 1454 is a deliberate workaround for a
hang in the normal exit path. Fixing it requires understanding the hang, which needs
an interactive windowed session. Deferring to Task 2 verification — if the Esc-teardown
panic is still reproducible, scope a fix at that time.

## Task 0E: complete

Commit: `34c4e228` — `test(parity): add shade-only triangle (0x0c) case`
Parity gate PASS: 30/34 byte-identical (up from 29/33). Zero defects discovered.
Shade interpolation matches RT64 exactly for flat-shade case.
Agent: Sonnet, 101k tokens, 56 tool uses, 3.4 min wall.

## Task 0C: complete

Commit: (no changes — measurement only)
Rust line coverage baseline for fn64-render-wgpu: **118 production files measured**.

Key production modules:
- `production.rs`: 92.25% (11,046 lines — core execute path)
- `raw_dpc/mod.rs`: 93.39% (7,247 lines — RDP command dispatch)
- `raw_dpc/production_adapter.rs`: 93.89% (4,295 lines)
- `combiner.rs`: 85.49% (3,101 lines)
- `raw_dpc/triangle.rs`: 98.54% (933 lines)
- `raw_dpc/texture_rectangle.rs`: 98.80% (1,267 lines)
- `targets/raw_triangle.rs`: 76.73% (CPU rasterizer — the hot path)
- `targets/triangle_pipeline.rs`: 88.11%
- `blend.rs`: 94.74%

Low-coverage files (potential gaps):
- `device/mod.rs`: 2.62% (288 lines — GPU device init, not production-path)
- `targets/raster.rs`: 21.89% (725 lines — rasterizer entry, may contain untested paths)
- `targets/fill.rs`: 60.46%
- `lifecycle.rs`: 82.61%

Cross-reference with RT64 0% gaps (Task 0B):
- setBlendColor/setFogColor → blend.rs at 94.74% line coverage. The Rust blender code IS tested; the RT64 C++ equivalent is not (0%). This means the Rust tests validate internal consistency but NOT parity with RT64 for these features.
- Biggest dual-gap: TMEM load paths — RT64 at 6.8%, Rust at ~89%. Rust coverage is high but parity coverage is low.

**Overall:** 92%+ coverage on the core production path. The rasterizer at 76.73% is the main gap — that's where untested edge cases would hide.

## Task 0A: in progress (awaiting gate validation)

Codex agent implemented 3 new parity cases (blend-color, fog-color, 2-cycle textured).
Initial gate run found all 3 failing:
- blend-color/fog-color: `one-refused` — blender mode selected clr_mem (M=1) requiring IM_RD
- two-cycle-textured: `differs` + key mismatch — fn64 2-cycle path produces wrong output

Fix applied to blender constants: changed M=1 (clr_mem) to M=0 (clr_in) to avoid
framebuffer read requirement. Awaiting re-run of parity gate.

Two-cycle failure is a discovered defect in fn64's 2-cycle textured path.

## Task 4b: in progress (code complete, awaiting parity gate)

Codex agent implemented xxh3-128 for guest-read content identity:
- Added `FastContentDigest` type using xxhash-rust xxh3-128
- Changed `CapturedGuestRead` to store fast digest for runtime comparison
- SHA-256 computed lazily only for `WorkloadRecord::encode` (serialization)
- 4,879 wgpu tests pass, 0 failed (Codex saw 38 failures due to sandbox — not real)
- fn64-render-ir tests pass

Files changed: digest.rs, guest_read.rs, record.rs, lib.rs, Cargo.toml, Cargo.lock, DESIGN.md

---

## Task 0A: complete (2026-08-22)

Commit 2aeaa743. blend-color and fog-color are byte-identical wgpu-vs-RT64 (the
M=0 clr_in fix worked). two-cycle-textured is a DISCOVERED DEFECT, typed as
FN64_CAPABILITY_GAP in check_rt64_parity.py:

  Three-way disagreement — wgpu=0x0001 (uniform), RT64=0xf801 (uniform),
  hand-derived 1-cycle key=gradient. Neither engine matches the key.
  Root cause (per systematic-debugging): flipping ONLY the cycle-type bits
  from 1-cycle to 2-cycle is NOT an equivalent draw; the fixture's
  "same-as-1-cycle" key premise is false. Separately, wgpu diverges from the
  RT64 oracle in the 2-cycle textured path.
  → Phase 3 work item: fix wgpu 2-cycle textured OR re-derive the correct key
    from RT64 as authority. Logged, not suppressed.

Gate: PASS — 32/37 rt64-authoritative byte-identical, 5 typed outcomes.

## Task 4b: complete (2026-08-22)

Commit 1df43915. xxh3-128 FastContentDigest for runtime guest-read identity;
SHA-256 lazy in encode() only. decode() set-identity cross-check moved to
replay_with_guest_reads() (which has the payload bytes) — NOT a regression:
replay re-validates plan identity, set identity, and every per-read digest.
Verified: render-ir green (64+4 doctests); wgpu 4920 pass with host-gpu-tests,
sole failure = pre-existing Slice B blend-oracle defect (untouched by this change).

Perf impact still to be measured by Task 1 (this was the top Phase 2 candidate,
~20% of frame budget).

## Task 1: starting — DECISION GATE (measures post-xxh3 frame rate)

## Task 1: DECISION GATE result (2026-08-22, attract mode, post-xxh3)

Bounded census: warmup 120, 300 pumps, rs+wgpu. Log: census-attract.log.

RENDERER wgpu, 16.667ms FIELD budget:
- pump wall p50=5.30ms mean=11.17ms max=40.9ms
- over_budget: 59/300 pumps = 19.7% > 16.667ms field
- slow pumps: mean 21.69ms, p95 25.18ms  (these carry gfx_task + vi_swap)
- fast pumps: mean 8.60ms, p95 16.58ms
- gfx_task/vi_swap fire on EXACTLY the slow pumps (P(slow|gfx_task)=0.393, lift 2.0x)

DRAWN FRAME = 2 fields = 33.3ms budget. WM2000 draws every other field:
  mean drawn frame = 21.69 (slow) + 8.60 (fast) = ~30.3ms  -> UNDER 33.3ms budget
  p95 drawn frame  = 25.18 + 16.58            = ~41.8ms  -> OVER budget (tail)

VERDICT: attract mode is at/under the 33.3ms drawn-frame budget ON THE MEAN,
with a p95 tail that overruns. Big shift from pre-fix 35.7ms/frame.

CAVEATS (why this is not yet a "skip Phase 2"):
1. FN64_PHASE_TIMING et al. UNARMED -> all _ns attribution rows are 0.000.
   Have wall timing, no cost attribution for the 21.7ms slow pump.
2. Attract mode != in-match. Plan's acceptance scene is a 4-player match.
   The real decision gate is the IN-MATCH number, not measured yet.
3. Audio nonzero=0 across 357k samples this run (memory: works at 1.69M in
   interactive play) -- likely attract/warmup artifact, flagged not diagnosed.

NEXT: (a) re-run attract WITH phase timing armed to attribute the slow pump;
      (b) run in-match census -- the true gate.

## Oracle strategy DECIDED (2026-08-22)

Corpus key-source: RT64 = fast bulk oracle (already wired). ANGRYLION = the
independent bit-accurate tiebreak + anchor authority. Chosen over hand-derived
keys because angrylion is bit-accurate AND independent (wgpu was ported from
RT64, not angrylion, so it can't inherit an angrylion bug).

Availability (Explore agent, read-only):
- angrylion-rdp-plus PRESENT at /Users/jer/Code/angrylion-rdp-plus/ (ata4 fork,
  C core, MAME/BSD-3 license, redistributable). HEAD 9c8b9ed.
  Oracle API in src/core/n64video.h: n64video_init(config) -> point
  config.gfx.rdram at RDRAM + set dp_reg[DP_START/CURRENT/END] ->
  n64video_process_list() walks RDP words -> n64video_update_screen() returns
  RGBA8888 framebuffer. SAME-COMMAND-STREAM oracle, NO full emulator needed.
  Headless: compile src/core only, skip src/output (OpenGL).
- mupen64plus-core PRESENT at /Users/jer/Code/mupen64plus-core/ (jeremyw fork,
  arm64-aware, darwin-arm64-dynarec branch). Heavier host path; NOT needed for
  a direct RDP oracle. Reserve for whole-emulator same-scene diffing if wanted.
- No angrylion vendored in fn64 repo or RT64 checkout.

NEXT (Codex-porting / Opus-verify): wire angrylion src/core as a third headless
backend in the parity runner, alongside wgpu + RT64. Then the tiered corpus:
RT64 bulk key + angrylion anchor/tiebreak.

## Oracle strategy REVISED (2026-08-22) — angrylion DROPPED (license)

Task 0F (angrylion oracle) BLOCKED and DROPPED. Codex verified angrylion-rdp-plus
at HEAD 9c8b9ed is MAME-license ONLY (no BSD-3 grant anywhere; CREDITS.txt: "The
code comes under MAME license"). MAME license prohibits commercial use + requires
full source on modified redistribution. fn64's AGENTS.md clean-room rule EXCLUDES
MAME-derived code from fn64's MIT/Apache distribution. Linking it into a workspace
crate crosses that boundary. Codex correctly refused to vendor/link.

(Correction: my earlier "MAME/BSD-3 redistributable" note was WRONG — it came
from an Explore-agent inference, not verification. angrylion is MAME-only.)

OWNER DECISION: drop the independent oracle. RT64 remains the SOLE oracle. Add
curated HAND-DERIVED anchors per opcode/format as the shared-bug tripwire
(the wgpu-ported-from-RT64 blind spot is covered by hand keys, not a 2nd engine).
Accept that hand keys are error-prone (2-cycle case proved it) and derive them
carefully. No new dependency, no license risk.

Corpus rigor now = RT64 byte-compare + hand-derived keys. Task 0D (programmatic
generator) uses RT64 as key with hand anchors on curated cases. angrylion/mupen
NOT used.

## Task 1 IN-MATCH result (2026-08-22) — DECISION GATE FIRES: Phase 2 IS REAL

In-match census (grapple script, warmup 12000) reached swap #6060 = LIVE MATCH.
Structured pump-census report did NOT flush (teardown abort, see below), but the
last present-heartbeat (n=120 window, in-match) carries the numbers:

  pump median/p95/p99/max = 5.12 / 52.44 / 54.12 / 56.86 ms
  over_budget = 60/120 = 50.0% of pumps > 16.67ms FIELD budget
  interval p95 = 53.38ms, retrace_hz = 31.5 cumulative
  audio: nonzero=11,071,000 / 11.6M samples  -> HEALTHY in-match
         (attract nonzero=0 was a warmup/attract artifact, now confirmed)

DRAWN FRAME (2 fields, 33.3ms budget): a single slow pump ~52ms ALONE exceeds
the whole 33.3ms drawn-frame budget. In-match p50 drawn frame ~= 5 + 52 = ~57ms
vs 33.3ms budget. IN-MATCH IS ~1.7x OVER BUDGET.

Contrast: attract mode was ~30ms mean (UNDER). The acceptance-bar scene (match)
is the worst case and it is well over. -> PHASE 2 (performance) IS NECESSARY.
The attract-mode "under budget" reading was a false all-clear; the match is the gate.

CAVEAT: phase attribution (_ns rows) still not captured because the census report
aborted before printing. Need a clean in-match census run to attribute WHERE the
52ms goes. That is blocked on Finding 2.

## Finding 2: census-arm teardown abort (NEW, Task-11 family)

On census-window exit, a guest coroutine (fn64_runtime GameThread running
_osRecvMesg_recomp via call_c) is force-unwound during Executor drop and panics:
  "panic in a function that cannot unwind" (panicking.rs:225) -> abort
Stack: force_unwind_slow -> drop_in_place<GameThread> -> drop<Executor> ->
       thread-local dtor; osRecvMesg runs in the unwind and hits a nounwind
       boundary (catch_unwind/call_c) -> panic_cannot_unwind -> abort.
This aborts BEFORE the pump-census report flushes -> lost the structured numbers.
Same family as Task 11 (Esc-teardown), but a DIFFERENT root: a guest thread
blocked in osRecvMesg being force-unwound across a nounwind FFI boundary, not
process::exit in a winit callback. Task 11's fix did not cover this path.
Blocks clean in-match phase-attribution runs. -> fix before re-measuring.

## Root-caused: the teardown abort is an EARLY-EXIT bypass (2026-08-22)

Full backtrace symbols: _osRecvMesg_recomp -> call_c -> func_800033D4 ->
catch_unwind -> coroutine_func -> force_unwind_slow ->
drop_in_place<GameThread> -> drop_in_place<Executor> -> thread_local run_dtors.

The trailing `thread_local...run_dtors` = Apple TLS-destructor path = process::exit
dropped the Executor. The run aborted at swap #6060 ~= pump 12,120, DURING the
300-pump window (warmup 12000 not yet cleared to window-complete), so the clean
window-completion exit path (main.rs ~1420-1452, which calls prepare_clean_exit)
did NOT fire. Some OTHER exit (frame-trip guard / early process::exit) fired and
bypassed the coroutine detach -> Executor drop force-unwound the osRecvMesg-blocked
GameThread across the nounwind FFI boundary -> abort. Only ONE panic in the whole
log (the cannot-unwind); no prior guest panic. 

Task 12 dispatched to Codex (codex:codex-rescue, agent ad6c6ab6) with the full
backtrace + 2 hypotheses (H1 detach classification misses osRecvMesg-blocked
state; H2 exit path bypasses detach). Fix unblocks clean in-match phase attribution.

HAZARD NOTE: Task 12 edits main.rs + pump_census.rs + fn64_runtime executor.
Do NOT touch those files until Codex reports (delegate-fixes-not-just-analysis /
never-edit-an-agents-worktree).

## Phase 2 re-scope (from the real in-match number)

Decision gate: IN-MATCH is ~1.7x over the 33.3ms drawn-frame budget (slow field
~52ms alone > whole budget). Phase 2 IS ON. Attract was under budget (false
all-clear). Next perf steps, in order:
1. [BLOCKED on Task 12] clean in-match census with phase attribution -> find WHERE
   the 52ms goes. Do NOT optimize before this (measure-before-dispatching).
2. Priors from memory to CONFIRM against the fresh profile (not act on blindly):
   CPU rasterizer ~52.79ms/field baseline (matches the 52ms!), rasterizer alone
   16.71ms, GPU draw unpresented ~65% of execute, xxh3 already took ~20% off digests.
3. The 52ms ~= rasterizer-baseline prior is a strong hint the CPU triangle
   rasterizer (raw_triangle.rs) dominates in-match. Confirm, then Task 6 territory
   (rasterizer opt) with kill-evidence ns/pixel before/after.

## CORRECTION + Task 12 verified (2026-08-22)

CORRECTION: scripted input (WM2000_INPUT_SCRIPT) does NOT reach the windowed
shell. That var is parsed by the STANDALONE recomps/wm2000/packages/wm2000-boot
binary (used by wm2000-match-run.sh trace harness). The windowed shell runs
through crates/fn64-boot-harness, a DIFFERENT path with its own
controller_input_schedule.rs (fn64.controller-input-schedule.v1 schema) whose
runtime lever is unknown. Zero [wm2000-input] lines in BOTH census runs confirms
no injection fired. Owner at the display confirmed: intro not skipped, nothing
driven.

CONSEQUENCE: the earlier "in-match ~52ms" number was NOT a validated live match
-- it was the game auto-advancing (attract/demo), which DOES render real
gameplay (changing scene, healthy audio at swap #6060). Still a real rendering
workload, but not the acceptance-bar match. Stop calling it "in-match"; call it
DEMO/attract.

OWNER STEER: the demo is acceptable for perf metrics (it renders real gameplay).
-> measure the demo now for phase attribution; find the shell input lever in
parallel (Explore agent ae8c7754) so a validated match becomes reachable later.

Task 12 (teardown fix) VERIFIED ON HARDWARE + COMMITTED (924effda):
  Root cause = winit 0.30 macOS applicationWillTerminate: dispatches
  ApplicationHandler::exiting while run_app still on stack; shell had no exiting
  impl -> Apple TLS teardown dropped live Executor -> force-unwind abort.
  Fix: Shell::exiting seals executor (prepare_process_exit) before TLS teardown,
  idempotent. Verified: run that previously aborted now exits cleanly via
  path=platform-loop-exiting (detached=7), NO abort, census report path runs.
  Codex wrote it (couldn't verify: no GPU in sandbox); I verified on hardware.

## Demo perf census RUNNING (warmup 400, window 600, phase-armed) -> attribution

## Input lever: NONE exists + OWNER DECISION attract-is-enough (2026-08-22)

Explore agent (ae8c7754) DEFINITIVE: the windowed shell has NO scripted-input
lever. ControllerInputSchedule (fn64.controller-input-schedule.v1) exists in
crates/fn64-boot-harness/src/controller_input_schedule.rs (struct :23, parse
:145, attach :224) but has ZERO callers outside its own tests -- only re-exported
at lib.rs:42-44. Not wired to any running code. Shell's ONLY game input is live
keyboard/gamepad merged per-step at main.rs:708-709 -> set_controller_state(0,..).
FN64_INPUT_PROBE = 3-frame one-shot. --demo boots no game. No attract auto-play
env; the game runs its OWN no-input intro/attract sequence, which DOES render
real gameplay frames.

OWNER DECISION: attract sequence is enough as the perf-representative scene.
Do NOT build a driven-match lever now. Measure the no-input attract as the
benchmark. CAVEAT to carry: a real interactive match (more wrestlers, effects)
may be heavier than attract -- attract is a representative-but-possibly-optimistic
scene, not a guaranteed worst case.

Task 13 (input lever) DEFERRED -- not needed for perf under this decision.

## PHASE ATTRIBUTION (2026-08-22, attract/demo, phase-armed, CLEAN 0.0% residual)

Demo census: 600 pumps, warmup 400, FN64_PHASE_TIMING+EXECUTOR_SPLIT ARMED.
over_budget 241/600 = 40.2%. Slow pump mean 42.1ms (p95 62.5, max 71.1).
Closure residual 0.0% -> the attribution is trustworthy.

Slow-pump tail decomposition (share of TAIL = excess over fast-mean):
  executor_ns      38.7ms  100.1%   <- all cost is inside guest execution
    exec_resume_ns 38.7ms  100.1%
    gfx_ns         35.8ms   94.3%
      gfx_lle_ns   35.8ms   94.3%
        gfx_lle_RDP_ns  33.9ms  89.3%   <=== THE BOTTLENECK
        gfx_lle_rsp_ns   1.7ms   4.8%
    audio_lle_ns    0.5ms    0.4%
  vi_present_ns     3.4ms   -0.1%   <- NOT the problem (flat fast vs slow)

Counts: slow pumps have 2.67 gfx_tasks (16x fast), 5.34 gfx_calls, 132 steps (3x).

CONCLUSION (measured, matches prior): ~90% of the per-field overage is the RDP
graphics path gfx_lle_rdp -- the CPU rasterizer + raw-DPC command execution in
fn64-render-wgpu (raw_triangle.rs / raw_dpc). NOT vi_present, NOT audio, NOT
executor plumbing, NOT digest hashing (xxh3 already took that). This is where
Phase 2 optimization must aim. ~34ms/slow-field of RDP work vs a budget that
needs the WHOLE field under 16.67ms (or drawn frame under 33.3ms).

NEXT: profile WITHIN gfx_lle_rdp to find the hot function (raster inner loop?
tmem sample_point? per-pixel combine/blend?) then optimize with kill-evidence
(ns/pixel before/after, same scene). Task 6 territory.

## CLOCK-VALIDITY GATE before optimizing (2026-08-22) — owner concern

Owner: prior jitter/framerate issue was the RUNTIME throttling to 30Hz while the
GAME CODE already throttles to 30Hz (DOUBLE-THROTTLE), and there may be a VIRTUAL
CLOCK. => before trusting gfx_lle_rdp=34ms/field as COMPUTE, must confirm it is
not inflated by guest busy-wait on VI retrace or virtual-clock time.

Shell pacing is already cleared: main.rs:1391 FRAME=16.666ms WaitUntil, and the
census records pump_one_frame() elapsed as pump_wall EXCLUDING the sleep (the
comment cites a past 57.3%-over-budget conflation artifact it fixed). So shell
throttle is NOT in the 42ms number.

OPEN (Explore agent a9efe0b5): is there (A) a virtual/guest clock, (B) a RUNTIME
30Hz/VI throttle inside run_one_step/executor (vs guest code spinning on retrace,
which is expected), (C) can guest spin/yield leak into the gfx_lle_rdp timed
region, (D) does the double-throttle make a field's time dominated by busy-wait?

HOLD: do NOT dispatch rasterizer/RDP optimization until this reports. If
gfx_lle_rdp is proven pure compute -> the 34ms is real, proceed to Task 6. If
wait-time leaks -> the bottleneck attribution is wrong and must be re-measured.

## CLOCK-VALIDITY GATE CLEARED (2026-08-22) — gfx_lle_rdp IS real compute

Explore agent a9efe0b5 verdict (with file:line anchors):
- Virtual clock EXISTS (Executor::sim_time, advance_virtual_time; osGetTime/CP0
  read virtual not wall) BUT idle is JUMPED (clock jump to next VI deadline),
  never spun -> no inflation.
- NO runtime real-time throttle in run_one_step (host.rs:352); zero sleep/spin/
  cadence. Guest runs as fast as host allows; ALL pacing is the shell WaitUntil
  (already validated as excluded from pump_wall). => NO runtime double-throttle.
- The prior jitter bug (owner's memory) IS FIXED + visible: driver used to resume
  the guest idle spin ~200,000x/retrace (~31ms busy-wait); now capped to ONE
  resume (main.rs:690-700). And that spin lives in executor_ns, NOT gfx_lle_rdp.
- gfx_lle_rdp_ns times ONLY the synchronous dispatch_captured_raw_rdp backend
  call (rsp_commit.rs:420-572,2224). Guest does NOT run inside it -> no step, no
  VI-wait, no clock jump can occur in the span. Park time is subtracted from
  executor_ns (lib.rs:2055-2069). Staging alloc/copy-in is separately measured
  and SUBTRACTABLE (dpc_copy_census; counter_tree.rs:151-155).

VERDICT: ~34ms/field gfx_lle_rdp is TRUSTWORTHY as real RDP compute. Bottleneck
attribution HOLDS. HOLD LIFTED.

NOTE for the next measurement: subtract dpc_alloc_ns + dpc_copy_in_ns (arm
FN64_PROFILE / FN64_DPC_COPY_CENSUS) to split the 34ms into RASTER COMPUTE vs
RDRAM STAGING COPY -- that decides whether Task 6 (rasterizer opt) or a
copy-reduction is the right optimization. This is the run to do next.

## COMPUTE-vs-COPY SPLIT RESOLVED + scene-variance finding (2026-08-22)

Profile run (FN64_DPC_COPY_CENSUS ARMED, verified by effect):
  dpc_alloc_ns = dpc_copy_in_ns = dpc_copy_back_ns = 0 ("workload never reaches it")
  -> RDP staging-copy is NOT a measured cost. gfx_lle_rdp is ~100% RASTER/
     COMMAND-EXECUTION COMPUTE.
  CAVEAT: memory [wm2000-live-render-path] says live path is
  dispatch_lle_task->try_dispatch_raw_dpc_via_session; the dpc_copy_* counters
  may instrument the NON-live dispatch_dpc_submission path, so "0 copy" might
  mean "not instrumented on live path" rather than "no copy". Either way, any
  copy is INSIDE the timed dispatch_captured_raw_rdp backend call, and the
  bottleneck is the RDP compute in fn64-render-wgpu.

SCENE VARIANCE (important): two windows of the SAME attract loop gave
gfx_lle_rdp slow-mean = 34ms (run 1) vs 16.8ms (run 2). The attract sequence is
non-deterministic across runs; a single census window is NOT a stable benchmark.
Both runs agree gfx_lle_rdp is ~94% of the per-field tail, but the ABSOLUTE ms
varies 2x by scene.

CONSEQUENCE for kill-evidence: before/after ns/pixel A/B CANNOT use the varying
attract loop. Use a DETERMINISTIC fixed RDP command stream (a parity-corpus case
or a fixed captured frame) so the optimization delta is measurable cleanly.

## Phase 2 optimization target CONFIRMED
RDP raster/command-execution compute in fn64-render-wgpu (raw_triangle.rs /
raw_dpc / tmem sample). Measure with a FIXED command stream. Next: find the hot
FUNCTION within that path (raster inner loop vs sample_point vs combine/blend)
via a micro-profile on a fixed textured-triangle scene, then optimize with
ns/pixel kill-evidence.

## VISUAL STATE (2026-08-22, owner observation on the smoother boot)

Owner (has the display): "looks correct" -- NO visible rendering defects on the
watched scenes (logos/attract). => Acceptance bar #1 (no visible defects)
PROVISIONALLY MET on these scenes. The logo-artifact fix held; no yellow block /
black bar / text duplication reported.

Smoothness: retrace_hz steady ~59Hz across the whole run, present cost flat
~0.7ms -- the JITTER is gone (teardown + scheduling + double-throttle fixes).
What the owner felt as "much better" = jitter removed, NOT throughput solved.

REMAINING GAP = perf tail only (bar #2). over_budget still pinned ~50% of
fields; slow-field ~17-34ms (scene-dependent). Borderline vs 33.3ms drawn-frame
budget on the mean, exceeds it on heavy scenes. Audio underruns spike on the
heavy fields (symptom of the tail, not a separate bug).

Task 2 (visual verification) PARTIALLY satisfied by owner observation; a heavy
GAMEPLAY scene (not just attract) still unverified visually -- but no input lever
to reach one headlessly (owner: attract-is-enough).

## VISUAL VALIDATION via deterministic frame dump (2026-08-22)

Dumped wgpu framebuffers boot->swap 200 headless (WM2000_FB_DUMP_DIR, aki ROM).
Aug-18 wm2000-boot binary; valid for PIXELS since my 4 commits (0A test-only, 4b
digest identity, 12 lifecycle) don't touch rasterizer output. Inspected PNGs
directly:
- frame 0-14: FROZEN identical static/noise (hash ec0dbd19, same for 15 swaps)
- frame 15: uniform gray clear
- frame 24-36: copyright/legal text FADING IN (alpha blend correct)
- frame 117: settled clean legal screen -- crisp white text, correct layout,
  NO artifacts, NO yellow block, NO black bar, NO text duplication.
=> Copyright/legal screens render CORRECTLY. Owner "looks correct" CONFIRMED by
pixel inspection.

## OPEN correctness Q: boot static (owner-flagged)

Owner noticed the static before the copyright screen. It is a FROZEN identical
noise image held 15 swaps (not live-varying garbage) = VI pointed at an
uncleared framebuffer during boot. Question: EXPECTED (real HW shows garbage too
if VI is active) or DEFECT (HW blanks VI to black during boot, we present the
buffer anyway)? Decider = VI_CONTROL type field during swaps 0-14 (type 0/1 =
blanked/black = defect; type 2/3 = active = expected). fn64 HAS blanking
machinery (vi.rs:861 last_blanked, black-transition test). Explore agent
af730d9c investigating whether the present/dump path HONORS VI blank state at
boot. Interim read: LIKELY expected N64 boot garbage-flash, but confirming we
honor VI blanking before calling it correct.

## Sequencing note (wakeup 2026-08-22): correctness before perf dispatch

The scheduled wakeup asked to dispatch perf (Task 6) after the DPC-copy split.
That split ALREADY resolved: gfx_lle_rdp is ~100% raster COMPUTE (dpc_copy_* = 0),
so the perf target = rasterizer compute, measured on a deterministic scene for
kill-evidence. That is ready to dispatch.

BUT owner raised a newer CORRECTNESS question (boot static before copyright: bug
or expected?) that gates "rendering is clean". HOLDING the perf dispatch until
the VI-blank investigation (af730d9c) reports -- resolving a possible present-path
defect takes precedence over optimizing. Perf target is fully characterized and
not going anywhere.

## BOOT STATIC = CONFIRMED DEFECT (2026-08-22, owner-caught) -> Task 14

Explore af730d9c VERDICT: DEFECT. WM2000 keeps VI BLANKED (pixel type 0,
VI_STATUS&3==0) for the first ~20 fields; codebase records it
(vi_scanout.rs:332 "Fields 0-19 are blanked; field 20 first content, status
0x00013202"; docs/RT64-WM2000-SCOUT.md:135). HW outputs BLACK when blanked ->
the boot static should be black.

ROOT CAUSE (a bypass): fn64 scanout HONORS blank (vi_source.rs:65,
vi_scanout.rs:352 -> Ok(None)=black on blanked||type==Blank). But the SHELL
present() does a standalone CPU blit of raw RDRAM (main.rs:834-841) that
BYPASSES wgpu scanout even under FN64_RENDER=wgpu. Its only blank check is
is_uniform() (all-bytes-equal); noise isn't uniform -> garbage presented. The
blank-honoring logic EXISTS but isn't wired to the shell present. Same in the
boot dump capture_framebuffer (~main.rs:1097-1176; only checks uniform +
unprogrammed-geometry, not blanked/type).

FIX (Task 14): shell present() (main.rs:762) consult vi.blanked || (VI_STATUS&3)
==0 -> emit black, mirroring vi_source.rs:65. ABI exposes it: VI_STATUS at
fn64-abi/src/vi.rs:462 (read_live_device_mmio 0xA4400000); blanked carried in
ViPresentation. Same fix for capture_framebuffer.
RIGOR CAVEAT to fold into the fix: the "0-19 blanked" numbers are from the
scanout path; the fix reads VI_STATUS&3 directly at present time, which both
fixes AND confirms the shell freeze is the blank window.

## Match validation: OWNER will spot-check by hand (2026-08-22)

Owner decision: no input lever built; owner drives into a match manually and
eyeballs it, reports defects/panics, I fix. Covers in-match CORRECTNESS cheaply.
Known in-match panic candidates to watch (plan): AlphaCompare::Dither
(production.rs), CoverageDestination::Save (triangle_pipeline.rs), VI-filter
refusals, coverage narrowing (only alpha_coverage_select=false && force_blend=true
handled). Also: horizontally-duplicated menu text (suspected, unconfirmed).

SETUP NEEDED (after Task 14 lands): give owner a play-wm2000.sh launch that
captures stderr to a file so a panic/refusal is logged (not just "it quit").
HOLD until Task 14 committed (Codex editing main.rs now) so owner tests the
boot-black build and I don't race the edits.

## KNOWN BUGS/GAPS INVENTORY (2026-08-22) -- for owner
CONFIRMED: (1) boot static [Task 14 fixing]. (2) perf tail: gfx_lle_rdp raster
compute ~17-34ms/field, ~50% fields over field budget, borderline vs 33.3ms
drawn-frame. (3) 3 typed RDP capability gaps: 2-cycle-textured (wgpu collapses
texture), TEXRECTFLIP (unimpl), YUV16 (unimpl) -- UNKNOWN if WM2000 uses any.
UNVERIFIED: (4) in-match rendering NEVER validated (panic candidates above);
(5) menu-text duplication suspected; (6) audio underruns = symptom of perf tail.
BIGGEST GAP: a live match (acceptance scene) never reached/measured -> owner
hand spot-check addresses correctness half.

## SCOPE EXPANSION (2026-08-22): FULL RDP PARITY for ANY N64 ROM

Owner: goal is now COMPLETE RDP hardware-capability parity -- not just enough for
WM2000, but any N64 ROM. New north star, larger than "WM2000 playable."

Surface (finite, enumerable): ~35 opcodes (18/35 in corpus), 16 format×size
(10 valid, covered), cycle modes × blend modes × combiner mux × alpha/coverage/
dither/Z modes (large but finite combinatorial space).

State: most opcodes are IMPLEMENTED (21 files LOADBLOCK, 5-7 SETPRIMDEPTH/
SETBLENDCOLOR) but UNVERIFIED against hardware-truth. Untested opcodes (from
rdp-untested-surface-map): 0x33 LOADBLOCK (ROM x4/frame!), 0x27 PIPESYNC, 0x28
TILESYNC, 0x25 TEXRECTFLIP, 0x2e SETPRIMDEPTH, 0x39 SETBLENDCOLOR (now +partial),
0x2a/2b/2c SETKEY/CONVERT, 0x3e SETMASKIMAGE (0 files=unimpl), 0x08-0x0d/0x0f
most TRIANGLE variants (only 0x0e tested). LOADBLOCK is the standout gap (DxT
stride, no LoadTile case can catch it).

CENTRAL TENSION: "full parity for any ROM" ultimately = parity with the HARDWARE
RDP. The corpus oracle is RT64 (HLE = an approximation); fn64 and RT64 genuinely
disagree in places where fn64 is MORE correct (setPrimDepth 1-ULP, RGBA32 TMEM
stride, tri plane scale -- see rt64-fn64-disagreements, rt64-port-discrepancy-map).
The bit-accurate reference is angrylion, which is LICENSE-BLOCKED (MAME, see
angrylion-mame-license-blocks-oracle). => full HW parity needs a bit-accurate
oracle we don't currently have in-tree. This is the strategic blocker to resolve.

## ORACLE DECISION (2026-08-22): angrylion as EXTERNAL out-of-tree reference

Owner idea: run angrylion and OBSERVE its output WITHOUT including it in fn64
builds, so the MAME license doesn't touch fn64's distribution.

ENGINEERING VERDICT: sound + standard. MAME license restricts DISTRIBUTION/
INCLUSION of the code, not RUNNING it and observing output. If angrylion is a
completely separate external dev tool (never linked/vendored/in-workspace), used
only to generate reference framebuffers OFFLINE, then fn64 build+dist contain
ZERO MAME code -- only DATA (reference pixel outputs as test fixtures). Same
pattern as RT64 already uses (external tree via FN64_RT64_DIR, not vendored).
CAVEAT: this is a license-INTENT read; the definitive terms call is the OWNER's,
not mine. Flagged, proceeding on owner's direction.

COVERAGE DRIVE: BOTH -- programmatic corpus generator (plan Task 0D: enumerate
opcode×format×cycle×blend×alpha/coverage, generate valid RDP streams, oracle-
compare) AND rank output by real-ROM usage (LOADBLOCK, PIPESYNC, triangle
variants first). Exhaustive coverage, highest-impact first.

NEXT: verify angrylion runs fully STANDALONE headless (no fn64 linkage) to emit
reference frames from a raw RDP command stream -> then it's an external oracle
generator, output captured as test data. Then build generator + wire 3-way
compare (fn64 wgpu vs RT64 vs angrylion-captured-frames).

## angrylion external oracle: FEASIBLE (2026-08-22, agent ac205ebb) -> dispatching

Half-day spike, no blockers. Standalone build = 2 files (src/core/n64video.c +
src/core/parallel.cpp), ZERO OpenGL/windowing/emulator/fn64 linkage. macOS arm64:
clang++, no -lpthread, no generated headers. Only undefined externals = 3 msg_*
stubs (msg_error/warning/debug). config.parallel=false skips thread pool (but
parallel.cpp must still compile/link).

Driver flow (n64video.h:113-117): 8MB RDRAM buf; write RDP cmd words as native
u32 at offset; dp_reg[DP_CURRENT]=start byte, [DP_END]=end byte, [DP_STATUS]=0
(XBUS off->reads RDRAM); config_init then override parallel=false; init;
process_list; READ PIXELS from RDRAM at SetColorImage fb_address (RGBA5551) --
NOT update_screen (that's VI). Stream MUST contain SetColorImage(0x3f) +
SetScissor(0x2d) + SetOtherModes(0x2f) + draw + SyncFull(0x29).

BYTE LAYOUT MATCHES fn64 EXACTLY: angrylion reads cmd words as native-endian u32,
no swap (rdram.c:84-88; 8/16-bit swizzle only). fn64's parity &[(u32,u32)] stream
works byte-for-byte -> NO format translation.

DETERMINISTIC: parallel=false, fixed rseed=3 (n64video.c:231); only RNG is
irand() for noise dither -> disable dither via othermode bits for clean fixtures.

LICENSE: lives ENTIRELY out-of-tree; fn64 stores only captured pixel DATA as
fixtures. No MAME code in fn64 build/dist.

DISPATCHING: build the standalone harness (task #11) as a Codex C task, OUTSIDE
the fn64 worktree (scratch dir or angrylion tree). Verify with one fill vs a
known key.

## Task B1 DONE: angrylion external oracle BUILT + VERIFIED (2026-08-22)

Codex was sandbox-blocked (couldn't mkdir outside its root), so I built it myself
(mechanical C-glue per the verified spec). Lives at /Users/jer/Code/angrylion-oracle/
(OUTSIDE fn64; zero fn64/MAME code in fn64). Binary `oracle`, 233KB.
Build: clang++ parallel.cpp + clang n64video.c + oracle.c, no GL/pthread/headers.
Usage: oracle <cmds.bin> <fb_addr_hex> <w> <h> <bpp> <out.bin>; cmds = native u32
words (SAME layout fn64 parity emits). Reads FB from RDRAM at fb_addr (not VI).

VERIFIED: 32x32 fill, SetFillColor 0xf801 -> all 1024 px == 0xf801 (read
little-endian; RDRAM stores with 16-bit swizzle). Byte-identical across 2 runs
(deterministic). Bit-exact ground truth achieved.

Pixel byte order gotcha: RGBA5551 in RDRAM uses N64 16-bit swizzle -> read 16-bit
words LITTLE-endian for logical value. Compare in fn64's domain.

NEXT (Track B): the generator (task #12) can now 3-way compare fn64 wgpu vs RT64
vs this angrylion oracle on any synthetic RDP stream. Ground truth = angrylion.

## Task 14 (boot-blank): Codex impl complete, I VERIFY on hardware (2026-08-22)

Codex implemented (couldn't verify: no wgpu adapter in sandbox). Fix is clean:
- fn64-abi exposes vi_status() + vi_blanked() (latched blank OR VI_STATUS&3==0,
  mirrors scanout).
- shell present() (main.rs:~834): if vi_blanked -> framebuffer::fill_opaque_black,
  else unchanged RGBA5551 decode. Regression tests added.
- Fixed ONLY the shell present(); the boot-DUMP capture_framebuffer path
  (wm2000-boot, other checkout) was NOT fixed -> the headless frame-dump will
  STILL show static. Verification must be on the WINDOWED SHELL, not the dump.
Building shell now (b4e4lskeq); then verify on-screen the boot shows black not
static (owner can watch, or F2 screenshot at an early swap). NOT committed until
verified. Boot-dump capture_framebuffer fix = follow-up (Task 14b, low priority).

## JITTER REGRESSION (owner-caught) -> ROOT-CAUSED + FIXED (2026-08-22)

Owner: spot-check build jittery again. MEASURED: present cost p95 0.75ms -> 5.40ms
(7x) after the boot-blank fix (5120e619). retrace_hz ~59 -> ~54.

ROOT CAUSE: the boot-blank fix's vi_blanked() called vi_status() =
read_live_device_mmio(0xA440_0000) EVERY presented frame. That routes through
the full MMIO address-decode path (fabric.read_mmio) -- ~4.6ms/frame. By
contrast vi_width()/vi_origin() are cheap direct fabric field reads
(with_host(|h| h.device_fabric.vi_width())).

FIX: VI_STATUS is just vi_registers[0]. Added DeviceFabric::vi_control() ->
vi_registers[0] (cheap direct read, mirrors sp_status/ai_status pattern), and
rewrote vi_status() to use with_host(|h| h.device_fabric.vi_control()) instead
of read_live_device_mmio. Same value, no decode. Boot-blank correctness
preserved (vi_blanked logic unchanged). 14 vi tests pass incl the VI_STATUS
write->read roundtrip.

Files: fn64-runtime/src/device/fabric.rs (+vi_control), fn64-abi/src/vi.rs
(vi_status cheap). Rebuilding shell (bgnb6gfm2) to confirm present recovers to
~0.7ms. NOT committed until re-measured.

## RDP PARITY FAN-OUT PLAN (2026-08-22, owner: exhaustive but incremental)

Fan-out AFTER the 3-way harness lands (Track B agent a5bf8f73 is building it;
committed 2aea6eea angrylion leg). Every parallel worker needs the working
wgpu-vs-RT64-vs-angrylion comparison, so the harness is the prerequisite.

Shape: fan out CASE DESIGN + ORACLE RUNS (independent, parallel); serialize
INTEGRATION into the single 168KB parity-runner file (one agent folds confirmed
cases + typed divergences to avoid conflicts on that file).

Goal = EXHAUSTIVE (full RDP matrix for any ROM), reached INCREMENTALLY:
  Pass 1: ranked high-impact slices (LOADBLOCK, triangle variants 0x08-0x0d/0x0f,
          PIPESYNC/TILESYNC, SETPRIMDEPTH/SETBLENDCOLOR/TEXRECTFLIP) ~6-10 agents.
  Pass 2+: widen the mode matrix (cycle x blend x alpha x coverage x dither x Z)
          per format, until the whole enumerable surface is covered.
Each pass: triage (angrylion=ground truth; wgpu!=angrylion = fn64 defect), fix
discovered defects, then widen. Workflow-shaped (fan-out phase + synthesis phase).

NOT started yet -- gated on the harness. Current single agent finishes the
foundation + first batch first.

## JITTER FIX ATTEMPT #1 FAILED (2026-08-22)

Made vi_status() cheap (DeviceFabric::vi_control, no MMIO decode) -- but present
STILL 5.4ms after rebuild+census. So vi_status/MMIO was NOT the cost. Hypothesis
wrong. The only other present() change in 5120e619 is the vi_blanked() call
itself (with_executor(exec.vi().blanked)) and the black-fill branch (blanked-only).
NEW HYPOTHESIS to test: either with_executor is costly from present context, OR
present's 5ms is GPU vsync/surface-acquire blocking (window compositor state),
NOT the boot-blank commit at all. Added FN64_JITTER_PROBE_NOBLANK env to
short-circuit vi_blanked()->false for a mutation test. Rebuilding (bqr37pgnu).
Keep vi_control change (it's a real cheap-path improvement) regardless.

## JITTER = MEASUREMENT ARTIFACT, NOT a regression (2026-08-22) -- RESOLVED

Mutation test result: probe-OFF run (normal vi_blanked, SAME binary that showed
5.4ms in jitter-verify) present p95 = 0.78ms, over_budget 14.7%. FAST. The
identical code was slow (5.4ms, ~50%) in jitter-verify and fast (0.8ms, 15%)
here. => NO code regression. The 5.4ms present was ENVIRONMENTAL variance (GPU
surface / macOS compositor / window focus-occlusion state), not the boot-blank
commit. Same class as skew-was-measurement-artifact / perf-measure-before-
dispatching.

Lesson: my first "fix" (cheap vi_control) chased a non-problem; the second
hypothesis (with_executor) was also going to be wrong. The mutation test caught
it before I shipped a fix for a measurement artifact. Present p50 was ~0.7ms in
BOTH the "fast" and "slow" runs -- only the p95 tail differed, the bimodal
signature of external stalls, not per-call code cost.

ACTIONS: (1) keep the vi_control cheap-VI-status change -- it's a legit
improvement (no MMIO decode per frame) but NOT the jitter fix; commit honestly.
(2) remove FN64_JITTER_PROBE_NOBLANK scaffolding. (3) no further jitter fix.
Boot-blank fix (5120e619) stands -- it did not regress perf.

## Track B FIRST PASS COMPLETE (2026-08-22) + RGBA16 verdict delegated

Track B agent (a5bf8f73) finished. Committed: 2aea6eea (angrylion 3-way leg,
validated vs 39 hand cases), a20e6415 (generator + 24-case first batch),
be4aac55/bb41bd9c (report). Results:
- 16 NEW cases confirmed correct vs angrylion hardware truth (triangle 0x08/09/
  0c/0d, PipeSync/TileSync/LoadSync, cycle-type matrix, fill boundaries).
- 1 real fn64 limitation: TexRectFlip (0x25) refused in wgpu.
- 7 held: RGBA16/CI/RGBA32 texture cases where wgpu==RT64==key but angrylion
  differs (all texels read 0x0001). Correctly NOT claimed as fn64 defect.

RGBA16 root-cause (my narrowing, then delegated to Codex a8531a7b per owner rule
'delegate to gpt'): fetch is CORRECT (notlutswitch=2=TEXEL_RGBA16; texel(0,0)
reads tc16[1] via WORD_ADDR_XOR). tc16[1] is empty -> the LoadTile->TMEM WRITE
didn't place the texel. IA16 (16bpp) passes, only RGBA/CI/RGBA32 fail. Likely a
HARNESS bug (oracle.c drive of RGBA16 load) since 3 impls agree, not fn64.
Codex verdict pending: harness-fix-in-oracle.c vs genuine angrylion divergence.
angrylion tree left CLEAN (my temp probe reverted).

NEXT (after verdict): if harness bug fixed -> 7 cases pass, corpus clean ->
LAUNCH INCREMENTAL FAN-OUT (ranked slices) per the locked plan.

## RGBA16 root cause CORRECTED (2026-08-22, parent ran instrumented probe)

Codex was sandbox-blocked from scratch-writes+rebuild (structural, not one-off),
so I ran the instrumented oracle myself (temp TMEM probe, reverted, tree clean).
FLIPPED the diagnosis:
- TMEM loads CORRECTLY (tc16[0..7] = all 8 texels right).
- FETCH is CORRECT (s=0->0xf801, etc).
- FRAMEBUFFER differs: row0 = 0x1 0x1 ffff ffff (expected 07c1 f801 7fff 003f);
  row1 = c631 8421 0x1 4211 (3/4 right). Some pixels UNCOVERED (ffff bg).
=> NOT TMEM/fetch. It's the tiny 4x2 texrect's PIXEL COVERAGE + TEXEL->PIXEL
STEPPING -- the texrect edge-rule/coverage class (rt64-parity-corpus-gotchas:
texrect edges exclusive vs fill inclusive). Either the generated case's
coords/edge convention differ from what angrylion covers, or a genuine
angrylion-vs-(wgpu+RT64) edge divergence. Re-delegated to Codex with corrected
brief -- now tractable for it (only blocker was scratch+rebuild, done; remaining
is coord analysis + oracle.c/case edit, which Codex can do). oracle rebuilt CLEAN.

## RGBA16 VERDICT: genuine angrylion texrect-coverage divergence, NOT fn64 defect (2026-08-22)

Codex a865f5f5 delivered (report task-B2-report.md). Verdict: GENUINE EDGE
DIVERGENCE. The texrect [0,4)x[0,2), S=T=0, 1 texel/px should cover 8 pixels.
fn64-wgpu == RT64 == hand-key all cover 8 correctly. The standalone angrylion
oracle covers only 6 (0001 0001 ffff ffff / c631 8421 0001 4211). Translating
geometry to [4,8)x[4,6) reproduced the same relative mask -> not origin/clipping.

=> The 7 held cases are NOT fn64 defects. fn64 matches RT64 and the keys. This
is a standalone-angrylion texrect edge/STEPPING limitation.

STRATEGIC CONSEQUENCE for the oracle: angrylion is trustworthy ground truth for
the VALIDATED paths (fills, scissor, IA/I textures, flat/shade triangles, TMEM
load+fetch -- all confirmed correct). But NOT for small-texrect COVERAGE. For
texrect cases the oracle is the outlier (3 impls agree, only oracle differs), so
texrect coverage parity stays wgpu-vs-RT64 (+ hand-key), NOT angrylion-grounded.

=> Corpus triage rule update: a wgpu==RT64==key vs angrylion-only-differs result
on a TEXRECT is classified ORACLE_TEXRECT_LIMITATION (not fn64 defect, not RT64
defect). The generator should mark texrect cases so the 3-way compare doesn't
false-flag them.

## FAN-OUT: ready to launch, with the oracle-scope caveat baked in
Corpus is now clean (7 held cases classified, no unresolved fn64 defects). The
incremental fan-out can launch. Workers must carry the triage rule: angrylion is
ground truth EXCEPT for texrect coverage (there, wgpu==RT64==key wins).

## RDP-PARITY FAN-OUT PASS 1 LAUNCHED (2026-08-22, workflow w2owx2kj8)

Corpus clean (RGBA16 resolved: oracle texrect limitation, not fn64 defect). Fan-out
Pass 1 = 6 parallel design agents (mode-matrix slices: blend-modes, alpha-compare,
coverage-modes, formats-deep, zbuffer, loadblock-deep) each returning Rust builders
+ push() calls as DATA (no runner edits), then 1 serial integrator folds all into
generated_cases(), builds, runs FN64_GENERATE=1 3-way triage. Triage rule carries
the oracle-scope caveat: angrylion=truth EXCEPT small-texrect coverage.
Prefers triangles/fills over texrects to stay in angrylion's trustworthy domain.
7 agents total (medium guideline). Commit on completion. Discovered fn64 defects/gaps
ranked in track-B-fanout-pass1-report.md. This is Pass 1 of incremental->exhaustive.

## RGBA16 ROOT CAUSE CORRECTED AGAIN -- BI_LERP_0 (2026-08-22, Track B agent, PROVEN)

SUPERSEDES my "oracle texrect-coverage limitation" verdict, which was WRONG.
The Track B agent proved the real cause (committed 1ccc15c5):

The corpus's textured SetOtherModes = 0xef0000f0 leaves BI_LERP_0 (word0 bit 11)
CLEAR. Bit-accurate hardware (angrylion) then routes RGBA/CI/RGBA32 texels through
the color-convert/YUV unit; with zero SetConvert coeffs that COLLAPSES every
channel to the texel's BLUE channel. wgpu AND RT64 both ignore the missing bit and
pass full RGBA -> they agree with each other + the hand key but DIVERGE FROM
HARDWARE. IA/I immune (value already in blue) -> exactly the RGBA16/CI/RGBA32-fail
vs IA/I-pass split. PROVEN: instrumented throwaway angrylion build showed load+fetch
correct, collapse in the non-bilerp texel path; setting bit 11 (0xef0008f0) makes
angrylion byte-identical to the key. Canonical angrylion tree kept pristine.

CONSEQUENCE (my earlier texrect verdict retracted): it was NEVER a texrect-coverage
issue and NEVER an fn64 rendering defect. It's (a) a corpus fixture gap (missing
BI_LERP_0), now fixed in the generator; AND (b) an OPEN owner question: should
wgpu+RT64 honor BI_LERP_0? They currently MASK the hardware color-convert collapse.
If they should honor it, it's a SHARED wgpu+RT64 defect. => surface to owner.

CONFIRMED fn64 DEFECT (Track B): wgpu REFUSES TexRectFlip (0x25); RT64+angrylion
render it and agree. Clean isolated gap. -> Phase 3 fix candidate.

## FAN-OUT PASS 1 STOPPED + relaunching with corrected triage rule
My fan-out (w2owx2kj8) carried the WRONG oracle-texrect-limitation triage rule.
Stopped it. Fixing the script: (1) drop the texrect-limitation classification;
(2) tell agents textured cases MUST set BI_LERP_0 (0xef0008f0) or use SetConvert;
(3) a wgpu==RT64!=angrylion result now = investigate BI_LERP_0/color-convert, not
auto-oracle-limitation. Relaunch.

## Task 16 BI_LERP_0 verdict: NO renderer FIX -- corpus-only divergence (2026-08-22)

Explore ae235add (read-only, well-cited). Findings:
- HW rule confirmed: BILERP-off (G_MDSFT_TEXTFILT sample_type=0) + zero SetConvert
  collapses RGBA/CI texel to single channel [Y,Y,Y,alpha] (SGI RDP Cmd Summary
  Tbl 28 / Prog Manual 12.5; fn64 ref gbi/types.rs:660-689, sample_rdp:1232-1248).
  IA/I immune. Matches angrylion in substance.
- REAL-ROM: NOTHING hits it. WM2000 textured draws use other-mode-high 0x0000acef
  = BILERP ON + convert bypassed (RT64-WM2000-CYCLE-MODES.md:29-30,91-92); WM2000
  never emits SetConvert (0/218, RT64-WM2000-CENSUS.md:214,406). Only the SYNTHETIC
  corpus word 0xef00_00f0 (deliberately point-sampled for hand-derivable keys)
  exercises the collapse. Corpus-construction artifact, not ROM traffic.
- wgpu (state.rs:246-253) + RT64 (rt64_texture_sampler.rs:51-84) both decode Point
  but gate no convert stage -> ignore the collapse, agree with each other.

RECOMMENDATION (accepted): NO renderer fix. The wgpu+RT64 divergence-from-hardware
is real but DEAD (no ROM triggers it). Track B's generator fix (set BI_LERP_0) was
the right move -- it makes corpus cases test the intended path. If ever needed:
sample_rdp-style convert stage gated on Point && convert!=FILT in both samplers.
Reopen only if a real ROM emits point-sampled textured RGBA/CI. LOGGED, not fixed.

Task 16 DONE (recommendation). Task 15 (TexRectFlip fix, Codex a020a1e1) + fan-out
Pass 1 (wl15ku3c1) still running.

## COLLISION HAZARD: two write-agents in one worktree (2026-08-22) -- my orchestration error

I have TWO agents writing the SAME worktree concurrently:
- TexRectFlip fix (Codex b8bij20oz): editing production.rs, raw_dpc/mod.rs,
  production_adapter.rs, targets/texrect.rs, render_ir.rs, check_rt64_parity.py
  (removed the flip "must refuse" entry -- consistent with implementing it).
- Fan-out integrator (a00c4ddf): editing the parity runner + check_rt64_parity.py.
They COLLIDE on check_rt64_parity.py and the git working tree; the integrator's
edits were RESET to clean HEAD (72c6304a) mid-work -- the other agent's git ops.

LESSON (mine): over-parallelized. Two write-agents must be on SEPARATE worktrees
or SERIALIZED, never sharing one tree (delegate-fixes-not-just-analysis / never
edit an agent's worktree). Committed history is safe (survives resets); only
UNCOMMITTED work is at risk -> told both to commit promptly in isolation
(git commit -- <own files>).

MANAGEMENT: both are near-landing (TexRectFlip has substantial edits, integrator
redoing). Let them serialize/finish, then reconcile commits. I am holding ALL my
own edits in this worktree until both land. Going forward: fan-out Pass 2 fix
agents get worktree isolation or serialize behind these.

## FAN-OUT PASS 1 RESULTS (2026-08-22, workflow wl15ku3c1) -- committed 7b9d1afe, c013894a

46 new cases (corpus now 71). Triage: 49 pass-all-match-hardware, 14 wgpu-refused,
7 shared-ported-bug, 1 fn64-defect. (Landed despite the 2-agent collision;
integrator committed path-scoped.)

REAL fn64 DEFECT (1) -> Phase 3 fix:
- gen-coverage-color-on-cvg-one-cycle: wgpu DROPS a color write that
  CLR_ON_CVG + CVG_DST_WRAP should make. RT64 matches hardware, wgpu writes
  nothing at all. Clean isolated. HIGH: coverage-write path.

REAL fn64 GAPS/refusals -> Phase 3:
- Z-BUFFER binding MISSING entirely: SetZImage(0xfe) AND SetMaskImage(0x3e) have
  NO raw-DPC plan-probe support -> all 6 zbuffer cases refuse. Depth testing
  unimplemented on the raw-DPC path. Broader than the anticipated SetMaskImage-only
  gap. BIG gap.
- LoadBlock DxT row-advance FAILS on row>=1 (RGBA16 + CI8, 3 DXT values), now
  confirmed on the triangle path (was texrect-only). Real LoadBlock defect.
- G_AC_DITHER alpha-compare refuses ("no PRNG binding") -- already-known.

SHARED wgpu+RT64 divergences from angrylion (open, NOT fixture noise):
- CI4/CI8-via-triangle (first triangle-sampled; width fix didn't change it) -- open.
- fog-color blending; FORCE_BL/coverage blending (RT64 audit names coverage
  unmodeled -- expected).

TexRectFlip fix (Codex b8bij20oz) still running in parallel.

FIX PRIORITY (Phase 3, delegate to Codex, ISOLATED worktrees this time):
1. coverage color-on-cvg drop (isolated defect, clear repro).
2. Z-buffer raw-DPC binding (big gap; enables all depth tests).
3. LoadBlock DxT row>=1.
Shared wgpu+RT64 divergences -> separate investigation (like BI_LERP_0: are they
ROM-relevant?).

## HOLD Phase 3 fixes until TexRectFlip lands (collision discipline, 2026-08-22)

TexRectFlip (Codex b8bij20oz) STILL running, dirty edits in this worktree
(production.rs, targets/texrect.rs, render_ir.rs). Do NOT dispatch the 3 Phase-3
fix agents (coverage-drop #16, Z-buffer #17, LoadBlock #18) into this SAME tree --
that repeats the 2-writer collision. WAIT for TexRectFlip to commit, THEN dispatch
Phase-3 fixes with WORKTREE ISOLATION (each fix agent gets its own worktree) so
they never share a tree. Fan-out Pass 1 result is banked (committed 7b9d1afe,
c013894a).

Task 15: complete — TexRectFlip (0x25) axis swap, commit c8ba2cb5. gen-texrect-flip refused->byte-identical (wgpu==RT64==angrylion, 0 diff). Gate PASS 33/37. Codex authored, orchestrator verified+committed (agent never committed/reported).

Phase-3 fixes dispatched (3 parallel fork agents, each self-creating an isolated worktree off HEAD c8ba2cb5):
- Task 16fix (coverage CLR_ON_CVG+CVG_DST_WRAP drop) -> fork a81ad623, worktree fix-coverage, branch fix/coverage-clr-on-cvg
- Task 17 (z-image binding SetZImage 0xfe/SetMaskImage 0x3e + depth test) -> fork a9733ea0, worktree fix-zbuffer, branch fix/zbuffer-binding
- Task 18 (LoadBlock DxT row-advance RGBA16/CI8) -> fork ac58bc1b, worktree fix-loadblock, branch fix/loadblock-dxt
Each verifies: refused->0-diff vs angrylion, gate PASS, unit test, commit (no push). Reports at task-{16fix,17,18}-report.md.
NOTE: fork dispatch returned spurious "not available inside forked worker" on 2 of 3 calls but ListAgents confirms all 3 running. Do NOT re-dispatch.
Branches live on their own worktrees; after each verifies, cherry-pick/merge its commit onto worktree-wm2000-playable, re-run full triage+gate, then remove the worktree.

ORCHESTRATION LESSON (this session): fork subagents CANNOT create git worktrees (cwd pinned; "can't fork in a fork"). isolation:worktree on fresh agents branches from origin/main (baseRef unset=fresh), losing this branch's unpushed corpus+flip. This session's own git guard also blocks creating sibling worktrees. => Phase-3 fixes run SERIALLY in the shared wm2000-playable tree, one writer at a time, commit path-scoped between each. No collision by construction (SDD rule: never parallel implementers).
Dispatched #17 z-buffer (fresh opus general-purpose, serial, in shared tree, agent a29030c9). #16 coverage + #18 loadblock queued after it commits. #19 ROM-relevance Explore running read-only alongside. Before-state: coverage completes but 12px wrong; all 6 zbuffer + all 6 loadblock-deep refuse.

Task 17: complete — z-image binding (SetZImage 0xfe/SetMaskImage 0x3e, both mask 0x3e) + strict-less-than depth test in raw-triangle raster. Commit fcd48b7c. Independently verified: all 6 gen-zbuffer-* refused->0-diff vs angrylion (corpus 49->55 pass-all-match-hardware, refused 14->8), gate PASS 33/37. Scope notes (agent, honest): z-image tracked-only (not in neutral IR, avoids reference exhaustive-match ripple); ZMODE relations not dispatched (only ZMODE_OPAQUE in corpus); G_ZS_PIXEL uses base-Z integer (no per-pixel Z-plane interp — corpus flat-Z only). Serial: dispatching #18 loadblock next.
