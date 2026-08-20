# Can WM2000 play a match to completion on fn64's all-Rust stack?

This file is the standing answer to that question, and is updated as runs land.
Every claim is CONFIRMED (measured, with the command that produced it) or
HYPOTHESIS.

## The question, made precise

"Does a match end" turned out to be three separable questions, and the prior
framing conflated them:

1. **Does the ROM keep running?** Answered before this card: yes, to swap
   17,473 with zero raw-DPC backend errors, stopped only by `WM2000_MAX_STEPS`.
2. **Does the match END?** Requires knowing what "end" *is* in this ROM's own
   terms. It does now: the state machine `func_801226A0` moves `D_801589D6`
   from 2 (live match) to 3 (decision), and that transition IS the answer.
   See `RT64-WM2000-MATCH-GRAMMAR.md`.
3. **Does input reach gameplay?** A distinct question from either, and the one
   the frame hash has repeatedly answered wrongly.

## What the ROM says it takes to end a match (CONFIRMED)

Read from the disassembly, recorded in full in `RT64-WM2000-MATCH-GRAMMAR.md`:

- The state byte is **`0x801589D6`**: 0 init, 1 entrance, **2 live match**,
  **3 decision**, 4 post-match. Monotonically increasing.
- Six conditions inside state 2 can advance it: the **time limit**
  (`func_80123D64`), the **all-down countout** (`func_80123B48`), and four
  per-wrestler end-condition flags in `D_8016722E`.
- The end flags land in **`0x8016ED2A`**: `0x40` normal finish, `0x10`
  time-limit draw, `0x80` sequence over (which gates the only call to the
  exiting fade `func_800EE4AC`, and so the loop exit).

### The time limit cannot be waited out (CONFIRMED)

This is the finding that reshapes the card. `D_8014E1C4` holds **minutes**, not
ticks -- the ramp is 0,5,10..60, and `D_8016F0AC`, the counter compared against
it, is the minutes counter (its sibling `D_80166F88` rolls over at `0x3C`).

At the measured host rate of ~50,000 steps/min and ~508 steps/swap in a match
(`RT64-WM2000-MATCH-RUN-BUDGET.md`), the time limit costs **6.1 hours of wall
clock at the 10-minute setting, 18.3 at 30 minutes, and 36.6 at 60** -- and the
only hard-coded non-zero default found selects 60. Setting 0 means *no limit*
and is reachable, in which case no budget ends the match at all.

**So "run it long enough and the match will end by itself" is not a strategy.**
A run has to either produce a pin or a countout, or measure its progress
against the game's own clock and report that.

## What a run must therefore do

Point the long run at a **variable**, not a picture. The instruments:

| Probe | Address | Answers |
|---|---|---|
| match state | `0x801589D6` | did the match end (2 -> 3) |
| end flags | `0x8016ED2A` | how it ended (`0x40` finish / `0x10` draw / `0x80` fade) |
| winner | `0x801589D4` | who won |
| match clock | `0x8016F0AC` (min), `0x80166F88` (sec) | how far into the match |
| limit + setting | `0x8014E1C4`, `0x800961D2` | which of the five cost rows this run is in |
| gameplay input | `0x80095184/86` p0, `/90/92` p1 | **did the button reach gameplay** |
| spirit | `0x801671F0` p0, `0x801672F4` p1 | is damage accumulating |
| referee count | `0x8016ECC0` | is a countout in progress |

All of these are in `wm2000-match-run.sh`'s default watch set. The probe costs
one comparison per swap and prints only on change, so it is affordable on a run
long enough to matter -- unlike frames and traces, which are not.

## Two harness gaps this card had to close first

**Guest memory was not readable from a run.** `WM2000_WATCH`
(`docs/tools/wm2000-watch-patch.py`) adds it. It is a committed, anchor-checked,
idempotent patcher rather than a `.patch` file because `run-rs-lane.sh`
re-copies the harness from the sibling repo on every run -- and the traps doc
records what the `.patch` shape cost the last time.

**Both wrestlers received identical input.** The harness mirrors one composed
pad onto every plugged port, so two wrestlers performed the same move on the
same frame -- the shape of a stalemate, not a fight, and a poor test of whether
a match can resolve. `WM2000_INPUT_SCRIPT_P1` drives port 1 independently.
Making that work needed two changes together: the per-port block must run after
the mirror loop, *and* the mirror must skip a port that has its own schedule.
Either alone leaves port 1 driven by port 0's script, which would have shown up
as "input does nothing" -- a false negative on the exact question being asked.

## Run log

| # | mode | steps | schedule | result |
|---|---|---|---|---|
| -- | -- | -- | -- | (pending: the ROM lane was held by a boot-ladder calibration) |
