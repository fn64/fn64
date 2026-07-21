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
3. sim >16.07B: **no further task submissions of any kind.** Persists at
   any budget (verified to 50M steps ≈ 38 virtual hours).

Deadlock shape (2026-07-21 trace diagnosis, using the new
`QueueOpKind::Drop` + `TraceKind::EventMesg` instrumentation): the gfx
manager (executor thread 17) and audio manager (thread 18) share **one
FIFO mesg queue** (rdram 0x52320) for `OS_EVENT_SP` task-done messages —
registered once at boot, never re-registered, zero messages dropped, all
completions delivered intra-field (~192 cycles after kick). The final
sequence (trace seq 312716–312800):

1. gfx task loaded+kicked by 17; 17 parks on 0x52320 — but 18 has been
   parked there since 23M cycles earlier, so the SP-done wakes **18**;
2. 18 treats it as its grant, kicks the last audio task (#9307), parks;
3. that audio task's SP-done wakes **17** (`Wake, thread: 17` — the
   crossover), which proceeds as if its gfx task completed: it loads the
   next gfx task, asks the service thread (3) for the go-ahead on queue
   0x522E8, and never kicks it;
4. terminal state: main thread (6) blocked forever on the sound-service
   response queue 0x559C0; service thread (3) wakes every field on its
   retrace queue 0x55948 but never replies; 18 parked forever on
   0x52320; the AI pump starves (its feeder queues 0xE0908/0xE0930 go
   quiet); no task of any kind is ever submitted again.

Eliminated by direct evidence: message drops (0 `Drop` events),
`OS_EVENT_SP` re-registration (a single boot-time `EventMesg`),
`osSpTaskYield` preemption (never called in the window), completion
latency past a field boundary (clamping latency to 500k cycles produces a
byte-identical trace), and the stand-in audio ucode (routing audio
through the real RSP LLE replay — `set_audio_task_lle` — changes
nothing).

RESOLVED (2026-07-21): decompiling the two manager loops answered the
open question and located the divergence in fn64, not the game.

The guest protocol (funcs_0.c): `func_80000B30` builds the AKI event
manager — per-runner command queues 0x522B0 (audio) / 0x522E8 (gfx), the
SHARED `OS_EVENT_SP` done queue 0x52320 (= 0x522E8+0x38), the DP queue
0x52358, and the handoff queue 0x52390 — then starts thread 18 (audio
runner, entry `func_80001024`, **priority 0x6E**) and thread 17 (gfx
runner, entry `func_80001180`, **priority 0x64**). The gfx runner kicks
a task and parks on 0x52320 for its done. The audio runner, on
receiving a command while a gfx task is in flight (`*0x52760 != 0`),
calls `osSpTaskYield`, parks on 0x52320 to consume the gfx task's
yield-or-done message, kicks its own task, parks on 0x52320 AGAIN for
the audio done, then either restarts the yielded gfx task or re-posts
the done to 0x52320 for a gfx task that completed instead of yielding.
So BOTH runners being parked on 0x52320 simultaneously is a designed,
legitimate state — the protocol is correct on hardware because libultra
wakes blocked receivers in THREAD-PRIORITY order (`__osEnqueueThread`
keeps `mq->mtqueue` priority-sorted; `osSendMesg` pops the head): every
SP-done that arrives while both are parked wakes the higher-priority
audio runner (0x6E > 0x64) first, and only the final resumed-gfx done
falls through to the gfx runner.

fn64's divergence: `fn64-runtime`'s `BlockedList` popped waiters in
ARRIVAL order. After the audio runner consumed the gfx yield-done and
re-parked, the gfx runner was the longer-parked waiter — so the AUDIO
task's done woke the GFX runner (the trace's crossover, seq
312716-312800). Fixed in `crates/fn64-runtime/src/mesgqueue.rs` by
making both blocked lists priority-sorted at insertion (descending,
FIFO among equals), with the parking thread's priority captured at
block time. With the fix the boot runs straight through the former
3-frame wall (fade ramp white → black completes and gfx frames keep
pacing 2 per second-ish of virtual time alongside the audio cadence).

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
