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

The HLE foundation has four independent pieces:

- `hle.rs` validates and iterates family-neutral 8-byte command framing without
  assigning semantics to an unknown family.
- `hle_outcome.rs` models the complete candidate task effect and compares it to
  the LLE effect in a stable first-divergence order.
- `hle_transaction.rs` owns checked 4 KiB DMEM and a logical-byte,
  copy-on-write RDRAM overlay. It produces canonical patches without mutating
  the live task input.
- `hle_snapshot.rs` validates an owned post-rspboot capture and forks deep,
  pointer-free lane state. It retains the load-time and DMEM-entry headers
  separately, exact native-word physical RDRAM, complete RSP memory and
  non-memory state, canonical low-12 SP PC, and rspboot work. HLE receives
  logical transactional access; only a consumed LLE lane exposes native
  backing.

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

The non-DSP commands also retain an evidence frontier. The public patent
describes SETBUFF/CLEARBUFF counts as 16-bit samples, while the released
`abi.h` does not name units and its CLEARBUFF, DMEMMOVE, and LOADADPCM macros
pack wider fields than their C structs name. Exact count units, DMA
alignment/rounding, zero-count behavior, DMEM wrap, move overlap, segment-table
initialization/persistence, and codebook/loop-state lifetime therefore remain
LLE differential questions rather than assumptions.

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

The first differential implementation will materialize only the physical
8 MiB RDRAM device for each lane. An audio task that requires a sparse
host/static alias outside physical RDRAM remains a loud frontier until the RSP
memory owner supports segmented copy-on-write storage. Speculative lanes do not
notify executable writes, submit DPC work, schedule completion, or touch live
device state; those effects occur once after comparison.

The snapshot constructor proves that caller-supplied boundary state is
internally consistent; it does not prove that rspboot historically produced
that state. Runtime integration must construct it only from the boot-overlay
handoff, initially grant exactly physical `0..8 MiB` DMA authority, and trap if
the task actually requires a static alias. Direct-IMEM tasks remain a loud
differential frontier. Moving the pure rspboot-to-entry capture behind the
`fn64-audio` seam is the next step toward making provenance structural.

## Work sequence

1. **Framing and outcomes** — complete the allocation-free command view,
   physical-RDRAM journals, exact identity, visible-state outcome, and
   first-divergence comparator.
2. **Standard wire decoder** — decode all 16 documented packets and reject
   unknown selectors/unsupported flag combinations without mutating state.
3. **Task-entry snapshot** — capture complete RSP memory/non-memory state and
   exact physical RDRAM once after rspboot, then fork independently owned LLE
   and HLE lanes. The owned types are complete; runtime capture wiring is next.
4. **Memory commands** — implement `SETBUFF`, `LOADBUFF`, `SAVEBUFF`,
   `CLEARBUFF`, `DMEMMOVE`, `SEGMENT`, and `LOADADPCM` with complete preflight
   and overlap/bounds tests.
5. **DSP commands** — add ADPCM, resample, mixer/envelope, interleave, and
   pole-filter behavior incrementally. Each arithmetic edge is accepted only
   through exact LLE differentials.
6. **Runtime policy** — add a non-release HLE/differential policy and
   transactional commit. Keep `LleAccuracy` as the default and release
   authority.
7. **End-to-end evidence** — run fixed-cycle framebuffer/audio/device/memory
   digests and zero-unsupported reports over representative games.

Each deterministic behavior claim requires ten consecutive clean runs. A
runtime synchronization change requires at least twenty and a comment naming
the closed interleaving. Private ROM-derived captures remain outside git; only
hand-authored public fixtures and non-content evidence belong in the tree.

The first memory-command differential matrix varies counts around
`0,1,2,7,8,15,16,17`, all low-three/low-four address alignments, both
DMEMMOVE overlap directions and exact aliasing, segment zero/nonzero and
base-plus-offset overflow, repeated SETBUFF with `A_AUX`, partial/oversized
codebook reloads, and SETLOOP pointer mutation before looped ADPCM. Paired
tasks establish whether any command state survives a task boundary.

## Release frontier

HLE is not release-authoritative until every admitted task in a representative
full-ROM run compares exactly and the installed policy is represented in the
release evidence wire. WM2000 additionally retains a documented harness
voice-map intervention; no result from that harness is a hardware-parity
certificate until the intervention is removed or independently certified.
