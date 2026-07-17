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

## RDRAM layout and live timing loops

The recurring native-word/guest-byte layout class has a sub-second structural
gate. Run both modes when touching any host/device/RDRAM boundary:

```sh
scripts/lint-rdram-layout.py --selftest
scripts/lint-rdram-layout.py
```

The live shell heartbeat is also a bounded timing probe now. Alongside the
decoded-frame hash it reports windowed retrace Hz and interval, guest pump,
and present median/p95. A slow `pump` with a cheap `present` is an executor or
renderer-work budget problem; a slow `present` is the window/GPU blit path.
The cumulative `retrace_hz` remains useful for long-run drift but must not be
used to locate a phase transition by itself.
The same heartbeat reports cumulative and per-window callback underrun samples,
AI submission count, current-DMA `AI_LEN`, guest/device rates, and host frame
depth. For live audio acceptance, wait past startup/title and require stable
depth with neither an overflow warning nor new per-window underruns; a single
point-in-time depth or a nonzero-sample count is not an audible-health test.

To split synthesis/AI decoding from host resampling, set
`FN64_DUMP_AUDIO_STREAM_PCM=/tmp/fn64-guest.pcm`. It records at most 12 seconds
of consecutive pre-resample stereo `s16le` plus a `.meta` sidecar. Convert the
local evidence without committing it:

```sh
ffmpeg -f s16le -ar 32006 -ac 2 -i /tmp/fn64-guest.pcm \
  -ar 48000 /tmp/fn64-guest-hq.wav
```

If the WAV is clean while live output buzzes, the host resampler is the fault;
if both buzz, continue upstream at AI decoding/RSP synthesis.

For task-level RSP replay, set `FN64_DUMP_AUDIO_TASK=/tmp/fn64-task.rdram`.
By default this captures the first submitted audio task; use one-based
`FN64_DUMP_AUDIO_TASK_INDEX=N` to capture the task aligned with a later audible
event. The sidecar records `task_offset`, `task_index`, and `rdram_len`.
Pair it with `FN64_TRACE_AI_BUFFERS=1` when checking whether the AI consumes the
same RDRAM address/length range the replayed task's `A_SAVEBUFF` commands
produced.

For generated-vs-interpreter RSP checks:

```sh
RSP_TRACE_WRITE_RDRAM=/tmp/interp.rdram \
  cargo run -p fn64-audio --bin rsp_trace -- \
  --task-dump /tmp/fn64-task.rdram /tmp/fn64-task.meta 2000000

RSP_TRACE_WRITE_RDRAM=/tmp/generated.rdram \
  cargo run --manifest-path examples/oot-boot/audio-ucode/Cargo.toml \
  --bin replay_task -- /tmp/fn64-task.rdram /tmp/fn64-task.meta

cmp /tmp/interp.rdram /tmp/generated.rdram
```

`RSP_TRACE_DMA=1` logs every RSP DMA read/write with decoded length and a source
checksum in both paths. Use it to find the first divergent hardware seam before
looking at instruction traces.

For crackle that survives stable ring depth and zero underruns, capture the
post-resample stream too:

```sh
FN64_DUMP_AUDIO_OUTPUT_STREAM_PCM=/tmp/fn64-output.pcm
ffmpeg -f s16le -ar 48000 -ac 2 -i /tmp/fn64-output.pcm /tmp/fn64-output.wav
```

The heartbeat also reports `late_callbacks` and `max_callback_gap_us`. A clean
pre/post PCM pair with late callbacks points at host output delivery; crackle
already present in the pre-resample file points upstream at AI decoding/RSP
synthesis.
