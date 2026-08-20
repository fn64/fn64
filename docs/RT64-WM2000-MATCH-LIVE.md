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
