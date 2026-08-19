# Wall: texrect refuses `Combined` in slot B

## Static analysis (before any ROM measurement)

`production.rs:2175` passes `self.rdp_state.combine()` to `execute_raw_dpc_inner`
AFTER `plan_raw_dpc` has already run `self.rdp_state.apply(&delta)` (line 2163).
`other_mode` (line 2173) and the tiles (line 2181) both use pre-delta snapshots
taken at lines 2159/2161. `combine` does not. The struct doc at production.rs:118
states this outright: "`combine`, the constant colors and `color_image` are also
folded early and the same reasoning applies to them, but no measurement
implicates them yet, so they are deliberately still seeded live."

So the same time-travel defect fixed by f2c52822 (tiles) and d53e4835 (other_mode)
is structurally present for `combine`. Whether it is THIS wall's cause is not yet
measured.

## Second candidate, independent of time travel

`texrect.rs:960-990`. `CombinerProgramSlice::reads_second_bitfield_slice()` is
false ONLY for `FirstOfTwoCycles`, so one-cycle mode reads the SECOND bitfield
slice (matching RT64's one-cycle behaviour). But
`carries_a_first_cycle_result()` is true ONLY for `SecondOfTwoCycles`. A
one-cycle program therefore decodes its inputs from the cycle-2 bitfield while
being refused any `Combined` it finds there. If WM2000 latches a combine word
whose cycle-2 slot B is COMBINED and runs it in one-cycle mode, this refusal
fires on a program the RDP executes fine.

Both candidates predict the same error text. Distinguishing them requires the
actual combine word and cycle type at the failing draw.

## MEASURED on the real ROM: candidate 2, NOT time travel

Marker-file-gated probe at the refusal site (`texrect.rs`
`validate_combiner_program`), real WM2000 ROM, all guards live. Aborted at
`vi_swaps=1887`, reproducing the baseline exactly. The probe fired ONCE:

```
REFUSE color slot=B input=Combined slice=OnlyCycleOfOneCycleMode
second_cycle=true combine_hi=0xf00ff23f combine_lo=0xfc15fea3
slices=OnlySecondSlice
```

`slices=OnlySecondSlice` is one-cycle mode. So this is NOT the plan/execute
fold defect: the failing draw is a genuine one-cycle program, and no amount of
correct `combine` seeding changes which selector it names. Candidate 1 remains
a real latent bug (see below) but is not this wall.

Hand-derived from the wire layout, not from the code under test.
`parse_color_b(second_cycle = true)` is `(high >> 24) & 0xF`:
`0xf00ff23f >> 24 = 0xf0`, `& 0xF = 0`. Selector 0 is `C_COMBINED`
(`rt64_color_combiner.h:23`, first enumerator). So slot B genuinely selects
COMBINED in the only slice this one-cycle program evaluates.

## What the RDP does, from RT64's pinned 5473732a

`src/shared/rt64_color_combiner.h`:

- `run()` (line 611) zero-initializes: `combinerColor = float4(0,0,0,0)`
  (line 612), then calls `runCycle(inputs, twoCycle ? 0 : 1, twoCycle,
  combinerColor)` (line 620). For one-cycle, `twoCycle == false`, so the
  cycle argument is **1** -- one-cycle reads the SECOND bitfield slice.
  fn64's `run_one_cycle`'s `SECOND_CYCLE = true` already matches this.
- `fromColorInput()` `case C_COMBINED: return combinerColor.rgb;`
  (lines 470-471) -- **unconditional**. There is no refusal, no
  special-casing, and no cycle guard. In a one-cycle program it returns the
  zero-initialized accumulator.
- The input-wrap that makes COMBINED a *carry* runs only under
  `const bool secondCycle = twoCycle && secondCycleInputs;` (line 577), i.e.
  only in an actual two-cycle program's second pass (lines 580-601). A
  one-cycle program skips it entirely, so the accumulator stays the
  untouched zero.

So COMBINED in a one-cycle program is **defined behaviour reading zero**, not
undefined and not a hardware edge case needing an authority call.

## The capability already exists in this crate

`combiner.rs:698-736` `run_one_cycle` already implements exactly that:
`combiner_color_in = [0.0; 3]`, `combiner_alpha_in = 0.0`, passed to
`resolve_color_input` for all four slots with the TEXEL-swap flag `false`.
Its own comment cites RT64 `run`'s zero-init. `Combined` resolves to that
zero today.

The ONLY thing refusing this draw is the admission gate
`CombinerProgramSlice::admits_color`, whose `carries_a_first_cycle_result()`
is false for `OnlyCycleOfOneCycleMode`. The evaluator downstream of that gate
handles the input correctly. This is a wiring defect, not a missing feature:
the gate is stricter than the evaluator it guards.

## Note on the reference lane's rule (read-only second opinion)

`fn64-render-reference/src/backend/validate.rs:403-404,476-478` refuses
COMBINED at `cycle_index == 0`, and it indexes by EVALUATION order
(`validate.rs:389-400`: `cycle_count` is 1 for one-cycle, `.take(cycle_count)
.enumerate()`). So the reference would refuse this program too. But the
reference also reads cycle 0's FIRST bitfield slice, while RT64 and fn64's
evaluator both read the SECOND slice for one-cycle mode. The two lanes model
one-cycle differently; RT64's pinned source is the authority this crate ports
against, and it is unambiguous. The reference's rule is not evidence that
hardware refuses this -- it is that lane's own unsupported-feature boundary.

## Result on the ROM

Recompiler rebuilt in-worktree at the fix HEAD. Two runs, fresh scratch each:

| run | scratch | vi_swaps | panics | render errors | ended on |
|-----|---------|----------|--------|---------------|----------|
| A | /private/tmp/cl-A | 2149 | 0 | 0 | 400,000-step cap |
| B | /private/tmp/cl-B | 2149 | 0 | 0 | 400,000-step cap |

They agree exactly. Baseline was 1,887 swaps aborting on the combiner
refusal; the run now passes that point and reaches the harness's own step
cap with `last render error: None`, so the wall is cleared rather than
moved. Audio keeps flowing (3,951 buffers / 4,142,576 samples).

### On the reported "825 regression"

An earlier reading of the post-fix log as 825 swaps was a mid-run PROGRESS
line, not a failure point: the harness prints `progress:` every 50,000
steps, and that run had been killed mid-flight when the session ended. The
same truncated log's last line already read `vi_swaps=1887` with zero
panics. The two clean runs above supersede it. No regression exists.

## What was NOT fixed

- **`combine` is still seeded post-fold** (`production.rs`'s
  `execute_raw_dpc` passes `self.rdp_state.combine()` after
  `plan_raw_dpc` applied the delta). This is a real latent instance of the
  same time-travel defect `f2c52822` and `d53e4835` repaired for the tiles
  and `other_mode`, and the struct doc still says so. It is NOT this wall:
  the failing draw was measured as one-cycle (`slices=OnlySecondSlice`), so
  no seeding change affects which selector it names. Left alone rather than
  changed speculatively -- the same standard the `other_mode` fix was held
  to, which waited for a measurement before widening.
- **`COMBINED` in cycle 0 of a TWO-cycle program** stays refused. RT64
  evaluates it (against the same zero-init), but no measurement in this
  repo covers that shape.
- **Fill cycle, RGB dither, two-cycle texrects in the wild** -- untouched
  pre-existing gaps, each already refused by name.
