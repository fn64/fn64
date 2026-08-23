# Task 34 (measure) — Is the RSP gfx interpreter separate perf headroom? READ-ONLY

**Date:** 2026-08-23
**Lane:** all-fn64 Rust stack — `FN64_RECOMP=rs` (fn64-cpu-runtime) + `FN64_RENDER=wgpu`
**Method:** `/fn64-perf-method`. This is a **code-structure + existing-evidence**
resolution: the bucketing question is answered by *where the timers bracket*, and
that is a source fact, not a measurement. The share numbers are re-used from
already-recorded, renderer-tagged (wgpu+rs), phase-armed runs on the exact WM2000
attract lane — no new machine time spent (perf-method: "search for existing
evidence before spending machine time"). No code changes, no worktree.

---

## VERDICT

**SEPARATE from the execute-88% bucket — but it is a SMALL, ALREADY-MEASURED,
ALREADY-CLOSED cost, not untapped headroom.**

The RSP gfx interpreter (`gfx_lle_rsp_ns`) is bracketed by its **own** timer,
a **sibling** of `gfx_lle_rdp_ns`, not nested inside it. So the 383k steps/field
are NOT counted inside the ~88% execute bucket Task 32 owns. Answer (b) — separate
— is structurally correct.

BUT the separate cost is only **~1.7 ms/slow-render-field = 4.8 % of the slow-field
tail** (wgpu+rs attract, phase-armed 2026-08-22, 0.0 % closure residual), against
`gfx_lle_rdp_ns` at **33.9 ms = 89.3 %**. The interpreter runs at a uniform
**~11.25 ns/instruction with no defect** — it is *large, not slow*. The one decode
hot spot the brief hypothesizes (a per-step double-decode) **was already found and
fixed** (`predecode_imem`). There is no reducible per-step overhead left to chase
without changing what the interpreter *is* (HLE-ing the microcode), which is a
correctness/architecture question, not a perf micro-optimization.

**Net: real but tiny. Not the lever. Task 32's RDP/execute bucket is where the
overage lives.**

---

## 1. Where `rsp_steps_gfx` comes from and what a "step" is

- Counter increment: `crates/fn64-abi/src/task_dispatch/rsp_commit.rs:304`
  → `dpc_copy_census::note_rsp_chunk(recognize_graphics_microcode, result.steps, words.len())`,
  which does `RSP_STEPS_GFX.fetch_add(steps)` (`crates/fn64-abi/src/dpc_copy_census.rs:203-213`).
- The step loop is `fn64_audio::rsp::run_imem`
  (`crates/fn64-audio/src/rsp/interpreter.rs:93-158`). **One "step" = one retired
  RSP instruction** (`steps += 1` per fetched word at :158; `InterpreterResult.steps`
  at interpreter.rs:88 / context.rs:88).
- It is driven from the LLE task loop at `rsp_commit.rs:262-362`
  (`dispatch_lle_task`), chunked at `CHUNK_STEPS = 1<<20`, hard-capped at
  `MAX_TASK_STEPS = 1<<26`. `rsp_entries` counts `run_imem` calls; ~9.3/render-field
  means ~41k steps per entry — consistent with the ~68k steps/entry measured
  elsewhere (per-chunk setup amortized ~68,000-fold; not a dispatch hot spot).

So **383k gfx steps/render-field = 383k emulated RSP instructions executed by the
graphics microcode** to *produce* the DPC command stream. That is inherent to the
display-list size (WM2000's gfx ucode is genuinely running that many instructions),
not redundant re-execution — cross-checked by the uniform ns/instruction rate below.

## 2. The bucketing — RESOLVED at the timer level (the crux)

`rsp_commit.rs:200` opens `gfx_started` (the `gfx_lle_ns` bracket) BEFORE the RSP
step loop, and closes it at :563-573 AFTER the DPC dispatch. Inside that one
outer bracket there are **two independent sub-timers**:

- `rsp_execution_ns` — accumulated ONLY around `run_imem` (rsp_commit.rs:291-296),
  committed to **`GFX_LLE_RSP_NS`** at :571. **This is the RSP interpreter.**
- `raw_rdp_ns` — accumulated ONLY around `try_dispatch_raw_dpc_via_session` /
  `dispatch_captured_raw_rdp` (rsp_commit.rs:420-492), committed to
  **`GFX_LLE_RDP_NS`** at :572. **This is the raw-DPC execute path — Task 32's ~88 %.**

The counter tree confirms they are declared as **siblings**, both children of
`gfx_lle_ns` (`crates/fn64-abi/src/counter_tree.rs:143-145`):
```
gfx_lle_ns
├─ gfx_lle_rsp_ns   (RSP interpreter — this task's domain)
└─ gfx_lle_rdp_ns   (raw-DPC execute — Task 32's domain; contains plan/execute/commit)
```
The `session_phase_census` execute/plan/commit/finalize split (Task 22 §2, Task 32,
Task 33) decomposes **`gfx_lle_rdp_ns` only** — the RSP interpreter is not inside it.

**Therefore: the 383k-steps interpreter cost is NOT double-counted inside the
execute-88 % bucket. It is separately measured. (b), not (a).**

## 3. Its measured share on the wgpu+rs render field

From the phase-armed WM2000 attract run recorded in this worktree's
`progress.md:355-371` (2026-08-22, 600 pumps / 400 warmup, `FN64_PHASE_TIMING`
+ `EXECUTOR_SPLIT`, **0.0 % closure residual** — trustworthy), slow-field tail
(excess over fast-mean):

| phase | ms/slow-field | share of tail |
|---|---:|---:|
| `gfx_lle_ns` | 35.8 | 94.3 % |
| — `gfx_lle_rdp_ns` | **33.9** | **89.3 %**  ← Task 32 |
| — **`gfx_lle_rsp_ns`** | **1.7** | **4.8 %**  ← this task |
| `vi_present_ns` | 3.4 | −0.1 % |

Cross-checks (all renderer-tagged, consistent):
- Task 22 §2 same lane: `gfx_lle_ns` 36.847 − `gfx_lle_rdp_ns` 34.757 ≈ **2.1 ms** RSP residual.
- Historic frozen split (perf-method.md:2530): `gfx_lle_rsp_ns` 5.98 ms — and it is
  **flat fast-vs-slow (−1.7 %)**: render fields do not add RSP time the way they add
  RDP time. Its render-field *tail* contribution is small precisely because it barely
  moves between populations.

**1.7 ms is above the instrument resolution floor (the phase-timing perturbation on
this lane is <0.3 %, Task 22 §2), so it is a real cost — just a small one.**

## 4. Is it reducible? — the hot spot is already fixed; the rate has no defect

- **Per-step decode/dispatch hot spot: ALREADY ELIMINATED.** The double-decode the
  brief asks about (decoding both the current word and its delay slot every retired
  step — once ~55 % of interpreter self-time) was fixed by `predecode_imem`
  (interpreter.rs:106-116): the IMEM image is decoded once per `run_imem` entry
  (≤1024 decode calls) instead of ~128k–214k per entry. The measured recovery was
  ~2.5–2.8 ms/field. There is no remaining per-step decode overhead of that size.
- **Uniform rate, no defect (perf-method REFERENCE.md:48-57, perf-method.md:1260-1319):**
  both gfx and audio interpreters run at **11.25 / 11.27 ns/instruction (−0.1 %)** —
  a same-run, same-timer comparison, robust to drift. The RSP line is **"large, not
  slow": 526,161 instructions/render-field at ~39 cycles each is normal.** All four
  candidate defects (instruction mix, chunking/re-entry, memory access, guard-in-timer)
  were eliminated because there is nothing to explain.
- This line is **on the perf-method closed-lines ledger** ("RSP micro-optimization —
  17.6 % of graphics — NOT the renderer", scoped 2026-08-08). Do not re-propose
  without reading that entry.

## 5. Scoping sketch — IF a fix were pursued (it should not be, at 4.8 %)

Two levers, and only one touches the render field:

1. **Faster interpreter (per-instruction win).** Would touch `interpreter.rs` /
   `ops.rs` / `vu.rs` (the scalar + 8-lane VU semantics). Pays across *all* ~4.1B
   instructions (gfx AND audio), so it is a uniform mean-shaver, not a render-field
   fix. After `predecode_imem` the only sized remaining candidate cited is further
   decode-table work, which is a mean-shaver at best. Size: sub-1 ms/field ceiling,
   spread across both populations. **Not worth it at 4.8 % of the tail.**

2. **Fewer instructions (the only render-field-specific lever).** This means
   **HLE-ing the graphics microcode** instead of interpreting all 383k steps — an
   architecture/correctness change, not a perf micro-optimization. It is out of this
   task's domain and much larger in scope/risk than the 1.7 ms it would recover.

**Load-bearing correctness caveat (as required by the brief):** the RSP interpreter
*produces the DPC command stream*. Any change to `run_imem` or the LLE loop must be
**byte-identical** and validated against the parity/replay path — `dispatch_lle_task`
asserts committed IMEM generation == expected (rsp_commit.rs:383-389), commits RDRAM
writes deterministically, and feeds the `fn64.rsp-rdp-observations` stream. This is
exactly why 1.7 ms of high-risk surface is not an attractive target next to Task 32's
33.9 ms of RDP work.

---

## Deliverable — one line

**SEPARATE, not inside the execute-88 % (the RSP interpreter has its own
`gfx_lle_rsp_ns` timer, a sibling of `gfx_lle_rdp_ns`), but it is only ~1.7 ms /
4.8 % of the slow render field, runs at a defect-free uniform 11.25 ns/instruction,
its one decode hot spot is already fixed (`predecode_imem`), and the line is on the
closed-lines ledger. Real, tiny, not the lever — Task 32's RDP/execute bucket
(33.9 ms / 89.3 %) is where the overage is.**

## Provenance / caveats
- Bucketing is a **source fact** (timer brackets in `rsp_commit.rs`, tree in
  `counter_tree.rs`), independent of any single run.
- Share numbers reused from renderer-tagged (wgpu+rs) phase-armed runs with 0.0 %
  closure residual (progress.md:355-371, cross-checked Task 22 §2). Thermal/drift
  handled by using recorded within-run ratios, not fresh absolutes.
- No overlap with Task 32 (RDP/execute) or Task 33 (plan phase): both decompose
  `gfx_lle_rdp_ns`; this task is strictly `gfx_lle_rsp_ns` and the step loop.
- Ns/instruction and "large-not-slow" framing from perf-method.md:1260-1319 and
  REFERENCE.md:48-57 (already-recorded, not re-measured).
