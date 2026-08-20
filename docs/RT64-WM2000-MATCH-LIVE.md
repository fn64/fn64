# CONFIRMED: WM2000 reaches a live match on the all-Rust stack

Measured from the guest's own state, not inferred from pixels or swap counts.

## The evidence

A run of the committed two-controller lead-in plus a grapple schedule, with
`WM2000_WATCH` sampling guest RDRAM once per VI swap:

```
swap #1     0x801589D6 = 0x0     match state: not started
swap #6306  0x801589D6 = 0x1     transitioning
swap #6336  0x801589D6 = 0x2     LIVE MATCH
swap #8136  0x8016F0AC = 0x1     match clock: 1 minute
swap #9936  0x8016F0AC = 0x2     match clock: 2 minutes
```

`0x801589D6` is the match state byte (`func_801226A0` switches on it; 2 = live,
3 = decision). `0x8016F0AC` is the match clock in MINUTES (the carry logic at
`0x80123E6C` fixes the units; its sibling rolls over at 0x3C).

**So the guest is in a live match and its own clock is advancing** -- roughly
1,800 VI swaps per game-minute. Zero panics and zero raw-DPC backend errors
through swap 11,039, where the run was stopped for unrelated reasons.

## What this settles, and what it does not

**Settles:** the ROM reaches gameplay, not merely gameplay-looking content. The
earlier in-match frames are corroborated by the guest's own state machine, and
the "is the lead-in reaching a match or a stable screen" question recorded in
`RT64-WM2000-INMATCH-GAPS.md` is answered -- it reaches a match.

**Does NOT settle: whether input reaches gameplay.** The clock advancing proves
the match is running, not that either wrestler responds to the pad. That needs
a differential -- two runs identical except one presses in-match -- and it is
cheap: the match goes live by swap ~6,336, so a run of ~8,000 swaps suffices.
There is no need to run to a step budget that takes hours.

**Does NOT settle: whether a match ENDS.** Established separately and
independently of fn64: the time limit is 60 game-minutes by default (index 0 is
unlimited), and no button test gates a pin anywhere in the ROM -- the pin bits
are set only inside move-script handlers reached by landing a grapple. A
scripted schedule cannot force either ending. At ~1,800 swaps per game-minute,
a 60-minute match is ~108,000 swaps.

## Cost note, so this is not re-derived expensively

The measured rate is ~53,000 emulator steps per wall-clock minute. A 40M-step
budget is therefore 12+ hours. Do not reach for a bigger budget to answer a
question the state probe answers in minutes: watch `0x801589D6` and
`0x8016F0AC` and read the verdict directly.

## MEASURED: where the wall-clock actually goes (do not optimise the wrong half)

A natural instinct on seeing a 12-to-30-minute run is to cache the recompiled
crate and the built harness, since every run rebuilds into a fresh scratch
root. **Measured on a real run, that instinct is wrong.**

Timestamps from one control run (`--long`, 2M step budget):

```
07:04:59  run dir created, log opened
07:05:01  emitted crate written        (recompile_rom: ~2s)
07:05:45  harness binary built          (cargo:       ~44s)
07:37:58  still executing               (emulator:    32+ min and counting)
```

**Build and recompile together are ~46 seconds. Execution is 32+ minutes.**
Caching the build would save roughly 2% of the cycle. It is not worth doing,
and anyone proposing it should be pointed here first.

The cost is the emulator itself, at a measured ~53,000 steps per wall-clock
minute. The levers that actually matter, in order:

1. **Ask for fewer swaps.** The match goes live at swap ~6,336, so a question
   about in-match behaviour needs ~9,000 swaps (~2M steps), not a 40M-step
   budget. Most of this project's long runs were answering a question that a
   short run answers identically.
2. **Watch guest state, not pixels.** `0x801589D6` and `0x8016F0AC` report a
   verdict directly; scanning thousands of dumped frames does not.
3. **Do not trace unless you need frames.** A traced run buffers every executor
   event in memory as well as appending JSONL -- one wrote a 996 MB sink and
   ran roughly 3x slower.

A true savestate would be the real fix, and it is genuinely hard here rather
than merely unbuilt: guest threads run on `corosensei::Coroutine`, a real
machine stack executing recompiled native code. Serialising that means
capturing and restoring native stacks, which is the hard part of savestates for
recompiler-based emulators. RDRAM and the device models are the easy half. If
it is ever attempted, the tractable version is a snapshot at a quiescent point
-- between VI retraces, with no guest thread mid-syscall -- not a general one.
