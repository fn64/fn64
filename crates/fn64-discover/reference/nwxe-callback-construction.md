# NWXE callerless functions with proven address construction

The six NWXE answer functions that no `jal` reaches anywhere in the 32 MB
ROM, but whose addresses ARE assembled in code from split `lui`+`addiu`/`ori`
immediate pairs. A 32-bit pointer scan cannot see these; only immediate
tracking can.

Each row names the function that builds the address and the exact site, so a
mechanical verifier has a specific claim to confirm and a trace run has a
specific place to look.

| callback | constructed inside | site VA | note |
|---|---|---|---|
| `0x80000460` | `func_80000400` (ROM entry point) | `0x8000042c` | boot path installs it |
| `0x80028ba0` | `func_80025FE8` | `0x80026108` | two sites |
| `0x80028ba0` | `func_80029BD0` | `0x80029cf0` | |
| `0x8002c290` | `func_8002C054` | `0x8002c074` | |
| `0x8002c290` | `func_8002CA70` | `0x8002cac8` | next insn stores it to memory |
| `0x8002ceb0` | `func_8002BF9C` | `0x8002bfb4` | |
| `0x8002db80` | `func_8002C1EC` | `0x8002c20c` | |
| `0x8002dfa0` | `func_8002BB04` | `0x8002bb54` | next insn stores it to memory |

Shape evidence that these are a real API rather than linker residue: bodies
run 0x140-0x500 bytes, save six to eight callee-saved registers, and read
arguments past `$a3` off the incoming stack (`lw $8,104($sp)`). Two
constructions are immediately followed by `sw`, which is a callback being
installed into a struct or table.

`func_8002DB80` also builds `0x00200440` and `0x00200580` by `lui`+`ori`;
those are not code addresses and read as DMA lengths or DMEM offsets, which
would place it in a hardware or microcode lane.

## Trace verdict (3,000,000 executed-PC records, WM2000 boot, 2026-08-03)

A real mupen64plus DEBUGGER=1 capture of the boot path settles two of the
six. 5,282 distinct PCs executed, spanning 0x80000180..0x800385f8.

| callback | entry hits | body hits | verdict |
|---|---|---|---|
| `0x80000460` | 1 | 19 | **CONFIRMED CALLED** -- exactly as the entry-point construction predicted |
| `0x80028ba0` | 12 | 4,896 | **CONFIRMED CALLED** -- hot code, not dead |
| `0x8002c290` | 0 | 0 | not reached in boot |
| `0x8002ceb0` | 0 | 0 | not reached in boot |
| `0x8002db80` | 0 | 0 | not reached in boot |
| `0x8002dfa0` | 0 | 0 | not reached in boot |

So the hypothesis lane works: a hunch directed a mechanical check, the check
named construction sites, and execution confirmed two of them. Neither
confirmation is a proof that the site's target set is closed -- an observed
edge is existence, never exhaustiveness -- but both are sound callable roots.

## Second capture: 40,000,000 records with driven input (2026-08-03)

A 13x longer capture with a deterministic controller schedule (Start/A
across the intro and menu sequence) reached 17,271 distinct PCs spanning
0x80000180..0x8011f674 -- into the overlay banks, versus 5,282 PCs ending at
0x800385f8 in the boot capture. The control confirms the extra depth is
real: func_80028BA0 went from 12 entries / 4,896 body instructions to **300
entries / 122,426**.

All four tabled functions still show **entry=0, body=0**.

**STILL TABLED (4 of 6), now with stronger evidence.** They are not simply
"past the boot screens" -- a capture that reaches deep into overlay code and
executes their neighbours 300 times does not touch them at all. Candidate
explanations, none yet tested:

- a mode this input schedule never selects (a specific match type, options
  screen, or unlockable path);
- a two-player or peripheral-gated path (the capture ran controller 1 only,
  with nothing plugged into ports 2-4);
- an error/recovery path taken only on a condition the capture never
  produced (pak removal, save corruption, disconnect);
- genuinely unreachable in the shipped build, with the construction sites
  being dead stores in code that itself never runs.

**Discriminator run, and it settles the shape.** Do the CONSTRUCTING
functions themselves execute in the 40M capture?

| constructor | builds | entries |
|---|---|---|
| `func_80000400` (ROM entry) | 0x80000460 | 1 |
| `func_80025FE8` | 0x80028ba0 | 1 |
| `func_80029BD0` | 0x80028ba0 | **0** |
| `func_8002BB04` | 0x8002dfa0 | **0** |
| `func_8002BF9C` | 0x8002ceb0 | **0** |
| `func_8002C054` | 0x8002c290 | **0** |
| `func_8002C1EC` | 0x8002db80 | **0** |
| `func_8002CA70` | 0x8002c290 | **0** |

Every constructor of a confirmed callback ran; every constructor of a tabled
one did not. The callbacks are therefore NOT installed-but-uninvoked -- the
whole `0x8002bb04..0x8002ca70` neighbourhood is gated upstream and never
entered. That is one question ("what enables this subsystem?"), not four,
and it is the right thing to answer before capturing again.

## M5 classifier: first measured assessment (2026-08-03)

`classify_callerless` (bin, commit `1de2c1f`) emits a typed hypothesis plus a
machine-checkable verification plan per callerless function. On NWXE's 66:

| hypothesis | count |
|---|---|
| `unclassified` | 42 |
| `libc_like` | 12 |
| `global_accessor` | 8 |
| `struct_field_writer` | 2 |
| `device_register_access` | 1 |
| `float_math` | 1 |

**Independently verified.** `func_80000648` classifies as `GlobalAccessor` for
`0x8004c29c`; decoding its three words from raw ROM bytes confirms exactly
that address (`lui 0x8005` + `lw -15716`).

**The verification plan holds at scale, contrary to a first impression.**
`func_80000648`'s own global is touched by no external code, which briefly
looked like the plan failing on its best case. Measured across all 18
getter-shaped functions in the resident image, **16 of 18 have their global
touched by external code** -- so "whoever else touches this global is the
caller neighbourhood" does produce leads; `func_80000648` is the outlier.

**Honest limits.** 42 of 66 are `unclassified`, which is the correct default
when nothing fires but leaves most of the worklist without a directed lead.
`LibcLike` (12) is the weakest class -- small byte-copy loops are genuinely
ambiguous and "find a byte-identical twin" is a fallback, not a lead. Of the
six split-constructed functions, only two classify at all, and both as
`LibcLike`.
