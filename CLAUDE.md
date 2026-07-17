# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Start here

`AGENTS.md` is the operating contract — read it in full before touching code. It
is short and non-negotiable: clean-room protocol, validation bars, loud traps,
evidence-cited commits. The rules below are the mechanics; AGENTS.md is the
mandate. Then `docs/DESIGN.md` (architecture) and `docs/ROADMAP.md` (what's
actually in flight, with per-item evidence).

Docs are load-bearing: behavior change and its doc land in the same commit.
`docs/ROADMAP.md` is updated in the same commit as the work it tracks.

## Commands

```sh
cargo nextest run --workspace          # the authoritative test gate
cargo test -p fn64-runtime             # per-crate; runtime is pure Rust, no C++ toolchain needed
cargo nextest run -p fn64-render-rt64 f3dex2_replay   # single test target
cargo clippy -p <crate> --all-targets  # run per-crate alongside tests
```

`opt-level = 1` on the dev profile is deliberate — recompiled MIPS call graphs
are deep, and debug builds are unusable at opt-level 0. Don't "fix" it.

### Booting OoT (the real debug loop)

`examples/oot-boot/oot` bakes in the fast-loop env; use it rather than
hand-assembling cargo invocations:

```sh
./oot run [SWAPS]      # bounded release boot (default 6), prints summary + PNG paths
./oot where            # run + report the deepest frame / loud trap — the frontier
./oot trace [SWAPS]    # run + dump last executor events at the stop
./oot fn FILE:LINE     # resolve recompiled-C location -> RECOMP_FUNC name
```

Fast loop (see `docs/FAST-LOOP.md` — use this for every rs-lane job):

```sh
export CARGO_TARGET_DIR=/tmp/fn64-shared-target        # reuse deps across worktrees
export RECOMP_RS_DIR="$(./scripts/native-emit.sh)"     # content-addressed emit cache, ~2-4s -> 0.13s
export FN64_RECOMP=rs
```

Game paths (ROM, `RecompiledFuncs`, configs) default to an out-of-tree
`aki-recomp` checkout via `$AKI`. No ROM bytes, game content, or recompiled
output ever enters git — check what you stage.

## Architecture

Two dimensions of lane selection cut across everything; most confusion comes
from not knowing which lane a symptom is in.

**Recomp lane** (`FN64_RECOMP=c|rs`): `c` links compiled `RecompiledFuncs` C
from N64Recomp; `rs` links an emitted typed-Rust whole-ROM crate. Both link the
*identical* recompiled semantics, so they A/B each other. Because the emitted
crate is out-of-tree game-derived material that must never enter the main
workspace graph, the rs lane builds through **standalone manifests with their
own `[workspace]`** (`examples/oot-boot/rs/Cargo.toml`,
`crates/fn64-shell/rs/Cargo.toml`) that reuse the sibling `src/main.rs` and
`build.rs`. A `recompiled` symlink is refreshed from `RECOMP_RS_DIR` before
Cargo resolves the graph.

**Render backend** (`FN64_RENDER=reference|rt64`, feature `rt64`):
ReferenceBackend is the software oracle; RT64 is the faithful lane.

Dependency direction is strictly one-way and enforced by review:

```
fn64-shell -> fn64-abi -> fn64-runtime
           -> fn64-boot-harness -> fn64-abi + fn64-runtime
           -> fn64-render-rt64 -> fn64-runtime (types only)
```

- `fn64-runtime` depends on nothing else in the workspace. Pure safe Rust:
  scheduler, message queues, timer wheel, rdram ownership. It does not know it
  is called from generated C, and does not know RT64 exists.
- `fn64-abi` is deliberately dumb — a signature-and-marshalling adapter per
  symbol. New policy does not get invented here. Reviewing it in isolation
  should answer "does this match ABI-SURFACE.md" with no runtime internals.
- `fn64-boot-harness` owns the game-agnostic generated-C boot boundary shared by
  the shell and the headless examples. Input/save/render/audio policy stays
  local to each harness — those differ per game and host.
- `fn64-render-rt64` is the **only** crate permitted to contain C++ / call
  RT64's C++ API. License and language boundary are the same boundary; scheduler
  work must never require a C++ toolchain.

## Working rules that bite

- **Loud traps, no silent shrugs.** Unimplemented ABI surface panics with the
  symbol name and call context. A defensive null-guard or fallback that hides
  corruption is a bug, not a fix — if you're tempted to guard, you haven't found
  it yet.
- **Validation bars.** Deterministic fix: 10 consecutive clean runs before
  saying "fixed." Concurrency fix: 20+, and name the exact interleaving your fix
  closes in a comment at the fix site. One green run proves nothing. "Not
  verified" is respectable; a false "done" is the one unforgivable sin.
- **Clean room.** Never read GPL runtime implementation code (ultramodern /
  librecomp internals) — not for inspiration, not to check one thing. If a
  behavior is only knowable from there, run a black-box differential against the
  reference runtime. Every design claim cites its allowed source.
- **Mechanism over patch.** Fixing one instance of a bug class means building
  the sweep that finds the rest. One-off fixes to recurring shapes get bounced.
  (The rdram word-swizzle bug has bitten three times — that's the pattern.)
- **Types before audits.** An invariant enforced by review is a bug with a delay
  timer; put it in the type system.

## Delegation

Work is dispatched as parallel codex waves on disjoint crates
(`docs/DELEGATION.md`). `scripts/dispatch.sh <name> <card.md>` creates
`../fn64-<name>-wt` on `wave/<name>`, prepends AGENTS.md to the card, and runs
`codex exec` in the background. `scripts/wt.sh` lists/prunes worktrees.
Verification is adversarial and stays with the dispatcher: run the gates
yourself, never trust a job's own claim.
