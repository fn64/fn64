# Fast loop: cache what's deterministic, share the build

Two tooling levers cut per-job time (measured 2026-07-16):

## 1. `scripts/native-emit.sh` — cache the deterministic whole-ROM emit
`recompile_rom` emits a BIT-IDENTICAL 139MB Rust-recompiled crate for the same
(ROM + oot.toml + recompiler binary). Re-emitting per job wastes ~2-4s + 133MB.
`native-emit.sh` content-addresses the emit (hash of those 3 inputs) into
`/tmp/fn64-emit-cache/<hash>` and reuses it. Measured: **~2-4s emit -> 0.13s
cache hit.** Use it instead of calling recompile_rom directly:
```
RECOMP_RS_DIR="$(./scripts/native-emit.sh)"   # emits on miss, instant on hit
```

## 2. Shared `CARGO_TARGET_DIR` — reuse compiled deps across worktrees
Every worktree cold-compiles the same deps into its own `target/` (GBs, minutes).
A shared target dir lets jobs reuse each other's builds. Measured: a fresh
worktree building `fn64-runtime` with the shared target = **0.05s (full reuse)**
vs a cold compile. Set it for every job build:
```
export CARGO_TARGET_DIR=/tmp/fn64-shared-target
```
CAVEAT: the fn64-audio lockfile collision (oot-boot's oot-audio-ucode dep pins
fn64-audio to the main checkout) can still bite worktree oot-boot builds — the
rs manifest sidesteps it; the shared target does not change that.

## The combined fast rs-boot loop
```
export FN64_GAME_DIR=/path/to/your/rom-derived/workspace  # no default: set it
export CARGO_TARGET_DIR=/tmp/fn64-shared-target
export RECOMP_RS_DIR="$(./scripts/native-emit.sh)"
export FN64_RECOMP=rs
# build via examples/oot-boot/rs/Cargo.toml (crate emit compiles in parallel,
# incremental after the modcrate fix) -> run ./oot
```

## Late gameplay state/task differential

`OOT_STATE_TRACE=1` revalidates the live `Play_Main` allocation on every swap
and reports control, player, and generated-C-grounded `RoomContext` load state.
To compare selected RT64 task indices without committing game data, add:

```sh
export FN64_GFX_TASK_DUMP=4149,4289
export FN64_GFX_TASK_DUMP_DIR="$PWD/.eyegate/r6"
```

Each report contains the `OSTask`, independent reference triangle count, full
F3DEX2 command walk, resolved segment/DL targets, and bounded content
fingerprints. `.eyegate/` is evidence-only and remains untracked.
Dispatch every rs-lane job with these exports; it reuses the cached
emit + the shared compiled deps instead of redoing both.
