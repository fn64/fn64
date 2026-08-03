# WM2000 audio: status and the remaining step

Status note, not a from-scratch spec. fn64 already has the MIT-clean audio
machinery; this documents the current live-IMEM LLE path and its remaining
WM2000 evidence frontier.

## What already exists (MIT-clean, do NOT rebuild)

fn64 has its own clean-room RSP audio stack — there is no dependency on the
GPL-3.0 `librecomp/rsp.hpp` the RSPRecomp-generated ucode `#include`s:

- **RSP scalar + VU ISA interpreter** — `crates/fn64-audio/src/rsp/`
  (`interpreter.rs`, `vu.rs`, `vu_ops/`, `ops.rs`, `tables.rs`, `context.rs`,
  `dmem.rs`). Its module doc (`rsp/mod.rs:1`) states it provides "the headers
  the RSPRecomp-generated audio-ucode C `#include`s" — i.e. fn64's MIT
  replacement for the GPL runtime header. Differential VU oracle tests exist
  (`test(rsp): add independent VU differential oracle`).
- **Clean-room RSP → typed-Rust recompiler** — `crates/fn64-audio/src/rsp/
  recomp/`. It supports optional content-bound experiments, but translated
  artifacts are not the boot harness or release-authority path.
- **Live output path** — `fn64_audio::CpalBackend` + drain-aware AI pacing
  (`fix(audio): drain-aware osAiGetLength restores the game's own audio
  pacing`). Verified working end to end in `fn64-shell` (32kHz guest →
  device-default resample; producer-side rate negotiation kills the static).

So the LLE/clean-room audio subsystem is real and complete enough to make
**OoT** audible on fn64's own stack, GPL-free.

## The actual entry points (correct citations)

- Audio task dispatch: `osSpTaskStartGo_recomp` admits the loaded `OSTask`,
  runs rspboot, then executes live IMEM through the clean-room interpreter
  when `LleAccuracy` is selected.
- The command list the ucode consumes lives at the OSTask's `data_ptr` /
  `data_size` fields; ucode text/data at `ucode`/`ucode_data` (masked to
  physical with `& 0x1fff_ffff` / `& 0x00ff_ffff`).
- Accuracy policy (live-task IMEM): `set_audio_task_lle_accuracy`.
- Output boundary: `osAiSetNextBuffer_recomp` — where finished PCM is named and
  flows to the cpal backend (NOT inside `dispatch_audio_task`; AKI/OoT tasks
  carry zero `OSTask.output_buff` and select destinations via ucode commands).

## Diagnostics already built (use these, don't reinvent)

- `FN64_DUMP_AUDIO_TASK=<path>` (+ `FN64_DUMP_AUDIO_TASK_INDEX`, one-based):
  dumps the exact rdram image + task offset of one audio task, so an AKI audio
  ucode can be replayed offline against the real command list. Masked to the
  real 8 MiB RDRAM window.
- `set_audio_task_diagnostic_skip`: explicitly skips synthesis for
  non-certifying render diagnostics. Fixed-cycle release evidence rejects it.
- `crates/fn64-audio/examples/rsp_replay.rs`: offline RSP replay harness.

## WM2000 audio execution

WM2000 (and the AKI family: WT / Revenge / VPW2 / No Mercy) do NOT use OoT's
aspMain. They ship the **AKI shared audio library** ucode: 3156 bytes
(0xC54), byte-for-byte identical across the family. Identified for NWXE at
ROM 0x39510 → vram 0x80038910 (see
`aki-recomp/games/NWXE/rsp/wm2000_audio.toml`; a single unique full-length
match against Revenge's `revenge_audio.toml`).

The harness now executes admitted live IMEM through fn64's clean-room RSP
interpreter. Remaining work is evidence and performance:

One guarded private-input diagnostic on 2026-08-02 ran 100,000 scheduler
steps through live-image LLE and completed 985 audio tasks. Its AI boundary
received 984 buffers / 1,031,872 samples, including 459,481 nonzero samples
in the range -13,904..=13,463. All 984 buffers reached the live cpal backend;
the device rejected 28,800 Hz, opened at 48,000 Hz with band-limited
resampling, and its callback ran. Peak guarded RSS was 788 MiB.

That is one diagnostic run, not repeat evidence. The unpaced headless producer
overflowed cpal's bounded 250 ms ring, so it proves live custom-ucode → AI →
backend transport and nonzero signal but not uninterrupted audible output.
One follow-up at the same 100,000-step horizon used
`FN64_AUDIO_PACE=1`, held the host-only queue to at most 6,000 frames, emitted
no overflow diagnostic, and drained the queue before process exit. It retained
the same task/buffer/sample/nonzero/range totals and peaked at 782 MiB guarded
RSS. A health-instrumented repetition showed that the reference software
renderer, not audio synthesis or CoreAudio callback scheduling, still starved
the integrated loop: 28.086 of 30.087 executor seconds were graphics work,
with 1,855,348 underrun samples and zero late callbacks.

An explicit audio-isolation run selected `GraphicsTaskExecutionPolicy::DiagnosticSkip`
while retaining live-IMEM LLE audio. It reproduced all 985 audio tasks and the
same AI sample totals, reported zero active-window underruns and zero late
callbacks, and reduced executor work to 1.355 seconds. Its 884 additional
silence samples occurred only after the measured window while draining the
partial terminal callback. This is a subsystem diagnostic, not graphics or
release evidence.

A native sample attributed 5,046 of 6,615 main-thread samples to raw-RDP
processing; triangle rasterization and texture sampling dominated. The
reference sampler was reading and filtering the adjacent tile even when LOD
was disabled and the combiner did not select `TEXEL1`. Reusing `TEXEL0` in
that no-consumer case reduced the same audio-disabled 100,000-step workload's
graphics time from 20.921 to 14.815 seconds (29.2%) and executor time from
22.012 to 15.767 seconds (28.4%), with identical step, task, and AI totals.
The focused no-unused-tile-read regression passed 10/10 guarded runs. The
optimized reference lane still burst-starved integrated playback: one paced
run reported 453,492 active-window underrun samples.

The intended interactive RT64/LLE lane completed the same paced horizon with
zero active-window underruns, zero late callbacks, 2.920 seconds of graphics
work, and the same AI totals. Its only 884 silence samples were again the
labeled terminal partial-buffer drain. This is one guarded diagnostic run,
not repeated release evidence.
The native-C game lane also retains callable empty bodies, and this run used
the harness's documented diagnostic voice-map virgin-memory intervention;
neither condition is release authority. A standalone replay of the first
captured task hit its 2,000,000-step cap; `rsp_trace` labels that cap
`UnhandledResumeTarget`, so the dump did not independently close the task.

1. Run repeated content-bound WM2000 tasks through live-IMEM LLE and retain
   fixed-cycle framebuffer/audio/device/memory evidence.
2. Verify every exercised AKI RSP operation and loud unsupported frontier.
3. Compare any optional translated artifact against the live-IMEM baseline;
   it remains diagnostic evidence, not release authority.
4. Repeat the integrated paced RT64/LLE horizon to the ten-run deterministic
   bar with zero active-window underruns; the current listener evidence is one
   run. Add decode/resample/envmix unit vectors.

## Clean-room boundary (unchanged, still binding)

Do NOT read, copy, "take inspiration from", or link `librecomp/rsp.hpp`, any
RSPRecomp-generated `.cpp`, or N64ModernRuntime GPL code
(`aki-recomp/games/NWXE/rsp/wm2000_audio.cpp` line 1 is `#include
"librecomp/rsp.hpp"`; the include is emitted unconditionally by
`RSPRecomp/src/rsp_recomp.cpp:1179`). fn64's recompiler consumes the raw ucode
TEXT BYTES (public N64 machine code from the ROM) and emits fn64-owned Rust
against fn64-owned headers. Permissively-licensed emulator RSP-HLE audio
may be read for documented command semantics only after verifying the specific
file's license.

## Bottom line

fn64's production accuracy path is live-IMEM LLE. WM2000 still needs repeated
private-input audio validation and any exercised RSP gaps closed before an
end-to-end sound claim is made.
