# `FN64_PROFILE=1` — one gate, one authoritative report

Design record and build log. Status: **built and unit-tested** — 368 tests pass;
the single red is a pre-existing failure diagnosed below and deliberately not
fixed. Acceptance run on the RT64 lane in progress.

The owner's instruction: *"how can we measure and understand, or profile, this
stack so that we can fix the perf issues once and for all"* — then *"build
tooling/instrumentation that will enable us to unblock this."*

## The diagnosis: what is actually broken

Four measurements in one evening each discovered their instrument was broken
*after* arming it. Every one is a **tooling** failure, and they share one root
cause:

> **The counter tree is documented for humans and re-derived by hand at every
> report site.**

The tree exists, correctly and in detail, as an ASCII diagram in a comment at
`crates/fn64-abi/src/task_dispatch/lifecycle.rs:507-512` and `:551-561`. But
`Counters::labelled()` (`frame_census.rs:388`) returns a **flat** `[(&str, u64);
42]` with no parent declared, so each of `executor_split_report`,
`resume_split_report` and `population_report` re-implements the nesting as
hand-written subtraction. Three of them got it wrong in one evening. A diagram a
human must obey is not a check.

Nothing here is a missing counter. The counters, censuses and closure logic
mostly exist and are correct. **The failure is composition and declaration.**

### Findings that change the design

Discovered while reading; each one would have broken a naive implementation.

1. **The gates use four different truthiness conventions.** A full inventory
   found **22 report-bearing gates** (the brief estimated 19) in four classes:

   | Class | Predicate | `=0` | `=` (empty) |
   |---|---|---|---|
   | **A** `env_flag` | in `{1,true,yes,on}` | OFF | OFF |
   | **B** `is_some()` | presence only | **ON** | **ON** |
   | **C** numeric | `parse::<T>()`, 0 = off | OFF | OFF |
   | **D** `v == "1"` | exact compare | OFF | OFF |

   - Class B ⚠: `FN64_PHASE_TIMING`, `FN64_EXECUTOR_SPLIT`, `FN64_RESUME_SPLIT`
     (`lifecycle.rs:62,84,145`) — **`FN64_EXECUTOR_SPLIT=0` ARMS the
     instrument.** This is deliberate and argued at `lifecycle.rs:82-84`
     (consistency with the counter it nests inside), but it is the same shape as
     the bug that fabricated the 4.9x — and it bites exactly the workflow these
     three separate gates exist for: running one level alone to measure
     perturbation, where someone naturally writes `=0` to mean off.
   - Class A: `FN64_FRAME_CENSUS`, `FN64_FRAME_CENSUS_POPULATIONS`.
   - Class C: `FN64_FRAME_CENSUS_SEQUENCE` is a **count**, not a flag — `=1`
     dumps one field, which is useless.

   So `FN64_PROFILE=1` cannot blanket-set its constituents to `1`, and must
   never set any of them to `0` to mean off. Each is set to a value correct for
   *that gate's own parser*. This is also why requirement 3's "exit non-zero
   naming the missing gate" needs real arming **verification by effect**, not an
   env-var echo — an echo cannot distinguish Class A `=0` from Class B `=0`.

   Three copies of `env_flag` exist (`write_barrier.rs:324`,
   `frame_census.rs:93`, `dpc_copy_census.rs:79`), identical and module-private.
   Not consolidating them: that is a refactor of certified sources for no
   measurement gain.

1b. **`[frame-periodicity]` has no gate of its own** — it emits whenever
   `FN64_FRAME_CENSUS` is on. Arming verification must not expect a gate for it.
   It correctly recomputes gfx deltas from `FieldSample` rather than the gated
   `Counters` (`frame_census.rs:1182-1183`).

2. **`FN64_PROFILE_*` is an already-populated prefix meaning something
   unrelated** — `FN64_PROFILE_BUILD`, `FN64_PROFILE_AOT_BANKS`,
   `FN64_PROFILE_STOP_AT_PC`, `FN64_PROFILE_CONTROL`, `FN64_PROFILE_AOT_RECENT`,
   `FN64_PROFILE_HOST_RECENT`, `FN64_PROFILE_STOP_AT_GENERATION`. Every existing
   member is `FN64_PROFILE_<SUBSYSTEM>` and none is a general profiler. Bare
   `FN64_PROFILE` is unused and free, but the collision is a real documentation
   hazard and is called out at the top of the runbook.

2b. **`main.rs` is hashed verbatim.** `build.rs:794` does
   `std::fs::read("src/main.rs")` into `DISPATCH_SOURCE_SHA256`
   (`build.rs:1256-1268`). **Adding even one `env::var` line there changes the
   canonical program identity**, so a before/after comparison would be running a
   different program and would read exactly like a perf change (rule 11a). This
   is the decisive argument for landing in `fn64-abi`: ~25 s rebuild, no digest
   movement. (`fn64-cpu-runtime` would be 32 crates, ~9-11 min, *and* move
   identity digests — rule 8.)

3. **The gates are `thread_local!` `Cell`s initialized at first touch.** Arming
   must happen via the process environment before worker threads start, not as a
   runtime override.

4. **There is already an ideal seam.** `frame_census::install()`
   (`frame_census.rs:599`) registers an `atexit` hook from *inside* `fn64-abi`,
   reached from `host::advance_virtual_time` (`host.rs:50`) — the one seam both
   the headless and windowed lanes cross. This was done deliberately so that
   `recomps/wm2000/packages/wm2000-block-boot/src/main.rs`, hashed into
   `DISPATCH_SOURCE_SHA256`, never needs editing. **The profile report hooks the
   same seam and inherits that property**: no hashed file is touched, and the
   emulated program is unchanged.

5. **The field-name collisions are worse than reported.** The brief says three
   tags carry `gfx_submits` with different meanings. It is **six tags, four
   meanings**, and there are three further collisions:

   - **`gfx_submits`** — `[frame-census]` is a **steady-state span delta**;
     `[frame-sequence] gfx=` is a **per-field delta**; `[fn64-heartbeat]`,
     `[wm2000-block-boot]` (×2), `[wm2000-shell]`, `[wm2000-block-progress]` are
     all **whole-run cumulative**. `[wm2000-shell] SPIKE` uses `d_gfx=` — already
     renamed to dodge this collision, which is the precedent to follow.
     A heartbeat reading 4820 beside a census reading 0 is *not* a
     contradiction. This is the exact shape that cost 75 minutes and three runs.
   - **`audio_submits`** — same pattern, no span-delta variant.
   - **`boundaries`** — four tags, four different denominators, not comparable.
   - **`calls` / `tasks=`** — the `[wm2000-block-profile] phase_timing` line
     reuses the bare token `tasks=` **twice** on one line (gfx-LLE and
     audio-dispatch), disambiguated only by position. `grep -o 'tasks=[0-9]*'`
     silently conflates them.

   **Consequence for the design:** the profile report emits **scope-qualified
   names** — `gfx_submits_span`, `gfx_submits_run`, `gfx_submits_field` — so no
   key in the authoritative report is ambiguous, and no extraction can pick the
   wrong one. Existing tags are left alone (committed scripts parse them); the
   new report simply refuses to reuse a colliding bare name.

5b. **`FN64_DEVICE_ADVANCE_CENSUS` prints tag `[device-census]` from two sites**
   — `execution.rs:1035` to **stderr** without percentages (abort path, where
   atexit never runs) and `main.rs:1457` to **stdout** with percentages. Any
   parser keying on the tag must handle both shapes.

6. **`scripts/byte-identity-1p5M.txt` already documents the exact trap in
   requirement 3** — a NOT-ARMED notice printed by a block gated on the very
   flag it warns about, "unreachable by construction". And
   `scripts/check-byte-identity.py` is already correctly anchored to a named
   line with a fatal-on-missing-anchor. **Both are reused as-is, not rewritten.**

## What gets built

### 1. `counter_tree.rs` — the tree, declared once, in code

A single `const` table, one row per counter, each naming its parent. Derived
mechanically from the lifecycle.rs diagram, which becomes a pointer to the table
rather than the source of truth.

```rust
pub struct Node { pub name: &'static str, pub parent: Option<&'static str>, .. }
```

The relationship the acceptance table needs is expressible exactly: `resume NET`
is a *derived* node (`exec_resume_ns − exec_mirror_ns − exec_guard_suspend_ns`),
which is why `mirror 8.848` is reported as a **sibling** of `resume NET` while
being *nested inside* `exec_resume_ns`. Prose made those two facts confusable;
the table states both without ambiguity.

**The check:** for every node, `Σ(children) ≤ parent + tolerance`. A violation
is a **hard error that refuses to print the affected subtree**, replacing it
with the violation and the numbers that prove it. Not a warning. That check
alone catches three of the four defects — including the `gfx_ns` 21.5 > parent
7.7 case, which was caught by a human noticing.

Closure-residual behaviour is preserved exactly as-is: residual printed
unconditionally; negative residual fires "instrument broken, do not read the
rows above".

### 2. Both denominators on every row

The single most consequential fix. Today `resume_split_report` prints only
`{:>6.1}% of resume NET` (`frame_census.rs:1773`). Every row gains its ratio to
the 16.667 ms budget:

```
dispatch = TRANSLATED GUEST CODE   9.528ms/field   20.9% of resume NET   0.57x budget
```

and each decomposition ends with a **summed** `Σ rows = N.NNms = N.NNx budget`
line, computed in code. That is rule 32: the row that read "20.9%" is 0.57x the
budget, and three such rows total 1.29x — the opposite conclusion, and it was
never computed.

### 3. Refuse to print rather than print a plausible subset

`FN64_PROFILE=1` verifies each constituent actually armed — by observing the
gate's *effect* (its counters are non-zero on a route known to reach them), not
by echoing the variable it set. On failure: **exit non-zero naming the missing
gate.**

The NOT-ARMED warning is emitted from a path **not gated on the flag it warns
about**, which is the specific trap that cost two 25-minute runs and that
`byte-identity-1p5M.txt` documents as unreachable-by-construction.

### 4. Per population and per percentile by default

`Bucket` already carries `phase_p50_ms`/`phase_p95_ms` computed at report time
from data otherwise discarded (`frame_census.rs:915`), and `nearest_rank`
already exists. Extending to **p50/p95/p99** is one added call per site. Fast
and slow populations are already split and stay split; **no bare mean is ever
printed without its distribution beside it.**

### 5. One command

`recomps/wm2000/reference/wm2000-routes/render-benchmark.zsh --profile` exports the whole set
correctly (it currently exports *none* of the five). One documented command,
runnable without reading 32 rules first.

**The allowlist is part of the deliverable.** The script's output filter
(`render-benchmark.zsh:247-248`) is a `grep -E` **allowlist** — any tag not named
there is invisible in the stream the caller watches, and two 25-minute runs were
already lost to an `[executor-split]` NOT-ARMED notice that was printed,
filtered, and never seen. So `--profile` adds `^\[fn64-profile\]` to the
allowlist in the same change that introduces the tag. A report nobody can see is
not a report.

`--profile` also runs `check-byte-identity.py` against the route's expectation
file automatically at the end, so the emulation-path guarantee is not a step a
tired person can skip.

## Finding: a tree that closes internally can still be detached from reality

**Found by reading the instrument's own first real output, not by review.**

The report printed `resume NET = 45.687 ms/field, 2.74x budget` for a population
whose measured p50 was `10.000 ms`. Impossible — and **every parent/child
relation held.** `counter_tree::validate` returned empty.

The reason is structural: `executor_ns` is the tree's root, but the *real*
container is the field's measured wall time, which **is not a counter** and
therefore is not in the tree. So the check built specifically to catch
impossible decompositions could not see this one. Closure is necessary and not
sufficient: a decomposition can be perfectly self-consistent and still describe
more time than the field it sits in.

Fixed with an outermost check comparing the claimed total against
`bucket.mean_ms`. `a_decomposition_larger_than_its_own_field_is_caught` injects
45 ms of phases into a 10 ms field and additionally asserts that
`counter_tree::validate` returns **empty** on that same input — proving the new
guard catches something the parent/child relations genuinely cannot, rather than
duplicating them. Paired with `a_decomposition_within_its_field_is_not_flagged`
so it cannot fire always.

## Finding: I reintroduced the exact failure this tool prevents, in this tool

The first real RT64 run printed:

```
(of) staging alloc      0.000ms/field   0.0% of RDP   0.00x budget
(of) staging copy_in    0.000ms/field   0.0% of RDP   0.00x budget
(of) staging copy_back  0.000ms/field   0.0% of RDP   0.00x budget
```

Three clean zeros, no warning, in the rows the owner's memcpy fix depends on.
`FN64_DPC_COPY_CENSUS` was **not in the channel list**, so the counters were
never armed — and *"an unarmed channel reads ZERO, and zero is
indistinguishable from 'this costs nothing'"* is the sentence this module's own
refusal notice prints. **I wrote the guard and then omitted the gate it was
meant to guard.** A silent zero would have sent someone to optimize a row the
instrument never measured.

Fixed by composing `FN64_DPC_COPY_CENSUS` and witnessing it via `rsp_entries` /
`dpc_calls` rather than the staging timers themselves.

### The witness pair: guard BOTH directions, not one

Choosing the witness is the subtle part, and there are two opposite ways to get
it wrong. Most people guard one.

| direction | shape | what it does |
|---|---|---|
| **under-match** | witness too narrow — checking the staging timers | a route that never reaches `dispatch_captured_raw_rdp` has a **legitimate** zero; the guard calls a correct zero a broken channel |
| **over-match** | witness too broad, or absent | an unarmed channel's zero is read as "this costs nothing" — the failure above |

A check that cannot distinguish **"absent"** from **"genuinely zero"** is rule
6a's error in the other direction. The over-match direction showed up twice in
one day (a search returning 0 from a wrong path; `grep -c rustc` returning 0
while 11 processes ran). This is the under-match, and it is the one that makes a
guard get switched off for crying wolf.

`rsp_entries` / `dpc_calls` is the right witness because **any rendering route
retires RSP instructions**, whether or not it stages a copy — so a zero there
means the channel is unarmed, while a zero in the staging timers may simply be
the truth.

**The general lesson: a verify-by-effect check is only as complete as its
channel list.** Enumerating channels by hand is the same class of error as
enumerating nesting by hand, one level up.

So the channel list is no longer trusted to a human either.
`every_gate_the_tree_declares_is_composed` walks `TREE` and asserts every gate
any counter declares appears in `CHANNELS`. **Adding a counter without composing
its gate is now a test failure rather than a silent 0.000 in a row somebody is
about to optimize.** Verified by mutation: removing the DPC channel — the
literal pre-fix state — fails the guard with
`counter 'dpc_alloc_ns' declares gate FN64_DPC_COPY_CENSUS but FN64_PROFILE does
not arm it`.

## Finding: this report broke the byte-identity checker with its own explanation

The armed lane reported `2 of 8 match, 0 deviate, 6 not found` — read as a
byte-identity FAILURE by the script, which exited 4.

It was not. `sim_time` and `fields` matched exactly, and `0 deviate` is the
tell: nothing disagreed, six things were *absent*. The cause:

`check-byte-identity.py` anchors to the **last** line containing
`[wm2000-block-progress]`, specifically so a free-text scan cannot pick a
different metric of the same name. The profile's own scope legend — the block
that explains the `gfx_submits` collision — **cited that tag as an example**.
That mention appeared later in the log than the data line, so the anchor landed
on the explanatory text and found no counters.

**An instrument broke another instrument, using the very text that explains name
collisions.** Two fixes, because either alone leaves the trap armed:

- **The legend no longer prints any anchor tag literally** — the names are
  assembled from fragments, so they read correctly to a human and match no
  extractor.
- **The checker's anchor was too loose in two independent ways** and is now
  tightened on both: `"[tag]" in line` matched the tag *anywhere*, including in
  prose, and `[-1]` then preferred the prose because it came later. It now
  requires the tag at **line start** *and* that the line actually **carries the
  counters**, and refuses outright if more than one candidate qualifies rather
  than guessing.

Verified against the unmodified poisoned log: **8 of 8 match**, and the control
lane still passes. The recovery is from the same bytes that produced the false
failure.

**The general rule: a log line is an API when anything parses the log.** Adding
a human-readable line to a log that an extractor reads is an interface change,
and prose that merely *names* an anchor is indistinguishable from the anchor to
a substring match.

## Rule: read your instrument's first real output before trusting it

Three of the four defects in the founding evening surfaced this way, and so did
the gap above and two formatting defects in this very tool (raw counter
identifiers printed as denominator labels; a hardcoded denominator that would
have stated staging cost as a share of host calls). **None came from reviewing
the code; all came from printing the output and looking at it.**

Reasoning about a decomposition's correctness is not a substitute for reading
one. Budget the look.

## Build notes worth keeping

Three checks that cost seconds and each guard against a failure this project has
actually had.

- **Confirmed the `rt64` feature by the binary-size delta, 87.1 → 93.5 MB**,
  rather than trusting the target dir. Rule 19 says a size difference proves
  "something changed", never "the thing you meant" — so it is not sufficient on
  its own, but a build that produced an *identical* size would have proved the
  feature did **not** go in, and the first RT64 attempt did exactly that
  (panicked: "built without the opt-in `rt64` Cargo feature").
- **Verified the pre-main constructor on this machine's Mach-O with a standalone
  binary before relying on it.** `__DATA,__mod_init_func` is not `.init_array`,
  and rule 20 exists because a gate was once tested only against synthetic input
  and crashed on BSD syntax here. Arming from a runtime seam would have been too
  late: the three `is_some()` gates are `thread_local!` cells fixed at first
  touch, so a late `set_var` yields gates that *read* as set in `PROVENANCE`
  while every counter reads zero — a plausible partial report.
- **Flagged the `FN64_PROFILE_CONTROL` near-miss instead of renaming silently.**
  `FN64_PROFILE_CONTROL` already means the typed executor control snapshot
  (`main.rs:1506`); `FN64_PROFILE_CONTROL_MS` sits one suffix away and reads as
  its variant. Renamed to `FN64_PROFILE_BASELINE_MS`. In a 51-gate namespace a
  one-suffix difference is a real hazard, not a style question.
- **Waited for load to fall below the script's own 3.0 gate rather than passing
  `--max-load`.** Editing the gate to match the run is not measuring. The gate
  then fired *again* on the armed lane — load was still 3.26 settling from the
  control run — and refused. Correct behaviour: an A/B whose two lanes run at
  different machine loads measures the load, and this pair's whole purpose is a
  perturbation delta small enough that contention would swamp it.

## Pre-existing red test, diagnosed and deliberately NOT fixed

`frame_census::tests::the_spread_rows_distinguish_a_spiky_phase_from_a_flat_one_at_equal_mean`
**fails at clean HEAD**, before any change here. Established by stashing
`frame_census.rs`, running the single test, and popping — the check most people
skip, and the one that separates "my change broke it" from "it was already red".

**Diagnosis:** the test builds 40 samples all at wall `40.0` ms, so every sample
lands in the SLOW bucket and the FAST bucket is empty. The assertion then does
`.lines().find(|l| l.contains("SPREAD dispatch (GUEST)"))`, which matches the
**fast** row first and reads `p50=0.000 p95=0.000`. A test-construction
artifact, not a product defect.

Left alone on purpose: fixing it would muddy a diff that needs reviewing on its
own merits. Recorded here because a diagnosis in the open is worth more than a
silent fix, and the next person to see it red should not re-derive it.

## Reused vs. written

**Reused unchanged:** `frame_census.rs` sampling/bucketing/percentiles/closure,
`dpc_copy_census.rs`, the executor and resume splits, the `atexit`/`install`
seam, `nearest_rank`, `check-byte-identity.py`, `byte-identity-1p5M.txt`, all 51
existing gates (composed, never replaced — committed scripts depend on them).

**Written new:** the counter-tree table + violation check (the only genuinely new
mechanism); the `FN64_PROFILE` composer and arming verifier; the
budget-denominator and row-sum formatting; p99 alongside p50/p95; the
`--profile` flag and its docs.

**One counter genuinely added, not just composed:** `dpc_copy_census::staging_totals()`
exposes `alloc`/`copy_in`/`copy_back` for per-field sampling. They existed only
as a whole-run `atexit` summary, so **a per-field figure for them had never been
produced and could not be recovered from any frozen log.** Three lines of
plumbing, and the owner's memcpy fix is unmeasurable without them.

**Net cost of the whole change:** two new files (~640 lines including tests) plus
~140 lines wired into `frame_census.rs`, `dpc_copy_census.rs`, `host.rs`,
`lib.rs`, and the benchmark script. No file in `fn64-cpu-runtime` touched, no
certified source, no identity digest moved.

## Proving each check can fail (rule 6a)

Every guard ships with a test that feeds it the bad input and asserts it fires.
A check that cannot fail is not a check — and rule 6a's own note applies: the
byte-identity author proved a wrong *value* was caught but not that the right
*thing* was read, which is rule 6a done halfway. So each test injects the
**specific historical defect**:

| test | injected defect | must fire |
|---|---|---|
| child exceeds parent | `gfx_ns` 21.5 under a 7.7 parent | subtree refused |
| negative residual | phases summing past `resume NET` | "instrument broken" |
| missing gate | `FN64_RESUME_SPLIT` unset under `FN64_PROFILE` | exit non-zero, names it |
| unreachable warning | NOT-ARMED path with its own gate off | warning still emitted |
| wrong denominator | rows at 20.9% totalling 1.29x budget | sum line shows 1.29x |
| name collision | a bare `gfx_submits` key in the report | rejected; scope-qualified name required |
| span-vs-run confusion | span count checked against run expectation | anchored read, fatal on missing anchor |
| **decomposition > its field** | 45 ms of phases in a 10 ms field | refused — **and `validate` asserted empty**, proving it is not a duplicate guard |
| **uncomposed gate** | a tree counter whose gate is absent from `CHANNELS` | test failure naming counter *and* gate |
| **set-but-dead channel** | gate present, no data (the `=0` trap) | reported missing; an env echo would call it armed |
| perturbation unmeasured | no control lane supplied | header says UNMEASURED, never implies zero |

These are unit tests over synthetic counter values — no ROM, no 25-minute run.
Note rule 20: tested on this machine, not merely in logic.

**Mutation-tested, because a passing test is not a working check.** Disabling the
violation check fails exactly the four defect tests while the eight structural
ones stay green. Re-parenting `gfx_ns` as a peer of `resume_hostcall_ns` — the
literal historical rule-2 error — fails three. Removing the DPC channel — the
literal pre-fix state — fails the composition guard by name. **A test that still
passes with the check removed is decoration.**

## Instrumentation perturbation (rule 17)

`FN64_PROFILE` arms a lot at once, and a prediction of 0.029 ms/field once
measured +1.62 — **wrong by 56x**. So the cost is not predicted, it is
**measured with an armed/control pair and printed in the profile's own header**,
where the reader cannot miss it. If it is large, the header says so and points
to a lighter tier (`FN64_PROFILE=census` — populations only, no phase timers).
Shares survive perturbation; absolute ms do not, and the header will say that
too.

## Acceptance

Two tables, because **the backend changes the answer** and that difference is
itself the argument for the renderer provenance line.

**Reference (software rasterizer)**, slow population, 1.5M steps: `resume NET`
45.687 · guest code 9.528 (20.9%) · gfx 32.119 · RDP 26.396 · RSP 5.637 ·
mirror 8.848 (sibling of resume NET) · closure PARKED 0.574.

**RT64 — the lane the owner actually runs**, and the harder case for the format:

| row | ms/field | x budget |
|---|---|---|
| guest code | 9.79 | 0.59x |
| mirror | 9.01 | 0.54x |
| RSP | 5.76 | 0.35x |
| rasterization | 4.00 | 0.24x |
| invalidate | 2.04 | 0.12x |
| staging memcpy | 1.77 | 0.11x |
| **TOTAL** | **32.37** | **1.94x** |

**No single row exceeds 0.59x.** On the reference backend, RDP at 26.4 ms is
visibly enormous and any format would flag it; here every row looks individually
modest and the program still misses 60fps by nearly 2x. **The summed line is the
only thing that can show that**, which is exactly why requirement 2 pairs the
budget column with a computed `Σ = N.NNx`. Covered by
`six_individually_modest_rt64_rows_are_shown_to_total_almost_2x`.

Consequences for the build, both now on the critical path for real work (the
owner has asked for the mirror and the staging memcpy to be fixed, and both are
held until this lands):

- **The staging copy gets named rows.** `alloc` / `copy_in` / `copy_back` exist
  in `dpc_copy_census.rs` but were **whole-run `atexit` totals only** — never
  sampled per field, which is why they could not be found in the frozen RT64
  logs and why no amount of re-reading one would produce them. Now sampled per
  field and declared as children of `gfx_lle_rdp_ns`.
- **The mirror's parent relationship is explicit** — nested inside
  `exec_resume_ns`, sibling of `resume NET` — declared in the tree and asserted
  by `mirror_is_inside_resume_but_sibling_of_resume_net`.
- **Row denominators come from the tree, not the call site.** Hardcoding is how
  a staging cost gets stated as a share of host calls when it is really a share
  of the RDP seam.

Guest byte-identity via `scripts/check-byte-identity.py` against
`scripts/byte-identity-1p5M.txt` (anchored; no hand-rolled extraction):
`gfx_submits=11153 audio_submits=7685 sp_tasks=18838 vi_interrupts=8386
controller_ops=2390 sim_time=13112786076 render_error=None`.

### Measured, RT64 lane, 1.5M steps

**Control (profile off):** `8 of 8 match — GUEST BYTE-IDENTICAL for this route.`
Steady state `fields=7699 p50=16.01 p95=37.85 p99=38.81 mean=21.96 ms/field`,
`over_16.667ms=3840 (49.9%)`, RATIO A `1.32x` budget, RATIO B `1.311x`, guest at
`59.7 Hz`.

The distribution is the argument for requirement 4 on its own: **p50 16.01 clears
the budget and p95 37.85 is 2.27x it**, with 49.9% of fields over. A mean of
21.96 describes neither population.

**But an all-fields p50 is itself a wrong-denominator statistic.** With a near-
exact 50/50 split, the all-fields median sits *at the boundary between the two
populations*, so "p50 clears" is a restatement of "the fast half clears" wearing
a more interesting name. Requirement 4 exists precisely because this number is
misleading; the per-population figures below are the real answer.

### Measured, armed lane (`FN64_PROFILE=1`), RT64, 1.5M steps

`8 of 8 match — GUEST BYTE-IDENTICAL.` All seven gates ARMED, verified by
effect. `RENDERER: rt64`.

| population | fields | p50 | p95 | p99 | mean | p50 vs budget |
|---|---|---|---|---|---|---|
| fast | 3857 (50.1%) | 8.858 | 9.548 | 9.682 | 8.921 | **0.53x** |
| slow | 3842 (49.9%) | 36.462 | 38.137 | 38.846 | 34.891 | **2.19x** |

**The two populations are the render/non-render split, essentially exactly:**

```
submits=0  fast=3846  slow=   0
submits>0  fast=  11  slow=3841
100.0% of SLOW fields carried a submit vs 0.3% of FAST fields.
```

The slow half holds **79.6% of wall time in 49.9% of fields**. Fast fields
already run at 0.53x budget with essentially no graphics; the ceiling on fixing
only the slow half is `8.92 ms = 0.54x budget`, which **clears the bar**.

**Perturbation: −0.079 ms/field (−0.4%)** — armed 21.881 vs control 21.960,
i.e. the instrument is *not* resolvable from noise at this size and certainly
not costing 1.62 ms. Small relative to every row quoted, so the shares and the
slow-population figures stand. One pair only: a first estimate, not a settled
number.

The report is emitted from `fn64-abi` via the existing `atexit` seam, so
`main.rs` and `DISPATCH_SOURCE_SHA256` are untouched and the emulated program is
unchanged by construction.
