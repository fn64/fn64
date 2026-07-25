# Handoff: full static recompilation of the AKI family

Written 2026-07-25. Baseline: `main` at `26e375e`.

## Why this family

Five ROMs share one engine, and discovery is measurably complete on all of them:

| ROM | env var | static residue |
|---|---|---:|
| WWF No Mercy (NW4E) | `FN64_DISCOVER_NW4E_ROM` | 0 |
| WWF WrestleMania 2000 (NWXE) | `FN64_DISCOVER_NWXE_ROM` | 0 |
| Virtual Pro Wrestling 2 | — | 0 |
| WCW vs. nWo World Tour | — | 0 |
| WCW-nWo Revenge | — | 0 |

Proving the recompilation path once should generalise across all five, which is
what makes this the right family rather than the right ROM. WM2000 already boots
on the Rust pipeline + RT64, so there is a working baseline to measure against.

**Read the zero honestly.** "Static residue 0" means *every byte the code detector
classified as code has an admitted load address*. It does NOT mean all code was
found. For four of these five, `code_like` is 0.0% — the detector found nothing
unmapped, rather than finding and proving everything. `high_entropy` (compressed
or asset payload, never examined as code) is 36–54% of each of these ROMs.

The strongest single piece of evidence is not the static zero but an independent
one: a boot capture of **No Mercy** observed **0** code loads outside the static
mappings — two unrelated methods agreeing. That result does not extend to the
whole family. The same capture on **WCW World Tour** found **26** uncovered code
loads at `0xa21940+`. They agree with the static island `delta_vote` now proves,
so this is very likely already closed — but nobody has re-run the capture since
that landed, so treat WCW as unconfirmed rather than clean. Captures are ~20 s
boots, so even No Mercy's zero proves boot coverage only, not gameplay.

## Where the work actually is

Discovery is not the bottleneck. Composition and proof are:

```
NW4E  6 banks, 53,378 proven blocks, 22,958 reachable destinations
      exact_aot 0 | block_aot 22,215 | dynamic_mips 732 | unsupported 11
NWXE  5 banks, 38,194 proven blocks, 19,909 reachable destinations
      exact_aot 349 | block_aot 17,642 | dynamic_mips 1,898 | unsupported 20
```

`unsupported` is the release blocker and must reach zero. It is **11 and 20
destinations** — an enumerable punch-list, not a research problem. `gate_closure`
prints the concrete VAs (`unsupported_destinations=[...]`).

### Task 1 — Close `unsupported` on NW4E and NWXE

31 destinations total. Each is a concrete, statically-resolved transfer landing
outside every known mapping or in proven data. Get the address list from
`gate_closure`, classify each, and fix or explain it.

### Task 2 — Run closure on the other three AKI titles

`gate_closure` (`crates/fn64-discover/src/bin/gate_closure.rs`) is hardcoded to
NW4E / NWXE / OoT via a `RomSpec` list. VPW2, WCW World Tour and WCW Revenge have
**never been measured** — they compose, but nobody has their closure numbers.

The composition machinery is already generic: `physical_banks(facts)` reads proven
`RomMapping` facts with no game-specific input, and `compose_materialized_banks_v1`
plus `closure::scoreboard` are library functions. Only the per-game `RomSpec` list
is hardcoded. Wiring this generically also fixes `exec_bytes = 0`, which is the
state for every ROM through the normal CLI path.

### Task 3 — Emit and run

The block lane has **never executed an AKI title**. `examples/wm2000-block-boot`'s
documented expected outcome is still a typed fault naming the first un-admitted
destination. Whole-ROM `BlockProgram` emission is open (`docs/RECOMP-RS-COVERAGE.md`).

## What NOT to redo

All measured negatives; re-attempting them wastes a session:

- **Untabled static PI-DMA operand recovery.** At the interesting call sites the
  vrom/size operands are read from the very table the scan would need. Built,
  measured, reverted. Locating the PI *primitive* is separate, works, and landed.
- **Extending mapping extents / truncation repair.** Extents are read verbatim
  from explicit u32 fields (`banks.rs:755,765`) and cannot be short. Several
  Kirby records share one `vram_dest`, so extension would be actively wrong.
- **Pointer-table and mid-span container detection.** VA-shaped words are 0% of
  residue spans; embedded container magics run 0–1 per megabyte.
- **Yaz0 decompression as a general technique.** Zelda-only: OoT 1,456 blocks,
  MM 2,033, every other corpus ROM zero — including all five AKI titles.
- **Debugger-based PI DMA length capture.** `read_pi_regs` returns `0x7F`
  unconditionally for the length registers, so length is unrecoverable through
  the public debugger path. The core-side emitter is the only route and exists.

## Environment

Six env vars, all required together to reproduce `expected_closure`:

```sh
export FN64_DISCOVER_NW4E_ROM=/Users/jer/Code/aki-recomp/games/NW4E/nomercy.z64
export FN64_DISCOVER_NWXE_ROM=/Users/jer/Code/aki-recomp/games/NWXE/wm2000.z64
export FN64_DISCOVER_OOT_ROM=/Users/jer/Code/aki-recomp/refs/oot-decomp/baseroms/ntsc-1.0/baserom.z64
export FN64_DISCOVER_NW4E_DUMP=/Users/jer/Code/aki-recomp/games/NW4E/syms/dump.toml
export FN64_DISCOVER_NWXE_DUMP=/Users/jer/Code/aki-recomp/games/NWXE/syms/dump.toml
export FN64_DISCOVER_OOT_DUMP=/Users/jer/Code/aki-recomp/games/OOTU/syms/dump.toml
```

The other AKI ROMs live in `~/Downloads`. Note `WWF No Mercy (E) (V1.1) [!].z64`
there is **PAL** and is NOT the ROM the NW4E answer key was built against — using
it makes even `gate_loaders` mismatch.

`gate_corpus_homology` mismatches under this env and that is pre-existing: it
needs ~17 donor-ROM vars, not these six. The script exits on first mismatch, so
run `gate_closure` directly to check that one.

## Working agreement

- Branch from `origin/main`; never work on `main` itself, and never leave a
  worktree holding it.
- The shared stash stack is used by other sessions. Never bare `git stash` /
  `git stash pop`.
- `scripts/gate-determinism.sh` pins gate stdout digests and CI does not run it
  (no ROMs). Before moving a pin: reproduce the OLD digest under the env above
  first, then diff full stdout, then confirm the new value across ≥3 runs.
- **Stacked PRs are squash-merged, so `MERGEABLE/CLEAN` is not "safe to merge".**
  Always `git diff --stat origin/main origin/<branch>` before merging a stacked
  PR — a stale base will silently revert already-merged work. This nearly
  happened once.

## Verification

```sh
cargo test -q --workspace
cargo clippy -q -p fn64-discover --all-targets --no-deps -- -D warnings
python3 scripts/lint-docs.py
```

Success for this handoff is: `unsupported = 0` on NW4E and NWXE, closure numbers
existing for all five AKI titles, and `exec_bytes > 0` through the generic path.
