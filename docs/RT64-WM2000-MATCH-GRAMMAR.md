# WM2000's in-match grammar, and the search for a match-end signal

`RT64-WM2000-INPUT-GRAMMAR.md` reads the ROM's **menu** input path. This file
reads the **gameplay** path, because the two are different code with different
globals, and because the question "can a match be played to completion" needs
the second one.

Every claim below is marked CONFIRMED (instructions read, address quoted) or
HYPOTHESIS. Source is the NWXE disassembly at
`~/Code/aki-recomp/games/NWXE/disasm/asm/`.

## The gameplay input path is not the menu path (CONFIRMED)

The menu globals `D_8011C37C` (HELD) / `D_8011C37E` (PRESSED) / `D_8011C380`
(REPEAT) are a **derived, OR-ed copy** made by `func_800E24D4` for the
frontend. Gameplay does not read them: the gameplay overlay `D2720.s` (bank 4,
VRAM `0x800E1B90`) contains **zero** references to any of the three, and calls
`func_80004628` directly at `0x800E1C6C` and `0x800E1F74`.

The primary record is written by `func_80004628` (`1050.s`), with `$a1` set to
`D_80095180` at `0x80004918` and advanced `0xC` per port at `0x800049C4`:

```
/* 80004964 */  lhu   $v1, 0x0($a3)     # cur
/* 8000496C */  sh    $v1, 0x4($a1)     # +4 HELD
/* 8000497C */  sh    $v0, 0x6($a1)     # +6 PRESSED
/* 80004988 */  sh    $v1, 0x8($a1)     # +8 RELEASED
/* 80004998 */  sb    $v0, 0xA($a1)     # +A stick_x
/* 800049A8 */  sb    $v0, 0xB($a1)     # +B stick_y
/* 800049C4 */  addiu $a1, $a1, 0xC     # stride 0xC, 4 ports
```

**Probe addresses (CONFIRMED).** These are what `WM2000_WATCH`
(`docs/tools/wm2000-watch-patch.py`) is pointed at:

| Address | Meaning |
|---|---|
| `0x80095184` | port-0 HELD |
| `0x80095186` | port-0 PRESSED |
| `0x80095190` / `0x80095192` | port-1 HELD / PRESSED |
| `0x80166ED8` | per-player latch array, stride 8 (+0 HELD, +2 PRESSED), written ~`0x80126DF0` |

## The move grammar (CONFIRMED)

`func_8013EAD0` (`0x8013EAD0`) remaps physical to logical buttons through a
9-entry per-player config table `D_8009DD98` and the logical-bit table
`D_8014EEF0` (dumped from ROM offset `0x13FA80`:
`8000 4000 0004 0010 0020 0008 0001 0002`, with `D_8014EF00 = 0x2000` as the
catch-all). It rewrites the word in place (`sh $t1, 0x0($a1)` at `0x8013EBF8`).

`func_8013ECC0` (`0x8013ECC0`) is the move classifier. It walks an 8-deep
input history ring `D_80166EB0` (stride `0x2A`/player, shifted per frame by
`func_8013EBFC`) and emits an action-flags byte:

| Address | Mask | Flag | Meaning |
|---|---|---|---|
| `0x8013ED58` | `== 0xC000` | `0x11` | A+B together -- special |
| `0x8013ED4C` | held >= 8 frames | `0x1` | **charged** |
| `0x8013ED68` | `0x8000` | `0x4` | A -- weak grapple/strike |
| `0x8013ED84` | `0x4000` | `0x8` | B -- strong grapple/strike |
| `0x8013EDAC` | no A/B | `0x2` | neutral / tap-only |
| `0x8013EDA4` | `0x0F00` | `0x20` | D-pad directional modifier |
| `0x8013EEEC` | `0xC030` | -- | counter/reversal window (`func_8013EE44`) |

Two consequences for any scripted schedule:

- **A hold of 8 frames or more is a charge, not a repeated tap.** The committed
  lead-in's 10-swap holds are single charged moves. `wm2000-schedule.py` uses
  3-swap taps in its in-match phase for this reason, keeping two deliberate long
  holds as charges.
- The classifier reads a history *ring*, so it wants edge transitions -- which
  the harness's feed-on-change seam already produces.

`0x1000` (START) is pause/continue (tested against port-0 PRESSED at
`0x800E1DE8`, `0x800E1E18`, `0x800E1E54`, `0x800E3090`). `0x2000` (Z) at
`0x80126E94` gates a C-button check (`andi 0xF` on HELD at `0x80126EAC`), i.e.
Z is a modifier -- consistent with the menu-side finding that Z is never a
confirm.

**Correction to the older card.** `RT64-WM2000-INPUT-GRAMMAR.md` says "L|R
(`0x30`) is not menu input; all 12 sites reading it are gameplay/AI state".
The 12 sites it found are **not** input at all: `0x8000904C`, `0x80009054`,
`0x800090E0`, `0x80009154` in `1050.s` are the audio engine (`0x30` is a
pan/loop flag feeding `D_8005A924`), and the `809D0.s` hits are byte-field
tests at struct `+0x1A`/`+0x40`. L and R *do* participate in gameplay, but via
the counter mask `0xC030` at `0x8013EEEC` -- the right conclusion reached at
the wrong sites.

## How a match ENDS: FOUND, and it is pollable (CONFIRMED)

**Poll guest address `0x8016ED2A` (u8). While a match is running bit `0x80` is
clear; at match end it is set, and one frame later the gameplay loop exits.**

The chain, read end-to-end:

**1. The loop exit is gated on exactly that bit** (`0x800E1C9C`). This is the
only call to `func_800EE4AC` in `D2720.s`, and `$s4` is the register whose
being set ends the frame loop:

```
800E1C9C  lbu   $v0, %lo(D_8016ED2A)($v0)
800E1CA0  andi  $v0, $v0, 0x80
800E1CA4  beqz  $v0, .L800E1CC4      # not set -> skip the fade, keep playing
800E1CB4  jal   func_800EE4AC        # fade to FF,FF,FF
800E1CC0  addiu $s4, $zero, 0x1      # <- loop exit
```

(The other fade, `func_800EE550` at `0x800E1C90`, is gated on `$s5` and is the
fade-*in*.)

**2. Bit `0x80` has exactly one writer**, `func_80122AF4`. Every other write to
`D_8016ED2A` in the bank touches a different bit -- `0x800F0074`/`0x80122ABC`
clear `0x20` via `0xDF`, `0x800EF774` sets `0x20`, `0x80123F00` sets `0x10`,
`0x80122940` sets `0x40`, `0x801221B4` zeroes the byte:

```
80122AF4  lh    $v0, %lo(D_801589D2)
80122AFC  slti  $v0, $v0, 0x7530     # 30000
80122B00  bnez  $v0, func_80122C7C   # below 30000 -> do nothing
80122B18  ori   $v0, $v0, 0x80
80122B20  sb    $v0, %lo(D_8016ED2A)
```

**3. `D_801589D2` (`0x801589D2`, s16) is the post-match sequence counter**,
ticked once per state-machine tick in `func_801229E0` (the state machine is
`func_801226A0`, called from the frame loop at `0x800E1CD0`):

```
801229E0  lhu   $v0, %lo(D_801589D2)
801229EC  addiu $v0, $v0, 0x1
801229F4  sh    $v0, %lo(D_801589D2)
80122A00  bne   $v0, 0x5A, ...       # at frame 90 -> func_800E3A24(D_801589E0)
80122A1C  slti  $v0, $v1, 0x5B       # < 91 keep counting
80122A40/A48                         # else force $v0 = 0x7530 -> immediate end
```

It is reset to 0 at `0x80122950` and set from `$a2` at `0x80122AD4`.

**Do not confuse it with `D_801589D0`** (adjacent, +0), which is incremented at
`0x80122758` and compared `slti 0x1F` (31) -- an intro/entrance timer, not the
match clock. Both are zeroed at `0x801226E0`/`E4` on state entry.

`D_801567B0`/`B2` (thresholds `0xA`/`0x14`, referenced by `func_800E9D8C`) is a
referee-count / display index. It never touches `D_8016ED2A`, so it is not the
loop terminator.

Both per-frame ticks the previous pass suspected are display code, not rules:
`func_800E4C94` is a 2D HUD/sprite interpolator (4 slots, stride `0x24`, over
`D_8015862E..D_80158649`, decrementing a per-slot tween countdown at
`0x800E4FE0`), and `func_800E9C50` draws one glyph, ramping `D_801581E5` toward
`0x6E`. Neither writes a terminal flag.

## Earlier candidates, refuted

Recorded so the next reader does not re-read them. Three candidate leads were
followed before the real one above was found, and all three are something else:

| Candidate | What it actually is | Evidence |
|---|---|---|
| `D_80166E62` | pause/system-menu selection code | `func_800EB284`: `== -1` clears it; low 6 bits are a menu-item id (`andi 0x3F`, `slti 0x20` at `0x800EB504`, `slti 0x14` at `0x800EB530`); bits `0x700` a sub-mode; high byte a controller index (`0x800EB490`-`0x800EB498`) |
| `D_80167230` bit `0x4` | debug-controller display toggle | write gated on `D_8008DC00 & 0x400/0x800`; toggled by a button press (`andi 0xFB` / `ori 0x4`) at `0x80126F3C`-`0x80126F68`. Engine state is not toggled by input. |
| `D_80166E64` | "entrance/cutscene running" flag | `lh $v0, %lo(D_80166E64); bnez` at `0x800E1DB8` only **skips controller polling**. Set 1 at `0x800E5DB8` and `0x800EA2B0`, cleared at `0x800E78C4` when the entrance list hits sentinel `D_8014C7F0`. Does not exit the loop. |

Also not found, with the negative evidence:

- **Pin / 3-count as such:** no counter compared against 3 in a per-second-tick context.
  The 68 `slti`/`sltiu ...,0x3` sites in `D2720.s` are loop bounds and mode
  comparisons (`D_801586F2 < 3` at `0x800E7864` is a match-*type* check).
- **Submission meter:** not found.
- **Health / spirit meter globals:** not found; the HUD draw code was not reached.
- **Match time limit:** grep of `D2720.s` for `0x1770`, `0x4650`, `0x8CA0`,
  `0x2710`, `0x1C20`, `0x3840`, `0x7080` returns **zero hits**. If a match
  timer exists it is not initialized from an immediate in this bank.

### The loop structure that a match-end must pass through (CONFIRMED)

```
0x800E1B98  sw    $zero, %lo(D_80153F10)($at)   ; clear
0x800E1B9C  jal   func_800E1BC0                 ; run the whole overlay
0x800E1BA8  lw    $v0, %lo(D_80153F10)($v0)
0x800E1BAC  bnez  $v0, .L800E1B98               ; nonzero = restart the overlay
```

`D_80153F10` (`0x80153F10`) is written `=1` at `0x800E1D18` only when
`func_800E2FE4` returns 3 -- a pause-menu "restart match" selection. The inner
gameplay frame loop `.L800E1C6C` (`0x800E1C6C`-`0x800E1F34`) exits on register
`$s4`, which is set only by the fade-completion helpers `func_800EE4AC` /
`func_800EE550`.

That hypothesis is the one the section above confirmed: the match end does run
exactly that fade, and `D_8016ED2A & 0x80` is its gate.
