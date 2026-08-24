# Task 6 — Within-rasterizer attribution + ranked optimization plan (READ-ONLY, measure-first)

**Date:** 2026-08-23
**Lane (stated on every number):** all-fn64 stack — `FN64_RECOMP=rs` (fn64-cpu-runtime) + **`FN64_RENDER=wgpu`**. Reference/RT64-lane numbers are excluded by construction.
**Scene:** WM2000 attract/demo, bounded pump census `FN64_PUMP_CENSUS=1 FN64_PUMP_CENSUS_WARMUP=300 FN64_PUMP_CENSUS_PUMPS=1200` (the same deterministic lever Task 22 and the 2026-08-21 attribution used; gap-histogram 2:498 confirms one drawn frame per two VI fields, i.e. it is reproducible and comparable A/B).
**Method:** `fn64-perf-method` skill + `REFERENCE.md` read. No code changed. No temporary instrumentation was armed for this report (see §5 for why, and the one probe that *would* be needed to go finer).

---

## 0. TL;DR — the framing the brief inherited has already shifted

The brief asks me to break the 43.24 ms render field's **execute** stage (87.9 % of `gfx_lle_rdp`) down "inside `raw_triangle.rs` and `sample.rs`". Reading the current tree against the last direct-timing evidence, the single most important fact is:

> **The largest term that used to live inside `execute` — the unpresented GPU triangle draw `draw_admitted_triangles`, ~13 s of an 18.18 s execute bucket (~65 % of execute) — is ALREADY GATED OFF on the play lane** by commit `1a10b939` (`production.rs:279–280`: `gpu_triangle_draw_enabled = cfg!(test) || FN64_GPU_TRIANGLE_DRAW=1`). This is why Task 22's render field is 43.24 ms vs the 2026-08-21 pre-fix 59.84 ms. It is the reason the CPU rasterizer is now the honest majority of `execute` rather than a ~15 % minority.

So the "biggest single lever" is spent. What remains inside `execute` on the current lane is genuinely the CPU rasterizer (`targets/raw_triangle.rs::raster_triangle` → `tmem/sample.rs::sample_point` + `combine_one_texel` + `blend_and_write_pixel`). The ranked plan below targets *that* path, and every top candidate is a per-pixel redundancy verified by reading the source, none of which appears in the closed-lines ledger.

---

## 1. The measured chain, top to bottom (rs + wgpu)

All rows below are **measured**, with their provenance and profiled/unprofiled state marked. The shipped drawn-frame figure is the **unprofiled** mean, doubled.

| layer | value | source | profiled? |
|---|---:|---|---|
| **drawn frame = render field + off-field** | **49.07 ms** (43.243 + 5.828) → **~20.4 fps**, budget 33.333 ms, **1.47×** | Task 22, unarmed run | unprofiled ✓ |
| render (slow) field mean | 43.243 ms | Task 22 | unprofiled |
| off field mean | 5.828 ms | Task 22 | unprofiled |
| `gfx_lle_rdp_ns` share of the render-field tail | **88.9 %** | Task 22, armed (shares only) | profiled→shares |
| ├ raw-DPC **execute** stage | **87.9 %** of the graphics bucket (15.72 s / 55,486 submissions = 0.283 ms/sub) | Task 22 `FN64_SESSION_PHASE_CENSUS` | profiled→shares |
| ├ plan | 11.0 % | Task 22 | profiled→shares |
| └ commit / finalize | 1.1 % / 0.1 % | Task 22 | profiled→shares |

The four-phase census (`session_phase_census.rs`) is inclusive **and stops at `execute`** — by design it "does not reach inside the rasterizer" (module doc). So the split *below* `execute` is not re-measurable from the shipping census; it comes from the last direct-`Instant::now()` timing run.

### Within `execute`, from direct timing (2026-08-21, rs+wgpu, `Instant::now()` brackets — the numbers are pre-draw-gate; the *structure* is current)

```
session census Execute            18.18 s   (pre-gate denominator)
 ├─ draw_admitted_triangles      ~13   s    ← GATED OFF on play lane since 1a10b939 → ~0 s now
 └─ execute_raw_dpc_inner          5.29 s    ← the CPU path that writes guest RDRAM
      └─ stage_color_commands cmd loop ~3.6 s  == raster_triangle
           raster_triangle          3.88 s   94–102 ns/covered-pixel (a normal SW-rasterizer figure)
```

**Post-gate reprojection (the current lane):** with `draw_admitted_triangles` ≈ 0, `execute` collapses onto `execute_raw_dpc_inner` ≈ 5.29 s, of which `raster_triangle` is **≈ 3.88 s / 5.29 s ≈ 73 %**. The remaining ~1.4 s of `_inner` is TMEM load loops (measured 0.80 s, 5.5 loads/call), the accumulation-buffer clone (0.03 s), and the plan walk (0.02 s) — already characterised and mostly rule-12 territory. **So on the current lane the CPU per-pixel rasterizer is ~70–75 % of `execute`, i.e. ~62–66 % of the whole render field.** That is now the wall.

> **Caveat, stated loudly:** the 3.88 s / 5.29 s absolutes are from the pre-gate run. The *shares* survive (rule 17), but a fresh armed run on the current binary would tighten the raster-vs-loads split. §5 proposes the exact probe; I did not run it because it needs a temporary edit + a ~15 min full emit/build, and the shares are already decisive for ranking.

### The per-pixel structure of `raster_triangle` (verified by reading `raw_triangle.rs:487–758`)

Per **covered textured pixel** (WM2000's triangles are 0x0e shaded+textured; point-sampled), in order:

1. `attribute_sample` — one subsample scan (already de-duplicated from two scans; comment lines 497–523).
2. **shade planes** (4×) — **already incrementally stepped** via `attribute_plane_step` when the run continues (one `checked_add` each); full `attribute_plane` only on run breaks.
3. **texture S/T/W planes** (3×) — **always full `attribute_plane`** (lines 603–605): each is an **i128 multiply + `div_euclid` + two `checked_add`** (`triangle_span.rs:389`). **Not stepped**, unlike shade.
4. `texture_coordinates_s10_5` — perspective divide.
5. **`sample_point`** (`sample.rs:406` → `read.rs::read_texel:419`): `preflight` + `validate_address_scope` + `AddressScope::of` + `state.snapshot()` + `read_raw_texel` (per-byte `valid_byte` Option-check) + format decode + (for CI) TLUT lookup.
6. depth compare (only when z-wired).
7. `combine_one_texel` (`texrect.rs:2072`).
8. `blend_and_write_pixel` (`texrect.rs:2647`) — ranked joint-first with digests in an old `sample` profile (but `sample` is untrustworthy here per [[sample-cannot-see-guest-coroutines]]).

---

## 2. Ranked cost centers inside the rasterizer (current lane)

Ranked by measured/derived share of the render field. Denominators stated (rule 32).

| rank | cost center | current share | basis |
|---|---|---|---|
| **1** | **`raster_triangle` per-pixel loop as a whole** (sample + combine + blend + plane interp) | **~62–66 % of the render field** (~70–75 % of `execute`) | 3.88 s raster / 5.29 s `_inner`, post-gate reprojection |
| 2 | — inside it: **`sample_point` / `read_texel`** (TMEM addr + format/TLUT decode + per-byte validity) | dominant sub-term of the per-pixel loop (largest single callee; textured pixels only) | structural read; `read_texel` is the heaviest per-pixel callee and runs the redundant `preflight`/`snapshot` per pixel |
| 3 | — inside it: **texture S/T/W plane interpolation** (3× full `attribute_plane`, i128 mul/div) | 3 i128-mul+div per pixel vs shade's 3 adds | `raw_triangle.rs:603–605` + `triangle_span.rs:389` |
| 4 | `combine_one_texel` + `blend_and_write_pixel` | remainder of the per-pixel loop | structural; blend ranked high in an (untrusted) leaf profile |
| — | ~~unpresented GPU draw `draw_admitted_triangles`~~ | **~0 (already gated off, `1a10b939`)** | not a candidate — spent |
| — | ~~SHA-256 content digests (`guest-read-content.v1`)~~ | **largely already migrated to xxh3-128** | see §4 |
| — | per-triangle `to_vec` framebuffer copies; GPU buffer pooling; plan double-walk | ruled out | closed / rule-12 |

---

## 3. Top candidates with kill-evidence design

Every candidate is **byte-identity preserving by construction** (it changes *how* a value is computed, never the value), which is exactly the property the byte-identity gate exists to police.

### Candidate A (HIGHEST VALUE) — step the texture S/T/W planes across a run, like shade

**What to change.** In `raw_triangle.rs::raster_triangle`, the shade planes already carry `previous_sample`/`continues_run` incremental state and use `attribute_plane_step` (one `checked_add`) when the subsample advances exactly one pixel. The **texture S/T/W planes do not** — line 603–605 unconditionally calls the full `attribute_plane` (an i128 multiply + `div_euclid`) for all three planes every textured pixel. Extend the existing run-state machinery to also carry the three S/T/W plane values and step them with `attribute_plane_step` on `continues_run`, restarting from `attribute_plane` on a run break — identical to the shade arm already 30 lines above.

**Expected mechanism.** Replaces `3 × (i128 mul + i128 div_euclid + 2 checked_add)` per textured pixel with `3 × checked_add`. On WM2000's spans (long horizontal runs; the `attribute_sample` de-dup already established runs are the common case) the run-continues branch dominates, so nearly every textured pixel drops the three most expensive integer ops in the loop. i128 div is the single most expensive scalar op in the per-pixel body.

**Why it is bit-exact (the whole safety argument).** `attribute_plane_step` is documented and proven bit-identical to `attribute_plane` while the selected subsample is unchanged and `edge_delta_x_q16` grows by exactly `Q16_ONE` — verified over 200,000 random `(dx, delta_x)` pairs spanning both signs (`triangle_span.rs:404–426`). The shade path already relies on this exact identity; texture planes have the identical algebra. The `continues_run` predicate (same subsample Y, `delta_x + Q16_ONE`) is already computed for shade and is reused unchanged.

**Kill-evidence measurement (deterministic, ns/pixel).** Same pump census (`WARMUP=300 PUMPS=1200`), interleaved A/B (rule 5), ≥2 reps (rule "two or more reps, always"):
- Metric: **ns per covered textured pixel** = `raster_triangle` wall (a temporary `Instant::now()` bracket around the call, corrected by an armed/control ratio per rule 17) ÷ total pixels from the combiner census histogram (`note_pixel`, summed TEXEL_LUMA buckets — that counter already runs per covered pixel).
- Prediction to pre-register: a measurable drop in ns/pixel with the effect same-signed across both reps; falsifier = no drop or reversed sign (would mean run-breaks dominate, i.e. spans are short — then this candidate dies and #2 is the target).

**Byte-identity check.** (1) The 1.5M-step render-benchmark route must reproduce the frozen tuple exactly (`scripts/check-byte-identity.py` vs `scripts/byte-identity-1p5M.txt`: `gfx_submits=11153 audio_submits=7685 sp_tasks=18838 vi_interrupts=8386 controller_ops=2390 sim_time=13112786076 render_error=None`). (2) The parity/conformance gate must stay green. (3) A frame tripwire over the census scene must be byte-identical (the same 120/120 byte-identical proof `1a10b939` used). Any deviation means the step identity was violated on some subsample transition — refute and stop.

### Candidate B — hoist `preflight` / `AddressScope::of` / `snapshot()` out of the per-pixel read

**What to change.** `read_texel` (`read.rs:419`) runs, **per pixel**: `preflight(tile, lut_mode)` — which *decodes a zero texel every call* just to classify Direct vs Indexed — plus `validate_address_scope`, `AddressScope::of`, and `state.snapshot()`. For one triangle the tile descriptor, `lut_mode`, and TMEM image are **constant**, so `ReadKind`, the address scope, and the snapshot identity are constant across all its pixels. Resolve them once per triangle (or once per `sample_point` binding) and pass the resolved `ReadKind`/`AddressScope`/snapshot into the per-pixel read, exactly as `raw_triangle.rs` already hoists `first_row_parity` and `census_pixels` out of the loop for the same stated reason ("this is the per-PIXEL path, and a live lane is measuring frame rate", lines 452–486).

**Expected mechanism.** Removes a texel-decode-of-zero (`preflight`), a scope recompute, and an atomic/identity read (`snapshot`) from every textured pixel; leaves only the addressing + real read + decode + TLUT. This is the classic "loop-invariant hoist" — pure setup, no arithmetic change.

**Why it is bit-exact.** The hoisted values are functions of `(tile, size, lut_mode, image)` only, all invariant within a triangle's `raster_triangle` call. The per-pixel result is unchanged; only the number of times the invariant is recomputed changes.

**Kill-evidence + byte-identity.** Same ns/pixel A/B harness and the same three-part byte-identity gate as Candidate A. Additional guard: assert (in a temporary debug build) that the once-per-triangle `ReadKind`/scope equals the per-pixel recomputation for a full census run before trusting the hoist — this is the "prove the check can fail" discipline (rule 6a) applied to the invariant.

### Candidate C (smaller, sequenced last) — reduce per-byte `Option` validity checks in `read_raw_texel`

`read_linear_bytes` (`read.rs:584`) calls `read_valid_byte` per byte, each returning `Option<u8>` and `?`-propagating an `InvalidTexelByte`. For a texel that is N bytes this is N branch-on-Option. Candidate: a single validity/range check per texel word rather than per byte where the scope mask guarantees contiguity. Lower confidence (the odd-row XOR4 exchange complicates contiguity) and smaller share; propose **only after** A and B are measured, and drop if A/B already close the gap. Same ns/pixel + byte-identity gate.

---

## 4. Closed-ledger / prior-work cross-check (which lines I cleared, and what I dropped)

Checked every entry in `REFERENCE.md`'s closed-lines ledger and the relevant memories:

- **async RSP dispatch, HLE graphics for this ucode, `FN64_FAST_MUTATION_JOURNAL`, instruction budgeting, reducing dispatch count, RSP threading, RSP micro-optimization, depth-buffer copy elimination, narrowing the DPC copyback** — none touch the per-pixel texture-plane interpolation or the per-pixel TMEM-read setup. Candidates A/B/C are outside all of them. In particular the "RSP micro-opt" line is explicitly scoped to the **RSP interpreter (17.6 % of graphics)**, not the RDP/renderer (82.3 %); my candidates are in the RDP renderer.
- **DROPPED — unpresented GPU triangle draw (~65 % of execute).** Not in the ledger but **already fixed** by `1a10b939` (memory [[wm2000-perf-gpu-draw-is-unpresented]]): gated behind `cfg!(test) || FN64_GPU_TRIANGLE_DRAW=1`. On the play lane it is ~0. This was the biggest single lever and it is spent; re-proposing it would be re-doing landed work.
- **DROPPED / RE-MEASURE-BEFORE-TOUCHING — SHA-256 content digests ("~20 %", memory [[wm2000-wgpu-perf-attribution]], 2026-08-21).** The hot guest-read **set identity** has since migrated from SHA-256 to **`FastContentDigest` = xxh3-128** (`digest.rs:8`; `guest_read.rs:177,208` use `guest_read_fast_content_digest`). The slow SHA-256 `guest_read_content_digest` (`guest_read.rs:426`) now runs only on `try_new_with_digest` (a verify path) and the documented-cold `content_digest()` accessor — `try_new`'s own comment (lines 155–165) records that the double-SHA on the hot path was already removed. So the "~20 % SHA-256" figure is **stale**; do not re-cite it. If a residual digest cost is suspected, it must be *re-measured* on the current binary before any optimization — it is not a current top candidate.
- **DROPPED (rule 12) — per-triangle `to_vec` framebuffer copies (13.8 GB / 0.43 s streaming), GPU buffer pooling (−7.7 %), plan double-walk (no), TMEM-load O(n²) (no).** All ruled out by direct measurement in [[wm2000-perf-gpu-draw-is-unpresented]]; do not re-run.

---

## 5. What is genuinely unmeasured, and the smallest substrate to close it

The shipping census stops at the four phases and never enters `execute`. To convert the §2 ranking from "shares reprojected from a pre-gate run" into fresh current-lane ns/pixel numbers you need **one temporary probe**, not a new harness:

- Add three `Instant::now()` brackets inside `execute_raw_dpc_inner`: around the whole call, around the `stage_color_commands` command loop, and around `raster_triangle` itself; accumulate into three atomics reported at census exit (mirror `session_phase_census.rs`'s at-exit reporter). Gate on a temporary raster-split env flag with the same strict `env_flag` semantics (empty/`0`/absent all off) so an unrecognised value can't default a lane on (rule 6b). Divide by the combiner census pixel total for ns/pixel. Arm an armed/control pair and quote **shares**, correcting absolutes by the ratio (rule 17).
- This is a `fn64-render-wgpu` edit (cheap-ish) but it forces a full `recompile_rom` emit + play build (~15 min, GUI, one owner per run). I did **not** run it for this report: the *shares* already rank the candidates decisively and the top candidate (A) is a bit-exact algebraic swap whose own A/B run (§3) will produce the fresh ns/pixel number as a side effect. Instrument once, when landing A, rather than twice.

**Deterministic-scene verdict:** the pump-census lever (`FN64_PUMP_CENSUS_WARMUP/PUMPS`) **exists and is clean** — Task 22 and the 2026-08-21 run both reproduced to <0.3 % on every headline, and the 2:498 gap histogram proves the render/off-field structure is stable. No new substrate is needed for the A/B; the only new code needed is the optional finer-grained raster-split probe above.

---

## 6. Provenance / honesty notes

- Every number states its renderer (all rs+wgpu) and profiled/unprofiled state. The shipped 49.07 ms/20.4 fps figure is unprofiled-mean × the paired off-field (not from p50s, not from profiled numbers).
- The within-`execute` absolutes (3.88 s / 5.29 s / ~13 s) are **pre-draw-gate** (2026-08-21); I reproject them as shares (rule 17) and flag the draw term as now ~0. I did not fabricate a post-gate absolute.
- No production code changed; no worktree; no commit; no instrumentation armed. The one probe that would sharpen §2 is specified but deliberately deferred to the A-landing run.
- The closed-lines ledger was read in full; two big-looking candidates (GPU draw, SHA-256) were dropped as already-landed/stale rather than re-proposed.
