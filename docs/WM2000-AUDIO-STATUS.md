# WM2000 audio: status and the remaining step

Status note, not a from-scratch spec. fn64 already has the MIT-clean audio
machinery; this documents what exists, what runs today, and the one remaining
step to get WM2000 sound — so nobody rebuilds what's already there.

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
  recomp/` (`feat(fn64-audio): clean-room RSP -> typed-Rust recompiler +
  OoT aspMain`). Turns a raw RSP ucode text image into runnable Rust.
- **A recompiled, runnable OoT audio ucode** — the `oot-audio-ucode` generated
  crate (`rsp/recomp/mod.rs:209`), produced by the recompiler above. OoT's
  aspMain synthesizes real PCM through this path.
- **Live output path** — `fn64_audio::CpalBackend` + drain-aware AI pacing
  (`fix(audio): drain-aware osAiGetLength restores the game's own audio
  pacing`). Verified working end to end in `fn64-shell` (32kHz guest →
  device-default resample; producer-side rate negotiation kills the static).

So the LLE/clean-room audio subsystem is real and complete enough to make
**OoT** audible on fn64's own stack, GPL-free.

## The actual entry points (correct citations)

- Audio task dispatch: `dispatch_audio_task` in
  `crates/fn64-abi/src/task_dispatch.rs` (~line 1712). Runs the registered
  audio ucode with `(rdram, task_offset)`, where `task_offset` is the OSTask's
  rdram OFFSET used to seed RSP DMEM[0xFC0] (rspboot loads the 64-byte OSTask
  there; the audio ucode reads `ucode_data`@0x18). Called from
  `osSpTaskStartGo_recomp` (the Load+StartGo path AKI/OoT audio drivers use).
- The command list the ucode consumes lives at the OSTask's `data_ptr` /
  `data_size` fields; ucode text/data at `ucode`/`ucode_data` (masked to
  physical with `& 0x1fff_ffff` / `& 0x00ff_ffff`).
- Registration hook (recompiled-ucode path): `set_audio_ucode_fn`
  (`task_dispatch.rs:2400`), type `AudioUcodeFn = unsafe extern "C"
  fn(*mut u8, u32) -> u32`.
- Output boundary: `osAiSetNextBuffer_recomp` — where finished PCM is named and
  flows to the cpal backend (NOT inside `dispatch_audio_task`; AKI/OoT tasks
  carry zero `OSTask.output_buff` and select destinations via ucode commands).

## Diagnostics already built (use these, don't reinvent)

- `FN64_DUMP_AUDIO_TASK=<path>` (+ `FN64_DUMP_AUDIO_TASK_INDEX`, one-based):
  dumps the exact rdram image + task offset of one audio task, so an AKI audio
  ucode can be replayed offline against the real command list. Masked to the
  real 8 MiB RDRAM window.
- `FN64_SKIP_AUDIO_UCODE`: skips the per-frame RSP synth (dominant per-swap
  cost) while keeping the audio-reset handshake — iterate on the renderer at
  boot speed. A real audio run must NOT set it.
- `crates/fn64-audio/examples/rsp_replay.rs`: offline RSP replay harness.

## The one remaining step for WM2000 sound

WM2000 (and the AKI family: WT / Revenge / VPW2 / No Mercy) do NOT use OoT's
aspMain. They ship the **AKI shared audio library** ucode: 3156 bytes
(0xC54), byte-for-byte identical across the family. Identified for NWXE at
ROM 0x39510 → vram 0x80038910 (see
`aki-recomp/games/NWXE/rsp/wm2000_audio.toml`; a single unique full-length
match against Revenge's `revenge_audio.toml`).

The remaining work is therefore NOT "build an RSP interpreter" — it's:

1. Run the AKI audio ucode text (0x39510, 0xC54) through the **existing**
   clean-room RSP → typed-Rust recompiler (`fn64-audio/src/rsp/recomp/`), the
   same path that produced `oot-audio-ucode`, to emit an `aki-audio-ucode`
   crate. One implementation covers the whole AKI family (identical bytes).
2. Register it for WM2000 in place of the stand-in:
   - `examples/wm2000-boot/src/main.rs:119` still wires
     `stand_in_audio_ucode` — replace with the AKI ucode fn.
   - `fn64-shell` currently only has an `oot-audio` feature
     (`wire_audio_ucode`, `main.rs:~965`); add the AKI ucode crate the same
     way (non-cross-pinning generated crate, mirroring the OoT lane).
3. Verify AKI RSP ops the recompiler emits are all covered by the interpreter
   (the AKI audio library may exercise VU/scalar ops OoT's aspMain didn't);
   add differential-oracle vectors for any new ones. This is where real
   effort may hide — unknown until the AKI ucode is decoded.
4. Confirm WM2000 menu music / SFX is audible through the already-live cpal
   path; add decode/resample/envmix unit vectors.

## Clean-room boundary (unchanged, still binding)

Do NOT read, copy, "take inspiration from", or link `librecomp/rsp.hpp`, any
RSPRecomp-generated `.cpp`, or N64ModernRuntime GPL code
(`aki-recomp/games/NWXE/rsp/wm2000_audio.cpp` line 1 is `#include
"librecomp/rsp.hpp"`; the include is emitted unconditionally by
`RSPRecomp/src/rsp_recomp.cpp:1179`). fn64's recompiler consumes the raw ucode
TEXT BYTES (public N64 machine code from the ROM) and emits fn64-owned Rust
against fn64-owned headers — that is the whole point, and it is already how
`oot-audio-ucode` was produced. Permissively-licensed emulator RSP-HLE audio
may be read for documented command semantics only after verifying the specific
file's license.

## Bottom line

fn64's audio subsystem is MIT-clean and complete enough to make OoT audible.
WM2000 needs the AKI shared-audio-library ucode run through the **existing**
recompiler (like OoT's aspMain was) and registered — plus filling any RSP-op
gaps the AKI library exercises. It is an extension of a working system, not a
new build.
