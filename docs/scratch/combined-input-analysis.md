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
