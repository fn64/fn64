# Executor yield/resume census

`FN64_EXECUTOR_YIELD_CENSUS=1` arms a title-neutral, diagnostic-only census at
the linked whole-function lane's one scheduler boundary:
`Executor::run_one_step`. That function already owns thread selection, the
typed `Resume` supplied to `GameThread::resume`, and the typed `Yield` (or
return) received from it. Counting there covers every coroutine handoff
without instrumenting generated functions or translated blocks.

The census reports, for each retained `ThreadId`:

- exact counts for all five `Resume` variants;
- exact counts for each `Yield` shape, separating blocking/nonblocking receive
  and blocking/nonblocking, head/tail send;
- returns; and
- total and maximum wall time around the outer `GameThread::resume` call.

For instruction checkpoints it also reports a bounded exact histogram of the
charged instruction counts and follows each owner until that thread next
resumes. The follow-up row says whether another coroutine resume was interposed,
the maximum number interposed, and the owner's next typed yield or return. This
distinguishes the C lane's synthetic 250-cycle pre-yield accounting checkpoint
from back-edge and MMIO checkpoints, and measures how often the accounting
bridge returns immediately to the same thread. It does not claim those bridges
are safe to remove: device deadlines, timer delivery, interrupt state, and
priority selection are committed while the coroutine is suspended.

The shell prints the snapshot beside the bounded pump report. Arm both and set
the existing pump bound, for example:

```sh
FN64_PUMP_CENSUS=1 \
FN64_PUMP_CENSUS_PUMPS=600 \
FN64_EXECUTOR_YIELD_CENSUS=1 \
./scripts/play-wm2000.sh
```

When the executor gate is absent, empty, or `0`, the snapshot and pump report
say `NOT ARMED`; zero counters are never presented as measured zero work.
Values other than `0` and `1` trap at executor construction instead of being
guessed.

Memory is bounded to 64 exact per-thread rows and 16 distinct checkpoint-charge
rows per retained thread. Further observations accumulate
in fixed-size overflow resume/yield arrays, and the report says
`INCOMPLETE PER-THREAD EVIDENCE`; it never retains an unbounded set of overflow
thread IDs. Additional distinct checkpoint charges increment an explicit
per-thread overflow count. Counts use checked arithmetic and trap rather than
wrap.

This is not emulated timing, scheduling authority, or a performance fix. When
armed, it deliberately perturbs the measured program with two host `Instant`
reads per coroutine resume and bounded counter updates. The wall durations are
only host profiling evidence and never advance or retime the executor, device
fabric, VI, AI, CP0 Count, or OS timers. Comparative performance runs must keep
the gate consistently armed or consistently unarmed.
