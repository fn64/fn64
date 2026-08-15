# Shell frontend gaps: controller UI, ROM loading, display options

Raised 2026-08-08 after a play session. Three questions, three answers, and one
structural problem underneath all of them.

## The structural split: bridged by one shared UI seam

| | `crates/fn64-shell` (binary `fn64`) | `recomps/wm2000/packages/wm2000-block-boot` (binary `wm2000-shell`) |
|---|---|---|
| runs WM2000 | **no** — OoT function lane, hardcoded OoT cart VRAM, no scripted input | **yes** |
| controller GUI | **yes** — shared `overlay.rs`, behind **F1** | **yes** — includes that same module by path |
| `input_map` + saved config | **yes** — platform config dir (`fn64/input.toml`) | **yes** — same module and file |
| RT64 selector | yes | yes (added `42271e3`) |

The function and block lanes remain different boot contracts, but frontend
behavior no longer needs two implementations. The extracted WM2000 shell
includes fn64-shell's game-neutral framebuffer, input map, gamepad, overlay,
and timing modules directly; its RT64 and reference present paths both end in
the same overlay compositor. Frontend changes therefore land once in fn64 and
are compiled by each game harness.

## 1. Controller configuration GUI — AVAILABLE IN BOTH SHELLS

`crates/fn64-shell/src/overlay.rs` already provides, behind **F1**:

- press-to-bind remapping and explicit unbinding for **keyboard and gamepad**
- a deadzone slider with a **live analog-stick scope**
- connected-controller status and explicit keyboard/gamepad columns
- guarded restore-defaults and a direct Done action
- auto-save to `InputConfig` TOML in the platform config directory (via `dirs`)

It renders egui over the paused-input framebuffer. Note the version pin it
documents at `overlay.rs:6-12`: pixels 0.15 pins wgpu 0.19, which pins egui to
0.27, whose `egui-winit` wants winit 0.29 — but the shell is on winit 0.30. The
overlay therefore hand-rolls ~40 lines of event translation (cursor, clicks,
scroll) rather than taking `egui-winit`. **Any port must carry that constraint,
not rediscover it.**

WM2000 consumes the module through the same `#[path]` seam as its input map and
gamepad support. Opening the modal clears held keyboard state, supplies neutral
live input while editing, consumes armed keyboard/gamepad captures, and leaves
committed controller schedules authoritative. Both renderer lanes composite
the panel after filling the same Pixels surface.

## 2. ROM drag-and-drop / no-ROM startup — DOES NOT EXIST

`shell.rs` has **zero** `DroppedFile` handling. The ROM arrives via the `ROM`
env var, and it is needed at **build** time, not just run time — the shard
catalog is generated against a specific ROM by
`recomps/wm2000/packages/wm2000-block-shards/build.rs`.

**That makes "drop a ROM to play it" a much larger feature than it appears.**
The block lane is compiled *per title*; it cannot accept an arbitrary ROM at
runtime the way an interpreter would. Options, in ascending cost:

1. **A ROM picker over already-built titles** — honest and small. The window
   opens with no game, lists the shard packs compiled into this build, and boots
   the chosen one. Drag-and-drop can validate the dropped file against those.
2. **Drop-to-build** — accept a ROM, run discovery + shard generation, then
   boot. That is a 9-11 minute build (rule 8), so it needs progress UI and is
   really a launcher feature.
3. **Runtime-general loading** — would require a non-shard execution path. Out
   of scope for the block lane by design.

**Recommend (1).** State plainly in the UI that fn64 recompiles per title, so
"drop any ROM" is not implied and then disappointing.

## 3. Widescreen and display upgrades — DO NOT EXIST

No aspect-ratio handling, no internal-resolution scaling, no enhancement
options. Two things now make this tractable that did not before:

- **RT64 renders at its own internal resolution**, not the guest's 320x240 —
  established while wiring the present path (`42271e3`), which is why geometry
  is read from the capture rather than `vi_width()`. The scaling seam exists.
- **The present path is already env-configurable** (`FN64_PRESENT_MODE`,
  `FN64_FRAME_PACE_MS`), so display policy has a home.

Candidate options, cheapest first: integer-scale vs stretch; aspect ratio
(4:3 / pixel-perfect / 16:9); internal resolution multiplier (RT64 only);
present mode; frame pacing target.

**Widescreen specifically is not just a viewport change.** The guest computes
its own projection at 4:3; rendering wider without touching the game's matrices
crops or stretches rather than revealing more scene. True widescreen is a
per-title patch, and should be labelled as such rather than offered as a
checkbox that appears not to work.

## Priority note

None of this is on the 60fps critical path. The measured blocker there is
~~audio delivered at **91.5% of real time**~~ **CORRECTED 2026-08-08: audio is
delivered at 45.7% of real time** and a render field at **2.15x budget** — see
`perf-method.md`. These are playability-and-polish items and should be
scheduled against that, not ahead of it.

The 91.5% figure was wrong because `AudioOutputStats::samples` counts **i16
channel samples, not frames**; dividing by 32,000 instead of 16,000 frames/s
halved the apparent deficit. The corrected figure is not a second problem:
**45.7% of real time and 2.15x the frame budget are the same measurement**, so
do not budget separate work against them. The wrong number is left visible
because the error is the instructive part — see rule 21 in `perf-method.md`.
