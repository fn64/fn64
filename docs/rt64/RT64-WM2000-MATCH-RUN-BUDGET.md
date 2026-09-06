# What a "run the match to the end" run actually costs

Every prior WM2000 run was stopped by `WM2000_MAX_STEPS`, and the obvious
remedy -- "raise the budget a lot" -- runs into a wall that is worth writing
down once, because it determines what a single lane can and cannot answer.

## The two rates (CONFIRMED, measured 2026-08-19)

**Host throughput: ~50,000 executor steps per minute.** Measured directly by
sampling the harness's own `steps=` progress line 60 seconds apart during a
boot-ladder run on an idle 15-core host (load average 2.1, the emulator pinned
at ~92% of ONE core -- it is single-threaded, so host cores do not help):

```
steps=400000 -> steps=450000 over 60s
```

**Guest cost: ~155 steps per VI swap in menus, ~508 in a match.** From the same
run's progress lines for the menu phase (400,000 steps -> 2,522 swaps), and by
difference against the committed 8,000,000-step run that reached swap 17,473:

```
(8,000,000 - 400,000) / (17,473 - 2,522) = 508 steps/swap in match
```

## What that buys

A VI swap is one 60 Hz field, so **3,600 swaps is one minute of game time**.

| Game time | Swaps | Steps | Wall clock at 50k/min |
|---|---|---|---|
| 1 min | 3,600 | ~0.9 M | 18 min |
| 3 min | 10,800 | ~4.6 M | 1.5 h |
| 5 min | 18,000 | ~8.3 M | 2.8 h |
| 10 min | 36,000 | ~17.4 M | 5.8 h |
| 20 min | 72,000 | ~35.7 M | 11.9 h |

Two consequences follow, and both are load-bearing:

**1. The famous 17,473-swap run was 4.9 minutes of game time.** That is about
one match's worth if the match is short, and less than one if it is not. The
run did not "stop just before the end"; it stopped in the middle, and there was
never any reason to expect it to have finished.

**2. A 40,000,000-step run is a 13-hour run.** It is a perfectly reasonable
thing to launch and a completely unreasonable thing to iterate on. A lane that
wants an answer in one sitting gets roughly one long run, so the run has to be
instrumented to answer the question the first time rather than to be repeated.

## What follows for instrumentation

Frames and traces cannot be left on for a run of this length. The harness
buffers every trace event in memory (`write_trace_file` reads
`fn64_abi::copy_trace()` at exit) as well as appending JSONL, and a traced run
has been measured at roughly 3x slower with a 996 MB sink. At 13 hours the
multiplier is the difference between a run that finishes and one that does not.

That is why `docs/tools/wm2000-match-run.sh` splits the lanes:

- `--long` is untraced and dumpless. It carries `WM2000_WATCH` instead, which
  costs one comparison per swap and prints only on change. Guest memory is a
  far better instrument than the framebuffer here anyway: the frame hash is the
  documented WRONG instrument on this ROM, and the match-end condition is a
  single byte (`D_8016ED2A` bit `0x80`, see `RT64-WM2000-MATCH-GRAMMAR.md`).
- `--frames` is bounded with `WM2000_STOP_AT_SWAP` so the sink stays finite,
  and exists only to capture a picture once the long lane says where to look.

## The rule this implies

**Point the long run at a variable, not at a picture.** A run that has to be
watched by eye is a run you cannot afford to make long enough to answer this
question. A run that prints one line when `0x8016ED2A` changes can be launched
and left.
