# wm2000-boot

Headless boot harness for WM2000 (NWXE) on fn64. Contains **zero game
content** — see `build.rs`'s module doc for the exact env-var contract.
Standalone workspace (deliberately not a member of the main `fn64`
workspace), since it requires out-of-tree, game-derived build inputs the
main workspace must never depend on.

## Running

```
RECOMPILED_DIR=/path/to/aki-recomp/games/NWXE/RecompiledFuncs \
RECOMP_H_DIR=/path/to/aki-recomp/refs/WCWnWoRevengeRecomp/lib/N64ModernRuntime/N64Recomp/include \
ROM=/path/to/your/own/wm2000.z64 \
cargo run
```

- `RECOMPILED_DIR` — a directory containing N64Recomp-generated
  `RecompiledFuncs/*.c` + `recomp_overlays.inl` for WM2000 (NWXE).
- `RECOMP_H_DIR` — N64Recomp's own MIT-licensed `recomp.h` include directory.
- `ROM` — your own legally-obtained WM2000 (NWXE) ROM file. Never copied
  anywhere; read once at startup.

## What it does

1. Loads the ROM, registers every section from the real
   `recomp_overlays.inl` via `fn64-boot-harness`'s shared C-side walk into
   Rust, then marks the always-resident sections loaded.
2. Boots thread 0 running the real `recomp_entrypoint` symbol and drives
   the executor: `run_one_step` while runnable, `advance_virtual_time`
   (which fires the armed VI retrace ticker) when idle, up to a bounded
   step budget — logs periodically so a genuine stall/spin is visible
   rather than silently indistinguishable from real progress.
3. On every `osViSwapBuffer`, hashes the framebuffer region and dumps a
   PNG if non-uniform (`/tmp/fn64-fb-<n>.png`).
4. Writes the full `TraceEvent` stream to `/tmp/wm2000-boot-trace.jsonl`.

## Step budget (`WM2000_MAX_STEPS`)

Default 20,000,000 scheduler steps. Override with `WM2000_MAX_STEPS=<n>`
(positive integer). Measured economics (2026-07-21 trace,
`/tmp/wm2000-boot-trace.jsonl`): one full VI field of boot activity —
retrace fan-out, the AKI sound pump's ~6 raw `AI_STATUS`/`AI_LEN` polls
(each charged 32 guest cycles and one scheduler step), and one audio-task
submit — costs ~10–16 steps, so the default budget covers on the order of
a million VI fields (hours of virtual boot time). Audio tasks are paced
at exactly one per field by the retrace, the natural hardware cadence,
**not** a starved-feedback loop from the stand-in ucode completing
instantly — verified by the constant 1,563,558-cycle submit gaps in the
trace. Raise the budget for long captures; the budget is not what limits
graphics frames (see below).

## Boot stall after the first 3 gfx frames (2026-07-21)

With the IPL3 boot copy + RDRAM swizzle + RSP replay fixes in place, boot
reaches real rendered frames, then stalls:

1. sim 0 → ~16.03B cycles (~10,300 fields): audio-pump-only phase — one
   `M_AUDTASK` per field, no gfx.
2. sim ~16.03B–16.07B: the game submits 3 gfx frame pairs (F3DEX2 XBUS
   tasks); `osViSwapBuffer` fires and non-uniform framebuffers are dumped
   (near-white dithered fade frames).
3. sim >16.07B: **no further task submissions of any kind.** The retrace
   fan-out thread stops posting to the sound pump's queue (rdram 0xE0908),
   so the pump (blocked in `osRecvMesg`) starves and audio stops; the
   audio thread blocks on 0x559A0 and the gfx-task thread on 0x522E8,
   each waiting for a message the per-field main-loop thread never sends.
   Persists for 20k+ fields — a real handshake stall, not a pause.

## Known frontier (2026-07-14)

See `docs/DESIGN.md`'s "M1 boot-host attempt" section for the full,
byte-cited writeup: this run gets substantially deeper than any prior
milestone (real thread creation, a real stack pointer, a real PI DMA three
call-levels deep on a second thread) before stalling inside a long-or-
unbounded recompiled loop in `func_800004D0` — not yet root-caused. Four real
bugs (executor reentrancy, thread-identity-by-handle, an unseeded stack
pointer, and a pervasive native-vs-big-endian `MEM_W` mistranscription)
were found and fixed along the way, each with a regression test in
`fn64-abi`/`fn64-runtime`.

The real translated `wm2000_audio_ucode` (RSPRecomp-generated) could not be
linked in this harness: RSPRecomp's own codegen template unconditionally
`#include`s `librecomp/rsp.hpp`, which is GPL-3.0-licensed
(`N64ModernRuntime`'s top-level `COPYING`), disallowed by `AGENTS.md`'s
clean-room protocol. `stand_in_audio_ucode` in `src/main.rs` exercises the
real `M_AUDTASK` dispatch plumbing without linking the disallowed
dependency — it does nothing to rdram, and says so loudly when invoked.
