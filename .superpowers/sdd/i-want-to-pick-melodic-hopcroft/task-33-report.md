# Task 33 — Plan-phase attribution (rs + wgpu), READ-ONLY

**Date:** 2026-08-23
**Lane (renderer-tagged):** all-fn64 Rust stack — `FN64_RECOMP=rs` (fn64-cpu-runtime) + **`FN64_RENDER=wgpu`** (registered, not reference-fallback; play-wm2000.sh banner `renderer=wgpu`).
**Route:** `scripts/play-wm2000.sh`, `FN64_PUMP_CENSUS=1 WARMUP=300 PUMPS=1200` — identical to Task 22. Bounded; exits via `pump-census-window-complete`.
**Method:** `/fn64-perf-method`. Temporary sub-phase instrument inside `plan_raw_dpc_inner` (nine `Instant` laps into atomics, gated by a temporary plan-subphase env flag; disarmed path takes NO clock reads). **Reverted after measurement** (see "Instrument" below). Shares read as the result; absolutes cross-checked, not quoted as ship figures (rule 17). Two interleaved reps for thermal drift (rule 5).

---

## 0. Instrument validation + a structural correction to the brief

**Cross-check (rule 6 — prove the instrument is real):** the sub-buckets SUM to the phase they decompose. Same run, same thread:
- `FN64_SESSION_PHASE_CENSUS` plan total = **2147.3 ms** / 55,486 submissions.
- My sub-phase instrument cumulative at 55,000 submissions = **2104.2 ms**.
Agreement ~2% (the gap is the 486 trailing submissions + the un-bucketed function prologue). This also closes against Task 22's plan total (1962 ms; this run reads slightly hotter overall — thermal, shares are stable). The split closes; it is not a decomposition that reads outside its parent.

**Correction to the brief's premise (important).** The brief located the plan cost in `PlanCollector` / `ExactRawDpcPlanVisitor::command` (`production.rs:1071`) — the per-command triangle/texrect/fill traversal. **That visitor does NOT run in the plan phase.** It is driven by `coordinator.execution_view(...)` inside `execute_raw_dpc_inner` (`production.rs:2967`), which the census bills to **`execute`** — Task 32's area. The Vec pushes, ContentDigest concerns, `to_vec`/clones, and resource-access bookkeeping the brief asks about live on the *execute* side of the seam, not the plan side. So none of the brief's named candidates are plan-phase costs.

**What the plan phase (`plan_raw_dpc_inner`, `production.rs:2563`) actually is:** it decodes the captured command stream **twice** — a discarded probe decode to learn the journal shape, then the real decode — plus two preflight/ticket builds and the seal. That double decode is where the 9-11% goes, and it is genuinely reducible.

---

## 1. Per-submission plan attribution (ranked, steady-state)

Warmup is severe in this phase (the first 5k submissions bill `real_finalize_submit` at 63%, decaying to 38% cumulative), so the honest figure is the **steady-state window 25,000→55,000 (30,000 submissions)**, not the cumulative:

**Window total = 958.0 ms → 0.032 ms/submission** (Task 22: 0.035 ms/sub — agrees).

| sub-region | ms/window | share | us/sub | poolable/hoistable? |
|---|---:|---:|---:|---|
| `real_finalize_submit` | 279.1 | **29.1%** | 9.30 | partly (alloc reuse) |
| `real_decode` | 203.6 | **21.3%** | 6.79 | no (the one decode you must keep) |
| `probe_decode` | 184.3 | **19.2%** | 6.14 | **YES — wholly redundant** |
| `begin_plan+push` | 103.7 | 10.8% | 3.46 | no (builds the plan) |
| `journal_build` | 68.6 | 7.2% | 2.29 | **YES — probe-only** |
| `finish` | 59.3 | 6.2% | 1.98 | no (seals the plan) |
| `probe_finalize_submit` | 50.8 | 5.3% | 1.69 | **YES — probe-only** |
| `probe_journal` | 6.1 | 0.6% | 0.20 | **YES — probe-only** |
| `setup` | 2.4 | 0.3% | 0.08 | — |

**Two-rep confirmation (interleaved, thermal):** rep 1 / rep 2 agree tightly — probe-pass share **32.3% / 31.4%**, doubled-decode **40.5% / 41.1%**, per-sub **31.9 / 30.9 us**, census plan total **2147 / 2135 ms** (9.2% / 9.7% of graphics). Direction and magnitude both hold.

**Grouped (rep 1; rep 2 in parentheses):**
- **Probe pass (journal discovery), wholly redundant: 309.8 ms = 32.3% (31.4%) of plan** (`probe_journal + probe_finalize_submit + probe_decode + journal_build`).
- Real pass (decode + preflight, kept): 482.7 ms = 50.4%.
- Seal (`begin_plan+push` + `finish`): 163.0 ms = 17.0%.
- Doubled decode alone (`probe_decode + real_decode`): 387.9 ms = **40.5%**.
- Doubled finalize/submit (`probe_finalize_submit + real_finalize_submit`): 329.9 ms = 34.4%.

---

## 2. Why the probe pass exists, and why it is redundant

`plan_raw_dpc_inner` needs a `ResourceJournal` (the exact access list) to build a valid ticket **before** it can run the production decode. But the access list is only knowable by decoding the stream. So the code:

1. Builds a deliberately-wrong 2-access "single-source" journal (`single_source_probe_journal`), finalizes+submits a ticket with it, and decodes — which fails with `RawDpcDecodeError::JournalMismatch { expected, .. }`. The `expected` field carries the **real** access list the decode computed.
2. Rebuilds the correct journal from `expected` (`journal_build`), finalizes+submits again, and decodes **a second time** — this time succeeding, yielding the `DecodedRawDpc` (commands + state delta) the plan actually uses.

Read `decode_from_state` (`raw_dpc/mod.rs:981`): the journal is used only for `tmem_source_identity` and the final `actual != planned` comparison. **The decode COMPUTES `planned` itself** from the stream (`push_access` + `decode_stream`). The journal is a *product* of decoding, not an input to it. The probe therefore does the entire `decode_stream` walk purely to read back a value the real decode would compute anyway, then throws away its `commands`, `delta`, and `planned`.

---

## 3. Highest-value candidate (kill-evidence sketch)

**Change:** eliminate the probe decode. Add a decode entry point that returns the computed `planned` accesses to the caller regardless of the supplied journal — either (a) a "journal-less" decode that builds the ticket from a self-derived journal, or (b) surface the `DecodedRawDpc` **together with** its computed access list so `plan_raw_dpc_inner` runs `decode_stream` exactly once and constructs the sealed journal from that single pass. This is **dedup, not removal** — the exact same journal and the exact same accesses are still produced and still sealed into the plan.

**Expected mechanism / magnitude:** removes `probe_decode` (19.2%) + `probe_finalize_submit` (5.3%) + `probe_journal` (0.6%) + `journal_build` (7.2%) = **~32% of the plan phase**. Plan is ~9-11% of graphics, so this is **~3% of graphics / ~0.6 ms per render field** at ~0.7 s over the census window. Modest in absolute terms (plan is not the bottleneck — execute/rasterizer is, at ~88-90%), but it is the single largest freely-reducible slice **outside the rasterizer**, and it is a pure duplicate-work removal with no correctness surface change.

Secondary, smaller: `real_finalize_submit` (29.1%) is a preflight + `vec![0; read.len()]` zero-buffer allocation per submission (WM2000 is 100% XBUS, so it also `.to_vec()`s the DMEM payload). Its steep warmup decay (63%→~28%) says early submissions allocate large read buffers that shrink as content settles. A pooled/reused scratch buffer for the zero-fill preflight could shave part of it, but it is decode-shaped work that must run once regardless; lower confidence and lower ceiling than killing the probe.

**Measurement that proves it (before/after):**
- Primary: re-run this exact armed route (`FN64_SESSION_PHASE_CENSUS=1` + the sub-phase instrument), same caps, interleaved with a control. Falsifier: plan-share of graphics must drop from ~9.2% toward ~6.3% (× 0.68), and the sub-phase instrument's `probe_*` + `journal_build` buckets must go to ~0. If plan-share does not fall by the predicted ratio, the probe was not the cost.
- Correct absolutes by the armed/control ratio (rule 17); read shares.

**Byte-identity / correctness plan (digests are load-bearing — dedup/defer only, never remove):**
- The produced `PlannedRawDpcSubmission` and its sealed `journal` must be **identical** to today's. The probe already asserts the real decode's `planned` equals the rebuilt journal (that IS the current mechanism), so a single-decode path that seals the same `planned` is byte-identical by construction. No digest is removed — the same journal identity flows into `finish`.
- Guest byte-identity gate on the 1.5M-step render-benchmark route (`scripts/check-byte-identity.py` vs `byte-identity-1p5M.txt`): must reproduce `gfx_submits=11153 … render_error=None` exactly.
- Parity gate + `validate_effects` must stay green (plan output feeds both; unchanged output ⇒ unchanged effects).
- Retain the current probe's one *safety* property (it proves the single-source journal is genuinely insufficient) as a `#[cfg(test)]` assertion or debug-only check, so the loud "probe unexpectedly succeeded" trap is not silently lost.

---

## 4. Verdict

The plan phase is **not near-floor and not diffuse** — it is dominated by a single, structurally-redundant probe decode. ~32% of the plan phase (≈0.7 s / census, ≈3% of graphics) is duplicated work that can be removed as a pure dedup with byte-identical plan output. It is the top non-rasterizer perf opportunity, exactly as scoped — but small in absolute terms: the drawn-frame gap remains overwhelmingly the execute/rasterizer phase (Task 32).

---

## Provenance / caveats
- Renderer tagged: **rs + wgpu**, banner-confirmed. Shares are the result; absolutes cross-checked against the session-phase census (2147 ms plan) and Task 22 (1962 ms).
- Instrument reads clocks + atomics only; never touches decoded content, so it cannot change the emulated program (disarmed = zero clock reads).
- Frozen log: `/private/tmp/task33-run1.log` (+ run2). Steady window 25k→55k; warmup (first 25k) discarded because `real_finalize_submit` warmup skews the cumulative.
- **Temp instrument REVERTED** — `git diff` on `crates/fn64-render-wgpu/src/production.rs` is empty after this task.
- A 9-hour-old leftover `fn64` (pid 49191) from a prior session was idle (0.30 s CPU) throughout; not this session's, not killed, not perturbing.
