# The versus-screen plateau is guest state 18, waiting on a player-ready check

Everything below marked CONFIRMED was read off a live run of the real ROM on
the all-Rust lane (fn64's own recompiler + `fn64-render-wgpu`, no `--features
rt64`), on the tree where the swap-1901 `lookup` trap is fixed
(`recompile_rom` emits 2,480 functions). Everything marked HYPOTHESIS is not.

## How the running code was identified

Two read-only harness probes, both applied to a scratch copy of
`~/Code/recomps/wm2000` (see `docs/tools/wm2000-versus-probe-harness.patch`):

- `WM2000_WATCH` samples guest half-words once per VI swap.
- `WM2000_CENSUS=<lo>-<hi>` counts every emitted function ENTERED inside a
  swap window, through the existing
  `fn64_abi::recompiled::copy_function_execution_destinations()`.

**Gotcha that cost an hour, worth keeping:** fn64 backs RDRAM word-swapped.
`Rdram::store_h` (`crates/fn64-cpu-runtime/src/runtime/host.rs:1014-1020`)
writes at `backing_offset(vaddr) ^ 2` with `to_ne_bytes`. A probe reading
`rdram[vram & 0xffffff]` big-endian therefore samples the guest variable at
`vram ^ 2`, byte-reversed. Read at `off ^ 2`, decode little-endian.

## CONFIRMED: the screen is guest state 18, and how it got there

WM2000's global screen id is `D_8003DD04` (`0x8003DD04`). `func_801255E4`
bounds-checks it against `0x22` and dispatches `jtbl_8016DAE8` at
`0x80125AB4`. Watching it under the proven lead-in (START at 1100, A every 100
swaps to 2400, then A every 60):

| swap | `D_8003DD04` | menu depth `D_8016EC24` | arm |
|---|---|---|---|
| 1318 | 0 -> 1 | 0 | |
| 1402 | 1 -> 5 | 0 -> 1 | |
| 1502 | 5 -> 3 | 1 -> 2 | |
| 1602 | 3 -> 4 | 2 -> 3 | |
| 1702 | 4 -> 10 | 3 -> 4 | |
| 1802 | 10 -> **19** | 4 -> 5 | `0x80125B8C` -- P1 select |
| 1902 | 19 -> **20** | 5 -> 6 | `0x80125B8C` -- P2 select |
| 2002 | 20 -> **17** | 6 -> 7 | `0x80126008` -> `func_80144A4C` |
| 2102 | 17 -> **18** | 7 -> 8 | `0x80126048` -- **the plateau** |
| 2520 | still 18 | 8 | through A at 2200/2300/2400/2460/2520 |

Every A press advances the state exactly two swaps later, which is why the
menus walked eight screens. At **state 18 that stops.** Two wrestler models on
screen at 2300 is states 19 and 20 having each committed a character.

## CONFIRMED: what state 18 polls

The arm at `0x80126048`:

```
80126048  lhu $v0, 0xA($s3)      # sub-state
8012604C  bnez -> .L80126074     # every frame after the first
80126058  sw 0x53 -> D_8011BC84  # first frame only: request scene 0x53
80126070  sh 1    -> 0xA($s3)
.L80126074:
80126074  jal func_8012BA94
8012607C  beqz $v0 -> .L8012608C # advance only when it returns 0
80126084  j func_801261D4        # else idle this frame
.L8012608C:
8012608C  lh $v0, 0x4($s3)
80126090  bnez -> .L801260A8
80126098  jal func_801456C8      # <-- reached EVERY frame
```

`func_8012BA94` (`0x8012BA94`, three instructions) returns the word at
`D_80161FF8`. The census settles which branch is live.

**Census over swaps 2600-2603 (14,652 entries, 201 distinct):**

| function | entries | meaning |
|---|---|---|
| `func_801255E4` | 4 | the state machine, once per frame |
| `func_8012BA94` | 4 | the poll, once per frame |
| **`func_801456C8`** | **4** | reached every frame, so the poll returns 0 |
| `func_80144A4C` | **0** | state 17's arm -- absent, so the state really is 18 |

So the countdown at `D_80161FF8` is NOT the blocker: it completes, and the
guest gets past it every single frame. The screen sits inside
**`func_801456C8`**.

## CONFIRMED: what `func_801456C8` is

`func_801456C8` (`0x801456C8`) is a **player-ready check**. It loops over four
player entries (`$a0 = D_801702A4 + 0x512`, stride `0x88`, `slti $a3, 4` at
`0x801457AC`):

```
80145708  lhu $v0, 0x16($a0)     # per-entry port field
8014570C  andi $v0, $v0, 0xF
80145710  beqz -> .L801457A4     # 0 => entry SKIPPED entirely
80145718..80145730                # index D_80095186[(field-1) * 12]
80145738  andi $v0, $v1, 0x8000  # A
8014577C  andi $v0, $v1, 0x4000  # B
80145748  andi $v0, $v0, 0x1000  # START, via D_8011C37E
801457B8  beqz $a2 -> .L801457F0 # nobody ready
801457F8  addiu $v0, $zero, -1   # return -1
```

`D_80095186` is the **per-controller-port button array, stride 12** -- the
same array `func_800E236C` (`0x800E236C`) merges into the global
`D_8011C37E`/`D_8011C380` words at `0x800E23EC..0x800E2418`, gated on the
controller count `D_800FEF2C` at `0x800E23A0`.

The caller stores the return in `$s1`; `-1` is the "not ready" answer that
leaves the state unchanged.

## What is NOT yet established

- **HYPOTHESIS**, not yet measured: which of the two exits of the loop is
  taken -- every entry skipped because its `0x16 & 0xF` port field is 0, or
  entries visited but `D_80095186[(field-1)*12]` never showing A. These are
  different defects and the probe distinguishing them is
  `WM2000_WATCH`/`WM2000_WATCHP` on `D_800FEF2C`, `D_80095186` and the four
  entry port fields.
- Whether fn64 populates `D_80095186` per port at stride 12 at all. fn64's
  `PifModel` answers the controller-read block, and `osContGetReadData`
  (`func_8002F788`) unpacks at stride 6 into `OSContPad[]`; `D_80095186` is a
  *game* array the game fills from those pads, so the question is whether the
  game's own fill loop runs for more than port 0.
- No claim is made here that a match was reached. It was not.

## CONFIRMED: fn64's controller path is NOT the defect

The array `func_801456C8` reads, `D_80095186`, is filled by the game's own
per-frame pad poll `func_80004628` (`0x80004628`), which:

- loops all four ports unconditionally (`sltiu $s1, 4` at `0x800049BC`),
- strides **12** bytes per port into `D_80095180` (`addiu $a1, $a1, 0xC` at
  `0x800049C4`) and 6 bytes per port through the `OSContPad[]` at
  `D_80057210`,
- lays out `+0x4` = held, **`+0x6` = pressed-this-frame**, `+0x8` = released,
- and **skips a port whose `OSContPad.errno` is nonzero** -- `lbu $v0,
  %lo(D_80057214)($v0)` at `0x80004928`, which zeroes that port's entry.

Watching `D_80095186` (port 0's pressed word) directly against the merged
global `D_8011C37E`:

| swap | `D_8011C37E` | `D_80095186` | script |
|---|---|---|---|
| 1102 | `0x1000` | **`0x1000`** | START press |
| 1103 | 0 | 0 | released |
| 1202 | `0x8000` | **`0x8000`** | A press |
| 1203 | 0 | 0 | released |
| 1302 | `0x8000` | **`0x8000`** | A press |

Port 0's pressed word is populated correctly, at the right stride, with the
right edge semantics, in the exact array the ready check reads. fn64's
`osContGetReadData_recomp` (`crates/fn64-abi/src/si/mod.rs:1113+`) models
`errno` deliberately: port 0 reports `errno == 0` with live input, ports 1-3
report `CONT_NO_RESPONSE_ERROR`, which is the correct emulation of a console
with one controller plugged in and is what makes the game's own poll zero
ports 1-3.

**So the plateau is not an input-delivery defect.** A is reaching the guest,
in the right place, in the right format, every time it is pressed.

## CONFIRMED: A is delivered during the plateau and the guest refuses it

The same watch, inside the plateau (state 18, entered at swap 2102):

| swap | `D_8011C37E` | `D_80095186` | `D_8003DD04` |
|---|---|---|---|
| 2102 | `0x8000` | `0x8000` | 17 -> **18** |
| **2202** | **`0x8000`** | **`0x8000`** | **still 18** |
| **2302** | **`0x8000`** | **`0x8000`** | **still 18** |
| **2402** | **`0x8000`** | **`0x8000`** | **still 18** |
| **2462** | **`0x8000`** | **`0x8000`** | **still 18** |
| **2522** | **`0x8000`** | **`0x8000`** | **still 18** |

Five presses inside the plateau. Port 0's pressed word carries A on every one
of them, exactly as it did at 1202, 1302, 1402, ... 2002, each of which
advanced the screen. The screen does not move. **The press is delivered and
refused.** Zero traps and zero panics across the whole run.

Two runs (`run3`, `run4`) produced this identical state ladder swap for swap,
so the sequence is deterministic and not a sampling artefact.

## CONFIRMED: why it is refused -- the menu graph is data-driven

`func_801261D4` (`0x801261D4`) is the state machine's epilogue, and it is
where every one of the eight advancing transitions actually happened:

```
801261D4  bgez $s1, .L80126608    # handler returned >= 0 -> no transition
801261DC  lhu  $v1, D_8011C37E    # else read the pressed word
801261E4  andi $v0, $v1, 0x9000   # A (0x8000) | START (0x1000)
801261E8  beqz -> .L80126384      # neither pressed -> done
801261F0  lw   $v0, 0x64($s2)
801261F4  lh   $a1, 0x12($v0)     # the menu descriptor's NEXT-SCREEN field
801261F8  bltz $a1, .L80126384    # <-- NEGATIVE => the press is DISCARDED
...
.L80126218:                       # the A path
80126258  sh   $v1, D_8016EC24    # push: menu depth += 1
80126270  sh   $v0, D_8016EB78[]  # push: remember the screen we came from
80126280  lw   $v0, 0x64($s2)
80126284  lh   $s1, 0x12($v0)     # the next screen id comes from THE SAME field
```

So WM2000's menu graph is not coded per screen -- it is **read out of a
descriptor at `$s2->0x64`, field `+0x12`**, and the same field both gates the
press (`bltz` at `0x801261F8`) and supplies the destination
(`0x80126284`). `func_80126288` then decodes it: `< 0x23` is a literal screen
id, otherwise the `0x7F00` bits pick an action.

Note that state 15's arm (`0x80126010`) compares this very field against
`0x41C` before advancing, and state 18's own arm reaches
`func_801456C8` -- the four-player ready check -- every frame, which returns
`-1` when nobody is ready and hands control to exactly this epilogue.

## What this makes the plateau

**HYPOTHESIS** (consistent with everything measured, not yet proven): state 18
is the "waiting for players to be ready" screen, its descriptor's `+0x12`
field is negative or names an action rather than a screen, and the transition
out of it is meant to be driven by `func_801456C8` finding a *ready* player
rather than by the epilogue's generic A handling. On a real console with the
same single controller, the same code would have to reach the same decision --
so the interesting question is which input `func_801456C8` accepts as "ready"
that a bare A press is not.

**What is ruled out, with measurements:** it is not input delivery (A reaches
`D_80095186` in the plateau), not the `D_80161FF8` countdown (the census shows
the guest clears it every frame), not a second controller (431 byte-identical
frames, prior card), not the analog stick (identical gfx rate and frames), and
not a recompiler trap (0 traps, 0 panics across every run here).

**No fn64 defect has been demonstrated.** Every fn64 mechanism this path
depends on -- the PIF controller-read block, `osContGetReadData`'s `errno`
discipline and swizzle, the per-port stride-12 array the game builds from it,
the state machine's own dispatch -- was measured working. That is a real
result: it moves the remaining question from "what is fn64 failing to model"
to "what does this screen actually want", and the next probe is a watch on
`$s2->0x64 + 0x12` itself.

## No match was reached

The furthest state reached remains the two-wrestler versus screen. The game
walks eight menu states on scripted A presses and stops at state 18.
