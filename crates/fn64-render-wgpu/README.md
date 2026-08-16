# fn64-render-wgpu

`fn64-render-wgpu` is the pure-Rust GPU backend being built to replace the
quarantined RT64 C++ adapter. The current raw-DPC surface is intentionally small:
it decodes a bounded raw-DPC subset into ordered typed commands, a
transaction-local RDP state delta, and an exact resource plan. M3.1's headless
wgpu 30 fixture remains byte-for-byte frozen as a separate lifecycle bridge.

The decoder admits only the eight low no-op variants, fill-cycle Set Other
Modes, Set Color Image, Set Fill Color, whole-pixel Fill Rectangle, and
FullSync. The two non-command bits in the wire opcode are ignored for command
selection, and command stepping uses `fn64-render::raw_rdp_command_width`.
FullSync preserves its unassigned payload bits; the command identity and the
IR-owned completion observation are the admitted semantics.
Unsupported, unknown-width, truncated, and state-invalid commands fail with
workload, stream, chunk, source-byte-offset, and wire-opcode identity. A fill
requires transaction-local fill cycle, color image, and fill color state;
RGBA16/32 row ranges are planned exactly. The input journal must equal the
entire ordered plan, including operation identities. No durable state is
mutated: decode consumes the submitted ticket, and staged state is move-only,
queue-bound, submission-ordinal-bound, and exact-successor-sequence-bound.

M4.1 additionally stages the public Set Texture Image, Set Tile, Set Tile
Size, Load Sync, Load Block, Load Tile, and strict public-macro Load TLUT
forms. M4.2.0 binds each admitted Load Block/Load Tile to two distinct views:
an ordered immutable list of complete 64-bit transfer words, and a canonical
sorted/disjoint union of physical TMEM bytes touched by those words. Each word
retains its exact source-access/offset, defined-source-byte mask, destination
word, DXT or tile-row advance, starting-TL-aware odd-row exchange, and linear
or split-bank physical fragments. The union is the resource-journal effect;
it deliberately deduplicates repeated destinations while the ordered list
does not. Thus a wrapped, overwritten, or differently ordered transfer cannot
borrow another transfer's checked plan merely because it touches the same bytes.

Public RDP documentation defines row padding to complete 64-bit transfers but
does not define padding values. The contract therefore distinguishes logical
defined-content masks from whole-word effect coverage; it does not mark
padding as defined texture content or invent zeroes. Texture Image alone owns
source format, size, width, and address even when Set Tile declares another
size. Direct four-bit loads fail loudly because the public guidance requires a
16-bit load form. RGBA32 plans use low/high 2 KiB bank fragments and reject a
load base outside low TMEM. YUV retains its exact owned sources but has no
physical destination contract in this slice: YUV pairing and descriptor
constraints still require a public-authority contract. Load TLUT does have a
destination contract: its starting destination tile must be in high TMEM
(`tmem >= 256`, admission-gated), and once admitted, M4.3.1c wraps the
running destination word across the full 512-word TMEM domain (word 511's
successor is word 0) rather than staying inside the high half -- an explicit
RT64/reference (`write_tlut`) parity policy, not a proven silicon fact; see
`crates/fn64-render-wgpu/src/tmem/types.rs`'s `project_tlut_full_domain_word`.

`RawDpcResourcePlan::bind_tmem_transfer` returns an immutable checked view. It
revalidates workload, journal, submission, memory layout, source slice,
canonical destination slice, operations, counts, byte totals, and both slice
identities before exposing ordered words. It is deliberately not a consuming
execution capability.

M4.2a separately adds renderer-owned 4 KiB physical TMEM storage, per-byte
validity, per-byte last-touch generation, a durable generation, and the last
published load epoch. One move-only packet transaction clones the durable
candidate once, chains every accepted Load Block/Load Tile in command order,
and consumes only M4.2.0's exact word/fragments. The state layer never
recalculates DXT carries, odd-row XOR4 placement, or RGBA32 bank mapping.
M4.2b's LoadTile and M4.2c's LoadBlock executors jointly own the
crate-private logical-source to physical-lane assertion. That assertion carries eight explicit optional
physical lanes and validates the accepted physical defined-lane mask: in a
split-bank word, lanes 0..4 are the low fragment and lanes 4..8 are the high
fragment, so a four-source-byte RGBA32 tail has physical mask `0x33`, not the
logical prefix `0x0f`. The state engine validates only that physical shape; it
does not rearrange captured source bytes. Each payload is move-only and bound
to uncaller-chosen packet/load transaction identities plus the exact state,
source, queue, submission, generation, epoch, and source/destination access
plans, so equal geometry cannot rebind bytes across transactions. Defined
lanes write bytes and become valid;
undefined complete-word tail lanes preserve unobservable backing bytes, clear
validity, and still stamp the next generation.

Each completed load immediately snapshots canonical device-local effects, so
a later overlapping load cannot retroactively change the earlier digest.
Invalid data is normalized to zero in that projection, while validity, load
epoch, and touch generation remain digest inputs. A `CompletedWrite` byte count
is the entire declared physical destination access, including invalid lanes;
it is not the number of defined source bytes. Its content-digest preimage is
exactly the big-endian byte count, big-endian load epoch, normalized postimage
bytes, one-byte validity flags, and big-endian per-byte touch generations.
Render-IR owns the frozen renderer-neutral domain and helper for that preimage;
wgpu does not introduce a backend-specific content identity.
State allocation, packet transaction, workload, journal, queue, submission,
and access identities are deliberately excluded from that content digest and
remain bound by the proposal identity, `CompletedWrite` access, and lifecycle
tickets. Thus identical physical postimages with identical validity/epoch/touch
semantics have one content digest across state allocations and submissions.
Sealing requires exact ordered coverage of every `TmemLoadDestination` journal
write. The pending owner exposes immutable `CompletedWrite` proposals for
later report assembly, but does not create a `BackendEffectReport` or issue any
receipt. It becomes publication-ready only after an exact `GpuCompleteTicket`
report contains those writes; durable bytes publish once, after the matching
`GuestCommittedTicket`, under an exclusive authority that rejects a different
state identity or stale base generation.

The stale-publication test exercises the exact sequential interleaving where
transactions A and B stage from generation N, B publishes N+1, and A is then
rejected. It does not run publication concurrently across threads, so
concurrent publication remains explicitly unverified in M4.2a.

M4.2b and M4.2c jointly own mapping one checked M4.2.0 transfer's source bytes
into M4.2a's physical TMEM lanes: M4.2b for LoadTile
(`tmem/execute/load_tile.rs`), M4.2c for LoadBlock (`tmem/execute/load_block.rs`).
Each executor consumes one checked transfer together with the exact submitted
packet's M4.0-owned guest reads. Preparation (`prepare_load_tile` /
`prepare_load_block`) matches every global access index, operation, RDRAM
range, layout, and submission before it retains source bytes, returning a
one-use `PreparedLoadTile` / `PreparedLoadBlock`. Execution consumes that
prepared operation and an already-`StagedTmemTransaction` (freshly staged for
a first load, or chained onto a packet an earlier load already staged) — the
submitted/captured-read binding is exact: source identity, queue identity,
submission ordinal, load epoch, and every ordered transfer word must match
the staged transaction bit-for-bit, or execution fails closed with a typed
`StagedBindingMismatch` before touching any physical byte.

Both executors map row-local complete 64-bit words in command order and never
spill a partial row's bytes into the next row; a row's undefined physical
lanes are still touched (their generation advances) and invalidated by M4.2a,
never zero-filled or marked valid. Two source layouts are mapped, identically
between the two executors:

- **Linear, DXT/starting-TL-aware odd-row XOR4.** When a word's
  `odd_row_exchange()` bit is set (starting-`T`-relative odd row, driven by
  DXT row advance), the two 4-byte halves of that 64-bit word swap physical
  lanes — source byte `n` lands at physical lane `n ^ 4`. This holds for a
  **full** word (all 8 lanes defined, mask `0xff`: both 4-byte halves swap)
  and for a **partial** tail word (e.g. a 2-byte RGBA8 remainder, mask
  `0x03`): the exchange still applies, so the payload lands at physical lanes
  4-5, not the unexchanged logical-prefix lanes 0-1. `physical.rs`'s own
  `physical_defined_lane_mask` independently derives the same rotated mask
  (`rotate_left(4)`), so a correctly-exchanged payload is cross-checked by an
  independently-derived authority rather than trusted once.
- **RGBA32 split.** A 32-bit-per-texel word's four source byte pairs
  interleave into TMEM's low/high 2 KiB banks: lanes 0-1 and 4-5 land in the
  low bank, lanes 2-3 and 6-7 in the high bank (`TmemTransferPhysicalWord::SplitBanks`),
  independent of the Linear odd-row path above.

Each executor's result carries only the transaction-local candidate plus
ordered physical fragment descriptors. A packet transaction chains any number
of LoadTile/LoadBlock executions in command order; the chain is move-only —
each `execute` call consumes its `PreparedLoadTile`/`PreparedLoadBlock` and
the current `StagedTmemTransaction`. A wrong-kind preparation fails before
staging and leaves the packet transaction caller-owned and untouched. An
execute-time binding mismatch or rejected/poisoned word payload drops the
consumed chained candidate and leaves the packet's durable generation,
last-published load epoch, and every physical byte's validity exactly as they
were before that load began — never partially applied. This holds however many
loads completed transaction-locally earlier in the same chain.

Provenance for this mapping is unchanged from M4.2a/M4.2b: the public SGI
*Nintendo 64 RDP Command Summary*, Tables 1, 3, and 6-10, and Programming
Manual section 13.9 for hardware fields and transfer rules; project design
docs for move-only commit sequencing. RT64 is not hardware authority for this
state.

Neither executor constructs a `BackendEffectReport`, issues a lifecycle
receipt, or publishes durable state — that remains M4.2a's publication
authority after an exact `GpuCompleteTicket`/`GuestCommittedTicket` pair.
Neither executor uploads to a GPU or issues a GPU dispatch; both are
CPU-only, transaction-local byte mapping. Neither handles TLUT or YUV
destinations. Neither this slice nor its combination with M4.2a establishes
visual parity or performance; no such claim is made anywhere in this
document.

`tmem::execute_ordered_tmem_loads` (`tmem/execute/packet.rs`) is the packet-level
outer loop neither executor nor M4.2a itself owns: it validates a decoded
raw-DPC command stream in one pass, then — only if every command in it clears
validation — walks it again in order, dispatches each `LoadTile`/`LoadBlock`
to its executor, and chains the resulting packet transactions into one sealed
`PendingTmemTransaction` — the N-load generalization of each executor's
single-load case. The validation pass first checks the caller-supplied
`SubmittedTicket` is exactly the decoded packet's own ticket (queue,
submission ordinal/identity, workload, journal, and memory-layout identity),
rejecting a foreign or reordered ticket with a named `SubmissionMismatch`
error before any other exit; it then rejects a `LoadTlut` command or a
YUV-deferred Tile/Block contract before any load in the packet stages. Since
M4.3.1, `bind_tmem_transfer` returns a valid, bindable transfer plan for TLUT
loads (the destination transfer-plan is closed), but no physical executor
(`prepare_load_tlut`, M4.3.2) exists yet to write TMEM from it, so this loop
refuses it as an explicit scope boundary rather than executing a load it
cannot back — the same treatment a YUV-deferred Tile/Block contract already
receives. Because rejection is decided by a validation pass that runs to
completion before the execution pass stages anything, a later TLUT/YUV
command can never be preceded by an already-staged earlier Tile/Block load.
Like the executors it chains, this function stops at `PendingTmemTransaction`:
it holds neither `BackendCompletionAuthority` nor `GuestCommitAuthority`, so
the caller assembles the packet-wide `BackendEffectReport` (TMEM proposals
from `pending.proposed_effects()` plus any other declared writes from the
same packet) and drives the remaining ticket/publication steps itself.

M3.3a freezes the contract immediately after that decoder. Its only admitted
candidate is an exact synthetic 4x2 RGBA16 red fill: 8 MiB installed RDRAM,
commands at `0x100..0x128`, color writeback at `0x400..0x410`, transaction
sequence 7, and the exact ten command words exported by
`NATIVE_FILL_COMMAND_WORDS`. The logical/device RGBA16 result is `[f8 01] x8`.
The ABI transaction does not accept those logical bytes: its existing commit
loop flat-copies staged effects into N64Recomp's native-word RDRAM allocation,
so `N64RecompRdramStorageBytes` instead carries `[01 f8] x8`. `RdramView`
mechanically proves that backing storage reads as eight logical `0xf801`
pixels. The frozen backing-storage, native RGBA8, and post-VI BGRA8 SHA-256
values are respectively `007d65aa7365956d4ae38da6ee8849b14b7a5d88658adfb49df757255249f248`,
`5ed2cb747cf2014feda8638a6894704f15eb46c867ce7bae38d0447556f80549`,
and `f9d2bc2ea8345a97d8a514eae7f50c165175355a80ca805309429d83748f7ee2`.
The render-IR workload, raw-stream, and journal identities are also frozen as
`08dc8fbed0143100b556b7b8bce27a31b78ff5e7bb1f0c914e29963275eb22d0`,
`057b789d4989fe90faf753f8f6802db8aa64b94249dadffdda8e3a70ff4753d1`,
and `1206767d7c857d57832d88bb557a450d0e8f3fb331669e827316b676db83bc50`.

The linear ownership path is `DecodedRawDpc -> PreparedNativeFill ->
InFlightNativeFill -> PendingNativeCommit -> guest-owned commit ->
CommittedNativeFrame`. Preparation compares the decoder's retained predecessor
with an exclusively borrowed `NativeDurableState`. Backend modules alone may
advance prepared work or report GPU output. `DeviceRgba16Bytes` can never be
mistaken for `N64RecompRdramStorageBytes` at the guest boundary. Pending work
is assembled only from the separately named crate-private `from_device_bytes`
and `from_n64recomp_storage_bytes` constructors; the native-output constructor
accepts those types rather than adjacent raw vectors. It transfers its
`GpuCompleteTicket` and immutable backing-storage bytes through a guest-owner callback;
only the exact returned `GuestCommittedTicket` publishes the RDP delta, target
identity, generation, and last-commit lineage. Decode rejection, target/raster
failure, incorrect output, callback failure, a hostile receipt, or dropping any
pre-commit state leaves the prior renderer state unchanged. The renderer never
receives live guest-memory authority, interrupt/scheduler authority, or a guest
receipt-issuing capability.

`DecodedRawDpc` now has one crate-private consuming decomposition for this
path. It retains the immutable predecessor state as part of its proof, while
the public `into_staged_state` path remains a mutually exclusive speculative
decode choice. Native preparation rejects a decode produced from a consumed
speculative predecessor because that route cannot return the prior staged
token on later failure. It consumes a durable-origin decoded owner and checks
the predecessor, delta-derived staged state, queue, submission ordinal, and
transaction sequence against the exclusive durable owner. Consequently the
same submission cannot be sent both to native execution and staged chaining,
and a staged result cannot become durable before guest commit.

M3.3b adds CPU-only native color-target ownership beneath that contract.
Typed keys bind the installed-memory layout, physical range, extent, and
RGBA16/32 format. Exact row plans are distinct from move-only completed-write
capabilities; the latter bind target key, generation, full range, device-byte
domain, and exact byte count before a resident generation can be published.
The completed-write type intentionally has no production constructor yet, so
planning cannot masquerade as raster completion. The RGBA5551/RGBA32 CPU
pack/unpack oracles and the M3.3a `DeviceRgba16Bytes` narrowing seam are
executable mechanism evidence only.

M3.3d adds one exact CPU-only VI/capture mechanism after that contract. It
admits the complete fourteen-word register image for the synthetic 4x2 RGBA16
target at origin `0x400`, stride four, a progressive field, 1:1 U2.10 scale,
replicate mode, and every optional VI filter disabled. Its oracle consumes only
M3.3a's typed `DeviceRgba16Bytes` domain and produces the frozen tightly packed
BGRA8 bytes; N64Recomp backing-storage bytes cannot cross that API. VI and
capture have separate typed resource plans. The capture extractor requires the
exact presentation/capture identity and strips two 256-byte padded GPU-copy
rows without admitting padding as visible pixels. Every fixed register,
manager control, identity component, extent, pitch, and byte length has hostile
mutation coverage. A repository-owned WGSL implementation parses and validates
under Naga 30; M3.3d alone does not wire or execute it.

M3.3c wires the exact M3.3a transaction through one real wgpu queue. Device
creation prewarms a repository-owned 4x2 RGBA16 fill pipeline and M3.3d's
bounded replicate pipeline; submission creates no pipeline. One in-flight
owner binds the decoded workload, queue/submission/transaction identity,
target key, target generation, physical range, native ordinal, and wgpu
`SubmissionIndex`. It can mint target-completion authority only after the
exact indexed wait, that encoder's completion callback, a 48-byte bounded
readback, and byte validation. The first 16 readback bytes remain the typed
logical/device RGBA16 domain. N64Recomp storage bytes are produced only by the
explicit adjacent-byte conversion, while native RGBA8 comes from the typed
RGBA5551 oracle. The final 32 bytes are checked against both M3.3d's typed CPU
oracle and its frozen BGRA8 fixture.

Target publication is prepared before the guest ticket moves. Its move-only
capability retains an exclusive registry borrow, so no predecessor, alias, or
capacity mutation can make publication fail after a successful guest commit.
Dropping or rejecting any earlier state publishes neither the target registry
nor `NativeDurableState`. A required host with no native adapter fails the GPU
test through the typed `NoAdapter` outcome; it is never counted as a skip.

The combined contract is deliberately not a general implementation of raster,
guest dispatch, a live `ViPresentation` adapter, ordinary headless capture, or
surface presentation. M3.3c proves only the exact synthetic target allocation,
RGBA16 fill, guest writeback, and fixed GPU VI mechanism described above. It
admits no depth image or depth write, no TMEM, textures,
blending, coverage, multisampling, ray tracing, interlacing, resampling,
optional VI filters, surface path, or performance/parity claim. Its byte-exact
synthetic fixture is mechanism evidence, not the required real captured
workload. It consumes no M2.5 shader artifact: the M3.3d WGSL is a separately
reviewed repository mechanism only, while any RT64 HLSL corpus claim must wait
for M2.5's complete 56-artifact receipts.

The retained GPU fixture is a lifecycle proof, not a broad RDP implementation.
It continues to require the exact M3.1 eight-word DRAM stream: Set Color Image,
Set Fill Color, Fill Rectangle, and FullSync, with exact wire words apart from
the fixture's selected fill-color value and one exact 16-byte RDRAM effect. Its
FullSync remains at byte 24 and its observation timeline is exactly `CMD_END ->
FullSync -> DP interrupt`. The fixed host evidence vector preserves RGBA byte
order as `21 3c 4d 59` for each of four pixels. Effect bytes use render-IR's
canonical digest, shared with the M1.2 guest-staging adapter. This slice does
not claim TMEM, persistent framebuffer ownership, broad raster, live or GPU
VI, surface presentation, RT64 parity, or performance. Those remain later work in
[`../../docs/RENDER-WGPU-PORT-PLAN.md`](../../docs/RENDER-WGPU-PORT-PLAN.md).

The lifecycle keeps the renderer's paired backend-completion authority private.
One in-flight type owns the semantic ticket and wgpu `SubmissionIndex`; it can
yield a completion only after an exact indexed wait, completion-callback
observation, bounded readback, and byte validation. Every output byte must be
covered by the packet's resource journal before the backend effect receipt is
issued. Dropping any earlier state cancels the synthetic operation without a
guest commit.

Ordinary tests are GPU-independent. The host test is explicit:

```sh
scripts/guarded-cargo-test.zsh -p fn64-render-wgpu --features host-gpu-tests
```

A machine with no selected native adapter returns the typed `NoAdapter`
outcome; that is unsupported host evidence, not a skipped or passing GPU
claim.

Provenance: command fields, load-word layout, DXT, and fill-cycle rules use the
public SGI *RDP Command Summary* and the public Nintendo 64 Programming
Manual section 13.9, plus public libultra `gDPSetCycleType`, `gDPSetColorImage`,
`gDPSetFillColor`, `gDPFillRectangle`, and `gDPFullSync` descriptions. State
field interpretation follows the permitted MIT RT64 semantic source pinned by
the port plan; no RT64 code is copied. The shader is a repository-owned
mechanism fixture. No RT64 shader, C++, CMake, DXC artifact, GPL runtime
implementation, texture hasher, game content, or excluded tool is used here.
M3.3d's VI register/scale and RGBA5551 sources are recorded beside its code;
its 256-byte row pitch is explicitly a wgpu mechanism rather than console
behavior.
