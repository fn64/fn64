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

Task 18: complete — LoadBlock (0x33) DxT row-advance row>=1. Commit 1d8c0d11. Independently verified (FN64_ONLY=loadblock-deep): 3 RGBA16 cases refused->0-diff vs angrylion (pass-all-match-hardware). 3 CI8 cases refused->completed, wgpu==RT64 exactly (both diverge from angrylion by identical 8-9px at tile row 0, pixel 0,0: 0x07c1 vs 0x0843) = the Rank-2 CI8+TLUT color-decode divergence [[shared-divergence-rom-relevance-verdict]], NOT a DxT issue -> honest documented residual, cross-confirms #19/#20. Root cause was READ-side: DXT>=0x800 skips words; render tile's line re-reads a skipped word; old validity check refused as InvalidTexelByte at row 1. Fix: mark_block_footprint_valid back-fills skipped words with zero storage byte. LoadTile strict refusal (WM2000 origin guard) untouched. Gate PASS 33/37. Suite 4884 green. New env FN64_ONLY=<substr> filter added. Note: full runner intermittently stalls in Metal device init (re-run clears it). Serial: dispatching #16 coverage next.

Task 16fix: complete — CLR_ON_CVG + CVG_DST_WRAP color-write drop. Commit 435dbbab. Independently verified (FN64_ONLY=coverage): gen-coverage-color-on-cvg-one-cycle 12-diff(fn64-defect)->0-diff(pass-all-match-hardware); the 2 FORCE_BL rows (all-modes-combined, force-blend) stay shared-ported-bug@12 unchanged (log-only per #19). Gate PASS 33/37, suite 4885 green. Root cause: gate used coverage.wraps (image_read_enabled && sum>8) which short-circuits false when IM_RD clear, dropping every CLR_ON_CVG pixel. angrylion: CLR_ON_CVG only SELECTS color source, never gates the write; write gated by carry-out (memcvg+cvg)&8, IM_RD clear => memcvg=0 => cvg=8 always carries. Fix: keep coverage.wraps on IM_RD-set path (preserves shared rows), change only IM_RD-clear case. Mutation test clr_on_cvg_with_wrap_writes_a_full_coverage_fragment_without_image_read.

=== ALL 3 ORIGINAL PHASE-3 FIXES LANDED + VERIFIED ===
#17 z-buffer fcd48b7c, #18 loadblock 1d8c0d11, #16 coverage 435dbbab. Corpus fn64-defect now 0. Remaining wgpu-refused: alpha-dither (2) + yuv (accounted). Next: #20 CI-triangle S-plane (gated on licensing-admissible source), then fan-out Pass 2.

Launched: fan-out Pass 2 (workflow w3p6kz2es — 6 design slices: two-cycle-combine, blend-deep, tlut-palette-deep, lod-mip, zmode-deep, formats-wider -> 1 opus integrator, serial write). Concurrently: #20 admissible-source investigation (read-only, agent aeb0bf01) to find a licensing-clean authority for the correct CI triangle S-origin before any fix — angrylion alone is clean-room-excluded, RT64 agrees with the bug. #20 fix stays GATED until this returns Y.

Task 20: BLOCKED (investigation aeb0bf01, admissible-source = NO). Do NOT fix on angrylion alone. Critical correction to #19 framing: angrylion's (0,0) value 0x0843 is ITSELF suspect — it's PALETTE[8..15], the runner's documented "marker that must never appear" = an unwritten-TLUT-slot decode (parity-runner.rs:973-983). So all three (wgpu, RT64, angrylion) land on wrong-but-DIFFERENT addresses; the CORRECT value is unestablished on every side, not "angrylion-right vs ported-wrong". "reads x=0 index at x=3" is INFERRED, never reduced to a code-confirmed S delta. ADMISSIBLY PROVEN (fn64 hand key + BI_LERP-corrected angrylion, RGBA16): non-perspective S = plane>>16 to S10.5, then S-SL floored >>5; fn64 implements it correctly for RGBA16 (triangle_span.rs:618, sample.rs:535-539). Whether CI needs a different rule is UNESTABLISHED. Allowed sources (AGENTS.md:26-45) silent on CI S-origin: no gbi.h/RDP-spec vendored; ares + n64-systemtest are intake targets but NEVER RAN (Gatekeeper wall); parallel-RDP admitted for TLUT palette-fold ONLY (already proven correct). Unblock needs: real HW capture, OR booted ares/n64-systemtest RDP differential on the CI triangle stream, OR hardware-grade public spec.

Fan-out Pass 2 (w3p6kz2es): integrator HUNG uncommitted (waiting on its own gate/Monitor sub-tasks). Salvaged like TexRectFlip: built + triaged the dirty +1265-line runner edits. Corpus 71->103 cases, 70 pass-all-match-hardware, 0 fn64-defect, 8 wgpu-refused (real gaps: two-cycle TEXEL1 chain x3, LOD_FRACTION x3, alpha-dither x2, AA-coverage-edge x1), 22 shared-ported-bug, 1 rt64-hle-defect (gen-blend-aa-sloped-edge), 2 all-three-differ (construction suspects). BUT gate FAILED: 3 pre-existing cases (textured-rect-line{16,17}-low-t9{4,5}) regressed wgpu_matches_key True->False. PROVEN: deterministic, reproduces with FN64_ONLY=textured-rect-line (not a runtime state-leak), render-wgpu source untouched => a Pass 2 addition changed a shared compile-time key symbol these old cases read. Also 2 huge-diff shared cases (d=320, d=38400=whole-fb) smell mis-constructed. NOT committed. Dispatched #21 (agent ab44365d, opus, serial) to root-cause+fix the key collision, resolve the construction suspects, restore gate PASS, then commit Pass 2. check_rt64_parity.py also dirty (obsolete flip-entry removal, correct — commit with Pass 2).

Task 21: complete — Pass 2 committed f84f87f1. Independently verified: gate PASS 33/37, 3 low-t cases restored (matches_key=True, diff=0); triage 101 cases, 0 fn64-defect, 0 all-three-differ, 70 pass-all-match-hardware, 22 shared-ported-bug (all mapped to known #20/task-19/bilerp0/LOD domains), 8 wgpu-refused (real gaps: 2-cycle TEXEL1 x3, LOD_FRACTION x3, alpha-dither x2, AA-coverage-edge x1), 1 rt64-hle-defect (gen-blend-aa-sloped-edge). Root cause of low-t regression: RDRAM ADDRESS COLLISION — Pass 2 source arrays at 0x8000-0x8600 clobbered skew cases' texel rows (extend to 0x8680), corrupting guest memory both backends read identically (verdict=identical but matches_key=False); fixed by relocating new arrays to 0xc000-0xdc00. 2 malformed all-three-differ cases dropped; 38400px case shrunk to clean 128px divergence.

=== SESSION RDP-PARITY BATCH COMPLETE (clean stopping point) ===
Fixes landed+verified: TexRectFlip c8ba2cb5, z-buffer fcd48b7c, loadblock 1d8c0d11, coverage 435dbbab. Corpus 71->101 cases (Pass 2 f84f87f1). fn64-defect count: 0. Verdicts: #19 shared-divergence ROM-relevance (CI-triangle=fix-worthy, FORCE_BL/fog=log-only), #20 CI S-plane BLOCKED (no admissible source; angrylion value itself suspect). Open follow-ups: #20 (needs HW/spec/booted ares), 8 wgpu-refused gaps (2-cycle TEXEL1 chaining, LOD_FRACTION, alpha-dither, AA-coverage-edge), 1 rt64-hle-defect to investigate. Gate PASS 33/37 throughout.

"next" = subagents for all fronts. Dispatched read-only fleet (parallel, no collision): #22 WM2000 perf+visual verify (aa0f87bb, general-purpose — original acceptance bars), #23 rt64-hle-defect characterization (a344bb3c, Explore), #24 LOD/mipmap dead-vs-live assessment (a9d04ee3, Explore), #25 two-cycle TEXEL1 scoping (a8ff2561, Explore). Writer-fixes (the actual gap implementations) are QUEUED to run SERIALLY afterward, scoped by these investigations — parallel writers recreate the shared-tree collision. Key framing to confirm: WM2000's own two-cycle usage already passes, so the 8 refused gaps are likely any-ROM breadth, not WM2000-blocking; #22 settles where the primary goal (30Hz + no artifacts) actually stands post-fix.

Task 24: complete (report saved from read-only agent's message). LOD path = DEAD CODE: compute_lod is a complete/tested RT64 port with ZERO live referrers; wgpu hardcodes lod_fraction=0.0 at production.rs:5786/:14059/:15470; combiner already CONSUMES LOD_FRACTION (combiner.rs:600/637/1938/1943), only producer missing. Tier A (small: supply 1.0 for texture_lod_en-OFF control) + Tier B (real feature: mip tile select/blend via inert rt64_texture_sampler.rs + tmem/sample.rs). NOT WM2000-blocking (WM2000 = untextured shade-fog two-cycle, no LOD). => any-ROM breadth, deprioritized vs WM2000 goal.

Task 25: complete (report saved from read-only agent). Two-cycle TEXEL1 gap: refusal at texrect.rs:1518-1519 (Texel1 not in ADMITTED_COLOR_INPUTS; single-descriptor TexrectTileBinding). Fix MEDIUM ~150-250 LOC (second-tile addressing absent: 2nd binding + 2nd sample_point + admit Texel1 gated on tile+1 + wire run_two_cycle combiner.rs:1021). NOT WM2000-blocking (0 two-cycle texrects of 2,520 in boot). OVERLAP w/#24: gen-two-cycle-lod-fraction-gap needs BOTH LodFraction admit (24) AND Texel1 fetch (25) in same file texrect.rs — coordinate/serialize if both fixed.

Task 23: complete. gen-blend-aa-sloped-edge is a REAL backportable RT64 HLE defect. wgpu==angrylion 0-diff; RT64 wrong on 2 fractional-coverage edge pixels (x=3 y=1/2, dXHdy=0.5; pixel 323 angrylion=0xffff STALE vs RT64=0xfbdf). Mechanism: 2 intentional RT64 stubs — RasterPS.hlsl binary full coverage (no sub-pixel edge fraction, 1spp pixel-center) + rt64_blender.h fromInputB returns 1.0 for B_FRAMEBUFFER_ALPHA ("coverage not emulated"). Case's blend B1=1=B_FRAMEBUFFER_ALPHA so blend can't recover it. Backportable but feature-level (real analytic edge coverage). -> RT64 issue-review card. Admissibility: wgpu correctness rests on angrylion; a hand-derived key for the 2 edge pixels would make it independently admissible.

INVESTIGATION FLEET STATUS: #23 #24 #25 done. Only #22 (WM2000 perf+visual) still running — the primary-goal result. All 3 refused-gap domains (LOD, two-cycle TEXEL1, and by extension the CI #20) confirmed ANY-ROM BREADTH, not WM2000-blocking. So writer-fixes for them are lower priority than whatever #22 shows about the actual 30Hz/no-artifact bars.

Task 22: complete — WM2000 post-fix perf+visual (all-fn64 rs+wgpu, read-only, ROM+config present). DECISIVE FINDING: gap to playability is PERF, not correctness.
- PERF: drawn frame 49.1 ms (render 43.24 + off-field 5.83) = 1.47x over 33.3ms budget = ~20.4 fps (target 30). Clean 30Hz cadence (every-other-field 99%). 28% faster than pre-fix wgpu (59.84->43.24 slow field) but not at 30Hz. Overage: gfx_lle_rdp 88.9%, within it raw-DPC execute/rasterization 87.9% over 55,486 submissions (matches [[wm2000-gameplay-perf-baseline]]/[[wm2000-wgpu-perf-attribution]]). CPU rasterizer is the wall.
- VISUAL: 540 FN64_FRAME_DUMP PNGs — logos/copyright/in-match all CLEAN (no boot static, yellow block, black bar, or duplicated text). One flag: washed-out/yellow entrance transition frames (likely fades) -> human glance.
- 4 fixes' specific opcodes: can't confirm firing from counters (none exist; fixtures not WM2000 capture). Seam heavily exercised. Definitive check = 1-line probe at try_dispatch_raw_dpc_via_session (code change).
- Needs human live-GUI glance: logo cross-fades, entrance color cast, menu text dup, gameplay z-order/texrect/coverage over time.

=== "NEXT" REFRAMED BY THE FLEET ===
All 4 investigations done (#22-25). Consensus: WM2000 playability blocker = CPU RASTERIZER PERF (49.1ms, plan Phase 2 / Task 6), NOT correctness or the RDP-parity gaps. The 3 refused-gap domains (LOD #24, two-cycle TEXEL1 #25, CI #20) are all any-ROM BREADTH, none WM2000-blocking. rt64-hle-defect #23 = RT64 issue-card, not our fix. => highest-value next = attack the rasterizer perf gap with kill-evidence (Task 6), NOT more corpus breadth.

"go" = attack rasterizer perf (plan Task 6/Phase 2). Dispatched #26 measurement-first agent (a17cdf23, opus, READ-ONLY): invoke fn64-perf-method skill + clear closed-lines ledger, profile within-rasterizer attribution of the 43.24ms render field (raw_triangle.rs + sample.rs) on a deterministic scene, return ranked cost centers + kill-evidence optimization sketches. NO code changes. Writer-optimizations follow SERIALLY, each gated on before/after ns-per-pixel + byte-identity. Measurement runs SOLO (parallel profilers contend/skew). Rule: shipped figure = unprofiled mean x2; every number renderer-tagged (wgpu+rs); phase counters not leaf profiles.

Task 26: complete — rasterizer profiled. CORRECTED framing: the 2 old big terms are ALREADY GONE — unpresented GPU draw_admitted_triangles (~65% of execute) gated off on play lane (1a10b939, production.rs:279, why field dropped 59.84->43.24); SHA-256 "~20%" is STALE (hot guest-read identity migrated to xxh3-128 FastContentDigest, SHA only cold/verify). Current: raster_triangle ~3.88s of ~5.29s execute = 70-75% of execute, ~62-66% of render field, 94-102 ns/covered-pixel. Highest-value candidate = incremental texture S/T/W plane stepping (see #27). Deterministic lever = WM2000 attract pump-census. Finer split needs a temp FN64_RASTER_SPLIT probe (deferred; candidate A's A/B yields it).

Task 27: dispatched (ad625c1a, opus, serial sole writer) — step 3 texture planes incrementally like the 4 shade planes (raw_triangle.rs ~599-605 full attribute_plane i128 mul+div/pixel -> attribute_plane_step add on continues_run; template at ~529-538). Bit-identical by the same 200k-case identity (triangle_span.rs:404-426). STRICT kill-evidence: before/after ns/textured-pixel + unprofiled-mean-x2 ms/drawn-frame (wgpu+rs, >=2 reps interleaved), byte-identity (identity tests + gate 33/37 + frame tripwire), REVERT if null.

NOTE (cross-session noise): an orphan perf agent a2fc4f6e "Profile wgpu execute hot spots" (NOT spawned by this session — another job/session) notified trying to relay collision-answers to a peer it couldn't reach; a perf-attribution [f8cbee] fork (also not mine) completed alongside. Not my agents, no action. Its content cross-confirms: no census/play-lane run contention (emit1/fresh-fn64/PID 49191 are other sessions'), no FN64_RASTER_SPLIT probe exists (bracket per-triangle, read shares not per-pixel atomics which distort +13%). My #27 agent ad625c1a is the sole WRITER and progressing (7m). Ignore f8cbee/a2fc4f6e artifacts if they appear.

Task 27 update: agent code-complete + correctness-proven (compiles, 20 tests, mutation-caught step-identity test) but BLOCKED on perf A/B — the windowed pump census needs a GUI window sandboxed agents can't drive (session-long structural blocker) + shared fn64 binary has cross-session build contention (gmake fn64_rt64_shim running in-tree from another session). RESOLUTION (avoids the blocker entirely): raster_triangle (raw_triangle.rs:422) is a PURE CPU fn over TmemByteSource — no wgpu device/window. Redirected agent to measure ns/textured-pixel via a HEADLESS in-crate raster microbench (fixed deterministic textured triangle, N iterations, Instant::now, A/B by toggling the change) — cleaner kill-evidence than the windowed census (no GPU/compositor noise). Byte-identity still via step-identity tests + parity gate. Commit only on measured ns/px drop + byte-identity, else revert. IGNORED the orphan perf-attribution session (a2fc4f6e, not mine) which kept re-notifying confused about who owns the display — not part of this task.

Task 27: NULL RESULT — reverted per kill-evidence gate (textbook measure-first). Headless raster_triangle microbench (66000px x 400 iters x 4 reps): baseline (full attribute_plane/px) ~489 ns/px vs stepped ~497 ns/px — OVERLAPPING, no measurable improvement. Isolation bench (plane arith only, 120M px): full ~0.9-1.2 ns/px, stepped ~0.67 ns/px, delta only ~0.2-0.5 ns/px = ~1% of the ~490 ns/px raster (below resolution on every lane). The i128 mul+div is the most expensive SCALAR op but is ~1% of per-pixel cost. CONCLUSION: #26's RANKING was right (raster_triangle is the bottleneck) but the plane-stepping CANDIDATE was a red herring — real per-pixel cost is sample_point/TMEM-read + combine + blend, NOT plane setup. Byte-identity PASS (step-identity test + mutation-caught + suite 4886/0 + gate 33/37). KEEPING the deterministic headless microbench (reusable kill-evidence substrate for the next candidate — first clean ns/px A/B for raster_triangle) + step-identity test; reverting only the production change. => CLOSED-LEDGER LINE: "incremental texture-plane stepping" — do not re-propose. NEXT rasterizer candidate: target sample_point/TMEM decode + combine/blend, measured via the kept microbench.

Task 27 FINALIZED: commit 9ffa4e69 (tests-only, +357: microbench + isolation bench + step-identity test). Verified production raw_triangle.rs clean at baseline (line 604 full attribute_plane, no diff vs HEAD). Suite 4886/0, 5 ignored. #27 done as NULL-with-kept-infra.
Task 28 dispatched: measure-first profile WITHIN sample_point (sample.rs:406) using the kept microbench (texture_plane_raster_microbench, raw_triangle/tests.rs:1181) — attribute hoistable setup (preflight/AddressScope/snapshot) vs irreducible TMEM-read+decode+TLUT; confirm-or-CLOSE Candidate B (agent-27's named next target: hoist per-pixel invariants out of read_texel); rank combine/blend alternatives. Same resolution floor (~0.5 ns/px) — if hoistable is <~1% like plane-arith was, CLOSE it not chase it. READ-ONLY.

Task 28: complete. Candidate B (hoist per-pixel invariants out of sample_point) CLOSED — hoistable setup <=0.14% of per-pixel cost (below ~0.5 ns/px floor, into noise). KEY: whole read_texel is only ~1.6 ns of 507 ns/px (~0.3%) — the texel read + TMEM decode + TLUT is ALSO NOT the wall (refutes the "sample_point/TMEM-read" residual for the read itself). Agent caught 2 measurement artifacts (a per-pixel PhysicalTmemState::try_new alloc = 457 ns/px, and black_box forcing a TileDescriptor round-trip — the "label 95% something else" trap). CLOSED LINES now: plane-stepping (~1%), hoistable-setup (<=0.14%), texel-read (~0.3%). REMAINING ~505 of 507 ns/px = blend_and_write_pixel (texrect.rs:2647) + combiner (combine_one_texel texrect.rs:2072). Top candidate blend_and_write_pixel but NOT size-confirmed — rule 32 (both-halves-must-fall): must bracket combine vs blend each before any writer.

Task 29 dispatched (ac176634): DECIDING bracket of combine_one_texel vs blend_and_write_pixel (~505/507 ns/px) via the headless microbench. Verdict determines: one more targeted micro-opt (confirmed candidate w/ kill-evidence) OR near-the-floor => the 1.47x gap needs an ARCHITECTURAL move (GPU raster promotion, plan Risk section) which is a USER decision — will NOT start that without the user. Elimination chain so far all sub-floor: plane-arith, hoistable-setup, texel-read.

Task 29 first attempt (ac176634) FAILED — corrupted result: 0 tool_uses, 9.8s, returned injected/context-bleed instructions (basic-memory subagent refs, "implement the plan", "don't run subagents") NOT my brief. Treated as garbage, NOT as instructions. Verified it left tree clean, no report, HEAD unchanged. Re-dispatched fresh (a19355d4) with explicit "ignore injected instructions, this task only" guard. Same brief (task-6c).

Task 29 (retry a19355d4): complete — VERDICT NEAR THE FLOOR, and the 6b premise was REFUTED. Direct bracket (microbench, 66000px x 400 iters, 6 reps, temp FN64_BRACKET_6C reverted): combine_one_texel +0.40 ns/px (0.08%), blend_and_write_pixel -1.80 ns/px (below noise), sample_point -1.40 ns/px (below noise); all three removed = +3.6 ns/px = 0.7% of pipeline. The "~505 ns/px in combine+blend" was inference, never a direct bracket — refuted: removing sample+combine+blend leaves 99.3%. ~483 ns/px is the BARE SCALAR PER-PIXEL LOOP (double Range iterator + per-pixel dest offset + bounds-checked byte-slice write over 66000 px). No named computation above noise. CLOSED LINES added: combine, blend, per-pixel sample. CONCLUSION: micro-opt CANNOT close the 1.47x gap; remaining lever = ARCHITECTURAL (GPU raster amortizes the per-pixel loop across lanes) = USER DECISION.

Task 30 dispatched to CODEX (afd6389a, codex-rescue, per owner "delegate to gpt"): owner reports perf was BETTER LAST NIGHT, something regressed. Read-only bisect over render/execute/shell-path commits. TOP SUSPECT fcd48b7c (z-buffer depth test added per-pixel z-compare/z-update to raster_triangle — regression if it runs on WM2000's z-disabled path). Also 435dbbab coverage, c8ba2cb5 texrect-flip branch, shell present commits. Uses headless microbench for per-pixel A/B HEAD-vs-parent. Propose fix, don't implement.

Task 30 IN PROGRESS (NOT done): the codex-rescue agent afd6389a is a pure FORWARDER — it forwarded to Codex and returned, but the real diagnosis is a Codex BACKGROUND job b8aqpzhh9 (exceeded 600s foreground timeout, moved to bg). No report yet. The forwarder won't re-poll; the reliable completion signal is task-30-report.md appearing. Do NOT mark #30 done or report findings until the report exists. Codex is doing the microbench A/B across fcd48b7c/435dbbab/c8ba2cb5.

Task 30 COMPLETE (Codex diagnosis). CONFIRMED REGRESSION: fcd48b7c (z-buffer depth-test wiring) = +10.06 ns/px (+2.04%) on WM2000's depth-DISABLED hot triangle loop, 8/10 interleaved pairs slower (492.6->502.7 ns/px microbench, commit-vs-parent). Mechanism: expensive Z math IS correctly gated (WM2000 census 0 Z_CMP/0 Z_UPD, depth==None), BUT per-pixel bookkeeping runs UNCONDITIONALLY — every covered pixel computes a Z pixel-index + two match(depth.as_ref/as_mut, fragment_depth) dispatches that fall through to nothing. Other suspects CLEARED: 435dbbab coverage (noise/null -0.47%), 1d8c0d11 (per-load not per-pixel), c8ba2cb5 (texrect-only, not raster_triangle), shell commits (per-present not per-pixel), 9ffa4e69 (test-only). CAVEAT: +2% is microbench (covered-pixel only), not a whole-frame number; a night-to-night change much larger than a few % would also need lane/config/compositor-variance check.
Task 31 dispatched (ab5a8996, opus serial sole writer): implement Codex's proposed fix — branch on depth.is_none() BEFORE the pixel loop, depth-FREE body (no Z index/Option matches/flag checks); NOT an inner if (that preserves the regression). Kill-evidence: microbench ns/px recovered to ~492 + byte-identity BOTH paths (depth-disabled bytes unchanged + 6 zbuffer cases still 0-diff + gate PASS 33/37 + suite green).

Task 31 status (agent ab5a8996, ~52m): impl DONE + compiling (inline MACRO not closure — closure itself regressed +7ns/px; byte-identity agreement test passes). BUT current null is UNTRUSTWORTHY — two confounds: (1) WRONG BASELINE: compared macro-fix vs HEAD, but HEAD CONTAINS the regression — can't show recovery even if fix is perfect; must compare vs pre-fcd48b7c parent e98ade37 via transplant (task-30 method, bench didn't exist at fcd48b7c^). (2) THERMAL DRIFT: machine drifted 478->530 ns/px over the long session; noise floor > the 10ns signal. Guided agent to redo as 3-point transplant A=e98ade37 (parent, true baseline ~492) / B=HEAD (has regression, +10 vs A per task-30) / C=macro-fix, ABCCBA-interleaved, min-of-N (drift-canceling). Verdict axis = C-vs-A (not C-vs-B). Mechanistically the fix MUST help (task-30 proved the per-pixel match/dispatch is the +10ns; fix deletes it) — if a correct cooled A/B/C still shows C≈B, check the macro actually bypasses the machinery, else it's a measurement-floor limit not proof-of-no-help (I'll decide keep-mechanically-sound-change vs revert). Byte-identity gate (zbuffer 0-diff + spot-check + suite) only after perf axis resolved.

Task 30/31 RESOLVED — THE REGRESSION WAS NOT REAL. Properly-controlled 3-point transplant (A=true parent e98ade37-equivalent with depth machinery physically stripped / B=HEAD / C=macro-fix; ABCCBA, min-of-N, 12 triplets, shared thermal state): B-A = -0.35 ns/px (per-triplet sign 6+/6-, pure noise). min-of-N A=483.4 B=486.6 C=493.5. Task-30's +10ns/+2% was a THERMAL/CONTENTION MEASUREMENT ARTIFACT, not a real cost — LLVM already elides the dead depth==None per-pixel work (loop-invariant None match; pixel index recomputed as offset regardless). Nothing to recover => C≈A≈B. DECISION: REVERT the fix (kill-evidence gate: no measurable win => don't ship, even a clean change; compiler already handles it, macro+split = complexity for zero benefit). Keep only the depth-path-no-op-when-disabled agreement test if it applies to reverted code. This RETROACTIVELY CORRECTS task-30. LESSON: single-machine perf A/B under thermal drift is untrustworthy; require min-of-N + controlled transplant + true-parent baseline before believing a regression. WM2000 perf is UNCHANGED from HEAD (~49ms/1.47x); the standing lever remains #29's GPU-raster architectural decision (user's call).

Task 31 COMPLETE + VERIFIED. Production raw_triangle.rs reverted byte-identical to HEAD (git diff HEAD~1 HEAD -- raw_triangle.rs empty; confirmed loop still merged-depth shape). Only guard test shipped: commit 2908e419 (+111 tests.rs, no production change) pinning the z-buffer invariant (depth:None draw == Some(depth) with Z_CMP/Z_UPD clear, byte-identical fb + untouched depth cells). Suite 4887/0. 3-point transplant final: A(parent)=500.23 B(HEAD)=499.89 C(fix)=501.36 mean; B-A=-0.35 pure noise; per-triplet spread +/-16ns swamps the alleged 10ns. Task-30 retroactively corrected (its A/B ran across a warming machine, no min-of-N control). Memory perf-measure-before-dispatching updated with the thermal-artifact guard.

=== SESSION AT CLEAN REST ===
Perf regression report = NOT a real code regression (thermal measurement artifact); WM2000 perf unchanged+correct at ~49ms/1.47x over 33.3ms budget, visuals clean. All render fixes this session verified. ONLY remaining lever for 30Hz = architectural GPU-raster promotion (#29 near-floor), = USER decision. Other open items all any-ROM breadth or blocked (#20 CI S-plane needs HW/spec; 8 refused gaps; 1 rt64-hle-defect for RT64 issue-card). No agents running.

Task 32 dispatched to CODEX (ab4913b4, codex-rescue, FULL OWNERSHIP diagnosis+impl, per owner "delegate to codex, fix it"): make WM2000 hit 33.3ms/drawn frame (from ~49ms/1.47x). Micro-opt exhausted -> architectural. Prime lever: promote diagnostic GPU triangle pipeline (triangle_pipeline.rs) to write guest RDRAM (GPU draw->readback->RGBA16 requantize@SetColorImage extent->VI). Alt: parallelize single-threaded CPU raster. Codex picks+justifies. Authority to change production code, commit incrementally. GATE: measured before/after min-of-N (thermal-aware) + parity gate 33/37 + GPU-pixels match CPU raster (or documented tolerance) + suite green. Partial-but-verified OK. Report task-32-report.md.

MERGE STATUS: branch is 714 commits ahead of origin/main, 0 behind (515 files, +271k/-3.4k, but +271k is mostly vendored RT64 port source + generated parity fixtures, not hand logic). Types: 309 docs, 83 test/corpus, 99 fix, 62 feat, 76 render-wgpu, 11 perf, 5 wip. OWNER DECISION (answered "1"): WAIT for #32 (perf fix, Codex ab4913b4/bkz33smfv) to land+verify before ANY merge — don't merge while a large architectural change is writing production render-wgpu. THEN carve into small reviewable PRs lowest-risk-first: (a) parity corpus+gate (tests only), (b) the 4 gate-verified render fixes (flip/z-buffer/loadblock/coverage), (c) RT64 port bulk + perf architecture as their own reviewed PRs, docs separately. NEVER push/merge without explicit owner go; merge strategy (squash vs history, PR granularity) is owner's call. Do NOT push to main/force-push/merge.

Owner asked for MORE perf opportunities to delegate. From Task 22's measured phase census (the only headroom outside the rasterizer/#32): execute ~88% (=#32), plan 11.0% (1962ms, 0.035ms x 55486 subs), commit 1.1%, finalize 0.1%, vi_present flat. Two non-overlapping opportunities dispatched (read-only, measure-first):
- #33 (acffa68c, opus): profile the PLAN phase (production.rs PlanCollector:789) — 11% = biggest non-execute cost. Attribute per-submission (allocs/digests/clones/traversal), find reducible sub-cost. Orthogonal to raster.
- #34 (a62032950, Explore): resolve whether the RSP gfx interpreter (rsp_steps_gfx ~383k/render-field) is INSIDE execute-88% (no new opportunity) or SEPARATE upstream headroom. Bucketing crux = where gfx_lle_rdp_ns brackets relative to the RSP step loop.
Both READ-ONLY, distinct domains (PlanCollector vs RSP step loop), neither touches #32's raster area. commit/finalize/vi_present all sub-floor — not worth an agent. Writer-fixes (if any candidate confirmed above floor) follow serially after #32 lands, to avoid shared-tree collision.

Task 34: complete — RSP interpreter is SEPARATE but NOT a lever. gfx_lle_rsp_ns is a SIBLING timer of gfx_lle_rdp_ns (both children of gfx_lle_ns; rsp_commit.rs: rsp_execution_ns around run_imem vs raw_rdp_ns around raw-DPC execute) — so 383k steps/field are NOT double-counted in Task 32's 88%. But RSP = only 1.7ms = 4.8% of slow-field tail, nearly flat fast-vs-slow; 383k steps = graphics microcode legitimately producing the DPC stream (inherent to display-list size, ~11.25 ns/instr, defect-free "large not slow"). The delay-slot double-decode hot spot is ALREADY FIXED (predecode_imem, on closed-lines ledger). Only lever = HLE the microcode (architectural, correctness-risky, out of scope for 1.7ms). CONFIRMS: Task 32's gfx_lle_rdp=33.9ms=89.3% of tail is NOT inflated by interpreter time — it's the whole game. Perf picture: rasterizer (33.9ms/89.3%, #32) dominates; plan phase (#33 pending) is the only other non-trivial slice; everything else sub-floor or closed.

Task 32 COMPLETE + VERIFIED + COMMITTED (f864242e). Codex chose CPU-raster PARALLELIZATION over GPU promotion (lower risk, preserves clean RGBA16 output). Rayon persistent pool, par_chunks_mut(row_stride) row-exclusive, depth-free + census-off + >=256px gate, FN64_PARALLEL_RASTER default-ON (0 forces scalar). Substrate speedup 5.85x mean/6.14x min-of-N (516.6->88.2 ns/px microbench). Projected ~25-27ms/drawn frame = INSIDE 33.3ms budget (conditional — needs GUI/Metal census to confirm live). Codex couldn't commit (sandbox git index.lock denied) or run Metal — I VERIFIED on this host: compiles, agreement test parallel==scalar byte-identical, parity gate PASS 33/37 with parallel default-on, suite green. Required Sync bound on TmemByteSource (tmem/read.rs, not production.rs as report said) — caught by verifying, staged with it. REMAINING: GUI/Metal pump census for the real drawn-frame number (structural GUI blocker).
Task 33 COMPLETE: plan phase — the probe/double-decode is ~32% of plan but only ~3% graphics / ~0.6ms/field. Real dedup (decode_from_state computes the access list itself; probe pass reads it back via a deliberate JournalMismatch — redundant). Low priority vs rasterizer. NOTE: the PlanCollector visitor traversal is billed to EXECUTE not plan (brief was structurally wrong).
Task 35 dispatched (a0effea5): OWNER REPORT "rightmost pixel is noise, rounding error". Read-only capture-and-diff. NOT the parallel change (byte-identical, gate passes). Lead: triangle_span.rs:218 x1=ceil_ratio(max_right-Q16/8) right-edge subpixel rule; also edge conventions/scissor S10.2/sample-past-extent. Propose fix.

Task 35 COMPLETE (rightmost-pixel diagnosis). RULED OUT: triangle_span.rs:218 right-edge rounding (proven exact over all 1/8 positions; triangle path skips zero-coverage edge pixels -> writes resident byte not garbage). Parity corpus does NOT reproduce (35/39 identical incl one-cycle-fill-band exclusive-right-edge, single-pixel, last-column-last-row). INFERRED cause: CPU TEXRECT path (targets/texrect.rs) writes full [first_column,column_limit) extent UNCONDITIONALLY (no coverage skip) — candidate (a) right-edge texel sampled past tile's loaded S extent (mask_s clamp/wrap), or (b) framebuffer col 479/480 never covered = stale RDRAM. Missing proof = live 480-wide frame dump (cols 476-479) to distinguish a-vs-b (couldn't run, read-only). texrect history records exactly this symptom (texrect.rs:1846-1850, a since-fixed TMEM parity bug).
Task 36 dispatched (ac0acedbb, opus serial sole writer): reproduce candidate (a) via a hand corpus case (1-cycle texrect right edge at WIDTH-1, tile S extent ending there, mask_s clamp+wrap, BI_LERP_0 set) -> if rightmost column diverges wgpu-vs-oracle, fix the right-edge texel address (step_axis/>>5/mask_s in texrect.rs); if NOT reproduced, stop (candidate b, I take a live frame dump). Gates: new case identical + no regression (gate 33/37) + unit test.

Task 36 COMPLETE (commit ea19ad28, 2 corpus cases + unit test + report). Candidate (a) RULED OUT (proven): gen-texrect-right-edge-overread-clamp/-wrap — texrect drawn 1px wider than loaded tile so rightmost col samples texel index 4 (past loaded [0,3]); BOTH pass-all-match-hardware (wgpu==angrylion==RT64), clamp->0x7fff wrap->0xf801=texel0. step_axis+address_axis_texel correctly clamp/wrap, never read texel 4. Gate PASS 33/37, suite 4888/0, new unit test right_edge_one_past_extent_addresses_within_the_loaded_row. => residual noise is candidate (b): live-only stale/uncovered rightmost column; fix belongs at the FILL/CLEAR seam (does the frame's initial fill cover col 479 of the 480-wide fb?), NOT the sampler. Matches owner's "intro sequence" observation (stale RDRAM signature).
Static follow-up (me): framebuffer 480-wide (docs/frames dumps 480x237); present path framebuffer.rs:80 copy_width=dst_width.min(src_stride) is a straight per-row copy — copies what's in RDRAM, doesn't invent noise. So confirming (b) needs a LIVE 480-wide intro frame dump (cols 476-479, FN64_FRAME_DUMP), the GUI-capture the sandboxed agents can't take. Existing docs/frames/wm2000-attract-ring dumps are 320x240 + stale (predate tree) — unusable. Next: delegate the bounded live capture (task-22 proved it's doable), then inspect cols 476-479: scene-uncorrelated bytes => stale RDRAM uncovered column => fix at fill/clear coverage.
