# fn64 measurement method — cases and closed lines

Companion to `SKILL.md`, which carries the rules. This file carries the
evidence: the runs behind each rule, and the lines already closed by
measurement. Read it when a rule surprises you, when you are about to lean on
one hard, or when you are tempted to re-propose something below.

Full case detail beyond this — every run, retraction and code site — is in
`docs/plans/perf-method.md`.

## Closed lines — do not re-propose without new evidence

Each was closed by measurement, not opinion.

**Every entry states its DENOMINATOR.** Audited 2026-08-08 after one entry was
found closing far more territory than it measured: *"RSP micro-optimization"*
read as *graphics is closed* while covering 17.6% of graphics and never touching
the 82% that is RDP. **An entry whose scope is narrower than its title silently
closes ground it never examined, and the whole function of this list is that
nobody re-derives its entries.** The other six were checked and are correctly
scoped — each names the specific mechanism it ruled out, not a category.

- **async RSP dispatch (ceiling 0 ms, not 13.4)** — the deadline is computed
  *from* the work: `lle.steps` does not exist until interpretation finishes, so
  the scheduler must block on the worker. The off-field is not host idle either
  — the harness *jumps* the virtual clock, so there is no slack to donate.
- **HLE graphics for THIS title's microcode** — 16,586/16,586 submits rejected
  with `NeedsLle`; 100% XBUS (`dram_dpc=0`) and 3.66 IMEM overlay swaps per
  task, so no display list exists in RDRAM to decode and no 4 KiB digest can
  identify the ucode. Admitting F3DZEX2 is a clean-room RE project, not config.
- **`FN64_FAST_MUTATION_JOURNAL` (this flag only, barrier ON in both lanes)** —
  −0.14 ms, sd 0.35, three interleaved pairs, deltas of both signs. The barrier
  absorbs it; it can only gate the site that was already cheap. *Not* a
  statement about the barrier, which is a separate A/B — conflating the two
  produced a retracted claim.
- **instruction budgeting** — changes the emulated program
- **reducing the dispatch COUNT (167.8/render field; ceiling 6.60 ms, and
  unreachable)** — the count is guest-determined: 48.3 dispatches and 15.8
  `BlockExit::HostCall` exits per SP graphics task, while `audio_lle_calls` is
  identical (0.925 vs 0.917) across populations. **The phase worth 81% of the
  bucket does not scale with the dispatch counter at all:** `exec_mirror_calls`
  == `executor_calls` (3.972x), not `resume_dispatch_calls` (5.313x), and its
  per-call cost *falls* 58.49 -> 32.32 us as the count rises — elasticity 0.570,
  because the reconcile's work is proportional to changed bytes, not to calls.
  The store-forced boundary this hunt was chasing was already removed
  2026-08-06 (`ExecutableWrite` 99.8% -> 0). Per-dispatch machinery is 2.332 ms
  = 9.1% of resume NET; `gfx_ns` is 11.626 ms = 45.6%.
- **RSP threading** — thread-local state
- **RSP micro-optimization (17.6% of graphics — NOT the renderer)** — uniform
  11.25 ns/instruction, no defect. The interpreter is **large, not slow**:
  526,161 instructions per render field at ~39 cycles each is normal.
  **Scope, added 2026-08-08 because this entry read far broader than it was
  measured:** RSP is **5.64 ms/field, 17.6% of graphics**; the RDP is
  **26.40 ms, 82.3%** and was never examined by that work. Do not read this
  line as "graphics is closed". *A closed line is dangerous in proportion to
  how broadly its title reads versus how narrowly it was measured — state the
  denominator in the title.*
- **depth-buffer copy elimination** — measured +0.84%, kept for correctness
  only (rule 12)
- **narrowing the DPC copyback (0.413 ms/call = ~1.9% of the 22.6 ms gap)** —
  measured 2026-08-09 on the post-mirror-fix binary. The renderer changes only
  **0.5138%** of the 8 MiB image, and the line still dies twice over: the scan
  to find those bytes costs **1925.3 us/call against the 412.6 us/call copy it
  would avoid (4.67x)**, and the changes are **2,890 runs/call at 15 bytes
  each** — a narrowed copyback is thousands of short memcpys replacing one
  streaming one. **The strongest rule-12 case yet: 99.49% provably dead bytes,
  eliminating them is still the wrong direction.** Do not re-derive from the
  byte count.

## The reference-lane correction, in full

The largest single correction on record (2026-08-09). It is summarised in
`SKILL.md`; the numbers are here.

Every figure in the pre-correction table was a **software-rasterizer** number:
`render-benchmark.zsh` does not export `FN64_RENDER` and `main.rs` defaults to
`"reference"`. The owner runs **`FN64_RENDER=rt64`**. Re-measured there, same
route, same commit, **two unprofiled reps agreeing to 0.17%, both guest
byte-identical**:

| | `reference` | **`rt64` (what ships)** |
|---|---:|---:|
| per-field mean | 27.96 ms | **17.25 ms** |
| drawn frame (x2) | 55.9 ms | **34.49 ms** |
| fps | 17.9 | **29.0** |
| **gap to 33.33** | **22.6 ms** | **1.16 ms** |

**The gap is 20x smaller than the reference lane says, and WM2000 is at 29.0
fps against a 30 fps target.** An agent was dispatched to find 22.6 ms in
graphics; the honest answer was that the deficit is 3.5% of budget on the lane
the owner runs. Post-fix graphics is **53.9%** of the render field, not 75.6%.
Full result and the post-mirror-fix decomposition: `rt64-on-the-block-lane.md`.

This trap has now cost two separate investigations — the first produced a
"graphics is 70% of the field" decomposition of a lane nobody runs, and the
second happened *to an agent that had read the dead-ends list warning about the
first*.

### The unprofiled-mean table it replaced

Kept because the error is the instructive part. Measured on the shipping
binary, same route, 7,699 steady fields, `reference` lane:

| | before the mirror fix | after (`8109435`) |
|---|---:|---:|
| per-field p50 | 31.13 ms | **23.98 ms** |
| per-field p95 | 64.65 ms | 54.56 ms |
| **per-field mean** | **34.89 ms** | **27.96 ms** |
| **drawn frame (x2)** | 69.8 ms — **14.3 fps** | **55.9 ms — 17.9 fps** |
| 30fps budget | 33.33 ms | **gap: 22.6 ms** |

**Two figures quoted for hours were wrong, both optimistic, both from the same
mistake.** "45.32 ms pair / 22 fps / 11.99 ms gap" was built from **profiled
per-population p50s** (8.858 + 36.462). Profiling inflates the absolute cost,
and on a bimodal distribution **p50 is not the mean** — the mean carries the
tail that actually consumes wall time.

## Cases behind the rules

### Pre-registered tolerances caught three defects in one hour (rule 31)

Buckets must sum to their parent, the residual prints unconditionally, and the
report refuses to present a split that does not close. On three consecutive
smoke runs this caught: a walking clock lapping **across a coroutine suspend**
(−697%; `resolve` read 197 ms/field inside a 40 ms field); the **partial fix**
that covered one suspend site of many (−533%); and a **mislabelled bucket**
where `gfx_ns` (21.5) exceeded its own parent (7.7). **None was visible in the
phase values — all three looked like findings.**

Corollaries: *a wall clock cannot time a stack that yields* (correct at the one
chokepoint every yield passes, never at the N call sites — N was 16 and no
enumeration is complete); *always report a nested counter against its containing
bucket*, because **a child exceeding its parent** was the only signal that fired
on the third; and **a label that is 95% something else is a wrong measurement
wearing a plausible name.**

### Probes that could never have answered (rule 6c)

Three attempts to verify one claim, all returning clean zeros, all meaningless —
and the speed and agreement made them convincing:

| probe | result | why it could never have answered |
|---|---|---|
| `strings \| grep commit_with_optional_view` | 0 in both lanes | the function **inlines at O3**; the name is in neither file |
| `nm -U \| grep -c set_expected` | 1 in both | `nm` lists **symbols, not call sites** — one symbol called twice counts once |
| `strings \| grep <catalog digest>` | absent in both | the digest is **computed at runtime**, never a literal |

Each asked *the file* what only *the running program* or *the disassembly* can
answer. Three fast agreeing answers are more dangerous than one slow ambiguous
one, and this happened while checking a subagent's work, not while doing the
work.

### A dead run read as a live one

A run reported as "a quarter through, ~75 minutes left" had in fact been dead
for 29 minutes — read from a log mtime. This route writes only on controller
edges and heartbeats, so a quiet log is normal. Related: `pgrep -f
"release/wm2000-block-boot"` returns 2 with **zero** benchmarks running because
it matches your own monitoring shells — use `pgrep -x`.

**And when a run dies, "cause unknown" beats a mechanism that fits.** The agent
whose run was killed inferred a harness reap: plausible, self-consistent, and
wrong — **the coordinator had restarted a run he did not own**, three drivers
collided, and killing the duplicates took the original down with them. There
was no direct evidence of a reap, only a dead run and an exited waiter, and a
mechanism was built to fit. *A mechanism that explains the evidence is not
thereby the cause.*

The coordination rule this violated: three agents were told to route machine
access through a single coordinator so concurrent restarts could not happen;
the coordinator then restarted an agent's run without checking whether it was
already restarting. **Cost: ~40 minutes and two dead A/Bs.**

### `pgrep -f` under-matches, which is the worse direction (rule 18)

`pgrep -f "cargo build --release --bin wm2000-block-boot"` returned nothing
while **15 `rustc` processes were running** — cargo's real argv is not the
command line you typed, and the compile work lives in `rustc` children whose
argv never contains your string. An over-match reads as "still busy" and costs
you a wait; an **under-match reads as "finished" and is acted on.** Mine fired a
"BUILD PROCESS EXITED" that was false, seconds before I would have called a
mid-flight build a failure. *Absence of a match is not evidence of absence*
(rules 19, 23).

### A key that appears twice is two metrics

A byte-identity checker did `findall(...)[-1]` on `gfx_submits=` and silently
compared the **steady-state span** count (`[frame-census]`, line 175) against a
**whole-run** expectation (`[wm2000-block-progress]`, line 63). The 303
difference was exactly the `warmup_gfx=300` fields the census excludes. It
reported a guest byte-identity failure that did not exist, and drove three
25-minute attribution runs.

Its author had proven the checker could detect a wrong *value* — rule 6a done
halfway. Proving a check can fail is not proving it reads the right thing.
**And four runs agreeing was not evidence: consistency across runs is not
validity when every run goes through the same broken instrument.**

### Narrow checks sharing a blind spot (rule 23)

Three individually-true probes concluded a commit was not on the branch: `git
log --oneline -3` (it was 15 back), a grep of `host.rs` for an env-var string
(the file imports the symbols, never names the gate), and a line number taken
from a diff hunk header rather than the current file. Each answered the question
literally asked; none answered the question meant. Ancestry is `git merge-base
--is-ancestor`, not a log window. Same family: one `runner.rs` grepped and 0
found, when 136 of 142 carried the detector.

### Both lanes were the control lane (rule 6b)

One line — `FN64_X=${FN64_X:-<unset>}` printed by the lane runner itself —
caught an A/B 90 seconds in where **both lanes were the control lane.** A zsh
`set -- $spec` did not word-split, so the lane name arrived as `"armed 1"`,
matched neither branch, and fell through to the else that *unsets* the gate.
Four runs, a clean-looking A/B, a perturbation of ~0.000, and a believable
fabricated result.

### A wait is a workload (rule 30)

Symptom: *load stays high with zero builders and zero benchmarks* — `ps -Ao
comm | grep -c sleep` returned **19**, stacked `until…sleep…done` waiters (some
nested on other waiters), holding load above the route script's own
`--max-load 3.0` guard. Self-obscuring: the load was being read to decide
whether to start, and the reading moved the number — a control loop with the
wrong sign.

### A live log is not a population (rule 29)

Two agents analyzing one live log got n = 117…183 — a growth curve — and the
differing counts were read as a wrong-key extraction bug (plausible: that log
carries `frame_interval_ms[` and `pump_ms[` one space apart), producing a
confident correction of a *correct* extraction.

Two corollaries earned in the same hour: a gap called "clean" must be compared
against the other gaps in its own series (the "clean" one was 6.5 ms, largest
of seven >2 ms, and the distribution was trimodal not bimodal); and a series of
**windowed p50s cannot distinguish "the game got slow" from "the emulator has
regimes"** — only raw per-field data can (`FN64_FRAME_CENSUS_SEQUENCE`).

### The sum was never computed (rule 32)

Having decomposed a 45.7 ms parent, I wrote that the non-graphics rows "sum to
well under the budget" and concluded the answer survived the renderer question.
Every individual number was right; **the sum was never computed.** 9.53 + 8.85 +
2.31 + … = **21.55 ms = 1.29x a 16.667 ms budget** — so an infinitely fast
renderer still misses 60fps, the opposite conclusion. Corollary: when you catch
this, the "X is the bottleneck" framing usually becomes "both halves must fall",
which is a materially different plan.

### A unit error that read as jitter (rule 21)

`AudioOutputStats::samples` counts i16 **channel** samples, not frames. Read as
frames it gave "91.5% of real time"; the true figure was **45.7%**. Not a
rounding apart — 91.5% sounds like jitter, 45.7% is the emulator running at half
speed. Hours were spent on the wrong diagnosis.

### A healthy delivery path is not a healthy stream (rule 22)

`backend_buffers == ai_buffers` held exactly — zero drops — while the device
played gaps. **Equal counts prove nothing is being LOST; they say nothing about
whether enough is ARRIVING.** For any producer/consumer seam the health metric
is *delivered ÷ wall-clock seconds against the rate the consumer demands*. Queue
depths and drop counts are downstream of it and all read clean under starvation.

### A source edit is a build

A shared checkout means *file* contention, not just CPU contention. An agent
wrote six files over ~90 seconds while a peer's 32-crate build was reading the
tree; the build died 14 minutes in on `telemetry.rs` consuming a `PhaseTiming`
field `lifecycle.rs` had not yet declared. **Editing in dependency order would
not have helped: no ordering makes a multi-file change atomic to a concurrent
reader.**

Corollary for the reader: a build that fails on a file you never opened is
someone else's in-flight edit, not your bug. Check `git status` mtimes before
debugging it — and when a peer's uncommitted work is linked into *your* binary,
name it as the first suspect if a result looks wrong.

### The day that earned the sequencing rules (2026-08-08)

Four separate measurements each discovered their instrument was broken *after*
arming it, at hour three, hour six, hour nine and hour twelve. Three targets
were named and briefed; all three were wrong when measured — the apparatus (5.1%
of the field), then "translated guest code" (20.9%, briefed as the bulk of 83%),
then RDP (79% of it an artifact of a renderer the owner does not use). Three
25-minute runs were dispatched to answer a question two already-finished logs
contained. Three agent findings reached the owner as fact and all three were
wrong: a VI-attribution claim, a mirror regression that was a single noisy rep,
and a byte-identity deviation that was a parser bug. Three agents sharing one
worktree cost ~2 hours of holding plus a 14-minute build destroyed by a file
collision the coordinator authorized.

**What worked, and should be default:** pre-registering thresholds, predictions
and falsifiers *before* data (caught three real defects; three agents retracted
their own findings unprompted); enforcing closure tolerances **in code** rather
than in intent; and a **one-minute smoke run** before any 25-minute one — one
smoke caught two defects that would each have cost a full run.

## The environment

`.claude/local.env` is **canonical** — source it, never reconstruct. It defines
`FN64_DISCOVER_NWXE_ROM`, **not** a bare `ROM`; the shard build wants `ROM` and
panics with `ROM must name the user's NWXE image: NotPresent` without it. Builds
also need `FN64_EXECUTABLE_IMAGES` (three capture paths) and `FN64_BOOT_CONTEXT`
— see `reference/wm2000-routes/render-benchmark.zsh`.

RT64 lives at `~/Code/no-mercy-recompiled/third_party/rt64` (MIT; `build.rs`
asserts the LICENSE). The `lib/rt64` copies in jessetbh repos are UNLICENSED —
do not use them. Build from inside `examples/wm2000-block-boot`.

Do not edit `examples/wm2000-block-boot/src/main.rs` — it is hashed into
`DISPATCH_SOURCE_SHA256`.

## Guest byte-identity gate

Any change on the emulation path must reproduce these exactly, or it changed the
emulated program and does not ship:

On the **1.5M-step render-benchmark route** (the one `--profile` uses), verify
with `scripts/check-byte-identity.py` against `scripts/byte-identity-1p5M.txt`.
**Do not write your own extraction** — one did `findall(...)[-1]` and
manufactured a phantom failure that cost 75 minutes:

```
gfx_submits=11153  audio_submits=7685  sp_tasks=18838
vi_interrupts=8386  controller_ops=2390  sim_time=13112786076
render_error=None
```

On the older/deeper route:

```
gfx_submits=16586  audio_submits=11005  sp_tasks=27591
vi_interrupts=12008  controller_ops=3115  sim_time=18776001537
render_error=None
```
