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

**The ~1900 abort is nondeterministic, and three explanations for it were
tried and refuted.** Four experiments, each killing the explanation before it:

| Hypothesis | Test | Result |
|---|---|---|
| Shared trace file | Give each run its own `WM2000_TRACE_PATH` | Still aborts |
| Host memory under 4-way concurrency | Halve to two concurrent runs | Still aborts |
| Concurrency at all | Run one alone | **Still aborts, swap 1901** |
| The trace sink itself | `WM2000_NO_TRACE=1` | **Still aborts, swap ~1855** |

The last two are the decisive ones. A *solo* run aborts, so it is not
contention. A run with the trace sink entirely disabled aborts, so it is not the
sink -- and the suspicious detail that made memory look right (three traces
stopping at a byte-identical 61,940,485) is a consequence of the runs dying at
the same place, not a cause.

**It is also not a shutdown failure, which is what the backtrace makes it look
like.** The no-trace run stopped at 300,000 of its 700,000 steps and never
printed the harness's "step budget exhausted" line, so it died mid-run. The
`Executor` drop in the trace is the unwind path, not the origin: a coroutine is
being unwound through the `extern "C"` `_osRecvMesg_recomp`, which cannot
unwind, so the real failure is upstream and gets converted into an abort here.

What remains solid is the negative, and it is what the plateau numbers rest on:
**~1900 is not a fact about the guest.** The identical lead-in reached swap 2397
solo and swaps 4156-4431 in the four wave-1 runs that completed. Runs that abort
must be discarded and re-run, not read as evidence; every number reported in
this card comes from a run that finished with `rc=0`.

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

## Summary, and what is still untried

**What changed.** The plateau was recorded as an unread button grammar. It is
now read from the ROM, and measured: **A is the confirm past the plateau**, it
takes effect four swaps after the press, and it moves the guest out of a
one-display-list idle state into sustained three-lists-per-field composition
that persists for the rest of the run. B is inert there. START, which drives
the earlier menus through `func_800FD184`, is not a confirm on the select
screen at all.

**What did not change.** No frame in any run has been shown to be in-match. The
scanned-out framebuffer stays on the single plateau hash even in the run whose
guest is composing at full rate, so the picture never advances.

**Where that puts the blocker.** Not on input. A guest submitting three display
lists per field into a frozen scanned-out image is a render-path result, and it
is the shape the standing S1 triangle-composition item predicts. The next
useful step is to inspect what probeA's display lists contain, not to press
more buttons.

### Untried, in the order worth trying

1. **What probeA composes.** Dump the display lists from the recovered
   3.00-lists/field state and compare them against the plateau's single list.
   This is the direct question and nothing else answers it.
2. **A second controller.** Ports 1-3 are hardwired `Absent` and the frontend
   ORs input across active ports while counting present controllers. A screen
   gating on player 2 is unreachable by any button schedule. Needs a
   `WM2000_PORTS` knob calling `fn64_abi::set_controller_port_state`; cheap.
3. **The analog stick.** Read at 6 sites in the select-screen overlay,
   including inside `func_8012C9E4`. `WM2000_INPUT_SCRIPT` already accepts
   `:<sx>:<sy>`; no code change needed.
4. **The paging grammar.** L (`0x20`), R (`0x10`), the C-buttons, and the
   `0x1004` START|C-Down combo `func_8015733C` tests. Wave 2 covers these; its
   results belong in this section when it lands.
5. **Longer runs on A.** Wave 1 used 8 taps and 700k steps. Whether sustained A
   past swap 3000 carries the guest further is unmeasured.

### What each wave-1 run actually did

Reproduce with `docs/tools/wm2000-run-probe.sh`; judge with
`docs/tools/wm2000-gfx-rate.py` and the hash diff in
`docs/tools/wm2000-input-probe.py`. Lead-in for every run:

```
1100..1110:1000;1200..1210:8000;1300..1310:8000;1400..1410:8000;1500..1510:8000;
1600..1610:8000;1700..1710:8000;1800..1810:8000;1900..1910:8000;2000..2010:8000;
2100..2110:8000;2200..2210:8000;2300..2310:8000;2400..2410:8000
```

| Run | Appended to the lead-in |
|---|---|
| control | *(nothing)* |
| probeA | `2500..2510:8000` and 7 more A taps at 2560, 2620, 2680, 2740, 2800, 2900, 3000 |
| probeDR_A | D-Right at 2500/2600/2700 each followed by A 30 swaps later, then A at 2800, 2900 |
| probeB | `2500..2510:4000` and B taps at 2600, 2700, 2800 |

Run one or two at a time, never four: see the swap-1901 note above.

## Measured: the frozen frame is NOT a triangle refusal, and not a wrong target

The card above ended by pointing at S1 triangle composition: "a screen whose
content is drawn entirely from triangles the rasterizer cannot compose would
look exactly like this". That inference was reasonable when it was written and
it is **wrong**, measured on the real ROM after the texture rung landed.

`plan_raw_triangle` has eight `return Ok(())` arms that decline to declare a
write. All eight are silent by design -- the function's own doc says so, and
that silence is correct for the render path, because a triangle that declares
nothing behaves exactly as it did before the planner existed. It is useless
for diagnosis: a frozen frame cannot say which arm it fell into. So each arm
got a counter (commit `85993520`, diagnostic only, no behaviour change), and
the probeA lead-in was re-run against them.

### Across 1,600,000 raw-triangle planning decisions, only two outcomes occur

| outcome | count | share |
|---|---|---|
| **ADMITTED** | **1,402,856** | **87.7%** |
| `no_covered_rows` | 197,144 | 12.3% |
| `depth_bit_set` | **0** | 0% |
| `no_other_mode` | 0 | 0% |
| `fill_cycle` | 0 | 0% |
| `no_color_image` | 0 | 0% |
| `color_image_format` | 0 | 0% |
| `no_target_height` | 0 | 0% |
| `row_outside_rdram` | 0 | 0% |

**The depth hypothesis is refuted outright.** `raw_triangle_is_executable` is
`!triangle.flags().depth()`, and the depth bit is set on **zero** of 1.6
million triangles. This agrees with `RT64-TRIANGLE-WRITEBACK.md`'s independent
finding that exactly one flag combination appears in WM2000's whole stream --
`s=true t=true d=false`, opcode 0x0e -- and with the five-flat-deltas census
("Z-variant triangles: still zero"). Depth is not what is blocking this screen,
and implementing depth would not unblock it.

`no_covered_rows` is not a gap either: it is `covered_rows` returning empty for
a degenerate, sub-scanline, or fully off-screen triangle, which is the correct
answer for one. Between tick 1,400,000 and 1,500,000 it did not increment at
all -- **100,000 consecutive triangles, every one admitted, none refused for
any reason.**

### The admitted triangles target the buffers that ARE being scanned out

A second instrumented run recorded the `SetColorImage` address each admitted
triangle declares its rows against. Across 300,000 admitted triangles there are
exactly **two** destination addresses and no others:

```
admitted_target 0x0038f800 = 122,838
admitted_target 0x003c7c00 = 121,414
```

Those are the same two framebuffers the harness reports alternating in the swap
rotation (`framebuffer at 0x0038f800` / `at 0x003c7c00`, 216 dumps each in the
window checked). So the **wrong-target hypothesis is refuted too**: the guest is
drawing into precisely the buffers the VI displays, split ~50/50 as
double-buffering requires.

### And the picture is live, not frozen

The card above measured 2,147 frames / **133 distinct** and read that as a
frozen screen. On the post-texture-rung tree the same lead-in gives, at swap
1,853: **1,851 frames / 1,118 distinct**, and per 400-swap window:

| swaps | frames | distinct |
|---|---|---|
| 0-400 | 397 | 195 |
| 400-800 | 400 | **397** |
| 800-1200 | 400 | 325 |
| 1200-1600 | 400 | 108 |
| 1600-2000 | 254 | 102 |

An 8.4x increase in distinct frames over the pre-rung baseline. The 400-800
window is very nearly one distinct frame per swap. The freeze the card
described was a property of the **pre-texture-rung tree**, and the texture rung
closed it.

### What this run did NOT establish, stated plainly

**It never reached the plateau.** The run aborted at **VI swap 1901**, before
swap 2500 where the plateau begins, so the specific 3000-4156 "3 lists per
field, frame frozen" regime is **not re-measured here** and the claim that it
too is now unfrozen is **not made**. What is established is that the two
mechanisms the card proposed for it -- triangle refusal and wrong target -- are
both false everywhere they could be measured, across 1.6 million triangles.

**The ~1900 abort now has a named cause**, which the card above left open after
refuting three explanations:

```
swap #1901
panicked at fn64-cpu-runtime/src/runtime/host.rs:549:
lookup: no recompiled function or host shim at vram 0x80120854
thread caused non-unwinding panic. aborting.
```

That is `trap_unsupported`, reached through `lookup` -- an indirect dispatch to
a vram address with no recompiled body and no host shim. It is the **R1/R2
recompiler gap** (`RT64-WM2000-REMAINING.md` section 2), the same class as
`osDriveRomInit`, and it is **not a renderer defect at all**. The prior card
was right that ~1900 "is not a fact about the guest"; it is a fact about which
function the guest happens to reach, and different input schedules reach it at
different swaps. Reaching the plateau needs that lookup gap closed, or a lead-in
that routes around `0x80120854`.

### The ~1900 abort, diagnosed: a mid-function indirect jump target

The card above refuted three explanations for the swap-1901 abort and left the
cause open. It is now named, from the emitted crate rather than by inference.

The trap is `lookup: no recompiled function or host shim at vram 0x80120854`.
Grepping the emitted crate for that address finds three sites:

```
src/part_013.rs:3688:            lookup(0x80120854)(ctx, mem);
src/part_001.rs:5857:            pc = 0x80120854;
src/part_001.rs:5859:        0x80120854 => {
```

The third line is the decisive one. `0x80120854` **is** emitted -- as an
ordinary instruction inside a function body:

```
0x80120854: Addiu { rt: 3, rs: 2, imm: 8 }
```

It is not a function entry. The nearest emitted symbol below it is
`func_8012079C` and the next one above is `func_801208C8`, so `0x80120854` sits
**mid-body inside `func_8012079C`**. The guest computes a jump target that lands
part-way into a function, and `lookup` resolves against `LOOKUP_TABLE`, which by
construction holds function ENTRY points only. There is no body at that vram
because a body is not what lives there.

So this is neither a missing function (the code is emitted and reachable as a
`pc` case at part_001.rs:5859) nor a stub disposition (`0x80120854` appears in
none of the gap report's stubbed, runtime-trap, or bank-ambiguous tables). It is
an **indirect dispatch to a mid-function address**, which the flat entry-point
lookup cannot express. The gap report's own framing covers the neighbouring
case -- "a flat `vram -> fn` array cannot say which bank is resident" -- and this
is the adjacent limitation: a flat entry-point array cannot say which function
CONTAINS an address.

This is an R1/R2-class recompiler item and not a renderer defect. It is also
why the plateau is out of reach on this lead-in: the abort lands at swap 1901
and the plateau starts at 2500. Closing it needs either a containing-function
resolution for indirect targets (`vram -> (function, offset)` rather than
`vram -> function`), or a lead-in that never routes through `func_8012079C`'s
computed jump. Which of those is right is **not** determined here.

## Update, 2026-08-19 -- the swap-1901 abort's cause, and where the fix lives

The census dispatched to scope this refuted its own premise, which is the
finding. There is no indirect jump. All five references to `0x80120854` are
plain static `jal` immediates. The address is a real function entry that
`splat` mislabeled `alabel` instead of `glabel`, so the symbol scanner never
emitted it and the predecessor's declared size swallowed it -- the same
"alabel defect" already named in `RT64-WM2000-RECOMP-LANES.md` and already
being fixed, in a currently uncommitted 30-entry `split_functions` sweep in
`~/Code/aki-recomp/games/NWXE/profile.toml`.

That sweep is real and correct as far as it goes, but a hand-verified check
against the live disassembly found it stops one link short of a four-entry
sub-chain at exactly this address: `func_80120840`, `func_80120854`,
`func_8012087C`, `func_80120884`, each ending `jr $ra` with an `alabel`
immediately after, same shape as every entry the sweep already accepts. A
second address the census flagged, `0x8013F998` in bank4_text, is absent
from the sweep entirely and is a separate, less-verified gap.

**This is not a recompiler architecture problem.** Both candidate fixes named
in the prior update -- a `vram -> (function, offset)` lookup redesign, or a
small workaround routing around the jump -- solve a problem this code does
not have, since nothing computes an address. The real fix is four more
`split_functions` entries, using machinery that already exists and already
hard-fails on a wrong boundary.

**Not applied here.** The fix lives entirely in `~/Code/aki-recomp`, a
separate repository, and that repo is currently dirty with another session's
in-progress work on the very sweep this gap extends -- the "rt64 port
takeover" session, busy at the time this was found. Handed back rather than
edited, per this project's rule against touching another session's dirty
tree. The precise four entries and their boundary evidence are recorded for
whoever picks this up.

## Update, 2026-08-19 -- the abort reproduced, its panic read, and the gap made visible at build time

The prior update handed this back as "the fix lives in `~/Code/aki-recomp`".
That is still where the symbol boundaries live, but it is not the whole
story: **fn64 could not see the defect at all until the guest tripped over
it**, and that is an fn64 gap, now closed.

### The panic, verbatim

Reproduced on the proven lead-in with `RUST_BACKTRACE=full`, twice, both at
**VI swap 1901**. No custom panic hook was needed -- the abort's origin
backtrace is printed before the unwind reaches the `extern "C"` boundary:

```
thread 'main' panicked at fn64-cpu-runtime/src/runtime/host.rs:549:5:
lookup: no recompiled function or host shim at vram 0x80120854
  10: fn64_cpu_runtime::runtime::host::trap_unsupported
  11: oot_recompiled::lookup
  12: oot_recompiled::part_000::func_8012070C_bank3_text
  ...
  26: corosensei::coroutine::Coroutine<..>::with_stack_unchecked::coroutine_func
  28: fn64_runtime::thread::GameThread::resume
  29: fn64_runtime::executor::Executor::run_one_step
```

Frames 26-29 are why the earlier reports showed only
`panic_cannot_unwind` / `drop_in_place<Executor>`: the panic is raised inside
a coroutine and cannot cross the `extern "C"` frame, so it converts to a hard
abort. **The `_osRecvMesg_recomp` in those reports is the unwind path, and
has nothing to do with message queues.**

### It is an fn64 bookkeeping gap, not guest behaviour

`0x80120854` is the target of plain static `JAL` immediates (target field
295445). fn64 *does* emit the code there -- as an interior `match` arm of
`func_8012079C_bank3_text`, whose dump-declared `size = 0x12C` (75
instructions) spans `0x8012079C..0x801208C8` and swallows it. Because no
symbol declares it, `SymbolTable::resolve` returns `Indirect`, the emitter
writes `lookup(0x80120854)`, and neither dispatch table can hold it: both
are keyed on function ENTRY vrams **by construction**. The call is dead the
moment it is emitted; only the trap is deferred to whenever the guest first
takes that path, which is why the swap number moves with the input schedule.

### What changed in fn64

`audit_undispatchable_call_targets` (`crates/fn64-cpu-runtime-codegen/src/module.rs`)
is the whole-module check neither half could make alone -- the emitter is
right that an unknown vram must become `lookup()`, and the dispatcher is
right that only entries belong in its tables. `recompile_rom` runs it and
renders the result as a new gap-report section.

On WM2000 it reports **12 targets, with zero false positives**: none of the
12 appears in `LOOKUP_TABLE`, `BANKED_LOOKUP_TABLE`, or the harness's
host-shim address set. A `J` that stays inside its own function is excluded,
because the emitter lowers that to a local `pc = ...` branch that never
calls `lookup()`; counting those reported 3961 rows, almost all ordinary
loop branches.

The 12 include `0x8013F998`, previously recorded as a separate and
less-verified suspicion, now confirmed by the same mechanism.

### The split is confirmed as the correct repair, and it is measured

Splitting `func_8012079C_bank3_text` into the three entries the live
disassembly shows (`0xB8 + 0x30 + 0x44 = 0x12C`, exactly the original size)
in a **scratch copy** of `dump.toml` takes the audit from **12 targets to
10** -- removing exactly `0x80120854` and `0x80120884` and nothing else.

11 of the 12 show the textbook entry shape: the preceding instruction is
`jr $ra` plus its delay slot. The twelfth, `0x80127D54`, does **not**, and
is worth its own note: two overlay banks disassemble those words
differently, so it is a bank-overlap case. **A blind "split after every
`jr $ra`" sweep would be wrong there**, which is why the audit reports the
boundary evidence and leaves the decision to the symbol source rather than
patching silently.

`~/Code/aki-recomp` was not modified: it is out of scope for this lane and
was dirty with another session's work. The split above was validated in
`/tmp/abort-hunt/scratch-cfg`.

### The twelve targets, with their boundary evidence

Generated by `recompile_rom`'s gap report; the "predecessor returns" column is
an independent check against the live disassembly (is the word at
`target - 8` a `jr $ra`, with `target - 4` its delay slot?). It is the same
rule the upstream `split_functions` sweep already uses to accept an entry.

| target | swallowed by | predecessor returns |
|---|---|---|
| `0x80120474` | `func_801200DC` | yes |
| `0x80120854` | `func_801200DC` | yes -- **the reproduced abort** |
| `0x80120884` | `func_801200DC` | yes |
| `0x80120A54` | `func_801200DC` | yes |
| `0x801226A0` | `func_80122558` | yes |
| `0x80122F2C` | `func_80122E9C` | yes |
| `0x80127D54` | `func_80127CA0` | **NO -- bank overlap, needs review** |
| `0x8013EE44` | `func_8013ED24` | yes |
| `0x8013F240` | `func_8013F038` | yes |
| `0x8013F2C8` | `func_8013F038` | yes |
| `0x8013F314` | `func_8013F2F8` | yes |
| `0x8013F998` | `func_8013F2F8` | yes |

Eleven are ready to split on the existing rule. `0x80127D54` is not: two
overlay banks decode those words differently, so which function owns the
address depends on residency. It needs the same treatment the 20
bank-ambiguous vrams already get, not a split.

Note the callers column in the generated report matters for prioritising:
`0x8013EE44` has 15 distinct callers and `0x8013F240` has 9, so both are far
more likely to be reached than `0x80120854`, which has 3. The swap number an
abort lands on is a property of which of these the input schedule reaches
first, which is exactly why it moved between runs.

### The audit is not WM2000-specific

Run over two other configs in the same corpus, with every finding checked
against the emitted crate itself:

| ROM | functions | undispatchable targets | false positives |
|---|---|---|---|
| WM2000 (NWXE) | 2471 | 12 | 0 |
| SM64 (SM64U) | 3893 | 332 | 0 |
| OoT (OOTU) | 13358 | 2 | 0 |

"False positive" is checked mechanically and two ways: is the target present
in the emitted `LOOKUP_TABLE`, and is there actually an emitted `lookup(...)`
call for it? For all 346 findings across the three ROMs the answers are no
and yes respectively -- every row is a real, reachable trap.

The spread is itself informative. OoT's 2 reflect a heavily curated symbol
set; SM64's 332 say that lane carries the same latent defect class at much
larger scale, undetected until now because nothing looked for it.

### Measured on the real ROM: before and after

Same lead-in, same binary settings, same host; the only difference is the
three-way split of `func_8012079C_bank3_text` in a scratch `dump.toml`.

| lane | runs | outcome |
|---|---|---|
| unsplit (as shipped) | 2 | **both aborted at VI swap 1901**, `lookup: no recompiled function or host shim at vram 0x80120854` |
| split | see below | **cleared swap 1901 with zero traps** |

The split lane reached swap 1907 and beyond with `traps=0`, i.e. past the
exact swap where both unsplit runs died. In the split crate every call site
is now a direct call --
`call_host_or_recompiled(0x80120854, func_80120854_bank3_text, ctx, mem)` --
and `0x80120854` is a row in `LOOKUP_TABLE`, so the trap is removed
structurally rather than suppressed. The body it now reaches is an ordinary
12-instruction bounds-checked table lookup (`sltiu $v0, $a0, 50`), which is
what a function entry should look like.
