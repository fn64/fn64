# fn64

**A Rust foundation for Nintendo 64 recompilation projects and native ports.**

fn64 connects ROM discovery, typed-Rust code generation, the N64Recomp ABI, a
libultra runtime, rendering seams, and executable evidence in one clean-room,
permissively licensed stack.

[Explore fn64](https://fn64.github.io/) ·
[Read the design](docs/DESIGN.md) ·
[See the roadmap](docs/ROADMAP.md) ·
[Contribute](AGENTS.md)

> [!IMPORTANT]
> fn64 is pre-alpha. Ocarina of Time boots through native recompiled code with
> RT64 rendering, audio-ucode execution, and input, but fn64 is not ready to
> ship a finished port. Open fidelity and release gates are tracked in public.

## Who fn64 is for

fn64 is for developers building N64 native ports who want to spend less time
rebuilding the infrastructure around a game:

- **Already using N64Recomp?** Link generated code against a strongly typed
  implementation of its ABI, backed by a Rust libultra runtime and device
  model.
- **Starting from a ROM?** Use evidence-carrying analysis to recover functions
  and sections even when a finished decompilation does not exist.
- **Trying to trust the result?** Keep ABI tests, instruction oracles,
  deterministic backends, and named release gates beside the code they grade.

fn64 does not contain game assets, ROM bytes, or recompiled-game output. Users
provide and recompile their own game content locally.

## What fn64 provides

```text
local ROM ──▶ discover ──▶ typed Rust runners ──▶ fn64 ABI + runtime
                                                        │
                                                        ▼
                                      reference or RT64 rendering ──▶ native app
```

The reusable stack covers function and section discovery, typed static
recompilation, scheduler and libultra services, DMA and overlays, input, save,
audio and RSP task dispatch, renderer integration, and certification. The core
stays game-agnostic.

## Why fn64

| | The promise | What that means here |
|---|---|---|
| **Future** | A reusable Rust foundation | Discovery, typed codegen, execution, the libultra runtime, rendering seams, and certification are designed to outlast any single port. |
| **Faithful** | Evidence over optimism | ABI smoke tests link a real C caller, instruction oracles compare emitted code, unsupported behavior traps loudly, and every known gap stays named. |
| **Friendly** | A project people can join | Humans and agentic contributors share one explicit contract; load-bearing docs preserve context; project code is MIT OR Apache-2.0. |
| **Fast** | Native execution with measurable headroom | Static recompilation produces native host code. Rust adds predictable memory behavior and zero-cost abstractions; performance claims require measurements. |

### ROM discovery without pretending every ROM is solved

`fn64-discover` recovers function and section metadata directly from a locally
supplied ROM. Structural analysis, load-image recovery, provenance-recorded
signatures, and owner-proof rules map executable code without guessing across
ambiguous boundaries.

Discovery fails honestly. Every unresolved bank keeps a named blocker, every
corpus run retains its misses, and grading against games with public answer
keys uses a strict `wrong == 0` posture. The cold, config-free CPU-recompile
gate reaches zero unsupported destinations on all five tested AKI titles,
including a first-run Japanese title with no decomp, answer key, or per-game
configuration. This is a promising result, not a universal-discovery claim;
the remaining cold-start frontier is documented in
[`docs/DISCOVER-PLAN.md`](docs/DISCOVER-PLAN.md).

### Rust is the mechanism, not the pitch

The goal is not to translate C syntax into Rust syntax. The goal is to encode
the invariants a runtime depends on: one runnable guest thread, typed RDRAM and
MMIO addresses, explicit queue ownership, deterministic device state, and
one-way crate dependencies. Where an invariant can be a type, it is. Where a
feature is missing, the runtime traps with the symbol and call context instead
of silently continuing with corrupted state.

Foreign code is bounded instead of scattered. The pure-Rust runtime can be
tested without a window or C++ toolchain, while optional RT64 integration is
quarantined behind one explicit crate and unsafe-code boundary.

## Start here

- Port builders: read the [architecture and migration plan](docs/DESIGN.md).
- Discovery contributors: inspect the [current discovery frontier](docs/DISCOVER-PLAN.md).
- Runtime contributors: start with the [ABI completeness ledger](docs/COMPLETENESS.md).
- Humans and agents: follow the same [operating contract](AGENTS.md).

## Architecture

One workspace, separate crates, each publishable alone:

| Crate | Role |
|---|---|
| `fn64-runtime` | Core: scheduler, OS message queues, timers, PI/SI/VI/AI plumbing, rdram model, overlays |
| `fn64-abi` | The `extern "C"` surface recompiled code links against |
| `fn64-boot-harness` | Shared generated-section bridge, registration callback, and RDRAM allocation for boot hosts |
| `fn64-shell` | The executable: window, input, audio out, ROM intake |
| `fn64-render-ir` | GPU/runtime-independent render semantics: bounded command streams, resource effects, replay records, and move-only completion ownership |
| `fn64-render` | Backend-neutral render seam, content-addressed ordered microcode admission, and raw-DPC completion inspection |
| `fn64-render-reference` | Deterministic pure-Rust `ReferenceBackend`, geometry/object decoders, software rasterizer, and VI reference path |
| `fn64-render-rt64` | FFI bridge to [RT64](https://github.com/rt64/rt64) (MIT, C++); all C++ interop remains quarantined here |
| `fn64-certification` | Executable cross-backend and native RT64 behavioral evidence gates |
| `fn64-cpu-runtime` | Linked typed execution runtime for generated VR4300 Rust runners |
| `fn64-cpu-runtime-codegen` | Build-side typed-Rust emitter and whole-ROM driver; absent from generated runners' runtime dependency graph |
| `fn64-recomp` | N64Recomp adapter used by the comparison lane |
| `fn64-audio` | RSP audio ucode execution |
| `fn64-diff` | The first-divergence comparator (pure; no I/O) |
| `fn64-discover` | ROM discovery: symbol/section metadata without a decomp |

`fn64-recomp` was once "planned, once the runtime earns it" — it exists and
boots OoT. See `docs/DESIGN.md` §1.1 for the two lanes it introduces.

## Status

Pre-alpha, but not speculative and no longer design-only: fn64 boots OoT with
the RT64 renderer, real audio ucode, and input, in a windowed shell. The
graded target is **Ocarina of Time** — the zeldaret decomp's 10,833 named
functions make it a rare answer key to grade discovery and recompilation
against. (The original proving ground was a WM2000/No Mercy port effort on the
AKI engine; that lineage is why `aki-recomp` appears in older docs. It is a
legacy checkout fn64 is cutting loose — see ROADMAP Phase H.)

fn64 and the reference runtime link the *identical* recompiled code, so every
fn64 behavior gets A/B'd against reality before the swap. The boot ladder is
the test suite; it does not grade on a curve. What is NOT done is tracked
honestly in `docs/ROADMAP.md` — audio is still broken (R5) and the outdoor
gameplay eye-gate is unmet (R3b).

The renderer target includes RT64's modern host features, not only stock N64
pixels. `docs/RT64-PUBLIC-FEATURE-INVENTORY.md` is the machine-generated
denominator: runtime settings, build capabilities, and the small subset that
requires game/Extended-GBI cooperation are tracked separately so base-renderer
evidence cannot silently close an enhancement claim. The exact host-control
families and their live/recreate/game-cooperation boundaries are recorded in
`docs/RT64-RUNTIME-CONTROLS.md`.

The Rust replacement is tracked as a proof-driven program in
`docs/RENDER-WGPU-PORT-PLAN.md`. Its exact dual-pin source authority is generated
in `docs/RT64-PORT-AUTHORITY.md`. The plan keeps performance evidence as a
separate milestone; removal of the C++ FFI boundary by itself is neither a
parity result nor a speed result.

### Discovery corpus

Function-boundary discovery is graded against decomp answer keys under a strict
`wrong == 0` posture: a discovered boundary that splits a real answer function
fails the grade — only machine-checked evidence promotes a boundary, nothing is
guessed. The numbers below grade the ROM's **resident boot bank** (the code
present at the entrypoint before any overlay DMA — a few hundred functions per
game, not the whole ROM; OoT's 13,358 total — the count
`gate_d1_oot_overlays` asserts against its held-out dump — live mostly in
later banks and overlays). Recall on merged `main`:

| Game | Boot-bank matched / total | Recall | Wrong |
|------|---------------------------|-------:|------:|
| Ocarina of Time (primary answer key) | 119 / 137 | 86.9% | 0 |
| Majora's Mask | 402 / 486 | 82.7% | 0 |
| Super Mario 64 | 2816 / 3030 | 92.9% | 0 |
| Kirby 64 | 402 / 531 | 75.7% | 0 |
| WM2000 (AKI, NWXE) | 698 / 847 | 82.4% | 0 |
| No Mercy (AKI, NW4E) | 835 / 985 | 84.8% | 0 |

Every row above is graded, not blind: each game has an answer key only
because it already has a public decompilation project (zeldaret/oot,
zeldaret/mm, the SM64 and Kirby 64 decomps) or, for the AKI titles, a
splat-autogenerated symbol dump from the `aki-recomp` project. OoT, MM, SM64,
and Kirby 64 additionally take hand-written, cited TOML input (load tables,
DMA requests, and callback arguments not yet derived automatically) transcribed
from those same decomp projects, and
every game uses a same-engine "donor" ROM plus its answer key to seed
signature scanning (OoT and MM donate to each other; SM64 and Kirby 64 borrow
OoT's; WM2000 and No Mercy donate to each other). See
`crates/fn64-discover/reference/corpus-invocations.md` for the exact
per-game inputs. None of this weakens the `wrong == 0` grading bar, but the
recall numbers are not cold-start discovery on an unstudied ROM.

For the ROM-only breadth baseline, declare private ROM paths and expected
normalized digests in an external `fn64.cold-coverage-panel-input.v1` manifest,
then run the bounded panel tool:

```sh
python3 scripts/cold-coverage-panel.py \
  --manifest /path/to/private-panel.json \
  --binary "$PWD/target/release/fn64-discover" \
  --output /path/to/private-results.jsonl
```

The manifest must list canonical regular files explicitly; directory contents
are never inferred. Each ROM runs in an environment-cleared process group with
a timeout, a sampled aggregate-RSS watchdog, a system-free-memory floor, and
kernel-limited output files. Linux additionally receives a per-process address-
space backstop; macOS reports that no reliable hard memory limit is available.
The default ten repetitions bind the exact executable digest and emit one
path-free, digest-verified receipt per image plus content-free wall-time and
peak-RSS distributions only after the complete panel succeeds. See
`docs/DISCOVER-PLAN.md` for the measured nine-ROM baseline.

To characterize a whole local ROM corpus rather than a curated panel,
`scripts/rom-catalog.py` measures every ROM in a directory and
`scripts/rom-frontier.py` joins those measurements to live discovery outcomes:

```sh
python3 scripts/rom-catalog.py \
  --rom-dir "$FN64_ROM_CORPUS_DIR" \
  --output /path/to/private-catalog.jsonl

python3 scripts/rom-frontier.py \
  --catalog /path/to/private-catalog.jsonl \
  --binary "$PWD/target/release/fn64-discover" \
  --output /path/to/private-frontier.jsonl \
  --failures-output /path/to/private-failures.jsonl
```

`FN64_ROM_CORPUS_DIR` has no default; both tools exit rather than guess a ROM
location, and both refuse to write inside the repository.

The frontier runner requests snapshot-composed owner proof and emits
`fn64.rom-frontier.v2`. Each ROM row retains bank-local owner totals, exact
blocker-kind combinations, per-kind affected/occurrence/sole counts,
executable-range conclusion rules, indirect-resolution distributions, wall
time, the exact discovery-binary digest, and one-second sampled direct-process
RSS. The RSS value is a sampled lower bound, not a hard memory limit. Owner
composition is materially heavier
than plain discovery, so the default is two concurrent ROMs; raise `--jobs`
only after measuring available headroom. Failed inputs are retained separately
rather than disappearing from the denominator. A corpus pass is opportunity-
ranking evidence; it is not the ten-run determinism evidence required by
`AGENTS.md`.

`rom-catalog.py --dat-dir` optionally attaches developer, publisher, release
year, and genre from `libretro/libretro-database` clrmamepro DAT files
(CC BY-SA 4.0; place `developer.dat`, `publisher.dat`, `releaseyear.dat`, and
`genre.dat` in one directory). The join key is CRC32 of the whole ROM file,
computed locally, so it is verifiable rather than matched by filename.
Measured coverage on a 287-ROM corpus is 276 rows (96.2%); the misses are ROM
hacks, translations, and prototypes that No-Intro does not catalog, and they
are recorded as `dat_match: false` with empty fields rather than guessed. The
developer field gives the frontier report an engine axis: studios reused
in-house engines, so a fixture proven on one title plausibly transfers to that
studio's others. The catalog records
cartridge identity, IPL3 cluster, and per-ROM structural measures — boot-copy
entropy, the share of bytes inside long code runs, and a
`loader_stub_ratio` (distinct `jal` targets over resident `jr $ra` returns)
that separates ROMs whose code is resident in the boot bank from those whose
boot bank is only a loader stub. Recompiler-hazard counts (unaligned
`lwl`/`lwr`, `cache`, branch-likely, COP0) are taken only inside long code
runs, never over the raw bank: decoding data as instructions reports hazards
no title executes.

The frontier tool reports why load-image recovery did not produce a multi-bank
mapping, which is the measure that matters: ROMs that recover load geometry
harvest roughly ten times what boot-bank-only ROMs do (median entries by
selected strategy: `recovered_vrom` 13,158, `recovered_overlays` 4,476,
`untabled_delta_vote` 2,074, `boot_bank_only` 1,313). Each ROM gets a named
unmet condition — `no_candidate_table_found`, `candidate_table_under_mapped`,
`admitted_without_mapping`, `wrappers_examined_none_proven`, or a resource
ceiling such as `decode_limit_hit` — so a corpus of failures becomes a
histogram of specific proof-rule gaps rather than a uniform "did not work".
Resource ceilings outrank the emptier verdicts: a truncated search is a
frontier, not proven absence.

The boot-bank measures classify but deliberately do not rank. Measured across
178 resident-code ROMs, `code_run_share` correlates with actual harvest at only
r=+0.14 and `loader_stub_ratio` at r=-0.10, so ordering by them selects
candidates no better than chance.

`crates/fn64-cpu-runtime/tests/corpus_decode_sweep.rs` uses the same corpus as a
decoder coverage guard, gated by `FN64_RECOMP_SWEEP_DIR` and skipped when
unset. It selects prologue-delimited functions from each ROM's fixed boot-copy
window; it does not scan leaf/prologueless functions or recovered overlays.
That selected slice measured 23,808,397 decoded words and 289 `Unknown`
(0.0012%), strongly deprioritizing decoder widening without claiming whole-ROM
or ISA completeness.

`wrong == 0` holds across every game. The open remainder is an honest,
characterized gap — genuinely unreferenced library/dead code (Kirby 64),
struct-callback dispatch whose producer and loaded-image authority are not yet
connected (AKI), and variable-length script callbacks — not
mis-attributed boundaries.

## How we work

This is an **agent-forward** codebase: most code is written by AI agents in
orchestrated waves, humans direct and hold the gates. That's not a disclaimer,
it's the design — and it's why the norms below are written down instead of
tribal:

- **`AGENTS.md` is the contract.** Read order, scope rules, validation bars,
  clean-room protocol. Agents and humans follow the same one.
- **Evidence or it didn't happen.** Commits cite what they verified — a spec
  section, a differential trace, a run count. "Not verified" is an honest
  state we respect. A false "done" is the one sin.
- **Validation bars with teeth.** Deterministic fix: 10 consecutive clean
  runs. Concurrency fix: 20+. One green run proves nothing and we treat it
  that way.
- **Differential testing.** Behavior changes get diffed against something that
  actually runs. `scripts/lane-parity.sh` first audits whether the two generated
  callable-body sets are aligned; it currently rejects the legacy C lane as a
  semantic arbiter, while `--observe` retains a labeled, non-authoritative
  per-swap framebuffer comparison. `c_smoke` link-tests the ABI with a real C
  caller, and the recompiler has per-instruction oracle suites. Each has a
  documented blind spot in `docs/PARITY-METHOD.md` — a differential you can't
  run isn't evidence.
- **Types carry the invariants.** One-runnable-game-thread, queue ownership,
  rdram addressing — modeled so misuse fails to compile where possible, and
  fails *loudly* where not.
- **Docs are load-bearing.** Design docs and evidence trails are how the next
  agent inherits context. They get maintained like code because here, they are.

## Clean room

fn64 is written from our own behavioral specs (earned the hard way, in a
debugger), the public libultra documentation, and the recompiler's MIT source.
GPL implementation code is not read, not referenced, not laundered. The full
protocol — allowed sources, disallowed sources, citation rules — lives in
`AGENTS.md` and every design doc states its provenance.

## License

MIT OR Apache-2.0, at your option.
