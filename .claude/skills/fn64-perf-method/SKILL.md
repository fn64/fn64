---
name: fn64-perf-method
description: Use when measuring, profiling, benchmarking, or optimizing anything in fn64 — before dispatching a perf agent, before believing a benchmark number, before committing in a shared tree, or when a measurement looks surprising. Every rule earned by a specific wrong answer that was believed for hours.
---

# fn64 measurement method

Every rule below was earned by a measured failure on this project. They are not
style preferences: each one names a number that was wrong, believed, and acted
on.

**The evidence lives beside this file in `REFERENCE.md`** — the closed-lines
ledger (do not re-propose without reading it), the worked cases behind each
rule, the environment contracts, and the byte-identity gate. Read the relevant
entry there before acting on any rule you are about to lean on hard. Deeper
still: `docs/plans/perf-method.md`.

**The standing bar depends on the title's own render rate, and getting this
wrong reframed the whole project.** 16.667 ms per emulated VI field is
*hardware parity* — every VI field on time. But **WM2000 renders at 30 Hz**, so
a drawn frame gets **two** field budgets: **33.333 ms**.

**A DRAWN FRAME COSTS BOTH FIELDS**, and the only numbers that describe what
ships are **unprofiled means**. Never build a shipped-frame figure from profiled
numbers or from p50s — profiling inflates absolute cost, and on a bimodal
distribution p50 is not the mean. Take an unprofiled run's mean, double it for a
30 Hz title, and quote that.

## STATE THE RENDERER OR SAY NOTHING

`render-benchmark.zsh` does not export `FN64_RENDER` and `main.rs` defaults to
`"reference"` — the *software* rasterizer. The owner runs **`FN64_RENDER=rt64`**,
where the same route measures **17.25 ms/field mean, 29.0 fps, 1.16 ms from the
30 Hz budget** — a gap **20x smaller** than the reference lane reports.

**A graphics figure without its renderer beside it is not a result.** This trap
has cost two separate investigations, the second to an agent that had read the
warning about the first. `render-benchmark.zsh` echoes `renderer:` in its
provenance block and `[fn64-profile]` prints `RENDERER:`. Check that line before
reading any other number in the report. Full correction: `REFERENCE.md`.

## Running the work (not the experiment)

**Instrument, then measure, then optimize. Never interleave.** If the current
cost of a thing is not already in a trustworthy report, the next task is the
report — not the fix.

**Do not dispatch an optimization for a cost you have not measured.** Violating
this in a brief is the same error as violating it in an experiment.

**Search for existing evidence before spending machine time.** Check the log
directory, the frozen snapshots and the doc before queueing a run.

**Verify a subagent's finding before relaying it upward, or label it
unverified.** A relayed claim inherits your credibility, not the reporter's.

**Fewer agents on entangled work.** Two agents on genuinely independent work
finish sooner than four on coupled work, because coordination is the cost.

**One owner per run.** If you did not start it, do not restart it — ask the
owner.

**Read your instrument's first real output before trusting it.** Print the
report and look at it; code review does not catch a decomposition that reads
45.7 ms inside a 10 ms field.

**Default to:** pre-registering thresholds, predictions and falsifiers *before*
data; enforcing closure tolerances **in code** rather than in intent; and a
**one-minute smoke run** before any 25-minute one.

## The three that cost the most time

**1. Measure before dispatching, not after.** Every win on this project came
from a number; every wasted day came from reasoning about code structure. If
you cannot state the current cost of the thing you are about to optimize, you
are not ready to optimize it.

**31. Pre-register the closure tolerance IN CODE.** Buckets must sum to their
parent, the residual prints unconditionally, and the report refuses to present a
split that does not close. This caught three defects in one hour, **none of them
visible in the phase values — all three looked like findings.** Corollaries: *a
wall clock cannot time a stack that yields* (instrument the one chokepoint every
yield passes, never the N call sites); *always report a nested counter against
its containing bucket*; and **a label that is 95% something else is a wrong
measurement wearing a plausible name.**

**A frozen log is not a dead process; a rising CPU TIME is the only liveness
check.** This route writes only on controller edges and heartbeats, so a quiet
log is normal. **`ps` with an exact match plus an advancing `TIME` column is the
check.** Use `pgrep -x`, never `pgrep -f`.

**And when a run dies, "cause unknown" beats a mechanism that fits.** *A
mechanism that explains the evidence is not thereby the cause.* Ask before
diagnosing someone else's process death.

**6c. Match the probe's LAYER to the property's layer.** `strings`, `nm` and a
grep for a runtime-computed digest all ask *the file* what only *the running
program* or *the disassembly* can answer. **When a probe returns zero, first ask
whether it could have returned nonzero.** Three fast agreeing answers are more
dangerous than one slow ambiguous one.

**6a. Before trusting a check, confirm it CAN fail.** A check that returns the
same answer regardless of the state it is checking is not a check. Prove the
verifier distinguishes the two outcomes it exists to distinguish — ideally by
injecting the defect and watching it catch it.

**A `*-block-boot` binary STOPS AT FIRST OVERLAY ENTRY by design unless
`FN64_BLOCK_CONTINUE_AFTER_OVERLAY` is set** (`main.rs:1131`). The failure shape
is rc=0 with a plausible-looking log — ~16k steps, `gfx_submits=0`, a dump that
reads like progress. **The discriminator is `gfx_submits`, never the exit code.**

**15. Verify the state you meant to cause, not the call you made.** `kill -STOP`
returning 0 says a signal was sent, not that it landed. A green test run in a
dirty tree says nothing about HEAD.

## Reading a profile

**2. Self time = count minus immediate children.** A sampling profiler
attributes samples to every frame on the stack. Use
`scripts/wm2000_self_time.py`. Corollary: **`executor_ns` is INCLUSIVE** —
reading it as a peer of `gfx_ns` rather than its parent concealed 21.72 ms of a
35.84 ms field.

**3. Count, do not infer.** A profiler attributes *samples*, not *calls*. Twelve
`FN64_*_CENSUS` / `_SYSCALLS` / `_STATS` gates exist in `fn64-abi` for this —
add one.

**12. A large byte count is not a bottleneck.** Eliminating 5.92 GB of provably
dead depth-buffer copying moved the metric **+0.84%, the wrong direction**. A
bulk `memcpy` runs at streaming bandwidth; a gather-and-blend over the same
bytes does not.

**17. Do not budget instrumentation cost by counting clock reads.** A predicted
0.029 ms/field cost measured **+1.62 ms/field — wrong by 56x**. Always run an
**armed/control pair** and correct absolutes by the ratio — shares survive,
absolute ms do not. Any phase measuring below the perturbation is below the
instrument's resolution.

## Running an experiment

**4. Only measure on a quiet machine.** A concurrent shard rebuild made a 421 ms
baseline read 775 ms.

**30. A WAIT is a workload.** A probe is bounded; a **polling loop is unbounded
and compounds**. **One waiter at a time, chained not nested** (put the wait and
the work in ONE background command); **never poll a metric your polling
affects**; prefer waiting on an artifact over a machine state. Do not pass
`--max-load` to escape your own contention — that is editing the gate to match
the run.

**5. Interleave A/B pairs.** Not six of A then six of B. Interleaving preserves
an effect's *direction* through contention but not its *magnitude*.

**Two or more reps, always.** A single pair reversed sign on rep 2.

**6b. Echo the gate's VALUE at the moment it takes effect.** One line —
`FN64_X=${FN64_X:-<unset>}` printed by the lane runner itself — caught an A/B
where **both lanes were the control lane.** Make an unrecognized lane or rep
**exit non-zero** rather than defaulting into a branch.

**6. Prove the lanes differ before believing a number.** Check a counter or
symbol that must appear in one lane and not the other.

**19. A size difference proves "something changed", never "the thing you
meant".** **Check the generated source, not the binary size.**

**9. Never run a rebuild-triggering agent beside a benchmarking one.** Serialize
them.

**11. A frame-time distribution needs frames in it.** The standard route
(`FN64_BLOCK_MAX_STEPS=19523`) has `gfx_submits=0` — every latency statistic
over it describes an idle guest.

**29. FREEZE a log before analyzing it.** A file a process is still writing is
not a population. `cp` to `$CLAUDE_JOB_DIR/tmp` and **cite the snapshot path in
every claim.** A series of **windowed p50s cannot distinguish "the game got
slow" from "the emulator has regimes"** — only raw per-field data can
(`FN64_FRAME_CENSUS_SEQUENCE`).

**Report per population, not just the mean.** WM2000's field distribution is
bimodal; an average over both populations hid the render cost for a day. Give
mean and p50/p95/p99, split by render vs off-field.

**A targeted counter improving while the program regresses does not ship.**

## Reading a number

**32. A share of the wrong denominator is not a size — SUM the list, do not eye
it.** Rows that look modest against their decomposition's parent can exceed the
parent the *decision* uses. **Check every row against the denominator the
decision uses, and add the numbers up in a tool.** Corollary: when you catch
this, "X is the bottleneck" usually becomes "both halves must fall".

**10. State both ratios, or you have said nothing.** *Wall ms per emulated VI
field* (target 16.667) and *wall-versus-virtual* (target 1.000x) are different
questions.

**21. The counter's UNIT is part of the counter, and a rate inherits its error
silently.** `AudioOutputStats::samples` counts i16 **channel** samples, not
frames — read as frames it gave "91.5% of real time" against a true **45.7%**.

**22. A healthy delivery path is not a healthy stream.** **Equal counts prove
nothing is being LOST; they say nothing about whether enough is ARRIVING.** For
any producer/consumer seam the health metric is *delivered ÷ wall-clock seconds
against the rate the consumer demands*. Put the counter that can contradict you
on the routine line, not the exceptional one.

**14. A rate-limited loop cannot report its own cost.** `shell.rs` sleeps to a
frame deadline, so `frame_interval_ms` measures the clamp, not the work.

**7. A line printed on a state CHANGE cannot prove absence of progress.** Print
on a cadence the *process* controls (`FN64_HEARTBEAT=<steps>`).

## Checking the machine and your tools

**18. `pcpu` and `pgrep -f` both lie about this workload.** `pcpu` reads 0.0
while the benchmark runs at 100% — **advancing CPU TIME is the only honest
liveness check**, sampled twice. `pgrep -f` both over-matches (your own
monitoring shells) and **under-matches, which is worse** — it missed 15 running
`rustc` processes because cargo's real argv is not the command line you typed.
**Wait on the ARTIFACT (`until [[ -x $BIN ]]`) or on a completion marker your
own script writes — never on a process pattern.** *Absence of a match is not
evidence of absence.*

**20. Test the verifier on this machine, not just its logic.** `find -newermt`
is GNU syntax and BSD `find` on macOS rejects it. Prefer constructions whose
failure mode is a crash over an empty result. There is no `readelf` and no
`timeout` here; binaries are Mach-O.

**A key that appears twice is two metrics.** **Anchor every extraction to a
named line, and make a missing anchor fatal rather than falling back to a
scan.** Consistency across runs is not validity when every run goes through the
same broken instrument.

**23. Several narrow checks agreeing is not corroboration when they share a
blind spot.** Each may answer the question literally asked while none answers
the question meant. Ancestry is `git merge-base --is-ancestor`, not a log
window.

## Working in this tree

**8. Editing `fn64-recomp-rs` costs 32 crate rebuilds** — ~9-11 minutes, versus
~25 s for `fn64-abi`. Every file in `crates/fn64-recomp-rs/src` is a certified
source, so an edit changes an identity digest. Prefer `fn64-abi`; when you must
cross, say so in the commit.

**13. Commit with a pathspec — `git add` is not enough.** `git commit` writes
**the whole index**, not the paths you just added. A commit recording rule 12
ran `git add` on one path and landed two, because a peer had `vi.rs` staged;
**HEAD did not compile and it was pushed.** Always `git commit -- <paths>`.
Never `git add -A`.

**Background every build** (`nohup` + log). Use `$CLAUDE_JOB_DIR/tmp`, not
`/tmp`, for scratch. Give each agent its own `CARGO_TARGET_DIR`.

**A source edit is a build.** A shared checkout means *file* contention, not
just CPU contention. **No ordering makes a multi-file change atomic to a
concurrent reader** — the only remedy is not writing while someone reads.
Serialize writers against builders, not merely benchmarkers. A build that fails
on a file you never opened is someone else's in-flight edit, not your bug.

## Licensing — non-negotiable

fn64 stays MIT. `~/Code/aki-recomp` is **GPL-3.0**: do not open its `.c`, `.h`,
or `.cpp` files. Measured observations may be read from any project; **code may
not**. No code may be copied or adapted from any GPL runtime.

## Writing it down

Record negatives. "I measured X and it is not the problem" is a result and
prevents the next agent re-running it. When a figure you published turns out
wrong, **leave it visible with a correction** rather than silently swapping it —
the error is the instructive part.
