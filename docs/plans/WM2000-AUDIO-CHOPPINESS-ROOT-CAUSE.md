# WM2000 audio choppiness — rigorous root-cause & fix plan

**Status:** Phase 0 (measure the fork) not yet run. No fix code until the fork resolves.
**Branch:** `perf/wm2000-audio-lockfree-land` (worktree `/private/tmp/fn64-audio-land-verify`).
**Author of diagnosis:** verification pass, 2026-08-30.

## Problem

WM2000 audio is choppy/staticky during gameplay on the all-Rust stack (`FN64_RECOMP=rs`,
`FN64_RENDER=wgpu` — the `play-wm2000.sh` default). This must be fixed correctly, at the root,
once — not masked.

## What is already verified (code-level, ruled in / ruled out)

Verified by reading the code on this branch (not inferred from docs):

1. **The audible defect is hard zero-fill on underrun.** `RealtimeOutputConsumer::drain_into_f32`
   ends with `output[delivered..].fill(0.0)` (`crates/fn64-audio/src/lib.rs:1100`). Every callback
   that finds the ring empty/short writes silence into the gap. That silence is the static.
2. **NOT contention.** The lock-free `ArrayQueue` landing (commit 8f4305fa) removed producer mutex
   contention; measured zero contention slots. Confirmed by the race/oracle tests.
3. **Switching render backend is not the fix — but render is NOT free of the pump thread**
   (corrected after review). `FN64_RENDER=wgpu` registers `Threaded`
   (`crates/fn64-shell/src/main.rs:557-577`); `play-wm2000.sh` defaults to it
   (`scripts/play-wm2000.sh:58`), so the choppy build is already on the threaded renderer — a
   different backend won't fix it. HOWEVER, `Threaded` offloads only the DP *execute* phase to the
   `fn64-rdp` thread; "Guest code, RDRAM publication, device completion, and presentation remain on
   the emulation thread" (`lifecycle.rs:1665-1666`), and the pump thread BLOCKS on the render worker
   at every VI edge via `settle_renderer_before_vi` → blocking `recv()` inside `advance_virtual_time`
   (`host.rs:119-129`, `lifecycle.rs:986-992`). So sustained wgpu execute cost above a field's slack
   serializes back onto the pump critical path and delays audio production just like guest-CPU cost.
   **Render is a live per-field cost contributor; branch A must keep it in scope. Only "switch the
   backend" is ruled out, not "render cost matters."**
4. **NOT the resampler.** `BandlimitedResampler` exists and is correct; guest-rate/host-rate
   mismatch is explicitly guarded (`lib.rs:2351-2354`). Ruled out as a static source.
5. **Audio is produced one buffer per retrace, inside the guest pump loop, on the emulation
   thread.** The pump loop (`main.rs:806-870`) calls `run_one_step()` on the winit/emulation thread,
   which steps ALL guest tasks — CPU, the AI/audio task (AudioMgr), and DP. The AI task's
   `queue_samples`/`push_dma` runs via a `thread_local` backend
   (`crates/fn64-abi/src/task_dispatch/lifecycle.rs:572`, push at `setup.rs:281-283`), i.e. on the
   pump thread. Comment `main.rs:878`: "The audio thread produces one buffer per retrace." So audio
   production is gated by how long each guest field's steps take — NOT by render (already offloaded).

**Net:** the coupling that matters is audio-production-inside-guest-execution, gated by per-field
wall cost on the pump thread. That per-field cost includes guest CPU steps AND the wgpu execute
join at each VI edge (point 3) — both serialize audio production. Render being "threaded" reduces
but does not remove the render contribution to per-field cost.

## The fork (the unresolved question — Phase 0 resolves it) — A / B / C

**Corrected after adversarial review (Fable, 2026-08-30): the fork is THREE-way, not two.** The
review found a live third mechanism the two-way fork missed, and it is the one a naive buffering fix
would make worse. Evidence in `[brackets]`.

- **(A) Throughput deficit.** The guest+present pipeline runs *slower than realtime* per field
  (wall ms/field sustained > 16.667 ms). Audio is under-produced at the source: guest emits < 60
  buffers/wall-second, callback consumes 60, the ring drains regardless of buffering. **Fix =
  reduce sustained per-field cost to realtime** — guest-CPU recompile throughput AND wgpu execute
  cost (render is NOT free of the pump thread; see below). Buffering is cosmetic only. Symptom:
  `underrun_sample_slots` > 0, ring runs empty.
- **(B) Bursty-but-realtime.** Guest is fast on *average* (mean ≤ 16.667) but *spiky* — heavy
  fields (overlay fault-in, GC, big scene) blow the budget while the ring holds too little runway.
  **Fix = ride the bursts** with ring runway. Buffering is a real fix. Symptom: bursts of
  `underrun_sample_slots` during spikes, ring empty at the spike.
- **(C) Catch-up over-production → drop-oldest skip.** `EmulatedWallClock` is an immutable epoch
  that NEVER re-anchors [`crates/fn64-shell/src/timing.rs:131-136`]. After any stall, every
  subsequent VI deadline is already in the past, so the shell pumps back-to-back with no sleep
  [`main.rs:2323-2328`, WaitUntil on a past instant fires immediately]. Each catch-up pump advances
  one virtual field and pushes one audio buffer — audio produced FASTER than 60/wall-second during
  catch-up. If the ring nears its 250 ms cap [`lib.rs:2217`, `rate/4`], `push_dma` evicts the
  OLDEST unplayed sample [`lib.rs:1051-1060`, `force_push` → `dropped_sample_slots`] — an audible
  skip. This is exactly the "drop-oldest skipped playback = the static" mechanism memorialized at
  [`main.rs:880-884`] (the rate-error CAUSE is fixed; the drop-oldest MECHANISM is live). Symptom:
  `dropped_sample_slots` > 0, ring near cap.

**B and C are two audible artifacts of the SAME bursty profile with OPPOSITE ring symptoms**
(B = ring empty during the spike; C = ring overfull during the catch-up after it). A steady-fill
buffering fix aimed at B, pushed near the cap, makes C WORSE — it removes the headroom catch-up
needs before drop-oldest fires. This is the single biggest design risk (see below).

Distinguished by measurement: **wall ms/field distribution (mean, p95, max), % pumps over
16.667 ms, AND both ring counters — `underrun_sample_slots` and `dropped_sample_slots`.** Mean over
budget → (A). Mean under but p95/max over, with underruns at spikes → (B). Nonzero drops during
gameplay → (C) is live today (not just a post-fix hazard).

Prior memory says "52.79 ms/field = 3.17x budget" but that may be stale / a different config;
Phase 0 re-measures on THIS branch + wgpu. We do not plan against memory.

## Workflow (the discipline)

```
Phase 0  MEASURE THE FORK          -> a number decides A vs B     (no fix code yet)
Phase 1  PLAN AGAINST THE ANSWER   -> the branch the number picks
Phase 2  IMPLEMENT + VALIDATE      -> the validation ladder below
```

Rule: **no fix code is written until Phase 0's number is in hand.** Efficiency comes from not
building the wrong fix.

### Validation ladder (established, reused in Phase 2)

Fastest → slowest, run the cheapest thing that catches the regression / proves the win:

1. **Lib unit tests (seconds):** `cargo test -p fn64-audio --lib`. Buffer/drain/underrun logic.
   Canaries: `f32_drain_converts_i16_samples_at_the_host_boundary` (lib.rs:3895, asserts the
   zero-fill tail — a hold-last-sample change must update it) and
   `realtime_callback_distinguishes_empty_short_and_complete_ring_pulls` (lib.rs:3496).
2. **New focused tests + race/oracle re-runs (seconds):** assert warmup-floor / hold-last-sample
   behavior; re-run `realtime_transport_matches_the_independent_span_oracle` and 20× fresh
   `producer_callback_race_cannot_become_contention_silence`.
3. **Headless throughput census (windowless, minutes):** `FN64_FRAME_CENSUS=1` on the block route —
   wall ms/field + wall-vs-virtual + fast/slow populations. No window, no wgpu surface needed for the
   throughput number. THIS is the Phase 0 instrument and the Phase 2 before/after for branch (A).
4. **Headless bounded presentation trace (windowed, minutes):** the shell census with
   `FN64_PRESENTATION_TRACE` → `summarize-presentation-trace.py` for `underrun_sample_slots`. This
   needs the winit window (the audio-underrun telemetry is wired into the shell pump). Use for
   branch (B)'s before/after silent-slot count. Take N runs/side; compare distributions (the cpal
   callback is a real device thread — single counts carry jitter).

## Phase 0 status — BLOCKED on a WM2000 boot-context capture

Attempted 2026-08-30. The shell BUILT clean on the wgpu lane (recompile_rom + shell link,
sha256 4e588b4e…), but the rs lane requires `FN64_BOOT_CONTEXT` — a ROM-bound post-IPL3 capture
that **cannot be synthesized** (`crates/fn64-shell/src/main.rs:502` under `#[cfg(fn64_cpu_runtime)]`;
`scripts/capture-boot-context.zsh` header: "the two BootContext builders in the tree are test
fixtures"). No committed WM2000 capture exists in this tree (only a WorldTour one under
`recomps/wm2000/packages/worldtour-block-boot/captures/`). So a live WM2000 gameplay census is
blocked until a capture is produced (via `capture-boot-context.zsh <wm2000.z64>`, which itself needs
a boot that reaches match-setup — the same boot-handoff capture dependency the AKI block-boot lanes
have). **The live A/B/C measurement (below) is deferred to when that capture exists.** Meanwhile the
boot-free lane proves the mechanisms and lands the buffer fixes that are valid regardless of A/B/C.

## Phase 0b — BOOT-FREE mechanism proof + buffer fixes (do this now)

The ring producer/consumer are pure and fully unit-testable without a boot
(`push_dma`/`force_push` drop-oldest at `lib.rs:1051-1060`; `drain_into_f32` zero-fill at
`lib.rs:1100`). This lane proves the review's mechanism-C claim and lands the buffer fixes that hold
under any fork verdict, with `cargo test -p fn64-audio --lib` (seconds).

- [x] **0b.1** Mechanism C confirmed a live code path — already unit-covered by
      `realtime_queue_preserves_drop_oldest_without_a_producer_mutex` (fills cap-4, pushes 6, asserts
      `dropped_sample_slots: 2`, drain returns samples 3–6 = oldest dropped, newest kept). No new test
      needed; C exists at the code level (whether it FIRES in practice is the boot-dependent question).
- [x] **0b.2** DONE (commit f84cb1ae). Replaced hard `fill(0.0)` with `fill_underrun_tail`: a linear
      fade from the last delivered sample to zero across the gap (click-free; empty pull stays silent).
      Both drain paths. Canary `f32_drain_converts_i16_samples_at_the_host_boundary` updated + 4 new
      tests. `fn64-audio --lib` 400/400.
- [ ] **0b.3** Implement warmup/prebuffer floor in the delivery gate; unit-prove no drain below the
      floor and normal drain at/above it. (PENDING — the more involved gate change; a floor must be
      bounded BELOW the ring cap per the C constraint.)
- [x] **0b.4** Race + oracle + conservation green with the drain change; 20/20 fresh-process race
      sweep (`producer_callback_race_cannot_become_contention_silence`). No concurrency regression.

These are valid regardless of A/B/C: hold-last-sample removes the click on ANY short pull; the floor
removes startup underrun; C-proof confirms the bounded-fill constraint is real. None masks a
throughput deficit (branch A stays a separate, boot-dependent effort).

## Phase 0 — LIVE MEASURE (deferred until a WM2000 boot-context capture exists)

**Instrument:** `crates/fn64-abi/src/frame_census.rs`. It hooks `advance_virtual_time` (`host.rs:150-165`)
— the one seam BOTH the headless and windowed lanes cross — and reports the two ratios it was built to
separate (wall ms/field vs wall-vs-virtual; it documents the trap of quoting one as the other). Env:
`FN64_FRAME_CENSUS=1`, `FN64_FRAME_CENSUS_WARMUP_GFX=<n>` (discard boot transient until n gfx submits),
`FN64_FRAME_CENSUS_POPULATIONS=1` (fast/slow split), `FN64_FRAME_CENSUS_SEQUENCE=<n>` (raw per-field dump).

**WINDOWLESS vs WGPU is a real constraint (corrected after review):** the two cannot both hold with
existing routes. The windowless gameplay harness `examples/wm2000-census` hard-codes the SOFTWARE
`ReferenceBackend` via the synchronous `set_render_backend` (`examples/wm2000-census/src/main.rs:189-203`)
— it measures guest-CPU + synchronous software-raster, a DIFFERENT critical path from the wgpu shell.
`WgpuBackend` is registered ONLY in the windowed shell (`fn64-shell/src/main.rs:557-577`). Since the
census hook fires inside the windowed shell too (and correctly counts the render-worker join from point
3, while excluding present/blit — `frame_census.rs:63-71`), **the honest classification of the wgpu
build requires running the census INSIDE the windowed wgpu shell. Phase 0 needs a window.** (The user's
"no popup" ask: the decisive measurement can't be windowless; say so. A reference-backend windowless run
is a useful *secondary* number — it isolates guest-CPU cost from render — but must NOT be quoted as the
wgpu verdict.)

**Trap to avoid:** the plain block route `FN64_BLOCK_MAX_STEPS=19523` reports `gfx_submits=0` — renders
nothing, cannot measure a real gameplay frame (`frame_census.rs:8-9`, `docs/plans/perf-method.md`). Any
census route must show `gfx_submits > 0`.

- [ ] **0.1** Build the WM2000 wgpu shell (`play-wm2000.sh` path). Confirm gameplay is reached and
      `gfx_submits > 0`. (Optional secondary: build `examples/wm2000-census` for a windowless
      guest-CPU-only reference number, clearly labeled NOT the wgpu verdict.)
- [ ] **0.2** Run the census in the windowed wgpu shell over a steady-state gameplay window
      (post-warmup, a scripted input path for repeatability). Record: mean, p95, max wall ms/field;
      % fields > 16.667 ms; wall-vs-virtual ratio; fast/slow population means.
- [ ] **0.3** Run the presentation trace in the SAME window and record BOTH ring counters:
      `underrun_sample_slots` AND `dropped_sample_slots` (the C discriminator).
- [ ] **0.4** Classify:
      - mean > 16.667 → **(A) throughput deficit** (with underruns, ring empty).
      - mean ≤ 16.667, p95/max over, underruns clustered at spikes → **(B) bursty**.
      - `dropped_sample_slots` > 0 during gameplay → **(C) catch-up drop-oldest is LIVE today**
        (independent of A/B; can co-occur). Record which of A/B/C are present.
      Record the numbers and verdict in this doc.

**Exit criterion:** recorded ms/field distribution, BOTH ring counters, and an A/B/C presence verdict,
on THIS branch's wgpu build. Everything downstream forks here.

## Phase 1 — PLAN AGAINST THE ANSWER

### If (A) throughput deficit — the honest, correct fix

The root cause is per-field cost > realtime. Audio buffering CANNOT fix an under-production deficit;
it only changes how the shortfall sounds. The fix is throughput:

- [ ] **A.1** Profile one steady-state over-budget field (Time Profiler / PMU) to attribute cost:
      guest-CPU recompiled MIPS vs wgpu draw submit vs VI present. Split guest-CPU from render.
- [ ] **A.2** Target the dominant cost. (Memory: rasterizer/guest-CPU heavy; the perf branch has
      chipped at both. This plan does not pre-judge — A.1 names the target.)
- [ ] **A.3** Re-run Phase 0 census after each change; the bar is mean ms/field ≤ 16.667 in
      steady state. Only then does the audio ring stop under-running structurally.
- [ ] **A.4** THEN, and only as polish, add the branch-(B) buffering below so the last residual
      spikes don't crackle. Scope it explicitly as cosmetic, not the fix.

### If (B) bursty-but-realtime — buffering + variance is the real fix

**HARD CONSTRAINT from review: branch B must not create branch C.** A steady fill pushed toward the
250 ms cap removes the headroom catch-up needs and converts underrun-static into drop-oldest-static.
Every B task must bound the target fill BELOW the cap with explicit catch-up headroom, and B is not
"done" until `dropped_sample_slots` stays ~0 too (not just `underrun_sample_slots`).

- [ ] **B.1** Add a **warmup/prebuffer floor**: gate playout (`HostPcmDeliveryGate` / the drain)
      until the ring holds a target runway (start ~50-100 ms; tune). Removes startup underrun.
      (No such concept exists today — `lib.rs:1118, 612` mention it in comments only.)
- [ ] **B.2** Maintain a **steady target fill with bounded headroom** — a target well below the
      250 ms cap (`lib.rs:2217`, `rate/4`), leaving explicit slack for catch-up over-production, so
      B does not trigger C. Do NOT fill toward the ceiling.
- [ ] **B.3** Replace hard `output[delivered..].fill(0.0)` (lib.rs:1100) with **hold-last-sample or
      a short fade** on short pulls, so a brief dip is inaudible rather than a click.
- [ ] **B.4** Reduce per-field variance if a specific spike source is identified (e.g. overlay
      fault-in, GC) — optional, driven by the fast/slow population split from Phase 0.

### If (C) catch-up over-production → drop-oldest (may co-occur with A or B)

The root is the never-re-anchoring wall clock (`timing.rs:131-136`) turning a repaid transient
deficit into a back-to-back catch-up burst that overruns the ring cap and drops oldest.

- [ ] **C.1** Bound catch-up: cap how many fields the pump advances back-to-back before yielding a
      wall slice (or re-anchor / clamp the deadline lag), so audio is not produced in a burst that
      exceeds ring headroom. This is a shell/timing change (`main.rs` pump loop / `timing.rs`), not
      an audio-crate change.
- [ ] **C.2** Alternatively/additionally, make `push_dma` overflow policy explicit for this case
      (e.g. block-newest vs drop-oldest is already the policy; the fix is not to reach the cap, per
      C.1, not to change drop semantics that the race/oracle tests pin).
- [ ] **C.3** Validate `dropped_sample_slots` → ~0 during gameplay after the bound.

## Phase 2 — IMPLEMENT + VALIDATE

For whichever branch Phase 1 selected, implement in small commits, each validated on the ladder:
- Every buffer-logic edit: ladder steps 1-2 (unit + race/oracle), seconds.
- The real win: ladder step 3 (throughput census) for (A), or step 4 (silent-slot before/after,
  N runs/side, distributions) for (B).
- Final sign-off only: a windowed `play-wm2000.sh` listen. Not per-iteration.

**Definition of done (once and for all) — BOTH ring counters ≈ 0, not just underruns:**
- (A): steady-state mean ms/field ≤ 16.667 on the census, AND `underrun_sample_slots` ≈ 0 AND
  `dropped_sample_slots` ≈ 0 on the presentation trace during a gameplay window.
- (B): `underrun_sample_slots` → ≈ 0 (within jitter) across N presentation-trace runs, WITH
  `dropped_sample_slots` still ≈ 0 (proof B did not create C), unit tests proving the warmup-floor +
  hold-last-sample + bounded-fill invariants, AND no frame-pacing regression on
  `benchmark-wm2000-render.zsh` (gap2% / over-budget% held).
- (C): `dropped_sample_slots` → ≈ 0 during gameplay after bounding catch-up, with no new underruns
  introduced by the bound.

## Non-goals / explicit nonclaims

- Not fixing this by switching render backend (already on the threaded one) — but NOT claiming render
  cost is irrelevant: the wgpu execute join is on the pump critical path each VI edge (see verified
  point 3), so it counts toward per-field cost in branch A.
- Not claiming buffering fixes a throughput deficit — if Phase 0 says (A), buffering is cosmetic.
- Not letting a branch-B buffering fix silently create branch C (drop-oldest) — bounded fill is a
  hard constraint, and `dropped_sample_slots` is in the done-criteria.
- Not quoting wall-ms-per-field and wall-vs-virtual as the same number (frame_census's documented
  trap), and not quoting a reference-backend windowless number as the wgpu-build verdict.
- Single silent-slot counts are jittery (real device thread); always compare distributions.

## Review trail

Adversarial review by a Fable subagent (2026-08-30) refuted the original two-way fork and two
ruled-out claims. Corrections folded in above: added mechanism (C) catch-up drop-oldest; corrected
the renderer framing (Threaded offloads execute only, pump joins it every VI edge); corrected Phase 0
to require the windowed wgpu shell for the wgpu verdict; added `dropped_sample_slots` throughout.
Confirmed unchanged: audio is produced on the pump thread (thread_local backend); branch-A logic that
buffering cannot fix a sustained deficit (guest audio is virtual-time-locked; sustained
production < consumption drains any finite ring).
