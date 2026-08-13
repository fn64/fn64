# RT64 on the block lane: the ownership objection does not hold

Written 2026-08-07, after the first phase-resolved profile of a *rendering*
route. This is a **finding, not an implementation**. Nothing here was built;
everything here was read.

## Why this was re-opened

The blocker ledger records:

> RT64 does not apply to this lane, correcting an earlier assumption:
> `examples/wm2000-block-boot/Cargo.toml:40-41` depends only on
> `fn64-render`/`fn64-render-reference`. [...] Different contracts, not
> variants -- the reference backend is the correct choice here, not a
> fallback, and the recorded RT64 speedup does not transfer.
>
> -- `docs/plans/wm2000-playable-blocker-ledger.md:272`

That was written before anyone had profiled a route that renders. The profile
now exists, and it changes the stakes: **the software rasterizer is the
single largest line in the whole system.**

| component | % of executor | ms per field |
|---|---:|---:|
| **RSP graphics LLE - raw RDP rasterization** | **31.6%** | **11.00** |
| executor self (guest + runtime + guard) | 37.1% | 12.93 |
| VI present (`vi::scanout` chain) | 11.6% | 4.05 |
| RSP audio LLE (ucode interpretation) | 11.0% | 3.83 |
| graphics LLE other (setup/commit/copies) | 3.6% | 1.26 |
| graphics HLE preflight | 3.6% | 1.24 |
| RSP graphics LLE - RSP interpretation | 1.5% | 0.53 |

And a second fact the profile settled: **WM2000's graphics are not HLE'd.**
`gfx_lle tasks=4900` equals every graphics submit on the route. The display
list goes RSP-LLE -> raw RDP commands -> `dispatch_captured_raw_rdp` ->
`RenderBackend::process_rdp_commands` -> the scalar software rasterizer.

## The objection, and why it fails

The ownership argument originates at
`examples/wm2000-block-boot/src/shell.rs:15`:

> They also differ on RDRAM OWNERSHIP [...] The function lane hands `fn64-abi`
> a pointer to RDRAM the harness keeps [...] The block lane's bootstrap
> transaction VALIDATES an owned allocation and MOVES it into the runtime, so
> nothing outside `fn64-abi` holds the framebuffer bytes afterwards. **A
> windowed block-lane runner must therefore read the VI framebuffer back
> through the runtime** [...] which is a different present path.

Read precisely, that paragraph is about **the present path and the file
layout**. It is correct about both. It has since been generalized into a claim
about *backend applicability*, which it does not support. Three independent
levels refute the general claim.

### 1. Both lanes converge on the same registration call

- Function lane: `boot_thread0` -> `register_process_rdram(rdram, rdram_len)`
  (`crates/fn64-abi/src/host.rs:315`).
- Block lane: `install_owned_process_rdram` -> `host.owned_runtime_rdram =
  Some(storage)` -> `register_process_rdram(pointer, length)`
  (`crates/fn64-abi/src/host.rs:127-145`).

After that call `HostState.runtime_rdram` is the same raw pointer and the same
length in both lanes. The residual difference is **which struct's `Drop` frees
the allocation**, plus page alignment for the `mprotect` barrier. Neither is
observable to a render backend.

### 2. The `RenderBackend` trait expresses no ownership at all

`crates/fn64-render/src/lib.rs:1144`:

```rust
fn process_rdp_commands(
    &mut self, rdram: &mut [u8], start: u32, end: u32, output_addr: u32,
) -> Result<FrameStatus, RenderError>;
```

No `Vec`, no `Box`, no `'static`, no handle — a call-scoped `&mut [u8]`. A
backend that satisfies this against a harness-owned buffer satisfies it
identically against a runtime-owned one, because `fn64-abi` synthesizes the
slice in both cases (`renderer_rdram_slice`,
`crates/fn64-abi/src/task_dispatch/rsp_phase.rs:767`).

### 3. On WM2000's actual path the backend never sees process RDRAM anyway

`dispatch_captured_raw_rdp` hands the backend a **staging copy**, not the
registered allocation (`crates/fn64-abi/src/task_dispatch/rsp_commit.rs:1086`):

```rust
let mut image = vec![0u8; staged_end];
image[..physical_len].copy_from_slice(real);
...
backend.process_rdp_commands(&mut image, staging_start as u32, ...)
```

then copies back at `rsp_commit.rs:1131`. So today, in the block lane, with the
reference backend, the renderer already operates on an ABI-local `Vec` that has
no relationship to who owns the process allocation. **Swapping the backend
behind that same `&mut image` changes nothing about ownership.**

## RT64 supports the path WM2000 actually uses

This was the crux and the answer is favorable.
`crates/fn64-render-rt64/src/lib.rs:1285` implements `process_rdp_commands`
against a real native call (`fn64_rt64_process_rdp_commands`,
`crates/fn64-render-rt64/src/ffi/context.rs:386`), with an RDRAM rollback
transaction, and it sets `last_dp_full_sync` — which is what
`require_committed_full_sync_evidence`
(`crates/fn64-abi/src/task_dispatch/rsp_commit.rs:1140`) demands. It satisfies
the dispatcher's contract, not merely the signature.

`raw_dpc_batch_capability` returns `Unsupported`
(`crates/fn64-render-rt64/src/lib.rs:1347`) — **and this does not matter.** The
reference backend's own capability is `DiagnosticOnly`
(`crates/fn64-render-reference/src/backend/render_backend.rs:145`), and
`dispatch_captured_raw_rdp` calls `process_rdp_commands` directly. Batching is
a diagnostics seam, not the production route.

## The real blockers, in order

**A. RT64 is simply not wired in.** `examples/wm2000-block-boot/Cargo.toml`
lists only `fn64-render` and `fn64-render-reference`; there is no `rt64`
feature and no backend selector. Compare `examples/wm2000-boot/Cargo.toml:20`
(`rt64 = ["fn64-render-rt64/rt64"]`) and the `FN64_RENDER` selector in
`examples/oot-boot/src/main.rs:591`. **Missing plumbing, not an
incompatibility.**

**B. RT64 needs a GPU and a display server, even "headless."** There is a
hidden-window mode, but it is hidden-*window*, not window-*less*
(`crates/fn64-render-rt64/ffi/fn64_rt64_shim.cpp:1400`, `SDL_WINDOW_HIDDEN |
SDL_WINDOW_METAL`). Hard prerequisites: macOS **main thread**
(`shim.cpp:1380`, an explicit guard that turns a worker-thread embedder into a
recoverable error rather than an Objective-C crash), `SDL_VideoInit`, a Metal
system-default device, and a real `NSWindow`/`CAMetalLayer`. Satisfiable
locally; **not** satisfiable in detached CI, and the `rt64` Cargo feature is
non-default precisely so CI/no-GPU hosts still build.

Whether `wm2000-block-boot`'s benchmark loop runs on the main thread must be
checked before assuming this works. The precedent exists: `examples/wm2000-boot`
already does it.

> **Checked 2026-08-07: it does.** `examples/wm2000-block-boot/src/main.rs`
> contains exactly one `std::thread::spawn`, at `:1036`, and it is an opt-in
> `FN64_BLOCK_WATCHDOG` diagnostic that only prints `entries=`/`last_pc=` every
> five seconds. The execution loop itself is not spawned — it runs on the main
> thread, so RT64's macOS main-thread guard (`shim.cpp:1380`) is satisfied for
> the headless lane without restructuring.
>
> Taken with **C** being "mostly moot" for headless, this narrows the headless
> benchmark path to essentially **A alone** — the Cargo wiring, with a working
> precedent at `examples/wm2000-boot/Cargo.toml:20`. That is the cheapest place
> to get a real number for the 31.6% rasterizer line, and it is a different
> question from shipping RT64 in the windowed shell, where **C** is real work.

**C. Presentation semantics genuinely differ — this is where the original
intuition had real content.** RT64's `present` requires
`PresentMemory::Physical` (`crates/fn64-render-rt64/src/lib.rs:1375`,
"RT64 presentation requires current physical RDRAM authority") and renders into
its own GPU surface rather than writing back to `rdram[output_addr..]`. The
block lane reads the framebuffer back through
`fn64_abi::with_registered_physical_rdram_read`
(`examples/wm2000-block-boot/src/shell.rs:903`) exactly as the shell.rs comment
says. **For the headless benchmark lane this is mostly moot** — there is no
windowed present. For a windowed `wm2000-shell` it is real work.

**D. Behavioral risk, not a blocker.** RT64 requires 8-byte-aligned
`start`/`end` and an in-bounds `output_addr`
(`crates/fn64-render-rt64/src/ingress.rs:53`). `dispatch_captured_raw_rdp`
stages commands *past* `physical_len`, so RT64 receives an 8 MiB+N buffer whose
tail is synthetic. The reference backend does the equivalent
(`render_backend.rs:121`), but RT64's native side should be verified against it.

## What the recorded 12x actually measured

`rt64-throughput-win` records reference 57.31 s vs RT64 4.82 s, ~11.9x, ~56 fps.
That was measured on **`examples/wm2000-boot`, the function lane** — which
carries the `rt64` feature — not on `wm2000-block-boot`. The ledger is right
that the measurement was taken elsewhere. It does not follow that the speedup
cannot transfer: both lanes drive the same `dispatch_captured_raw_rdp` ->
`process_rdp_commands` seam.

## The correct restatement

> RT64 has **not been wired into** the block lane, and the recorded speedup was
> measured on the function lane, so it is **unverified here** — not "RT64
> cannot apply."

`docs/plans/wm2000-playable-blocker-ledger.md:272` should be corrected, and
`examples/wm2000-block-boot/src/shell.rs:15` should scope its ownership
argument to the present path, which is the only place it is load-bearing.

## What it would take to get a number

Add `fn64-render-rt64` and an `rt64` feature to
`examples/wm2000-block-boot/Cargo.toml`, mirror the `FN64_RENDER` selector from
`examples/oot-boot/src/main.rs:591` into the backend registration at
`examples/wm2000-block-boot/src/main.rs:830`, confirm main-thread execution,
and build with `FN64_RT64_DIR` set. **Nothing in the ownership model has to
move.** Roughly thirty lines of plumbing to a measurement, against a component
that is 31.6% of executor time — and that share is a *floor*, because it was
measured on the pre-`5ed7f2c` menu route, which carries 2.42x less graphics
work per step than the gameplay route.

## What this does not claim

That RT64 will be faster here. That is what the measurement is for. This
document only establishes that **the stated reason for not measuring is
wrong**, and that the cost it would attack is the largest one there is.

---

# MEASURED 2026-08-08: under RT64 the RDP is 5.75 ms, not 26.9. "Fix the RDP" does not describe the owner's configuration.

The question that prompted this: the owner said **"fix the rdp"** after a
decomposition showed graphics at 70% of the render field and the RDP at 57.8%
of `resume NET`. **That decomposition ran on the SOFTWARE reference
rasterizer.** `reference/wm2000-routes/render-benchmark.zsh` never exports
`FN64_RENDER`, and `examples/wm2000-block-boot/src/main.rs:863` defaults to
`"reference"`. The owner's windowed sessions run `FN64_RENDER=rt64`.

So the prior figure was re-measured against the configuration he actually runs,
before anything was optimized.

**Four runs, `reference`/`rt64` interleaved, two reps, 1.5M steps, route
`entrance-to-match.schedule`, quiet machine (load settled below the
benchmark's own 3.0 gate before each run rather than relaxing the gate).**
Frozen logs (rule 29):
`$CLAUDE_JOB_DIR/tmp/frozen-rdp-step1/{reference,rt64}-rep{1,2}.full.log`.
Pre-registration, written before the measuring binary existed:
`$CLAUDE_JOB_DIR/tmp/PREREG.md`.

**Guest byte-identical in all four runs**, checked with
`scripts/check-byte-identity.py` against `scripts/byte-identity-1p5M.txt`:
8 of 8 — `gfx_submits=11153`, `audio_submits=7685`, `sp_tasks=18838`,
`vi_interrupts=8386`, `controller_ops=2390`, `sim_time=13112786076`,
`render_error=None`, `fields=7699`. Same guest program, different host cost.

## The answer

| slow (render) field | reference | rt64 | change |
|---|---:|---:|---|
| `gfx_lle_rdp_ns` (**the RDP**) | 26.953 / 26.807 | **5.771 / 5.736** | **4.7x less** |
| — of which staging memcpy | 1.972 / 2.158 | 1.772 / 1.740 | — |
| — of which rasterization+ | 24.981 / 24.649 | **3.999 / 3.995** | **6.2x less** |
| `gfx_lle_rsp_ns` (RSP interp) | 5.691 / 5.666 | 5.769 / 5.754 | unchanged |
| `vi_present_ns` | 3.882 / 3.876 | **0.890 / 0.893** | **4.4x less** |
| dispatch = guest code | 9.789 / 9.706 | 9.780 / 9.793 | unchanged |
| `resume NET` | 46.711 / 46.330 | **25.499 / 25.457** | 1.82x less |
| `executor_ns` | 55.949 / 55.567 | **34.795 / 34.741** | 1.60x less |
| **RDP as % of `resume NET`** | **57.7 / 57.9** | **22.6 / 22.5** | −35 pp |

Whole-route, in the owner's terms:

| | reference | rt64 |
|---|---:|---:|
| **ratio A** | 2.13x / 2.12x | **1.34x / 1.34x** |
| mean ms/field | 35.49 / 35.26 | **22.31 / 22.33** |
| p50 | 32.90 / 33.44 | **16.61 / 17.02** |
| p95 | 65.98 / 66.06 | 38.43 / 38.19 |
| over budget | 50.1% / 50.0% | 50.0% / 50.0% |

**Ranges fully disjoint; each lane reproduces within 1%.** RDP share agrees
across reps to **0.2 pp** (reference) and **0.1 pp** (rt64), far inside the
3 pp pre-registered threshold.

## Judged against the thresholds fixed before the data existed

| # | threshold | result |
|---|---|---|
| T1 | RDP "still dominant" iff > 8.33 ms/field | **5.75 ms — NOT dominant** |
| T2 | RDP "collapsed" iff < 20% of `resume NET` | **22.6% — not collapsed** |
| T3 | reps agree within 3 pp | **0.1–0.2 pp — passes** |
| T4 | closure within 5% | **slow 1.2–2.3% — passes** (see the fast-field note) |

**T1 and T2 disagree, and that is reported rather than resolved in favour of
the tidier story.** The RDP is no longer the thing that makes the field miss
the bar by itself — 5.75 ms against a 16.667 ms budget — but at 22.6% it is
still the second-largest line in `resume NET`. The honest statement is
**"greatly reduced, not eliminated"**, which is neither of the two headlines
that were available before measuring.

### The falsifier fired against my own brief, in the direction I wrote down

The brief that commissioned this predicted RT64 would collapse the RDP and that
the 70% would prove to be an artifact of a lane the owner does not use. The
pre-registered falsifier was: *if rt64's RDP is within 20% of reference's, that
framing is wrong.* It is **78.6% lower**, so the framing survives — the lanes
genuinely differ, and the reference-lane figure did not describe the owner's
configuration.

**But the framing was still half wrong, and the half that failed matters more.**
"The RDP is already fixed for his configuration" is **not** what the data says.
5.75 ms/field is real, it is 22.6% of `resume NET`, and 1.77 ms of it is a
staging memcpy that has nothing to do with rasterization.

## The prior 1.28x figure cannot be compared to this, in either direction

`perf-method.md:502-532` records RT64 on this lane at **1.28x** (56.28 → 44.13
ms/field). Anyone reading the 1.60x above will reach for it. **It is not
comparable:** it was measured before `abc7871` (the nested-writer view fix,
44.13 → 22.51), and this file's own note on that commit says *"Not
re-profiled. Everything in the table below this line describes the 44.13 ms
world and is now stale."* The profile inverted at that commit. Both numbers are
correct about different programs.

The 2026-08-07 entry's third finding — *"the speedup does not scale with
graphics density, so a large share of what RT64 was expected to remove is not
rasterization"* — is **confirmed and now has a mechanism**: rasterization was
never the whole of the reference lane's RDP bucket, and the guard work that
dominated the field then has since been removed by `abc7871`.

## What the reference lane's 26.9 ms actually contained

`fn64-render-reference`'s `process_rdp_commands`
(`crates/fn64-render-reference/src/backend/render_backend.rs:121` and `:140`)
does **a second whole-RDRAM `to_vec()` clone and a full copy-back**, nested
inside `dispatch_captured_raw_rdp` — i.e. *inside* the counter that read 26.9
ms, and on top of the staging copy `FN64_DPC_COPY_CENSUS` already names. RT64
pays none of it.

So the reference lane's "RDP" was software rasterization **plus** an 8 MiB
round-trip per submission that the owner's configuration does not perform.
**A bucket named for a hardware unit contained an artifact of one backend
choice** — the same failure mode as an unnamed 83% inviting a story about its
contents.

## The DPC staging copy: still not the target, now measured on both lanes

`FN64_DPC_COPY_CENSUS` was armed in all four runs.

| | reference | rt64 |
|---|---:|---:|
| staging (alloc+copy_in+copy_back) | 1.97 / 2.16 ms/field | 1.77 / 1.74 ms/field |
| as share of the RDP seam | **7.3% / 8.1%** | **30.7% / 30.3%** |

**Unit note, because getting this wrong is easy:** the census reports
*whole-run totals*; the RDP figures are *per-field on the slow population*.
The only sound bridge is the census's **per-call** microseconds multiplied by
that population's own `dpc_calls`/field (2.816–2.818), which
`[frame-populations]` samples separately. Subtracting a whole-run total from a
per-field value would be a cross-population subtraction wearing a disguise.

Read this carefully: **the staging copy barely moved between lanes (1.97 →
1.77), so its share tripled purely because its denominator collapsed.** It is
~1.8 ms/field either way — 10.6% of the whole 16.667 ms budget for a memcpy
that produces no pixels. Candidate 0 is **not** vindicated as a large win, but
it is no longer negligible relative to what remains. Rule 12 still applies: the
129.6 GB byte count is not the argument, the 1.8 ms is, and eliminating it is
worth at most that.

## A negative control nobody designed, and it is the strongest evidence here

`gfx_lle_rsp_ns` is armed around `run_imem` **only**
(`crates/fn64-abi/src/task_dispatch/rsp_commit.rs:142-147`). The renderer
choice cannot reach it, so it is a built-in control on whether the two lanes
are otherwise the same measurement.

    reference  RSP 5.691 / 5.666 ms/field
    rt64       RSP 5.769 / 5.754 ms/field

And `[rsp-step-census]` is **identical to the instruction** across lanes:
`entries=41144`, `gfx_steps=1949499081`, `audio_steps=780097224`,
`total_steps=2729596305`, `imem_rebuild_words=42131456`. Guest code p50 also
matches (9.923 vs 9.893). The RSP interpreter retired the same 2.73 billion
instructions in both lanes, so the RDP difference is the renderer and nothing
else.

## What the render field is under RT64, and why graphics is no longer the lever

| row | ms/field | share of `resume NET` |
|---|---:|---:|
| `executor_ns` | 34.77 | — |
| — mirror boundary | 9.01 | (25.9% of executor) |
| — **`resume NET`** | **25.48** | **100%** |
| — — **dispatch = translated guest code** | **9.79** | **38.4%** |
| — — RDP total | 5.75 | 22.6% |
| — — — *of which* staging memcpy | 1.76 | 6.9% |
| — — — *of which* rasterization+ | 4.00 | 15.7% |
| — — RSP interpretation | 5.76 | 22.6% |
| — — invalidate writes | 2.04 | 8.0% |
| — — audio LLE | 1.17 | 4.6% |
| — — PARKED | 0.58 | 2.3% |

**The arithmetic that closes the question:**

- Take **rasterization to zero** → 30.8 ms/field = **1.85x budget**.
- Take **all graphics (RDP + RSP) to zero** → 23.3 ms/field = **1.40x budget**.

**Neither reaches 60fps.** This agrees closely with the independently measured
host-side-with-graphics-at-zero figure of 21.55 ms = 1.29x. **Under the
configuration the owner runs, graphics cannot get WM2000 to 60fps, because
everything that is not graphics already exceeds the budget.**

The two largest lines in `resume NET` are now **translated guest code (38.4%)**
and, outside it, the **mirror boundary (9.01 ms, 25.9% of `executor_ns`)** —
neither of which is graphics.

### And the closed RSP line needs its denominator restated again

The closed-lines list carries *"RSP interpretation (17.6% of graphics)"*, whose
scope was narrowed after the reference-lane measurement. **Under RT64 the
denominator changes completely: RSP is 50.0% of graphics and 22.6% of `resume
NET` — dead level with the entire RDP seam.** It is not reopened here (11.25
ns/instruction with no defect is still a fact, and 2.73 billion instructions is
the reason it is large), but a reader scanning that line for "is the RSP worth
looking at?" would now get a materially different answer than the title
suggests. **A closed line's title should state the denominator, and this one's
denominator just moved.**

## The one caveat on the numbers above

The fast (off-render) population's closure residual is **6.5–6.7%**, above the
5% pre-registered tolerance. Reported rather than waved through, per the
protocol. It is **0.193–0.203 ms of positive PARKED time in all four runs,
both lanes** — genuine coroutine suspension, never the impossible negative
gap, and constant regardless of renderer. The percentage is large only because
the denominator (3.0 ms) is small. **Every conclusion above rests on the slow
population, which closes at 1.2–2.3%.**

## What this redirects to

1. **Stop treating the RDP as the barrier.** Under `FN64_RENDER=rt64` it is
   5.75 ms of a 34.77 ms field, and 1.77 ms of that is a memcpy.
2. **The headless benchmark should state its renderer.** Every number produced
   by `render-benchmark.zsh` to date is a *reference-backend* number unless the
   caller exported `FN64_RENDER`, and nothing in its output says so. That is
   how a 70% graphics share came to describe a lane nobody runs. The script
   should echo the active renderer, and the census should print it.
3. **The remaining target is host-side**, in this order by size: translated
   guest code (9.79), the mirror boundary (9.01), RSP interpretation (5.76),
   invalidate writes (2.04), the DPC staging memcpy (1.77).

# MEASURED 2026-08-09, POST-MIRROR-FIX: the gap is 1.16 ms, not 22.6 ms. WM2000 is at 29.0 fps.

Everything above this line was measured **before** `8109435` (the mirror fix).
That commit removed a wholesale 1 MiB byte-by-byte RDRAM rebuild caused by
passing `None` where a view was in scope, and it invalidated every
decomposition on this page — the mirror boundary was **9.01 ms/field**, 25.9%
of `executor_ns`, and it is now **0.19 ms**.

This section re-measures the same route on the same lane after that fix.

## The headline, and it reframes the project

An agent was dispatched to close a **22.6 ms** gap on the premise that
"graphics is 75.6% of the render field". **Both figures describe the
`reference` software rasterizer, which is not the configuration the owner
runs.** Re-measured under `FN64_RENDER=rt64`:

| | briefed (`reference`) | **measured (`rt64`)** |
|---|---:|---:|
| per-field mean, unprofiled | 27.96 ms | **17.25 ms** |
| drawn frame (30 Hz, x2) | 55.9 ms | **34.49 ms** |
| fps | 17.9 | **29.0** |
| gap to the 33.33 ms budget | **22.6 ms** | **1.16 ms** |

**Two unprofiled reps, 17.23 / 17.26 ms/field, agreeing to 0.17%, both
`GUEST BYTE-IDENTICAL` (8 of 8) against `scripts/byte-identity-1p5M.txt`.**
Frozen: `$CLAUDE_JOB_DIR/tmp/rt64-control-FROZEN.log` and
`rt64-control-rep2-FROZEN.log`.

**The gap is 20x smaller than briefed, and it is 3.5% of the budget.** WM2000
renders at 29.0 fps against a 30 fps target on the headless block lane. The
strategic question "where do 22.6 ms come from" is malformed: *that gap does
not exist on the owner's lane.*

**Caveat that must ride with this number:** headless excludes presentation. A
windowed frame is this plus present cost, never less
(`render-benchmark.zsh:86`). 29.0 fps is the emulation ceiling, not a measured
player experience.

## The post-fix decomposition

`FN64_PROFILE=1`, `FN64_RENDER=rt64`, byte-identical, one run, frozen at
`$CLAUDE_JOB_DIR/tmp/rt64-postfix-rep1-FROZEN.log`. **Perturbation measured
against the control above: profiled 20.26 vs unprofiled 17.25 ms/field, so
+17.6%, correction factor 0.850.** Rule 17 — shares survive, absolute ms do
not, so both are given.

Render (slow) field, n=3614; off-field n=4085; the two populations reproduce
the census mean to 0.00%, and `over_16.667 == n_slow` exactly.

| row | profiled ms | **corrected ms** | % of render field |
|---|---:|---:|---:|
| **render field** | 32.56 | **27.68** | 100% |
| — graphics (`gfx_ns`) | 17.54 | **14.91** | **53.9%** |
| — — RDP total | 11.47 | 9.75 | 35.2% |
| — — — rasterization+ | 9.76 | 8.30 | 30.0% |
| — — — staging copies | 1.71 | 1.45 | 5.2% |
| — — RSP interpretation | 5.99 | 5.09 | 18.4% |
| — non-graphics | 12.64 | 10.74 | 38.8% |
| — — translated guest code | 9.68 | 8.23 | 29.7% |
| — — invalidate writes | 1.98 | 1.68 | 6.1% |
| — — audio LLE | 1.15 | 0.98 | 3.5% |
| mirror boundary | 0.19 | 0.16 | 0.6% |

Graphics is **53.9%** of the render field post-fix, not the 75.6% briefed —
and the briefed figure was a reference-lane number besides.

## An unexplained 2x that is probably RE-ATTRIBUTION, not regression

Comparing whole-route totals, same route, same lane, same guest:

| phase | Aug-8 (pre-fix) | Aug-9 (post-fix) | change |
|---|---:|---:|---:|
| `executor_ms` | 171,477 | 135,773 | **−35.7 s** |
| `gfx_lle_rdp_ms` | 22,887 | 43,742 | **+20.9 s (1.91x)** |
| — of which staging | 7,012 | 6,486 | −0.5 s |
| — of which rasterization | 15,875 | 37,256 | **+21.4 s (2.35x)** |
| `gfx_lle_rsp_ms` | 22,392 | 22,060 | 0.99x (flat) |
| `vi_present_ms` | 10,918 | 28,839 | **+17.9 s (2.64x)** |

**The program got faster overall** (census mean 22.31 → 20.26 ms/field
profiled; 17.25 unprofiled), so this is not a regression. RSP is flat, which
rules out a global slowdown. The two phases that rose are the two that touch
the framebuffer/RDRAM mapping, and the mirror they used to sit behind is gone
— so the natural reading is that work formerly billed to the mirror now
surfaces in `gfx_lle_rdp` and `vi_present`.

**That reading is NOT established.** No probe was run to confirm it, the
barrier stats were unarmed in both eras, and this page's own rule applies: *a
mechanism that explains the evidence is not thereby the cause.* Recorded as an
open question, with the arithmetic attached, rather than as a finding.
`vi_present_ns` is a tree ROOT (`counter_tree.rs:164`, parent `None`) — it is
**beside** `gfx_ns`, not inside it, so its 28.8 s is additional to graphics
and not double-counted in the table above.

## What this redirects to, replacing the list above

1. **The 22.6 ms framing is retired.** The gap is **1.16 ms**. Any plan sized
   against the old number is sized against the wrong problem by 20x.
2. **No single component "must" fall.** At a 1.16 ms gap, *four* separate rows
   are individually gap-closing if eliminated: staging copies (1.45), the
   invalidate writes (1.68), audio LLE (0.98 — nearly), or ~14% of
   rasterization. This is the opposite of the pre-fix situation where nothing
   sufficed.
3. **The copyback is still not one of them** — see the closed-lines entry:
   0.45% of the image changes and finding out costs 4.78x the copy. Confirmed
   on THIS lane, not just the reference one.
4. **Measure the windowed lane next.** The headless number is now close enough
   to the bar that presentation cost decides whether the target is met, and it
   has never been measured post-fix.

# MEASURED 2026-08-09: the full per-field distribution. The spikes are an early burst worth 0.29 ms/frame, and the render field is TIGHT.

The post-fix rt64 p50/p95/p99 **already existed** in the two frozen control
logs (`rt64-control{,-rep2}-FROZEN.log:177`) as a whole-population figure —
p50=10.39/10.31, p95=28.08/28.10, p99=28.80/28.82, max=1066/1073, mean
17.23/17.26. What did not exist was the **per-field sequence**, and therefore
the population split and the location of the ~1 s spikes.

`FN64_PROFILE` sets `FN64_FRAME_CENSUS_SEQUENCE=400` with skip=0, so the
profiled log samples only the **leading edge** of steady state: its in-window
max is 47 ms against a run max of 1117, and its window mean is 12.68 against a
run mean of 20.26. **That window is not representative and no spike was ever
inside it.**

One run, `FN64_FRAME_CENSUS_SEQUENCE=8000` + `FN64_FRAME_CENSUS_POPULATIONS=1`,
`FN64_RENDER=rt64`, 100% coverage of all 7,698 dumpable steady fields.
Frozen: `$CLAUDE_JOB_DIR/tmp/seq-rep1-FROZEN.log`. Pre-registration written
before the run: `$CLAUDE_JOB_DIR/tmp/PREREG-spike-distribution.md`.
**Guest byte-identical 8 of 8**, `fields=7699`.

## The pre-registered falsifier F1 FIRED: the populations gate perturbs +1.86%

Census mean **17.57 ms** against the unperturbed control's 17.25, outside the
pre-registered 17.08–17.42 window. `FN64_FRAME_CENSUS_SEQUENCE` is exit-time
only and costs nothing, but `FN64_FRAME_CENSUS_POPULATIONS` adds a
`with_executor` borrow plus ~20 atomic loads per field (frame_census.rs:469).
**Correction factor 0.9820; shares survive, absolutes are corrected** (rule 17).
Corrected drawn frame reproduces the control's 2 x 17.25 = 34.50 to **0.04%**.

## The distribution, per population

| population | n | mean | p50 | p95 | p99 | max |
|---|---:|---:|---:|---:|---:|---:|
| **render** (carries a gfx submit) | 3852 | 26.12 | 27.45 | 28.98 | 29.85 | 1136.05 |
| **non-render** (no submit) | 3846 | 9.00 | 8.93 | 9.73 | 9.89 | 10.20 |
| all steady | 7698 | 17.57 | 10.45 | 28.65 | 29.52 | 1136.05 |

**The populations are 1:1 (3852 / 3846, ratio 1.0016) and cleanly separated**
— 100% of over-budget fields carry a submit, and the non-render population's
entire range is 8.9–10.2 ms. So a 30 Hz **drawn frame is one render field plus
one non-render field**, not two average fields:

    render 25.65 + non-render 8.83 = 34.49 ms/frame = 29.00 fps   (corrected)

**All of the 1.15 ms overage sits on the render field.** The non-render field
is 8.83 ms — 25.6% of the frame — and is not a target.

## The p95 concern resolves to nothing: the render population is TIGHT

    render p25 25.27  p50 27.45  p75 28.11  p90 28.65  p95 28.98  p99 29.85

**IQR = 2.84 ms. p99 − p50 = 2.40 ms.** There is no fat tail to harvest inside
the render population; it is a dense band at 25–29 ms. **This is a mean
problem, not a tail problem** — which means the brief's "p95 materially
reduced" sub-goal is not a separate lever. Anything that moves the render
field's mean moves its p95 with it, and nothing else will.

## The ~1 s spikes: an EARLY BURST, not a recurring stall. Worth 0.29 ms/frame.

Every field above 100 ms, in the whole run:

| field | cost | gfx |
|---:|---:|---:|
| 829 | 135.77 ms | 2 |
| 1460 | 121.32 ms | 4 |
| **1806** | **1136.05 ms** | 3 |

**Three spikes, all inside fields 829–1806, and ZERO in the remaining 5,892
fields (~103 s).** The pre-registered discriminator was: *arena-load predicts
few + early + none in the second half; recurring-stall predicts even spacing
across both halves.* **Arena-load confirmed on all three counts;
recurring-stall refuted.**

Falsifier F3 (spike is a residual warmup transient the `warmup_gfx=300` gate
missed) **did not fire** — the earliest spike is field 829, deep inside steady
state, not in the first 100.

**Decision arithmetic, by the rule fixed before the data.** Replacing each
spike with the population median (a perfect fix still has to run the field):

| removed | fields | mean effect | **per drawn frame** |
|---|---:|---:|---:|
| >500 ms | 1 | −0.146 ms | **−0.29 ms** |
| >100 ms | 3 | −0.177 ms | **−0.35 ms** |
| >50 ms | 5 | −0.189 ms | **−0.38 ms** |

The pre-registered materiality floor was 0.20 ms/frame. **At 0.29–0.38 ms/frame
this CLEARS the floor** — it is 25–33% of the 1.15 ms gap from three fields —
but it is a one-off burst, so it is a **warmup/load** cost, not a steady-state
one. Its value is real but bounded and it does not recur.

**Cadence is NOT a rate for this run and must not be quoted as one.** A mean
gap of 488 fields reads as "a hitch every 8.6 s", which is false: all three
spikes fall in a 977-field span and nothing follows for 103 s. As a
player-experience item — kept separate from the mean, per the protocol — the
honest statement is **"a short burst of hitches early in the route, then none"**.

## What this redirects to

1. **The target is the render field's MEAN, and only that.** 25.65 ms
   corrected, one per drawn frame. To reach 33.33 ms/frame needs −1.15 ms off
   it; for the owner's "room to spare" (≤31.5 ms/frame, 5% margin) needs
   **−2.99 ms = 11.6% of the render field.**
2. **The non-render field (8.83 ms, 25.6% of the frame) is not a target** and
   neither is the tail. Both are already tight.
3. **The spikes are worth 0.29 ms/frame and are a load transient**, not a
   steady-state defect. Cheap if the arena load is schedulable; not the main
   line either way.
4. **Do not re-derive the distribution.** It is in `seq-rep1-FROZEN.log` at
   100% coverage, and the perturbation factor for that run is 0.9820.

---

# Addendum, 2026-08-13: the windowed RT64 lane, and where its remaining cost is

Written after a session that shipped nine perf commits on
`fix/overlay-stride-aliases`. Everything below was **measured on the windowed
RT64 lane** (`FN64_RENDER=rt64`, `wm2000-shell`), which is what a person
actually plays — not the headless reference lane the body of this document
profiles. That distinction cost most of a session to notice and is the first
finding.

## Finding 0: six of nine fixes were in a backend the windowed lane never runs

`FN64_RENDER` selects `reference` XOR `rt64` (`shell.rs:801`). Every windowed
session runs `rt64`. Four of this session's commits (`10b1996`, `f46ef27`,
`6adb548`, `10a2d68`) optimize `fn64-render-reference` — the software
rasterizer — and are **inert on the windowed lane**. They are real, verified,
byte-identical wins for the headless/reference lane and for this document's
own numbers; they are not wins for what a player experiences. Check which
backend a measurement's binary actually used before attributing a windowed
symptom to a reference-lane fix.

## Finding 1: audio starvation is a symptom of field-rate, not of audio

`audio_lle_ns` is 0.773 ms/field in the fast population and 0.774 ms/field in
the slow one — flat, ~2-5% of host-side cost, and it does **not** grow when
frames get slower. Starvation is measured at the host CPAL callback, which
drains on a real-time clock regardless of emulator progress. A shell comment
already recorded the mechanism: 29,281 channel-samples/sec delivered against
the 64,000/sec that 32 kHz stereo demands, sustained, with **zero** frame
spikes logged. Audio starvation therefore tracks whatever fraction of
real-time the whole emulator achieves; it cannot be fixed in the audio path.
Session movement: ~58% → ~49.6%, entirely from CPU/graphics-side work.

## Finding 2: the "it gets worse over time" report is the game, not a leak

`pump_ms` p95/p99 step from ~12-16 ms to a ~27 ms plateau inside the first
~1800 frames, then hold flat through frame 7320. Not continued decay.

The cause is measurable and mundane: `gfx_submits` per unit `sim_time` is
**~0.32 for the route's first ~1.6M sim-time units, then ~0.72-0.96 for the
rest** — the guest submits roughly 3x more RDP work per unit of game time once
`entrance-to-match.schedule` reaches match play. `GBI_RDP::fullSync` sample
count roughly doubles across the same boundary. The emulator is correctly
working harder because WM2000 is genuinely drawing more.

`workload_id` vs `present_id` is **not** a backlog: present increments
1.008/field, workload 1.332/field — a constant ratio for the whole route. The
absolute gap grows because the rates differ by design, not because anything
falls behind.

## Finding 3: the remaining windowed cost is a wait, and the GPU is idle during it

Live `sample` on the windowed process, busy/match phase:

| item | share of a 25 s capture |
|---|---:|
| `pump_one_frame` | 35.6% |
| `run_one_step` (guest emulation + dispatch) | 32.5% |
| `run_catalog_block_program` (recompiled MIPS) | 30.1% |
| `try_store_w_translated` | 7.4% |
| RT64 `fullSync` | 3.4% |
| RT64 `processDisplayLists` | 1.8% |

Within RT64's `fullSync`, **143 of 190 samples (75%) are
`_dispatch_semaphore_wait_slow` → `semaphore_wait_trap`**, inside
`RT64::State::fullSync()::$_5::operator()` (the `renderAndSynchronize`
lambda) at compiled offset +3320.

**And the GPU is idle while that wait happens.** `ioreg -r -c IOAccelerator`
`Device Utilization %` reads **0 across 8 consecutive 1-second samples** while
the shell is in its busy phase and the CPU is blocked. Ruled out by direct
measurement, in order: render resolution (fn64 correctly uses
`resolution_multiplier: 1.0`, native 320x240; the 1280x960 window is
presentation-layer scaling only), debug/validation overhead
(`CMAKE_BUILD_TYPE=Release`, no `MTL_*`/`METAL_*` env, nothing in entitlements
or CMake), hardware class (M5 Pro, 15 cores), and GPU saturation (0%).

Two candidate waits were checked and are **not** the dominant one:
`TextureCache::waitForGPUUploads` (3 and 6 samples in the early/late captures)
and `checkRDRAM`'s `RenderWorkerExecution` (86 → 77 samples, i.e. flat, and
falling as a share while other costs grow).

## Finding 4: fn64 does not compile RT64's `rt64_state.cpp` at all

This is the fact a future investigator most needs and it is not obvious.

`crates/fn64-render-rt64/ffi/CMakeLists.txt:872-910` reads
`${FN64_RT64_SOURCE_DIR}/src/hle/rt64_state.cpp`, asserts its SHA256 against a
pinned literal held at `ffi/CMakeLists.txt:876` (read the value there; the
build is the only thing that should assert it), string-replaces two exact
contexts to inject
`fn64_rt64_vi_retrace_event(...)` into the screen-update condition, writes the
result to `${CMAKE_CURRENT_BINARY_DIR}/fn64_rt64_state.cpp`, marks the
**original** `HEADER_FILE_ONLY TRUE`, and compiles only the generated copy.
`rt64_video_interface.cpp` gets the same treatment at `:820-867`
(`fn64_rt64_vi_filter_constants`), and `rt64_interpreter.cpp` at `:919+`.

Consequences, both learned the hard way:

1. **Editing `rt64_state.cpp` in the RT64 checkout has no effect on the
   build** — the raw file is excluded from compilation.
2. **It also breaks the build**, because the SHA256 pin fires first:
   `CMake Error ... RT64 State changed; review the fn64 VI retrace overlay`.

Temporary instrumentation of these files must therefore be added *as another
string-replace stanza in fn64's own CMakeLists*, with the pinned hash updated
in the same commit. That is a change to an integrity guard on a third-party
dependency and deserves its own review; it was **not** attempted here.

Also note `crates/fn64-render-rt64/build.rs` declares no
`cargo:rerun-if-changed` for the RT64 tree, so cargo will not notice upstream
source changes. Forcing a genuine RT64 rebuild takes
`cargo clean -p fn64-render-rt64 --release` — deleting `out/librt64.a` and the
`rt64-cmake-build/` directory by hand is **not** sufficient (a build that
appears to succeed in ~3 s with an unchanged binary size has silently relinked
a cached archive).

## Finding 5: the +3320 wait is real but small; it is not the busy-phase cost

Answered Finding 3's open question directly, with live numbers instead of a
sampling profiler's offset. Extended `ffi/CMakeLists.txt`'s existing
hash-pinned patch mechanism (Finding 4) with a new, explicitly TEMPORARY,
opt-in stanza gated on `-DFN64_RT64_WAIT_TRACE=ON` (wired from `build.rs` via
the `FN64_RT64_WAIT_TRACE` env var, same pattern as the existing
`CARGO_FEATURE_*` gates). It times exactly
`renderAndSynchronize`'s `commandList->end()` / `execute()` / `wait()` triple
at `rt64_state.cpp:1443-1447` (the branch that actually runs whenever there
are framebuffers to draw — the sibling `else` branch at `:1544-1558` is the
no-render tile-only path and is not live during normal play) and prints a
running average every 200 calls.

Live numbers, same route, same windowed process:

| phase | calls | wait total | avg wait/call |
|---|---:|---:|---:|
| light (menu), calls 0-200 | 200 | 84.35 ms | 0.4217 ms |
| busy (match), calls 1600-1800 | 200 | 71.47 ms | 0.3848 ms |

**The average is flat, and if anything slightly lower under load.** Call
*frequency* grows with the game's own submission rate (Finding 2's ~3x), so
the wait's **estimated per-field contribution** grows too — roughly 0.07
ms/field early, roughly 0.43 ms/field busy — but that is 2-3% of the ~15
ms/field step-up `pump_ms` shows (12 ms → 27 ms plateau), not the cause of
it. The `sample` finding that put 75% of `fullSync`'s time in this wait
(Finding 3) does not survive contact with a direct timer; treat it as an
artifact of `sample`'s ~1 ms bucket granularity colliding with a lot of very
short adjacent frames, not as a real attribution. **This closes the
question Finding 3 opened, in the negative:** this wait is not where the
busy-phase cost lives.

The instrumentation stanza is left in place (inert, `OFF` by default) rather
than deleted, since it is now proven mechanically sound (matched both
anchors on the first real build, zero `FATAL_ERROR`s) and answering the next
candidate wait needs the identical apparatus. Re-enable with
`FN64_RT64_WAIT_TRACE=1`, and `cargo clean -p fn64-render-rt64 --release`
first per Finding 4's caution about stale cached archives.

## Finding 6: dispatch_lle_task's RSP-interpretation guess was wrong. RDP rasterization is the real cost, at 27.5 ms/field, measured directly.

Finding 5's redirect proposed timing `dispatch_lle_task` (RSP interpretation)
directly, since a `sample` capture showed its *symbol* share roughly doubling
between light and busy windows. That instrumentation already exists and
didn't need building: `PHASE_TIMING`
(`crates/fn64-abi/src/task_dispatch/lifecycle.rs:61`, armed by
`FN64_PHASE_TIMING`, which `--profile` arms automatically) already wraps
`dispatch_lle_task` in `std::time::Instant` and reports real wall-clock
ms/field for both `RSP interpretation` and `RDP rasterization` as siblings
under `gfx_ns`, split by a fast/slow field-population census
(`FN64_FRAME_CENSUS_POPULATIONS`, also auto-armed).

Ran the same sanctioned command the file's own header prescribes,
`./reference/wm2000-routes/render-benchmark.zsh --profile`, on the same
1.5M-step route (byte-identical: 8/8 counters matched). Real numbers, split
by population:

| population | fields | mean ms/field | RSP interpretation | RDP rasterization |
|---|---:|---:|---:|---:|
| fast (menu/idle) | 3846 | 5.826 | 0.000 ms/field (0.0%) | 0.000 ms/field (0.0%) |
| slow (match/busy) | 3853 | 48.337 | 3.881 ms/field (12.3% of gfx LLE, 0.23x budget) | **27.523 ms/field (87.4% of gfx LLE, 1.65x budget)** |

**RSP interpretation is not the cost.** It is a real, present cost in the
slow population (3.881 ms/field) but a minor one relative to the whole
budget. **RDP rasterization is** — at 27.523 ms/field it is, by itself,
1.65x the entire 16.667 ms/field 60fps budget, and roughly 7x larger than
RSP interpretation. The `sample` symbol-share doubling that motivated
Finding 5's redirect was measuring the wrong denominator (symbol-frequency
share, not wall-clock ms/field) — the same class of error `perf-method`
warns about, now caught by direct measurement rather than repeated.

This is not a new finding out of nowhere: `docs/plans/rt64-on-the-block-lane.md:25`
(2026-08-07, this file's own opening table) already named "RSP graphics LLE –
raw RDP rasterization" as the single largest line in the whole system, at
11.00 ms/field on that day's route/build. Finding 6 confirms the same
component is still the dominant cost under the current windowed RT64 lane and
gives its current, larger, population-split number. The `f46ef27`/`10a2d68`
commits this session (raw-RDP row-edge precomputation, scratch-buffer reuse)
already targeted this exact code path and were real, measured wins
(32.764→26.571 ms/field on the reference-backend suite) — this is
confirmation to keep cutting the same line, not a new direction.

The staging-copy breakdown under RDP rasterization is small in comparison
and already known: `staging alloc` 0.110 ms/field, `staging copy_in` 0.392
ms/field, `staging copy_back` 1.391 ms/field — together under 7% of the
27.523 ms/field RDP total. The remaining ~25.6 ms/field is rasterization work
itself, not copy overhead.

## What this redirects to

1. **RDP rasterization is the dominant remaining cost, measured directly
   at 27.523 ms/field in the busy population (Finding 6), not RSP
   interpretation.** The next concrete step is the same kind of targeted,
   measured cut `f46ef27`/`10a2d68` already made in this area this session —
   profile *inside* `draw_raw_rdp_triangle_impl`/`raw_pixel_coverage*` and
   `CoverageMask` construction with `sample` while a busy-population field is
   in flight, to find what's actually consuming the 25+ ms, rather than
   guessing from the coarse RDP-vs-RSP split alone.
2. **Do not chase audio.** Finding 1 settles it.
3. **Do not chase a leak or a backlog.** Finding 2 settles both.
4. **Do not chase RT64's GPU-side wait.** Finding 5 settles it — flat
   ~0.4 ms/call, 2-3% of the step-up.
5. **`DisplayBuffering::Triple` was tried and measured flat** (49.6-50.1%
   starvation, tail latency unchanged); fn64 defaults to `Double` at
   `settings.rs:249`, matching RT64 upstream. `PresentMode::Mailbox` is
   unsupported by this Metal surface (`Fifo`/`Immediate` only) and panics.
   Both are dead ends already paid for.
