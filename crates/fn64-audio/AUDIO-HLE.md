# Audio HLE migration plan

Fn64's clean-room RSP interpreter is the audio accuracy authority. Audio HLE is
an optional optimization for an exact admitted microcode identity; it is not a
replacement for LLE evidence and it must never be selected from a familiar
opcode or task shape.

## Current boundary

`AudioTaskExecutionPolicy::LleAccuracy` runs rspboot and the live audio image,
commits the resulting RDRAM/DMEM/IMEM/SP state, and reports measured RSP work to
the device fabric. `osAiSetNextBuffer` later identifies the PCM range consumed
by AI. An HLE task therefore cannot be modeled as an immutable RDRAM input that
returns a `Vec<i16>`: its result is a transactional set of machine effects.

The HLE foundation has two independent pieces:

- `hle.rs` validates and iterates family-neutral 8-byte command framing without
  assigning semantics to an unknown family.
- `hle_outcome.rs` models the complete candidate task effect and compares it to
  the LLE effect in a stable first-divergence order.

The standard ABI decoder is a separate layer. Possessing a valid standard
packet does not prove that the loaded microcode implements that packet family.

## Clean-room evidence

The public libultra
[`abi.h`](https://ultra64.ca/files/documentation/online-manuals/man-v5-2/allman52/header/abi.htm)
defines the 16 standard opcodes, exact 64-bit packet packing, public flags, and
state-block sizes. The public `OSTask` and `alAudioFrame` manuals define task
geometry, command counts, and the command-list/PCM relationship. The SGI RSP
Programmer's Guide defines the 4 KiB IMEM/DMEM banks and DMA boundary.

Those sources do not define bit-exact ADPCM, resampler, envelope, mixer, or
pole-filter arithmetic. They also do not prove that a game image is stock
`aspMain`, `n_aspMain`, or a modified derivative. Fn64 will establish those
details through same-snapshot differential execution against its existing
clean-room LLE path. No GPL runtime implementation is an input to this work.

## Admission and transaction

Admission binds:

1. SHA-256 of the complete 4 KiB task-entry IMEM after rspboot.
2. The declared microcode-data length.
3. SHA-256 of exactly that microcode-data image.
4. One named HLE command family and implementation revision.

Both lanes begin from one rspboot-completed snapshot. They receive independent
copy-on-write RDRAM and RSP state, run to a terminal boundary, and produce an
`AudioTaskOutcome`. The comparator covers:

- every final RDRAM write range and byte;
- declared PCM ranges;
- complete DMEM and IMEM plus IMEM generation;
- SP PC, status, semaphore, and DMA registers;
- ordered DPC submissions, if any;
- terminal reason and deterministic completion work.

Only an exact comparison permits the candidate outcome to commit. Unknown
identity, unknown opcode, malformed geometry, unsupported flags, or arithmetic
divergence is a typed loud frontier. A future optimized production policy may
fall back transactionally to untouched LLE only when that fallback is explicit
in the installed per-ROM policy and its evidence records the disposition.

## Work sequence

1. **Framing and outcomes** — complete the allocation-free command view,
   physical-RDRAM journals, exact identity, visible-state outcome, and
   first-divergence comparator.
2. **Standard wire decoder** — decode all 16 documented packets and reject
   unknown selectors/unsupported flag combinations without mutating state.
3. **Memory commands** — implement `SETBUFF`, `LOADBUFF`, `SAVEBUFF`,
   `CLEARBUFF`, `DMEMMOVE`, `SEGMENT`, and `LOADADPCM` with complete preflight
   and overlap/bounds tests.
4. **DSP commands** — add ADPCM, resample, mixer/envelope, interleave, and
   pole-filter behavior incrementally. Each arithmetic edge is accepted only
   through exact LLE differentials.
5. **Runtime policy** — add a non-release HLE/differential policy, task-entry
   snapshot seam, catalog identity, and transactional commit. Keep
   `LleAccuracy` as the default and release authority.
6. **End-to-end evidence** — run fixed-cycle framebuffer/audio/device/memory
   digests and zero-unsupported reports over representative games.

Each deterministic behavior claim requires ten consecutive clean runs. A
runtime synchronization change requires at least twenty and a comment naming
the closed interleaving. Private ROM-derived captures remain outside git; only
hand-authored public fixtures and non-content evidence belong in the tree.

## Release frontier

HLE is not release-authoritative until every admitted task in a representative
full-ROM run compares exactly and the installed policy is represented in the
release evidence wire. WM2000 additionally retains a documented harness
voice-map intervention; no result from that harness is a hardware-parity
certificate until the intervention is removed or independently certified.
