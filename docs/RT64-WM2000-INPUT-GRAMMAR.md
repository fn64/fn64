# WM2000's own input grammar, read from the ROM

The scripted-input plateau recorded in
[`RT64-WM2000-GAMEPLAY-GAP.md`](RT64-WM2000-GAMEPLAY-GAP.md) section 5 was
attacked by guessing buttons. This card replaces the guessing with the game's
own code: the controller path in the NWXE disassembly
(`~/Code/aki-recomp/games/NWXE/disasm/asm/`) was traced from
`osContStartReadData` down to the `andi` masks the menu screens branch on.

That disassembly has **no input-related symbols** -- `syms/dump.toml` names
~1900 functions `func_XXXXXXXX`, and the only real names anywhere
(`identified_libultra.toml`, 34 of them) are threading/VI/SI entries. No
`osCont*` function had been identified. The identifications below are from
reading the code, and each one carries the evidence that fixes it.

## The controller path

| Address | Function | How it was identified |
|---|---|---|
| `func_8002F824` | `__osPackReadData` | Writes `FF 01 04 01 FFFF FF FF` per port into the PIF buffer at stride 8, terminated `FE` -- the canonical controller-read command block, and byte-for-byte the block fn64's `PifModel` already answers (gameplay-gap card section 3.1). |
| `func_8002F700` | `osContStartReadData` | Calls `__osPackReadData`, then `__osSiRawStartDma`. |
| `func_8002F788` | `osContGetReadData` | Unpacks PIF into `OSContPad[]`, stride 6. |
| `func_8002F8E0` | `osContInit` | Writes `__osContNumControllers`; carries the `0x165A0BB` timer constant. |

`osContGetReadData` also confirms the struct offsets the harness's
`set_controller_state` feeds (`button` u16 at +0, `stick_x` s8 at +2,
`stick_y` s8 at +3):

```
/* 8002F7E0 */  lhu  $v0, 0x4($sp)
/* 8002F7E4 */  sh   $v0, 0x0($t1)      # -> OSContPad.button
/* 8002F7E8 */  lbu  $v0, 0x6($sp)
/* 8002F7EC */  sb   $v0, -0x1($a2)     # -> stick_x
/* 8002F7F0 */  lbu  $v0, 0x7($sp)
/* 8002F7F4 */  sb   $v0, 0x0($a2)      # -> stick_y
```

## The edge detector -- why a held button is not a press

`func_80004628` is the sole caller of `osContGetReadData`. At `0x80004964` it
turns the raw button word into the held/pressed/released triple the rest of
the game reads:

```
/* 80004964 */  lhu  $v1, 0x0($a3)     # cur  = pad->button
/* 80004968 */  lhu  $a0, 0x0($t0)     # prev = previous frame
/* 8000496C */  sh   $v1, 0x4($a1)     # [+4] HELD
/* 80004974 */  xor  $v1, $v1, $a0     # changed = cur ^ prev
/* 8000497C */  sh   $v0, 0x6($a1)     # [+6] PRESSED = changed & cur
/* 80004988 */  sh   $v1, 0x8($a1)     # [+8] RELEASED
```

`func_800E24D4` (frontend overlay, slot A) aggregates these across all active
ports -- OR-ed, so port 0 alone suffices -- into three globals. The read counts
are the useful part:

| Global | Meaning | Read sites |
|---|---|---|
| `D_8011C37C` | HELD | 14 |
| `D_8011C37E` | **PRESSED** | **96** |
| `D_8011C380` | **REPEAT** | **96** |

**Menus are driven almost entirely by PRESSED and REPEAT, not HELD.** Auto-repeat
has an initial delay of 4 frames (`addiu $t6, $zero, 0x4` at `0x800E2548`) and
then re-fires every frame.

The consequence for scripted input: a press must be a *transition*. The
harness's `WM2000_INPUT_SCRIPT` already produces transitions (it feeds the
seam only when the composed state changes), and its 10-swap holds separated by
90 neutral swaps clear the 4-frame repeat delay with room to spare. So the
plateau is **not** explained by the edge detector -- the presses were real
presses.

## What the plateau screen branches on

`func_8015733C` (`0x8015733C`, overlay bank 3) has the richest button grammar
in the ROM and sits directly under a 2-D grid navigator, which is what makes
it the strongest candidate for the observed wrestler-select-looking plateau.
All reads are of PRESSED unless marked REPEAT:

| Address | Mask | Button |
|---|---|---|
| `0x801573D8` | `0x800` (REPEAT) | D-Up |
| `0x801573E0` | `0x400` (REPEAT) | D-Down |
| `0x8015761C` | `0x200` (REPEAT) | D-Left |
| `0x80157624` | `0x100` (REPEAT) | D-Right |
| **`0x80157790`** | **`0x8000`** | **A -- primary confirm** |
| `0x80157898` | `0x4000` | B -- cancel/back |
| `0x801578A0`, `0x80157A0C` | `0x1` | C-Right |
| `0x80157930` | `0x2` | C-Left |
| `0x80157960` | `0x10` | R |
| `0x80157B6C` | `0x20` | L |
| `0x80157C84` | `0x8` | C-Up |
| `0x80157D20` | `0x1004` | START \| C-Down |

The grid walker `func_8015B2F0` moves a row/column cursor on the same D-pad
bits (`0x800`/`0x400` row, `0x200`/`0x100` column), confirming a cell grid.

**START is not a confirm on this screen.** `func_8015733C` tests `0x1000` only
inside the `0x1004` combo. The frontend state machine one layer up
(`func_800FD184`, `0x800FD184`) is where `0xD000` (A|B|START) and `0x9000`
(A|START) accept START -- which is exactly why Start-only navigation worked
through the earlier menus and then stopped mattering.

Two further notes that constrain the search:

- **Z is never a confirm.** Only two menu sites test `0x2000`
  (`0x801226DC`, `0x8012CA88`) and both read HELD, i.e. Z is a modifier.
- **L|R (`0x30`) is not menu input.** All 12 sites reading it are gameplay/AI
  state, not `D_8011C37C/E/380`. Menus test L and R separately.
- `func_8015C5F8` *writes back* to PRESSED at `0x8015C6C4`, masking `0x6FFF`
  and OR-ing in A or B -- a synthesized confirm. A screen fed by that path can
  be waiting on a precondition (an asset, a second player slot) rather than on
  a button at all.

## Consequence for the harness

The disassembly does not by itself say which screen the plateau is. What it
does is bound the search: on any select screen the confirm is **A**, motion is
the **D-pad**, back is **B**, and paging is **L**/**R** or a C-button. That is
the matrix `docs/tools/wm2000-input-probe.py` drives, and the measured results
of driving it are recorded alongside it.

**The analog stick is not ruled out.** A first pass concluded no menu code read
the stick globals. That was wrong, and counting the references is what corrects
it: `D_8011C382`/`D_8011C383` are read **8 times in the frontend overlay
(`4C160.s`)** and **6 times in the select-screen overlay (`809D0.s`)**, the
latter including `0x8012CE0C`/`0x8012CE2C` inside `func_8012C9E4` -- one of the
two functions that also tests Z as a held modifier. The grid *walkers*
(`func_8015B2F0` and its siblings) really do read only the D-pad, which is what
the first pass generalised from; the screens around them do not. The stick
stays in the untried space, and the harness already carries it
(`WM2000_INPUT_SCRIPT` takes optional `:<sx>:<sy>`).

## Ports 1-3 are permanently absent, and no script can change that

`func_800E236C` OR-s menu input across *active* ports, and the ROM tracks how
many controllers are present. fn64 models port 0 as `StandardControllerNoPak`
and ports 1-3 as `Absent` (`crates/fn64-runtime/src/si.rs:120`), which is the
honest default and what the gameplay-gap card measured the guest receiving.

`fn64_abi::set_controller_port_state` (`crates/fn64-abi/src/si/mod.rs:773`)
can change that -- but **the WM2000 harness never calls it**, and exposes no
env knob that would. So a screen that gates on a second player being plugged in
cannot be satisfied by any button script whatsoever, no matter how long the
matrix runs. That is a distinct, cheap, and entirely untried axis: give the
harness a `WM2000_PORTS` knob and re-run the same matrix with two ports live.

## Two harness collisions that must be cleared before any probe matrix

Both were measured, and both produce failures that look like guest behaviour.

**The scratch tree.** `run-rs-lane.sh` re-emits the whole-ROM crate and rsyncs a
scratch sibling tree on every invocation. Four parallel probes raced on it and
all four died on `cp: .../recomps/wm2000: File exists`, with zero frames
dumped. A probe matrix runs one fixed binary many times, so the build has to
come out of the loop; `docs/tools/wm2000-run-probe.sh` runs the prebuilt binary
only.

**The trace file.** The harness's trace sink defaults to one hardcoded path,
`/tmp/wm2000-boot-trace.jsonl`. Four concurrent runs wrote to that single file
and all four aborted at an **identical swap 1901**:

```
_osRecvMesg_recomp
core::panicking::panic_cannot_unwind
core::ptr::drop_in_place<Box<fn64_runtime::thread::GameThread>>
core::ptr::drop_in_place<fn64_runtime::executor::Executor>
thread caused non-unwinding panic. aborting.
```

Identical-across-runs is exactly the shape a real plateau has, which is what
makes this worth writing down: **an infrastructure collision can counterfeit
the very evidence a plateau claim rests on.** What separated them was a solo
run of the same script reaching swap 2397 without incident. `WM2000_TRACE_PATH`
gives each run its own sink.

Note that `WM2000_NO_TRACE=1` is not the fix, because the harness computes
`dumps_disabled = trace_disabled || ...` -- turning off the trace also turns off
the framebuffer dumps the probes are judged by. Per-run trace paths are the
only way to have isolation and pictures at once.

## Measured: the plateau screen reads input, and does not advance on it

Wave 1 of the matrix, four runs to ~700k steps (VI swap ~4200), each with its
own trace sink. Lead-in is the proven chain from the gameplay-gap card (START
at 1100, then A every 100 swaps to 2400). The control adds nothing at the
plateau; each probe taps one button from swap 2500.

Every probe is judged against the **set of every hash the control produced
anywhere**, so a screen that changes on its own cannot be credited to a button.

| Run | Input at plateau | Max swap | Novel frames (>=2500) | Which swaps |
|---|---|---|---|---|
| control | none | 4216 | -- | -- |
| **probeA** | **A, 8 taps** | 4156 | **17** | 2504-2513, 2520-2526 |
| **probeDR_A** | **D-Right then A** | 4431 | **7** | 2550-2556 |
| probeB | B, 4 taps | 4216 | **0** | -- |

Two things follow, and they point in opposite directions.

**The screen is live.** A produces frames the control never produces, and it
produces them **four swaps after the press** (press at 2500, first novel frame
at 2504) -- the same four-swap latency the gameplay-gap card measured for the
Start press that left attract. The input seam reaches this screen. Pixel-wise
the novel frames differ from the control across essentially the whole image
(76,640 of 76,800 captured pixels), as a palette/fade shift rather than a new
layout.

**It does not advance.** Every novel frame is inside a press window, and the
screen returns to the plateau hash `5d29bcadf69b` as soon as the button is
released. All four runs -- including the ones that produced novel frames -- sit
at **98% plateau hash** across the 1,700+ swaps after 2500, and every run's
late tail (swap >= 3500) is that one hash and nothing else.

So this is the "the button does something but does not transition" case, not
the "the button does nothing" case, and it is now distinguished by measurement
rather than assumed. **B is inert here** (0 novel frames), which is itself
informative: on `func_8015733C` B is the cancel, so a screen that answers A and
ignores B is not that function's select screen, or is in a state where cancel
is suppressed.

**No frame in any run has been shown to be in-match.** The novel frames are a
recoloured plateau, not a new screen.

## The frame hash was the wrong instrument: A *does* advance the game

Frame hashes say all four wave-1 runs are equally stuck. The **gfx-task rate**
says they are not, and it is the measurement that changes the conclusion.

Per-swap graphics tasks, from the harness's own `task_counts()` progress lines:

| swaps | control | probeA | probeB | probeDR_A |
|---|---|---|---|---|
| ..2460 | 3.01 | 3.01 | 3.01 | 3.01 |
| ~2800 | 1.50 | 1.28 | 1.50 | 1.43 |
| ~3200 | **1.00** | 1.87 | **1.00** | **1.00** |
| ~3500 | **1.00** | **3.00** | **1.00** | **1.00** |
| ~3850 | **1.00** | **3.00** | **1.00** | **1.00** |
| ~4200 | **1.00** | **3.00** | **1.00** | **1.00** |

Every run enters the plateau the same way: the rate collapses from a steady
~3.0 display lists per field to exactly **1.00**, which is the signature of one
static list being re-presented rather than a screen composing itself.

**Only probeA comes back out.** After its last A press (swaps 3000-3010) the
rate climbs 1.28 -> 1.87 -> **3.00** and holds 3.00 for the remaining ~650
swaps. The control, which pressed nothing, never leaves 1.00. Neither does B,
nor D-Right+A.

So A is not merely "doing something": it moves the guest from a one-list idle
state back into a three-lists-per-field composing state, and the guest stays
there. Audio is flat at 1.83 tasks/swap throughout and does not distinguish the
runs, so this is specifically the graphics pipeline waking up.

**And the framebuffer still does not change.** probeA's dumped frames from
swap 3000 to 4156 are 100% the single plateau hash `5d29bcadf69b`, while it
submits three display lists per field. Both framebuffers (`0x0038f800` and
`0x003c7c00`) remain in the swap rotation in every run, so this is not a
stalled buffer flip.

That combination -- guest composing at full rate, scanned-out image frozen --
is not an input problem. **The remaining gap is downstream of input**, in what
those display lists produce. Which is consistent with the standing renderer
blocker (S1 triangle composition in `RT64-WM2000-REMAINING.md`): a screen whose
content is drawn entirely from triangles the rasterizer cannot compose would
look exactly like this, live and frozen at the same time.

**Nonclaim.** This does not show a match was reached. It shows the input
grammar past the plateau is A, that A restores full-rate composition, and that
the reason no new picture appears is downstream of the button. Identifying the
screen probeA is composing needs the render path, not another button.
