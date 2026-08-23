# Task 22 — WM2000 post-fix perf + on-screen verification (read-only)

**Date:** 2026-08-23
**Lane:** all-fn64 Rust stack — `FN64_RECOMP=rs` (fn64-cpu-runtime) + `FN64_RENDER=wgpu`
**Runner:** `scripts/play-wm2000.sh` (ROM `aki-recomp/games/NWXE/wm2000.z64`, host lookup `recomps/wm2000/.../host_lookup.rs` — both present, no missing inputs, no interactive login needed)
**Method:** `/fn64-perf-method` skill. Pump census `FN64_PUMP_CENSUS=1 WARMUP=300 PUMPS=1200`. Shipped-frame numbers taken from an **unarmed** run (no phase timing) per the rule that profiling inflates absolutes; phase attribution taken from a **separate armed** run and read as SHARES only.
**Stack banner confirmed:** `recompiler: rs (fn64-cpu-runtime)`, `renderer: wgpu` (registered, not reference-fallback), `game: linked`.

---

## 1. Drawn-frame rate (attract/demo, clean unarmed run)

Full census, RENDERER=wgpu, 1200 measured pumps (fields) after 300 warmup:

| population | n | wall mean | p50 | p95 | max |
|---|---:|---:|---:|---:|---:|
| all fields | 1200 | 21.511 ms | 4.932 | 54.233 | 66.173 |
| fast (off-render) | 697 | 5.828 ms | 4.491 | 15.416 | 16.325 |
| slow (render) | 503 | **43.243 ms** | 44.364 | 57.648 | 66.173 |

- **Over the per-field 16.667 ms budget: 503 / 1200 = 41.9 % of fields.**
- Slow-pump gap histogram = **2:498 (99 %)** — slowness is strictly every-other-field. WM2000 renders one drawn frame per two VI fields (30 Hz), exactly as expected.
- `P(slow | gfx_task>0) = 0.838` (lift 2.00×); `P(slow | vi_swap) = 0.838` (lift 2.00×); `P(slow | audio_task>0) = 0.417` (lift 1.00× — audio is not the trigger).
- Sequence dump confirms the pattern: gfx/swapped field ≈ 15–26 ms alternating with a ~3.9 ms off-field.

**A drawn frame costs BOTH fields** (one render field + one off-field):

- **ms/drawn-frame = 43.243 + 5.828 = 49.07 ms**
- vs the 33.333 ms / 30 Hz budget → **1.47× = +47 % over budget**
- **drawn-frame rate ≈ 20.4 fps** (target 30)

Cross-check (2× all-field mean = 43.02 ms) is close but the paired slow+fast figure is the honest one given the clean gap=2 structure.

### Comparison to baselines
- vs `wm2000-gameplay-perf-baseline` (52.79 ms/field, 3.17× on the reference/RT64 gameplay route): not directly comparable — that is the software-reference/RT64 lane and a denser gameplay route.
- vs `wm2000-wgpu-perf-attribution` (2026-08-21, same rs+wgpu lane, pre-fix): all mean **27.15 → 21.51 ms**, slow mean **59.84 → 43.24 ms**, over-budget share **~40 % → 41.9 %**. The post-fix wgpu lane is measurably faster (slow field −28 %), attributable to this session's landed work plus the mutation-guard fix noted in that memory. Still ~1.47× the 30 Hz drawn-frame budget.

---

## 2. Phase attribution of the overage

Separate **armed** run (`FN64_PHASE_TIMING=1 FN64_EXECUTOR_SPLIT=1 FN64_SESSION_PHASE_CENSUS=1 FN64_DPC_COPY_CENSUS=1`), same census caps. Reproducibility confirmed — all mean **21.466 ms** (clean 21.511), slow **43.229 ms** (clean 43.243), over-budget **41.8 %** (clean 41.9 %). Phase-timing perturbation is negligible on this lane, so shares and absolutes agree.

Per-field means (fast | slow | Δ | share of the slow-field TAIL):

| phase | fast ms | slow ms | Δ ms | share of tail |
|---|---:|---:|---:|---:|
| `gfx_ns` (all graphics) | 1.526 | 36.850 | +35.324 | **94.4 %** |
| `gfx_lle_ns` | 1.526 | 36.847 | +35.321 | 94.4 % |
| **`gfx_lle_rdp_ns`** | 1.504 | **34.757** | +33.252 | **88.9 %** |
| `vi_present_ns` | 3.333 | 3.305 | −0.028 | −0.1 % |

**Top cost center: the graphics LLE/RDP path (`gfx_lle_rdp_ns`), 88.9 % of the per-field overage; graphics as a whole 94.4 %.** VI present is flat (~3.3 ms both populations) and is not a driver. This matches memory `wm2000-wgpu-perf-attribution` (there `gfx_lle_rdp_ns` was ~100 % of the tail); still the same domain, now slightly smaller in absolute ms.

Inside that graphics bucket, `FN64_SESSION_PHASE_CENSUS` decomposes the raw-DPC dispatch over **55,486 submissions**:

| stage | ms | share | per submission |
|---|---:|---:|---:|
| **execute** | **15,715.4** | **87.9 %** | 0.283 ms |
| plan | 1,962.2 | 11.0 % | 0.035 ms |
| commit | 188.6 | 1.1 % | 0.003 ms |
| finalize | 21.4 | 0.1 % | 0.000 ms |

`execute` (the backend actually running the decoded commands — the CPU rasterizer + per-pixel work) owns ~88 % of the graphics cost. This closes with the 2026-08-21 baseline (execute 91.2 % then, 87.9 % now). Slow render fields run `rsp_steps_gfx ≈ 383,170` and `rsp_entries ≈ 9.3` per field (vs ~4,000 / 1.3 on off-fields) — the RSP gfx interpreter is heavily exercised on exactly the render fields.

**Attribution answer:** the ~+16 ms/drawn-frame overage is almost entirely the graphics render field, and within it the raw-DPC **execute** stage (rasterization). `gfx_lle_rdp` remains ~90 % of the overage, as in the prior baseline.

---

## 3. Do the 4 fixed opcodes fire in WM2000?

**Not directly confirmable from the available counters, and here is exactly why.** There is no per-opcode census gate in `fn64-render-wgpu` for TexRectFlip (0x25), SetZImage, LoadBlock DxT, or CLR_ON_CVG coverage. The four fixes were gate-verified against **synthetic parity fixtures** (`gen-texrect-flip`, mode-matrix fans), not against a live WM2000 capture (see commits `c8ba2cb5`, `fcd48b7c`, `1d8c0d11`, `435dbbab`).

What the live run DOES prove: the graphics LLE/RDP path that decodes and executes these opcodes fires heavily in WM2000 — `gfx_lle_calls > 0` on every render field (`P(slow | gfx_lle_call>0) = 0.837`, lift 2.0×), 55,486 raw-DPC submissions decoded/executed over the census window, `rsp_steps_gfx ≈ 383k`/render-field. So WM2000 unquestionably exercises the raw-DPC decode+execute seam these fixes live in. Whether its command stream contains those **specific** opcodes is unproven by counters.

`FN64_DPC_COPY_CENSUS` armed but `dpc_calls = 0` and no `[dpc-copy-census]` line printed — WM2000 does not use the `dispatch_captured_raw_rdp` copy path that census counts; it funnels through `try_dispatch_raw_dpc_via_session` (per memory `wm2000-live-render-path`), so that census is silent by design here.

**To answer definitively (needs a code change, out of read-only scope):** add the one-line capture probe at the top of `try_dispatch_raw_dpc_via_session` (recipe in memory `wm2000-live-render-path`), dump one swap's command words, and grep the decoded opcodes for `0x25` (TexRectFlip), SetZImage (0xfe/0x3e), LoadBlock (0x33) with DxT, and the OtherMode CLR_ON_CVG / CVG_DST_WRAP bits.

---

## 4. Visual findings (measurable from frame dumps)

`FN64_FRAME_DUMP` wrote **540 PNGs** across the run (every presented frame; all named `frame-0000-*` because no `FN64_FRAME_TRIP` was set, so they're disambiguated by rgba-hash — content is live and distinct). Frames span boot → logo screens → copyright text → entrance → in-match gameplay. Inspected a spread by capture time.

**Clean / correct (no known artifacts):**
- **AKI / Asmik Ace logo** and **THQ / JAKKS Interactive logo** screens: correct colors (AKI red/blue/yellow mark; THQ green + blue box + red bar), correctly positioned. **No yellow block, no black bar.** Consecutive logos cross-fade with a diagonal wipe (a translucent ghost of the outgoing logo overlaps the incoming one) — consistent with a normal dissolve transition.
- **Copyright / legal text screen:** crisp, single-instance text, correctly laid out. **No horizontal text duplication.**
- **In-match gameplay:** wrestler with detailed textured attire, crowd, ropes, canvas — correct colors, depth, and silhouette. Clearly playable and coherent.
- **Character close-ups / entrance poses:** correct skin tones and attire colors against dark backgrounds.
- **No boot static observed** (that artifact was already fixed; confirmed absent).

**Flag for a human (color cast on entrance/transition frames):**
- Several **entrance / attract** frames captured mid-transition are strongly **washed out** (very pale, low contrast) or **yellow/sepia tinted**. Because the dump captures every present including the extreme ends of a fade, these are most likely **fade-in/fade-out transition frames**, not steady-state defects — the steady frames around them are correctly colored. But a human should confirm in motion that entrance-scene lighting/fades look right and the yellow cast is intentional spotlight/lighting, not a blend/coverage regression.

Sample PNGs live in `/private/tmp/task22/frames_clean/`.

## 4b. What still needs a human on-screen glance
The genuine visual-correctness bar requires the live GUI (memory `shell-interactive-probe`); a human should eyeball, with the game running `FN64_RECOMP=rs FN64_RENDER=wgpu ./scripts/play-wm2000.sh`:
1. **AKI/THQ logo screens in motion** — confirm the cross-fade dissolve is intended and there is no transient yellow block or black bar.
2. **Entrance scenes** — confirm the pale/yellow color cast is the game's own lighting/fade, not a blend or coverage defect (this is the one measurable anomaly above).
3. **Menu text** — confirm no horizontal duplication in interactive menus (static captures showed none, but menus weren't necessarily reached in this attract-only window).
4. **Gameplay over time** — confirm no depth-fighting / z-order glitches (relevant to the SetZImage/depth fix), no texture-rectangle mirroring glitches (TexRectFlip), and no coverage/edge artifacts on transparent silhouettes (CLR_ON_CVG).

---

## Caveats / provenance
- Two separate `fn64` GUI runs (unarmed for absolutes, armed for shares), serialized, one at a time — no concurrent GPU contention.
- A 9-hour-old leftover `fn64` process (pid 49191) from a prior session was idle (CPU frozen at 8.7 s) throughout and did not perturb these runs; not killed (not this session's process).
- Log snapshots: unarmed run `b80u0f3db`, armed run `brsy432la` under the task output dir; 540 frame PNGs in `/private/tmp/task22/frames_clean/`.
- Clean and armed runs agree to <0.3 % on every headline (all-mean, slow-mean, over-budget share), which is the two-reps-minimum bar.
- **Bounded**: both real runs capped at 300 warmup + 1200 pumps and exited 0 via the census window-complete path; no unbounded GUI loop. Both my `fn64` processes exited cleanly.
- **This is attract/demo content**, reached by a passive warmup (no scripted input). In-match gameplay frames appear in the capture but a sustained gameplay route (denser than attract) was not driven; the `wm2000-gameplay-perf-baseline` gameplay figure (52.79 ms/field on a different lane) suggests dense gameplay would be somewhat heavier than this 43 ms/render-field attract figure.
