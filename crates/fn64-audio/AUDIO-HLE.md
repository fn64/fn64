# Audio HLE migration plan

Fn64's clean-room RSP interpreter is the audio accuracy authority. Audio HLE is
an optional optimization for an exact admitted microcode identity; it is not a
replacement for LLE evidence and it must never be selected from a familiar
opcode or task shape.

## Current boundary

`AudioTaskExecutionPolicy::LleAccuracy` runs rspboot and the live audio image,
commits the resulting RDRAM/DMEM/IMEM/SP state, persists the complete scalar
GPR and VU/accumulator/flag/divider image for the next task, and reports
measured RSP work to the device fabric. CPU/MMIO-owned semaphore, DMA, and DPC
latches are overlaid from the live fabric at each new interpreter entry rather
than restored from a stale duplicate. `osAiSetNextBuffer` later identifies the
PCM range consumed by AI. An HLE task therefore cannot be modeled as an
immutable RDRAM input that returns a `Vec<i16>`: its result is a transactional
set of machine effects.

The HLE foundation has ten independent pieces:

- `hle.rs` validates and iterates family-neutral 8-byte command framing without
  assigning semantics to an unknown family.
- `hle_outcome.rs` models the complete candidate task effect and compares it to
  the LLE effect in a stable first-divergence order.
- `hle_transaction.rs` owns checked 4 KiB DMEM and a logical-byte,
  copy-on-write RDRAM overlay. It produces canonical patches without mutating
  the live task input.
- `hle_executor.rs` consumes the sole standard-family HLE lane paired with its
  whole-task reference. It decodes only through exact admission, advances only
  public-contract memory state already proved by `hle_memory.rs`, and returns
  typed unknown-opcode, memory, DSP, or terminal/work frontiers before any
  guessed behavior. It owns no live mutation or DPC publication handle.
- `hle_effects.rs` defines the phase-neutral, content-bound IMEM replacement
  record used by both rspboot and ucode execution. Replacement journals retain
  complete images in DMA installation order without becoming scalar/vector
  architectural state.
- `hle_snapshot.rs` validates an owned post-rspboot capture and forks deep,
  pointer-free lane state. It retains the load-time and DMEM-entry headers
  separately, exact native-word physical RDRAM, complete RSP memory and
  non-memory state, canonical low-12 SP PC, and rspboot work. HLE receives
  logical transactional access; only a consumed LLE lane exposes native
  backing.
- `hle_lle.rs` consumes that isolated native lane and executes the loaded
  image through BREAK without access to the live device fabric, renderer,
  executable-write notifier, scheduler, interrupts, or timing. It returns
  complete final RSP state, exact written RDRAM coverage and logical patches,
  ordered ucode IMEM replacements, and deferred raw DPC submissions for later
  comparison and one-time commit.
- `hle_rspboot.rs` owns physical RDRAM and RSP memory while it executes the
  boot overlay to the first instruction of the image that overlay installed.
  It returns the neutral entry snapshot plus exact boot-phase RDRAM patches
  and ordered IMEM replacements; direct-IMEM and static-alias tasks remain
  typed loud frontiers.
- `whole_task.rs` consumes one pre-boot snapshot, runs pure rspboot once, forks
  the proven entry into admitted HLE and authoritative LLE lanes, and prepares
  a whole-task reference with no deferred DPC submission. It composes exact boot-plus-ucode write
  intent using final LLE bytes, so later ucode writes win overlaps while
  same-valued DMA writes remain visible to publication preflight. The value is
  deliberately not a commit token: the compact-family executor currently
  returns command memory effects, not a complete architectural task outcome.
- `hle_commit.rs` is quarantined as test-only characterization. Its reduced
  `RspVisibleState` projection does not bind complete architectural state, and
  its comparison does not consume the exact paired HLE lane. No production
  commit token or ABI adapter surface exists. Reintroduction requires complete
  RSP architectural state and a completed-result seal that consumes the paired
  same-snapshot `WholeAudioTaskHleLane`.

`AUDIO-ABI-CHARACTERIZATION.md` documents a separate private-input black-box
harness. It constructs hand-authored tasks, packets, and sentinels around the
same owned rspboot/LLE kernels. Request v2 separates independently cloned
control/probe trials from intentional cross-task persistence phases, binds the
common pre-rspboot baseline, and emits content-free exact first-divergence and
range evidence. It retains rspboot patches and DMA/IMEM-replacement journals
for every phase, so same-valued boot writes and persistence-phase provenance
remain part of trial equivalence. Ordered SP DMA journals are diagnostic-only:
they are excluded from architectural snapshots and commit authority, though
v2 compares them as evidence. Deferred DPC submissions drained before the
terminal machine snapshot are retained and compared separately. Private LLE
and rspboot/snapshot/memory-install failures cross this boundary through
exhaustive static variant names without addresses, PCs, counts, digests, raw
command words, or other error context. The v2 RDRAM difference union covers
both boot and ucode patches from both lanes.

The standard ABI decoder is a separate layer. Possessing a valid standard
packet does not prove that the loaded microcode implements that packet family.

`compact_memory_abi.rs` records a second, narrower exact-image result without
promoting it to a family catalog entry. A private same-snapshot WM2000 control
replay terminated through BREAK and duplicate trials matched complete RDRAM,
DMEM, IMEM, architectural state, DPC state, and work. An 84-case address
redirection sweep then covered every 32 load-shaped and 52 save-shaped packet
in that captured task. Each transfer used an eight-byte-masked 24-bit RDRAM
address, a 16-byte length quantum from the high payload byte, and DMEM
`0x04f0 + low16`; all six distinct observed field/DMEM combinations fit that
rule. Ten consecutive fresh 84-case sweeps produced byte-identical
content-free reports. A compact LOAD then seeded controlled nonzero DMEM
immediately before a truncated opcode-2 task tail. Counts `0..=16` cleared one
16-byte block, `17..=32` cleared two, and the `33`, `367`, `368`, and `369`
boundaries followed a minimum-one-block, round-up-to-16 rule; each additional
block cost four RSP instructions. Ten consecutive fresh runs of that
14-boundary matrix produced byte-identical content-free reports. The module
then applied the same seeded boundaries to opcode `0x0a`: it used the same
minimum/round-up rule, with seven RSP instructions per additional block. A
four-case direct-versus-scratch-copy matrix matched non-overlap, backward
overlap, and exact alias, while forward overlap differed in exactly the bytes
predicted by ascending 16-byte block copies. Both matrices produced
byte-identical content-free reports in 10/10 fresh runs. The module implements
those four speculative transactional memory commands. The four opcode-`0x0b`
occurrences in the same task each issued one 32-byte RDRAM read to fixed DMEM
`0x03f0`. Redirecting each occurrence independently preserved BREAK and work;
ten consecutive fresh four-case sweeps produced byte-identical content-free
reports. A separate positive-count matrix at `1`, `2`, `7`, `8`, `9`, `15`,
`16`, `17`, `31`, `32`, `33`, `47`, `48`, `49`, `255`, `256`, and `257`
issued raw DMA length `count - 1` to the same fixed destination, which gives
the public hardware's round-up-to-eight transfer size. It too produced
byte-identical content-free reports in 10/10 fresh runs. The module implements
that LOADADPCM table load transactionally; zero count remains uncharacterized
after it left the bounded valid-task path. A 177-block opcode-`0x0d` dependency
sweep then perturbed every 16-byte block in the compact audio region. Only two
368-byte inputs participated: compact offsets `1248..1616` and `1616..1984`.
Their halfwords mapped without byte or arithmetic transformation to the even
and odd halfwords, respectively, of the 736-byte output at compact offset
zero. A four-case first/last-block matrix for both channels checked the exact
two-byte output digests against the seeded input slices; every pair was
identical, and 10/10 fresh matrix runs produced byte-identical content-free
reports. `compact_dsp_abi.rs` implements that fixed zero-operand INTERLEAVE
transactionally. The remaining 52 opcode-`0x0c` packets formed ten distinct
field shapes. A same-snapshot sweep checked 43 independently hashed changed
ranges and established two fixed 368-byte buffers at compact offsets from
DMEM `0x04f0`, a signed low-16 gain, and this saturating sample operation:
`(prior * 32767 * 2 + 0x8000 + input * gain * 2) >> 16`. The implementation
uses a wide accumulator before the final signed-16 clamp. Opcode `0x0e` is not
the standard ABI pole filter for this identity: a 12-case field sweep showed
that it ignores both `word0` payload and the high half of `word1`, and stores
only the low half of `word1` as a big-endian scalar at DMEM `0x0fea`. Ten
fresh runs of that matrix were byte-identical.

`compact_abi.rs` now dispatches all eight selectors present in the captured
164-command list: `2`, `4`, `6`, `10`, `11`, `12`, `13`, and `14`. It stages
each 320-byte command batch at DMEM `0x02b0`, matching the observed command
processor rather than comparing only command effects. The content-safe
`audio_compact_verify` tool ran admitted compact HLE and LLE from the same
post-rspboot snapshot; all 164 commands decoded, terminal DMEM was identical,
and canonical RDRAM write coverage and bytes were identical. Ten consecutive
fresh whole-list runs produced byte-identical reports. This closes the command
semantics represented by that captured list. Zero transfer quanta for
LOAD/SAVE, a distributable exact identity catalog, complete terminal
scalar/vector/SP state and work equivalence, and live policy selection remain
loud frontiers. Private task inputs and reports remain outside the repository.

The 2026-08-01 private OoT fixture attempt did not establish a valid synthetic
task entry. Bounded VROM materialization recovered the named rspboot, text, and
data images, and the standalone interpreter executed the complete text image
to BREAK. The characterization path instead exceeded its ucode step bound
after rspboot placed the supplied task text at IMEM `0x1080`; that placement
does not preserve the image's observed absolute-control-flow layout. An
8-byte-aligned source-offset sweep was not an answer: several offsets reached
BREAK without performing any command DMA, and the sole longer terminating
candidate performed one DMA but no output write. This rules out adopting a
terminating offset as task geometry. The exact next step is to capture one
real audio `OSTask` at the existing common pre-policy dump boundary and use
its header geometry and referenced images as the private characterization
input.

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
internally consistent; provenance is structural only when the pure rspboot
kernel constructs it. Runtime integration must feed that kernel the complete
pre-boot RSP state, initially grant exactly physical `0..8 MiB` DMA authority,
and trap if the task actually requires a static alias. Direct-IMEM tasks remain
a loud differential frontier.

The verified ucode-phase adapter does not publish deferred DPC work. The
available staged-RDRAM batch loses per-CMD_END memory/device timing, interrupt
and FullSync order, and exposes its synthetic suffix to RDP addressing; it is
therefore render-only diagnostics. Reference reports `DiagnosticOnly`, RT64
reports `Unsupported`, and any deferred DPC submission traps before renderer,
device, or RDRAM mutation. DPC publication remains blocked on a native
separate-command-buffer seam plus temporal device observations.

For DPC-free phases, the adapter requires typed ownership of the matching
`InFlight` task address, process-monotonic load generation, and `Running`
lineage, then rechecks all three atomically at publication. Planned writes overlapping a
live executable region reject before mutation because native generation
installation is fallible and no transaction spans it with RDRAM/device state;
ordinary non-executable audio-data writes may publish through the existing
post-commit notification seam.

This adapter is still not selected by live audio-task policy. Its authority
begins after rspboot, so it cannot claim whole-task atomicity or publish
rspboot's earlier effects. The pure whole-task reference now retains the
pre-boot baseline, boot and ucode write intent, ordered replacement history,
complete final LLE state, and an inseparable no-submission seal, but exposes no
publication authority until a concrete HLE lane produces and exactly compares
a complete task outcome. The compact verifier's DMEM/RDRAM comparison is
deliberately narrower than that boundary.
`LleAccuracy` replaces the cross-task owner with the
exact terminal image after draining DPC submissions. Optimized boot-overlay HLE
exposes no post-ucode scalar/VU image, so successful completion is explicitly
labeled `HleCompatibility` and carries the rspboot-entry image with the
consumed overlay continuation cleared. A renderer failure leaves the owner
`InFlight`; the next task traps instead of fabricating a reset. Direct-IMEM HLE
is labeled `HleCompatibilityUnavailable` and likewise cannot silently reuse a
prior exact snapshot. These HLE labels are bounded residuals, not release
accuracy claims.

## Work sequence

1. **Framing and outcomes** — complete the allocation-free command view,
   physical-RDRAM journals, exact identity, visible-state outcome, and
   first-divergence comparator.
2. **Standard wire decoder** — decode all 16 documented packets and reject
   unknown selectors/unsupported flag combinations without mutating state.
3. **Task-entry and whole-task reference** — run pure owned rspboot once,
   capture complete RSP memory/non-memory state and exact physical RDRAM at its
   handoff, fork independently owned LLE and HLE lanes, and retain an
   overlap-correct whole-task LLE reference with no deferred DPC submission.
   DPC register changes remain part of the compared final RSP state rather than
   being implied absent by that seal. The ABI can now acquire a
   non-cloneable exact-generation pre-boot owner without publishing boot
   effects. Candidate comparison and atomic publication remain next. The
   standard executor consumes its lane into typed frontiers but cannot produce
   a completed-result seal or commit authority.
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

The first memory-command differential matrix uses v2 trial zero as the
same-baseline control and varies counts around
`0,1,2,7,8,15,16,17`, all low-three/low-four address alignments, both
DMEMMOVE overlap directions and exact aliasing, segment zero/nonzero and
base-plus-offset overflow, repeated SETBUFF with `A_AUX`, partial/oversized
codebook reloads, and SETLOOP pointer mutation before looped ADPCM. Paired
phases inside each persistence trial establish whether command state survives
a task boundary. The report identifies exact RDRAM write-coverage, DMEM, IMEM,
architectural-state, journal, or work divergence without exposing guest bytes.

## Release frontier

HLE is not release-authoritative until every admitted task in a representative
full-ROM run compares exactly and the installed policy is represented in the
release evidence wire. WM2000 additionally retains a documented harness
voice-map intervention; no result from that harness is a hardware-parity
certificate until the intervention is removed or independently certified.
