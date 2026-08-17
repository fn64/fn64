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

Neither the LoadTile nor LoadBlock executor constructs a `BackendEffectReport`,
issues a lifecycle receipt, or publishes durable state — that remains M4.2a's
publication authority after an exact `GpuCompleteTicket`/`GuestCommittedTicket`
pair. Neither executor uploads to a GPU or issues a GPU dispatch; both are
CPU-only, transaction-local byte mapping. Neither this slice nor its
combination with M4.2a establishes visual parity or performance; no such claim
is made anywhere in this document.

M4.3.2 adds a third executor, `prepare_load_tlut`/`PreparedLoadTlut::execute`
(`tmem/execute/load_tlut.rs`), that maps one checked LoadTLUT transfer's exact
packet-owned guest reads into M4.2a's physical TMEM the same way M4.2b's
LoadTile and M4.2c's LoadBlock already do. TLUT's mapping is a different
shape from Block/Tile's one-source-byte-per-destination-byte copy: each
entry's 2 captured source bytes (a big-endian 16-bit palette value) are
quadricated into all four 16-bit lanes of that entry's 8-byte high-bank
destination word — `[hi, lo, hi, lo, hi, lo, hi, lo]` — never split-bank and
never odd-row-exchanged, matching M4.3.1's frozen `Linear64` transfer-plan
geometry (row advance equals the entry index directly, with no row/DXT
accumulation grouping — unlike Block/Tile's row-grouped advance) and
M4.3.1b's `defined_destination_byte_mask() == 0xff` fact. Like its siblings, this executor constructs no
`BackendEffectReport`, issues no lifecycle receipt, publishes no durable
state, and returns only transaction-local state plus ordered physical
fragment descriptors.

`tmem::execute_ordered_tmem_loads` (`tmem/execute/packet.rs`) is the packet-level
outer loop none of the three executors nor M4.2a itself owns: it validates a
decoded raw-DPC command stream in one pass, then — only if every command in
it clears validation — walks it again in order, dispatches each
`LoadTile`/`LoadBlock`/`LoadTlut` to its executor, and chains the resulting
packet transactions into one sealed `PendingTmemTransaction` — the N-load
generalization of each executor's single-load case. The validation pass first
checks the caller-supplied `SubmittedTicket` is exactly the decoded packet's
own ticket (queue, submission ordinal/identity, workload, journal, and
memory-layout identity), rejecting a foreign or reordered ticket with a named
`SubmissionMismatch` error before any other exit; it then rejects a
YUV-deferred Tile/Block contract before any load in the packet stages.
Because rejection is decided by a validation pass that runs to completion
before the execution pass stages anything, a later YUV command can never be
preceded by an already-staged earlier Tile/Block/TLUT load. Like the
executors it chains, this function stops at `PendingTmemTransaction`: it
holds neither `BackendCompletionAuthority` nor `GuestCommitAuthority`, so the
caller assembles the packet-wide `BackendEffectReport` (TMEM proposals from
`pending.proposed_effects()` plus any other declared writes from the same
packet) and drives the remaining ticket/publication steps itself.

M4.3.3a adds `tmem::RawTexel`, a format-neutral raw-value carrier, and
`tmem::decode_direct_texel`, a pure, allocation-free decoder for exactly the
seven console "direct" `(format, size)` pairs that read one texel's color
straight out of TMEM without a palette lookup: RGBA16, RGBA32, IA4, IA8,
IA16, I4, and I8. `RawTexel::try_new(size, value)` is the sole constructor;
its fields are private, so a caller cannot assemble an already-invalid
instance. It loudly rejects (typed `RawTexelError`, never masked or
truncated) a `value` that does not fit `size`'s defined bit width — 4, 8, 16,
or 32 bits — and multi-byte values combine big-endian, matching the rest of
this crate's `to_be_bytes`/`from_be_bytes` convention. `RawTexel` takes no
position on which `(format, size)` pairs are meaningful, so M4.3.3b's CI/TLUT
functions below and a future YUV layer can reuse the same carrier instead of
inventing format-specific raw wrappers.
`decode_direct_texel(format, raw)` then classifies `format` against an
already-width-valid `RawTexel` and maps it to one `DecodedTexel` RGBA8888
color, or a typed `DirectTexelDecodeError` naming why the pair is not one of
the seven direct pairs: `IndexedDecodeIsSeparate` for `ColorIndex` (decoded
through M4.3.3b's separate, TLUT-aware `resolve_indexed_texel` path),
`YuvConversionDeferred` for `Yuv` (per M4.3's scope), and `UnsupportedPair`
for every other combination (for example 4-bit or 8-bit direct RGBA, which
the console does not define as real formats). All three types and both
functions use no `Option`, no `unsafe`, and no allocation. Nothing here reads
TMEM, RDRAM, a CI palette, a TLUT, or a GPU; this is a pure function of an
already-isolated raw texel value.

Decode formulas are transcribed from the permitted MIT RT64 Rust-port source
pinned at commit `5473732a822a4423b5696e7cb18fecc425a59875`
(`docs/RT64-PORT-AUTHORITY.md`): `src/shaders/Formats.hlsli` (digest
`9b5765371d19de1e410dbe919433922db975994e2a6077bf9e499a8a94f33b7b`) for
`I4ToFloat4`, `IA4ToFloat4`, `I8ToFloat4`, `IA8ToFloat4`, `RGBA16ToFloat4`,
`IA16ToFloat4`, and `RGBA32ToFloat4`; and `src/shaders/TextureDecoder.hlsli`
(digest `63b2c1ce683e7e7880c9508d3232d90e90236157ac86ae91947c62ae1d359f07`),
whose `sampleTMEM4b`/`sampleTMEM8b`/`sampleTMEM16b`/`sampleTMEM32b` select
those functions. Which `(format, size)` pairs are legal at all is the public
SGI *RDP Command Summary* Table 4 image-data-format legality matrix. The
format and size selector encodings dispatched on here are not owned by this
module: SetTextureImage (Table 3) and SetTile (Table 6) each define both the
`G_IM_FMT_*` format field and the `G_IM_SIZ_*` size field on their own
command word, and both are already transcribed in this crate at
`ImageFormat`/`PixelSize` (`tmem/wire.rs`) — this module reuses that prior
transcription rather than re-deriving selector values. Exact source line
ranges are cited at each decode function in `tmem/texel.rs`. RT64's
TMEM-address and four-bit RGBA/I aliasing ("not a real format, replicated by
observing hardware behavior") are read for citation only and are out of this
slice's scope. The pinned source's `sampleTMEM`
(TextureDecoder.hlsli:149-208) supplies M4.3.3b's observed CI branch: a TLUT
lookup occurs only while a table is active, while disabled CI aliases to
intensity. M4.3.3b implements that branch only for CI4/CI8. Although RT64 also
exhibits disabled CI16/CI32 intensity aliases, this slice deliberately rejects
CI16/CI32 rather than silently widening the admitted pair set. No RT64 code is
copied; only the numeric behavior is transcribed, matching this crate's
existing `raster.rs` and `state.rs` provenance convention.

M4.3.3b extends that pure-value layer without crossing into physical TMEM.
`OtherMode::texture_lut_mode()` decodes high bits 15:14 into the typed
`TextureLutMode::{Disabled,Rgba16,Ia16}` set and rejects reserved encoding 1.
`unpack_ci4_texel` extracts the high nibble for even columns and low nibble
for odd columns from an already-isolated eight-bit packed value.
`resolve_indexed_texel` admits only CI4/CI8, combines CI4's typed four-bit
palette selector with its nibble, and ignores the selector for CI8. Disabled
TLUT mode aliases the normalized eight-bit index to I8, including CI4's
composite palette/index value. Enabled modes return a `TlutLookup` containing
the index, RGBA16/IA16 entry interpretation, and canonical quadricated entry
address `0x800 + index * 8`; they never fall back to direct decode.
`decode_tlut_entry` accepts that lookup plus exactly one caller-supplied
big-endian 16-bit `RawTexel` and reuses the existing RGBA16/IA16 conversion.
Private fields and distinct `ResolvedIndexedTexel::{Direct,Tlut}` variants
prevent a disabled result from consuming an entry or an enabled result from
masquerading as a direct color.

M4.3.3b makes no physical TMEM address/read, validity, epoch, generation,
snapshot, first-row, odd-row, footprint, sampling, filtering, bilerp, LOD,
cache, WGSL/GPU/upload, production-dispatch, YUV, non-CI TLUT-mode, parity, or
performance claim. On its own it leaves snapshot binding and the quadricated
validity footprint to the physical reader below.

M4.3.3c supplies that bounded physical reader as
`read_committed_texel(&PhysicalTmemState, TileDescriptor,
AddressedTmemTexel, TextureLutMode)`. `AddressedTmemTexel` contains only an
already-normalized integer column/row and explicit caller-supplied first-row
parity; the reader never infers parity or performs shift, mask, mirror, clamp,
sampling, filtering, bilerp, or LOD. It preflights the format/size/mode through
M4.3.3a/b's pure decoders before reading any byte, then applies the tile's
64-bit line stride, 12-bit linear wrapping, and first-row-parity XOR row-parity
XOR4 exchange. Four-bit texels use high-nibble-first packed bytes, eight-bit
texels use one byte each, 16-bit values combine big-endian, and RGBA32 requires
a low-half tile base and reads RG from the low 2 KiB bank plus BA from the
corresponding high-bank address.

Every physical byte in a selected texel must be valid. Valid bytes may have
different touch generations or predate the durable state's current generation:
the result is instead bound to one `PhysicalTmemSnapshotIdentity` containing
the state allocation's uncaller-chosen identity and the generation captured
once for the immutable read. Enabled CI4/CI8 sources must resolve inside
canonical low-half TMEM. Their lookup always uses the absolute M4.3.3b address
`0x800 + index * 8`, never a tile-relative or rebased palette location, and
accepts an entry only when all eight quadricated bytes are valid and all four
big-endian 16-bit lanes are equal. The Programming Manual section 13.8
partial-CI8 example places indices 40..=69 at absolute TMEM words 296..=325
(`256 + index`); the committed fixture uses that placement and proves an
unloaded lower index is not silently rebased. The all-eight-valid/equal rule is
an admitted conservative canonical subset, not a hardware claim about partial
or unequal words: both remain distinct typed errors here while their actual
sample-lane behavior awaits hardware measurement. Disabled CI remains
M4.3.3b's I8 alias and CI8 still ignores the tile palette.

This is a CPU-only read/decode mechanism over already-published durable state.
It does not add YUV, CI16/CI32, cache identity/publication, WGSL, GPU upload,
sampling, production dispatch, visual parity, or performance evidence.

M4.3.3d adds `address_point_texel` and `sample_committed_point` as a typed,
allocation-free CPU point path over that reader. Its input coordinates are
already-quantized signed S10.5 values: each axis applies the tile's public
shift encoding, subtracts the exact S10.2 low endpoint in five-fraction-bit
integer space, selects the containing texel with Euclidean division, and
applies clamp before mirror/mask. Mask zero implies clamp. A required reversed
clamp extent is a typed error; a nonzero-mask axis with clamp clear does not
consult that unused extent. The sampler then delegates all physical address,
validity, format, CI, and TLUT behavior to `read_committed_texel`, preserving
its state/generation snapshot identity and errors.

`PointSampleRequest` still requires explicit `TmemFirstRowParity`. The
reference lane derives parity from the render tile's ULT while pinned RT64's
raw-TMEM shader uses the relative row, and neither settles load/render-tile
aliasing on hardware; this layer therefore derives neither and has no default.
Partial or unequal TLUT banks remain the reader's named errors. This point-only
slice does not convert float or perspective coordinates, decode filter state,
select filtered neighbours or lanes, implement copy-cycle clamp bypass, add
LOD/YUV/cache/WGSL/GPU work, connect a raster path, or claim visual/silicon
parity or performance.

M4.3.3e reuses that exact integer path to expose the 2x2 cell containing the
point. `address_texture_cell` returns the post-shift/post-origin five-bit S/T
fractions and independently clamp/mirror/mask-addresses the semantic
upper-left, lower-left, upper-right, and lower-right corners.
`gather_committed_texture_cell` then reads those four corners in that fixed
order through `read_committed_texel`, preserving explicit first-row parity,
per-corner CI/TLUT lookup and errors, and one equal committed snapshot
identity. Clamp or mirror may make semantic corners address the same texel;
they remain four named entries rather than being deduplicated. This cell shape
follows the public Programming Manual sections “TF: Texture Filter” and
“Sampling Overview”; Chapter 13.7 “Texture Level of Detail” supplies the
five-fraction-bit coordinate grid.

The all-four gather is a diagnostic and average-filter candidate, not a
bilerp result. It does not select the three nearest corners, require the exact
three-corner validity footprint, average or interpolate colors, decode filter
state, settle diagonal/tie/output rounding or accumulator width, convert
float/perspective coordinates, infer first-row parity, relax unequal TLUT
lanes, or implement copy addressing, LOD, YUV, cache, WGSL, or GPU work. It
adds no production-DPC integration; primitive, rectangle, or triangle decode;
combiner, coverage, depth, blend, target, or VI behavior; derivatives,
detail/sharpen, or two-cycle selection; full-ROM qualification; or RT64 pixel
parity. It claims neither visual/silicon parity nor performance. A later
three-nearest path must choose its corners before reading them so an unused
fourth corner cannot create a false validity failure.

M4.3.3f ports the RDP's "three nearest" triangular bilerp as a pure function
over `CommittedTextureCell`: `filter_three_nearest_committed_cell` remaps the
cell's stored `[UpperLeft, LowerLeft, UpperRight, LowerRight]` corner order to
`fn64-render-reference`'s `filter_three_nearest_s10_5` formula order
(`[c00, c10, c01, c11]` = `[UpperLeft, UpperRight, LowerLeft, LowerRight]`)
before applying the same fixed-point arithmetic, selecting the lower-left or
upper-right triangle by `sf + tf <= 32` and rounding-to-nearest with a
clamp-to-`u8` output policy carried unchanged from the reference lane. Public
documentation does not establish the silicon filter accumulator width or its
tie-break rule; this is a preserved convention, not a verified hardware fact.
The formula and its Programming Manual "TF: Texture Filter"/"Sampling
Overview" citation are ported from the already-tested
`crates/fn64-render-reference/src/gbi/types.rs:954-972`; a same-repo
Rust-to-Rust differential drives the pure arithmetic against the reference
lane's own literal 262,144-case sweep (all `sf, tf` in `0..32` across 256
pseudo-random four-corner seeds), plus a TMEM-address-grounded fixture at the
`sf + tf == 32` boundary reading real committed RGBA16 bytes. This slice does
not select which filter mode applies (point vs. bilerp vs. box-average vs.
copy), wire the filtered texel into the crate's pure one-cycle color
combiner (see "Color combiner: one-cycle selector arithmetic" below —
that seam is not connected to this decoder's texel output), drive
per-pixel UV/gather from a triangle rasterizer, or claim RT64
pixel/visual/silicon parity or performance.

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

M2.5.3a retains a separate repository-owned compute shader for the seven
direct texel conversions already implemented by `tmem::decode_direct_texel`:
RGBA16, RGBA32, IA4, IA8, IA16, I4, and I8. Its closed manifest exposes no raw
feature or limit selection, requests no optional wgpu feature, and labels the
21 denominator rows whose source closure contains the shared format helper as
candidate consumers only. The manifest state is explicitly `NotQualified` and
`NativeUnverified`. Naga validation plus the deterministic 131,710-case CPU
oracle fixture establish candidate mechanics; they qualify no component and
promote zero complete shader rows. The available host had no native adapter,
so no native receipt exists. A future opt-in host run requires a named native
adapter, exact frozen source/entry/fixture/input/expected identities, the exact
typed device contract, checked module/pipeline creation, exact submission
completion, callback observation, bounded readback, and exact output bytes.
Qualification requires 10 consecutive clean native runs of one frozen source;
a missing adapter, one run, or CPU/Naga evidence cannot satisfy that gate. See
[`../../docs/RT64-RUNTIME-SHADER-CORPUS.md`](../../docs/RT64-RUNTIME-SHADER-CORPUS.md).

M2.5.3b adds a second repository-owned compute shader, `three_nearest_filter`,
implementing the RDP three-nearest triangular interpolation formula over four
caller-supplied RGBA8888 corners and 5-bit S/T fractions. Its oracle is
`fn64-render-reference::filter_three_nearest_s10_5` directly; because that
function is `pub(super)` and its visibility is out of this component's scope,
the deterministic 262,144-case fixture duplicates the reference sweep's exact
seed/formula logic (`fn64-render-reference/src/gbi/tests/group4.rs`) rather
than calling it cross-crate. The struct fields use the reference formula's own
corner names (`c00`/`c10`/`c01`/`c11` = upper-left/upper-right/lower-left/
lower-right), not `CommittedTextureCell`'s `[UL, LL, UR, LR]` order; a future
caller wiring gathered corners into this component must remap by name. The
per-channel accumulator is `i32` rather than the reference's `i64`; a range
proof (checked exhaustively by a dedicated test) shows the accumulated value
is always non-negative for valid byte-range corners and in-range fractions, so
WGSL's toward-zero integer division never diverges from the reference's. The
manifest state is `NotQualified` and `NativeUnverified`, matching M2.5.3a's own
zero-row-promotion precedent. This component does not wire itself to
`CommittedTextureCell`/`gather_committed_texture_cell`, select filter mode, or
perform TMEM read/decode/addressing; it claims no RT64 visual/pixel parity and
no hardware-accuracy verdict on the accumulator width. No RT64 upstream source
exists for this formula — its sole provenance is this repository's own
reference lane and its citation to the public Nintendo 64 Programming Manual,
"TF: Texture Filter" and "Sampling Overview".

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

## Blender: full selector/cycle semantics (port-card §1)

`blend` is a characterization-first, selective literal port of
`fn64-render-reference`'s `blend_fragment`/`blend_color`/`blend_a`/`blend_b`
(`crates/fn64-render-reference/src/raster/blend.rs:157-292`), per
`/private/tmp/rt64-blender-depth-port-card.md` §1 ("Blender"). It covers the
full RDP blender: `P`/`M`/`A`/`B` selector semantics for both cycles
(`BlendColorInput`/`BlendAlphaInput`/`BlendBInput`, resolved from the
already-landed `state::BlenderCycle` raw 2-bit wire fields rather than a
duplicate decode), sequential cycle handoff (`Combined` reads the raw source
on cycle 0 and the running composite on cycle 1), the no-`FORCE_BL`
last-cycle bypass, the zero-factor (`a==0`/`b==0`) divisor collapse, and the
`Framebuffer`/`FramebufferAlpha` selectors' hard dependency on a caller-supplied
memory sample.

Like `depth_strict_less` and `alpha_compare`, `fn64-render-wgpu` has no crate
dependency on `fn64-render-reference`, so this is a self-contained literal
re-expression citing the reference's line numbers. `blend_fragment` reuses
`state::OtherMode`'s `cycle_type`/`force_blend`/`blender_cycle_1`/
`blender_cycle_2` accessors directly rather than re-decoding those bitfields
or introducing a second `CycleType`/`BlenderCycle` type.

**Loud rejection, not fallback.** `blend_fragment` returns
`Result<BlendedFragment, BlendImageReadError>`. Any fragment that reaches a
`Framebuffer`-color or `FramebufferAlpha` selector without a supplied
`BlendFramebufferSample` errors with the exact selector name
(`"framebuffer color"` / `"framebuffer coverage alpha"`), matching the
reference's `read_framebuffer_memory` panic. This includes a load-bearing,
non-obvious fact this port's exhaustive test suite caught and pins explicitly
(`p_selects_framebuffer_still_requires_memory_to_resolve_ps_own_discarded_value`):
`blend_color` is evaluated for **both** `P` and `M` before the
`Framebuffer`-dispatch branch runs, exactly mirroring the reference's own
unconditional `blend.rs:191-192` evaluation order — so a `Framebuffer`
selector on either input requires a memory sample even when that particular
input's resolved value is then discarded by the taken branch. This is not a
simplification introduced by the port; it is a literal, faithful
reproduction of the reference's evaluation order, verified against it.

**Dual-source WGSL/Rust seam.** The port card poses an open design choice
between reproducing RT64's actual fixed-function dual-source blend mechanism
and computing the full software composite in-shader. This module reuses the
exact contract the already-accepted M2.2 Metal-execution evidence
(`docs/RT64-PORT-DASHBOARD.md` `M2.2`, `probes/m2-wgpu-metal-headless`) proved
executable on real wgpu/Metal, rather than inventing a third model:
`DualSourceBlendOutput` carries the `source`/`source1` pair a real
`@blend_src(0)`/`@blend_src(1)` fragment shader would emit for
`wgpu::BlendFactor::Src1`/`OneMinusSrc1` (and `Src1Alpha`/`OneMinusSrc1Alpha`)
fixed-function blend state, and `manual_blend_composite` performs the exact
integer fallback arithmetic `(src*factor + dst*(255-factor) + 127) / 255`
that M2.2's `execute_manual_blend` proved for adapters without
`wgpu::Features::DUAL_SOURCE_BLENDING`. Neither function reads or writes an
actual render target, submits a GPU draw call, or claims production
integration — this is the pre-blend output contract and downstream
pipeline-state seam, not a claim of current render-target read or draw path.

The retained `shaders/blend.wgsl` is a Naga-validated compute-shader oracle
over the general A/B divide arithmetic (including the exact zero-factor
collapse branches), not a compiled render pipeline; it is not wired into any
draw path, bind group layout, or pipeline used elsewhere in this crate,
matching `alpha_compare.wgsl`/`depth_strict_less.wgsl`'s precedent.

**Characterization.** The one-cycle selector space (`P`×`M`×`A`×`B` = 256
combinations) is exhaustively enumerated against an independently-derived
Rust oracle, crossed with both `force_blend` and `image_read_enabled` (1024
total cases). Two-cycle mode is covered by curated sequential-handoff cases
(including the reference's own documented "fog then pass" pattern) rather
than the full 256² space, per the port card's sampling guidance. Boundary
alpha/divisor-collapse values, the no-`FORCE_BL` bypass, `IM_RD` legality
gating, and the dual-source/manual-fallback contract each have dedicated
tests.

**Nonclaims.** No framebuffer resource binding/readback, raster
primitive/triangle execution, target storage, coverage/depth ordering
integration, presentation, native adapter qualification, full-ROM/pixel
parity, or performance claim. Combiner evaluation, coverage accumulation
(the `blend_enabled` derivation is caller-supplied, matching the reference's
own `blend_fragment` signature), alpha compare, depth test, and dither are
upstream/sibling concerns this module does not implement.

## Color combiner: one-cycle/two-cycle selector arithmetic

`combiner` is a characterization-first port of RT64's color combiner,
sourced from the MIT `src/shared/rt64_color_combiner.h`'s selector decode
tables (`colorInputA/B/C/D`, `alphaInputABD/C`, `decodeColorInput`,
`decodeAlphaInput`), `Inputs` struct, `fromColorInput`/`fromAlphaInput`, and
`wrap`/`wrapInputC`/`wrapInputABD`/`wrapClamp`/`runCycle`/`run` — the pinned
commit `5473732a822a4423b5696e7cb18fecc425a59875` recorded as this crate's
Rust-port source authority in `docs/RT64-PORT-AUTHORITY.md`. It provides
typed `ColorInput`/`AlphaInput`/`CombineParams` selector decode — exact and
complete for every wire-legal `(slot, index, second_cycle)` triple, matching
RT64's `decodeColorInput`/`decodeAlphaInput` bit-for-bit — plus full
one-cycle and two-cycle `(A-B)*C+D` arithmetic (`run_one_cycle`,
`run_combiner`/`run_two_cycle` taking a typed `CombinerCycleMode` rather than
a raw boolean) that evaluates every selector either enum can hold:
`COMBINED`, `TEXEL0`, `TEXEL1`, `PRIMITIVE`, `SHADE`, `ENVIRONMENT`,
`KEY_CENTER`, `KEY_SCALE`, `COMBINED_ALPHA` and the other `*_ALPHA`
cross-reads, `LOD_FRACTION`, `PRIM_LOD_FRAC`, `NOISE`, `K4`, `K5`, `ONE`, and
`ZERO`. An independently-derived Rust oracle (`combiner.rs`) is matched by an
owned, Naga-validated WGSL transcription (`shaders/color_combiner.wgsl`,
`COLOR_COMBINER_WGSL`) that implements identical arithmetic — neither is
compiled into any pipeline or wired to a draw path.

Two-cycle mode reproduces `runCycle`/`run`'s exact wiring: cycle 0 runs
before cycle 1, threaded through one shared accumulator (RT64's single
`inout float4 combinerColor`, never two independent evaluations); cycle 1's
`COMBINED`/`COMBINED_ALPHA` selectors read cycle 0's real output, not the
zero-init accumulator one-cycle mode always sees; `TEXEL0`/`TEXEL1` swap on
cycle 1 specifically (`fromColorInput`/`fromAlphaInput`'s own `secondCycle`
parameter, distinct from the bitfield-slice selector of the same name); the
cross-cycle carry is wrapped by `wrapInputC`
(`[-1-1/255,1+1/255]`)/`wrapInputABD` (`[-0.5-1/255,1.5+1/255]`) *before*
any arithmetic reads it, with the range chosen independently for color and
alpha by whether that channel's own slot-C selector is
`COMBINED`/`COMBINED_ALPHA` this cycle, not by which slot is being resolved;
and `alphaCompareValue` is captured immediately after cycle 0, never
overwritten by cycle 1.

`NOISE`, `LOD_FRACTION`, and `PRIM_LOD_FRAC` are caller-supplied typed
fields on `CombinerInputs`, not generated here: RT64's own PRNG
(`initRand`/`nextRand`, `src/shaders/Random.hlsli`) and per-pixel derivative
computation (`computeLOD`) are not part of the pinned
`rt64_color_combiner.h`/`RasterPS.hlsl` combiner-arithmetic surface this
module ports, so this module proves only that the formula correctly
consumes whatever value it is given, mutating each field
independently to confirm it actually participates rather than merely
type-checking. The final `wrapClamp` (`wrapInputABD` then `clamp(0,1)`)
applies unconditionally to every output channel regardless of cycle count;
extreme/out-of-range caller-supplied scalars are exercised against the real
wrap step boundary, not only the plain-clamp reduction that in-range
texel/prim/shade/env inputs always hit.

**Nonclaims.** No copy mode, no real NOISE/LOD generation, no shader-keying
or pipeline-variant selection, no texture/rasterizer integration (the crate's
texel-fetch and three-nearest-filter machinery — see "M4.3.3f" above — is
not wired to this module), no draw-path or production-DPC integration, no
target/framebuffer write, and no RT64 pixel/visual/silicon parity or
performance claim. `SetCombine`'s raw-DPC decode and durable `RdpState`
retention are covered separately below ("SetCombine: raw-DPC decode and
durable state retention"); this module still performs no decode of its own
and constructs `CombineParams` only in its own tests.

## Depth: strict-less compare/update (port-card slice 1)

`depth_strict_less` is the smallest fragment-pipeline slice from the
characterization-first RT64 blender/coverage/alpha-compare/depth/render-target
port card: a literal, self-contained re-expression of
`fn64-render-reference`'s `Framebuffer::set_depth_tested`
(`crates/fn64-render-reference/src/raster/draw.rs:632-653`) as a typed CPU
oracle (`StrictLessDepthSample`/`StrictLessDepthOutcome`/`StrictLessDepthWrite`,
`strict_less_depth_test`, `strict_less_depth_write`) plus a matching WGSL
compute-shader seam (`STRICT_LESS_DEPTH_WGSL`, entry point
`STRICT_LESS_DEPTH_ENTRY_POINT`). `fn64-render-wgpu` has no crate dependency on
`fn64-render-reference`, so the port is a literal re-expression with a citation
comment, matching this crate's existing convention (see `tmem/sample.rs`).

The comparison is exactly `fragment_z < memory_z`: strictly nearer fragments
pass and commit both color and depth; anything else (farther, or exactly
equal) rejects and mutates neither target. There is no `DepthMode` dispatch
(`mode_passes`'s four-way Opaque/Interpenetrating/Translucent/Decal split is
out of scope), no DeltaZ/coverage-wrap tightening
(`depth::relations`/`depth_coverage_decision`), no encoded 18-bit
exponent/mantissa Z-buffer packing (`depth.rs`'s `EncodedDepth`/`encode_z`/
`decode_z`), and no blend, coverage, or alpha-compare gating — this is
`set_depth_tested`'s own deliberately simplified always-write-on-pass
contract, not `set_depth_controlled_blended`'s full conjunction. Both `f32`
inputs use plain IEEE-754 `<` semantics, including that any `NaN` operand
always rejects and that `set_depth_tested` performs no range clamp (unlike
the full pipeline's `.clamp(0.0, 0x3ffff as f32)` before comparison) — a
deliberate scope boundary carried into this slice's tests, not an oversight.

The WGSL seam mirrors this arithmetic in a closed compute shader over a
`(fragment_z, memory_z, fragment_rgba, memory_rgba)` storage-buffer pair, not
wired into any draw path, bind group layout, or pipeline used elsewhere in
this crate. `naga` validates the retained source under an empty capability
set; a differential test cross-checks the Rust oracle's decision against the
shader source's own frozen comparison text across a representative value
grid, and a hostile-mutation test confirms a flipped `<=` direction still
parses/validates (naga cannot catch a semantic direction flip) while a
separate structural assertion pins the exact `<` text so such a flip fails
loudly at the Rust level instead.

Provenance correction: `set_depth_tested` itself cites `F3DEX2-CONCEPTS.md`
§4.3 "Z-buffer compare", not RT64 — that section is sourced entirely from the
public N64 Programming Manual (Chapters 15-16) and libultra's
`gDPSetPrimDepth`, with no RT64 attribution. RT64's `Depth.hlsli` (pin
`5473732a822a4423b5696e7cb18fecc425a59875`) is the *encoded* 18-bit
exponent/mantissa depth codec this slice explicitly does not port;
`docs/rt64-port-inventory.json` records its `port_state` as `not-started`,
targeting a different, not-yet-created file. This slice cites, reads, and
claims no RT64 source byte. No blend, coverage, alpha
compare, dither, `Interpenetrating`-mode coverage-wrap adjustment (an
unresolved gap in the reference itself), framebuffer read, draw-call
integration, or native GPU execution is claimed or exercised by this slice.

## Coverage: pure `cvg_dst` semantics (port-card slice 5)

`coverage` is the fifth characterization-first slice from the same RT64
blender/coverage/alpha-compare/depth/render-target port card the depth slice
above draws from (a read-only planning artifact, §2 "Coverage" — not a
file committed to this repository): a literal, self-contained re-expression
of `fn64-render-reference`'s coverage
model (`crates/fn64-render-reference/src/raster/{mod,coverage}.rs`) as plain
value-in/value-out Rust functions plus a matching WGSL compute-shader seam.
As with the depth slice, `fn64-render-wgpu` has no crate dependency on
`fn64-render-reference`; every arithmetic fact here is inherited from that
crate's existing citations, ported as literal Rust, not rederived, and no
RT64 source byte is read or cited directly by this module.

The module carries: `Coverage`, an invariant-carrying newtype for the RDP's
0..=8 subpixel population count (`Coverage::new` panics rather than clamps
above eight, matching the reference's loud-trap convention), its RDRAM
3-bit `stored`/`from_stored` round-trip (`count - 1`, only the low three
bits consulted on decode), and its two blender-facing encodings —
`alpha()` (`(count*255 + 4) / 8`, an explicitly documented open frontier for
the RDP's unpublished internal encoding, not a hardware fact) and
`times_alpha()` (`(count*alpha + 127) / 255`, likewise an open rounding
frontier). `coverage_result` is the `cvg_dst` accumulation itself: given a
`pixel`/`memory` coverage pair and four independently-decoded `OtherMode`
bits (`image_read_enabled`, `force_blend`, `antialias_enabled`, and the
`CoverageDestination` selector reused from `crate::state` rather than
duplicated — see below), it derives `sum`, the `wraps` boundary
(`image_read_enabled && sum > 8`), the three-input `blend_enabled` truth
table (`force_blend || (antialias_enabled && !wraps)`), and the
`destination` coverage under each of the four `CoverageDestination` modes:
**Clamp** (`min(sum, 8)` when image-read and blend are both live, else the
raw pixel count), **Wrap** (`sum - 8` once wrapped, else `sum`, only under
image-read), **Full** (always `Coverage::FULL`), and **Save** (memory
passed through unchanged, no accumulation at all). `apply_coverage_alpha`
is the separate coverage-to-alpha interaction: `coverage_times_alpha`
multiplies coverage by the fragment's current alpha channel first;
`alpha_coverage_select` then independently overwrites that alpha channel
with the (possibly multiplied) coverage's `alpha()` encoding — the two bits
compose, and either, both, or neither may be set. `CoverageMask` carries the
eight public subpixel sample positions (`COVERAGE_SAMPLES`, in eighth-pixel
units) as an 8-bit population bitmask, and `attribute_sample` selects one
on-primitive attribute-correction point from a partial mask via the
reference's `NearestToPixelCenterStableOrder` policy — explicitly an fn64
policy choice for an unpublished silicon lookup, not a discovered hardware
centroid rule, carried into this module verbatim rather than re-derived.

`state::CoverageDestination` (added by the OtherMode bitfield-decode slice
that landed as this module's dependency) is imported and reused verbatim
here rather than duplicated — `coverage.rs` does not define its own
`CoverageDestination` enum or a `from_wire` decoder; that decode, and its
own exhaustive four-encoding test, live solely in `state.rs`'s
`OtherMode::coverage_destination`. This module still does not read
`OtherMode` itself: callers extract the four plain mode-bit values from
their own `OtherMode` and pass them in via `CoverageModeBits`, preserving
the pure value-in/value-out seam every other slice in this file follows.

The WGSL seam (`COVERAGE_WGSL`, entry point `COVERAGE_ENTRY_POINT`) mirrors
`coverage_result` and `apply_coverage_alpha`'s arithmetic in one closed
compute shader over a `CoverageInput`/`CoverageOutput` storage-buffer pair,
not wired into any draw path, bind group layout, or pipeline used elsewhere
in this crate. `naga` validates the retained source under an empty
capability set. Every branch — the four `CoverageDestination` modes, the
`wraps`/`blend_enabled` derivation, and the alpha-composition block
(`coverage_times_alpha`/`alpha_coverage_select`'s sequencing, where
`alpha_coverage_select` must read the *already* times-alpha-adjusted
coverage, not the raw destination) — has both a textual/structural
guard (pinning the exact source line so a semantic mutation that still
parses/validates under naga fails at the Rust level instead) and a
differential oracle interpreting that frozen WGSL text in Rust against the
Rust implementation across the exhaustive fixture matrices.

Explicitly out of scope, matching the port card's own step ordering
(coverage is step 5 of 10, framebuffer reads are step 8): the
framebuffer-read mechanism that supplies `memory: Coverage` in the first
place (no storage-texture read-write binding, no "read old value" pass, no
GPU-side old-coverage sampling of any kind), any draw-path wiring, RHI,
bind groups, or global/mutable state. `CoverageMask::from_samples` (the
reference's rasterizer-integration closure that samples real triangle/line
geometry) is likewise out of scope; this module's `CoverageMask` is
constructed only from a raw 8-bit mask (`CoverageMask::from_bits`), since
this slice owns no rasterizer to sample against. This slice cites, reads,
and claims no RT64 source byte, no framebuffer-read GPU mechanism, no
native/GPU-verified execution, and no parity of any kind.

## 2026-08-16: Coverage, fragment-callable WGSL seam

Per `/private/tmp/fn64-post-texture-next-production-card.md` (a read-only
planning artifact, not a file committed to this repository): re-expresses
the already-characterized, already-tested RDP `cvg_dst` accumulation
arithmetic (`coverage_result`/`apply_coverage_alpha`, "Coverage: pure
`cvg_dst` semantics (port-card slice 5)" above) as a plain, fragment-callable
WGSL function, alongside the existing standalone `@compute` seam it already
had. This card is a sibling of the alpha-compare fragment-callable slice
immediately above it in this program's own step ordering (alpha-compare is
step 4, coverage is step 5 of the same blender/coverage/alpha-compare/depth
port-card ladder) — same pattern, disjoint fixture family, no dependency
either direction.

`coverage.wgsl`'s existing `evaluate` function takes a single
`CoverageInput` struct read from a storage buffer and returns a
`CoverageOutput` struct written to a second storage buffer, matching that
file's `@compute @workgroup_size(64)` dispatch shape. The new sibling file,
`coverage_fragment_fn.wgsl`, re-expresses the identical arithmetic verbatim
as `coverage_fragment_fn` — an ordinary function taking nine scalar `u32`
arguments (`pixel_count`, `memory_count`, `image_read_enabled`,
`force_blend`, `antialias_enabled`, `coverage_destination`,
`coverage_times_alpha`, `alpha_coverage_select`, `fragment_alpha`) and
returning a plain `CoverageFragmentResult` struct, with no `@compute`,
`@group`/`@binding`, or entry point of its own. This mirrors
`coverage_result`/`apply_coverage_alpha`'s own Rust-side signature exactly
(`pixel`/`memory`/mode bits in, a result struct out), rather than inventing
a new call shape, and is callable from a future `@fragment` entry point via
WGSL's ordinary function-concatenation build seam — the same mechanism
`shaders/triangle_pipeline_fragment.wgsl`'s own header already documents for
`color_combiner.wgsl`. `coverage.rs` exposes it as a new constant,
`COVERAGE_FRAGMENT_FN_WGSL` (`include_str!`), re-exported from this crate's
root alongside the existing `COVERAGE_WGSL`/`COVERAGE_ENTRY_POINT`
constants. The existing `coverage.wgsl` `@compute` entry point, and every
existing test in `coverage.rs`/`coverage/tests.rs`, are unmodified.

The new function ports the **complete** four-way `cvg_dst` match
(`Clamp`/`Wrap`/`Full`/`Save`) — there is no honest partial port of
`evaluate`, since the reference's own `coverage_result` is one function body
with one `match`. `Clamp`/`Wrap` are transcribed but, per the boundary
below, not independently GPU-differentially validated by this slice.

**Tests.** Naga parse/validation and textual structural guards (the exact
`Full`/`Save` destination-assignment literals, the exact
`coverage_alpha`/`coverage_times_alpha_value` rounding-formula expressions)
mirror the sibling file's own existing convention. The load-bearing addition
is a CPU-vs-WGSL differential over a frozen fixture partition matching the
implementation card's own §4: `cvg_dst=Full` crossed with `pixel.count()` ∈
{0,1,4,8}, `memory.count()` ∈ {0,8} (proving `memory` is ignored under
`Full`), and every `image_read_enabled`/`force_blend`/`antialias_enabled`
combination (`destination=Coverage::FULL` unconditionally, `wraps`/
`blend_enabled` still asserted per `coverage_result`'s unconditional first
three statements); `cvg_dst=Save` crossed with `pixel.count()` ∈ {0,4,8},
`memory.count()` ∈ {0,3,8} (literal hand-supplied stand-ins for a future
framebuffer read, not claimed to come from one) and the same mode-bit
combinations (`destination=memory` unconditionally); and the four
`coverage_times_alpha`/`alpha_coverage_select` composition cases applied on
top of a `Full`-mode `destination=8`, including the sequencing-discriminator
case (`coverage_times_alpha=true, alpha_coverage_select=true` ->
`adjusted_coverage=6, adjusted_alpha=191` — a wrong implementation reading
the raw `destination` instead of the already-times-alpha-adjusted coverage
here would produce `alpha=255`, not `191`). Every fixture case is computed
three independent ways and asserted equal: the frozen, hand-derived expected
values; the Rust oracle (`coverage_result`/`apply_coverage_alpha`,
`#[cfg(test)]`, no GPU); and the new WGSL function itself, executed on a
real native adapter through a minimal `#[cfg(feature = "host-gpu-tests")]`
compute-shim harness — a throwaway `@compute` entry point that calls
`coverage_fragment_fn` directly per fixture case and reads its result struct
back via a storage buffer. That harness is new test-only scaffolding; it
does not claim the function runs inside any real fragment shader (see
Nonclaims below). The harness ran 10 consecutive times from a clean process
each time on this crate's certified M2.2 Metal adapter, all passing, with no
`NoAdapter` fallback observed.

**`Clamp`/`Wrap` GPU-validation boundary, stated explicitly.** The new
function's `Clamp`/`Wrap` branches are transcribed (the function is a
complete port of `evaluate`, not a partial one) but **not** independently
validated against a real GPU-sourced `memory: Coverage` value by this
slice's test suite — both modes need a real prior-frame/prior-draw coverage
read, which is exactly the framebuffer-read problem this slice exists to
route around (see Nonclaims below). A future slice resolving that problem
should extend this fixture to cover `Clamp`/`Wrap` with a real GPU-sourced
`memory` value at that time; until then, treat `Clamp`/`Wrap` as
"implemented but GPU-unvalidated," not "missing."

**Nonclaims.** This slice does not touch `targets/triangle_pipeline.rs`,
`shaders/triangle_pipeline_{fragment,vertex}.wgsl`, or `production.rs` — no
triangle drawn through the real production pipeline has its coverage
computed or written by this slice, and no bind-group/uniform layout for the
mode bits or the fragment's own rasterized `pixel` coverage count is decided
here. Three other lanes claim (or reserve) those files concurrently as of
this slice's freeze (varying-UV committed-TMEM texture sampling into the
production triangle draw path, `TextureRectangle`/`TextureRectangleFlip`
decode admission, and the alpha-compare fragment-callable seam immediately
above); this slice is independent of all three — it touches none of their
claimed paths and requires nothing any of them produce. The deferred
wire-in step (adding a `CoverageModeBits`-shaped bind-group entry or uniform
field to `triangle_pipeline_fragment.wgsl`, calling this function from
`fs_main`, deciding how the fragment's own rasterized coverage count is
sourced, and — the hard part — deciding the coverage-buffer write mechanism,
since a standard `discard`-based fragment shader cannot express "write
coverage but suppress color") becomes safe to dispatch once the contested
lanes' own diffs are known, so this new function can be composed into them
without reopening any lane's in-flight work — not attempted in this slice.
Also out of scope, matching the port card: the `memory: Coverage` source
for any mode (the framebuffer-read problem itself, named and deferred, not
solved, here), `CoverageMask`/`attribute_sample` (the 8-sample checkerboard
mask and its partial-coverage attribute-point policy — a separate,
rasterizer-level, still-unwired concern), `depth_mode`/`blend`/
`alpha_compare`'s equally-unwired or separately-owned characterized seams,
RT64 pixel/visual parity, and any performance or throughput claim.

## T3 Phase A/B: the production raw-DPC `WgpuBackend`

Separately from the M-numbered lineage above (a different migration track:
`docs/DESIGN.md`'s "Production raw-DPC seam" sections), T3 Phase A adds
`PendingTmemTransaction::into_physical_successor`, and T3 Phase B adds the
`production` module's concrete `WgpuBackend`, which owns
`fn64_render::RawDpcCoordinator<PhysicalTmemState>` and implements
`RenderBackend`'s `plan_raw_dpc`/`execute_raw_dpc`/`publish_raw_dpc` raw-DPC
production trio. `plan_raw_dpc` drives T1's real decoder/push loop
(`raw_dpc::production_adapter`); `execute_raw_dpc` reaches plan contents
exclusively through `BoundSubmittedRawDpc::execution_view` (never a bare
`SubmittedTicket`) and stages every load through a new, additive
`PhysicalTmemState::stage_neutral_transfer` counterpart to the existing
decoder-typed `stage_transfer` -- required because a production caller
cannot obtain the private `SubmittedTicket`/`BoundTmemTransfer` pair that
method needs, by the production seam's own design; `publish_raw_dpc` is
exactly `self.coordinator.prepare_publication(publication).commit()`. Scope
is TMEM-only, no-FullSync, no-guest-write, headless: no ABI/T4 ingress, no
visible presentation, no raster parity, no native GPU testing.
`WgpuBackend::process_task`/`present` are honest named rejections. See
`docs/DESIGN.md`'s "T3 Phase A/B" section for the full account.

## Depth: full four-mode compare/update (`depth_mode`)

`depth_mode` extends `depth_strict_less`'s smallest-slice foundation to the
full RDP depth-mode dispatch the port card's §4 "Depth compare/update"
describes: all four `ZMODE_*` variants (`state::DepthMode::{Opaque,
Interpenetrating, Translucent, Decal}`, reused directly from this crate's
existing `OtherMode` decode layer, not redefined) and their exact
comparison/update semantics, plus the coverage-wrap tightening
`depth_coverage_decision` layers on top of the plain per-mode test. It is a
literal, self-contained re-expression of `fn64-render-reference`'s
`depth::{relations, mode_passes}` (`crates/fn64-render-reference/src/depth.rs`)
and `raster::coverage::depth_coverage_decision`
(`crates/fn64-render-reference/src/raster/coverage.rs:39-59`), following
`depth_strict_less`'s established convention: a typed CPU oracle
(`DepthRelations`, `DepthModeDecision`, `relations`, `mode_passes`,
`depth_mode_decision`) plus a matching WGSL compute-shader seam
(`DEPTH_MODE_WGSL`, entry point `DEPTH_MODE_ENTRY_POINT`), citation comments
in place of a cross-crate dependency (`fn64-render-wgpu` still has no
dependency on `fn64-render-reference`).

**Behavior.** `relations()` computes the four Programming Manual Chapter 15
Equations 5-9 signals (`memory_is_max`, `farther`, `nearer`, `in_front`) from
a fragment's already-decoded Z/DeltaZ and a memory sample's Z plus *stored*
four-bit DeltaZ exponent — the reference's own asymmetric signature is
preserved exactly: `pixel_delta_z` is a direct `u16` value, while
`memory_encoded_delta_z` is the packed exponent decoded internally via
`decode_delta_z` before the two deltas are compared, because the memory side
is only ever available in its packed `EncodedDepth` form. `delta_z_max` is
the larger of the two decoded deltas; `farther`/`nearer` use `u32`
`saturating_add`/`saturating_sub` unconditionally (not just within the RDP's
documented 18-bit convention — `pixel_z`/`memory_z` are plain `u32`, and the
WGSL companion emulates true saturation, not a range-restricted
approximation, so it cannot silently wrap on out-of-convention input).
`mode_passes()` is Chapter 15 §15.7's exhaustive four-way dispatch — Opaque
and Interpenetrating both accept `relations.nearer` (delta-tolerant
correlation), Translucent requires the strict `relations.in_front`, and Decal
requires `relations.farther && relations.nearer && !relations.memory_is_max`
— with no default/fallthrough arm, so a hypothetical fifth mode fails to
compile rather than silently reusing an existing arm.

**Coverage-wrap tightening.** `depth_mode_decision()` layers
`depth_coverage_decision`'s wrap-aware override on top of `mode_passes()`:
Opaque combined with `coverage_wraps=true` tightens the delta-tolerant
`nearer` test to the strict `in_front` test; Interpenetrating combined with
`coverage_wraps=true` is the reference's own known, unresolved gap — the
Programming Manual's "Blender Modes and Assumptions" section requires a
coverage-adjustment path here but does not publish its arithmetic, so this
port preserves it as a first-class typed
`DepthModeDecision::UnsupportedInterpenetratingCoverageAdjustment` variant a
caller must handle explicitly, never a silent pass, reject, or decode-time
normalization (AGENTS.md "loud traps, no silent shrugs"). Translucent and
Decal receive no wrap-specific override in the reference, so this port
invents none for them either.

**Scope.** This slice ports the four-mode dispatch and its coverage-wrap
interaction only. Z encode/decode (`EncodedDepth`, `encode_z`/`decode_z`,
DeltaZ *encoding*, as opposed to the already-ported `decode_delta_z`
expansion this module needs for its memory-side argument) remains the
distinct, not-yet-started slice `depth_strict_less`'s README section already
names. Blend, coverage accumulation, alpha compare, dither, the
framebuffer-read problem, draw-call integration, and native GPU execution are
out of scope — this module only decides `Pass`/`Reject`/`Unsupported`, never
a byte write. It consumes `crate::state::DepthMode` as its mode parameter
without redefining or re-exporting it, and does not modify `state.rs`.

## Raw RDP triangle command decode

`raw_dpc::triangle` (re-exported as `RawTriangle`/`TriangleFlags`/
`RawWord`/`CoefficientWords`/`DepthWords`/`triangle_word_count`/
`TriangleDecodeError`) decodes all eight raw RDP triangle opcodes
(`0x08..=0x0f`), and `RawDpcCommandKind` gained a `RawTriangle(RawTriangle)`
variant so `decode_raw_dpc` admits them like every other command kind. Field
layout, block order, and the base(4)/shade(8)/texture(8)/depth(2) 64-bit
word counts come from the permitted MIT RT64 source (`src/hle/rt64_rdp.h`'s
`RDPTriangle` enum and `triangleBaseWords`/`triangleShadeWords`/
`triangleTexWords`/`triangleDepthWords`; `src/gbi/rt64_gbi_rdp.cpp`'s
`getTrianglePointers`/`decodeTriangles`) and the public SGI *RDP Command
Summary* triangle command sections; `fn64-render::raw_rdp_command_width`
already proved the exact eight stride values this decoder decodes against,
and a colocated test cross-checks the two tables agree for every opcode.

What is decoded: the low three opcode bits (Depth/Textured/Shaded, RT64's
`RDPTriangle` bit layout); tile index and mip level; the right-major (flip)
bit; signed YL/YM/YH; signed Q16.16 XL/XH/XM and their three edge slopes
(dXL/dY, dXH/dY, dXM/dY); every present raw shade, texture, and depth
coefficient word, retained as opaque `RawWord` pairs (no float conversion,
no per-channel/per-axis field splitting). A truncated command slice is
rejected before any optional block is read -- the length is checked once,
against the caller-supplied exact width, before any word is interpreted.

What is explicitly not yet done: no float conversion of any coefficient
(shade/texcoord/depth base+slope math stays RT64's job, ported separately
and named distinctly if it lands -- see the next section, which is exactly
that follow-on), no edge walk, no scanline/coverage generation, no
rasterization, no RDP state-machine transition (the base Edge command
carries no `RdpState` field any neighbor command already models), no
clipping/scissor, no texture sampling, no combiner/blender/depth/target
write, no GPU pipeline, no production-dispatch wiring (the T1
`production_adapter` seam rejects `RawTriangle` exactly like `NoOp`/
`SetOtherMode`/`FillRectangle`/`FullSync` -- loudly, as `UnadmittedRawDpcCommand`,
never silently dropped), and no RT64 parity or performance claim of any
kind.

## RGB dither and RGBA16 quantization (`rgb_dither`)

`rgb_dither` is a characterization-first literal port of the permitted MIT
RT64 Rust-port source pinned at commit
`5473732a822a4423b5696e7cb18fecc425a59875` (`docs/RT64-PORT-AUTHORITY.md`),
`src/shaders/Formats.hlsli`: `DitherPatternBayer`, `DitherPatternMagicSquare`,
`DitherPatternIndex`, `DitherPatternValue`, and the non-HDR integer tail of
`Float4ToRGBA16`. Like `depth_strict_less` and `alpha_compare`,
`fn64-render-wgpu` has no crate dependency on `fn64-render-reference`, so this
is a self-contained re-expression citing RT64's source directly, not a
re-derivation of the reference's own `apply_rgb_dither`/
`ordered_rgb_dither_threshold` (`crates/fn64-render-reference/src/raster/blend.rs:24-69`).
It is not wired into any draw path, `state.rs`, or the blender/depth/coverage/
alpha-compare modules.

**Selector and threshold lookup.** `dither_pattern_value(pattern, x, y,
noise)` selects a `DitherThreshold` (an invariant-carrying `0..=7` newtype,
private field, only constructible via its own checked `try_new` or this
module's own exhaustive-by-construction branches) under
`crate::state::RgbDither`'s four wire-decoded variants, reused directly
rather than redefined: `MagicSquare`/`Bayer` read a literal 4x4 ordered tile
at `dither_pattern_index(x, y)` (RT64's own `((coord.y & 3) << 2) + (coord.x &
3)`, row-major); `Noise` returns the caller-supplied `DitherNoiseByte`'s low
three bits; `Disabled` returns zero. RT64's `coord` is an unsigned `uint2`
with no negative representation; this port's `x`/`y` are `i32` and wrap
negative screen coordinates with `i32::rem_euclid` before indexing (matching
this crate's own `alpha_compare.rs`/`ordered_dither_threshold` and the
admitted reference's `x.rem_euclid(4)` precedent) rather than reproducing
HLSL's unsigned-cast reinterpretation, which is not itself RT64-observed
behavior since RT64's shader never receives a negative coordinate at the type
level — this is a documented, honestly-labeled extension beyond the ported
source, not an inherited RT64 fact. The WGSL companion mirrors this with an
explicit `euclid_rem4` helper, since WGSL's `%` truncates toward zero like C
(not Rust's `rem_euclid`) and would otherwise wrap negative coordinates
incorrectly.

**Matrix cross-check against the existing reference oracle (a characterized
frontier, not a resolved one).** This module independently transcribes RT64's
flat 16-element tables and compares them cell-by-cell against
`fn64-render-reference`'s existing `[[u8;4];4]` `MAGIC_SQUARE`/`BAYER`
tables (`blend.rs:29-30`, duplicated at `alpha_compare.rs:161-162`):
**MagicSquare is byte-identical** at every cell; **Bayer disagrees at rows 1
and 2** (RT64 row 1 `[4,0,5,1]` vs. the reference's `[6,2,7,3]`; RT64 row 2
`[3,7,2,6]` vs. the reference's `[1,5,0,4]`; rows 0 and 3 agree). Both tables
remain valid Bayer-shaped tiles — each covers every threshold `0..=7` exactly
twice — so this is a phase/arrangement difference, not a malformed table on
either side. This module ports RT64's literal tables (the supplied authority)
rather than silently reconciling the two, and pins the exact disagreeing
cells with an exhaustive test
(`bayer_matrix_disagrees_with_reference_oracle_at_documented_cells`) so a
future change to either table fails loudly here instead of silently drifting
further apart. Which table (if either) matches real hardware is unresolved by
either this module or the reference's own citation — no silicon measurement
is claimed.

**Quantization: `quantize_post_float_rgba16_non_hdr`.** This function is
deliberately *not* named or shaped like a complete `Float4ToRGBA16` port: it
takes no `float4` and performs none of RT64's float rounding. RT64's real
signature is `Float4ToRGBA16(float4 i, uint dither, bool usesHDR)`, and three
float-facing steps precede the integer tail this module actually covers:

1. `r/g/b = round(clamp(i.r * 255.0f, 0.0f, 255.0f))` — a no-op identity for
   any value that already started as a `u8` RGBA8888 working channel (this
   crate's existing convention, matching `alpha_compare.rs`/`coverage.rs`);
   this module's `Rgba16QuantizeInput.r/g/b: u8` fields assume that identity
   rather than re-deriving it from a float.
2. `cvgModulo = round(i.a * cvgRange) % 8`, where `cvgRange` is
   `usesHDR ? 65535.0f : 255.0f` — **the HDR branch (`usesHDR == true`) is
   out of scope.** This crate has no HDR target format yet
   (`crate::state::ColorImage`/`PixelSize` do not encode one), and porting
   `65535.0f` would require inventing HDR coverage semantics this slice has
   no authority for. The function instead takes an already-checked
   `CoverageModulo8` (an invariant-carrying `0..=7` newtype, same
   private-field/`try_new` shape as `DitherThreshold`) rather than a raw
   alpha float or an unchecked byte — RT64's `cvgModulo` for the
   `usesHDR == false` case only, where `round(i.a * 255.0) % 8` is exactly an
   8-bit alpha channel's low three bits when `i.a` is an exact `u8/255.0`
   value. A future HDR-target slice must port the `usesHDR == true` branch
   separately; this module does not stub, approximate, or silently normalize
   it.
3. `int cvgModulo = ... % 8` is a signed-`int` HLSL modulo; `CoverageModulo8`
   sidesteps the signed/unsigned distinction entirely rather than
   re-deriving it, and its private field statically excludes values a
   `% 8` result could never produce.

From `cvgModulo` onward the ported arithmetic is exact bounded-integer, no
float rounding: `a = (cvgModulo & 0x4) ? 1 : 0`; each channel
`min(channel + dither, 255) >> 3` (saturate at 255, then truncate — not
round — to 5 bits); pack as `(r << 11) | (g << 6) | (b << 1) | a`
(`Formats.hlsli:95-106`). Both `DitherThreshold` and `CoverageModulo8` are
opaque outside this module: a caller cannot construct an out-of-range value
and feed it to the packer, and `try_new` rejects `>= 8` loudly rather than
masking or saturating (AGENTS.md "loud traps, no silent shrugs") — exercised
by hostile-input tests at the exact `7`/`8` boundary for both types.

**Tests.** Exhaustive/literal coverage includes: independent transcription
checks pinning both flat tables against the pinned source; the matrix
cross-check above; every `RgbDither` selector mode including noise-byte and
coordinate independence checks; index periodicity and negative-coordinate
wrapping (including `i32::MIN`/`i32::MAX` boundaries); every 8-bit channel
value against every `0..=7` threshold (256×8 exhaustive); saturation-at-255
(not wraparound) and truncation-not-rounding cases; full RGBA16 packing
including the R11/G6/B1/A0 bit-layout, all-ones/all-zero boundaries, and
independent per-channel dither application; `DitherThreshold`/
`CoverageModulo8` boundary and hostile-rejection tests at `7` (accepted) and
`8` (rejected); and a naga-validated WGSL companion (`rgb_dither.wgsl`) with
the same structural-guard/mutation-detection pattern as `alpha_compare.wgsl`.

**Scope.** This slice covers literal RGB-dither selection and the
`Float4ToRGBA16` non-HDR post-float integer tail only. Out of scope: the
HDR coverage-range branch; RGBA32/other target-format packing; where in the
fragment pipeline RGB dither is actually applied (this crate has no
combiner→blend→dither pipeline ordering yet — see the port card's own
step-10-of-10 placement, `alpha_compare.rs`'s module doc); triangle/rectangle
rasterization; fragment ordering; framebuffer ownership; production DPC
integration; depth/coverage/blend/combiner composition; VI; and native GPU
execution. This module does not modify `state.rs`, `blend.rs`, or any other
existing blender/depth/coverage/alpha-compare/triangle file.
## Raw RDP triangle decoded-to-three-vertices conversion

`raw_dpc::triangle_vertices` (re-exported as `decode_triangle_vertices`/
`TriangleVertices`/`TriangleVertex`) is the literal, characterization-first
Rust port of RT64's `decodeTriangles` fixed-point-to-vertex conversion --
the part of that function between its edge/coefficient reads and its
`state->rdp->drawTris(...)` call. It consumes one already-decoded
`RawTriangle` (the previous section's decoder) and a caller-supplied
`texture_perspective` boolean (RT64's `G_TP_PERSP` OtherMode read, decoded
by the caller -- this module performs no OtherMode decode of its own) and
returns exactly three `TriangleVertex` values, in RT64's exact
`workBufferIndex + 0/1/2` write order: vertex 0 is `(x1, y1 = yh)`, vertex 1
is `(x2, y2 = yl)`, vertex 2 is `(x3 = xl, y3 = ym)`. Source:
`src/gbi/rt64_gbi_rdp.cpp`'s `decodeTriangles`, permitted MIT RT64 pinned by
`docs/RT64-PORT-AUTHORITY.md` at commit
`5473732a822a4423b5696e7cb18fecc425a59875`.

What is reproduced bit-for-bit: RT64's `int16_t v = (w & 0x0000FFFF) << 2
>> 2;` for each wire YL/YM/YH half before dividing by `4.0f`. `w` is
`DisplayList::w0`/`w1`, a `uint32_t`, so both shifts are unsigned logical
shifts on the masked `uint32_t` value; `>>2` exactly undoes `<<2` (nothing
is shifted out, since the masked value never exceeds `0xFFFF`), so the
expression reduces to `w & 0xFFFF` before its narrowing conversion to
`int16_t` -- a plain 16-bit reinterpret, bit-identical to
`RawTriangle::yl()`/`ym()`/`yh()`'s existing `i16` accessors, and *not* the
14-bit sign extension an initial reading of the shift amounts might
suggest (a dedicated hostile test proves the two disagree across every
case where bits 14:15 matter, and that this module takes the 16-bit path);
the Q16.16 `/65536.0f` conversion for XL/XH/XM and their slopes; the
`floorf(yh)` y-floor and the H/M-line x-intercepts (RT64 computes but never
reads `mIntercept`/`l_intercept` again -- this module computes and discards
them identically, for the same reason: literal characterization, not a
functional dependency); the shared `dy_1`/`dy_2`/`dy_3`/`dx_3` coefficients
every optional block's math is built from; the shade/texture half-word
coefficient reconstruction (`((a>>16)<<16)|(b>>16)` / `((a&0xFFFF)<<16)|
(b&0xFFFF)` per RT64 word pair, words 0/2 for base, 1/3 for dx, 4/6 for de --
words 5/7, RT64's commented-out "dy" lane, are provably dead and covered by
a test that mutates them and asserts no change); shaded color normalized by
`1/255.0f`; textured coordinates for both perspective (divide by W, scale by
`1024.0f`, with W itself `65536000.0f / w`) and non-perspective (`*1024.0f /
16384.0f`, W fixed at `1.0f`) modes; and depth's `1/65536.0f/32768.0f`
scale. Optional-block absence reproduces RT64's exact defaults: zero color,
zero texcoord with W = `1.0`, and zero depth.

IEEE f32 results -- including the `+infinity` a zero-W perspective divide
produces, and any NaN a degenerate input can produce -- are preserved
exactly as RT64's own division produces them; no defensive fallback,
clamp, or NaN guard is added anywhere RT64 does not have one. The module's
test suite includes an oracle deliberately written as a second, independent
implementation (a flat `(u32, u32)` word array and hand-rolled bit tests
rather than the typed `RawWord`/`CoefficientWords` API or any of this
module's own helper functions) so a bug shared between the port and its
test would have to appear independently in both.

What is explicitly not yet done: no OtherMode/`RdpState` integration of any
kind (the `texture_perspective` bit is caller-supplied, not decoded here),
no rasterization, no edge walk, no clipping/scissor, no draw call, no GPU
buffer or pipeline, no combiner/blender/depth-compare/coverage/target
write, no batching beyond the one `RawTriangle` supplied, and no RT64
parity or performance claim. RT64's own `// TODO do more than 1 tri at a
time` is observed behavior in the source, not authority to silently drop
triangles here -- this function converts exactly the one triangle it is
given.
## Fragment constant registers: SetEnvColor/SetPrimColor/SetBlendColor/SetFogColor/SetPrimDepth

`state.rs` gains five new typed value types -- `Color4` (shared by
`SetEnvColor`/`SetBlendColor`/`SetFogColor`), `PrimLod` and `PrimColor` (for
`SetPrimColor`), and `PrimDepth` (for `SetPrimDepth`) -- and `RdpState`/
`RdpStateDelta`/`StagedRdpState` each gain matching `env_color`/`prim_color`/
`blend_color`/`fog_color`/`prim_depth` fields and accessors, threaded through
`fork_for_decode`/`apply` exactly like the pre-existing `other_mode`/
`color_image`/`fill_color` fields. `raw_dpc::mod` gains five opcode constants
(`SET_ENV_COLOR`/`SET_PRIM_COLOR`/`SET_BLEND_COLOR`/`SET_FOG_COLOR`/
`SET_PRIM_DEPTH`) and five matching `RawDpcCommandKind` variants, decoded by
`decode_stream`'s existing per-command match using the same `w0`/`w1`
two-32-bit-word extraction every other single-word command already uses; the
T1 `production_adapter` rejects all five exactly like `NoOp`/`SetOtherMode`/
`SetColorImage`/`SetFillColor`/`FillRectangle`/`FullSync`/`RawTriangle` --
loudly, as `UnadmittedRawDpcCommand`, never silently dropped.

This is a literal, characterization-first port of RT64's five setters
(`RDP::setEnvColor`/`setPrimColor`/`setBlendColor`/`setFogColor`/
`setPrimDepth`, `src/hle/rt64_rdp.cpp:837-968`) and their wire-word
extraction (`GBI_RDP::setEnvColor`/`setPrimColor`/`setBlendColor`/
`setFogColor`/`setPrimDepth`, `src/gbi/rt64_gbi_rdp.cpp:95-133`), pinned
commit `5473732a822a4423b5696e7cb18fecc425a59875` per
`docs/RT64-PORT-AUTHORITY.md`. Opcode values (`G_SETENVCOLOR=0xfb`,
`G_SETPRIMCOLOR=0xfa`, `G_SETBLENDCOLOR=0xf9`, `G_SETFOGCOLOR=0xf8`,
`G_SETPRIMDEPTH=0xee`, `src/shared/rt64_f3d_defines.h:145-157`) match the
public SGI *RDP Command Summary*'s "Set Environment Color"/"Set Primitive
Color"/"Set Blend Color"/"Set Fog Color"/"Set Primitive Depth" command
spellings; all five mask to the `RDP_SYNC_LOAD..=0x3f` single-64-bit-word
stride `fn64-render::raw_rdp_command_width` already assigns them, so no
width-table change was needed.

**Color decode (`Color4`, `SetEnvColor`/`SetBlendColor`/`SetFogColor`).**
Each of these three commands' second wire word (`w1`) is the entire payload:
RT64 reads `((color >> 24) & 0xFF)` as red, `>> 16` as green, `>> 8` as
blue, and `>> 0` as alpha, then divides each by `255.0f`. `Color4::rgba8`
reproduces the raw big-endian byte order (matching this crate's existing
`FillColor::rgba32`); `Color4::normalized` reproduces the exact `/255.0`
division as a separate derived accessor, so the raw wire bytes stay
mechanically auditable alongside the float RT64 actually consumes.

**`SetPrimColor` (`PrimColor`, `PrimLod`).** Unlike the other three color
setters, `RDP::setPrimColor` takes three parameters staged together:
`lodFrac`/`lodMin` (from `w0`) and `color` (from `w1`). RT64's own GBI-layer
comment is load-bearing and is preserved verbatim in `PrimLod`'s doc
comment: "While the manual states that lodMin has 8 bits of precision, the
RDP only uses 5 of them" -- `w0` bits 0:7 are the full `lodFrac` byte
(`p0(0, 8)`), but `w0` bits 8:12 are the *only* `lodMin` bits consulted
(`p0(8, 5)`); bits 13:15 of the public 8-bit `lodMin` field are decoded but
discarded, never folded into the 5-bit value. `PrimLod::lod_frac_normalized`
and `lod_min_normalized` reproduce RT64's `lodFrac / 256.0f` and
`lodMin / 32.0f` exactly.

**`SetPrimDepth` (`PrimDepth`).** `w1` bits 16:31 are `z`, bits 0:15 are
`dz` (`p1(16, 16)`/`p1(0, 16)`), matching this crate's existing
`FillRectangle`-style split-word convention. `RDP::setPrimDepth` then masks
`z` to `0x7FFFU` -- **15 bits, not 16** -- before normalizing by
`1.0f / 32767.0f`; `dz` uses the full `0xFFFFU` 16-bit mask and normalizes
by `1.0f / 65535.0f`. `PrimDepth::from_wire` reproduces the asymmetric mask
exactly (`z & 0x7fff`, `dz` unmasked because `p1(0, 16)` is already exactly
16 bits), and a dedicated test proves the wire word's top bit is discarded
even when every other bit is clear.

Nonclaims: no combiner, blender, or depth *consumer* reads these five
registers yet (they are decode/state-storage only, matching this crate's
existing `OtherMode`/`ColorImage`/`FillColor` precedent); no triangle or
rasterizer integration; no GPU or native execution of any kind; no
production-dispatch admission (the T3/T4 seam still rejects all five exactly
like every other still-unadmitted command kind, so a real ABI-driven
submission containing any of these five commands cannot reach a backend
through the production path); and no RT64 parity or performance claim.

## Raw RDP TextureRectangle decode and six-vertex conversion

`raw_dpc::texture_rectangle` (re-exported as `RawTextureRectangle`/
`RawTextureRectangleError`/`texture_rectangle_vertices`/
`TextureRectangleVertex`/`TextureRectangleVertices`/
`TEXTURE_RECTANGLE_COMMAND_BYTES`) is a literal, characterization-first Rust
port of the raw RDP TextureRectangle/TextureRectangleFlip command decode
(opcodes `0x24`/`0x25`) and RT64's deterministic rectangle-to-six-vertex
setup. `production_adapter.rs`'s `push_decoded_raw_dpc` now admits
`RawDpcCommandKind::TextureRectangle` into the sealed plan: it resolves
`texture_rectangle_vertices` against the `OtherMode` current at the
command's own stream position (a `TextureRectangleBeforeAnyOtherMode`
rejection if none has been established yet), rejects a degenerate/empty
rectangle (`DegenerateTextureRectangle`) the same way `texture_rectangle_vertices`
itself would, and splits the six returned vertices into two three-vertex
`RdpTriangleCommand` pushes sharing one `location`/cloned `raw_words` and one
`TriangleSource::TextureRectangle`/`viewport: Some(vertices.viewport)` pair —
the same `Copy` `RectViewportPixels` value on both halves, since RT64
computes `viewportRect` once per draw call, not once per triangle.

Source: the permitted MIT RT64 Rust-port source pinned at commit
`5473732a822a4423b5696e7cb18fecc425a59875` (`docs/RT64-PORT-AUTHORITY.md`).
Wire-field layout comes from `src/gbi/rt64_gbi_rdp.cpp`'s `texrectLLE`/
`texrectFlipLLE` (the raw/LLE wire-decode variants a raw-DPC command stream
actually carries, not the HLE `texrect`/`texrectFlip` that read from a live
RSP `DisplayList**` cursor) and `DisplayList::p0`/`p1`
(`src/gbi/rt64_gbi.cpp`: `((w0 >> pos) & ((0x01 << bits) - 1))` /
`((w1 >> pos) & ((0x01 << bits) - 1))`). The vertex conversion comes from
`src/hle/rt64_rdp.cpp`'s `RDP::drawTexRect` (the copy-mode `dsdx`/`lrx`/
`lry` mutation) and `RDP::drawRect` (fill/copy UL rounding, `FixedRect`
construction, `width`/`height`, `lrs`/`lrt`, `vFractionOffset`, and the six
`triPosFloats`/`triColorFloats`/`triTcFloats` vertex writes), plus
`src/common/rt64_common.cpp`'s `FixedRect::isEmpty`/`left`/`top`/`right`/
`bottom`/`width`/`height` (the bounded default-alignment quarter-pixel
rounding this module ports).

**Decode.** `RawTextureRectangle::decode(opcode, command)` accepts opcode
`0x24` or `0x25` and exactly `TEXTURE_RECTANGLE_COMMAND_BYTES` (16) bytes,
rejecting a wrong opcode before the length is checked and a wrong length
before any word is read. The wire shape is two 64-bit words: word 0's low
half carries `lrx`/`lry` (matching this crate's existing `FillRectangle`
decode of the same shape in `raw_dpc::mod`), word 0's high half carries
`tile`/`ulx`/`uly`, and word 1's two halves carry signed 16-bit
`uls`/`ult`/`dsdx`/`dtdy`. `flip` is not a wire bit: it is derived solely
from which of the two opcodes was decoded (RT64 dispatches `texrectLLE`
with `flip = false` and `texrectFlipLLE` with `flip = true`), exactly as
the pinned source does.

**Conversion.** `texture_rectangle_vertices(rectangle, cycle_type)` accepts
a caller-supplied `state::CycleType` (RT64: `otherMode.cycleType()`,
performing no OtherMode decode of its own) and reproduces, in order: copy
mode's `dsdx >>= 2; lrx |= 3; lry |= 3;`; fill-or-copy mode's `ulx &= ~3;
uly &= ~3;` UL rounding; `FixedRect`'s exact `isNull`/`isEmpty` check
(`ulx > lrx || uly > lry`, then `lrx == ulx || lry == uly`) with no
`movedFromOrigin`/`ExtendedAlignment` origin-stack offset applied (see
Nonclaims); `left`/`top`/`right`/`bottom`'s `(coordinate + 3) >> 2`
quarter-pixel rounding, both ends of both axes always using RT64's
`width(true, true)`/`height(true, true)` ceiling variant; the UV
width/height swap under `flip`; `lrs`/`lrt`'s exact `<<7`, multiply, `+`,
`>>7` operation order; `vFractionOffset`'s `(uly & 0x3) ? (dtdy >> 5) /
32.0f : 0.0f` (reading the *already* fill/copy-rounded `uly`, so
`vFractionOffset` is always exactly `0.0` in fill or copy mode, matching
the pinned source's own mutate-in-place `uly` parameter); and RT64's exact
six `triPosFloats`/`triColorFloats`/`triTcFloats` push order (two
triangles, always the same four clip-space corners and all-zero color,
with `flip` swapping only the texcoord pairing). A reversed or
zero-width/-height rectangle returns `None` -- the exact reproduction of
RT64's `if (drawRect.isEmpty()) { return; }` early return, not a silently
renormalized rectangle. IEEE f32 results (including infinities a
degenerate perspective-adjacent shift/divide can produce) are preserved
exactly as RT64's own arithmetic produces them; no defensive fallback is
added anywhere RT64 does not have one.

**Tests.** Independent oracle re-deriving every formula from the raw wire
bytes and the RT64 source text (i64/f64 internally, narrowing to i32/f32
only where RT64's own `int32_t`/`float` locals narrow) without calling any
production helper; both opcodes; all-zero/max coordinate and tile fields;
signed `uls`/`ult`/`dsdx`/`dtdy` boundaries including `i16::MIN`/`MAX`;
every high wire-prefix combination decoding identically once masked;
wrong-opcode and one-byte-short/oversized rejection; flip/nonflip and
copy/noncopy/fill combinations; fractional UL Y and the exact
`vFractionOffset` value including its always-zero fill/copy case; negative
`dsdx`/`dtdy` arithmetic-right-shift semantics; reversed and
zero-width/zero-height rectangles observed as `None`, not silently
normalized (including a case where copy mode's `lrx |= 3`/`lry |= 3`
mutation turns an otherwise-empty rectangle nonempty); exact six-vertex
position/color/order; `<<7`/multiply/`>>7`/`/32.0` operation-order
sensitivity checks; and a source-shape/mutation sweep (swapped word
halves, unsigned-vs-signed field misinterpretation, a missing copy
mutation, and a missing flip swap) each proven to disagree with the
correctly-mutated oracle path.

**Production admission.** Once admitted, both triangle halves of a
`TextureRectangle`/`TextureRectangleFlip` draw the same real GPU pipeline
`RawTriangle` does: `production.rs`'s `draw_admitted_triangles` maps every
collected draw (rectangle halves and ordinary triangles together, in stream
order) into one `TriangleFixture` each, sets each fixture's `is_rect` flag
from `draw.source == TriangleSource::TextureRectangle` (the vertex shader's
`RasterParams.is_rect` gate, `shaders/triangle_pipeline_vertex.wgsl`), and
derives each fixture's `screen_scale`/`screen_offset` from the draw's own
`viewport` when present (RT64's `convertViewportRect`) or the identity
placement `RawTriangle` has always used when absent. The whole batch then
submits through exactly one `TrianglePipelineRenderer::submit_triangles`
call — one shared render pass, one clear, no reordering — so a rectangle's
two triangles and any other triangles in the same `execute_raw_dpc` call all
survive into one `last_triangle_draw()` output; submitting them separately
would re-clear the target between draws and discard all but the last. A
pre-submit mapping error (`MissingTriangleDrawState`) surfaces before any
fixture reaches the GPU; a batch submission or shader-status error is only
detected after real GPU submission (`complete()` observes real output), but
`self.triangle_draw_output` is never touched until every stage succeeds, so
a failure anywhere leaves the prior successful value in place, unchanged.

**Nonclaims.** No scissor-rectangle intersection and no `movedFromOrigin`/
`ExtendedAlignment` origin-stack offset (RT64's `drawRect` applies both
before constructing its `FixedRect`; this module builds the `FixedRect`
directly from the wire/copy-mutated coordinates -- the exact bounded
default RT64's own `FixedRect` type performs, not an invented alignment or
scissor correction). No texture sampling or TMEM read of the rectangle's own
texel content, no VI/presentation, no full RT64 parity, and no performance
claim of any kind. `RDP::updateCallTexcoords`'s tracked-tile texcoord
bookkeeping and the scissor-intersected `intU1`/`intV1`/`intU2`/`intV2`
branch are workload/tile-tracking side effects this pure conversion has no
state to attach to and does not reproduce.

## RT64 fragment PRNG (`random`)

`random` is a characterization-first literal port of the permitted MIT RT64
Rust-port source pinned at commit `5473732a822a4423b5696e7cb18fecc425a59875`
(`docs/RT64-PORT-AUTHORITY.md`), `src/shaders/Random.hlsli` (SHA-256
`6ce04cebcd02f7269464684f60c1448e8fb2d0d172d93b8860ff1cca5a114fb9`): `initRand`,
`nextRandUint`, and `nextRand`. Its permitted call sites were read directly
from the same pinned local checkout (SHA-256-verified against
`RasterPS.hlsl`'s already-pinned `957b2834…` digest in
`docs/RT64-PORT-AUTHORITY.md`): `src/shaders/RasterPS.hlsl`,
`PostBlendDitherNoisePS.hlsl`, `FbReinterpretCS.hlsl`, `FbWriteColorCS.hlsl`,
and `DebugPS.hlsl` — every one uses the `backoff` default of `16` explicitly,
never a different literal. Like `depth_strict_less`, `alpha_compare`, and
`rgb_dither`, `fn64-render-wgpu` has no crate dependency on
`fn64-render-reference`, so this is a self-contained literal re-expression
citing RT64's source directly.

**API.** [`RandomState`] wraps its `u32` generator word in a private field —
a caller cannot construct one from an arbitrary integer, and a produced
sample (`f32`, `next_unit_float`'s bare return type) can never be confused
with a state. The only public constructors are `init_with_backoff(val0,
val1, backoff)` (the literal three-argument `initRand`) and `init(val0,
val1)` (the observed-default `backoff = 16` convenience form every named
call site actually uses — HLSL default arguments have no Rust equivalent).
`next_uint(&mut self)` is `nextRandUint`'s in-place `s = 1664525u * s +
1013904223u`. `next_unit_float(&mut self) -> f32` is `nextRand`: advances via
`next_uint` first, then returns `float(s & 0x00FFFFFF) / float(0x01000000)`
from the *already-advanced* state — both the 24-bit mask and the exact
`0x01000000` f32 divisor are ported as RT64's own literals, not simplified to
an equivalent shift or a different power-of-two false economy. `raw(self) ->
u32` is a read-only accessor matching the permitted overlay's own
`Fn64RdpTakeFragmentNoiseSample`'s `sample.raw = fragmentRandomState`
(`crates/fn64-render-rt64/ffi/fn64_rt64_raster_ps_overlay.hlsli:16-19`); there
is no `from_raw` constructor, since no surveyed call site ever reads a
generator word without first producing it through `init`/`init_with_backoff`.

All arithmetic is wrapping `u32`, matching HLSL `uint`'s defined
modular-add/multiply/shift semantics exactly — `backoff == 0` performs the
loop's zero iterations and returns `val0` unmodified, and `nextRandUint`'s
multiply-add wraps at the `u32` boundary rather than panicking or saturating.

**Tests.** Independent literal characterization (every expected value
hand-derived by a from-scratch reference implementation outside this crate,
never by calling `RandomState`'s own methods): `backoff` 0/1/16 at zero, max,
and mixed seeds; `u32` wrap boundaries for both `initRand`'s internal
arithmetic and `nextRandUint`'s multiply-add; first and multi-step
`nextRandUint`/`nextRand` sequences; the exact masked numerator and its f32
quotient. Caller-shaped fixtures reproduce each named call site's exact seed
composition (`RasterPS`'s `frameCount`/pixel-product seed with sequential
combiner-then-alpha-compare draws proving order-sensitivity; three-channel
sequential draws for `PostBlendDitherNoisePS` and `DebugPS`; the distinct
flat-index vs. coordinate-product seed shapes for `FbWriteColorCS` and
`FbReinterpretCS`). Mutation-shaped tests target swapped `v0`/`v1` update
constants, wrong shift direction, non-wrapping arithmetic, wrong update
order, mask/denominator drift, and return-before-advance. A bounded
Rust-vs-WGSL differential parses the retained `random.wgsl` into Naga IR and
cross-checks its literal hex/decimal constants and exposed function names
against the Rust side, plus a small deterministic seed×backoff grid run
against a from-scratch reference formula — this crate has no native-adapter
execution path for WGSL (matching `rgb_dither.rs`'s precedent), so this is a
structural/textual differential, not a GPU-executed one.

**Scope.** This module characterizes `Random.hlsli` in isolation. It does
not wire into `combiner`, `alpha_compare`, `rgb_dither`, any
shader-pipeline/draw-path, `raw_dpc`, `state.rs`, `tmem`, the ABI/runtime, or
any native GPU execution. It makes no randomness-quality claim (this is
RT64's own PRNG, transcribed exactly, not evaluated), no silicon/hardware
claim (the RDP's real noise generator remains unpublished, per
`docs/RDP-SILICON-VECTORS.md`), and no parity or performance claim. This
module does not modify `state.rs`, `combiner.rs`, `alpha_compare.rs`,
`rgb_dither.rs`, or any other existing file.

## Texture-coordinate generation (`texture_gen`)

`texture_gen` is a characterization-first literal port of the permitted MIT
RT64 Rust-port source pinned at commit
`5473732a822a4423b5696e7cb18fecc425a59875` (`docs/RT64-PORT-AUTHORITY.md`),
`src/shaders/TextureGen.hlsli`: `normalizeSafe` (lines 9-17) and
`computeTextureGen` (lines 19-34). Like `depth_strict_less`, `alpha_compare`,
and `rgb_dither`, `fn64-render-wgpu` has no crate dependency on
`fn64-render-reference`, so this is a self-contained literal re-expression
citing RT64's source directly.

**`normalize_safe`.** Returns `v / length(v)` when `length(v) > 0`, and `v`
unchanged otherwise -- covering both the zero vector (`length == 0`) and any
vector whose length is `NaN`: IEEE-754 `NaN > 0` is `false`, so HLSL's
`if (l > 0)` and this port's plain `f32` `>` both fall through to the
unchanged branch identically, with no special-case added for `NaN`.

**`compute_texture_gen`: the `mul(vector, matrix)` operand order.** RT64's
own pinned source tree calls `mul` two different ways: `RSPWorldCS.hlsl`
calls it matrix-first (`mul(worldMats[i], float4(pos, 1.0))`, the
column-vector convention), while `TextureGen.hlsli` calls it vector-first
(`mul(float4(lookAt.x, 0.0f), worldMatrix)`, the row-vector convention).
HLSL's `mul(x, y)` overload resolves structurally on argument shape, not a
project-wide convention, so both calls are individually well-defined but
compute the transpose of each other. This module ports `TextureGen.hlsli`'s
own literal row-vector form exactly as written (`result[c] = sum_r x[r] *
y[r][c]`) rather than reconciling it with the other file's opposite
convention -- reconciling the two would be silently changing the ported
arithmetic, not preserving it.

**Arithmetic, in RT64's exact operation order.** Each `RSPLookAt` axis is
transformed by the caller-supplied row-major `WorldMatrix`, `normalize_safe`d
on its `.xyz`, then `dot`ted against `inputNormal` to produce `texgenUV.x`/
`texgenUV.y`. Both scalars are `clamp`ed to `[-1, 1]` **unconditionally,
before** branching on `texture_gen_linear` -- the clamp is not
mode-conditional. Linear mode: `acos(-texgenUV) * 325.94932` (RT64's own
comment: `1024 / PI`, kept as the pinned source's literal digit sequence
rather than a runtime-computed `1024.0 / PI`, since both spellings round to
the same `f32` value and this port preserves RT64's cited literal text).
Non-linear mode: `texgenUV += (1, 1)` then `texgenUV *= 512.0`, reproduced as
the same two separate ops rather than a fused single expression. Finally,
regardless of mode: `(inputUV / 65536.0) * texgenUV`.

An independently-derived Rust oracle (`compute_texture_gen`) is matched by an
owned, Naga-validated WGSL transcription (`shaders/texture_gen.wgsl`,
`TEXTURE_GEN_WGSL`, entry point `TEXTURE_GEN_ENTRY_POINT`); neither is
compiled into any pipeline or wired to a draw path, matching
`alpha_compare.wgsl`/`rgb_dither.wgsl`'s precedent.

**Tests.** Independent hand-derivation of `normalize_safe` across zero,
unit-axis, arbitrary, and negative-component vectors; a `NaN`-length case
distinguishing the IEEE-754 comparison from a special-cased guard; identity-
and rotation-matrix `compute_texture_gen` cases with independently computed
expected values; both linear-mode boundary dot products (`-1`, `0`, `+1`)
matching `acos`'s known values at those points; non-linear-mode positive,
negative, and zero dot products; a structural test that a non-symmetric
matrix distinguishes row-vector from column-vector `mul` (so a silently
transposed operand order fails loudly); signed and zero UV-scale cases; and
the same naga parse/validation/structural-guard/hostile-mutation WGSL
pattern used by `rgb_dither.wgsl`.

**Scope.** No RSP lookat-matrix derivation (`RSPLookAt` is caller-supplied,
matching this module's pure value-in/value-out convention), no world-matrix
upload/storage-buffer plumbing, no vertex-shader integration, no
combiner/texture-sample consumption of the returned UV, no draw-path or
production-DPC wiring, and no RT64 visual/pixel/silicon parity or
performance claim. This module does not modify `state.rs` or any other
existing file besides `lib.rs`'s module registration and re-exports.

## `FloatToUINT8`/`Float4ToRGBA32`/`AlphaDitherValue` (`formats_dither`)

`formats_dither` is a characterization-first literal port of the permitted
MIT RT64 Rust-port source pinned at commit
`5473732a822a4423b5696e7cb18fecc425a59875` (`docs/RT64-PORT-AUTHORITY.md`),
`src/shaders/Formats.hlsli`'s three remaining unported primitives:
`FloatToUINT8` (line 67), `Float4ToRGBA32` (lines 122-127), and
`AlphaDitherValue` (lines 41-54). Like `depth_strict_less`, `alpha_compare`,
`rgb_dither`, and `random`, `fn64-render-wgpu` has no crate dependency on
`fn64-render-reference`, so this is a self-contained literal re-expression
citing RT64's source directly.

**API.** `float_to_uint8(f32) -> u8` is `round(clamp(i, 0.0f, 1.0f) *
255.0f)`: clamp first, scale, then round-half-to-even (`f32::round_ties_even`,
matching HLSL `round`'s tie-breaking, not `f32::round`'s round-half-away-from-
zero). `float4_to_rgba32(r, g, b, a) -> Rgba32Packed` packs four independently
quantized channels into one `u32`, `r` in the high byte and `a` in the low
byte -- no cross-channel coupling, unlike `Float4ToRGBA16`'s
alpha-as-coverage-modulo special case. `alpha_dither_value(color_dither_bit,
alpha_dither, x, y, noise) -> u8` selects a `0..=7` alpha-dither threshold: it
reuses `crate::rgb_dither::dither_pattern_value`/`RgbDither` for the
`Pattern`/`InversePattern` ordered-tile lookup (RT64's own
`AlphaDitherValue` calls `DitherPatternValue` internally) rather than
re-transcribing the Bayer/MagicSquare tables a third time, and reuses
`crate::state::AlphaDither` for the mode selector (its wire encoding --
`0=Pattern,1=InversePattern,2=Noise,3=Disabled` -- is byte-identical to
`Formats.hlsli`'s own `PATTERN`/`NOTPATTERN`/`NOISE`/`DISABLE` switch cases).
`InversePattern` is the literal bitwise `(~DitherPatternValue(...)) & 7`, not
a `7 - threshold` rewrite, even though both forms agree for every in-range
3-bit input.

`AlphaDitherValue` (this module's `alpha_dither_value`) is distinct from the
already-landed `crate::alpha_compare::apply_alpha_dither`: the latter ports a
different, higher-level RT64 function (`RasterPS.hlsl`'s alpha-dither
reduction, sourced from `fn64-render-reference`'s `blend.rs:75-103`) that
rounds a full eight-bit alpha down to a five-bit blender input; this module's
function is the lower-level `Formats.hlsli` primitive that selects a raw
`0..=7` threshold, structurally parallel to `DitherPatternValue`
(`crate::rgb_dither::dither_pattern_value`) rather than a duplicate of
`apply_alpha_dither`.

An independently-derived Rust oracle is matched by an owned, Naga-validated
WGSL transcription (`shaders/formats_dither.wgsl`, `FORMATS_DITHER_WGSL`,
entry point `FORMATS_DITHER_ENTRY_POINT`); neither is compiled into any
pipeline or wired to a draw path, matching `rgb_dither.wgsl`/`random.wgsl`'s
precedent.

**Tests.** Independent hand-derivation of `float_to_uint8` at every
representable byte (exhaustive round-trip), clamp boundaries (including `NaN`,
`+-infinity`), and explicit round-half-to-even fixtures; `float4_to_rgba32`
channel-placement, saturation, and full-word packing cases matched against an
independently composed expected `u32`; `alpha_dither_value`'s four modes each
checked against an independently derived oracle (`Pattern`/`InversePattern`
cross-checked directly against `dither_pattern_value`, `InversePattern`'s
bitwise-not-and-mask independently re-derived via `u32` arithmetic and
separately proven equal to `7 - threshold` for the `0..=7` domain, `Noise`
against `byte & 7`, `Disabled` against a constant `0`); mutation-shaped tests
distinguishing `Pattern` from `InversePattern`, `Disabled` from `Pattern`, and
proving `color_dither_bit` truly switches tables; and the same naga
parse/validation/structural-guard/hostile-mutation WGSL pattern used by
`rgb_dither.wgsl`/`random.wgsl`.

**Scope.** This module characterizes `Formats.hlsli`'s three named primitives
in isolation. It does not wire into `combiner`, `alpha_compare`, `rgb_dither`,
`random`, any shader-pipeline/draw-path, `raw_dpc`, `state.rs`, `tmem`, the
ABI/runtime, or any native GPU execution. It makes no parity or performance
claim. This module does not modify `state.rs`, `combiner.rs`,
`alpha_compare.rs`, `rgb_dither.rs`, `random.rs`, or any other existing file
besides `lib.rs`'s module registration and re-exports.

## `EndianSwapUINT16`/`EndianSwapUINT32`/`EndianSwapUINT` (`endian_swap`)

`endian_swap` is a characterization-first literal port of the permitted MIT
RT64 Rust-port source pinned at commit
`5473732a822a4423b5696e7cb18fecc425a59875` (`docs/RT64-PORT-AUTHORITY.md`),
`src/shaders/FbCommon.hlsli:9-33`: `EndianSwapUINT16` (line 10),
`EndianSwapUINT32` (line 19), and `EndianSwapUINT` (lines 23-36). Like
`depth_strict_less`, `alpha_compare`, `rgb_dither`, `random`, and
`formats_dither`, `fn64-render-wgpu` has no crate dependency on
`fn64-render-reference`, so this is a self-contained literal re-expression
citing RT64's source directly.

**API.** `endian_swap_uint16(u32) -> u32` swaps the low two bytes:
`((i << 8) & 0xFF00) | ((i >> 8) & 0xFF)`. `endian_swap_uint32(u32) -> u32`
reverses all four bytes: `((i << 24) & 0xFF000000) | ((i << 8) & 0xFF0000) |
((i >> 8) & 0xFF00) | ((i >> 24) & 0xFF)`. `endian_swap_uint(i, siz) -> u32`
dispatches by pixel size: `Bits4`/`Bits8` are no-ops, `Bits16` calls
`endian_swap_uint16`, `Bits32` calls `endian_swap_uint32`. `siz` reuses the
already-landed `crate::state::PixelSize` enum rather than a raw
`G_IM_SIZ_*` integer, so it is an exhaustive four-variant match with no
wildcard/default arm -- RT64's own `EndianSwapUINT` has an explicit
`default: return 0` case for a `siz` value outside the four known
`G_IM_SIZ_*` encodings, which has no counterpart to port on the **Rust
side** since `PixelSize` cannot represent an out-of-range size in the first
place: a stronger, compile-time guarantee than RT64's own runtime check, not
a divergence from it.

That does not extend to the WGSL companion. `shaders/endian_swap.wgsl`'s
`endian_swap_uint(i: u32, siz: u32)` takes an unrestricted `u32`, exactly
like RT64's HLSL, so an out-of-range `siz` is representable there at
runtime -- the WGSL function replicates RT64's `default: return 0` branch
literally rather than inheriting the Rust side's type-level exhaustiveness.
The two sides reject invalid input for different reasons: the Rust API
rejects it at compile time by construction (there is no way to construct an
invalid `PixelSize`), the WGSL function rejects it at runtime with the same
explicit fallback branch RT64 itself uses.

An independently-derived Rust oracle is matched by an owned, Naga-validated
WGSL transcription (`shaders/endian_swap.wgsl`, `ENDIAN_SWAP_WGSL`, entry
point `ENDIAN_SWAP_ENTRY_POINT`); neither is compiled into any pipeline or
wired to a draw path, matching `rgb_dither.wgsl`/`random.wgsl`/
`formats_dither.wgsl`'s precedent.

**Tests.** Independent hand-derivation for `endian_swap_uint16` (zero,
known byte-pair swaps, high-bit masking, self-inverse property, and an
exhaustive low/high-byte grid) and `endian_swap_uint32` (zero, known
four-byte reversals, cross-check against Rust's own unrelated
`u32::swap_bytes`, self-inverse property, and a byte-placement grid);
`endian_swap_uint`'s four-way size dispatch checked against
`endian_swap_uint16`/`endian_swap_uint32` directly plus a mutation-shaped
test proving the four sizes are observably distinct dispatch outcomes; the
same naga parse/validation/structural-guard/hostile-mutation WGSL pattern
used by `rgb_dither.wgsl`/`random.wgsl`/`formats_dither.wgsl`; a source-text
guard confirming the WGSL `endian_swap_uint` body's final branch is a bare
`return 0u;` (not a `return i;` no-op) so the default fallback can't
silently regress; and a parser-independent grid differential over a hostile
`siz` set including `4`, `5`, and `u32::MAX` -- values `PixelSize` cannot
represent -- proving the WGSL source's documented branch semantics return
`0` for every one of them, cross-checked against the Rust
`PixelSize`-typed API for every in-range `siz`.

**Scope.** This module characterizes `FbCommon.hlsli`'s three named
endian-swap primitives in isolation. It does not wire into `combiner`,
`formats_dither`, `rgb_dither`, `random`, `raster_vs`, `texture_gen`, any
shader-pipeline/draw-path, `raw_dpc`, `state.rs`, `tmem`, the ABI/runtime, or
any native GPU execution. It makes no RT64 parity or performance claim. This
module does not modify `state.rs`, `combiner.rs`, `formats_dither.rs`,
`rgb_dither.rs`, `random.rs`, `raster_vs.rs`, `texture_gen.rs`, or any other
existing file besides `lib.rs`'s module registration and re-exports.
## Fragment-register-to-combiner-input wiring (`combiner_inputs_from_fragment_registers`)

`combiner.rs` gains one pure conversion function,
`combiner_inputs_from_fragment_registers`, closing the gap the [Fragment
constant registers](#fragment-constant-registers-setenvcolorsetprimcolorsetblendcolorsetfogcolorsetprimdepth)
section above flagged as a nonclaim ("no combiner ... consumer reads these
five registers yet"): it is the first site in this crate that ever
constructs a [`CombinerInputs`] from real decoded fragment-register state
rather than a raw test literal (verified by grepping every existing
`CombinerInputs { .. }` construction site before writing this function --
each one was a hand-picked test fixture).

The exact field mapping is read directly from RT64's own combiner-input
assembly, `RasterPS.hlsl:169-183` (pinned commit
`5473732a822a4423b5696e7cb18fecc425a59875`, `docs/RT64-PORT-AUTHORITY.md`):

```text
ccInputs.primColor   = instanceRDPParams[instanceIndex].primColor;
ccInputs.envColor    = instanceRDPParams[instanceIndex].envColor;
ccInputs.primLodFrac = instanceRDPParams[instanceIndex].primLOD.x;
```

Cross-checked against where those three right-hand-side values are actually
staged (`RDP::setEnvColor`/`setPrimColor`, `src/hle/rt64_rdp.cpp:838-842,
861-871`): `primColor`/`envColor` are the already-`/255.0`-normalized
`float4` this crate's own [`Color4::normalized`] reproduces exactly, and
`primLOD.x` is `lodFrac / 256.0f`, exactly this crate's
[`PrimLod::lod_frac_normalized`] (reached through
[`PrimColor::lod`]/[`PrimColor::color`]). `env_color`, `prim_color`, and
`prim_lod_frac` are the only three [`CombinerInputs`] fields this mapping
touches; every other field (`tex_val0`/`tex_val1`/`shade_color`/
`key_center`/`key_scale`/`lod_fraction`/`noise`/`k4`/`k5`) is sourced from
texture sampling, vertex interpolation, separate host-tracked RDP state, a
per-pixel GPU LOD derivative, a PRNG draw, and a separate `convertK` table
respectively -- none of them read `env_color`/`prim_color`/`prim_lod_frac`/
`PrimDepth` in RT64's own assembly, so `combiner_inputs_from_fragment_registers`
takes a caller-supplied `base: CombinerInputs` for those fields and only
overrides the three it can actually derive, rather than inventing values for
fields this seam has no authority over.

**Nonclaims.** [`PrimDepth`] is not read here: `rt64_color_combiner.h`'s
`Inputs` struct (the type `ccInputs` is) has no `primDepth` field at all --
`instanceRDPParams[...].primDepth` is read elsewhere in the same shader
(`RasterPS.hlsl:100`, depth testing), a different consumer entirely. This
was verified by grep, not inferred from field-name absence alone.
`PrimLod::lod_min_normalized` (`primLOD.y`) is likewise decoded by this
crate but never read by this specific assembly block, and a dedicated test
proves varying `lod_min` alone leaves the result unchanged. As with every
other module in this crate, there is no `OtherMode` field folded into this
mapping: `RasterPS.hlsl` sets `ccInputs.otherMode` separately, and
`rt64_color_combiner.h`'s own `Inputs.otherMode` field is consulted only for
`cycleType()` cycle-mode dispatch (`runCycle`/`run`), which this crate's
`run_combiner` already takes as an explicit `CombinerCycleMode` parameter,
not a `CombinerInputs` field -- so no `OtherMode` field is reachable from
this mapping's three target fields, and none is threaded through. No
cycle-mode selection, no NOISE/LOD real generation, no texture-fetch
integration, no draw-path or production-dispatch wiring, and no RT64
parity/performance claim.

**Tests.** An independent-oracle test constructs distinct, non-zero
per-byte wire words for `env_color`/`prim_color`/`prim_lod_frac`, computes
the expected normalized floats by hand from those raw bytes
(`byte / 255.0`, `lodFrac / 256.0`) rather than by calling this crate's own
`Color4::normalized`/`PrimLod::lod_frac_normalized` and asserting against
their output, and separately confirms every other `CombinerInputs` field
passes through the caller-supplied `base` untouched. A boundary test covers
the all-zero and all-`0xFF` wire-word extremes, including the `0xFF / 256.0`
corner that never reaches exactly `1.0` (unlike the `/255.0` fields, which
do at `0xFF`). A `lod_min`-invariance test proves varying only `lodMin`
leaves the result identical. A final test exercises `PrimDepth::from_wire`
directly (independent of this function, since it takes no `PrimDepth`
parameter at all) to make the 15-bit-mask omission from this function's
signature a documented, tested API fact rather than a silent gap.

## SetCombine: raw-DPC decode and durable state retention

`raw_dpc`'s decoder admits `SetCombine` (`G_SETCOMBINE` = `0xfc`, masked to
this crate's low-six-bit command field as `0xfc & 0x3f = 0x3c`; public SGI
*RDP Command Summary* "Set Combine Mode" / libultra `gDPSetCombineMode` /
`gsDPSetCombineLERP`), a single 64-bit command word exactly like the other
already-admitted fragment constant registers (`SetEnvColor`, `SetPrimColor`,
`SetBlendColor`, `SetFogColor`, `SetPrimDepth`). `fn64_render::raw_rdp_command_width`
already assigned opcode `0x3c` an 8-byte width before this slice; this slice
adds the decode arm and typed `RawDpcCommandKind::SetCombine(CombineParams)`
variant that consumes it.

**Wire-fidelity property.** `RDP::setCombine` (pinned RT64 commit
`5473732a822a4423b5696e7cb18fecc425a59875`, `src/hle/rt64_rdp.cpp:295-302`)
stores `combineL` as the command's exact first wire word (`w0`) and
`combineH` as the exact second wire word (`w1`) — RT64 never masks or strips
`w0`'s top opcode byte before storing it. This decoder's `SET_COMBINE` arm
reproduces that exactly: it calls `CombineParams::from_wire(w0, w1)` with the
full, unmasked 32-bit `w0` this module already extracted before any opcode
masking, not a "cleaned" value with its top byte removed. A dedicated test
(`set_combine_w0_is_passed_through_completely_unmasked`) proves this with an
exact captured-shaped fixture (`w0 = 0xfc8f_ff1f`, `w1 = 0x88fc_f279`),
asserting `CombineParams::low()`/`high()` equal those words byte-for-byte,
plus independently hand-derived (not self-asserted) `decode_color`/
`decode_alpha` selector results for both cycles.

`combiner.rs`'s `CombineParams::from_wire` — previously `pub(crate)` with an
`#[allow(dead_code)]` marker and a comment stating no opcode handler existed
yet — is now called from exactly this decode arm; its signature and
arithmetic are unchanged, and the selector-decode tables/one-cycle/two-cycle
arithmetic this crate already ported for `CombineParams` remain exactly as
characterized in "Color combiner: one-cycle/two-cycle selector arithmetic"
above. This slice does not add a combiner-evaluation consumer of the newly
decoded state, does not wire `CombineParams` into
`combiner_inputs_from_fragment_registers`, and does not touch
`production_adapter.rs`'s admission logic beyond the strictly-additive match
arms every new `RawDpcCommandKind` variant requires to keep that module's two
exhaustive matches (`opcode_name`, the v11 TMEM-only admission dispatch)
compiling — `SetCombine` is rejected there by the same "outside the admitted
zero-guest-write TMEM/state subset" path every other non-TMEM command kind
already takes, not newly admitted into production.

**Durable state retention.** `RdpState`/`RdpStateDelta`/`StagedRdpState`
each gain a `combine: Option<CombineParams>` field, following exactly the
same "current value + tracked change" shape `env_color`/`prim_color`/etc.
already use: `RdpState::fork_for_decode` copies it, `RdpState::apply` takes
the delta's value when present (last-command-wins within one packet, proven
by `set_combine_sequential_overwrite_last_command_wins_in_one_packet`), and
`RdpStateDelta::set_combine` records only the decoded command
(`set_combine_state_delta_records_only_the_decoded_command`). A dedicated
cross-packet test
(`set_combine_is_retained_as_durable_state_across_a_packet_boundary_with_no_intervening_set_combine`)
proves the retention property the task required explicitly: a `SetCombine`
decoded in packet N is still the active `staged_state().combine()` value read
in packet N+1 after `decode_raw_dpc_after` chaining, with no intervening
`SetCombine` in packet N+1 and packet N+1's own `state_delta().combine()`
correctly reporting `None`.

**Nonclaims.** No combiner-evaluation wiring of the newly decoded state (no
`CombinerInputs`/`run_combiner` call site reads `RdpState::combine` in this
slice), no `production_adapter.rs` admission of `SetCombine` into the v11
TMEM-only production plan, no shader/pipeline/draw-path integration, and no
RT64 pixel/visual/silicon parity or performance claim.

## 2026-08-16: Alpha compare, fragment-callable WGSL seam

Per `/private/tmp/fn64-rt64-alpha-compare-fragment-seam-card.md` (a read-only
planning artifact, not a file committed to this repository): re-expresses
the already-characterized, already-tested RDP alpha-compare arithmetic
(`None`/`Threshold`/`Dither` general gate plus the copy-cycle RGBA16
hard-alpha-bit special case, "Alpha compare (port card §3)" above) as a
plain, fragment-callable WGSL function, alongside the existing standalone
`@compute` seam it already had.

`alpha_compare.wgsl`'s existing `general_compare`/`evaluate` functions take
a single `AlphaCompareInput` struct read from a storage buffer and return
`u32` (`1u`/`0u`), matching that file's `@compute @workgroup_size(64)`
dispatch shape. The new sibling file, `alpha_compare_fragment_fn.wgsl`,
re-expresses the identical arithmetic verbatim as `alpha_compare_fragment_fn`
— an ordinary function taking five scalar `u32` arguments (`mode`, `alpha`,
`threshold_alpha`, `noise_byte`, `copy_cycle_rgba16`) and returning `bool`,
with no `@compute`, `@group`/`@binding`, or entry point of its own. This
mirrors `alpha_compare_value`/`copy_alpha_compare_value`'s own Rust-side
signature exactly, rather than inventing a new call shape, and is callable
from a future `@fragment` entry point via WGSL's ordinary
function-concatenation build seam — the same mechanism
`shaders/triangle_pipeline_fragment.wgsl`'s own header already documents for
`color_combiner.wgsl`. `alpha_compare.rs` exposes it as a new constant,
`ALPHA_COMPARE_FRAGMENT_FN_WGSL` (`include_str!`), re-exported from this
crate's root alongside the existing `ALPHA_COMPARE_WGSL`/
`ALPHA_COMPARE_ENTRY_POINT` constants. The existing `alpha_compare.wgsl`
`@compute` entry point, and every existing test in `alpha_compare.rs`, are
unmodified.

**Tests.** Naga parse/validation and textual structural guards (the exact
cross-multiply expression, the exact `None`/`Threshold`/copy-cycle literal
returns) mirror the sibling file's own existing convention. The load-bearing
addition is a CPU-vs-WGSL differential over a frozen fixture partition
(Threshold mode's four boundary cases including the non-strict `>=` equal
case; Dither mode's exact `alpha*256 > noise_byte*255` cross-multiply tie
boundary and its neighbors; unconditional `None`-mode pass; and the
copy-cycle RGBA16 hard-alpha-bit special case, including the case that most
sharply distinguishes it from the general `Threshold` path). Every fixture
case is computed three independent ways and asserted equal: the frozen,
hand-derived expected boolean; the Rust oracle
(`alpha_compare_value`/`copy_alpha_compare_value`, `#[cfg(test)]`, no GPU);
and the new WGSL function itself, executed on a real native adapter through
a minimal `#[cfg(feature = "host-gpu-tests")]` compute-shim harness — a
throwaway `@compute` entry point that calls `alpha_compare_fragment_fn`
directly per fixture case and reads its boolean result back via a storage
buffer. That harness is new test-only scaffolding; it does not claim the
function runs inside any real fragment shader (see Nonclaims below). The
harness ran 10 consecutive times from a clean process each time on this
crate's certified M2.2 Metal adapter (`Apple M5 Pro`), all passing, with no
`NoAdapter` fallback observed.

**Nonclaims.** This slice does not touch `targets/triangle_pipeline.rs`,
`shaders/triangle_pipeline_{fragment,vertex}.wgsl`, or `production.rs` — no
triangle drawn through the real production pipeline is alpha-compare-gated
by this slice, and no bind-group/uniform layout for `threshold_alpha`,
mode, or a per-fragment noise sample is decided here. Two other lanes
already claim those three files concurrently as of this slice's freeze
(varying-UV committed-TMEM texture sampling into the production triangle
draw path, and `TextureRectangle`/`TextureRectangleFlip` decode admission);
this slice is independent of both — it touches none of their claimed paths
and requires nothing either produces. The deferred wire-in step (adding a
bind-group entry or uniform field to `triangle_pipeline_fragment.wgsl`,
calling this function from `fs_main`, deciding the discard-vs-alpha-zero
mechanism for a failed compare, and sourcing `threshold_alpha`/mode/noise
from already-staged `SetBlendColor` state and the fragment's own combiner
alpha output) becomes safe to dispatch once either lane's own
`RasterParams`/`fs_main` diff is known, so this new function can be
composed into it without reopening either lane's in-flight work — not
attempted in this slice. Also out of scope, matching the port card: alpha
dither (`apply_alpha_dither`, a distinct pre-blend mechanism, already ported
but untouched here), `Dither` mode's noise-source architecture (this slice
supplies literal noise bytes only, same as the existing Rust tests),
`depth_mode`/`coverage`/`blend`'s equally-unwired characterized seams, RT64
pixel/visual parity, and any performance or throughput claim.
## 2026-08-16: Published committed-TMEM textured-triangle draw (Option B)

Wires the already-characterized RDP texture-addressing/three-nearest-filter
arithmetic (`tmem/read.rs`+`tmem/sample.rs`) into `targets/triangle_pipeline`'s
previously flat-colored, textureless fragment shader, producing a published,
committed-TMEM, textured triangle for the first time: the fragment shader
runs the ported RDP addressing/filter math per fragment, for real varying UVs
interpolated across a triangle, not a single pre-resolved constant color.
This confirms Option B (the frozen, mandatory design per the implementation
card's §3): the raw committed physical-TMEM byte image and its validity
bitmap are uploaded once per commit generation as **read-only storage
buffers** and sampled by new WGSL functions running per fragment — never a
CPU-resolved RGBA8 image sampled through wgpu's own hardware
`textureSample`/`textureLoad` (Option A, rejected outright: that design never
executes the ported RDP sampling arithmetic per fragment for varying UVs).

**New WGSL module.** `shaders/tmem_sample.wgsl` — a library-only file (no
`@group`/`@binding` entry point of its own besides its module-scope
storage/uniform bindings), concatenated ahead of
`triangle_pipeline_fragment.wgsl`'s `@fragment` wrapper at the same Rust
build seam `color_combiner.wgsl` already uses
(`shader_manifest::triangle_pipeline_fragment_wgsl`). Ports, WGSL-for-Rust,
the same functions this crate's own `tmem/read.rs`/`tmem/sample.rs` already
characterize and test: `relative_axis_coordinate`/`address_axis_texel`
(S10.5 clamp/wrap/mirror/mask), `address_texture_cell_wgsl` (2x2 cell
gather plus 5-bit S/T fractions), `tmem_rgba16_texel_address`/
`tmem_rgba16_byte_address` (tile-relative linear byte address, odd-row XOR4
parity — applied independently per addressed byte, matching
`tmem/read.rs`'s own `read_linear_bytes` exactly, not once on a shared base
address), `decode_rgba16` (byte-for-byte port of
`direct_texel_decode.wgsl`'s RGBA5551 expansion), and
`filter_three_nearest_wgsl` (the same fixed-point formula
`shaders/three_nearest_filter.wgsl` already carries). Scope: RGBA16 direct
format only, matching this slice's own fixed fragment-shader call site —
CI4/CI8/TLUT, RGBA32, IA4/IA8/IA16, I4/I8, and YUV are out of scope.

**Frozen `TmemFirstRowParity::Even`** (this crate's own existing test-fixture
default wherever a caller must supply one without a load-tile-derived value)
for every sample this slice issues — not a claim about hardware's actual
load/render-tile aliasing resolution, which remains genuinely unsettled (see
the M4.3.3d passage this repeats verbatim: "the reference lane derives
parity from the render tile's ULT while pinned RT64's raw-TMEM shader uses
the relative row, and neither settles load/render-tile aliasing on
hardware").

**GPU-side validity-sentinel encoding** (a new, named choice this slice
introduces, not prior art): a parallel bitmap, one bit per TMEM byte
address, packed 32 bits per `u32` word (`tmem/gpu_projection.rs`'s
`TmemGpuProjection`/`TMEM_VALIDITY_WORDS = 128`). Chosen over a
reserved-sentinel-value scheme because a sentinel value would either forbid
a real byte value from ever equalling it or require widening every stored
byte to a larger type; a same-width parallel bitmap instead mirrors this
crate's own CPU-side `valid: Box<[bool; TMEM_LEN]>` shadow array one-for-one
at 1/32 the size a per-byte `u32` flag array would cost.

**Bind-group extension** (`targets/triangle_pipeline.rs`): bindings 2/3
(read-only storage buffers, `tmem_bytes: array<u32, 1024>`/
`tmem_validity_words: array<u32, 128>`, matching this crate's own
`direct_texel_decode.wgsl`/`three_nearest_filter.wgsl` storage-buffer
convention, not `texture_2d`/`sampler`) and binding 4 (`TileBindingParams`
uniform — `tmem/gpu_projection.rs`'s host-side mirror of the WGSL struct,
field-for-field in the same declaration order, mechanically proven equal by
`to_bytes_offsets_match_the_wgsl_tile_binding_params_declaration_order`). A
second color attachment (`@location(1)`, `R32Uint`) carries
`tmem_sample.wgsl`'s own `TMEM_SAMPLE_STATUS_*` code per fragment — the
observable-shader-failure-status channel a fragment shader has to report a
per-pixel typed failure back to the CPU; `production.rs`'s
`draw_admitted_triangles` reads this back and turns any non-OK status into
the new `WgpuRawDpcExecutionError::TmemSampleFailed` variant, never silently
accepted.

**Tile-binding snapshot** (`production.rs`'s `PlanCollector`, mirroring
`raw_dpc::triangle_draw_data::TriangleDrawStateCollector`'s identical
addition): tracks tile 0's `SetTile`/`SetTileSize` binding current at the
walk's own stream position — tile 0 is the RDP's default bound texture tile
for a standard triangle draw (`RdpTriangleCommand` carries no tile index of
its own) — snapshotted onto each `RetrievedTriangleDraw` alongside the
existing `OtherMode`/`CombineParams` walk-local tracking.
`TileBindingParams::unbound()` (loud `bound = 0`) when no tile-0 binding was
established at that triangle's stream position; the WGSL sampler checks
`bound` before touching TMEM and reports
`TMEM_SAMPLE_STATUS_NO_TILE_BINDING`, never a silent default.

**Production-format rejection.** `TileBindingParams` carries the bound
tile's `format`/`pixel_size` wire codes; `tmem_sample.wgsl`'s
`sample_committed_rgba16_three_nearest` checks both against
`TMEM_IMAGE_FORMAT_RGBA`/`TMEM_PIXEL_SIZE_BITS16` before any addressing math
runs, reporting `TMEM_SAMPLE_STATUS_UNSUPPORTED_FORMAT` for anything else —
never a silent fallback to treating unknown bytes as RGBA16.

**Distinct failure statuses** (an audit repair from this slice's own
verification pass): `TMEM_SAMPLE_STATUS_NO_TILE_BINDING` (missing tile
snapshot) and `TMEM_SAMPLE_STATUS_REVERSED_EXTENT`
(`address_texture_cell_wgsl`'s clamped-axis `high < low` check, mirroring
the CPU oracle's `PointAddressError::ReversedClampExtent`) are separate
codes, not collapsed into one — a missing `TileDescriptor` and a reversed
clamp extent are different failure causes.

**Independent verification against implementation (this slice's own
correction, not a copy of the card's prose):**

- *Per-byte address masking.* The initial WGSL draft computed one shared
  base address for an RGBA16 texel's two bytes and added an offset, which
  disagrees with `tmem/read.rs`'s `read_linear_bytes` exactly at the 0xfff
  TMEM-address wrap boundary (`base` and `base + 1` can straddle it, and the
  odd-row XOR4 exchange must also apply per byte, not once to a shared
  base). Fixed to mask (`& TMEM_ADDRESS_MASK`) and conditionally XOR each of
  the two bytes independently, matching the CPU oracle exactly
  (`tmem_rgba16_linear_base`/`tmem_rgba16_byte_address` in
  `shaders/tmem_sample.wgsl`).
- *Texel-center addressing convention.* This slice's own differential
  fixture initially assumed a texel's own unambiguous sample point was at
  `raw = base_texel*32 + 16` (the implementation card's own stated
  convention). Direct verification against `filter_three_nearest`'s formula
  showed this is wrong: `filter_three_nearest`'s corner weight is 100% at
  `(s0,t0)` only when `sf == tf == 0`, i.e. a texel's own unambiguous
  address is `raw = base_texel*32` — `+16` instead lands exactly halfway
  toward the *next* texel (a genuine four-corner blend). The differential
  fixture and its hand-derived expected colors
  (`production.rs::tests::required_cpu_tmem_oracle_matches_hand_derived_texel_colors`)
  use the corrected convention throughout.
- *LoadBlock row-alignment for a narrow source image.* A 2-texel-wide
  source image cannot produce a whole-8-byte-word row stride
  (`tmem/read.rs`'s `linear_byte_address` requires `line_words * 8` bytes
  per row), so the differential fixture instead uses a 4-texel-wide source
  image (whose natural row width is exactly one TMEM word) and addresses
  only its left 2x2 sub-region as the tile — avoiding an unrepresentable
  fractional `line_words` value the card's literal 2-wide byte string would
  have required.
- *Odd-row XOR4 write/read asymmetry.* `LoadBlock`'s own transfer is always
  one linear run (`tmem/wire.rs`'s `transfer_shape` reports `row_count = 1`
  for every `Block` load, so its own odd-row exchange never fires), but the
  *read* path (`tmem_rgba16_texel_address`) applies the XOR4 exchange to any
  texel whose tile-relative row is odd, under this slice's frozen
  `TmemFirstRowParity::Even`. The differential fixture places its row-1
  source bytes at their post-XOR4 TMEM offsets directly, since the load
  itself never re-orders them.

**CPU-vs-WGSL differential** (`production.rs::tests`, mandatory exit gate):
`required_cpu_tmem_oracle_matches_hand_derived_texel_colors` (CPU-only, no
GPU, runs in every environment) drives this fixture's real production
TMEM-load path through `WgpuBackend::plan_raw_dpc`/`execute_raw_dpc`/
`publish_raw_dpc`, then calls `address_texture_cell`/
`gather_committed_texture_cell`/`filter_three_nearest_committed_cell`
directly at four exact texel-center points (verifying all four fixture
colors independently) plus one hand-derived four-corner blend, with the
arithmetic shown explicitly in the test's own comments, not asserted as a
magic constant.
`required_host_textured_triangle_wgsl_sampling_matches_the_cpu_tmem_oracle`
(`host-gpu-tests`-gated) submits the same committed TMEM through the real
fragment pipeline (`TrianglePipelineRenderer::submit_admitted_triangle`,
reached via a `#[cfg(test)]`-only `WgpuBackend::triangle_pipeline_for_test`
accessor) with a textured triangle whose three vertices carry genuinely
different UVs, and asserts the GPU-observed color at two algebraically
interpolated pixel-center sample points against the same CPU oracle chain.
Both sides are computed independently; the GPU side additionally asserts
every fragment's `tmem_sample_status` readback is `TMEM_SAMPLE_STATUS_OK`.

**Nonclaims.** No RT64 pixel/visual/silicon parity, no guest-visible
publication beyond this backend's own existing `last_triangle_draw()`
diagnostic accessor, no two-cycle combiner/LOD/filter-mode-selection wiring
(this slice hardcodes three-nearest bilerp, matching the existing
`filter_three_nearest_committed_cell` precedent as the most-tested of the
two filter paths), no base-renderer-behavior-matrix row promotion (the new
WGSL addressing/filter functions inherit no independent behavioral
authority of their own — their correctness is established solely by the
CPU-vs-WGSL differential above), and no performance/throughput claim (the
CPU-side upload adapter writes the full committed TMEM byte image once per
commit generation change, a correctness/mechanism choice, not a throughput
claim).

**Repair (independent adversarial review, this session): negative-UV
floor-vs-truncation bug in `fs_main`.** `triangle_pipeline_fragment.wgsl`'s
`fs_main` computed `i32(uv.x)`/`i32(uv.y)` directly on the rasterizer's
interpolated `f32` raw S10.5 coordinate before calling
`sample_committed_rgba16_three_nearest_bound`. `i32(f32)` in WGSL truncates
toward zero, not toward negative infinity — for any negative interpolated
coordinate this silently disagreed with the CPU oracle's own
`div_euclid`/`rem_euclid` floor convention (`tmem/sample.rs`'s
`relative_axis_coordinate`, already ported faithfully in
`tmem_sample.wgsl`'s own same-named function, which assumes its `raw_s`/
`raw_t` integer inputs are already floored). Fixed to
`i32(floor(uv.x))`/`i32(floor(uv.y))`. The prior differential fixture
(`fixture_tile_descriptor`, clamp-addressed, `mask_s = mask_t = 0`) could not
have caught this: under clamp addressing every negative `base_texel`
collapses to column/row 0 on both the correct and buggy path, and the
`filter_three_nearest` formula's two branches are provably identical at that
saturated boundary — a clamp fixture cannot discriminate floor from
truncation for negative input, only wrap/mirror addressing can (see
`cpu_oracle_sample_with_tile`'s doc comment in `production.rs` for the full
argument). The repaired differential adds `fixture_wrap_tile_descriptor`
(mask=1 wrap, non-clamp, non-mirror, over the same committed 2x2 RGBA16
texel layout) and two new discriminating assertion points:
- `required_cpu_tmem_oracle_matches_hand_derived_texel_colors`'s own
  `negative_coordinate` assertion (CPU-only, no GPU): raw point `(s=-1,
  t=0)` under the wrap tile, hand-derived to `[247, 8, 0, 255]` (shown
  arithmetic, not a magic constant) — this is the value truncation-toward-
  zero would have gotten wrong (it would instead address `base_texel=0`,
  producing pure red `[255, 0, 0, 255]`).
- `required_host_negative_uv_floors_toward_negative_infinity_not_truncation`
  (`host-gpu-tests`-gated, new test): geometry chosen so the rasterizer's
  own per-fragment interpolation lands the actual `f32` UV at exactly `-0.5`
  at pixel (0,0) (an exact binary value, no rounding drift), exercising the
  real bug site — the fragment shader's own `f32`->`i32` conversion of an
  *interpolated* value, which the CPU-only test cannot reach since it starts
  from an already-integer coordinate. Asserts the real GPU fragment output
  against the CPU oracle's `[247, 8, 0, 255]` at **exact** equality (zero
  tolerance), not the ±2 channel tolerance the pre-existing positive-UV
  differential test uses — the two candidate answers here differ by up to 8
  in one channel, well outside that tolerance, so exact agreement is the
  correct bar at this specific, deliberately-exact-valued point.

Both new assertion points keep every pre-existing positive-UV assertion
unchanged (`required_cpu_tmem_oracle_matches_hand_derived_texel_colors`'s
four texel-center points and the tile-center blend;
`required_host_textured_triangle_wgsl_sampling_matches_the_cpu_tmem_oracle`'s
two positive interpolated points) — this repair is additive, not a
replacement of prior coverage.

**Reliability status, corrected.** The CPU-only half of this repair's
differential (`required_cpu_tmem_oracle_matches_hand_derived_texel_colors`)
has been run a handful of times in this sandbox session — nowhere near
AGENTS.md's 10-consecutive-clean-run bar for a deterministic fix, and this
document does not claim that bar is met. The GPU-side half
(`required_host_negative_uv_floors_toward_negative_infinity_not_truncation`,
like every other `host-gpu-tests` test in this crate) has not executed at
all in this sandbox — `NoAdapter{AnyNative}`, identical to every pre-existing
host-GPU test here, is unsupported-host evidence, not a passing or skipped
GPU claim, and does not count toward any run total. No "10x" or other
throughput/scale claim is made anywhere in this repair; the only "10"
figure that applies to this work is AGENTS.md's *run-count* reliability bar,
and it remains unmet pending a real Metal adapter.

**Repair: `fs_main` sampled TMEM unconditionally, aborting SHADE-only
triangles with no tile bound.** After the coverage/cvg_dst port, the full
host-GPU lib gate failed at
`production::tests::wgpu_backend_draws_a_real_admitted_triangle_matching_the_combiner_oracle`
— a pre-existing SHADE-passthrough fixture (`combine_d=SHADE`, no
`SetTile`/`SetTileSize` issued, so its `TileBindingParams` is
`unbound()`). `fs_main` called
`sample_committed_rgba16_three_nearest_bound` unconditionally, so this
legitimately-textureless triangle hit
`TMEM_SAMPLE_STATUS_NO_TILE_BINDING` and aborted the draw even though its
combiner formula never reads `TEXEL0`/`TEXEL1`.

**Mechanism, not a one-off guard.** `CombineParams::references_texels_in_first_cycle`
(`combiner.rs`) is a selector-reference predicate over the real
`SetCombine` value: true iff any first-cycle color slot (A/B/C/D) decodes
to `ColorInput::{Texel0,Texel1,Texel0Alpha,Texel1Alpha}` or any first-cycle
alpha slot (A/B/C/D) decodes to `AlphaInput::{Texel0,Texel1}`, via the
crate's own existing typed decoders (`decode_color`/`decode_alpha`,
`second_cycle = true` — matching `run_one_cycle`'s own slot-decode call).
Slots A, B, and D are unconditional references whenever they decode to a
texture selector — `run_one_cycle`'s `(A-B)*C+D` formula adds `D`
unconditionally and reads `A`/`B` unconditionally to form the `(A-B)`
term. Slot C is the one narrow exception: it is `(A-B)*C`'s own
coefficient, and `(A-B)` is exactly zero whenever A and B decode to the
same selector, so C only counts as a reference when A and B decode to
*different* selectors. This is a fact about the formula's own coefficient
structure, not an approximation of it or a guess about which fixtures
happen to exist — every SHADE-passthrough fixture in this crate (including
the one that triggered this repair) sets alpha A=B=`COMBINED` and alpha
C=`TEXEL0` (`alpha_input_c`'s own table, index 1 — distinct from
`alpha_input_abd`'s table, where index 1 is `COMBINED`) purely because C's
coefficient is forced to zero, never because the texel value is wanted;
treating C as an unconditional reference the same as A/B/D would report
"texture referenced" for every one of those fixtures and reproduce this
exact bug.

**Wiring.** The predicate is host-serialized into the existing 16-byte
`FragmentCombineParams` uniform (`targets/triangle_pipeline.rs`'s
`fragment_combine_params_bytes`, no new binding): bytes 0..8 unchanged
(`low`/`high`, the raw `SetCombine` wire split), bytes 8..12 the
predicate's result as a `u32` (`texture_referenced`), bytes 12..16 left
zero (`reserved_zero`, matching `tmem_sample.wgsl`'s own
`TileBindingParams::reserved_zero` convention). `fs_main` only calls
`sample_committed_rgba16_three_nearest_bound` when `texture_referenced !=
0u`; when the gate is closed, `tex_val0`/`tex_val1` are the fixed zero
color and the reported status is `TMEM_SAMPLE_STATUS_OK` — sampling is
conditional on selector references, not unconditional. The gate never
suppresses a real texture reference: both of this crate's real textured
differentials (`production.rs`'s TEXEL0-passthrough fixtures, `color_d =
TEXEL0`, an unconditional D reference) still evaluate to
`texture_referenced = 1` and the real sampler still runs for them, loud
failures included.

## 2026-08-17: Production alpha compare in the triangle fragment path

Wires `AlphaCompare::{None,Threshold}` into the real triangle fragment
path (`shaders/triangle_pipeline_fragment.wgsl`'s `fs_main`) as a real
per-fragment discard, reusing already-landed, already-admitted state —
`OtherMode` (previously discarded at `submit_admitted_triangle`'s own
`let _ = other_mode;`) and `SetBlendColor` (already durably retained in
`RdpState`, already admitted into the `RdpStateCommand` stream — no new
admission mechanism needed).

**BlendColor snapshot, a fourth instance of an existing pattern.**
`raw_dpc::triangle_draw_data::TriangleDrawStateCollector` and
`production.rs`'s `PlanCollector` (its intentional in-crate duplicate) each
gain a `current_blend_color: Option<Color4>` field, tracked on
`RdpStateCommand::SetBlendColor` exactly like the existing
`current_other_mode`/`current_combine` tracking — command-time snapshot at
each triangle's own stream position, never a whole-plan-final value, and
seeded from `WgpuBackend.rdp_state().blend_color()` for cross-submission
carry-in (`PlanCollector::seeded`'s new third parameter). `RetrievedTriangleDraw`
gains `blend_color: Option<Color4>` — `Some` only for `Threshold`-mode
triangles (a `None`-mode triangle never reads a possibly-absent threshold).

**Reserved/Dither: loud retrieval-time traps, not silent coercion.** Both
collectors' triangle-snapshot closures now match on
`other_mode.alpha_compare()`: `Reserved` panics naming the offending
triangle (the first real call site for `require_supported_alpha_compare`'s
documented purpose), `Dither` panics naming the exact reason — no
fragment-callable RT64 PRNG binding exists in this pipeline, and no
frame-count concept exists anywhere in this crate to seed one honestly
(`random.wgsl` stays an unwired `@compute`-only characterization shim,
`random.rs`'s own module doc already says so). A `Threshold`-mode triangle
missing a `blend_color` snapshot at its own stream position is a new named
`MissingTriangleDrawState::NoBlendColor` error, matching the existing
`NoOtherMode`/`NoCombine` hard-error convention exactly.

**WGSL wiring.** `alpha_compare_fragment_fn.wgsl`'s existing
`alpha_compare_fragment_fn` callable (landed characterization-only, port
card `541c5c9e`) is concatenated into the combined fragment source
(`shader_manifest.rs`'s `triangle_pipeline_fragment_wgsl`) after
`tmem_sample.wgsl` and before `triangle_pipeline_fragment.wgsl`'s own
`@fragment` wrapper — reused verbatim, never re-typed inline. New binding
5, `FragmentAlphaCompareParams` (16-byte uniform: `mode`, `threshold_alpha`,
two reserved words), matching this crate's existing pad-to-16-byte-multiple
convention. `fs_main` calls the callable after `run_one_cycle` produces
`result.combiner_color` (the RDP alpha-compare stage's real input is the
combiner's output alpha, not the raw vertex/texel alpha) and discards
before `output` construction — `noise_byte` and `copy_cycle_rgba16` are
always `0u` literals at this slice's call site (no wired PRNG, no
copy-cycle triangle path yet).

**`submit_admitted_triangle` stops discarding `other_mode`.** The method
now derives `FragmentAlphaCompareParams::mode` from
`other_mode.alpha_compare()` (guaranteed `None`/`Threshold` by the
retrieval-time rejection upstream; this call site itself asserts-unreachable
on `Reserved`/`Dither` for defense-in-depth) and takes a new `blend_color:
Option<Color4>` parameter, threaded through `TriangleFixture`'s two new
fields (`alpha_compare_mode`, `blend_color`) into the per-draw uniform
buffer.

**Host GPU evidence.** A new required host-GPU differential
(`targets/triangle_pipeline/tests.rs`) submits two single-triangle draws —
one `None`-mode at a low alpha (must still write, proving `None` never
gates on alpha at all) and one `Threshold`-mode with the same low alpha
against a provably higher `blend_color` threshold (must be discarded,
leaving the `LoadOp::Clear` background and un-written depth untouched) —
mirroring the existing depth-differential precedent's "second triangle
drawn... performing a real GPU discard" shape.

**Nonclaims.** No blend equation, no coverage accumulation, no depth
clip/test wiring — unchanged, still out of scope. No `Dither` support — a
named, tested, loud trap. No copy-cycle (`CycleType::Copy`) alpha-compare
special case — this pipeline has no copy-cycle triangle path to hang it
off of yet. No claim that discarded-fragment behavior matches real RDP
silicon pixel-for-pixel — WGSL `discard` is the standard GPU mechanism for
"this fragment contributes nothing," matching alpha-test semantics RT64
itself implements via `clip()`/discard in `RasterPS.hlsl`, but this slice
does not cite RT64's exact discard line.

## 2026-08-17: Production coverage node 1 — `cvg_dst`/coverage-alpha in the triangle fragment path

Wires the existing, already-GPU-differentially-validated `coverage_fragment_fn`
("Coverage, fragment-callable WGSL seam" above) into the real triangle
fragment path (`shaders/triangle_pipeline_fragment.wgsl`'s `fs_main`), the
`Full`/no-image-read subset only. This is node 1 of a three-node production
coverage plan (`.claude-handoffs/production-coverage-node1-card-v2.md`);
node 2 (`memory: Coverage` read/write-back — the framebuffer-read problem
both this file's own coverage sections and the alpha-compare section above
name and defer) and node 3 (`CoverageMask`/geometric sub-pixel coverage
machinery) remain separate, unresolved, out of scope here.

**Two authorities this section keeps explicitly distinct, by design.**

> `CoverageDestination::Full => Coverage::FULL` (count 8) is ported from
> `fn64-render-reference/src/raster/coverage.rs:89` under Programming Manual
> authority — the RDP's real 8-sample full-coverage value. RT64's own
> `CVG_DST_FULL` arm (`RasterPS.hlsl:249-250`, pin
> `f0728a2520d5aa735886240de3fee75cc805f6d6`) writes `7.0f/cvgRange`, one
> coverage unit below this port's value — a known, deliberate RT64
> approximation this port does not reproduce. This slice is
> **hardware-faithful, with RT64 cited only as structural precedent** (the
> alpha-channel-as-coverage-storage *resource choice*, and the fact that RT64
> independently also has no geometric 8-sample source to draw from for this
> mode) — **never as arithmetic authority, and never as a byte-for-byte
> match**. This matches this repo's standing policy of citing RT64 for
> structural/ordering inspiration only, per `RT64-GAP-REGISTER.md`'s existing
> convention.

RT64's real per-fragment order, re-read directly from `RasterPS.hlsl`: alpha
compare (`G_AC_DITHER`/`G_AC_THRESHOLD` discard, lines 204-213) runs first,
its `resultCvg` coverage estimate (lines 215-224, with its own
coverage-threshold discard) is computed second, and the `cvgDst`/
output-alpha composition switch (lines 241-255) runs last, once a fragment
has survived both. This slice's own `fs_main` sequencing — call
`coverage_fragment_fn` *after* `alpha_compare_fragment_fn`'s discard — is
consistent with that real RT64 order, not a divergence from it:

> Coverage/`alpha_coverage_select` composition does not depend on
> alpha-compare's outcome for a fragment that survives it — it only reads the
> already-computed combiner alpha. Sequencing coverage **after**
> `alpha_compare_fragment_fn`'s discard (i.e., call `coverage_fragment_fn`
> last in `fs_main`) avoids computing coverage arithmetic for a fragment about
> to be discarded — a dead-work/performance ordering choice, not a
> correctness-driven one, and this repo's own data-flow argument is
> sufficient on its own to justify the placement independent of any RT64
> citation. RT64's own real order (alpha compare first, coverage estimate
> second, `cvgDst` composition last) is cited here only as contextual
> precedent: this repo's placement (coverage after alpha-compare's discard)
> happens to be consistent with that real order, not a legitimate divergence
> from it.

**WGSL wiring.** `coverage_fragment_fn.wgsl`'s existing `Full`/`Save`
branches (the only independently GPU-differentially-validated ones —
`Clamp`/`Wrap` stay transcribed-but-unvalidated per that file's own boundary
doc) are reused verbatim, concatenated into the combined fragment source
(`shader_manifest.rs`'s `triangle_pipeline_fragment_wgsl`) after
`alpha_compare_fragment_fn.wgsl` and before `triangle_pipeline_fragment.wgsl`'s
own `@fragment` wrapper. New binding 6, `FragmentCoverageParams` (32-byte
uniform: `coverage_destination`, `image_read_enabled`, `force_blend`,
`antialias_enabled`, `coverage_times_alpha`, `alpha_coverage_select`, two
reserved words), the six already-decoded `OtherMode` coverage fields
verbatim, matching this crate's pad-to-16-byte-multiple convention. `fs_main`
calls the callable with `pixel_count = COVERAGE_FULL` (8u) and
`memory_count = 0u` always — this slice's rasterizer produces only
whole-pixel coverage (no `CoverageMask` sub-pixel geometry, node 3) and no
framebuffer-read mechanism exists to supply a real `memory` value (node 2) —
and, when `alpha_coverage_select` is set, overwrites `output.color.a` with
the callable's `adjusted_alpha`; otherwise `output.color.a` stays the
combiner's own alpha unmodified, exactly as before this slice.

**Loud rejection, not silent substitution.** `CoverageDestination::Save`
always, and `Clamp`/`Wrap` whenever `image_read_enabled` is set, panic in
`targets/triangle_pipeline.rs`'s `fragment_coverage_params_bytes` before GPU
submission — this pipeline has no framebuffer-read mechanism to supply the
real `memory: Coverage` value those paths need, so a value can never be
honestly synthesized. This is the pipeline's own submission-boundary
enforcement point (mirroring the `AlphaCompareMode::Reserved`/`Dither`
retrieval-time-panic precedent's shape), not a retrieval-time collector
change — `raw_dpc`/`production.rs` are out of this node's scope and are
untouched. With `image_read_enabled` clear, `Clamp`/`Wrap` degenerate to
`pixel` (== `Coverage::FULL`, this slice's only supplied `pixel` value) and
pass through unrejected.

**`TriangleFixture`/`submit_admitted_triangle`.** Six new `TriangleFixture`
fields (`coverage_destination`, `image_read_enabled`, `force_blend`,
`antialias_enabled`, `coverage_times_alpha`, `alpha_coverage_select`),
populated in `submit_admitted_triangle` from `other_mode`'s existing
decoded accessors verbatim — no new admission mechanism, matching this
node's "reuse already-landed, already-admitted state" scope exactly the way
the alpha-compare production section above reused `OtherMode`/`SetBlendColor`.

**Host GPU evidence.** A new required host-GPU differential
(`targets/triangle_pipeline/tests.rs`,
`required_host_ordinary_vs_alpha_coverage_select_writes_distinct_alpha`)
submits two single-triangle draws at the same low combiner alpha (0.2, byte
51) — one with `alpha_coverage_select` clear (must write alpha 51
unmodified) and one with it set against `Full`/`coverage_times_alpha=false`
(must write alpha 255, `coverage_alpha_fn(8)`) — mirroring the alpha-compare
precedent's "real GPU-observed... not the CPU oracle called in isolation"
shape. The required differential test has been added; it has not been run
on a certified Metal adapter in this sandbox (`host-gpu-tests` tests are
`NoAdapter`-unsupported here, identical to every other host-GPU test in this
crate), so native evidence and the 10-consecutive-clean-run bar remain
unrecorded pending the final Metal gate. No passing or skipped GPU claim is
made here.

**Nonclaims.** No `memory: Coverage` source of any kind (node 2, the
framebuffer-read problem, named and deferred here exactly as the
characterization-only coverage section above already deferred it — this
slice does not narrow that frontier, it only routes around it by staying in
the `Full`/no-image-read subset). No `CoverageMask`/geometric sub-pixel
coverage (node 3). No `Clamp`/`Wrap` GPU validation — inherited unchanged
from `coverage_fragment_fn.wgsl`'s own boundary; this slice adds no new GPU
evidence for either mode, only a host-side rejection when they would need
`memory`. No RT64 arithmetic-parity claim for `Coverage::FULL` (see the
7-vs-8 authority statement above) and no RT64 textual-order-replication claim
(see the ordering statement above) — both are structural/contextual
citations only. No blend-equation wiring — `blend: None` still holds at both
`ColorTargetState` sites, so no fixed-function GPU blend consumes
`output.color.a` today; a future blend-wiring slice must be told `.a` may
already carry `alpha_coverage_select`-derived coverage for such triangles,
not raw combiner alpha (the same forward-compatibility note the alpha-compare
section above's coverage precedent already flagged). No RT64 pixel/visual
parity claim, no performance claim.

## 2026-08-17: Production combiner state capture — EnvColor/PrimColor command-time seam

Slice A, sub-step 5a (corrected per independent review): threads
`SetEnvColor`/`SetPrimColor` through the same command-time-snapshot seam
`blend_color` already uses, so a triangle's real `ENVIRONMENT`/`PRIMITIVE`
combiner inputs are typed, captured, and tested — but not yet reaching WGSL.
This slice is `production.rs`/`raw_dpc/triangle_draw_data.rs`-only: no
uniform, no `fs_main` change, no binding slot. It deliberately does not
change any rendered pixel; it only builds the typed seam the WGSL half
(serialized after Coverage Node 1, per the corrected slice plan) will read
from.

**EnvColor/PrimColor snapshot, a fifth and sixth instance of an existing
pattern.** `raw_dpc::triangle_draw_data::TriangleDrawStateCollector` and
`production.rs`'s `PlanCollector` each gain `current_env_color:
Option<Color4>` and `current_prim_color: Option<PrimColor>` fields, tracked
on `RdpStateCommand::SetEnvColor`/`SetPrimColor` exactly like the existing
`current_other_mode`/`current_combine`/`current_blend_color` tracking —
command-time snapshot at each triangle's own stream position, never a
whole-plan-final value. `PlanCollector::seeded` gains two more parameters
(`env_color: Option<Color4>`, `prim_color: Option<PrimColor>`), seeded from
`WgpuBackend.rdp_state().env_color()`/`.prim_color()` in `execute_raw_dpc`
exactly as `other_mode`/`combine`/`blend_color` already are — durable
cross-submission carry-in, not a synthetic plan-stream entry. Both
collectors are mirrored in the same slice (independent review's Correction
B): `TriangleDrawStateCollector` has no live production caller today, but
the module's own doc already states it deliberately duplicates
`PlanCollector`'s behavior, and leaving one collector behind would silently
break that stated invariant.

**Unconditional, unlike `blend_color`.** `blend_color` is `Some` only for
`AlphaCompare::Threshold` triangles, gated by a retrieval-time match on
`other_mode.alpha_compare()`. `env_color`/`prim_color` have no such gate:
every triangle's one-cycle/two-cycle combiner arithmetic can reference
`ENVIRONMENT`/`PRIMITIVE` regardless of alpha-compare mode (`combiner.rs`'s
`ColorInput`/`AlphaInput` selector tables), so `RetrievedTriangleDraw` simply
carries whatever `current_env_color`/`current_prim_color` held at that
triangle's own stream position — `None` when neither the plan nor the
durable seed ever set one, never a hard-error `MissingTriangleDrawState`
variant the way absent `OtherMode`/`CombineParams`/`Threshold`-mode
`blend_color` are.

**Wire conversion.** `RdpStateCommand::SetEnvColor { color: NeutralColor4,
.. }` converts via the existing `Color4::from_wire(color.value)`, identical
to `SetBlendColor`'s own conversion. `RdpStateCommand::SetPrimColor { color:
NeutralPrimColor { lod_frac, lod_min, color }, .. }` converts via the
existing `PrimColor::from_wire(w0, w1)`, reconstructing `w0` as `lod_frac as
u32 | (lod_min as u32) << 8` and passing `color` as `w1` directly —
`NeutralPrimColor.lod_min` is already `PrimLod::from_wire`'s masked 5-bit
value (`raw_dpc::production_adapter`'s own `neutral_prim_color` produces it
that way), so shifting it back into wire bit position 8:12 and letting
`PrimColor::from_wire`'s own `& 0x1f` mask apply is idempotent, not a
re-derivation of new masking logic.

**Characterization tests.** Both collectors gain an
`a_and_b`/`snapshots_distinct`-shaped test proving the exact seam this slice
exists for: `SetEnvColor(A)`/`SetPrimColor(A)` → triangle A →
`SetEnvColor(B)`/`SetPrimColor(B)` → triangle B collects two distinct
snapshots, with an explicit `assert_ne!` proving triangle A is not
retroactively affected by state that arrives after it in plan order — the
same regression shape the crate's existing `OtherMode`/`CombineParams`/
`BlendColor` tests already guard against. `PlanCollector` additionally gains
a durable-seed test (`plan_collector_seeded_env_and_prim_color_resolve_a_
triangle_with_no_in_plan_state`) proving a triangle with no in-plan
`SetEnvColor`/`SetPrimColor` of its own still resolves both fields from
`PlanCollector::seeded`'s durable value, mirroring the existing
`other_mode`/`combine` durable-seed test. `TriangleDrawStateCollector`
additionally gains a no-gate test proving a triangle before any
`SetEnvColor`/`SetPrimColor` resolves cleanly with `None`, not an error —
the unconditional/gated distinction from `blend_color` stated above, made
executable.

**Nonclaims.** No WGSL, no `shader_manifest.rs`, no `targets/
triangle_pipeline.rs`, no new uniform buffer or binding slot — this slice's
own explicit non-scope, matching the independently-reviewed corrected card.
No rendered-pixel claim: `combiner_inputs_from_fragment_registers` still has
zero production callers after this slice: it wires the *retrieval* side and
does not yet call into it. `TriangleDrawStateCollector`'s general
non-production-caller status is unchanged by this slice — mirroring it here
is about keeping its own documented invariant honest for the next reader,
not about giving it a new caller.

## 2026-08-16: RawTriangle sealed-plan admission

Admits `RawTriangle` into the sealed `ExactRawDpcPlan`/
`production_adapter.rs` production mechanism via a real
`RawDpcSemanticCommandRef::Triangle`/`RdpTriangleCommand`/`push_triangle`
admission path, not a bypass reading `DecodedRawDpc::commands()` directly
(an earlier draft's rejected "escape hatch").

**New types.** `render_ir.rs` gains a third
`RawDpcSemanticCommandRef` variant, `Triangle(&RdpTriangleCommand)`
(alongside `TmemLoad`/`State`; the enum stays `#[non_exhaustive]`), and
`RdpTriangleCommand { location, raw_words, vertices: [NeutralTriangleVertex;
3] }` — deliberately no before/after identity fields: `ExactRawDpcPlanWriter::
finish()`'s access-ordering contract only compares `self.accesses` against
the journal's own access list and has zero coupling to `self.commands`, so a
triangle command pushing zero `ResourceAccess` entries trivially satisfies
it. `NeutralTriangleVertex` mirrors T1's private `TriangleVertex` shape
field-for-field (`fn64-render-wgpu` depends on `fn64-render`, not the
reverse, so the wgpu-crate type cannot be named directly).

**Stream-position `OtherMode` tracking.** `production_adapter.rs` decodes
each triangle against the `OtherMode` current AT ITS OWN STREAM POSITION,
never a later or final value: a local `Option<OtherMode>` walked forward
through the push loop, seeded from `decoded.base_state.other_mode()` (the
durable RDP state a caller held before this decode) and updated on every
in-plan `SetOtherMode`. A triangle walked with no `OtherMode` established at
all — neither carried in from `base_state` nor set earlier in this plan —
is a loud, typed `TriangleBeforeAnyOtherMode` rejection (new
`PushDecodedRawDpcError` variant), never a silent `(0, 0)` default: this
value feeds `texture_perspective()` directly into `decode_triangle_vertices`
and changes decoded geometry, so a fabricated default would be a real
correctness risk.

**Variable-width slicer.** A new `triangle_command_raw_words`, keyed off
`fn64_render::raw_rdp_command_width`, since triangle opcodes `0x08..=0x0f`
span 32..176 bytes depending on their shade/texture/depth coefficient flags
— `tmem_command_raw_words`'s fixed-2-word assumption cannot be reused.

**New `raw_dpc/triangle_draw_data.rs`** (sealed-plan composition into the
pipeline's input shapes; does not itself reach `WgpuBackend`/
`RenderBackend`): a plain `NeutralTriangleVertex` -> `RasterVertex` field
adapter, and `TriangleDrawStateCollector`, an `ExactRawDpcPlanVisitor` (same
shape as `production.rs`'s own `PlanCollector`) that snapshots the
`SetOtherMode`/`SetCombine` state current at each triangle's OWN stream
position onto that triangle — per-triangle `RetrievedTriangleDraw{vertices,
other_mode, combine_params}`, never a single whole-plan-final value (a bug
caught by independent review mid-implementation: the first draft collapsed
all triangles to one shared final state, which would retroactively apply a
later state change to an earlier draw — fixed to per-triangle snapshots with
the loud, indexed `MissingTriangleDrawState{triangle_index}` naming exactly
which triangle lacks state, before any decode happens against it).

**`targets/triangle_pipeline.rs`** gains `TrianglePipelineRenderer::
submit_admitted_triangle` — a thin function accepting real retrieved
vertices/`OtherMode`/`CombineParams` plus caller-supplied
`raster_params`/`extent` and forwarding into the existing `submit_triangle`
entry point. `other_mode` is accepted but not yet consumed: this slice's
vertex shader hardcodes `is_rect=false`/no Z-override, matching the
GPU-pipeline card's own fixed-fixture scope. (Superseded by the
TextureRectangle/Flip admission and batching described in "Raw RDP
TextureRectangle decode and six-vertex conversion" above: `is_rect` is no
longer hardcoded, and `draw_admitted_triangles` now batches every draw in
one `execute_raw_dpc` call into a single `submit_triangles` submission.)

**Out of scope, held (at this slice).** No multi-triangle batching, no texture
sampling/blend/alpha-compare/coverage-write, no two-cycle combiner branch,
no real draw-command-stream/frame assembly. Does not touch `combiner.rs`,
`state.rs`, `fn64-render-reference`, any GPU/WGSL shader file, or
`raster_vs.rs`/`raster_vs.wgsl`.

**Evidence.** 10 consecutive clean `cargo test -p fn64-render-wgpu --lib`
runs (925/925 passed) and 10 consecutive clean runs with `--features
host-gpu-tests` (932/932 passed, real Metal execution evidence for the
end-to-end host-GPU test `required_host_draws_a_real_admitted_triangle_
matching_the_combiner_oracle`, which reaches its sealed plan through a real
`RawDpcCoordinator` — `authority.into_coordinator(())` — not a bare-authority
route, matching how `WgpuBackend` reaches its own plan in production).
`cargo clippy -p fn64-render-wgpu --no-deps --all-targets` clean on both
feature sets. One bounded independent adversarial review ran against the
final diff and reported zero findings at or above its confidence threshold,
after two earlier review passes during implementation caught and fixed real
bugs (the decode-time ordering/silent-default gap, and the retrieval-time
whole-plan-final-value bug, both described above).

## 2026-08-17: Production blend wiring slice 1 — admitted-subset blend-cycle compositing

Wires the already-landed `crate::blend` CPU characterization ("Blender: full
selector/cycle semantics (port-card §1)" above — `blend.rs`,
`shared/rt64_blender.h:68-81,366-504`, pin
`5473732a822a4423b5696e7cb18fecc425a59875`) into the real triangle fragment
path, for the **admitted subset only**: Copy/Fill bypass, and OneCycle/
TwoCycle whose active cycles' `P`/`M`/`B` selectors never name
`BlendColorInput::Framebuffer`/`BlendBInput::FramebufferAlpha`. A triangle
whose real `OtherMode` needs a framebuffer sample is rejected before GPU
submission with a named `WgpuRawDpcExecutionError::BlendRequiresFramebuffer`
error — never silently rendered opaque, never given a manufactured
destination sample. The memory-dependent sub-case (native dual-source
blending under `wgpu::Features::DUAL_SOURCE_BLENDING`, or
`manual_blend_composite`'s integer-arithmetic fallback with a
destination-color read) is explicitly deferred to a follow-up slice — no
dual-source/manual-fallback wiring exists yet in this pipeline.

**Command-time state.** `G_SETBLENDCOLOR` is now tracked unconditionally at
every triangle's command-time position (`PlanCollector.current_blend_color`/
`RetrievedTriangleDraw.blend_color`), not only under
`AlphaCompare::Threshold` as before — both alpha-compare's threshold gate
and the new blend-cycle wiring's `BlendColorInput::Blend` selector read the
same real register value; each consumer validates presence only when its
own selector/mode requires it (alpha-compare's existing
`MissingTriangleDrawState::NoBlendColor` panic path is unchanged and still
fires under `Threshold` with no prior `SetBlendColor`). `G_SETFOGCOLOR` is
threaded the same way as a new field (`current_fog_color`/`fog_color`),
mirroring `env_color`/`prim_color`'s existing unconditional
command-time-snapshot pattern exactly — no second register-tracking shape
invented.

**Typed host-side parameter object.** `targets/triangle_pipeline.rs` adds
`ResolvedFragmentBlendParams` (cycle count, both cycles' four selectors as
`crate::blend`'s own closed enums, `blend_color`/`fog_color`) and
`fragment_blend_params_bytes`, an 80-byte fixed serialization matching the
new `FragmentBlendParams` WGSL uniform exactly. Built once per admitted
triangle in `production.rs`'s `draw_admitted_triangles`, from
`BlendModeState::cycle_count()`/`cycle()` constructed directly from the
triangle's own decoded `OtherMode` plus its resolved `blend_color`/
`fog_color` — no parallel `OtherMode` decode. Construction only proceeds
after `ResolvedBlendCycle::requires_framebuffer_sample()` has rejected every
active cycle that needs a memory sample; a `cycle_count == 0`
(`ResolvedFragmentBlendParams::NO_OP`) value is the same byte-identical
Copy/Fill bypass every pre-existing fixture already reused unmodified.

**WGSL wiring.** `shaders/blend_fragment_fn.wgsl` (new file, concatenated by
`shader_manifest.rs`'s `triangle_pipeline_fragment_wgsl` after
`coverage_fragment_fn.wgsl` and before the `@fragment` wrapper) is an
ordinary-WGSL re-expression of `blend_fragment`'s admitted-subset control
flow: the sequential 1-or-2-cycle composite, the no-`FORCE_BL` last-cycle
bypass gated on the same `coverage_fragment_fn`-produced `blend_enabled`
value (no second gate), the zero-factor divisor collapse, and the general
`(P*A + M*B) / (A+B)` divide — all restricted to selectors that never read a
destination color; the `Framebuffer`/`FramebufferAlpha` arms are
structurally unreachable (host-rejected before this shader ever runs) and
fall back to a defensive, never-actually-taken value rather than indexing
out of bounds. New binding 8, `FragmentBlendParams`, added at every
bind-group-layout/bind-group site (prewarm and per-draw).

**Manifest hash.** `shader_manifest.rs`'s frozen
`TRIANGLE_PIPELINE_FRAGMENT_SOURCE_SHA256`/`_FIXTURE_SHA256`/`_SOURCE_BYTES`
constants are recomputed and refrozen for the new six-file combined source
(previously five), via this file's existing `#[ignore]`
freeze-print-and-paste convention — the hash-gated test that would have
caught a stale freeze (`triangle_pipeline_fragment_manifest_retains_zero_
row_promotion_and_matches_its_own_source`) is green against the refrozen
values.

**Out of scope, held (this slice).** The memory-dependent sub-case (§3c of
the originating card): no storage-texture read-back binding, no second
render pass, no native dual-source `@blend_src` output, no
`manual_blend_composite` wiring — naming
`WgpuRawDpcExecutionError::BlendRequiresFramebuffer` is a rejection, not an
implementation of that path. No parity claim against RT64 or real hardware.
No presentation/full-ROM/performance claim.

**Evidence.** `cargo test -p fn64-render-wgpu --lib` clean (1091/1091
passed, 3 ignored freeze-print tests) after refreezing the manifest hash;
`cargo clippy -p fn64-render-wgpu --lib --tests --no-deps` clean (default and
`--features host-gpu-tests`); `rustfmt --check` clean on every file this
slice touches. All three `#[cfg(feature = "host-gpu-tests")]` tests added by
this slice have now run on this crate's certified M2.2 Metal adapter (Apple
M5 Pro): `draw_admitted_triangles_rejects_a_blend_cycle_that_reads_the_framebuffer`
(`production.rs`, 1/1 passed); card §3f's WGSL-vs-Rust differential
(`blend::tests::host_gpu_tests::required_host_fragment_fn_matches_cpu_oracle_across_frozen_fixtures`,
`blend/tests.rs` — a compute shim wrapping `blend_fragment_cycle_fn`
unmodified, dispatched over 11 frozen fixtures spanning Copy/Fill bypass,
both zero-factor collapses, the general divide, both directions of the
two-cycle sequential handoff, the documented fog-then-pass pattern, and the
no-`FORCE_BL` last-cycle bypass, each fixture's expected value computed by
calling `blend_fragment(..., memory: None, ...)` itself — the existing Rust
oracle, not a second formula, 11 fixtures x 4 channels passed); and card
§3f's physical-Metal general-divide draw fixture
(`targets::triangle_pipeline::tests::host_gpu_tests::required_host_general_divide_blend_draw_matches_the_rust_oracle_with_no_memory`,
`targets/triangle_pipeline/tests.rs` — a real `TriangleFixture` draw with a
`Blend`/`Fog`/`Fog`/`One` OneCycle selector combination chosen so neither
zero-factor collapse applies, `force_blend: true` so the last-cycle bypass
does not apply either, real-GPU readback compared against
`blend_fragment(..., memory: None, ...)`, 1/1 passed). 3/3 named tests
passed one run each. The first pre-repair run on this hardware failed the
11-fixture differential: `blend/tests.rs`'s `case_data` packer wrote
`RawCase`'s `src`/`blend_color`/`fog_color` `vec4<f32>` fields at tightly
packed Rust `repr(C)` offsets instead of the WGSL host-shareable layout's
required 16-byte-aligned offsets, so WGSL read `src`/`blend_color`/
`fog_color` starting 8/20/20 bytes earlier than Rust had written them. The
repair moved those three fields to their WGSL-correct offsets 48, 80, and 96
(stride 112 total, `WGSL_CASE_STRIDE`), after which the same run passed
3/3 clean. Final-source reliability then ran those same three exact tests in
10 consecutive fresh-process ordinals on the same Apple M5 Pro: 30/30 test
processes passed with no retry or source change. This satisfies AGENTS.md's
deterministic 10x bar for this bounded slice; it is not a broader parity,
performance, or full-ROM claim.

## RDP LOD-fraction / tile-index selection (`computeLOD`, `texture_lod`)

`texture_lod` is a characterization-first literal port of the permitted MIT
RT64 Rust-port source pinned at commit
`5473732a822a4423b5696e7cb18fecc425a59875` (`docs/RT64-PORT-AUTHORITY.md`),
`src/shaders/TextureSampler.hlsli:27-72` (`computeLOD`, whole-file SHA-256
`927ca2d1c748862f683b3d6115bc97a56cc2ff343474a641046a64788fecef3a`). Like
`texture_gen`/`math_hlsli`/`color_hlsli`, `fn64-render-wgpu` has no crate
dependency on `fn64-render-reference`, so this is a self-contained literal
re-expression citing RT64's source directly. `TextureSampler.hlsli`'s other
functions (`clampWrapMirrorSample`, `sampleTextureNative`,
`sampleTextureLevel`, `sampleTexture`) all bind `Texture2D`/`GPUTile`/
`RDPTile`/TMEM resources and are out of scope for this literal-CPU-port
slice; only `computeLOD`, the file's one pure scalar/vector formula, is
ported here.

**Reused accessors, no `state.rs` edit.** RT64's `otherMode.textLOD()`/
`textDetail()` map onto this crate's existing `OtherMode::texture_lod() ->
bool` and `OtherMode::texture_detail() -> u8` (`state.rs`), already landed
and unit-tested before this slice. `texture_lod()` reads other-mode high bit
16 (`G_MDSFT_TEXTLOD`) as a bare bit test, which is already
`textLOD() == G_TL_LOD` collapsed to a `bool` (`G_TL_LOD = 1 << 16`,
`G_TL_TILE = 0 << 16` are the field's only two values), so this port's
`uses_lod` is `other_mode.texture_lod()` directly. `texture_detail()` reads
high bits 17:18 (`G_MDSFT_TEXTDETAIL`) already right-shifted to a plain
`0..3` ordinal, so `lodSharpen` is `texture_detail() & 1 != 0`
(`G_TD_SHARPEN = 1 << 17`, ordinal bit 0) and `lodDetail` is
`texture_detail() & 2 != 0` (`G_TD_DETAIL = 2 << 17`, ordinal bit 1). This
slice adds no new `OtherMode` accessors and makes no edit to `state.rs`.

**`inout`/`out` params as an owned return value.** `tileIndex0`/`tileIndex1`
are HLSL `inout` parameters and `lodFraction` is an `out` parameter. This
port follows this crate's established conversion of HLSL out-params to owned
return values: [`LodTileIndices`] carries the caller-supplied
`tile_index0`/`tile_index1` *input*, and [`LodSelection`] is the owned
result. This input/output split matters because RT64's `!usesLOD` branch
writes only `lodFraction = 1.0f` and leaves `tileIndex0`/`tileIndex1`
untouched -- a naive port that defaults them to `0` instead of threading the
caller's prior value through would silently change behavior. `compute_lod`
preserves the pass-through literally.

**Arithmetic, in RT64's exact operation order.** `maxDdUV = max(|ddxUV|,
|ddyUV|)` per component; `maxDst = max(maxDdUV.x, maxDdUV.y) * resLodScale`;
when `lodDetail || lodSharpen`, `maxDst` is further maxed against
`primLOD.y` (and *only* then -- a dedicated fixture proves `primLOD.y` is
inert when neither flag is set, even when it would otherwise dominate).
`tileBase = floor(log2(maxDst))`, `lodFraction = maxDst / 2^max(tileBase, 0)
- 1.0`. Sharpen then may override `lodFraction` to `maxDst - 1.0` (strictly
`maxDst < 1.0`); detail then may replace a still-negative `lodFraction` with
`maxDst` and always advances `tileBase` by one; otherwise (`!lodDetail`) a
`tileBase >= tileMax` clamps `lodFraction` to `1.0`. A final gate --
`lodDetail || lodSharpen` clamps `tileBase` to `>= 0`, else `lodFraction` is
clamped to `>= 0` -- determines which of the two values receives the
non-negative floor. Both tile indices are `clamp(tileBase [+1], 0, tileMax)`.

**No defensive guards the source doesn't have.** `log2(x)` for `x <= 0` is
undefined in the HLSL source; this port lets plain IEEE-754 `f32::log2`
propagate (`0.0 -> -inf`, negative -> `NaN`, `inf -> inf`) rather than
special-casing it, matching `depth_strict_less`'s precedent.
`int(rdpTileCount) - 1` has no guard against `rdpTileCount == 0`, so
`tileMax` can go negative -- preserved rather than clamped away. Two extra
consequences follow directly from admitting these unguarded values, and this
port makes an explicit, literal choice for each:

- HLSL's `clamp(x, lo, hi)` is `min(max(x, lo), hi)` and never panics when
  `lo > hi` (it resolves to `hi`). Rust's `i32::clamp` instead panics on
  `lo > hi`, which `rdpTileCount == 0` (`tileMax == -1`) reaches directly.
  This port defines a small `hlsl_clamp_i32` helper reproducing the
  `min(max(...))` composition instead of calling `i32::clamp`.
- HLSL 32-bit `int` addition has no trap-on-overflow semantics. The extreme-
  `maxDst` fixtures this slice's sweep requires (`1e30`, `+inf`) drive
  `tileBase` to `i32::MAX`/`i32::MIN` via the `as i32` float-to-int cast, and
  `tileBase + 1` would then panic in a debug build under Rust's default
  overflow checking. This port uses `i32::wrapping_add` at both increment
  sites so the result is the same 32-bit two's-complement wrap in every
  build profile, not a build-profile-dependent panic.

**Tests.** `usesLOD == false` pass-through across several arbitrary
(including zero and negative) prior tile-index pairs, always producing
`lodFraction == 1.0`; the full sharpen/detail 2x2 cross product exercised at
representative `maxDst` values (a moderate in-range case, at-or-past-`tileMax`
clamping, the sharpen sub-`1.0` override boundary and its non-triggering
sibling just at `1.0`, the detail negative-fraction-replacement path and its
non-triggering sibling); `primLOD.y` proven inert when neither flag is set
and dominant when either is; `rdpTileCount == 0` proving the unguarded
negative-`tileMax` path does not panic; `ddxUV`/`ddyUV` sign and per-axis
dominance combinations; `maxDst` at `0.0`, exactly `1.0`, an exact power of
two, `1e30`, and `+inf` (the last two exercising the `wrapping_add` and
`hlsl_clamp_i32` decisions directly); and a mutation sweep pinning each `<`/
`<=`/`>=` boundary direction and each `||`/`&&` logical gate the formula
depends on, so a flipped comparison or connective fails a fixture rather
than passing silently.

**Scope.** No GPU, WGSL, texture sampling, TMEM wiring, mip-level selection
at a real draw call, combiner integration (`combiner_inputs_from_fragment_
registers` does not gain a `lod_fraction` producer here -- `combiner.rs`'s
`ColorInput::LodFraction`/`PrimLodFrac` selectors remain caller-supplied
typed fields), triangle/rasterizer integration, or RT64 visual/pixel/silicon
parity or performance claim. This module does not modify `state.rs` or any
other existing file besides `lib.rs`'s module registration. No WGSL
companion: `computeLOD` is not called from any of this crate's existing WGSL
fixtures.
