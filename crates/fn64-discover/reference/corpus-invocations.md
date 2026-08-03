# Canonical corpus invocations for `gate_decomp_functions`

The boot-bank function-boundary gate is env-driven. These are the canonical
per-game invocations that produce the graded corpus numbers (README
"Discovery corpus" scoreboard). Set `REF=crates/fn64-discover/reference` and
point the ROM/DUMP variables at your local assets (ROMs are not in-repo;
answer keys live in the `aki-recomp` corpus checkout under
`games/<CODE>/syms/dump.toml`).

Every run must end `wrong=0` — a nonzero `wrong` is a regression, never a
tolerable trade for recall.

OoT's entry-argument file contains its remaining cited callback anchors.
`osCreateThread` implementations and `$a2` thread entries are discovered
automatically from ROM bytes and proven boot-entry authority. MM needs no
entry-argument manifest: fixed-point snapshot composition follows the proven
boot call into `request_dma_0:0x80174bf0`, then its `jal Fault_Init` at
`0x80174c28`, derives `Fault_ThreadEntry`, and mechanically recovers the
intrusive callback registry contract. The grading binary composes the boot
bank with only the banks proven by its cited request-DMA scan; unrelated
materializable overlays do not enter that authority catalog.

## Ocarina of Time (OOTU, primary answer key)

```sh
FN64_DISCOVER_ROM="<oot-ntsc-1.0 .z64>" \
FN64_DISCOVER_DUMP=<corpus>/games/OOTU/syms/dump.toml \
FN64_DISCOVER_TABLES=$REF/oot-ntsc-1.0-load-tables.toml \
FN64_DISCOVER_REQUEST_DMA=$REF/oot-ntsc-1.0-request-dma.toml \
FN64_DISCOVER_ENTRY_ARGS=$REF/oot-ntsc-1.0-entry-args.toml \
FN64_DISCOVER_SIG_DONOR_ROM="<mm .z64>" \
FN64_DISCOVER_SIG_DONOR_DUMP=<corpus>/games/MMU/syms/dump.toml \
gate_decomp_functions
```

## Majora's Mask (MMU)

```sh
FN64_DISCOVER_ROM="<mm n64 us .z64>" \
FN64_DISCOVER_DUMP=<corpus>/games/MMU/syms/dump.toml \
FN64_DISCOVER_TABLES=$REF/mm-n64-us-load-tables.toml \
FN64_DISCOVER_REQUEST_DMA=$REF/mm-n64-us-request-dma.toml \
FN64_DISCOVER_SIG_DONOR_ROM="<oot n64 us .z64>" \
FN64_DISCOVER_SIG_DONOR_DUMP=<corpus>/games/OOTU/syms/dump.toml \
FN64_DISCOVER_SIG_DONOR_TABLES=$REF/oot-ntsc-1.0-load-tables.toml \
FN64_DISCOVER_SIG_DONOR_REQUEST_DMA=$REF/oot-ntsc-1.0-request-dma.toml \
gate_decomp_functions
```

## Super Mario 64 (SM64U)

```sh
FN64_DISCOVER_ROM="<sm64 us .z64>" \
FN64_DISCOVER_DUMP=<corpus>/games/SM64U/syms/dump.toml \
FN64_DISCOVER_REQUEST_DMA=$REF/sm64-us-request-dma.toml \
FN64_DISCOVER_ENTRY_ARGS=$REF/sm64-us-entry-args.toml \
FN64_DISCOVER_SCRIPT_PTRS=$REF/sm64-us-script-ptrs.toml \
FN64_DISCOVER_SIG_DONOR_ROM="<oot .z64>" \
FN64_DISCOVER_SIG_DONOR_DUMP=<corpus>/games/OOTU/syms/dump.toml \
gate_decomp_functions
```

(No `TABLES` for SM64.)

## Kirby 64 (K64U)

```sh
FN64_DISCOVER_ROM="<k64 us .z64>" \
FN64_DISCOVER_DUMP=<corpus>/games/K64U/syms/dump.toml \
FN64_DISCOVER_TABLES=$REF/k64-us-load-tables.toml \
FN64_DISCOVER_ENTRY_ARGS=$REF/k64-us-entry-args.toml \
FN64_DISCOVER_ADJUDICATED_ENTRIES=$REF/k64-us-adjudicated-entries.toml \
FN64_DISCOVER_SIG_DONOR_ROM="<oot .z64>" \
FN64_DISCOVER_SIG_DONOR_DUMP=<corpus>/games/OOTU/syms/dump.toml \
gate_decomp_functions
```

## WCW/nWo Revenge — the guard witness, graded WITHOUT a donor

Revenge is graded because it is the only ROM in this set that exercises
`sig_scan::admissible_entry_word`. That guard is inert on all six other
graded games -- identical output with it disabled -- but disabling it moves
Revenge from wrong=0 to wrong=4. A guard whose only witness sits outside the
gate set is how a wrong==0 violation stays invisible, so the witness is now
inside it.

It is graded with NO signature donor, and that is a measured decision rather
than an omission. 8-word boot-bank shingle Jaccard against the corpus
random-pair baseline (median 0.0025, p99 0.0176):

| pair | similarity |
|---|---|
| No Mercy <-> WM2000 (both late engine) | **0.0629** |
| Revenge <-> WM2000 | 0.0387 |
| Revenge <-> No Mercy | 0.0320 |

Revenge is a generation older than the late trio, so an AKI donor is
same-franchise but cross-generation, at roughly half the engine similarity
the late pair share. With the NW4E donor it grades **551/689 wrong=2**: the
lane adds 52 exact matches and two false splits, at `func_80028860` (split 8
words in) and `func_8002D524` (split 234 words in). Both split roots are
genuinely boundary-plausible -- one follows alignment padding, one follows a
real `jr $ra` -- so the boundary rule is behaving correctly and the donor
body simply matched an interior region of a larger target function.

Raising `MIN_SIGNATURE_WORDS` does not separate them: at 6 and at 8 the ROM
still grades wrong=2 while recall falls 551 -> 546 -> 543. These are long
matches at real internal boundaries, not short-body collisions, so the
existing degeneracy knobs are the wrong lever. Until a cross-generation
donor rule exists that admits the 52 without the 2, this ROM is graded
donor-free.

```sh
FN64_DISCOVER_ROM="<WCW-nWo Revenge (USA).z64>" \
FN64_DISCOVER_DUMP=<jessetbh-WCWnWoRevengeRecomp>/syms/dump.toml \
gate_decomp_functions
```

The answer key is jessetbh's WCWnWoRevengeRecomp `dump.toml` -- function
geometry only (name, vram, size), the measured-observation class AGENTS.md
permits. That project is GPL-3.0; none of its code or runtime is used.

## AKI wrestling engine — WM2000 (NWXE) and No Mercy (NW4E)

The AKI titles are one engine a year apart, so they donate signatures to
EACH OTHER. Generic argument-to-`jalr` dataflow now recovers callbacks passed
directly to reachable consumers; the same-engine donor still covers the
remaining struct-callback residual (GObj process callbacks reached through
caller-passed object fields without direct constant-argument evidence). The answer keys are
splat-autogenerated per game (aki-recomp `aki_profile.gen_symbols`), so
donor↔target circularity does not arise.

```sh
# NWXE (WM2000), donor = NW4E
FN64_DISCOVER_ROM=<corpus>/games/NWXE/wm2000.z64 \
FN64_DISCOVER_DUMP=<corpus>/games/NWXE/syms/dump.toml \
FN64_DISCOVER_SIG_DONOR_ROM=<corpus>/games/NW4E/nomercy.z64 \
FN64_DISCOVER_SIG_DONOR_DUMP=<corpus>/games/NW4E/syms/dump.toml \
gate_decomp_functions

# NW4E (No Mercy), donor = NWXE
FN64_DISCOVER_ROM=<corpus>/games/NW4E/nomercy.z64 \
FN64_DISCOVER_DUMP=<corpus>/games/NW4E/syms/dump.toml \
FN64_DISCOVER_SIG_DONOR_ROM=<corpus>/games/NWXE/wm2000.z64 \
FN64_DISCOVER_SIG_DONOR_DUMP=<corpus>/games/NWXE/syms/dump.toml \
gate_decomp_functions
```

No tables/claims files — the AKI boot banks are KSEG0-affine and run bare
apart from the donor.


## Expected grades (main @ cross-donor adoption)

| Game | matched_exact / total | wrong |
|------|----------------------:|------:|
| OoT  | 119 / 137  | 0 |
| MM   | 402 / 486  | 0 |
| SM64 | 2816 / 3030 | 0 |
| K64  | 402 / 531  | 0 |
| NWXE | 698 / 847  | 0 |
| NW4E | 835 / 985  | 0 |
| Revenge (no donor) | 499 / 689  | 0 |

## gate_rom_rebuild — Phase-8 whole-ROM byte-exact rebuild

No answer key, dump, donor, or claims file: the ROM byte is the oracle and
automatic discovery supplies every bank. Requires `mips-linux-gnu-{as,ld,objcopy}`
on PATH (macOS: a cross-binutils build; Homebrew formulae exist).

```sh
# Known-recomp proof target (Majora's Mask; zeldaret + Zelda64Recomp are the
# external ground truth for the DECOMPILE grade via gate_decomp_functions —
# this gate is the RECOMPILE proof and reads none of that).
FN64_DISCOVER_ROM=<roms>/mm-usa.z64 gate_rom_rebuild

# Novel proof target (corpus-picked by scripts/pick-novel-rom.py; no known
# decomp or recomp project exists for it).
FN64_DISCOVER_ROM=<roms>/"Clay Fighter 63 1-3 (USA).z64" gate_rom_rebuild
```

For corpus selection, set `FN64_REBUILD_REPORT` to retain each shortlisted
ROM's content-free `fn64.rom-rebuild-report.v1` receipt, then pass every
receipt back to `scripts/pick-novel-rom.py` with repeated `--rebuild-report`
arguments and `--selection-output`. The selector refuses incomplete, failed,
duplicate, or out-of-shortlist reports and ranks the complete set by absolute
`roundtripped_code_bytes`; this is why Clay Fighter's 811,944 bytes win even
though a smaller ROM can have a larger percentage.

The gate exits nonzero unless every attempted region round-trips byte-exact
AND the rebuilt image's sha256 equals the original's. Run it twice: the
reports must be byte-identical (assembly_text_sha256 included).

The physical-byte report is a checked, disjoint partition of
`header_ipl3_bytes`, `roundtripped_code_bytes`, and `opaque_bytes`; their sum
must equal `rom_bytes`, and accepted code overlapping header/IPL3 fails loud.
For the known target, the separate all-bank decompile grade is:

```sh
FN64_DISCOVER_ROM=<roms>/mm-usa.z64 \
FN64_DISCOVER_DUMP=<corpus>/games/MMU/syms/dump.toml \
FN64_DISCOVER_TABLES=$REF/mm-n64-us-load-tables.toml \
FN64_DISCOVER_REQUEST_DMA=$REF/mm-n64-us-request-dma.toml \
FN64_DISCOVER_SIG_DONOR_ROM=<roms>/oot-ntsc-1.0.z64 \
FN64_DISCOVER_SIG_DONOR_DUMP=<corpus>/games/OOTU/syms/dump.toml \
FN64_DISCOVER_SIG_DONOR_TABLES=$REF/oot-ntsc-1.0-load-tables.toml \
FN64_DISCOVER_SIG_DONOR_REQUEST_DMA=$REF/oot-ntsc-1.0-request-dma.toml \
FN64_DISCOVER_OVL_RELOCS=1 \
FN64_DISCOVER_GRADE_OVERLAYS=1 \
FN64_DISCOVER_JUMP_TABLES=$REF/mm-n64-us-jump-tables.toml \
gate_decomp_functions
```

Current result: 16,443 / 17,108 exact, 2 coarse/interior, 663 open,
and 0 wrong across the boot bank, 612 parsed overlays, and the code segment.

Expected results (2026-08-01, cold discovery, zero reference TOMLs):

| ROM | banks | regions | raw_words | roundtripped bytes | digest |
|---|---:|---:|---:|---:|---|
| Majora's Mask (USA) | 605 | all exact | 101 | 66,312 physical + 2,173,380 materialized | match |
| Ocarina of Time (USA) | 102 | all exact | ~3.7k | 18,904 physical + 2,559,588 materialized | match |
| Clay Fighter 63⅓ (USA) | 1 | 220/220 | 0 | 811,944 physical | match |
| Buck Bumble / Penny Racers / Lamborghini / RR64 | 1 each | all exact | — | 307,560 / 93,768 / 519,380 / 206,428 | match |
| Tom and Jerry (Europe) | 1 | all exact | — | 475,448 physical | match |
| Powerpuff Girls (USA) | 1 | all exact | — | 480,580 physical | match |
| Fighting Force 64 / Bass Hunter 64 | 1 each | all exact | — | 409,288 / 347,232 | match |
| Dual Heroes (USA) | 3 | all exact | — | 183,188 physical | match |
| MRC Multi Racing Championship (USA) | 10 | all exact | — | 748,088 physical | match |

`raw_words` counts words retained as numeric literals (out-of-region
branches, non-canonical encodings, embedded table data inside proven blocks)
— byte-exact either way, reported never hidden.

Full-corpus sweep (2026-08-01, 287 ROMs, cold): **284 pass byte-exact**,
111,014,792+ physical bytes regenerated through GNU `as`. The 3 failures are
snapshot-composition refusals (Banjo-Tooie, Donald Duck Goin' Quackers,
Rayman 2) — a discovery frontier, not an emission defect. Encodings gas
cannot express at `-mips3` are retained numerically by the gate's pre-pass:
`jalr rd==rs`, MIPS IV FPU conditional moves (`movz/movn/movt/movf .s/.d`),
and `bltzal`-family branches sourcing `$31`.

## gate_rom_recompile — generic whole-ROM CPU recompilation

The generic gate needs exactly one input and no per-game configuration:

```sh
FN64_DISCOVER_ROM=<rom.z64> \
FN64_RECOMPILE_REPORT=<scratch>/<label>.json \
  cargo run --release -p fn64-discover --bin gate_rom_recompile
```

It discovers banks, packs digest-bound block geometry, emits Rust, compiles
it with a real `rustc`, runs the result, and probes arbitrary guest PCs. The
pass criterion is the composed `HEADLINE unsupported=0`; per-bank
`unsupported` lines can be nonzero and still compose to zero, because a
destination unmapped in one bank may be resident in another.

### WWF WrestleMania 2000 (NWXE) — 2026-08-03, exit 0

First run of the GENERIC gate against any AKI title. Previously only
`gate_wm2000_recompile` (hardcoded to this game) had certified it.

```
composed_banks=5
HEADLINE unsupported=0 total_recompiled_exact_plus_block_aot_bytes=8188
exact_aot=110  block_aot=1937  dynamic_mips=19  unsupported=0
whole-ROM BlockPack v2: blocks=43032 words=223429
unsupported_punch_list=[]
```

Note `exact_aot=110` where the recorded `gate_wm2000_recompile` scoreboard
(2026-07-31, `docs/DISCOVER-PLAN.md`) reported 0 — discovery has improved
since, and the generic path is not weaker than the hand-configured one.

Boot bank alone reports `unsupported=3` and `recovered_overlay_2` reports 8;
both compose away. Reading a per-bank line as the verdict is a mistake the
HEADLINE exists to prevent.
