# 0x801226A0 is not a bank-overlap case: it is an uncovered entry in bank 4

The WM2000 all-Rust run died deterministically ~171 swaps past the versus
plateau, at swap ~2483:

```
panicked at crates/fn64-cpu-runtime/src/runtime/host.rs:549:
lookup: no recompiled function or host shim at vram 0x801226A0
```

This card corrects the diagnosis recorded in `RT64-WM2000-VERSUS-PLATEAU.md`
("NEW, and only reachable past the plateau") and `RT64-WM2000-INPUT-GRAMMAR.md`
("Three targets remain live traps ... bank-overlap cases"), and fixes it.

## CONFIRMED: what 0x801226A0 is in each bank

Three overlay banks map this vram (`~/Code/aki-recomp/games/NWXE/overlays.json`).
Every ROM offset below was derived by hand from that table
(`rom_start + (vram - vram_text)`) and matches the disassembly's own comment.

| bank | slot | vram_text | rom of 0x801226A0 | what the disassembly shows |
|---|---|---|---|---|
| 2 | B | `0x8011C900` | `0x79130` | `alabel func_801226A0`, `andi $v0, $v0, 0x80` -- a mid-function branch target inside `func_80122558` (`asm/73390.s:6610`) |
| 3 | B | `0x8011C900` | `0x86770` | `alabel func_801226A0_bank3_text`, `beqz $v0, .L801226B8` -- a different instruction, also mid-function (`asm/809D0.s:6716`) |
| **4** | **A** | **`0x800E1B90`** | **`0x113230`** | **`glabel func_801226A0`, size `0x5E8`** -- a real function entry (`asm/D2720.s:73605-74010`) |

Bank 4's body is a textbook function: `lui $v1, %hi(D_801589D6)` /
`addiu $sp, $sp, -0x18` / `sw $ra, 0x10($sp)`, a `jtbl_80151970` dispatch, and
`lw $ra, 0x10($sp)` / `jr $ra` / `addiu $sp, $sp, 0x18` at `0x80122C7C`.

**The caller is in the same bank.** `0x800E1CD0` lives in `asm/D2720.s:103` --
bank 4's file. Its word `0x0C0489A8` decodes by hand to
`((pc+4) & 0xF0000000) | ((0x0C0489A8 & 0x03FFFFFF) << 2) = 0x801226A0`.
So the call is **intra-bank and unambiguous**: no residency question arises.

The same holds for `0x80122F2C` (bank 4 `glabel`, size `0x610`, called by
`0x800E1F4C` whose word `0x0C048BCB` decodes to `0x80122F2C`).

`0x80127D54` is genuinely different: it appears ONLY as an `alabel` in bank 3
(`asm/809D0.s:12844`) and has no `glabel` anywhere. It is untouched by this
card and remains open.

## CONFIRMED: the real defect is a gap in the symbol dump

`syms/dump.toml`'s `bank4_text` section jumps straight from
`func_80122458` (size `0x248`) to `func_80122C88`:

```
0x80122458 + 0x248 = 0x801226A0     <- where the missing entry begins
0x801226A0 + 0x5E8 = 0x80122C88     <- exactly the next declared entry
```

A scan of all 884 bank-4 entries finds 31 inter-entry gaps. Twenty-nine are
4-to-12 bytes -- ordinary alignment padding. Exactly two are whole missing
functions: `0x801226A0` (`0x5E8`) and `0x80122F2C` (`0x610`).

The `glabel`-only symbol harvester dropped them. Because the SAME vrams are
mid-function `alabel`s in banks 2 and 3, the earlier lane searched those banks,
found ambiguity, and filed all three addresses as bank-overlap cases needing
residency. Bank 4's `glabel` was never checked.

## Why the existing machinery could not see them

`swallowed_entries::cross_check_region` reported a `jal`-proven root only when
it fell INSIDE some declared function's range. A root claimed by no function
hit an explicit skip:

```rust
// A `jal` target outside every declared function is a different
// defect (an entirely unmapped region), not this one. Left to the
// existing coverage reporting.
continue;
```

That "existing coverage reporting" never fired, so these two were invisible at
build time and surfaced only as a runtime trap 2,483 VI swaps into a run.

## The fix: the uncovered-entry class

A swallowed entry is hidden inside a preceding function's declared `size`;
repairing it means SPLITTING that function, which is why it needs the
head-returns precondition. An uncovered entry is claimed by nobody, so
adopting it takes bytes from no declared function and cannot corrupt a live
body. The risk profile differs, and so does the safe precondition:

* the root must sit exactly at the gap's start (otherwise preceding words
  would stay unclaimed -- a different, unmapped-region defect); and
* the gap must END by returning: `jr $ra` + delay slot, with only `nop` tail
  padding after it, so the adopted entry does not run off its own end.

Anything else is REPORTED, never adopted. `classify_gap_adoption` and
`apply_gap_adoptions` in
`crates/fn64-cpu-runtime-codegen/src/swallowed_entries.rs` implement this;
`recompile_rom` runs it alongside the split repair and renders both.

## MEASURED on the real ROM

```
uncovered-entry cross-check: 3 entry/entries claimed by no declared function,
                             2 adopted, 1 reported only
total functions: 2480 -> 2482
bank-ambiguous vrams: 20 (40 bodies)   <- UNCHANGED
```

* **adopted**: `0x801226A0` (gap `0x801226A0..0x80122C88`) and `0x80122F2C`
  (gap `0x80122F2C..0x8012353C`) -- both gap ranges equal to the sizes derived
  by hand above.
* **refused**: `0x800400CC` in `main_1050`, mid-gap in a `0xC1CC` span -- a
  word in a data table that merely decodes as a `jal` target. The
  "root must be at the gap start" precondition is load-bearing on real data,
  not just in fixtures.

The bank-ambiguous count is unchanged at 20, which independently confirms
these two were never bank-ambiguous: only one bank declares an entry at each,
so the ordinary flat `LOOKUP_TABLE` carries them.

In the emitted crate the call site changed from
`lookup(0x801226A0)` to
`call_host_or_recompiled(0x801226A0, func_801226A0_uncovered, ctx, mem)`,
and the body is 378 instructions (`378 * 4 = 0x5E8`) beginning
`Lui { rt: 3, imm: 32790 }` = `lui $v1, 0x8016` -- bank 4's body, as required.

## What remains

* `0x80127D54` is still undispatchable. Unlike these two it has no `glabel` in
  any bank, so it is not this class and is not fixed here.
* The uncovered-entry check is not WM2000-specific and should be run across the
  corpus; SM64's 332 swallowed entries suggest this class may also be present
  at scale.

## Verification

| suite | result | baseline |
|---|---|---|
| `cargo nextest run --workspace --offline` | **8637 passed, 13 skipped** | 8627 / 13 |
| `cargo nextest run -p fn64-render-wgpu --features host-gpu-tests --offline` | **4872 passed, 3 skipped** | 4872 / 3 |

The +10 on the workspace suite is this card's new tests.

### Tests, with fail-before / pass-after

All in `crates/fn64-cpu-runtime-codegen/src/swallowed_entries.rs`. Fail-before
was verified by restoring the silent `continue` in `cross_check_region`; each
of these then FAILS, and PASSES with the change in place.

| test | what it pins |
|---|---|
| `a_jal_proven_root_in_a_gap_is_reported_as_uncovered_and_adopted` | the WM2000 shape: reported, adopted, neighbours undisturbed |
| `a_jal_target_inside_no_declared_function_is_reported_but_not_swallowed` | rewrite of the test that asserted the OLD silent-skip behaviour |
| `an_uncovered_gap_that_never_returns_is_refused` | the terminator precondition |
| `an_uncovered_root_that_is_not_at_the_gap_start_is_refused` | the real `0x800400CC` refusal shape |
| `a_gap_whose_trailing_words_are_live_instructions_is_refused` | nop-tail-padding, at the one reachable shape |
| `the_gap_ends_at_the_nearest_declared_function_not_the_farthest` | `gap_end` uses `min` |
| `the_gap_starts_at_the_nearest_declared_function_below_the_root` | `gap_start` uses `max` |
| `adoption_declines_when_the_range_is_already_claimed` | the overlap guard |
| `a_root_inside_a_declared_function_is_swallowed_not_uncovered` | the two repair classes stay disjoint |
| `the_diagnostic_names_uncovered_entries_and_their_evidence` | the build-time report |

### Mutation results

Seven mutants, all KILLED: drop `NotAtGapStart`; always-accept; tail-padding
check -> `true`; adopt refused entries; `gap_end` `min` -> `max`; `gap_start`
`max` -> `min`; drop the overlap guard.

Four of those survived the first test pass, every one because the fixture
sampled a point where the correct and incorrect answers coincide:

* the tail-padding mutant was never even evaluated -- an earlier guard already
  refused each fixture. The reachable shape (return at `gap_end - 12`, a live
  word after its delay slot) was found by exhaustively comparing the real and
  mutated classifier over all small gaps built from `{jr $ra, nop, live}`.
* the `gap_end` and `gap_start` mutants needed two declared neighbours on the
  relevant side; every fixture had one, so `min` and `max` agreed.

One mutant is EQUIVALENT and was removed rather than papered over: the second
"only nop may precede the end" guard in `classify_gap_adoption` could not
change any answer (same exhaustive comparison). Untestable code that looks
load-bearing is its own hazard, so it is deleted with the reasoning kept in a
comment.

## The class is not WM2000-specific, and finding it exposed a second defect

Running the new check across the corpus:

| ROM | uncovered found | adopted | refused | total functions |
|---|---|---|---|---|
| WM2000 (NWXE) | 3 | 2 | 1 | 2482 |
| SM64 (SM64U) | 154 | 8 | 146 | 4247 |
| OoT (OOTU) | 0 | 0 | 0 | 13361 |

The refusal rate is the interesting number. SM64 refuses 146 of 154 -- almost
all are mid-gap words inside large unmapped spans that merely decode as `jal`
targets, exactly like WM2000's `0x800400CC`. The precondition is doing real
work; a "split on any proven root" sweep would have corrupted 146 ranges.

### The second defect: section names are not unique

The first SM64 run reported **"154 entries ... 805 adopted"** -- more
adoptions than entries found, which is impossible, with total functions
inflated from 4247 to 5044.

`repair_symbol_dump` scoped each section's findings by NAME
(`filter(|e| e.region == section.name)`). SM64's dump declares **154 sections
all called `_main`** (plus `_engine` x8 and `_goddard` x19), so every
same-named section received every other's findings and applied them at its own
ROM offsets.

Entries now carry `region_index`, and repairs scope on that. The name is kept
for reporting only. The pre-existing split path shared the flaw but was masked:
a split also has to match a containing function by name AND vram, which a
wrong section rarely satisfies.

This is worth keeping as a general caution: **anything in this codebase that
keys on a config section name is wrong.** Only the index is unique.

## MEASURED on the real ROM: the blocker is cleared

One solo run on an otherwise idle host, its own scratch root, its own trace
path, `WM2000_PORTS=2` with the proven lead-in (START at 1100, A every 100
swaps to 2400, then every 60):

```
WM2000_PORTS=2 FN64_RENDER=wgpu FN64_ABSENT_N64DD=1 FN64_NO_AUDIO=1
WM2000_INPUT_SCRIPT="1100..1110:1000;1200..1210:8000;...;3540..3550:8000"
WM2000_STOP_AT_SWAP=3600 WM2000_MAX_STEPS=6000000
```

The harness reports the stack it is on, and it is the intended one:

```
[fn64-stack] recompiler : fn64-cpu-runtime (FN64_RECOMP=rs, typed-Rust whole-ROM crate)
[fn64-stack] renderer   : wgpu
```

| | before (recorded in RT64-WM2000-VERSUS-PLATEAU.md) | after |
|---|---|---|
| outcome | `lookup: no recompiled function or host shim at vram 0x801226A0` | **no trap** |
| swap | died at ~2483 | **2842 and still running** (359 swaps past) |
| traps | 1 | **0** |
| panics | 1 (+ SIGABRT teardown) | **0** |

`grep -c "lookup: no recompiled|panicked at"` over the whole log is **0**, and
neither `0x801226A0` nor `0x80122F2C` appears in it at all.

The emitted crate the run was built from is byte-identical to what the final
tree produces (`diff -rq` over `src/` is empty), so the binary measured is the
binary this card ships.
